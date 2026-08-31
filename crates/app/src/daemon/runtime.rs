//! The daemon's shared state: everything the IPC handler, the tray, the
//! scheduler and the executor all need to see the same view of.
//!
//! ```text
//!             ┌──────────── Runtime (Arc) ────────────┐
//!   IPC ─────▶│  store (config + vault)               │◀──── tray
//!   tray ────▶│  persisted state (history, summaries) │
//!   engine ──▶│  live runs, recent events, health     │◀──── executor
//!             │  broadcast::Sender<StreamItem>        │
//!             └───────────────────────────────────────┘
//! ```
//!
//! ## Locking discipline
//!
//! There are two kinds of lock here and the distinction is load-bearing.
//!
//! * [`tokio::sync::Mutex`] guards the two things whose critical sections
//!   genuinely contain I/O: the [`Store`] (it writes `config.json` and the
//!   vault) and the [`PersistedState`] (shared with the engine, which the
//!   engine locks around its own writes).
//! * [`std::sync::Mutex`] guards everything else, and **no `std` guard is
//!   ever held across an `await`**. Every accessor below either takes the
//!   guard and returns an owned clone, or does its whole job inside one
//!   synchronous block. That is why `Runtime` is `Send + Sync` and why the
//!   handler's futures are `Send` without any care at the call sites.
//!
//! Poisoned `std` mutexes are recovered from rather than propagated: a
//! panicking IPC request must not take the backup daemon's health tracking
//! down with it. See [`recover`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use superbackup_core::config::Store;
use superbackup_core::engine::{SchedulerHandle, SchedulerStatus};
use superbackup_core::ipc::{StreamItem, Topic};
use superbackup_core::kopia::KopiaBinary;
use superbackup_core::model::{Config, Destination, Job, Settings};
use superbackup_core::paths::Paths;
use superbackup_core::platform::{self, Notifier, ServiceStatus};
use superbackup_core::state::{
    Event, Health, JobRun, PersistedState, RunStatus, Severity, StatusSnapshot,
};
use tokio::sync::broadcast;
use uuid::Uuid;

use super::Surface;

/// How many activity-log lines the status snapshot carries.
///
/// Enough for the dashboard's Activity card without turning every `status`
/// call into a kilobyte-scale message.
pub const RECENT_EVENTS: usize = 50;

/// Capacity of the IPC broadcast. Sized like the engine's own channel: a
/// subscriber that stalls for seconds catches up, one that stalls for longer
/// lags on purpose.
pub const STREAM_CAPACITY: usize = 1024;

/// How long a cached `service.status` answer is trusted.
///
/// Querying the SCM is a few milliseconds, but `status` is polled by every
/// front end several times a second and there is no reason to pay it each
/// time. Five seconds is imperceptible to a human watching a service start.
const SERVICE_STATUS_TTL_SECONDS: i64 = 5;

