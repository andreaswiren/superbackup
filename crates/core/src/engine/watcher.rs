//! `Schedule::OnChange`: run once the tree goes quiet.
//!
//! # The problem this has to survive
//!
//! A developer folder generates filesystem events in bursts of tens of
//! thousands: `npm install`, a Rust build, a `git checkout`. A naive watcher
//! does three things wrong with that, and all three are fatal:
//!
//! 1. It runs a backup per burst — or worse, per event.
//! 2. It spends the whole burst allocating and queueing events for paths that
//!    are excluded anyway, so `node_modules` alone can peg a core.
//! 3. It misses changes when the OS drops events, because every platform's
//!    watch API has a fixed-size buffer that overflows under exactly this
//!    load, and reports the overflow instead of the events.
//!
//! The answers here, in order:
//!
//! 1. [`ChangeDebouncer`] collapses a burst into one run, and enforces
//!    `min_interval_minutes` between runs on top of that.
//! 2. Exclusions are matched **before** the event reaches the debouncer, on
//!    the receiving thread, so an excluded path costs one glob match and no
//!    allocation downstream.
//! 3. An overflow is not treated as "nothing happened". It sets a rescan flag
//!    that fires a run with [`WatchTrigger::Rescan`], on the principle that
//!    the only safe response to "we do not know what changed" is to look.
//!
//! The debouncer is a pure state machine with no I/O and no clock of its own,
//! so all of that behaviour is tested deterministically; [`JobWatcher`] is the
//! thin shell that feeds it real events.

use crate::engine::cancel::CancelToken;
use crate::engine::clock::Clock;
use crate::engine::mirror::ExclusionMatcher;
use crate::model::Job;
use chrono::{DateTime, Duration, Utc};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

/// Why the watcher wants a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchTrigger {
    /// Changes were seen and the tree has been quiet for `debounce_seconds`.
    Debounced,
    /// The OS dropped events. What changed is unknown, so a full run is the
    /// only correct response.
    Rescan,
}

/// Collapses a storm of filesystem events into at most one run.
///
/// Pure: `on_event`, `on_overflow` and `poll` take the current instant as an
/// argument and touch nothing else, which is what makes "a `node_modules`
/// install produces exactly one backup" a unit test rather than a hope.
#[derive(Debug, Clone)]
pub struct ChangeDebouncer {
    debounce: Duration,
    min_interval: Duration,
    /// When the current burst started. Kept for diagnostics and for the
    /// maximum-delay guard below.
    burst_started: Option<DateTime<Utc>>,
    /// The most recent event in the current burst.
    last_event: Option<DateTime<Utc>>,
    last_fire: Option<DateTime<Utc>>,
    rescan: bool,
}

impl ChangeDebouncer {
    /// Hard ceiling on how long a continuously-churning tree can postpone its
    /// backup. Without it, a folder that is written to every few seconds all
    /// day — a log directory, an active build — would never go quiet and would
    /// never be backed up at all.
    pub const MAX_DEBOUNCE_MULTIPLIER: i32 = 10;

    pub fn new(debounce_seconds: u32, min_interval_minutes: u32) -> ChangeDebouncer {
        ChangeDebouncer {
            // A zero debounce would fire on every single event, which for a
            // watched build directory is thousands of runs a minute.
            debounce: Duration::seconds(debounce_seconds.max(1) as i64),
            min_interval: Duration::minutes(min_interval_minutes as i64),
            burst_started: None,
            last_event: None,
            last_fire: None,
            rescan: false,
        }
    }

    /// Seed the "last run" so a restart does not immediately re-fire a job
    /// that ran a minute ago.
    pub fn with_last_run(mut self, last_run: Option<DateTime<Utc>>) -> ChangeDebouncer {
        self.last_fire = last_run;
        self
    }

    /// Record a relevant change.
    pub fn on_event(&mut self, now: DateTime<Utc>) {
        self.burst_started.get_or_insert(now);
        self.last_event = Some(now);
    }

    /// Record that the OS lost events.
    pub fn on_overflow(&mut self, now: DateTime<Utc>) {
        self.rescan = true;
        self.on_event(now);
    }

