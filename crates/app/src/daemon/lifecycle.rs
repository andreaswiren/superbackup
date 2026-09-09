//! Unlocking, locking, and the auto-lock timer.
//!
//! The vault's lock state is the single most consequential flag in the daemon:
//! it gates every scheduled run, the tray icon, and half the IPC surface. It
//! is therefore changed in exactly two places — [`on_unlocked`] and [`lock`] —
//! and both of them update *every* consequence in one go:
//!
//! | Consequence | Why it cannot be forgotten |
//! |---|---|
//! | `Environment::vault_unlocked` | the scheduler's gate reads this, not the store |
//! | the retained master passphrase | `remote.pull` and the keychain need it; it must not outlive the unlock |
//! | the auto-lock deadline | an unlock with no deadline never re-locks |
//! | runs blocked while locked | the scheduler *drops* them; only this re-queues them |
//! | the status broadcast | the tray shows a padlock until it is told otherwise |
//!
//! Splitting those across call sites is how an application ends up unlocked
//! according to the GUI and locked according to the scheduler.

use std::sync::Arc;
use std::time::Duration;

use superbackup_core::platform::disk::{DiskLevel, DiskReport};
use superbackup_core::platform::notify::{Notification, NotificationKind};
use superbackup_core::secret::Secret;
use superbackup_core::state::{Event, Severity, Trigger};

use super::runtime::Runtime;

/// How often the auto-lock timer checks its deadline.
///
/// The deadline itself is minutes away, so a fifteen-second granularity costs
/// nothing and keeps the task's wakeups cheap on a laptop.
const AUTO_LOCK_TICK: Duration = Duration::from_secs(15);

/// How the vault came to be open.
///
/// The distinction the two-tier model rests on. Holding the keys and having a
/// person present are different facts, and on a machine that saves its
/// passphrase so backups run at 3am the first is true all the time while the
/// second is almost never true. Conflating them would make the saved
/// passphrase a way to hand whoever reaches the logged-in account the whole
/// configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// Somebody typed the master passphrase. A person is present.
    ByHand,
    /// Restored from the platform keychain, without anybody being asked.
    /// Backups run; changing anything still needs the passphrase.
    Unattended,
}

/// Everything that must become true when the vault opens.
pub async fn on_unlocked(runtime: &Arc<Runtime>, passphrase: Secret, how: Opened) {
    let settings = {
        let store = runtime.store.lock().await;
        store.config().settings.clone()
    };

    runtime.environment.set_vault_unlocked(true);
    runtime.arm_auto_lock(settings.auto_lock_minutes);

    // Opt-in, and every failure degrades to "we will ask again" — but says so,
    // because the user's real position after a failure is "scheduled backups
    // are skipped until I unlock by hand", and they can only act on that if
    // they are told.
    if settings.use_os_keychain {
        if let Err(e) = super::keychain::store(&runtime.paths, &passphrase).await {
            tracing::warn!(error = %e, "could not cache the passphrase in the OS keychain");
            runtime.record_event(Event::new(
                Severity::Warning,
                "vault.keychain_failed",
                format!("{} ({e})", super::keychain::explain_unavailable()),
            ));
        }
    }
    runtime.remember_master(passphrase);

    // The second tier, and the one line in the daemon that grants it.
    if how == Opened::ByHand {
        runtime.confirm();
    }

    runtime.record_event(match how {
        Opened::ByHand => Event::info("vault.unlocked", "The vault was unlocked."),
        Opened::Unattended => Event::info(
            "vault.unlocked_unattended",
            "The vault was opened with the saved passphrase, so backups can run. Changing \
             anything still asks for it.",
        ),
    });

    // Runs the scheduler dropped while the vault was shut. The scheduler
    // drains its queue rather than holding them (see `Runtime::blocked_by_lock`),
    // so if this did not exist, unlocking at 09:00 would leave a 02:00 backup
    // waiting until 02:00 tomorrow.
    let blocked = runtime.take_blocked_by_lock();
    if let Some(scheduler) = runtime.scheduler() {
        // Hand over the effective config first: `replace_config` makes the
        // scheduler resync, which is also what re-arms `run_missed_on_start`
        // catch-up for a job whose whole schedule elapsed while locked.
        let config = { runtime.store.lock().await.config().clone() };
        runtime.push_config(&config);

        for job_id in blocked {
            let name = config.job(&job_id).map(|j| j.name.clone());
            match scheduler.run_now(job_id, Trigger::CatchUp).await {
                Ok(run_id) => {
                    tracing::info!(%job_id, %run_id, "re-queued a run that the lock had blocked");
                    if let Some(name) = name {
                        runtime.record_event(
                            Event::info(
                                "job.unblocked",
                                format!("\"{name}\" was queued now that the vault is open."),
                            )
                            .with_job(job_id),
                        );
                    }
                }
                // Already running, or the job has since been deleted. Neither
                // is worth telling the user about.
                Err(e) => tracing::debug!(%job_id, error = %e, "blocked run was not re-queued"),
            }
        }
    }

    runtime.publish_status().await;
}