/// Take a `std` lock, recovering from poison.
///
/// A poisoned lock means some other task panicked while holding it. The
/// guarded values here are all caches and view state — losing consistency in
/// one of them is not a reason to refuse to answer `status` for the rest of
/// the process's life.
pub fn recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn recover_read<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn recover_write<T>(m: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A passphrase rotation that has been committed to the vault but whose
/// repositories have not all been re-passworded yet.
///
/// Held here rather than inside the handler because two things need it: the
/// migration driver, and the scheduler suppression that keeps a pending
/// destination out of the config the scheduler sees.
#[derive(Debug, Clone)]
pub struct PendingMigration {
    /// Destinations still on the old repository password.
    pub destinations: BTreeSet<Uuid>,
    /// The last report written to disk. Carried so a status line can name
    /// what is still pending without re-reading the file, and so a resume
    /// after a restart starts from the same picture.
    #[allow(dead_code)]
    pub report: superbackup_core::crypto::rekey::MigrationReport,
}

impl PendingMigration {
    pub fn is_empty(&self) -> bool {
        self.destinations.is_empty()
    }
}

/// Everything the daemon shares.
#[derive(Debug)]
pub struct Runtime {
    pub paths: Paths,
    /// Whether this instance shows a tray.
    ///
    /// Carried so that anything holding a `Runtime` can tell whether there is
    /// a user in front of it — the difference between raising a toast and
    /// writing a log line. `daemon::run` decides the notifier from it before
    /// the `Runtime` exists, so nothing inside reads it yet.
    #[allow(dead_code)]
    pub surface: Surface,
    /// Configuration and vault. `tokio` mutex: its critical sections write
    /// files.
    pub store: tokio::sync::Mutex<Store>,
    /// Run history and per-job summaries, shared with the engine.
    pub persisted: Arc<tokio::sync::Mutex<PersistedState>>,
    /// The IPC fan-out. `event_stream` hands out receivers on this.
    pub stream: broadcast::Sender<StreamItem>,
    pub environment: Arc<super::environment::DaemonEnvironment>,
    pub notifier: Arc<Notifier>,

    kopia: RwLock<Option<KopiaBinary>>,
    scheduler: RwLock<Option<SchedulerHandle>>,
    /// Runs that have started and not yet finished, keyed by run id.
    active: Mutex<BTreeMap<Uuid, JobRun>>,
    recent: Mutex<VecDeque<Event>>,
    auto_lock_at: Mutex<Option<DateTime<Utc>>>,
    migration: Mutex<Option<PendingMigration>>,
    /// The last `remote.pull`, waiting for `remote.diff` and `remote.apply`.
    /// `Arc` because [`superbackup_core::remote::PullPlan`] is deliberately
    /// not `Clone` — it is proof of one verification, not a value to copy.
    pull: Mutex<Option<Arc<superbackup_core::remote::PullPlan>>>,
    /// Cached `dest.stats`, so a dashboard refresh does not walk a bucket.
    stats: Mutex<BTreeMap<Uuid, superbackup_core::ipc::protocol::StorageStatsReply>>,
    service_status: Mutex<Option<(DateTime<Utc>, ServiceStatus)>>,
    /// Set of jobs the tray's "Disable all jobs" switched off, so unticking it
    /// re-enables exactly those and not the ones the user disabled by hand.
    bulk_disabled: Mutex<BTreeSet<Uuid>>,
    /// Jobs the scheduler refused to start because the vault was locked.
    ///
    /// The scheduler *drains* its queue rather than holding blocked runs — a
    /// deliberate choice, since a queue that fills up while a laptop is locked
    /// would stampede the moment it opened. The consequence is that nothing
    /// remembers the missed run, so the daemon does: `vault.unlock` re-queues
    /// exactly this set, which is what makes "unlock and my backup runs" true
    /// rather than "unlock and wait for the next occurrence".
    blocked_by_lock: Mutex<BTreeSet<Uuid>>,
    /// Appends to `events.ndjson` without blocking whoever raised the event.
    event_log: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Event>>>,

    /// The master passphrase, held only while the vault is unlocked.
    ///
    /// Retaining it is not free and is not done lightly. Two operations
    /// genuinely require the passphrase rather than the derived keys:
    ///
    /// * `remote.pull` — [`superbackup_core::remote::verify_pull`] must
    ///   *decrypt a different vault file*, and only the passphrase can do
    ///   that; the local vault's keys are useless against it.
    /// * `Settings::use_os_keychain` — caching for unattended unlock stores
    ///   the passphrase, by definition.
    ///
    /// The alternative is to prompt for the passphrase again on every pull,
    /// which is exactly the friction that makes people pick a weak one. It
    /// lives in a [`Secret`], which zeroes on drop, is dropped the instant the
    /// vault locks, and has no path to any reply, log line or notification.
    master: Mutex<Option<superbackup_core::secret::Secret>>,

    shutdown: broadcast::Sender<ShutdownRequest>,
    shutting_down: AtomicBool,
    started_at: DateTime<Utc>,
    /// Counts events dropped because nobody was listening; diagnostics only.
    lost_events: AtomicU64,
}

/// Why the daemon is stopping, and how much patience it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownRequest {
    /// Cancel in-flight runs immediately rather than letting them finish.
    pub stop_runs: bool,
}

impl Runtime {
    pub fn new(
        paths: Paths,
        surface: Surface,
        store: Store,
        persisted: PersistedState,
        environment: Arc<super::environment::DaemonEnvironment>,
        notifier: Arc<Notifier>,
    ) -> Arc<Runtime> {
        let (stream, _) = broadcast::channel(STREAM_CAPACITY);
        let (shutdown, _) = broadcast::channel(4);
        Arc::new(Runtime {
            paths,
            surface,
            store: tokio::sync::Mutex::new(store),
            persisted: Arc::new(tokio::sync::Mutex::new(persisted)),
            stream,
            environment,
            notifier,
            kopia: RwLock::new(None),
            scheduler: RwLock::new(None),
            active: Mutex::new(BTreeMap::new()),
            recent: Mutex::new(VecDeque::new()),
            auto_lock_at: Mutex::new(None),
            migration: Mutex::new(None),
            pull: Mutex::new(None),
            stats: Mutex::new(BTreeMap::new()),
            service_status: Mutex::new(None),
            bulk_disabled: Mutex::new(BTreeSet::new()),
            blocked_by_lock: Mutex::new(BTreeSet::new()),
            event_log: Mutex::new(None),
            master: Mutex::new(None),
            shutdown,
            shutting_down: AtomicBool::new(false),
            started_at: Utc::now(),
            lost_events: AtomicU64::new(0),
        })
    }

