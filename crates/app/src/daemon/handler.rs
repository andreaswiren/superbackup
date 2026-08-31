//! The [`Handler`] implementation: sixty commands wired to the store, the
//! engine, the kopia driver, the platform layer and the remote.
//!
//! ## What every method here obeys
//!
//! 1. **No secret leaves.** There is no `GetSecret` in the protocol and this
//!    file adds none. `vault.set_secret` and
//!    `provider.rotate_credentials` take [`SecretString`]s in; nothing returns
//!    one, and no error message, hint, detail or status field is built from
//!    one. `vault.list_refs` returns handles, never values.
//! 2. **No panic from input.** Every lookup is a `Result`, every numeric
//!    conversion is saturating, and no method indexes, unwraps or slices on
//!    data that arrived over the socket.
//! 3. **No lock across an await that could re-enter.** The store's `tokio`
//!    mutex is taken, used, and dropped inside one statement group; nothing
//!    calls back into the handler while holding it.
//! 4. **Handlers return promptly.** Anything that can take minutes — a
//!    backup, a restore, a repository creation over a slow link — returns a
//!    run id and does its work on a task. That is the transport's documented
//!    expectation, and the reason `handler_timeout` exists as a backstop
//!    rather than as a normal path.
//!
//! ## Authorisation
//!
//! The daemon may run as SYSTEM while its clients are ordinary user
//! processes, so authorisation is on what the caller *knows*, not on the fact
//! that it reached the socket. Everything that touches secret material goes
//! through [`DaemonHandler::require_unlocked`], which is the single place that
//! turns "the vault is locked" into [`Error::Locked`].

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use superbackup_core::ipc::protocol::*;
use superbackup_core::ipc::{Handler, RequestContext, StreamItem, Topic};
use superbackup_core::kopia::{
    cancellation, EventSink, KopiaEvent, RestoreOptions, RunContext, MINIMUM_KOPIA_VERSION,
};
use superbackup_core::model::{
    Destination, DestinationKind, EncryptionSettings, Job, PauseState, ProviderKind, SecretRef,
    Settings, StorageProvider,
};
use superbackup_core::platform::{self, ServiceScope};
use superbackup_core::secret::Secret;
use superbackup_core::state::{Event, RunStatus, Severity, Trigger};
use superbackup_core::{Error, ErrorCode, Result};
use tokio::sync::broadcast;
use uuid::Uuid;

use super::executor::build_driver;
use super::runtime::{
    health_summary, resolve_destination, resolve_job, resolve_provider, settings_changed_event,
    Runtime,
};

/// Largest directory listing `snapshot.browse` will serialise in one reply.
///
/// A `node_modules` with forty thousand entries must not become a forty-
/// megabyte IPC frame; the reply carries `truncated: true` and the caller
/// descends or filters instead.
const MAX_LISTING_ENTRIES: usize = 2_000;

/// Cap on `snapshot.list` when the caller passes 0 or something absurd.
const MAX_SNAPSHOTS: usize = 500;

/// The IPC surface of a running daemon.
#[derive(Debug, Clone)]
pub struct DaemonHandler {
    runtime: Arc<Runtime>,
}

impl DaemonHandler {
    pub fn new(runtime: Arc<Runtime>) -> DaemonHandler {
        DaemonHandler { runtime }
    }

    /// Refuse anything that needs secret material while the vault is locked.
    async fn require_unlocked(&self) -> Result<()> {
        if self.runtime.store.lock().await.is_locked() {
            return Err(Error::Locked);
        }
        Ok(())
    }

    /// The current configuration, cloned out from under the lock.
    async fn config(&self) -> superbackup_core::model::Config {
        self.runtime.store.lock().await.config().clone()
    }

    /// Apply a change to the configuration: validate, persist, hand it to the
    /// scheduler, and publish a fresh status.
    ///
    /// Every mutating command funnels through this so that "saved but the
    /// scheduler never heard about it" cannot happen — the failure that makes
    /// a user edit a schedule twice and then stop trusting the app.
    async fn commit(
        &self,
        mutate: impl FnOnce(&mut superbackup_core::model::Config) -> Result<()>,
    ) -> Result<superbackup_core::model::Config> {
        let mut store = self.runtime.store.lock().await;
        let mut config = store.config().clone();
        mutate(&mut config)?;
        store.set_config(config)?;
        let saved = store.config().clone();
        drop(store);
        self.runtime.push_config(&saved);
        self.runtime.publish_status().await;
        Ok(saved)
    }

    /// Look up a destination and build a kopia driver for it.
    async fn driver_for(
        &self,
        needle: &str,
    ) -> Result<(Destination, superbackup_core::kopia::KopiaDriver)> {
        let binary = self.runtime.kopia().ok_or(Error::KopiaMissing)?;
        let store = self.runtime.store.lock().await;
        let destination = resolve_destination(store.config(), needle)?.clone();
        let driver = build_driver(&store, &self.runtime.paths, binary, &destination);
        drop(store);
        Ok((destination, driver?))
    }

    fn unlocked_reply(&self, unlocked: bool) -> UnlockedReply {
        UnlockedReply { unlocked, auto_lock_at: self.runtime.auto_lock_at() }
    }

    async fn pause_reply(&self) -> PauseReply {
        PauseReply { pause: self.config().await.settings.pause.clone() }
    }

    fn service_reply(&self, detail: Option<String>) -> ServiceReply {
        let status = self.runtime.service_status();
        let autostart = platform::autostart::is_enabled().unwrap_or(false);
        ServiceReply {
            installed: status.installed,
            running: status.state == platform::ServiceState::Running,
            autostart,
            scope: if self.runtime.paths.service_scope { "system".into() } else { "user".into() },
            detail: detail.or(status.detail),
        }
    }

    async fn remote_status_reply(&self, detail: Option<String>) -> RemoteStatusReply {
        let config = self.config().await;
        let Some(remote) = &config.remote else {
            return RemoteStatusReply {
                url: None,
                branch: None,
                last_pull_at: None,
                last_known_commit: None,
                local_changes: false,
                remote_changes: false,
                detail: Some("No remote configuration repository is set up.".into()),
            };
        };
        let staged = self.runtime.pull();
        let remote_changes = staged.as_ref().map(|p| !p.diff.is_empty()).unwrap_or(false);
        RemoteStatusReply {
            url: Some(remote.url.clone()),
            branch: Some(remote.branch.clone()),
            last_pull_at: remote.last_pull_at,
            last_known_commit: remote.last_known_commit.clone(),
            local_changes: config.updated_at > remote.last_pull_at,
            remote_changes,
            detail,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers that are pure enough to test on their own
// ---------------------------------------------------------------------------

/// Map the protocol's conflict policy onto kopia's restore flags.
///
/// `KeepBoth` is refused rather than approximated. kopia's `restore`
/// (`cli/command_restore.go`) offers overwrite, skip-existing and
/// delete-extra, and nothing that writes `file (1).txt` — so honouring the
/// request would mean silently doing something else with the user's data
/// during a *restore*, which is the last operation in the program where a
/// surprise is acceptable.
pub fn restore_options(conflict: ConflictPolicy) -> Result<RestoreOptions> {
    let base = RestoreOptions::default();
    match conflict {
        ConflictPolicy::Skip => {
            Ok(RestoreOptions { overwrite_files: false, skip_existing: true, ..base })
        }
        ConflictPolicy::Overwrite => {
            Ok(RestoreOptions { overwrite_files: true, skip_existing: false, ..base })
        }
        // Neither overwrite nor skip: kopia then fails on the first collision,
        // which is exactly `Fail`.
        ConflictPolicy::Fail => {
            Ok(RestoreOptions { overwrite_files: false, skip_existing: false, ..base })
        }
        ConflictPolicy::KeepBoth => Err(Error::Validation(
            "kopia cannot restore a file alongside an existing one. Choose Skip, Overwrite or \
             Fail, or restore into an empty folder and merge afterwards."
                .into(),
        )),
    }
}

/// `snapshot.browse` addresses an entry as `<snapshot id>/<path inside it>`.
/// Normalise the caller's path into that shape without letting it escape.
pub fn browse_target(snapshot: &str, path: &str) -> Result<String> {
    if snapshot.trim().is_empty() {
        return Err(Error::Validation("no snapshot was named".into()));
    }
    if snapshot.contains('/') || snapshot.contains('\\') {
        return Err(Error::Validation("a snapshot id contains no path separators".into()));
    }
    let cleaned: Vec<&str> =
        path.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").collect();
    if cleaned.contains(&"..") {
        return Err(Error::Validation("a path inside a snapshot cannot contain `..`".into()));
    }
    if cleaned.is_empty() {
        return Ok(snapshot.to_string());
    }
    Ok(format!("{snapshot}/{}", cleaned.join("/")))
}

/// Normalise the `path` echoed back in a listing reply.
pub fn normalised_path(path: &str) -> String {
    path.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").collect::<Vec<_>>().join("/")
}

fn entry_kind(entry: &superbackup_core::kopia::DirEntry) -> EntryKind {
    use superbackup_core::kopia::EntryType;
    match entry.entry_type {
        EntryType::Directory => EntryKind::Directory,
        EntryType::Symlink => EntryKind::Symlink,
        _ => EntryKind::File,
    }
}

/// Turn a kopia manifest into the protocol's snapshot summary.
fn snapshot_info(
    manifest: &superbackup_core::kopia::SnapshotManifest,
    destination_id: Uuid,
) -> SnapshotInfo {
    let totals = manifest.totals();
    SnapshotInfo {
        id: manifest.id.clone(),
        destination_id,
        job_id: manifest.tags.get("superbackup-job").and_then(|v| Uuid::parse_str(v).ok()),
        created_at: manifest.start_time.unwrap_or_else(Utc::now),
        source_path: manifest.source.path.clone(),
        file_count: totals.map(|(files, _)| files),
        total_bytes: totals.map(|(_, bytes)| bytes),
        incomplete: !manifest.is_complete(),
        tags: manifest.tags.iter().map(|(k, v)| format!("{k}={v}")).collect(),
    }
}

fn check(id: &str, title: &str, status: CheckStatus) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        title: title.to_string(),
        status,
        detail: None,
        hint: None,
        fixable: false,
    }
}

// ---------------------------------------------------------------------------
// The Handler
// ---------------------------------------------------------------------------

impl Handler for DaemonHandler {
    // -- status -----------------------------------------------------------

