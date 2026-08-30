//! Time, injected.
//!
//! Every deadline in the engine — the scheduler's next-due sleep, the run
//! timeout, retry backoff, the progress coalescer's rate window, the token
//! bucket's refill — goes through [`Clock`]. Nothing in `engine/` calls
//! [`chrono::Utc::now`] or [`tokio::time::sleep`] directly.
//!
//! Why: a backup scheduler is almost entirely *timing* logic, and timing logic
//! that can only be tested by sleeping is timing logic that is never tested at
//! the interesting boundaries (a DST transition, a week-long suspend, a
//! 10-hour timeout). With the clock injected, "the machine was off for a week"
//! is three lines in a test and finishes in microseconds.
//!
//! The two implementations are [`SystemClock`] (production) and
//! [`TestClock`] (deterministic, manually advanced). `TestClock` is compiled
//! into the library rather than behind `#[cfg(test)]` because the engine's
//! integration tests live in `tests/` and can only reach the public API.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A boxed, `Send` future. Used instead of `async fn` in traits so that every
/// engine trait stays dyn-compatible: the daemon holds `Arc<dyn Clock>` and
/// `Arc<dyn BackupExecutor>` chosen at runtime, which RPITIT cannot express.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The engine's only source of "what time is it" and "wait until".
///
/// # Invariants an implementation must uphold
///
/// * [`Clock::now_utc`] is monotonically non-decreasing. The scheduler
///   tolerates a clock jumping *forward* (it re-derives the next due time on
///   every wake) but a clock that runs backwards can make a schedule fire
///   twice, so `TestClock::set` refuses to move backwards.
/// * [`Clock::sleep_until`] must complete once `now_utc() >= deadline`, and
///   must complete promptly if the deadline is already in the past.
/// * The returned future is `'static`, so a caller may hold it across a
///   `select!` arm without borrowing the clock.
pub trait Clock: std::fmt::Debug + Send + Sync + 'static {
    /// The current instant, in UTC. All engine arithmetic is done in UTC and
    /// converted to local time only at schedule-evaluation boundaries.
    fn now_utc(&self) -> DateTime<Utc>;

    /// Complete at `deadline`, or immediately if it has already passed.
    fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'static, ()>;

    /// Convenience wrapper. A non-positive duration completes immediately.
    fn sleep(&self, duration: ChronoDuration) -> BoxFuture<'static, ()> {
        let deadline = self.now_utc() + duration;
        self.sleep_until(deadline)
    }
}

/// The real clock: `Utc::now()` plus `tokio::time::sleep`.
///
/// `sleep_until` is expressed as a *duration from now* rather than a
/// `tokio::time::Instant` computed once, because the wall clock can jump
/// (NTP correction, resume from suspend) while a `tokio` timer cannot. The
/// scheduler additionally caps individual sleeps (see
/// [`crate::engine::scheduler`]) so that a resume-from-suspend is noticed
/// within one cap interval even if the timer itself was frozen.
#[derive(Debug, Clone, Default)]
pub struct SystemClock;

impl SystemClock {
    pub fn new() -> SystemClock {
        SystemClock
    }
}