    // ------------------------------------------------------------------
    // Scheduler and kopia handles
    // ------------------------------------------------------------------

    pub fn set_scheduler(&self, handle: SchedulerHandle) {
        *recover_write(&self.scheduler) = Some(handle);
    }

    /// The engine handle, or `None` before the scheduler has been spawned and
    /// after it has been torn down.
    pub fn scheduler(&self) -> Option<SchedulerHandle> {
        recover_read(&self.scheduler).clone()
    }

    /// The engine handle, as an error when the engine is not running — which
    /// is what every IPC method that needs it wants.
    pub fn require_scheduler(&self) -> superbackup_core::Result<SchedulerHandle> {
        self.scheduler()
            .ok_or_else(|| superbackup_core::Error::Internal("the scheduler is not running".into()))
    }

    pub fn set_kopia(&self, binary: Option<KopiaBinary>) {
        *recover_write(&self.kopia) = binary;
    }

    pub fn kopia(&self) -> Option<KopiaBinary> {
        recover_read(&self.kopia).clone()
    }

    pub fn kopia_version(&self) -> Option<String> {
        recover_read(&self.kopia).as_ref().map(|b| b.version().to_string())
    }

    // ------------------------------------------------------------------
    // Shutdown
    // ------------------------------------------------------------------

    pub fn subscribe_shutdown(&self) -> broadcast::Receiver<ShutdownRequest> {
        self.shutdown.subscribe()
    }

    /// Ask the daemon to stop. Idempotent, and safe from any thread — this is
    /// what the Ctrl-C handler, the SCM control handler, the tray's Quit item
    /// and `control.shutdown` all call.
    pub fn request_shutdown(&self, stop_runs: bool) {
        self.shutting_down.store(true, Ordering::SeqCst);
        // A dropped receiver is normal during teardown and is not an error.
        let _ = self.shutdown.send(ShutdownRequest { stop_runs });
    }

    /// Whether shutdown has already been requested.
    ///
    /// Answerable without a subscription, for anything that wants to skip
    /// work rather than start it during teardown.
    #[allow(dead_code)]
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    // ------------------------------------------------------------------
    // Event log and stream
    // ------------------------------------------------------------------

    pub fn set_event_log(&self, tx: tokio::sync::mpsc::UnboundedSender<Event>) {
        *recover(&self.event_log) = Some(tx);
    }

    /// Record an activity-log line: keep it in the ring for `status`, append
    /// it to `events.ndjson`, and publish it to every subscriber.
    ///
    /// Synchronous and non-blocking on purpose. It is called from the engine's
    /// event pump, from IPC handlers and from the executor, and none of those
    /// may be made to wait on a disk write or a slow subscriber.
    pub fn record_event(&self, event: Event) {
        tracing::info!(
            kind = %event.kind,
            severity = ?event.severity,
            message = %event.message,
            "activity"
        );
        {
            let mut ring = recover(&self.recent);
            ring.push_back(event.clone());
            while ring.len() > RECENT_EVENTS {
                ring.pop_front();
            }
        }
        if let Some(tx) = recover(&self.event_log).as_ref() {
            let _ = tx.send(event.clone());
        }
        self.publish(StreamItem::Event { event: Box::new(event) });
    }