    async fn ping(&self, _ctx: &RequestContext) -> Result<AckReply> {
        Ok(AckReply {})
    }

    async fn status(&self, _ctx: &RequestContext) -> Result<StatusReply> {
        Ok(StatusReply { snapshot: Box::new(self.runtime.current_snapshot().await) })
    }

    async fn version(&self, _ctx: &RequestContext) -> Result<VersionReply> {
        Ok(VersionReply {
            version: superbackup_core::VERSION.to_string(),
            protocol: superbackup_core::ipc::PROTOCOL_VERSION,
            min_protocol: superbackup_core::ipc::MIN_PROTOCOL_VERSION,
            target_os: std::env::consts::OS.to_string(),
            target_arch: std::env::consts::ARCH.to_string(),
            kopia_version: self.runtime.kopia_version(),
            service_scope: self.runtime.paths.service_scope,
        })
    }

    async fn health(&self, _ctx: &RequestContext) -> Result<HealthReply> {
        let snapshot = self.runtime.current_snapshot().await;
        let config = self.config().await;
        let (summary, reasons) = health_summary(&snapshot, &config);
        Ok(HealthReply { health: snapshot.health, summary, reasons })
    }

    async fn doctor(&self, _ctx: &RequestContext, fix: bool) -> Result<DoctorReply> {
        let mut checks = Vec::new();
        let mut fixed = Vec::new();

        // kopia
        match self.runtime.kopia() {
            Some(binary) => {
                let mut c = check("kopia.present", "kopia is installed", CheckStatus::Pass);
                c.detail = Some(format!("{} ({})", binary.version(), binary.source().title()));
                checks.push(c);
            }
            None => {
                let mut c = check("kopia.present", "kopia is installed", CheckStatus::Fail);
                c.hint = Some(format!(
                    "superbackup needs kopia {MINIMUM_KOPIA_VERSION} or newer. Open Settings → \
                     Kopia binary to install it."
                ));
                checks.push(c);
            }
        }

        // vault and configuration
        let store = self.runtime.store.lock().await;
        let locked = store.is_locked();
        let config = store.config().clone();
        let report = superbackup_core::config::validate(&config);
        drop(store);

        let mut vault = check(
            "vault.unlocked",
            "The vault is unlocked",
            if locked { CheckStatus::Warn } else { CheckStatus::Pass },
        );
        if locked {
            vault.hint = Some("Unlock superbackup so scheduled backups can run.".into());
        }
        checks.push(vault);

        let mut valid = check(
            "config.valid",
            "The configuration is valid",
            if report.is_ok() { CheckStatus::Pass } else { CheckStatus::Fail },
        );
        if !report.is_ok() {
            valid.detail =
                Some(report.errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "));
        }
        checks.push(valid);

        // jobs with no destination at all — a job that can never run
        let orphaned: Vec<&str> = config
            .jobs
            .iter()
            .filter(|j| j.destination_ids.iter().all(|id| config.destination(id).is_none()))
            .map(|j| j.name.as_str())
            .collect();
        let mut wired = check(
            "job.destinations",
            "Every job has a destination",
            if orphaned.is_empty() { CheckStatus::Pass } else { CheckStatus::Fail },
        );
        if !orphaned.is_empty() {
            wired.detail = Some(format!("{} has no usable destination", orphaned.join(", ")));
            wired.hint = Some("Open the job and choose where it should back up to.".into());
        }
        checks.push(wired);

        // destinations, only when there is a vault to resolve them against
        if locked {
            checks.push(check(
                "dest.reachable",
                "Destinations are reachable",
                CheckStatus::Skipped,
            ));
        } else {
            for destination in &config.destinations {
                if !destination.enabled {
                    continue;
                }
                let id = format!("dest.reachable.{}", destination.id);
                match &destination.kind {
                    DestinationKind::LocalMirror { path }
                    | DestinationKind::LocalRepository { path }
                    | DestinationKind::OneDrive { path, .. } => {
                        let exists = path.exists();
                        let mut c = check(
                            &id,
                            &format!("\"{}\" is reachable", destination.name),
                            if exists { CheckStatus::Pass } else { CheckStatus::Fail },
                        );
                        if !exists {
                            c.hint = Some(
                                "The folder is missing. Reconnect the drive, or point the \
                                 destination somewhere else."
                                    .into(),
                            );
                        }
                        checks.push(c);
                    }
                    DestinationKind::S3 { .. } => {
                        // A network round trip per destination would make
                        // `doctor` take minutes on a laptop with a dozen
                        // buckets. `dest.test` is the deliberate, per
                        // destination version of this check.
                        checks.push(check(
                            &id,
                            &format!("\"{}\" is reachable", destination.name),
                            CheckStatus::Skipped,
                        ));
                    }
                }
            }
        }

        // service and autostart
        let service = self.runtime.service_status();
        let mut svc = check(
            "service.healthy",
            "The background service is healthy",
            match (service.installed, service.state) {
                (false, _) => CheckStatus::Skipped,
                (true, platform::ServiceState::Running) => CheckStatus::Pass,
                _ => CheckStatus::Fail,
            },
        );
        svc.detail = service.detail.clone();
        checks.push(svc);

        let autostart_spec = platform::autostart::AutostartSpec::current().ok();
        if let Some(spec) = &autostart_spec {
            let status = platform::autostart::status(spec).ok();
            let needs_repair = status.as_ref().map(|s| s.state.needs_repair()).unwrap_or(false);
            let mut c = check(
                "autostart.healthy",
                "Start at login points at this build",
                if needs_repair { CheckStatus::Warn } else { CheckStatus::Pass },
            );
            c.fixable = needs_repair;
            if needs_repair && fix {
                match platform::autostart::heal(spec) {
                    Ok(Some(event)) => {
                        self.runtime.record_event(event);
                        fixed.push("autostart.healthy".to_string());
                        c.status = CheckStatus::Pass;
                        c.fixable = false;
                    }
                    Ok(None) => {}
                    Err(e) => c.detail = Some(e.to_string()),
                }
            }
            checks.push(c);
        }

        // Warnings do not clear `ok`; only failures do.
        let ok = !checks.iter().any(|c| c.status == CheckStatus::Fail);
        Ok(DoctorReply { ok, checks, fixed })
    }

    // -- jobs -------------------------------------------------------------

    async fn list_jobs(&self, _ctx: &RequestContext, include_disabled: bool) -> Result<JobsReply> {
        let config = self.config().await;
        let jobs = config.jobs.iter().filter(|j| include_disabled || j.enabled).cloned().collect();
        Ok(JobsReply { jobs })
    }

    async fn get_job(&self, _ctx: &RequestContext, job: String) -> Result<JobReply> {
        let config = self.config().await;
        Ok(JobReply { job: Box::new(resolve_job(&config, &job)?.clone()) })
    }

    async fn create_job(&self, _ctx: &RequestContext, job: Box<Job>) -> Result<JobReply> {
        let mut created = *job;
        // The daemon assigns the id, whatever the client sent: a client that
        // could choose ids could overwrite a job by guessing one.
        created.id = Uuid::new_v4();
        created.created_at = Utc::now();
        let name = created.name.clone();
        let id = created.id;
        let stored = created.clone();
        self.commit(move |config| {
            if config.jobs.iter().any(|j| j.name.eq_ignore_ascii_case(&stored.name)) {
                return Err(Error::Validation(format!(
                    "a job called \"{}\" already exists",
                    stored.name
                )));
            }
            config.jobs.push(stored);
            Ok(())
        })
        .await?;
        self.runtime.record_event(
            Event::info("job.created", format!("Job \"{name}\" was created.")).with_job(id),
        );
        let config = self.config().await;
        Ok(JobReply {
            job: Box::new(
                config.job(&id).cloned().ok_or_else(|| Error::JobNotFound(id.to_string()))?,
            ),
        })
    }

    async fn update_job(&self, _ctx: &RequestContext, job: Box<Job>) -> Result<JobReply> {
        let replacement = *job;
        let id = replacement.id;
        let name = replacement.name.clone();
        let stored = replacement.clone();
        self.commit(move |config| {
            let slot = config
                .jobs
                .iter_mut()
                .find(|j| j.id == stored.id)
                .ok_or_else(|| Error::JobNotFound(stored.id.to_string()))?;
            // `created_at` is history, not intent: a client that round-trips a
            // job must not be able to rewrite when it was made.
            let created_at = slot.created_at;
            *slot = stored;
            slot.created_at = created_at;
            Ok(())
        })
        .await?;
        self.runtime.record_event(
            Event::info("job.updated", format!("Job \"{name}\" was updated.")).with_job(id),
        );
        let config = self.config().await;
        Ok(JobReply {
            job: Box::new(
                config.job(&id).cloned().ok_or_else(|| Error::JobNotFound(id.to_string()))?,
            ),
        })
    }

    async fn delete_job(&self, _ctx: &RequestContext, job: String) -> Result<AckReply> {
        let config = self.config().await;
        let target = resolve_job(&config, &job)?.clone();
        // Stopping first, so a job cannot be deleted out from under a run that
        // is still writing to a repository.
        if let Some(scheduler) = self.runtime.scheduler() {
            let _ = scheduler.cancel_job(target.id);
        }
        let id = target.id;
        self.commit(move |config| {
            config.jobs.retain(|j| j.id != id);
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info(
            "job.deleted",
            format!("Job \"{}\" was deleted. Its snapshots were kept.", target.name),
        ));
        Ok(AckReply {})
    }

    async fn set_job_enabled(
        &self,
        _ctx: &RequestContext,
        job: String,
        enabled: bool,
    ) -> Result<JobReply> {
        let config = self.config().await;
        let id = resolve_job(&config, &job)?.id;
        self.commit(move |config| {
            let slot = config.job_mut(&id).ok_or_else(|| Error::JobNotFound(id.to_string()))?;
            slot.enabled = enabled;
            Ok(())
        })
        .await?;
        let config = self.config().await;
        let updated = config.job(&id).cloned().ok_or_else(|| Error::JobNotFound(id.to_string()))?;
        self.runtime.record_event(
            Event::info(
                "job.enabled",
                format!(
                    "Job \"{}\" was {}.",
                    updated.name,
                    if enabled { "enabled" } else { "disabled" }
                ),
            )
            .with_job(id),
        );
        Ok(JobReply { job: Box::new(updated) })
    }

    async fn run_job(
        &self,
        _ctx: &RequestContext,
        job: String,
        dry_run: bool,
    ) -> Result<StartedReply> {
        let config = self.config().await;
        let target = resolve_job(&config, &job)?.clone();
        if self.runtime.store.lock().await.is_locked() {
            return Err(Error::Locked);
        }

        if dry_run {
            return super::dryrun::start(&self.runtime, target).await;
        }

        let scheduler = self.runtime.require_scheduler()?;
        let run_id = scheduler.run_now(target.id, Trigger::Manual).await?;

        // A queued-but-not-started run is a normal answer, not a failure: it
        // means `max_parallel_jobs` is already satisfied.
        let status = scheduler.status().await.ok();
        let started =
            status.as_ref().map(|s| s.running.values().any(|r| *r == run_id)).unwrap_or(true);
        let note = (!started).then(|| {
            "Queued behind another backup; it will start when a slot frees up.".to_string()
        });
        Ok(StartedReply { run_id, started, note })
    }

    async fn stop_run(&self, _ctx: &RequestContext, run_id: Uuid) -> Result<StoppedReply> {
        // Idempotent by contract: stopping a run that already finished is a
        // success with an empty list, not an error.
        let Some(job_id) = self.runtime.job_for_run(&run_id) else {
            return Ok(StoppedReply { stopped: vec![] });
        };
        let scheduler = self.runtime.require_scheduler()?;
        scheduler.cancel_job(job_id)?;
        Ok(StoppedReply { stopped: vec![run_id] })
    }

    async fn stop_all_runs(&self, _ctx: &RequestContext) -> Result<StoppedReply> {
        let runs = self.runtime.active_runs();
        let scheduler = self.runtime.require_scheduler()?;
        let mut stopped = Vec::new();
        for run in runs {
            if scheduler.cancel_job(run.job_id).is_ok() {
                stopped.push(run.run_id);
            }
        }
        Ok(StoppedReply { stopped })
    }

    async fn job_history(
        &self,
        _ctx: &RequestContext,
        job: Option<String>,
        limit: u32,
    ) -> Result<RunsReply> {
        let filter = match &job {
            Some(needle) => {
                let config = self.config().await;
                Some(resolve_job(&config, needle)?.id)
            }
            None => None,
        };
        // 0 means "the daemon's own cap"; anything larger is clamped rather
        // than refused, so a client asking for a million gets two hundred.
        let cap = if limit == 0 {
            superbackup_core::state::MAX_HISTORY
        } else {
            (limit as usize).min(superbackup_core::state::MAX_HISTORY)
        };
        let persisted = self.runtime.persisted.lock().await;
        let runs = persisted
            .history
            .iter()
            .filter(|r| filter.is_none_or(|id| r.job_id == id))
            .take(cap)
            .cloned()
            .collect();
        Ok(RunsReply { runs })
    }

    // -- destinations -----------------------------------------------------

    async fn list_destinations(&self, _ctx: &RequestContext) -> Result<DestinationsReply> {
        Ok(DestinationsReply { destinations: self.config().await.destinations })
    }

    async fn get_destination(
        &self,
        _ctx: &RequestContext,
        destination: String,
    ) -> Result<DestinationReply> {
        let config = self.config().await;
        Ok(DestinationReply {
            destination: Box::new(resolve_destination(&config, &destination)?.clone()),
        })
    }

    async fn create_destination(
        &self,
        _ctx: &RequestContext,
        destination: Box<Destination>,
    ) -> Result<DestinationReply> {
        let mut created = *destination;
        created.id = Uuid::new_v4();
        created.created_at = Utc::now();
        // A generated passphrase needs a handle before anything can store one
        // against it, and the handle has to name the destination that owns it.
        if created.kind.is_repository() && created.passphrase_ref.is_none() {
            let derived = created
                .encryption
                .as_ref()
                .map(|e| e.passphrase_source)
                .unwrap_or(superbackup_core::model::PassphraseSource::Generated);
            if derived != superbackup_core::model::PassphraseSource::DerivedFromMaster {
                created.passphrase_ref = Some(SecretRef::new("repo-passphrase", &created.id));
            }
        }
        let id = created.id;
        let name = created.name.clone();
        let stored = created.clone();
        self.commit(move |config| {
            if config.destinations.iter().any(|d| d.name.eq_ignore_ascii_case(&stored.name)) {
                return Err(Error::Validation(format!(
                    "a destination called \"{}\" already exists",
                    stored.name
                )));
            }
            config.destinations.push(stored);
            Ok(())
        })
        .await?;
        self.runtime.record_event(
            Event::info("dest.created", format!("Destination \"{name}\" was added."))
                .with_destination(id),
        );
        let config = self.config().await;
        Ok(DestinationReply {
            destination: Box::new(config.destination(&id).cloned().ok_or_else(|| {
                Error::Validation(format!("destination {id} vanished during creation"))
            })?),
        })
    }

    async fn update_destination(
        &self,
        _ctx: &RequestContext,
        destination: Box<Destination>,
    ) -> Result<DestinationReply> {
        let replacement = *destination;
        let id = replacement.id;
        let stored = replacement.clone();
        self.commit(move |config| {
            let slot =
                config.destinations.iter_mut().find(|d| d.id == stored.id).ok_or_else(|| {
                    Error::Validation(format!("no destination with id {}", stored.id))
                })?;
            let created_at = slot.created_at;
            // The passphrase handle is not the client's to change: repointing
            // it at another destination's secret would open the wrong
            // repository, or silently orphan this one's key.
            let passphrase_ref = slot.passphrase_ref.clone();
            *slot = stored;
            slot.created_at = created_at;
            slot.passphrase_ref = passphrase_ref;
            Ok(())
        })
        .await?;
        let config = self.config().await;
        Ok(DestinationReply {
            destination: Box::new(config.destination(&id).cloned().ok_or_else(|| {
                Error::Validation(format!("destination {id} vanished during update"))
            })?),
        })
    }

    async fn delete_destination(
        &self,
        _ctx: &RequestContext,
        destination: String,
        force: bool,
    ) -> Result<AckReply> {
        let config = self.config().await;
        let target = resolve_destination(&config, &destination)?.clone();
        let users: Vec<String> =
            config.jobs_using(&target.id).iter().map(|j| j.name.clone()).collect();
        if !users.is_empty() && !force {
            return Err(Error::Validation(format!(
                "\"{}\" is still used by {} ({}). Delete it with force to remove it from those \
                 jobs.",
                target.name,
                users.len(),
                users.join(", ")
            )));
        }
        let id = target.id;
        self.commit(move |config| {
            config.destinations.retain(|d| d.id != id);
            for job in &mut config.jobs {
                job.destination_ids.retain(|d| *d != id);
            }
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::warn(
            "dest.deleted",
            format!(
                "Destination \"{}\" was removed from the configuration. Its stored data was not \
                 touched.",
                target.name
            ),
        ));
        Ok(AckReply {})
    }

    async fn test_destination(
        &self,
        _ctx: &RequestContext,
        destination: String,
    ) -> Result<ProbeReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let target = resolve_destination(&config, &destination)?.clone();
        let started = std::time::Instant::now();

        // A folder mirror has no repository, so the honest test is "can I
        // write a file here?" rather than a kopia round trip.
        if let DestinationKind::LocalMirror { path } = &target.kind {
            let (reachable, writable, detail) = probe_directory(path).await;
            return Ok(ProbeReply {
                reachable,
                writable,
                latency_ms: Some(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64),
                detail,
            });
        }

        let (_, driver) = self.driver_for(&destination).await?;
        let ctx = RunContext::new();
        let result = driver.test_connection(&ctx).await;
        let latency = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        Ok(match result {
            Ok(test) => ProbeReply {
                reachable: true,
                // Connecting proves the credentials are accepted; kopia has no
                // read-only connect, so a successful connect is a write probe.
                writable: true,
                latency_ms: Some(latency),
                detail: Some(test.summary()),
            },
            Err(e) => ProbeReply {
                reachable: false,
                writable: false,
                latency_ms: Some(latency),
                detail: Some(e.message),
            },
        })
    }

    async fn create_repository(
        &self,
        _ctx: &RequestContext,
        destination: String,
        encryption: Option<EncryptionSettings>,
    ) -> Result<RepositoryReply> {
        self.require_unlocked().await?;

        // Encryption settings belong to the destination, and the driver reads
        // them from it, so an override is stored first and then used.
        if let Some(settings) = encryption {
            let config = self.config().await;
            let id = resolve_destination(&config, &destination)?.id;
            self.commit(move |config| {
                let slot = config
                    .destinations
                    .iter_mut()
                    .find(|d| d.id == id)
                    .ok_or_else(|| Error::Validation(format!("no destination with id {id}")))?;
                slot.encryption = Some(settings);
                Ok(())
            })
            .await?;
        }

        // A repository with a generated passphrase needs one *before* the
        // repository exists, and it must be written to the vault first: a
        // repository whose password is not in the vault is unopenable.
        self.ensure_repository_passphrase(&destination).await?;

        let (target, driver) = self.driver_for(&destination).await?;
        let ctx = RunContext::new();
        let created = match driver.connect_repository(&ctx).await {
            Ok(()) => false,
            Err(e) if e.failure == superbackup_core::kopia::KopiaFailure::RepositoryNotFound => {
                driver.create_repository(&ctx).await.map_err(kopia_to_error)?;
                driver.connect_repository(&ctx).await.map_err(kopia_to_error)?;
                true
            }
            Err(e) => return Err(kopia_to_error(e)),
        };
        let status = driver.repository_status(&ctx).await.ok();
        self.runtime.record_event(
            Event::info(
                if created { "repo.created" } else { "repo.connected" },
                format!(
                    "{} the repository at \"{}\".",
                    if created { "Created" } else { "Connected to" },
                    target.name
                ),
            )
            .with_destination(target.id),
        );
        self.runtime.publish_status().await;
        Ok(RepositoryReply {
            destination_id: target.id,
            connected: true,
            repository_id: status.and_then(|s| s.unique_id),
            created,
        })
    }

    async fn connect_repository(
        &self,
        _ctx: &RequestContext,
        destination: String,
    ) -> Result<RepositoryReply> {
        self.require_unlocked().await?;
        let (target, driver) = self.driver_for(&destination).await?;
        let ctx = RunContext::new();
        driver.connect_repository(&ctx).await.map_err(kopia_to_error)?;
        let status = driver.repository_status(&ctx).await.ok();
        self.runtime.record_event(
            Event::info(
                "repo.connected",
                format!("Connected to the repository at \"{}\".", target.name),
            )
            .with_destination(target.id),
        );
        Ok(RepositoryReply {
            destination_id: target.id,
            connected: true,
            repository_id: status.and_then(|s| s.unique_id),
            created: false,
        })
    }

    async fn disconnect_repository(
        &self,
        _ctx: &RequestContext,
        destination: String,
    ) -> Result<RepositoryReply> {
        let config = self.config().await;
        let target = resolve_destination(&config, &destination)?.clone();
        // Disconnecting must work with a locked vault: it is how a user gets
        // out of trouble, and it needs no secret. Best effort against kopia,
        // then remove the per-destination config file either way.
        if !self.runtime.store.lock().await.is_locked() {
            if let Ok((_, driver)) = self.driver_for(&destination).await {
                let _ = driver.disconnect_repository(&RunContext::new()).await;
            }
        }
        let config_file = self.runtime.paths.kopia_config_for(&target.id);
        if config_file.exists() {
            if let Err(e) = std::fs::remove_file(&config_file) {
                tracing::warn!(error = %e, "could not remove the kopia config file");
            }
        }
        self.runtime.record_event(
            Event::info(
                "repo.disconnected",
                format!("Disconnected from the repository at \"{}\".", target.name),
            )
            .with_destination(target.id),
        );
        Ok(RepositoryReply {
            destination_id: target.id,
            connected: false,
            repository_id: None,
            created: false,
        })
    }

    async fn destination_stats(
        &self,
        _ctx: &RequestContext,
        destination: String,
        refresh: bool,
    ) -> Result<StorageStatsReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let target = resolve_destination(&config, &destination)?.clone();

        if !refresh {
            if let Some(cached) = self.runtime.cached_stats(&target.id) {
                return Ok(cached);
            }
        }

        // A folder mirror has no repository to interrogate; the honest answer
        // is the size on disk, which nothing here is willing to walk. Report
        // what is known and no more.
        if !target.kind.is_repository() {
            let reply = StorageStatsReply {
                destination_id: target.id,
                snapshot_count: 0,
                logical_bytes: None,
                stored_bytes: None,
                last_snapshot_at: None,
                computed_at: Utc::now(),
            };
            self.runtime.cache_stats(reply.clone());
            return Ok(reply);
        }

        let (_, driver) = self.driver_for(&destination).await?;
        let ctx = RunContext::new();
        let snapshots = driver.list_snapshots(None, true, &ctx).await.map_err(kopia_to_error)?;
        let blobs = driver.blob_stats(&ctx).await.ok();
        let logical: u64 =
            snapshots.iter().filter_map(|s| s.totals().map(|(_, bytes)| bytes)).sum();
        let reply = StorageStatsReply {
            destination_id: target.id,
            snapshot_count: snapshots.len() as u64,
            logical_bytes: (logical > 0).then_some(logical),
            stored_bytes: blobs.map(|b| b.total_bytes),
            last_snapshot_at: snapshots.iter().filter_map(|s| s.start_time).max(),
            computed_at: Utc::now(),
        };
        self.runtime.cache_stats(reply.clone());
        Ok(reply)
    }

    // -- providers --------------------------------------------------------

    async fn list_providers(&self, _ctx: &RequestContext) -> Result<ProvidersReply> {
        Ok(ProvidersReply { providers: self.config().await.providers })
    }

    async fn get_provider(&self, _ctx: &RequestContext, provider: String) -> Result<ProviderReply> {
        let config = self.config().await;
        Ok(ProviderReply { provider: Box::new(resolve_provider(&config, &provider)?.clone()) })
    }

    async fn create_provider(
        &self,
        _ctx: &RequestContext,
        provider: Box<StorageProvider>,
    ) -> Result<ProviderReply> {
        let mut created = *provider;
        created.id = Uuid::new_v4();
        created.created_at = Utc::now();
        // Credential handles name the provider that owns them, so they can
        // only be minted once the id is known. `ProviderKind` has one variant
        // today; the match keeps this correct when a second one arrives.
        match &mut created.kind {
            ProviderKind::S3 { credentials, .. } => {
                *credentials = superbackup_core::model::S3Credentials::for_provider(&created.id);
            }
        }
        let id = created.id;
        let stored = created.clone();
        self.commit(move |config| {
            if config.providers.iter().any(|p| p.name.eq_ignore_ascii_case(&stored.name)) {
                return Err(Error::Validation(format!(
                    "a provider called \"{}\" already exists",
                    stored.name
                )));
            }
            config.providers.push(stored);
            Ok(())
        })
        .await?;
        let config = self.config().await;
        Ok(ProviderReply {
            provider: Box::new(config.provider(&id).cloned().ok_or_else(|| {
                Error::Validation(format!("provider {id} vanished during creation"))
            })?),
        })
    }

    async fn update_provider(
        &self,
        _ctx: &RequestContext,
        provider: Box<StorageProvider>,
    ) -> Result<ProviderReply> {
        let replacement = *provider;
        let id = replacement.id;
        let stored = replacement.clone();
        self.commit(move |config| {
            let slot =
                config.providers.iter_mut().find(|p| p.id == stored.id).ok_or_else(|| {
                    Error::Validation(format!("no provider with id {}", stored.id))
                })?;
            let created_at = slot.created_at;
            // Credential handles are the daemon's, not the client's — see
            // `update_destination` for the same reasoning.
            let ProviderKind::S3 { credentials, .. } = &slot.kind;
            let existing = credentials.clone();
            *slot = stored;
            slot.created_at = created_at;
            let ProviderKind::S3 { credentials, .. } = &mut slot.kind;
            *credentials = existing;
            Ok(())
        })
        .await?;
        let config = self.config().await;
        Ok(ProviderReply {
            provider: Box::new(config.provider(&id).cloned().ok_or_else(|| {
                Error::Validation(format!("provider {id} vanished during update"))
            })?),
        })
    }

    async fn delete_provider(
        &self,
        _ctx: &RequestContext,
        provider: String,
        force: bool,
    ) -> Result<AckReply> {
        let config = self.config().await;
        let target = resolve_provider(&config, &provider)?.clone();
        let users = config.destinations_using(&target.id);
        if !users.is_empty() && !force {
            return Err(Error::Validation(format!(
                "\"{}\" is still used by {} destination(s). Delete it with force to remove it \
                 anyway.",
                target.name,
                users.len()
            )));
        }
        let id = target.id;
        let handles: Vec<SecretRef> = target.secret_refs().into_iter().cloned().collect();
        self.commit(move |config| {
            config.providers.retain(|p| p.id != id);
            Ok(())
        })
        .await?;
        // Forget the credentials, but only when the vault is open: silently
        // skipping is better than refusing the delete, and the orphan
        // collector will catch them later.
        let mut store = self.runtime.store.lock().await;
        if !store.is_locked() {
            for handle in handles {
                if let Err(e) = store.vault_file_mut().vault_mut().remove(&handle) {
                    tracing::warn!(error = %e, "could not remove a provider credential");
                }
            }
            if let Err(e) = store.vault_file_mut().save() {
                tracing::warn!(error = %e, "could not save the vault after deleting a provider");
            }
        }
        drop(store);
        self.runtime.record_event(Event::info(
            "provider.deleted",
            format!("Storage provider \"{}\" was deleted.", target.name),
        ));
        Ok(AckReply {})
    }

    async fn test_provider(&self, _ctx: &RequestContext, provider: String) -> Result<ProbeReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let target = resolve_provider(&config, &provider)?.clone();
        // A provider is only reachable through a destination that uses it —
        // kopia has no "test these credentials" command of its own — so the
        // probe borrows the first destination that inherits them.
        let Some(destination) = config.destinations_using(&target.id).first().map(|d| (*d).clone())
        else {
            return Ok(ProbeReply {
                reachable: false,
                writable: false,
                latency_ms: None,
                detail: Some(
                    "No destination uses this provider yet, so there is nothing to test against. \
                     Add a bucket destination first."
                        .into(),
                ),
            });
        };
        self.test_destination(_ctx, destination.id.to_string()).await
    }