    /// Should a run start now? Consumes the pending state when it says yes.
    pub fn poll(&mut self, now: DateTime<Utc>) -> Option<WatchTrigger> {
        let last_event = self.last_event?;
        let quiet_at = last_event + self.debounce;
        let deadline =
            self.burst_started.map(|start| start + self.debounce * Self::MAX_DEBOUNCE_MULTIPLIER);
        let quiet = now >= quiet_at || deadline.is_some_and(|d| now >= d);
        if !quiet {
            return None;
        }
        if let Some(last_fire) = self.last_fire {
            if now < last_fire + self.min_interval {
                // Still inside the minimum interval. The pending state is
                // deliberately *kept*, so the run happens as soon as the
                // interval expires rather than being lost.
                return None;
            }
        }
        let trigger = if self.rescan { WatchTrigger::Rescan } else { WatchTrigger::Debounced };
        self.rescan = false;
        self.last_event = None;
        self.burst_started = None;
        self.last_fire = Some(now);
        Some(trigger)
    }

    /// The next instant at which [`poll`](Self::poll) could return `Some`, or
    /// `None` when nothing is pending. The watcher task sleeps on this instead
    /// of polling.
    pub fn next_wake(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let last_event = self.last_event?;
        let mut at = last_event + self.debounce;
        if let Some(start) = self.burst_started {
            at = at.min(start + self.debounce * Self::MAX_DEBOUNCE_MULTIPLIER);
        }
        if let Some(last_fire) = self.last_fire {
            at = at.max(last_fire + self.min_interval);
        }
        Some(at.max(now))
    }

    /// True when a burst is being tracked.
    pub fn is_pending(&self) -> bool {
        self.last_event.is_some()
    }
}

/// A live watcher for one `OnChange` job.
///
/// Dropping it stops the watch: the handle owns a [`CancelToken`] that it
/// fires on drop, which is what makes "the user changed this job's sources"
/// a matter of replacing the entry in the scheduler's map.
#[derive(Debug)]
pub struct JobWatcher {
    cancel: CancelToken,
}

impl Drop for JobWatcher {
    fn drop(&mut self) {
        self.cancel.cancel(crate::engine::cancel::CancelReason::Shutdown);
    }
}

impl JobWatcher {
    /// Start watching `job`'s sources.
    ///
    /// Generic over the message type so that this module does not have to know
    /// the scheduler's private `Command` enum; `to_message` adapts.
    ///
    /// Failure to watch a path is reported and skipped rather than fatal: a
    /// job with three sources, one of which is an unplugged external drive,
    /// should still watch the other two.
    pub fn spawn<M, F>(
        job: Job,
        debounce_seconds: u32,
        min_interval_minutes: u32,
        clock: Arc<dyn Clock>,
        sender: tokio::sync::mpsc::UnboundedSender<M>,
        cancel: CancelToken,
        to_message: F,
    ) -> JobWatcher
    where
        M: Send + 'static,
        F: Fn(Uuid, WatchTrigger) -> M + Send + 'static,
    {
        let handle = JobWatcher { cancel: cancel.clone() };
        let matcher = match ExclusionMatcher::build(&job.exclusions) {
            Ok(m) => Arc::new(m),
            Err(e) => {
                tracing::warn!(job = %job.name, "invalid exclusions, watching unfiltered: {e}");
                // An unfiltered watch is noisy but correct; refusing to watch
                // at all would silently disable the job.
                match ExclusionMatcher::from_patterns(&[]) {
                    Ok(m) => Arc::new(m),
                    Err(_) => return handle,
                }
            }
        };

        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel::<WatchSignal>();
        let roots: Vec<PathBuf> = job.sources.iter().map(|s| s.path.clone()).collect();
        let job_name = job.name.clone();
        let job_id = job.id;

        // `notify` delivers on its own thread; the callback must be cheap and
        // must not block, so it filters and forwards, nothing more.
        let filter_roots = roots.clone();
        let filter_matcher = Arc::clone(&matcher);
        let forward = events_tx.clone();
        let watcher_result =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                match result {
                    Ok(event) => {
                        if event.paths.is_empty() {
                            // A pathless event is a "something happened, we do
                            // not know what" notification. Treat it as a
                            // dropped-event warning rather than ignoring it.
                            let _ = forward.send(WatchSignal::Overflow);
                            return;
                        }
                        if event
                            .paths
                            .iter()
                            .any(|p| is_relevant(p, &filter_roots, &filter_matcher))
                        {
                            let _ = forward.send(WatchSignal::Changed);
                        }
                    }
                    // Every backend reports buffer overflow as an error here.
                    // Missing changes silently is the one outcome that must
                    // not happen, so any watch error escalates to a rescan.
                    Err(e) => {
                        tracing::warn!("filesystem watch error, forcing a rescan: {e}");
                        let _ = forward.send(WatchSignal::Overflow);
                    }
                }
            });

        let mut watcher = match watcher_result {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(job = %job_name, "cannot start a filesystem watcher: {e}");
                return handle;
            }
        };
        {
            use notify::Watcher as _;
            for root in &roots {
                if let Err(e) = watcher.watch(root, notify::RecursiveMode::Recursive) {
                    tracing::warn!(job = %job_name, path = %root.display(), "cannot watch: {e}");
                }
            }
        }

        let mut debouncer = ChangeDebouncer::new(debounce_seconds, min_interval_minutes);
        tokio::spawn(async move {
            // Keep the notify watcher alive for exactly as long as this task.
            let _watcher = watcher;
            loop {
                let now = clock.now_utc();
                if let Some(trigger) = debouncer.poll(now) {
                    if sender.send(to_message(job_id, trigger)).is_err() {
                        return;
                    }
                    continue;
                }
                let wake = debouncer.next_wake(clock.now_utc());
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return,
                    signal = events_rx.recv() => match signal {
                        Some(WatchSignal::Changed) => debouncer.on_event(clock.now_utc()),
                        Some(WatchSignal::Overflow) => debouncer.on_overflow(clock.now_utc()),
                        None => return,
                    },
                    // With nothing pending there is no deadline, so park
                    // forever on the two branches above rather than spinning.
                    _ = async {
                        match wake {
                            Some(at) => clock.sleep_until(at).await,
                            None => std::future::pending::<()>().await,
                        }
                    } => {}
                }
            }
        });
        handle
    }
}