    /// Push one item to IPC subscribers. Never blocks; a send with no
    /// receivers is the normal case when nothing is connected.
    pub fn publish(&self, item: StreamItem) {
        if self.stream.send(item).is_err() {
            self.lost_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn subscribe_stream(&self, _topics: &[Topic]) -> broadcast::Receiver<StreamItem> {
        // Filtering by topic is the transport's job — it already does it in
        // `StreamItem::matches` — so every subscriber gets the same receiver
        // and the daemon keeps exactly one fan-out.
        self.stream.subscribe()
    }

    pub fn recent_events(&self) -> Vec<Event> {
        recover(&self.recent).iter().cloned().collect()
    }

    // ------------------------------------------------------------------
    // Live runs
    // ------------------------------------------------------------------

    pub fn set_active(&self, run: JobRun) {
        recover(&self.active).insert(run.run_id, run);
    }

    pub fn clear_active(&self, run_id: &Uuid) -> Option<JobRun> {
        recover(&self.active).remove(run_id)
    }

    pub fn active_runs(&self) -> Vec<JobRun> {
        recover(&self.active).values().cloned().collect()
    }

    /// Which job a run belongs to. `job.stop` takes a run id but the engine
    /// cancels by job, so this is the bridge.
    pub fn job_for_run(&self, run_id: &Uuid) -> Option<Uuid> {
        recover(&self.active).get(run_id).map(|r| r.job_id)
    }

    /// Update one destination's live progress inside the tracked run, so that
    /// a `status` call taken mid-backup shows real numbers rather than the
    /// zeroes the run started with.
    pub fn apply_progress(
        &self,
        run_id: &Uuid,
        destination_id: &Uuid,
        progress: &superbackup_core::state::Progress,
        final_update: bool,
    ) {
        let mut active = recover(&self.active);
        let Some(run) = active.get_mut(run_id) else { return };
        if let Some(dest) =
            run.destinations.iter_mut().find(|d| &d.destination_id == destination_id)
        {
            dest.progress = progress.clone();
            if !final_update && dest.status == RunStatus::Preparing {
                dest.status = RunStatus::Running;
            }
        }
        if run.status.is_active() {
            run.status = RunStatus::Running;
        }
    }

    /// Replace a finished destination's record inside a live run.
    pub fn apply_destination_finished(
        &self,
        run_id: &Uuid,
        destination: superbackup_core::state::DestinationRun,
    ) {
        let mut active = recover(&self.active);
        let Some(run) = active.get_mut(run_id) else { return };
        match run.destinations.iter_mut().find(|d| d.destination_id == destination.destination_id) {
            Some(slot) => *slot = destination,
            None => run.destinations.push(destination),
        }
    }

    // ------------------------------------------------------------------
    // Auto-lock
    // ------------------------------------------------------------------

    /// Arm (or disarm) the auto-lock deadline.
    ///
    /// `auto_lock_minutes == 0` means "never", which is a supported choice for
    /// a machine that must back up unattended, so it disarms rather than
    /// picking a default.
    pub fn arm_auto_lock(&self, minutes: u32) {
        let deadline = (minutes > 0).then(|| Utc::now() + ChronoDuration::minutes(minutes as i64));
        *recover(&self.auto_lock_at) = deadline;
    }

    pub fn disarm_auto_lock(&self) {
        *recover(&self.auto_lock_at) = None;
    }

    pub fn auto_lock_at(&self) -> Option<DateTime<Utc>> {
        *recover(&self.auto_lock_at)
    }

    /// True when the deadline has passed. Called by the auto-lock timer.
    pub fn auto_lock_due(&self, now: DateTime<Utc>) -> bool {
        matches!(*recover(&self.auto_lock_at), Some(at) if now >= at)
    }

    // ------------------------------------------------------------------
    // The master passphrase, while unlocked
    // ------------------------------------------------------------------

    /// Hold the passphrase for the lifetime of the unlock. See the field's
    /// documentation for why this is retained at all.
    pub fn remember_master(&self, passphrase: superbackup_core::secret::Secret) {
        *recover(&self.master) = Some(passphrase);
    }

    /// Drop and zero the retained passphrase. Called by every path that locks
    /// the vault, including the auto-lock timer and shutdown.
    pub fn forget_master(&self) {
        *recover(&self.master) = None;
    }

    /// A copy of the retained passphrase, for the two operations that need it.
    ///
    /// Returns [`superbackup_core::Error::Locked`] rather than `None` so a
    /// caller cannot accidentally continue with no passphrase — the whole
    /// point is that these operations are impossible while locked.
    pub fn master(&self) -> superbackup_core::Result<superbackup_core::secret::Secret> {
        recover(&self.master)
            .as_ref()
            .map(|s| superbackup_core::secret::Secret::new(s.expose().to_vec()))
            .ok_or(superbackup_core::Error::Locked)
    }

    // ------------------------------------------------------------------
    // Passphrase-rotation migration
    // ------------------------------------------------------------------

    pub fn set_migration(&self, pending: Option<PendingMigration>) {
        *recover(&self.migration) = pending;
    }

    pub fn migration(&self) -> Option<PendingMigration> {
        recover(&self.migration).clone()
    }

    /// Destinations whose repository is still on the old password.
    pub fn migration_pending(&self) -> BTreeSet<Uuid> {
        recover(&self.migration).as_ref().map(|m| m.destinations.clone()).unwrap_or_default()
    }

    // ------------------------------------------------------------------
    // Remote pull staging
    // ------------------------------------------------------------------

    pub fn set_pull(&self, plan: Option<superbackup_core::remote::PullPlan>) {
        *recover(&self.pull) = plan.map(Arc::new);
    }

    pub fn pull(&self) -> Option<Arc<superbackup_core::remote::PullPlan>> {
        recover(&self.pull).clone()
    }

    // ------------------------------------------------------------------
    // Storage stats cache
    // ------------------------------------------------------------------

    pub fn cache_stats(&self, reply: superbackup_core::ipc::protocol::StorageStatsReply) {
        recover(&self.stats).insert(reply.destination_id, reply);
    }

    pub fn cached_stats(
        &self,
        destination_id: &Uuid,
    ) -> Option<superbackup_core::ipc::protocol::StorageStatsReply> {
        recover(&self.stats).get(destination_id).cloned()
    }

    // ------------------------------------------------------------------
    // Bulk enable/disable, for the tray's checkbox item
    // ------------------------------------------------------------------

    pub fn set_bulk_disabled(&self, jobs: BTreeSet<Uuid>) {
        *recover(&self.bulk_disabled) = jobs;
    }

    pub fn bulk_disabled(&self) -> BTreeSet<Uuid> {
        recover(&self.bulk_disabled).clone()
    }

    /// Remember that a scheduled run was dropped because the vault was locked.
    pub fn note_blocked_by_lock(&self, job_id: Uuid) {
        recover(&self.blocked_by_lock).insert(job_id);
    }

    /// Take the blocked set, leaving it empty. Called exactly once per unlock.
    pub fn take_blocked_by_lock(&self) -> BTreeSet<Uuid> {
        std::mem::take(&mut *recover(&self.blocked_by_lock))
    }

    // ------------------------------------------------------------------
    // Service status, cached
    // ------------------------------------------------------------------

    /// The installed service's state, from a short-lived cache.
    ///
    /// Blocking: call from a blocking context or accept a few milliseconds.
    pub fn service_status(&self) -> ServiceStatus {
        let now = Utc::now();
        if let Some((at, status)) = recover(&self.service_status).as_ref() {
            if (now - *at).num_seconds() < SERVICE_STATUS_TTL_SECONDS {
                return status.clone();
            }
        }
        let status = platform::service::status(
            platform::service::DEFAULT_SERVICE_NAME,
            platform::ServiceScope::System,
        )
        .unwrap_or_else(|e| {
            tracing::debug!(error = %e, "could not read service status");
            ServiceStatus::not_installed()
        });
        *recover(&self.service_status) = Some((now, status.clone()));
        status
    }

    /// Drop the cached answer, so the next read is fresh. Called right after
    /// install/uninstall/start/stop, where a five-second-stale answer would
    /// make the GUI look broken.
    pub fn invalidate_service_status(&self) {
        *recover(&self.service_status) = None;
    }

    // ------------------------------------------------------------------
    // Config views
    // ------------------------------------------------------------------

    /// The configuration the *scheduler* should run, which is not always the
    /// configuration the user wrote.
    ///
    /// A destination whose repository is still on the old password after a
    /// passphrase rotation is marked disabled here, and only here. The user's
    /// `config.json` is untouched, so an interrupted migration cannot leave a
    /// destination permanently switched off on disk; the suppression is
    /// rebuilt from the persisted migration report at startup instead.
    ///
    /// Without this, every scheduled run against every migrated repository
    /// fails with "incorrect password" until the user notices — a wall of
    /// expected failures that buries the one message that matters.
    pub fn effective_config(&self, config: &Config) -> Arc<Config> {
        let pending = self.migration_pending();
        if pending.is_empty() {
            return Arc::new(config.clone());
        }
        let mut copy = config.clone();
        for destination in &mut copy.destinations {
            if pending.contains(&destination.id) {
                destination.enabled = false;
            }
        }
        Arc::new(copy)
    }

    /// Hand the scheduler the current effective configuration.
    pub fn push_config(&self, config: &Config) {
        if let Some(scheduler) = self.scheduler() {
            if let Err(e) = scheduler.replace_config(self.effective_config(config)) {
                tracing::warn!(error = %e, "could not hand the new configuration to the scheduler");
            }
        }
    }

    // ------------------------------------------------------------------
    // Status snapshot
    // ------------------------------------------------------------------

    pub fn uptime_seconds(&self) -> u64 {
        (Utc::now() - self.started_at).num_seconds().max(0) as u64
    }

    /// Assemble the one message the tray, the dashboard and `status` all use.
    ///
    /// Everything expensive is either cached (service status) or already in
    /// memory (runs, summaries, events), so this is cheap enough to call on
    /// every status change and on every poll.
    pub fn snapshot(
        &self,
        config: &Config,
        unlocked: bool,
        persisted: &PersistedState,
        scheduler: Option<&SchedulerStatus>,
    ) -> StatusSnapshot {
        let now = Utc::now();
        let active_runs = self.active_runs();
        let paused = config.settings.pause.is_active(now);
        let service = self.service_status();

        let stale_days = config.settings.notifications.stale_after_days;
        let mut any_failed = service.installed && service.health() == Health::Failed;
        let mut any_stale = false;
        let mut jobs = BTreeMap::new();
        for job in &config.jobs {
            let mut summary = persisted.jobs.get(&job.id).cloned().unwrap_or_default();
            summary.next_run = scheduler.and_then(|s| s.next_runs.get(&job.id).copied());
            if matches!(summary.last_status, Some(RunStatus::Failed)) {
                any_failed = true;
            }
            if job.enabled && summary.is_stale(stale_days, now) {
                any_stale = true;
            }
            jobs.insert(job.id, summary);
        }

        StatusSnapshot {
            health: StatusSnapshot::derive_health(
                unlocked,
                paused,
                !active_runs.is_empty(),
                any_failed,
                any_stale,
            ),
            version: superbackup_core::VERSION.to_string(),
            machine_label: config.machine.label.clone(),
            machine_slug: config.machine.slug.clone(),
            unlocked,
            paused,
            paused_until: config.settings.pause.until,
            service_installed: service.installed,
            service_running: service.state == platform::ServiceState::Running,
            kopia_version: self.kopia_version(),
            active_runs,
            jobs,
            next_scheduled: scheduler.and_then(|s| s.next_scheduled),
            recent_events: self.recent_events(),
            uptime_seconds: self.uptime_seconds(),
            generated_at: now,
        }
    }

    /// Build a snapshot from the live store and publish it.
    ///
    /// Async because it takes the store and state locks; called whenever the
    /// aggregate picture changes rather than on a timer, so a tray driven by
    /// the stream never polls.
    pub async fn publish_status(&self) {
        let snapshot = self.current_snapshot().await;
        self.publish(StreamItem::Status { snapshot: Box::new(snapshot) });
    }

    /// The current snapshot, taking every lock it needs in a fixed order:
    /// scheduler, store, persisted. Nothing else in the daemon takes them in a
    /// different order, so there is no cycle to deadlock on.
    pub async fn current_snapshot(&self) -> StatusSnapshot {
        let scheduler = match self.scheduler() {
            Some(handle) => handle.status().await.ok(),
            None => None,
        };
        let store = self.store.lock().await;
        let config = store.config().clone();
        let unlocked = !store.is_locked();
        drop(store);
        let persisted = self.persisted.lock().await;
        self.snapshot(&config, unlocked, &persisted, scheduler.as_ref())
    }
}

// ---------------------------------------------------------------------------
// Free helpers shared by the handler and the tray
// ---------------------------------------------------------------------------

/// Resolve a job by id or by a unique prefix of its name.
///
/// Ambiguity is an error rather than a silent first-match: `superbackup run
/// dev` picking one of "dev code" and "dev databases" at random is the kind of
/// helpfulness that deletes the wrong thing.
pub fn resolve_job<'a>(config: &'a Config, needle: &str) -> superbackup_core::Result<&'a Job> {
    if let Ok(id) = Uuid::parse_str(needle) {
        if let Some(job) = config.job(&id) {
            return Ok(job);
        }
    }
    if let Some(job) = config.jobs.iter().find(|j| j.name.eq_ignore_ascii_case(needle)) {
        return Ok(job);
    }
    let lower = needle.to_lowercase();
    let matches: Vec<&Job> =
        config.jobs.iter().filter(|j| j.name.to_lowercase().starts_with(&lower)).collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(superbackup_core::Error::JobNotFound(needle.to_string())),
        _ => Err(superbackup_core::Error::Validation(format!(
            "`{needle}` matches {} jobs ({}); use the full name or the id",
            matches.len(),
            matches.iter().map(|j| j.name.as_str()).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// Resolve a destination by id or by a unique prefix of its name.
pub fn resolve_destination<'a>(
    config: &'a Config,
    needle: &str,
) -> superbackup_core::Result<&'a Destination> {
    if let Ok(id) = Uuid::parse_str(needle) {
        if let Some(d) = config.destination(&id) {
            return Ok(d);
        }
    }
    if let Some(d) = config.destinations.iter().find(|d| d.name.eq_ignore_ascii_case(needle)) {
        return Ok(d);
    }
    let lower = needle.to_lowercase();
    let matches: Vec<&Destination> =
        config.destinations.iter().filter(|d| d.name.to_lowercase().starts_with(&lower)).collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => {
            Err(superbackup_core::Error::Validation(format!("no destination matching `{needle}`")))
        }
        _ => Err(superbackup_core::Error::Validation(format!(
            "`{needle}` matches {} destinations; use the full name or the id",
            matches.len()
        ))),
    }
}

/// Resolve a provider by id or by a unique prefix of its name.
pub fn resolve_provider<'a>(
    config: &'a Config,
    needle: &str,
) -> superbackup_core::Result<&'a superbackup_core::model::StorageProvider> {
    if let Ok(id) = Uuid::parse_str(needle) {
        if let Some(p) = config.provider(&id) {
            return Ok(p);
        }
    }
    if let Some(p) = config.providers.iter().find(|p| p.name.eq_ignore_ascii_case(needle)) {
        return Ok(p);
    }
    let lower = needle.to_lowercase();
    let matches: Vec<_> =
        config.providers.iter().filter(|p| p.name.to_lowercase().starts_with(&lower)).collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(superbackup_core::Error::Validation(format!("no provider matching `{needle}`"))),
        _ => Err(superbackup_core::Error::Validation(format!(
            "`{needle}` matches {} providers; use the full name or the id",
            matches.len()
        ))),
    }
}