    async fn provider_used_by(
        &self,
        _ctx: &RequestContext,
        provider: String,
    ) -> Result<UsedByReply> {
        let config = self.config().await;
        let target = resolve_provider(&config, &provider)?;
        let destinations: Vec<Reference> = config
            .destinations_using(&target.id)
            .iter()
            .map(|d| Reference { id: d.id, name: d.name.clone() })
            .collect();
        let destination_ids: BTreeSet<Uuid> = destinations.iter().map(|d| d.id).collect();
        let jobs = config
            .jobs
            .iter()
            .filter(|j| j.destination_ids.iter().any(|id| destination_ids.contains(id)))
            .map(|j| Reference { id: j.id, name: j.name.clone() })
            .collect();
        Ok(UsedByReply { destinations, jobs })
    }

    async fn rotate_provider_credentials(
        &self,
        _ctx: &RequestContext,
        provider: String,
        access_key_id: SecretString,
        secret_access_key: SecretString,
        session_token: Option<SecretString>,
    ) -> Result<ProviderReply> {
        self.require_unlocked().await?;
        if access_key_id.expose().is_empty() || secret_access_key.expose().is_empty() {
            return Err(Error::Validation("an access key and secret are both required".into()));
        }
        let config = self.config().await;
        let target = resolve_provider(&config, &provider)?.clone();
        // `ProviderKind` has one variant today, so this is an irrefutable
        // binding rather than a match; a second variant would make it a
        // compile error here, which is where the decision belongs.
        let ProviderKind::S3 { credentials, .. } = &target.kind;

        let mut store = self.runtime.store.lock().await;
        store.put_secret(credentials.access_key_ref.clone(), access_key_id.into_secret())?;
        store.put_secret(credentials.secret_key_ref.clone(), secret_access_key.into_secret())?;
        match (&credentials.session_token_ref, session_token) {
            (Some(handle), Some(token)) => {
                store.put_secret(handle.clone(), token.into_secret())?;
            }
            (Some(handle), None) => {
                // Rotating to a long-lived key pair: the stale session token
                // must go, or kopia will present an expired one.
                let handle = handle.clone();
                if let Err(e) = store.vault_file_mut().vault_mut().remove(&handle) {
                    tracing::warn!(error = %e, "could not clear the old session token");
                }
                store.vault_file_mut().save()?;
            }
            _ => {}
        }
        drop(store);

        let id = target.id;
        self.commit(move |config| {
            if let Some(slot) = config.providers.iter_mut().find(|p| p.id == id) {
                slot.last_verified_at = None;
            }
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info(
            "provider.rotated",
            format!("The credentials for \"{}\" were replaced.", target.name),
        ));
        let config = self.config().await;
        Ok(ProviderReply {
            provider: Box::new(config.provider(&id).cloned().ok_or_else(|| {
                Error::Validation(format!("provider {id} vanished during rotation"))
            })?),
        })
    }

    // -- snapshots and restore -------------------------------------------

    async fn list_snapshots(
        &self,
        _ctx: &RequestContext,
        destination: String,
        job: Option<String>,
        limit: u32,
    ) -> Result<SnapshotsReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let job_filter = match &job {
            Some(needle) => Some(resolve_job(&config, needle)?.id),
            None => None,
        };
        let (target, driver) = self.driver_for(&destination).await?;
        let manifests = driver.browse_roots(&RunContext::new()).await.map_err(kopia_to_error)?;
        let cap = if limit == 0 { MAX_SNAPSHOTS } else { (limit as usize).min(MAX_SNAPSHOTS) };
        let snapshots = manifests
            .iter()
            .map(|m| snapshot_info(m, target.id))
            .filter(|s| job_filter.is_none_or(|id| s.job_id == Some(id)))
            .take(cap)
            .collect();
        Ok(SnapshotsReply { snapshots })
    }

