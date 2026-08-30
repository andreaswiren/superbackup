//! The engine's view of the machine.
//!
//! [`engine::Environment`] is deliberately a trait rather than a call into
//! `crate::platform`, so the engine stays testable. This is the production
//! implementation: it answers "is the vault unlocked" from the daemon's own
//! authoritative flag, and the two power questions from
//! [`platform::power`].
//!
//! ## Why the power answers are cached
//!
//! `Environment` is documented as "cheap and non-blocking: called on the
//! scheduler's hot path". `platform::power::connection_cost()` is a COM call
//! on Windows and shells out to `nmcli` on some Linux systems — neither is
//! something to do inside a scheduler tick. So a background task samples both
//! on a slow timer and stores the answers in atomics, and the trait methods
//! are two relaxed loads.
//!
//! The cost of that staleness is bounded and one-directional: at worst the
//! scheduler starts a backup a few seconds after the user unplugged the
//! laptop. The cost of *not* caching is a scheduler that can block for the
//! length of a `nmcli` invocation on every gate evaluation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use superbackup_core::engine::Environment;
use superbackup_core::platform;

/// How often the power and network sample is refreshed.
///
/// Twenty seconds is far below any schedule granularity superbackup supports
/// (the finest is one minute) and far above the cost of the syscalls.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(20);

/// The real [`Environment`].
#[derive(Debug)]
pub struct DaemonEnvironment {
    unlocked: AtomicBool,
    metered: AtomicBool,
    battery: AtomicBool,
}

impl Default for DaemonEnvironment {
    fn default() -> Self {
        DaemonEnvironment::new()
    }
}

impl DaemonEnvironment {
    /// Start locked. The daemon flips this the moment `vault.unlock`
    /// succeeds; starting the other way round would let the very first
    /// scheduler tick try to run a job it cannot possibly complete.
    pub fn new() -> DaemonEnvironment {
        DaemonEnvironment {
            unlocked: AtomicBool::new(false),
            metered: AtomicBool::new(false),
            battery: AtomicBool::new(false),
        }
    }

    /// Record the vault's lock state. Called by `vault.unlock`, `vault.lock`
    /// and the auto-lock timer, and by nothing else — one flag, one writer
    /// path, so the scheduler and the status snapshot cannot disagree.
    pub fn set_vault_unlocked(&self, unlocked: bool) {
        self.unlocked.store(unlocked, Ordering::Relaxed);
    }

    /// Take one sample of the machine's power and network state.
    ///
    /// Public so the daemon can prime the values before the scheduler's first
    /// tick rather than spending the first twenty seconds believing the
    /// machine is on mains and unmetered.
    pub fn sample(&self) {
        let metered = platform::power::connection_cost().should_skip();
        let battery = platform::power::power_status().should_skip_on_battery();
        self.metered.store(metered, Ordering::Relaxed);
        self.battery.store(battery, Ordering::Relaxed);
    }

    /// Spawn the refresher. The task ends when the last `Arc` is dropped,
    /// which happens at shutdown, so it needs no cancellation of its own.
    pub fn spawn_sampler(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let Some(env) = weak.upgrade() else { return };
                // The platform calls are blocking; keeping them off the
                // runtime's worker threads is the whole point of the cache.
                let sampled = tokio::task::spawn_blocking(move || {
                    env.sample();
                })
                .await;
                if sampled.is_err() {
                    tracing::debug!("power sampler task failed; will retry");
                }
            }
        })
    }
}

impl Environment for DaemonEnvironment {
    fn vault_unlocked(&self) -> bool {
        self.unlocked.load(Ordering::Relaxed)
    }
    fn on_metered_connection(&self) -> bool {
        self.metered.load(Ordering::Relaxed)
    }
    fn on_battery(&self) -> bool {
        self.battery.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_locked_so_the_first_tick_cannot_run_a_job() {
        let env = DaemonEnvironment::new();
        assert!(!env.vault_unlocked());
        env.set_vault_unlocked(true);
        assert!(env.vault_unlocked());
        env.set_vault_unlocked(false);
        assert!(!env.vault_unlocked());
    }

    #[test]
    fn sampling_never_panics_on_this_machine() {
        let env = DaemonEnvironment::new();
        env.sample();
        // Both answers are booleans; the point is that reading them is safe
        // whatever the platform reported.
        let _ = (env.on_battery(), env.on_metered_connection());
    }
}