/// One line describing the health, matching the tray tooltip's second line.
///
/// Kept next to the snapshot rather than in the tray so that `health` over IPC
/// and the tooltip cannot drift apart.
pub fn health_summary(snapshot: &StatusSnapshot, config: &Config) -> (String, Vec<String>) {
    let mut reasons = Vec::new();
    let line = match snapshot.health {
        Health::Running => {
            let running = snapshot.active_runs.len();
            match snapshot.active_runs.first() {
                Some(run) if running == 1 => {
                    let pct = run.overall_fraction().map(|f| (f * 100.0).round() as u32);
                    match pct {
                        Some(p) => format!("{} — {p}%", run.job_name),
                        None => run.job_name.clone(),
                    }
                }
                _ => format!("{running} backups running"),
            }
        }
        Health::Paused => match snapshot.paused_until {
            Some(until) => format!("Paused until {}", until.format("%H:%M")),
            None => "Paused until you resume".to_string(),
        },
        Health::Failed => {
            let failed: Vec<&str> = config
                .jobs
                .iter()
                .filter(|j| {
                    matches!(
                        snapshot.jobs.get(&j.id).and_then(|s| s.last_status),
                        Some(RunStatus::Failed)
                    )
                })
                .map(|j| j.name.as_str())
                .collect();
            for name in &failed {
                reasons.push(format!("{name} failed"));
            }
            match failed.first() {
                Some(name) => format!("{name} failed"),
                None => "A backup failed".to_string(),
            }
        }
        Health::Attention => {
            if !snapshot.unlocked {
                reasons.push("The vault is locked".to_string());
                "The vault is locked".to_string()
            } else {
                let days = config.settings.notifications.stale_after_days;
                let stale: Vec<&str> = config
                    .jobs
                    .iter()
                    .filter(|j| {
                        j.enabled
                            && snapshot
                                .jobs
                                .get(&j.id)
                                .map(|s| s.is_stale(days, snapshot.generated_at))
                                .unwrap_or(false)
                    })
                    .map(|j| j.name.as_str())
                    .collect();
                for name in &stale {
                    reasons.push(format!("{name} has not succeeded for {days} days"));
                }
                match stale.first() {
                    Some(name) => format!("{name} has not succeeded for {days} days"),
                    None => "Needs attention".to_string(),
                }
            }
        }
        Health::Idle => match snapshot.next_scheduled {
            Some((_, at)) => format!("Next run {}", relative_future(at, snapshot.generated_at)),
            None => "No backups scheduled".to_string(),
        },
    };
    (line, reasons)
}