    async fn browse_snapshot(
        &self,
        _ctx: &RequestContext,
        destination: String,
        snapshot: String,
        path: String,
    ) -> Result<ListingReply> {
        self.require_unlocked().await?;
        let target = browse_target(&snapshot, &path)?;
        let (_, driver) = self.driver_for(&destination).await?;
        let entries =
            driver.list_directory(&target, &RunContext::new()).await.map_err(kopia_to_error)?;
        let truncated = entries.len() > MAX_LISTING_ENTRIES;
        Ok(ListingReply {
            path: normalised_path(&path),
            entries: entries
                .iter()
                .take(MAX_LISTING_ENTRIES)
                .map(|e| SnapshotEntry {
                    name: e.name.clone(),
                    kind: entry_kind(e),
                    size_bytes: e.size,
                    modified_at: e.modified_at,
                    object_id: (!e.object_id.is_empty()).then(|| e.object_id.clone()),
                })
                .collect(),
            truncated,
        })
    }

    async fn restore_snapshot(
        &self,
        _ctx: &RequestContext,
        destination: String,
        snapshot: String,
        path: String,
        target: PathBuf,
        conflict: ConflictPolicy,
        dry_run: bool,
    ) -> Result<StartedReply> {
        self.require_unlocked().await?;
        if !target.is_absolute() {
            return Err(Error::Validation("the restore target must be an absolute path".into()));
        }
        let options = restore_options(conflict)?;
        let source = browse_target(&snapshot, &path)?;
        let (dest, driver) = self.driver_for(&destination).await?;

        if dry_run {
            // kopia's restore has no dry run. Listing the entry is the closest
            // honest thing: it proves the snapshot and path resolve, without
            // writing anything.
            driver.list_directory(&source, &RunContext::new()).await.map_err(kopia_to_error)?;
            return Ok(StartedReply {
                run_id: Uuid::new_v4(),
                started: false,
                note: Some(format!(
                    "Dry run: {} would be restored to {}. Nothing was written.",
                    source,
                    target.display()
                )),
            });
        }

        let run_id = Uuid::new_v4();
        let runtime = Arc::clone(&self.runtime);
        let destination_id = dest.id;
        let destination_name = dest.name.clone();
        let target_display = target.display().to_string();
        let restore_into = target.clone();
        tokio::spawn(async move {
            let (events, rx) = EventSink::channel(64);
            let (handle, token) = cancellation();
            // A restore ends when the daemon does; nothing else can cancel it,
            // so it is bound to the shutdown signal rather than to a run.
            let mut shutdown = runtime.subscribe_shutdown();
            tokio::spawn(async move {
                if shutdown.recv().await.is_ok() {
                    handle.cancel();
                }
            });

            let pump_runtime = Arc::clone(&runtime);
            let pump = tokio::spawn(async move {
                let mut rx = rx;
                while let Some(event) = rx.recv().await {
                    if let KopiaEvent::Progress(progress) = event {
                        pump_runtime.publish(StreamItem::Progress {
                            run_id,
                            job_id: run_id,
                            destination_id,
                            status: RunStatus::Running,
                            progress: Box::new(progress),
                        });
                    }
                }
            });

            let ctx = RunContext::new().with_cancel(token).with_events(events);
            let outcome = driver.restore(&source, &restore_into, &options, &ctx).await;
            // `ctx` owns an `EventSink`; the pump ends only when the last one
            // is dropped, so joining before this would wait for ever.
            drop(ctx);
            let _ = pump.await;

            let event = match &outcome {
                Ok(result) => {
                    runtime.publish(StreamItem::Progress {
                        run_id,
                        job_id: run_id,
                        destination_id,
                        status: RunStatus::Succeeded,
                        progress: Box::new(result.progress.clone()),
                    });
                    Event::info(
                        "restore.finished",
                        format!(
                            "Restore from \"{destination_name}\" to {target_display} finished."
                        ),
                    )
                }
                Err(e) => Event::error(
                    "restore.failed",
                    format!("Restore from \"{destination_name}\" failed: {}", e.message),
                ),
            }
            .with_destination(destination_id)
            .with_run(run_id);
            runtime.record_event(event);
        });

        Ok(StartedReply {
            run_id,
            started: true,
            note: Some(format!("Restoring into {}.", target.display())),
        })
    }