/// Everything that must become true when the vault closes.
///
/// `kind` and `message` are the activity-log line, so the auto-lock timer and
/// a deliberate `vault.lock` are distinguishable in the history.
///
/// The cached passphrase goes with it. "Lock" has to mean locked: a machine
/// that re-opens itself the instant it is asked to shut has not locked
/// anything, and leaving the cache in place would make `vault.lock` a lie on
/// exactly the installs that opted into caching.
pub async fn lock(runtime: &Arc<Runtime>, kind: &str, message: &str) {
    {
        let mut store = runtime.store.lock().await;
        store.lock();
    }
    runtime.environment.set_vault_unlocked(false);
    runtime.forget_master();
    runtime.disarm_auto_lock();
    if let Err(e) = super::keychain::forget_if_cached(&runtime.paths).await {
        tracing::warn!(error = %e, "could not clear the cached passphrase");
        runtime.record_event(Event::new(
            Severity::Warning,
            "vault.keychain_not_cleared",
            format!(
                "The vault was locked, but the saved passphrase could not be removed from the \
                 keychain ({e}). Remove the superbackup entry by hand if that matters to you."
            ),
        ));
    }
    runtime.record_event(Event::info(kind, message));
    runtime.publish_status().await;
}

/// The auto-lock timer.
///
/// Runs until shutdown. Deliberately does **not** lock while a backup is in
/// flight: the run holds resolved secrets already, but its *next* destination
/// would fail to resolve one, turning an idle-timeout into a failed backup.
/// The deadline is pushed out instead and the vault locks once the machine is
/// quiet, which is what the user meant by "lock when I am not using it".
pub fn spawn_auto_lock(runtime: Arc<Runtime>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut shutdown = runtime.subscribe_shutdown();
        let mut ticker = tokio::time::interval(AUTO_LOCK_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.recv() => return,
                _ = ticker.tick() => {}
            }
            if !runtime.auto_lock_due(chrono::Utc::now()) {
                continue;
            }
            if !runtime.active_runs().is_empty() {
                let minutes = {
                    let store = runtime.store.lock().await;
                    store.config().settings.auto_lock_minutes
                };
                runtime.arm_auto_lock(minutes);
                continue;
            }
            // Step down a tier, or lock, depending on what the user asked
            // this machine to be.
            //
            // A machine that saves its passphrase was told to back up
            // unattended, and locking it defeats that: it is how 51
            // consecutive scheduled runs came to be skipped. What idleness
            // should cost there is the *confirmation* — come back to the
            // window and it asks for the passphrase before it will let you
            // change anything, while the backups behind it never stopped.
            //
            // Without a saved passphrase there is no unattended running to
            // protect, and locking is what the setting has always meant.
            let saved = {
                let store = runtime.store.lock().await;
                store.config().settings.use_os_keychain
            };
            if saved {
                runtime.unconfirm();
                runtime.disarm_auto_lock();
                runtime.record_event(Event::info(
                    "vault.confirmation_expired",
                    "You have been away a while, so superbackup will ask for your master \
                     passphrase before the next change. Backups carried on as normal.",
                ));
                runtime.publish_status().await;
            } else {
                lock(
                    &runtime,
                    "vault.auto_locked",
                    "The vault locked itself after a period of inactivity.",
                )
                .await;
            }
        }
    })
}

