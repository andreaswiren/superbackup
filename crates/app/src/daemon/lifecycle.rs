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

use superbackup_core::secret::Secret;
use superbackup_core::state::{Event, Severity, Trigger};

use super::runtime::Runtime;

/// How often the auto-lock timer checks its deadline.
///
/// The deadline itself is minutes away, so a fifteen-second granularity costs
/// nothing and keeps the task's wakeups cheap on a laptop.
const AUTO_LOCK_TICK: Duration = Duration::from_secs(15);

/// Everything that must become true when the vault opens.
pub async fn on_unlocked(runtime: &Arc<Runtime>, passphrase: Secret) {
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

    runtime.record_event(Event::info("vault.unlocked", "The vault was unlocked."));

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
            lock(
                &runtime,
                "vault.auto_locked",
                "The vault locked itself after a period of inactivity.",
            )
            .await;
        }
    })
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
    // A footgun worth naming once at start-up rather than leaving to be
    // discovered: locking clears the cache, so an auto-lock interval means
    // unattended unlocking survives only until the first timeout.
    if auto_lock_minutes > 0 {
        runtime.record_event(Event::new(
            Severity::Warning,
            "vault.keychain_auto_lock",
            format!(
                "Your passphrase is remembered, but auto-lock is set to {auto_lock_minutes}                  minutes and locking forgets it. Set auto-lock to 0 for unattended backups."
            ),
        ));
    }
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
    on_unlocked(runtime, passphrase).await;
    true
}