    async fn delete_snapshot(
        &self,
        _ctx: &RequestContext,
        destination: String,
        snapshot: String,
    ) -> Result<AckReply> {
        self.require_unlocked().await?;
        if snapshot.trim().is_empty() {
            return Err(Error::Validation("no snapshot was named".into()));
        }
        let (target, driver) = self.driver_for(&destination).await?;
        // `confirm: true` is this call site stating, in code, that a human
        // asked. The IPC command is itself the confirmation.
        driver
            .delete_snapshot(&snapshot, true, &RunContext::new())
            .await
            .map_err(kopia_to_error)?;
        self.runtime.record_event(
            Event::warn(
                "snapshot.deleted",
                format!("A snapshot was deleted from \"{}\".", target.name),
            )
            .with_destination(target.id),
        );
        Ok(AckReply {})
    }

    // -- vault ------------------------------------------------------------

    async fn unlock_vault(
        &self,
        _ctx: &RequestContext,
        passphrase: SecretString,
    ) -> Result<UnlockedReply> {
        let secret = passphrase.into_secret();
        {
            let mut store = self.runtime.store.lock().await;
            if store.is_locked() {
                store.unlock(&secret)?;
            }
        }
        super::lifecycle::on_unlocked(&self.runtime, secret).await;
        Ok(self.unlocked_reply(true))
    }