/// How often the volumes are looked at.
///
/// Half an hour. A disk does not fill in seconds, and a check that ran every
/// tick would spin up sleeping drives to ask a question whose answer changes
/// slowly.
const DISK_TICK: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Watch the free space on every volume superbackup writes to.
///
/// # Why the daemon and not the window
///
/// The failure this catches happens overnight, to a machine nobody is looking
/// at, and the point is to have said something before the run that could not
/// write. A check that only ran while the interface was open would report the
/// problem exactly when the user could already see it.
///
/// # Why it does not repeat itself
///
/// A volume that is low is low every half hour until somebody clears it. The
/// level is remembered per volume and an event is written only when it
/// *changes*, so the activity log gets one line when a disk starts running out
/// and one when it recovers, rather than forty-eight a day that train the user
/// to ignore the log.
/// How often the integrity task wakes to see whether anything is due.
///
/// Waking is not checking. The gap between *checks* is
/// `IntegritySettings::interval_days`, measured per destination; this is only
/// how promptly a machine that has been asleep for a week notices.
const INTEGRITY_TICK: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Read the backups back, now and then, and say so if they cannot be read.
///
/// # Why this exists
///
/// A backup nobody has ever read back is a hope, not a backup. Every failure
/// this catches — bit rot on a disk, a bucket that has quietly dropped an
/// object, a sync client that truncated a file, a drive that is on its way
/// out — is silent by nature: the writes keep succeeding, the runs keep
/// reporting success, and the damage is discovered by the person trying to
/// restore, which is the worst possible moment to discover anything.
///
/// # Why it is light, and why that is not a compromise
///
/// `kopia snapshot verify` does two different things. It walks every
/// snapshot and confirms every object it references exists and the index
/// agrees — that is cheap, needs no downloads, and catches a repository that
/// has lost blobs. And it can download file contents and rehash them, which is
/// the only way to catch data that is present and *wrong*, and which costs
/// bandwidth and, on a bucket, money.
///
/// So the first is always done in full and the second is sampled. A
/// destination losing data will show it within a few cycles, and the check
/// stays cheap enough that nobody turns it off — which is the property that
/// actually matters, because a thorough check that gets disabled catches
/// nothing at all.
///
/// # Why one destination at a time
///
/// A machine with four destinations should not spend an afternoon reading all
/// four back. The oldest one that is due goes, and the next tick takes the
/// next: over a week every destination is covered, and no single hour is
/// noticeably busier than any other.
pub fn spawn_integrity_check(
    runtime: Arc<Runtime>,
    executor: Arc<dyn superbackup_core::engine::BackupExecutor>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut shutdown = runtime.subscribe_shutdown();
        let mut ticker = tokio::time::interval(INTEGRITY_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.recv() => return,
                _ = ticker.tick() => {}
            }
            verify_one_due(&runtime, executor.as_ref()).await;
        }
    })
}