#[derive(Debug, Clone, Copy)]
enum WatchSignal {
    Changed,
    Overflow,
}

/// Is this path worth waking the debouncer for?
///
/// Runs on `notify`'s own thread, once per changed path, so it does the
/// cheapest thing that can be correct: find the source root the path lives
/// under, and glob-match the remainder.
fn is_relevant(path: &Path, roots: &[PathBuf], matcher: &ExclusionMatcher) -> bool {
    for root in roots {
        if let Ok(rel) = path.strip_prefix(root) {
            if rel.as_os_str().is_empty() {
                return true;
            }
            if matcher.matches_file(rel) {
                return false;
            }
            // A change *inside* an excluded directory arrives as a path to the
            // file, not to the directory, so every ancestor has to be checked
            // too — this is what stops `node_modules/.bin/x` waking the job.
            let mut ancestor = rel.parent();
            while let Some(dir) = ancestor {
                if dir.as_os_str().is_empty() {
                    break;
                }
                if matcher.matches_dir(dir) {
                    return false;
                }
                ancestor = dir.parent();
            }
            return true;
        }
    }
    // Outside every source root: not ours.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + seconds, 0).expect("timestamp")
    }

    #[test]
    fn nothing_pending_means_nothing_to_do() {
        let mut d = ChangeDebouncer::new(30, 0);
        assert_eq!(d.poll(at(0)), None);
        assert_eq!(d.next_wake(at(0)), None);
        assert!(!d.is_pending());
    }

    #[test]
    fn a_burst_of_events_produces_exactly_one_run() {
        let mut d = ChangeDebouncer::new(30, 0);
        // Simulate `npm install`: 10 000 events over 5 seconds.
        for i in 0..10_000 {
            d.on_event(at(i % 5));
        }
        assert_eq!(d.poll(at(4)), None, "still churning");
        assert_eq!(d.poll(at(20)), None, "not quiet for long enough yet");
        assert_eq!(d.poll(at(34)), Some(WatchTrigger::Debounced));
        assert_eq!(d.poll(at(100)), None, "one burst, one run");
    }

    #[test]
    fn each_new_event_restarts_the_quiet_period() {
        let mut d = ChangeDebouncer::new(30, 0);
        d.on_event(at(0));
        assert_eq!(d.poll(at(29)), None);
        d.on_event(at(29));
        assert_eq!(d.poll(at(31)), None, "the second event pushed the deadline out");
        assert_eq!(d.poll(at(59)), Some(WatchTrigger::Debounced));
    }

    #[test]
    fn a_never_quiet_tree_still_gets_backed_up() {
        let mut d = ChangeDebouncer::new(30, 0);
        // An event every 10 seconds forever: the quiet period never elapses.
        let mut fired = None;
        for i in 0..200 {
            let now = at(i * 10);
            d.on_event(now);
            if let Some(t) = d.poll(now) {
                fired = Some((i, t));
                break;
            }
        }
        let (tick, trigger) = fired.expect("a continuously churning tree must still run");
        assert_eq!(trigger, WatchTrigger::Debounced);
        // 30s debounce * 10 = 300s ceiling, i.e. the 30th ten-second tick.
        assert_eq!(tick, 30);
    }

    #[test]
    fn min_interval_delays_but_never_loses_a_run() {
        let mut d = ChangeDebouncer::new(10, 60); // at most once an hour
        d.on_event(at(0));
        assert_eq!(d.poll(at(20)), Some(WatchTrigger::Debounced));
        // A change arrives two minutes later.
        d.on_event(at(140));
        assert_eq!(d.poll(at(160)), None, "inside the minimum interval");
        assert!(d.is_pending(), "the pending change must survive the wait");
        // The interval expires an hour after the first run.
        assert_eq!(d.poll(at(20 + 3600)), Some(WatchTrigger::Debounced));
    }

    #[test]
    fn next_wake_accounts_for_the_minimum_interval() {
        let mut d = ChangeDebouncer::new(10, 60);
        d.on_event(at(0));
        assert_eq!(d.poll(at(20)), Some(WatchTrigger::Debounced));
        d.on_event(at(30));
        // Quiet at 40s, but the interval does not expire until 20 + 3600.
        assert_eq!(d.next_wake(at(30)), Some(at(3620)));
    }

    #[test]
    fn a_dropped_event_forces_a_rescan() {
        let mut d = ChangeDebouncer::new(10, 0);
        d.on_overflow(at(0));
        assert_eq!(
            d.poll(at(20)),
            Some(WatchTrigger::Rescan),
            "an overflow must produce a run, not silence"
        );
        // And the flag does not stick.
        d.on_event(at(30));
        assert_eq!(d.poll(at(50)), Some(WatchTrigger::Debounced));
    }

    #[test]
    fn a_restart_does_not_immediately_re_fire() {
        let mut d = ChangeDebouncer::new(10, 60).with_last_run(Some(at(0)));
        d.on_event(at(60));
        assert_eq!(d.poll(at(80)), None, "the job ran a minute ago");
        assert_eq!(d.poll(at(3601)), Some(WatchTrigger::Debounced));
    }

    #[test]
    fn zero_debounce_is_clamped_to_a_second() {
        let mut d = ChangeDebouncer::new(0, 0);
        d.on_event(at(0));
        assert_eq!(d.poll(at(0)), None);
        assert_eq!(d.poll(at(1)), Some(WatchTrigger::Debounced));
    }

    // -- event filtering ----------------------------------------------------

    fn matcher() -> ExclusionMatcher {
        ExclusionMatcher::build(&crate::model::ExclusionSet::developer_defaults()).expect("build")
    }

    #[test]
    fn excluded_paths_never_reach_the_debouncer() {
        let root = PathBuf::from(if cfg!(windows) { r"C:\code" } else { "/code" });
        let roots = vec![root.clone()];
        let m = matcher();
        assert!(is_relevant(&root.join("src/main.rs"), &roots, &m));
        assert!(!is_relevant(&root.join("node_modules/react/index.js"), &roots, &m));
        assert!(
            !is_relevant(&root.join("web/node_modules/.bin/next"), &roots, &m),
            "a change deep inside an excluded directory must be filtered by its ancestor"
        );
        assert!(!is_relevant(&root.join("app/debug.log"), &roots, &m));
    }

    #[test]
    fn paths_outside_every_source_are_ignored() {
        let roots = vec![PathBuf::from(if cfg!(windows) { r"C:\code" } else { "/code" })];
        let elsewhere = PathBuf::from(if cfg!(windows) { r"C:\other\x" } else { "/other/x" });
        assert!(!is_relevant(&elsewhere, &roots, &matcher()));
    }
}