    async fn lock_vault(&self, _ctx: &RequestContext) -> Result<UnlockedReply> {
        super::lifecycle::lock(&self.runtime, "vault.locked", "The vault was locked.").await;
        Ok(self.unlocked_reply(false))
    }

    async fn vault_is_unlocked(&self, _ctx: &RequestContext) -> Result<UnlockedReply> {
        let unlocked = !self.runtime.store.lock().await.is_locked();
        Ok(self.unlocked_reply(unlocked))
    }

    async fn change_passphrase(
        &self,
        _ctx: &RequestContext,
        current: SecretString,
        replacement: SecretString,
    ) -> Result<AckReply> {
        self.require_unlocked().await?;
        super::rekey::change_passphrase(
            &self.runtime,
            current.into_secret(),
            replacement.into_secret(),
        )
        .await?;
        Ok(AckReply {})
    }

    async fn set_secret(
        &self,
        _ctx: &RequestContext,
        secret_ref: SecretRef,
        value: SecretString,
    ) -> Result<AckReply> {
        self.require_unlocked().await?;
        if value.expose().is_empty() {
            return Err(Error::Validation("refusing to store an empty secret".into()));
        }
        if secret_ref.as_str().trim().is_empty() {
            return Err(Error::Validation("a secret needs a handle".into()));
        }
        let handle = secret_ref.clone();
        let mut store = self.runtime.store.lock().await;
        store.put_secret(handle, value.into_secret())?;
        drop(store);
        // The handle is not secret; the value is, and it appears nowhere here.
        self.runtime.record_event(Event::info(
            "vault.secret_set",
            format!("A credential was stored under {secret_ref}."),
        ));
        Ok(AckReply {})
    }

    async fn list_secret_refs(&self, _ctx: &RequestContext) -> Result<SecretRefsReply> {
        self.require_unlocked().await?;
        let store = self.runtime.store.lock().await;
        let refs = store.vault().list_refs()?;
        Ok(SecretRefsReply { refs })
    }

    // -- control ----------------------------------------------------------