/// Verify the destination that has gone longest without it, if any is due.
async fn verify_one_due(
    runtime: &Arc<Runtime>,
    executor: &dyn superbackup_core::engine::BackupExecutor,
) {
    let (settings, destinations) = {
        let store = runtime.store.lock().await;
        let config = store.config();
        (config.settings.integrity.clone(), config.destinations.clone())
    };
    if !settings.enabled {
        return;
    }
    // Never while the vault is shut: reading a repository needs its
    // passphrase. Not a failure — the machine is simply not in a position to
    // check right now, and it will be at the next tick.
    if runtime.store.lock().await.is_locked() {
        return;
    }
    // Nor while a backup is running. The point is to be unnoticeable, and
    // competing with the thing the user actually asked for is the opposite.
    if !runtime.active_runs().is_empty() {
        return;
    }
    if settings.only_when_convenient && !convenient(runtime) {
        return;
    }

    let now = chrono::Utc::now();
    let due = destinations
        .into_iter()
        .filter(|d| d.enabled && d.kind.is_repository())
        .filter(|d| settings.due(d.last_verified_at, now))
        // The one that has waited longest. `None` — never verified — sorts
        // first, which is right: a destination that has never been read back
        // is the one most likely to have never worked.
        .min_by_key(|d| d.last_verified_at);
    let Some(destination) = due else {
        return;
    };

    let name = destination.name.clone();
    let id = destination.id;
    // Progress goes nowhere on purpose. This runs unattended and hourly; the
    // interface learns the outcome from the event and from `last_verified_at`,
    // and a live byte counter for a check nobody asked to watch would only add
    // traffic to the status stream.
    let run_id = uuid::Uuid::new_v4();
    let (updates, _drain) = tokio::sync::mpsc::unbounded_channel();
    let progress = superbackup_core::engine::ProgressSink::new(
        updates,
        run_id,
        uuid::Uuid::nil(),
        id,
        Arc::new(superbackup_core::engine::clock::SystemClock::new()),
    );
    let request = superbackup_core::engine::VerifyRequest {
        run_id,
        destination: Arc::new(destination),
        sample_percent: settings.sample_percent / 100.0,
        progress,
        cancel: superbackup_core::engine::cancel::CancelToken::new(),
    };
    match executor.verify(request).await {
        Ok(outcome) if outcome.problems.is_empty() => {
            runtime.record_event(Event::info(
                "destination.verified",
                format!(
                    "\"{name}\" was read back and is intact ({} objects checked).",
                    outcome.blobs_checked
                ),
            ));
            mark_verified(runtime, id, now).await;
        }
        Ok(outcome) => {
            // Not marked verified. A destination that failed its check must
            // stay due, or the next tick would skip it for a week on the
            // strength of the run that found the damage.
            let detail = outcome.problems.join("\n");
            runtime.record_event(Event::new(
                Severity::Error,
                "destination.corrupt",
                format!(
                    "\"{name}\" did not read back cleanly. Backups written here may not be \
                     restorable.\n{detail}"
                ),
            ));
            super::events::notify(
                runtime,
                Notification::new(
                    NotificationKind::Failure,
                    "A backup destination did not verify",
                    format!(
                        "\"{name}\" could not be read back cleanly. Open superbackup to see what \
                         was wrong."
                    ),
                ),
            )
            .await;
        }
        Err(e) => {
            // Could not check, which is not the same as checked and bad. Said
            // once, at warning, and the destination stays due.
            runtime.record_event(Event::new(
                Severity::Warning,
                "destination.verify_failed",
                format!("\"{name}\" could not be checked this time: {e}"),
            ));
        }
    }
}

/// Is now a reasonable moment to read a few hundred megabytes back?
///
/// Battery and metered connections, the two the user is charged for. The
/// environment already answers both for the scheduler, so this asks the same
/// question the same way rather than inventing a second opinion.
fn convenient(runtime: &Arc<Runtime>) -> bool {
    use superbackup_core::engine::Environment;
    !runtime.environment.on_metered_connection() && !runtime.environment.on_battery()
}

/// Record that a destination read back cleanly.
async fn mark_verified(runtime: &Arc<Runtime>, id: uuid::Uuid, at: chrono::DateTime<chrono::Utc>) {
    let mut store = runtime.store.lock().await;
    let mut config = store.config().clone();
    if let Some(slot) = config.destinations.iter_mut().find(|d| d.id == id) {
        slot.last_verified_at = Some(at);
    }
    let _ = store.set_config(config);
}

pub fn spawn_disk_watch(runtime: Arc<Runtime>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut shutdown = runtime.subscribe_shutdown();
        let mut ticker = tokio::time::interval(DISK_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut seen: std::collections::HashMap<std::path::PathBuf, DiskLevel> =
            std::collections::HashMap::new();
        loop {
            tokio::select! {
                _ = shutdown.recv() => return,
                _ = ticker.tick() => {}
            }
            for report in disk_reports(&runtime).await {
                let previous = seen.insert(report.path.clone(), report.level);
                if previous == Some(report.level) {
                    continue;
                }
                match report.level {
                    DiskLevel::Fine => {
                        // Only worth saying when it *was* a problem, so a
                        // healthy machine writes nothing at all.
                        if matches!(previous, Some(DiskLevel::Low | DiskLevel::Critical)) {
                            runtime.record_event(Event::info(
                                "disk.recovered",
                                format!("{} has room again. {}", report.label, report.message()),
                            ));
                        }
                    }
                    DiskLevel::Low => {
                        runtime.record_event(Event::new(
                            Severity::Warning,
                            "disk.low",
                            format!("Running out of room. {}", report.message()),
                        ));
                    }
                    DiskLevel::Critical => {
                        runtime.record_event(Event::new(
                            Severity::Error,
                            "disk.critical",
                            format!(
                                "Almost out of room, and backups here will start failing. {}",
                                report.message()
                            ),
                        ));
                        super::events::notify(
                            &runtime,
                            Notification::new(
                                NotificationKind::Info,
                                "A backup disk is almost full",
                                report.message(),
                            ),
                        )
                        .await;
                    }
                }
            }
        }
    })
}