/// "in 4 minutes", "in about 2 hours" — the tray tooltip's vocabulary.
pub fn relative_future(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = (at - now).num_seconds();
    if seconds <= 0 {
        return "now".to_string();
    }
    if seconds < 90 {
        return format!("in {seconds} seconds");
    }
    let minutes = seconds / 60;
    if minutes < 90 {
        return format!("in {minutes} minutes");
    }
    let hours = minutes / 60;
    if hours < 36 {
        return format!("in about {hours} hours");
    }
    format!("in {} days", hours / 24)
}

/// "2 hours ago", for the tray's "Last backup" line.
pub fn relative_past(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = (now - at).num_seconds();
    if seconds < 60 {
        return "just now".to_string();
    }
    let minutes = seconds / 60;
    if minutes < 90 {
        return format!("{minutes} minutes ago");
    }
    let hours = minutes / 60;
    if hours < 36 {
        return format!("{hours} hours ago");
    }
    format!("{} days ago", hours / 24)
}

/// The event a settings change should record.
pub fn settings_changed_event(before: &Settings, after: &Settings) -> Option<Event> {
    let mut changed = Vec::new();
    if before.pause.paused != after.pause.paused {
        changed.push("pause");
    }
    if before.bandwidth.upload_kbps != after.bandwidth.upload_kbps {
        changed.push("bandwidth");
    }
    if before.auto_lock_minutes != after.auto_lock_minutes {
        changed.push("auto-lock");
    }
    if before.max_parallel_jobs != after.max_parallel_jobs {
        changed.push("parallelism");
    }
    if changed.is_empty() {
        return None;
    }
    Some(Event::new(
        Severity::Info,
        "settings.updated",
        format!("Settings changed: {}.", changed.join(", ")),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use superbackup_core::engine::testing::test_job;

    fn config_with(names: &[&str]) -> Config {
        let mut config = Config::default();
        for name in names {
            config.jobs.push(test_job(name));
        }
        config
    }

    #[test]
    fn a_job_resolves_by_id_by_exact_name_and_by_unique_prefix() {
        let config = config_with(&["dev code", "photos"]);
        let id = config.jobs[0].id;
        assert_eq!(resolve_job(&config, &id.to_string()).map(|j| j.id).ok(), Some(id));
        assert_eq!(resolve_job(&config, "dev code").map(|j| j.id).ok(), Some(id));
        assert_eq!(resolve_job(&config, "dev").map(|j| j.id).ok(), Some(id));
        assert_eq!(
            resolve_job(&config, "PHOT").map(|j| j.name.clone()).ok(),
            Some("photos".into())
        );
    }

    #[test]
    fn an_ambiguous_prefix_is_refused_rather_than_guessed() {
        let config = config_with(&["dev code", "dev databases"]);
        let err = resolve_job(&config, "dev").expect_err("ambiguous");
        assert!(err.to_string().contains("matches 2 jobs"), "{err}");
    }

    #[test]
    fn an_exact_name_wins_over_a_prefix_collision() {
        let config = config_with(&["dev", "dev databases"]);
        assert_eq!(resolve_job(&config, "dev").map(|j| j.name.clone()).ok(), Some("dev".into()));
    }

    #[test]
    fn an_unknown_job_is_job_not_found_not_validation() {
        let config = config_with(&["dev"]);
        let err = resolve_job(&config, "nope").expect_err("missing");
        assert_eq!(err.code(), superbackup_core::ErrorCode::JobNotFound);
    }

    #[test]
    fn relative_times_read_like_english() {
        let now = Utc::now();
        assert_eq!(relative_future(now + ChronoDuration::seconds(30), now), "in 30 seconds");
        assert_eq!(relative_future(now + ChronoDuration::minutes(5), now), "in 5 minutes");
        assert_eq!(relative_future(now + ChronoDuration::hours(3), now), "in about 3 hours");
        assert_eq!(relative_past(now - ChronoDuration::hours(2), now), "2 hours ago");
        assert_eq!(relative_past(now - ChronoDuration::seconds(3), now), "just now");
    }
}