    async fn pause(
        &self,
        _ctx: &RequestContext,
        seconds: Option<u64>,
        reason: Option<String>,
    ) -> Result<PauseReply> {
        // A pause longer than a year is a typo, not an intention.
        let until = match seconds {
            Some(s) if s > 0 => {
                Some(Utc::now() + ChronoDuration::seconds(s.min(365 * 24 * 3600) as i64))
            }
            _ => None,
        };
        let state = PauseState { paused: true, until, reason: reason.clone() };
        self.commit(move |config| {
            config.settings.pause = state;
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info(
            "control.paused",
            match until {
                Some(at) => format!("Backups paused until {}.", at.format("%H:%M")),
                None => "Backups paused until you resume them.".to_string(),
            },
        ));
        Ok(self.pause_reply().await)
    }

    async fn resume(&self, _ctx: &RequestContext) -> Result<PauseReply> {
        self.commit(|config| {
            config.settings.pause = PauseState::default();
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info("control.resumed", "Backups resumed."));
        Ok(self.pause_reply().await)
    }

    async fn pause_state(&self, _ctx: &RequestContext) -> Result<PauseReply> {
        Ok(self.pause_reply().await)
    }

    async fn set_bandwidth(
        &self,
        _ctx: &RequestContext,
        bandwidth: superbackup_core::model::BandwidthSettings,
    ) -> Result<BandwidthReply> {
        let stored = bandwidth.clone();
        self.commit(move |config| {
            config.settings.bandwidth = stored;
            Ok(())
        })
        .await?;
        Ok(BandwidthReply { bandwidth })
    }

    async fn reload_config(&self, _ctx: &RequestContext) -> Result<AckReply> {
        let mut store = self.runtime.store.lock().await;
        store.reload_config()?;
        let config = store.config().clone();
        drop(store);
        self.runtime.push_config(&config);
        self.runtime.publish_status().await;
        self.runtime.record_event(Event::info(
            "config.reloaded",
            "The configuration was re-read from disk.",
        ));
        Ok(AckReply {})
    }

    async fn shutdown(&self, _ctx: &RequestContext, stop_runs: bool) -> Result<AckReply> {
        self.runtime.record_event(Event::info(
            "daemon.stopping",
            if stop_runs {
                "Shutting down and stopping any running backups."
            } else {
                "Shutting down once running backups finish."
            },
        ));
        // Returns immediately; the reply must reach the client before the
        // socket closes, so the actual teardown happens on the main task.
        self.runtime.request_shutdown(stop_runs);
        Ok(AckReply {})
    }

    // -- settings ---------------------------------------------------------

    async fn get_settings(&self, _ctx: &RequestContext) -> Result<SettingsReply> {
        Ok(SettingsReply { settings: Box::new(self.config().await.settings) })
    }

    async fn update_settings(
        &self,
        _ctx: &RequestContext,
        settings: Box<Settings>,
    ) -> Result<SettingsReply> {
        let before = self.config().await.settings;
        let replacement = (*settings).clone();
        self.commit(move |config| {
            config.settings = replacement;
            Ok(())
        })
        .await?;
        let after = self.config().await.settings;

        self.runtime.notifier.update_settings(after.notifications.clone());
        // Re-arm the auto-lock against the new interval, so shortening it
        // takes effect now rather than after the next unlock.
        let unlocked = !self.runtime.store.lock().await.is_locked();
        if unlocked {
            self.runtime.arm_auto_lock(after.auto_lock_minutes);
        }

        // The setting *is* the consent, so it takes effect on the transition
        // rather than at the next unlock: switching it off must destroy what
        // is already cached, and switching it on must cache what the daemon is
        // holding right now — otherwise a user who ticks the box and reboots
        // finds it did nothing.
        if before.use_os_keychain != after.use_os_keychain {
            self.apply_keychain_setting(after.use_os_keychain, unlocked).await;
        }

        if let Some(event) = settings_changed_event(&before, &after) {
            self.runtime.record_event(event);
        }
        Ok(SettingsReply { settings: Box::new(after) })
    }

    // -- remote configuration ---------------------------------------------

    async fn remote_pull(&self, _ctx: &RequestContext) -> Result<RemoteStatusReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let source = config
            .remote
            .clone()
            .ok_or_else(|| Error::Remote("no remote configuration repository is set up".into()))?;

        let token = match &source.auth {
            superbackup_core::model::RemoteAuth::Token { token_ref } => {
                let store = self.runtime.store.lock().await;
                store.secret(token_ref)?
            }
            _ => None,
        };
        let client = superbackup_core::remote::RemoteClient::new()?;
        let fetched = client.fetch(&source, token.as_ref()).await?;

        // Verification needs the passphrase, not the derived keys: the fetched
        // vault is a *different* file with a different salt.
        let passphrase = self.runtime.master()?;
        let store = self.runtime.store.lock().await;
        let plan = superbackup_core::remote::verify_pull(
            &fetched,
            store.config(),
            store.vault(),
            &source,
            &passphrase,
        )?;
        drop(store);

        let changes = plan.diff.total();
        self.runtime.set_pull(Some(plan));
        self.runtime.record_event(Event::info(
            "remote.pulled",
            format!(
                "Fetched the shared configuration: {changes} change(s) are waiting to be applied."
            ),
        ));
        Ok(self.remote_status_reply(Some(format!("{changes} change(s) available"))).await)
    }

    async fn remote_diff(&self, _ctx: &RequestContext) -> Result<RemoteDiffReply> {
        self.require_unlocked().await?;
        let plan = self.runtime.pull().ok_or_else(|| {
            Error::Remote("nothing has been pulled yet; run `remote.pull` first".into())
        })?;
        Ok(RemoteDiffReply { changes: describe_diff(&plan.diff), remote_commit: plan.sha.clone() })
    }

    async fn remote_apply(&self, _ctx: &RequestContext) -> Result<RemoteStatusReply> {
        self.require_unlocked().await?;
        let plan = self.runtime.pull().ok_or_else(|| {
            Error::Remote(
                "nothing has been pulled yet. Pull and review the changes before applying them."
                    .into(),
            )
        })?;
        let changes = plan.diff.total();

        let mut store = self.runtime.store.lock().await;
        superbackup_core::remote::apply_pull(&mut store, &plan)?;
        // The pulled vault is sealed under the same master passphrase, so the
        // daemon can reopen it without asking again — and must, or the machine
        // would be silently locked out until the next manual unlock.
        let passphrase = self.runtime.master()?;
        store.unlock(&passphrase)?;
        let config = store.config().clone();
        drop(store);

        self.runtime.set_pull(None);
        self.runtime.push_config(&config);
        self.runtime.publish_status().await;
        self.runtime.record_event(Event::info(
            "remote.applied",
            format!("Applied {changes} change(s) from the shared configuration."),
        ));
        Ok(self.remote_status_reply(Some("applied".into())).await)
    }

    async fn remote_push(
        &self,
        _ctx: &RequestContext,
        message: Option<String>,
    ) -> Result<RemoteStatusReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let source = config
            .remote
            .clone()
            .ok_or_else(|| Error::Remote("no remote configuration repository is set up".into()))?;
        if !source.allow_push {
            return Err(Error::Remote(
                "publishing is disabled for this remote; enable it in Settings first".into(),
            ));
        }
        let superbackup_core::model::RemoteAuth::Token { token_ref } = &source.auth else {
            return Err(Error::Remote(
                "publishing needs a personal access token; add one in Settings".into(),
            ));
        };
        let store_token = {
            let store = self.runtime.store.lock().await;
            store.require_secret(token_ref)?
        };

        let (payload, sha) = {
            let mut store = self.runtime.store.lock().await;
            let payload = store.publication_payload()?;
            (payload, source.last_known_commit.clone())
        };
        let request = superbackup_core::remote::PushRequest::new(
            payload,
            message.unwrap_or_else(|| {
                format!("superbackup: configuration from {}", config.machine.label)
            }),
        )
        .replacing(sha)
        .confirmed();
        let client = superbackup_core::remote::RemoteClient::new()?;
        let new_sha = client.push(&source, &store_token, &request).await?;

        self.commit(move |config| {
            if let Some(remote) = &mut config.remote {
                remote.last_known_commit = Some(new_sha.clone());
            }
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info(
            "remote.pushed",
            "The sealed configuration was published to the shared repository.",
        ));
        Ok(self.remote_status_reply(Some("published".into())).await)
    }

    // -- service ----------------------------------------------------------

    async fn service_status(&self, _ctx: &RequestContext) -> Result<ServiceReply> {
        Ok(self.service_reply(None))
    }

    async fn install_service(&self, _ctx: &RequestContext) -> Result<ServiceReply> {
        let options = platform::ServiceOptions::current(&self.runtime.paths)?;
        if options.requires_elevation() && !platform::service::is_elevated() {
            return Err(Error::Service(
                "installing the background service needs administrator rights. Start superbackup \
                 with \"Run as administrator\" and try again."
                    .into(),
            ));
        }
        platform::service::install(&options)?;
        // Started here rather than left to the next boot: an installed service
        // that is not running looks identical to a broken one.
        if let Err(e) = platform::service::start(&options.name, options.scope) {
            tracing::warn!(error = %e, "the service was installed but did not start");
        }
        self.runtime.invalidate_service_status();
        self.runtime.record_event(Event::info(
            "service.installed",
            "The background service was installed.",
        ));
        Ok(self.service_reply(Some(service_reach_summary(&self.config().await))))
    }

    async fn uninstall_service(&self, _ctx: &RequestContext) -> Result<ServiceReply> {
        let options = platform::ServiceOptions::current(&self.runtime.paths)?;
        if options.requires_elevation() && !platform::service::is_elevated() {
            return Err(Error::Service(
                "removing the background service needs administrator rights.".into(),
            ));
        }
        let _ = platform::service::stop(&options.name, options.scope);
        platform::service::uninstall(&options.name, options.scope)?;
        self.runtime.invalidate_service_status();
        self.runtime.record_event(Event::info(
            "service.uninstalled",
            "The background service was removed. Your configuration and backups were not touched.",
        ));
        Ok(self.service_reply(None))
    }

    async fn set_autostart(&self, _ctx: &RequestContext, enabled: bool) -> Result<ServiceReply> {
        if enabled {
            platform::autostart::enable(&platform::autostart::AutostartSpec::current()?)?;
        } else {
            platform::autostart::disable()?;
        }
        self.commit(move |config| {
            config.settings.start_at_login = enabled;
            Ok(())
        })
        .await?;
        Ok(self.service_reply(None))
    }

    // -- streaming --------------------------------------------------------

    fn event_stream(
        &self,
        _ctx: &RequestContext,
        topics: &[Topic],
    ) -> Result<broadcast::Receiver<StreamItem>> {
        Ok(self.runtime.subscribe_stream(topics))
    }
}

// ---------------------------------------------------------------------------
// Support
// ---------------------------------------------------------------------------

impl DaemonHandler {
    /// React to `use_os_keychain` being switched on or off.
    ///
    /// Turning it off is destructive and immediate: the cache is the thing the
    /// user just withdrew consent for. Turning it on is best effort — the
    /// vault may be locked, in which case there is nothing to cache yet and
    /// the next unlock will do it.
    async fn apply_keychain_setting(&self, enabled: bool, unlocked: bool) {
        if !enabled {
            match super::keychain::forget(&self.runtime.paths).await {
                Ok(()) => self.runtime.record_event(Event::info(
                    "vault.keychain_cleared",
                    "The saved passphrase was removed. superbackup will ask for it again.",
                )),
                Err(e) => self.runtime.record_event(Event::new(
                    Severity::Warning,
                    "vault.keychain_not_cleared",
                    format!(
                        "The saved passphrase could not be removed from the keychain ({e}).                          Remove the superbackup entry by hand."
                    ),
                )),
            }
            return;
        }
        if !unlocked {
            // Nothing to cache: the passphrase is only in memory while open.
            return;
        }
        let Ok(passphrase) = self.runtime.master() else { return };
        match super::keychain::store(&self.runtime.paths, &passphrase).await {
            Ok(()) => self.runtime.record_event(Event::info(
                "vault.keychain_stored",
                "Your passphrase is now saved in this machine's credential store.",
            )),
            Err(e) => self.runtime.record_event(Event::new(
                Severity::Warning,
                "vault.keychain_failed",
                format!("{} ({e})", super::keychain::explain_unavailable()),
            )),
        }
    }