/// Every volume worth looking at, judged against the user's thresholds.
///
/// The destinations that are folders on this machine, plus superbackup's own
/// data directory — which holds the vault, the state and the staging folder,
/// and whose filling up breaks things that have nothing to do with any one
/// destination.
///
/// S3 destinations are absent on purpose: a bucket has no free space to read,
/// and inventing a figure for one would be worse than saying nothing.
pub async fn disk_reports(runtime: &Arc<Runtime>) -> Vec<DiskReport> {
    let config = { runtime.store.lock().await.config().clone() };
    let settings = config.settings.disk_space;
    if !settings.enabled {
        return Vec::new();
    }

    let mut out = Vec::new();
    // One report per *volume*, not per destination: three folders on D: are
    // one disk, and three identical warnings about it is two too many.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for destination in &config.destinations {
        if !destination.enabled {
            continue;
        }
        let Some(path) = destination.kind.local_path() else { continue };
        if !seen.insert(volume_key(path)) {
            continue;
        }
        if let Some(report) =
            superbackup_core::platform::disk::assess(path, &destination.name, &settings)
        {
            out.push(report);
        }
    }
    let own = runtime.paths.data_dir.clone();
    if seen.insert(volume_key(&own)) {
        if let Some(report) =
            superbackup_core::platform::disk::assess(&own, "Superbackup's own folder", &settings)
        {
            out.push(report);
        }
    }
    out
}

/// What counts as "the same disk" for the purpose of not saying it twice.
///
/// The path's root: `C:\` on Windows, `/` on Unix. Crude — two mount points
/// under `/` are one key when they are really two filesystems — but wrong in
/// the safe direction, because the cost is one warning instead of two rather
/// than a volume nobody was told about.
fn volume_key(path: &std::path::Path) -> String {
    path.components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Try to open the vault from the OS keychain at startup.
///
/// Returns true when it worked. Every way of failing leaves the user exactly
/// where they would have been without the feature — being asked for their
/// passphrase — but says so in the activity log rather than staying quiet,
/// because "scheduled backups are skipped until you unlock" is something they
/// can only act on if they are told.
pub async fn try_keychain_unlock(runtime: &Arc<Runtime>) -> bool {
    let (use_keychain, auto_lock_minutes) = {
        let store = runtime.store.lock().await;
        let settings = &store.config().settings;
        (settings.use_os_keychain, settings.auto_lock_minutes)
    };
    if !use_keychain {
        return false;
    }
    // This used to warn that auto-lock would discard the saved passphrase and
    // strand the machine, which it did: 51 consecutive scheduled runs were
    // skipped on the author's own PC for exactly that reason.
    //
    // It no longer does. With the passphrase saved, the auto-lock timer steps
    // down a tier rather than locking — it withdraws the confirmation, so
    // changing anything asks again, while the keys stay available and backups
    // keep running. There is nothing left to warn about.
    let _ = auto_lock_minutes;
    let passphrase = match super::keychain::load(&runtime.paths).await {
        Ok(Some(passphrase)) => passphrase,
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(error = %e, "the cached passphrase could not be read");
            runtime.record_event(Event::new(
                Severity::Warning,
                "vault.keychain_failed",
                format!("{} ({e})", super::keychain::explain_unavailable()),
            ));
            return false;
        }
    };
    let opened = {
        let mut store = runtime.store.lock().await;
        store.unlock(&passphrase).is_ok()
    };
    if !opened {
        // The cached passphrase no longer opens the vault — it was rotated
        // elsewhere. Drop it rather than leaving a stale secret in the
        // keyring for the next machine to trip over.
        let _ = super::keychain::forget(&runtime.paths).await;
        runtime.record_event(Event::new(
            Severity::Warning,
            "vault.keychain_stale",
            "The saved passphrase no longer opens the vault — it was probably changed on another machine — so it was discarded. superbackup will ask for the new one."
                .to_string(),
        ));
        return false;
    }
    on_unlocked(runtime, passphrase, Opened::Unattended).await;
    true
}