impl Clock for SystemClock {
    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'static, ()> {
        let delta = deadline - Utc::now();
        Box::pin(async move {
            match delta.to_std() {
                // Negative or zero durations fail `to_std`, which is exactly
                // the "deadline already passed" case.
                Err(_) => tokio::task::yield_now().await,
                Ok(d) => tokio::time::sleep(d).await,
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Test clock
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TestClockInner {
    /// Current time, published so that sleepers wake without polling.
    now: tokio::sync::watch::Sender<DateTime<Utc>>,
    /// Number of futures currently blocked in `sleep_until`. Published so a
    /// test can wait for the engine to actually be parked on a timer before
    /// advancing time, which is what removes the last race from these tests.
    sleepers: tokio::sync::watch::Sender<usize>,
    sleeper_count: AtomicUsize,
}

/// A manually advanced clock.
///
/// Typical use in a test:
///
/// ```no_run
/// # use superbackup_core::engine::clock::{Clock, TestClock};
/// # async fn demo() {
/// let clock = TestClock::at("2025-03-30T00:00:00Z");
/// // ... spawn the component under test ...
/// clock.wait_for_sleepers(1).await; // it is parked on its next deadline
/// clock.advance_hours(6);           // fire it
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct TestClock {
    inner: Arc<TestClockInner>,
}

impl TestClock {
    /// A clock starting at `start`.
    pub fn new(start: DateTime<Utc>) -> TestClock {
        let (now, _) = tokio::sync::watch::channel(start);
        let (sleepers, _) = tokio::sync::watch::channel(0usize);
        TestClock {
            inner: Arc::new(TestClockInner { now, sleepers, sleeper_count: AtomicUsize::new(0) }),
        }
    }

    /// A clock starting at an RFC 3339 instant.
    ///
    /// # Panics
    /// Panics on an unparseable literal. This is a test helper; a bad literal
    /// is a bug in the test, and failing loudly at the top of the test is more
    /// useful than threading a `Result` through every fixture.
    pub fn at(rfc3339: &str) -> TestClock {
        let dt: DateTime<Utc> = rfc3339
            .parse::<DateTime<chrono::FixedOffset>>()
            .unwrap_or_else(|e| panic!("TestClock::at({rfc3339}): {e}"))
            .with_timezone(&Utc);
        TestClock::new(dt)
    }

    /// Move time forward to `to`. Moving backwards is rejected (see the
    /// monotonicity invariant on [`Clock`]) and leaves the clock untouched.
    pub fn set(&self, to: DateTime<Utc>) {
        self.inner.now.send_if_modified(|cur| {
            if to > *cur {
                *cur = to;
                true
            } else {
                false
            }
        });
    }

    /// Move time forward by `delta`.
    pub fn advance(&self, delta: ChronoDuration) {
        let target = self.now_utc() + delta;
        self.set(target);
    }

    pub fn advance_secs(&self, secs: i64) {
        self.advance(ChronoDuration::seconds(secs));
    }

    pub fn advance_minutes(&self, minutes: i64) {
        self.advance(ChronoDuration::minutes(minutes));
    }

    pub fn advance_hours(&self, hours: i64) {
        self.advance(ChronoDuration::hours(hours));
    }

    /// How many futures are currently parked in [`Clock::sleep_until`].
    pub fn sleepers(&self) -> usize {
        self.inner.sleeper_count.load(Ordering::SeqCst)
    }

    /// Wait until at least `n` futures are parked on this clock.
    ///
    /// This is the deterministic replacement for `yield_now()` sprinkling: a
    /// test advances time only once the component under test has committed to
    /// a deadline, so there is no window in which the advance is missed.
    pub async fn wait_for_sleepers(&self, n: usize) {
        let mut rx = self.inner.sleepers.subscribe();
        loop {
            if *rx.borrow_and_update() >= n {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

/// Keeps the parked-sleeper count accurate even if the sleeping future is
/// dropped mid-flight (which is the normal outcome of a losing `select!` arm).
#[derive(Debug)]
struct SleeperGuard(Arc<TestClockInner>);

impl SleeperGuard {
    fn new(inner: Arc<TestClockInner>) -> SleeperGuard {
        let n = inner.sleeper_count.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = inner.sleepers.send(n);
        SleeperGuard(inner)
    }
}

impl Drop for SleeperGuard {
    fn drop(&mut self) {
        let n = self.0.sleeper_count.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
        let _ = self.0.sleepers.send(n);
    }
}

impl Clock for TestClock {
    fn now_utc(&self) -> DateTime<Utc> {
        *self.inner.now.borrow()
    }

    fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'static, ()> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            if *inner.now.borrow() >= deadline {
                // Yield so that a caller looping on an already-passed deadline
                // cannot starve the runtime.
                tokio::task::yield_now().await;
                return;
            }
            let mut rx = inner.now.subscribe();
            let _guard = SleeperGuard::new(inner);
            loop {
                if *rx.borrow_and_update() >= deadline {
                    return;
                }
                if rx.changed().await.is_err() {
                    // The clock was dropped; never wake. Parking forever is
                    // correct: the component owning this future is being torn
                    // down alongside the clock.
                    std::future::pending::<()>().await;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_clock_sleep_wakes_on_advance() {
        let clock = TestClock::at("2025-01-01T00:00:00Z");
        let c2 = clock.clone();
        let h = tokio::spawn(async move {
            c2.sleep(ChronoDuration::hours(3)).await;
            c2.now_utc()
        });
        clock.wait_for_sleepers(1).await;
        clock.advance_hours(3);
        let woke = h.await.expect("task");
        assert_eq!(woke, "2025-01-01T03:00:00Z".parse::<DateTime<Utc>>().expect("parse"));
    }

    #[tokio::test]
    async fn test_clock_refuses_to_run_backwards() {
        let clock = TestClock::at("2025-01-01T00:00:00Z");
        clock.set("2024-01-01T00:00:00Z".parse().expect("parse"));
        assert_eq!(clock.now_utc(), "2025-01-01T00:00:00Z".parse::<DateTime<Utc>>().expect("p"));
    }

    #[tokio::test]
    async fn passed_deadline_returns_immediately() {
        let clock = TestClock::at("2025-01-01T00:00:00Z");
        clock.sleep(ChronoDuration::seconds(-5)).await;
        assert_eq!(clock.sleepers(), 0);
    }
}