    /// Make sure a repository destination has a passphrase in the vault before
    /// its repository is created.
    ///
    /// Order matters and is not negotiable: a repository whose passphrase was
    /// never written to the vault is unopenable, and no amount of retrying
    /// recovers it. So the secret is generated and *saved* first, and only
    /// then does kopia create anything.
    async fn ensure_repository_passphrase(&self, needle: &str) -> Result<()> {
        let mut store = self.runtime.store.lock().await;
        let destination = resolve_destination(store.config(), needle)?.clone();
        if !destination.kind.is_repository() {
            return Ok(());
        }
        // A derived passphrase needs nothing stored; `destination_passphrase`
        // computes it from the master key.
        let source = destination
            .encryption
            .as_ref()
            .map(|e| e.passphrase_source)
            .unwrap_or(superbackup_core::model::PassphraseSource::Generated);
        if source == superbackup_core::model::PassphraseSource::DerivedFromMaster {
            return Ok(());
        }
        let Some(handle) = destination.passphrase_ref.clone() else {
            return Err(Error::Config(format!(
                "destination \"{}\" has no passphrase handle",
                destination.name
            )));
        };
        if store.secret(&handle)?.is_some() {
            return Ok(());
        }
        if source == superbackup_core::model::PassphraseSource::UserSupplied {
            return Err(Error::Config(format!(
                "\"{}\" is set to use a passphrase you supply, but none has been stored. Set it \
                 with `vault.set_secret` before creating the repository.",
                destination.name
            )));
        }
        // 32 random bytes, base64: 256 bits of entropy, and safe to pass
        // through an environment variable.
        let generated = superbackup_core::crypto::base64_for_upload(
            &superbackup_core::crypto::random_bytes(32)?,
        );
        store.put_secret(handle, Secret::from_string(generated))?;
        Ok(())
    }
}

/// Turn a kopia failure into the crate's error type for an IPC reply.
fn kopia_to_error(e: superbackup_core::kopia::KopiaError) -> Error {
    match e.failure.error_code() {
        ErrorCode::BadPassphrase => Error::BadPassphrase,
        ErrorCode::RepoNotConnected => Error::RepoNotConnected(e.message),
        ErrorCode::RepoExists => Error::RepoExists(e.message),
        ErrorCode::KopiaMissing => Error::KopiaMissing,
        ErrorCode::JobCancelled => Error::JobCancelled(e.message),
        _ => Error::Kopia { status: e.status.unwrap_or(-1), stderr: e.message },
    }
}

/// Can a folder be written to? Used by `dest.test` for folder mirrors, where
/// there is no repository to connect to.
async fn probe_directory(path: &std::path::Path) -> (bool, bool, Option<String>) {
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        if let Err(e) = std::fs::create_dir_all(&path) {
            return (false, false, Some(format!("{e}")));
        }
        let probe = path.join(".superbackup-write-test");
        match std::fs::write(&probe, b"superbackup") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                (true, true, None)
            }
            Err(e) => (true, false, Some(format!("{e}"))),
        }
    })
    .await;
    result.unwrap_or((false, false, Some("the write probe did not complete".into())))
}

/// Render a [`superbackup_core::remote::ConfigDiff`] as protocol changes.
fn describe_diff(diff: &superbackup_core::remote::ConfigDiff) -> Vec<ConfigChange> {
    let mut out = Vec::new();
    let sets = [
        ("job", &diff.jobs),
        ("destination", &diff.destinations),
        ("provider", &diff.providers),
        ("project", &diff.projects),
    ];
    for (entity, set) in sets {
        for (kind, changes) in [
            (ChangeKind::Added, &set.added),
            (ChangeKind::Removed, &set.removed),
            (ChangeKind::Modified, &set.modified),
        ] {
            for change in changes {
                out.push(ConfigChange {
                    kind,
                    entity: entity.to_string(),
                    id: Some(change.id),
                    name: change.name.clone(),
                    summary: match kind {
                        ChangeKind::Added => format!("{entity} \"{}\" would be added", change.name),
                        ChangeKind::Removed => {
                            format!("{entity} \"{}\" would be removed", change.name)
                        }
                        ChangeKind::Modified => {
                            format!("{entity} \"{}\" would change", change.name)
                        }
                    },
                });
            }
        }
    }
    if diff.machine_identity_changes {
        out.push(ConfigChange {
            kind: ChangeKind::Modified,
            entity: "settings".into(),
            id: None,
            name: "This machine".into(),
            summary: "The shared configuration describes a different machine identity; the local \
                      one is kept."
                .into(),
        });
    }
    out
}

/// An honest sentence about what a LocalSystem service can and cannot reach.
///
/// Shown after `service.install` rather than left for the user to discover
/// three days later when their OneDrive destination has never been written.
pub fn service_reach_summary(config: &superbackup_core::model::Config) -> String {
    let account = platform::service::ServiceAccount::LocalSystem;
    let mut unsupported = Vec::new();
    let mut degraded = Vec::new();
    for destination in &config.destinations {
        match platform::service::destination_support(
            &destination.kind,
            &account,
            ServiceScope::System,
        ) {
            platform::service::SupportLevel::Supported => {}
            platform::service::SupportLevel::Degraded { .. } => {
                degraded.push(destination.name.clone())
            }
            platform::service::SupportLevel::Unsupported { .. } => {
                unsupported.push(destination.name.clone())
            }
        }
    }
    if unsupported.is_empty() && degraded.is_empty() {
        return "Every destination is reachable from the service.".to_string();
    }
    let mut parts = Vec::new();
    if !unsupported.is_empty() {
        parts.push(format!(
            "The service cannot reach {}; back those up from the tray app instead.",
            unsupported.join(", ")
        ));
    }
    if !degraded.is_empty() {
        parts.push(format!("{} will work with caveats.", degraded.join(", ")));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_targets_are_built_without_letting_a_path_escape() {
        assert_eq!(browse_target("k123", "").ok(), Some("k123".to_string()));
        assert_eq!(browse_target("k123", "/src/main.rs").ok(), Some("k123/src/main.rs".into()));
        assert_eq!(browse_target("k123", "src\\lib").ok(), Some("k123/src/lib".into()));
        assert!(browse_target("k123", "../../etc/passwd").is_err());
        assert!(browse_target("k1/23", "x").is_err());
        assert!(browse_target("  ", "x").is_err());
    }

    #[test]
    fn conflict_policies_map_onto_flags_kopia_actually_has() {
        let skip = restore_options(ConflictPolicy::Skip).expect("skip");
        assert!(skip.skip_existing && !skip.overwrite_files);
        let over = restore_options(ConflictPolicy::Overwrite).expect("overwrite");
        assert!(over.overwrite_files && !over.skip_existing);
        let fail = restore_options(ConflictPolicy::Fail).expect("fail");
        assert!(!fail.overwrite_files && !fail.skip_existing);
        // Refused rather than approximated: see the function's documentation.
        assert!(restore_options(ConflictPolicy::KeepBoth).is_err());
    }

    #[test]
    fn paths_are_normalised_for_the_reply() {
        assert_eq!(normalised_path("/a//b/./c"), "a/b/c");
        assert_eq!(normalised_path(""), "");
        assert_eq!(normalised_path("a\\b"), "a/b");
    }

    #[test]
    fn a_service_summary_names_what_local_system_cannot_reach() {
        let mut config = superbackup_core::model::Config::default();
        config
            .destinations
            .push(superbackup_core::engine::testing::test_mirror("mapped", r"H:\backups"));
        let summary = service_reach_summary(&config);
        if cfg!(windows) {
            assert!(summary.contains("mapped"), "{summary}");
        }
    }
}
