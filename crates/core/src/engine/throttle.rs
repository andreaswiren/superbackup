//! Bandwidth resolution and rate limiting.
//!
//! Two separate jobs live here, and they are separate on purpose:
//!
//! * [`resolve_bandwidth`] answers "what is the ceiling for *this* run right
//!   now", folding job / destination / global overrides and any active
//!   [`BandwidthWindow`] into one [`ResolvedBandwidth`]. Repository
//!   destinations pass that number straight through to kopia, which does its
//!   own throttling.
//! * [`TokenBucket`] is a real limiter, used by [`crate::engine::mirror`],
//!   which copies files itself and has no external throttle to delegate to.
//!
//! # Precedence
//!
//! Job override → destination override → global. The *first override that
//! exists* wins as a whole; the levels are not merged field-by-field. Merging
//! would mean that clearing the upload limit on a job silently re-exposes the
//! global one, which is not what "override" means to anybody.
//!
//! Inside the winning level, an active window *does* merge field-by-field:
//! a window that sets only `upload_kbps` narrows uploads and leaves downloads
//! at the level's own value. A window field left empty means "unchanged", not
//! "unlimited", so a work-hours window cannot accidentally remove a limit.

use crate::engine::clock::Clock;
use crate::model::{BandwidthSettings, BandwidthWindow};
use chrono::{DateTime, Datelike, Duration, NaiveDateTime, TimeZone, Timelike, Utc};
use std::sync::Arc;

/// Which configuration level supplied the limit. Shown in the GUI so a user
/// who wonders why their upload is capped can find the setting that caps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandwidthSource {
    Job,
    Destination,
    Global,
}

impl BandwidthSource {
    pub fn title(&self) -> &'static str {
        match self {
            BandwidthSource::Job => "this job",
            BandwidthSource::Destination => "this destination",
            BandwidthSource::Global => "global settings",
        }
    }
}

/// The effective ceiling for one run against one destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedBandwidth {
    /// Kilobytes per second. `None` is unlimited.
    pub upload_kbps: Option<u32>,
    pub download_kbps: Option<u32>,
    pub source: BandwidthSource,
    /// True when a [`BandwidthWindow`] was in force at resolution time. The
    /// runner re-resolves at the start of each destination, so a run that
    /// crosses into a window is throttled from the next destination onwards.
    pub window_active: bool,
}

impl Default for ResolvedBandwidth {
    fn default() -> Self {
        ResolvedBandwidth {
            upload_kbps: None,
            download_kbps: None,
            source: BandwidthSource::Global,
            window_active: false,
        }
    }
}

impl ResolvedBandwidth {
    pub fn is_unlimited(&self) -> bool {
        self.upload_kbps.is_none() && self.download_kbps.is_none()
    }

    /// Bytes per second for the upload direction, or `None` for unlimited.
    /// "kbps" in this model means kilo*bytes*, matching kopia's
    /// `--upload-limit-mb` family rather than network kilobits.
    pub fn upload_bytes_per_second(&self) -> Option<u64> {
        self.upload_kbps.map(|k| (k as u64).saturating_mul(1024))
    }

    pub fn download_bytes_per_second(&self) -> Option<u64> {
        self.download_kbps.map(|k| (k as u64).saturating_mul(1024))
    }

    /// One line for the GUI, e.g. `2048 KB/s up (this job, reduced window)`.
    pub fn describe(&self) -> String {
        let limit = match (self.upload_kbps, self.download_kbps) {
            (None, None) => "Unlimited".to_string(),
            (Some(u), None) => format!("{u} KB/s up"),
            (None, Some(d)) => format!("{d} KB/s down"),
            (Some(u), Some(d)) => format!("{u} KB/s up, {d} KB/s down"),
        };
        let window = if self.window_active { ", reduced window" } else { "" };
        format!("{limit} ({}{window})", self.source.title())
    }
}

/// Resolve the ceiling for one run. See the module docs for precedence.
///
/// `now` is a UTC instant; windows are expressed in local wall-clock minutes,
/// so the conversion happens here rather than at the call site.
pub fn resolve_bandwidth<Tz: TimeZone>(
    job: Option<&BandwidthSettings>,
    destination: Option<&BandwidthSettings>,
    global: &BandwidthSettings,
    tz: &Tz,
    now: DateTime<Utc>,
) -> ResolvedBandwidth {
    let (settings, source) = match (job, destination) {
        (Some(j), _) => (j, BandwidthSource::Job),
        (None, Some(d)) => (d, BandwidthSource::Destination),
        (None, None) => (global, BandwidthSource::Global),
    };

    let local = now.with_timezone(tz).naive_local();
    let mut resolved = ResolvedBandwidth {
        upload_kbps: settings.upload_kbps,
        download_kbps: settings.download_kbps,
        source,
        window_active: false,
    };

    if let Some(window) = &settings.schedule {
        if window_contains(window, local) {
            resolved.window_active = true;
            // Field-by-field: an empty window field leaves the level's own
            // value in place.
            if window.upload_kbps.is_some() {
                resolved.upload_kbps = window.upload_kbps;
            }
            if window.download_kbps.is_some() {
                resolved.download_kbps = window.download_kbps;
            }
        }
    }
    resolved
}

const MINUTES_PER_DAY: u32 = 24 * 60;

/// Is `local` inside `window`?
///
/// Three things make this less trivial than it looks:
///
/// * A window may **wrap past midnight** (`22:00`–`06:00`). The half after
///   midnight belongs to the *previous* day's window, so the weekday filter
///   must be tested against yesterday for that half. A "Friday night" window
///   that stopped applying at 00:00 on Saturday would be useless.
/// * `start == end` is an **empty** window, not a 24-hour one. A user who
///   wants an all-day limit sets the level's own limit; treating a degenerate
///   window as always-on would throttle them by accident.
/// * Minutes are taken modulo a day so that a hand-edited `1500` cannot make
///   the comparison nonsensical.
pub fn window_contains(window: &BandwidthWindow, local: NaiveDateTime) -> bool {
    let start = window.start_minute % MINUTES_PER_DAY;
    let end = window.end_minute % MINUTES_PER_DAY;
    if start == end {
        return false;
    }
    let minute = local.hour() * 60 + local.minute();
    let today = local.date().weekday().num_days_from_monday() as u8;
    let yesterday = ((today + 6) % 7) as u8;

    if start < end {
        minute >= start && minute < end && weekday_allowed(window, today)
    } else {
        // Wrapped: [start, midnight) belongs to today, [midnight, end) to
        // yesterday's instance of the window.
        (minute >= start && weekday_allowed(window, today))
            || (minute < end && weekday_allowed(window, yesterday))
    }
}

/// An empty weekday list means every day — this matches the field's
/// documentation in [`crate::model`] and is the shape the GUI writes when the
/// user does not narrow the window.
fn weekday_allowed(window: &BandwidthWindow, weekday: u8) -> bool {
    window.weekdays.is_empty() || window.weekdays.contains(&weekday)
}

// ---------------------------------------------------------------------------
// Token bucket
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct BucketState {
    /// Available bytes. Allowed to go **negative**: a single write larger than
    /// the burst capacity borrows against future refill instead of
    /// deadlocking, which it would do if tokens were clamped at zero and the
    /// request could never be satisfied.
    tokens: f64,
    last_refill: DateTime<Utc>,
}

/// A byte-rate limiter for the mirror engine.
///
/// # Concurrency
///
/// The state is behind a `std::sync::Mutex`, and the critical section contains
/// no `await`: the lock is taken, the bucket is refilled and debited, the
/// required wait is computed, and the lock is dropped — then the sleep happens
/// outside it. Holding an async-aware mutex across the sleep would serialise
/// every writer behind the slowest one; holding a std mutex across an await
/// would be a deadlock waiting to happen. Neither occurs here.
#[derive(Debug)]
pub struct TokenBucket {
    rate: f64,
    capacity: f64,
    state: std::sync::Mutex<BucketState>,
    clock: Arc<dyn Clock>,
}

impl TokenBucket {
    /// Smallest burst allowed, so that a single 64 KiB write is never split
    /// across several sleeps at realistic limits.
    const MIN_BURST_BYTES: f64 = 64.0 * 1024.0;

    /// Longest uninterrupted blocking sleep, so cancellation stays prompt.
    const BLOCKING_SLICE_MS: u64 = 100;

    /// A limiter for `bytes_per_second`. The burst capacity is one second of
    /// traffic, floored at [`Self::MIN_BURST_BYTES`].
    pub fn new(bytes_per_second: u64, clock: Arc<dyn Clock>) -> TokenBucket {
        let rate = (bytes_per_second.max(1)) as f64;
        let capacity = rate.max(Self::MIN_BURST_BYTES);
        let now = clock.now_utc();
        TokenBucket {
            rate,
            capacity,
            state: std::sync::Mutex::new(BucketState { tokens: capacity, last_refill: now }),
            clock,
        }
    }

    /// A limiter for the upload direction of `limit`, or `None` when that
    /// direction is unlimited — in which case the caller skips the limiter
    /// entirely rather than paying for a no-op.
    pub fn for_upload(limit: &ResolvedBandwidth, clock: Arc<dyn Clock>) -> Option<TokenBucket> {
        limit.upload_bytes_per_second().map(|bps| TokenBucket::new(bps, clock))
    }

    /// Refill, debit `bytes`, and report how long the caller must wait.
    ///
    /// The whole critical section is here, and it contains no `await` and no
    /// blocking call — that is what makes both wrappers below safe.
    fn debit(&self, bytes: u64) -> Option<f64> {
        let now = self.clock.now_utc();
        match self.state.lock() {
            Ok(mut state) => {
                let elapsed =
                    (now - state.last_refill).num_microseconds().unwrap_or(0) as f64 / 1_000_000.0;
                if elapsed > 0.0 {
                    state.tokens = (state.tokens + elapsed * self.rate).min(self.capacity);
                    state.last_refill = now;
                }
                state.tokens -= bytes as f64;
                if state.tokens < 0.0 {
                    Some(-state.tokens / self.rate)
                } else {
                    None
                }
            }
            // A poisoned lock means another writer panicked. Not throttling
            // is preferable to propagating the panic into a backup run.
            Err(_) => None,
        }
    }

    /// Wait until `bytes` may be written, then account for them.
    ///
    /// Cancellation-safe in the sense that matters: if the future is dropped
    /// mid-sleep the bytes have already been debited, so the limiter errs
    /// towards being slightly *slower* than the target rather than faster.
    pub async fn consume(&self, bytes: u64) {
        if let Some(seconds) = self.debit(bytes) {
            let micros = (seconds * 1_000_000.0).clamp(0.0, 60_000_000.0) as i64;
            if micros > 0 {
                self.clock.sleep(Duration::microseconds(micros)).await;
            }
        }
    }

    /// The same limiter, for code already running on a blocking thread.
    ///
    /// The mirror engine copies files on a `spawn_blocking` worker, where
    /// `std::thread::sleep` is legal and awaiting is not. The wait is served
    /// in short slices so that a cancellation is noticed within
    /// [`Self::BLOCKING_SLICE_MS`] rather than after a multi-second stall.
    ///
    /// Only meaningful with a real clock: a [`crate::engine::clock::TestClock`]
    /// does not advance on its own, so this would sleep out its full deadline.
    /// Production callers all run under [`crate::engine::clock::SystemClock`].
    pub fn consume_blocking(&self, bytes: u64, cancel: &crate::engine::cancel::CancelToken) {
        let Some(seconds) = self.debit(bytes) else { return };
        let mut remaining_ms = (seconds * 1000.0).clamp(0.0, 60_000.0) as u64;
        while remaining_ms > 0 && !cancel.is_cancelled() {
            let slice = remaining_ms.min(Self::BLOCKING_SLICE_MS);
            std::thread::sleep(std::time::Duration::from_millis(slice));
            remaining_ms -= slice;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::SystemClock;
    use crate::engine::tz::DstZone;

    fn window(start: u32, end: u32, weekdays: Vec<u8>) -> BandwidthWindow {
        BandwidthWindow {
            start_minute: start,
            end_minute: end,
            upload_kbps: Some(256),
            download_kbps: None,
            weekdays,
        }
    }

    fn local(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").expect("literal")
    }

    #[test]
    fn simple_window_contains_only_its_own_hours() {
        let w = window(9 * 60, 17 * 60, vec![]);
        assert!(window_contains(&w, local("2025-01-08 09:00:00")));
        assert!(window_contains(&w, local("2025-01-08 16:59:00")));
        assert!(!window_contains(&w, local("2025-01-08 17:00:00")), "end is exclusive");
        assert!(!window_contains(&w, local("2025-01-08 08:59:00")));
    }

    #[test]
    fn wrapped_window_covers_both_sides_of_midnight() {
        // 22:00 -> 06:00.
        let w = window(22 * 60, 6 * 60, vec![]);
        assert!(window_contains(&w, local("2025-01-08 22:00:00")));
        assert!(window_contains(&w, local("2025-01-08 23:59:00")));
        assert!(window_contains(&w, local("2025-01-09 00:00:00")));
        assert!(window_contains(&w, local("2025-01-09 05:59:00")));
        assert!(!window_contains(&w, local("2025-01-09 06:00:00")));
        assert!(!window_contains(&w, local("2025-01-09 12:00:00")));
    }

    #[test]
    fn wrapped_window_weekday_filter_follows_the_start_day() {
        // Friday nights only. 2025-01-10 is a Friday, so the window runs from
        // Friday 22:00 to Saturday 06:00 — and Saturday 22:00 is *outside* it.
        let w = window(22 * 60, 6 * 60, vec![4]);
        assert!(window_contains(&w, local("2025-01-10 23:00:00")), "Friday night");
        assert!(
            window_contains(&w, local("2025-01-11 02:00:00")),
            "Saturday small hours still belong to Friday's window"
        );
        assert!(!window_contains(&w, local("2025-01-11 23:00:00")), "Saturday night is not listed");
        assert!(
            !window_contains(&w, local("2025-01-10 02:00:00")),
            "Friday small hours belong to Thursday's window, which is not listed"
        );
    }

    #[test]
    fn degenerate_window_is_never_active() {
        let w = window(600, 600, vec![]);
        assert!(!window_contains(&w, local("2025-01-08 10:00:00")));
    }

    #[test]
    fn out_of_range_minutes_are_taken_modulo_a_day() {
        let mut w = window(0, 0, vec![]);
        w.start_minute = 9 * 60;
        w.end_minute = 24 * 60 + 17 * 60; // hand-edited nonsense
        assert!(window_contains(&w, local("2025-01-08 10:00:00")));
    }

    // -- precedence ---------------------------------------------------------

    fn settings(up: Option<u32>, schedule: Option<BandwidthWindow>) -> BandwidthSettings {
        BandwidthSettings { upload_kbps: up, download_kbps: None, schedule }
    }

    fn at(s: &str) -> DateTime<Utc> {
        s.parse::<DateTime<Utc>>().expect("utc literal")
    }

    #[test]
    fn job_override_beats_destination_and_global() {
        let job = settings(Some(100), None);
        let dest = settings(Some(200), None);
        let global = settings(Some(300), None);
        let r = resolve_bandwidth(Some(&job), Some(&dest), &global, &Utc, at("2025-01-08T12:00:00Z"));
        assert_eq!(r.upload_kbps, Some(100));
        assert_eq!(r.source, BandwidthSource::Job);
    }

    #[test]
    fn destination_override_beats_global() {
        let dest = settings(Some(200), None);
        let global = settings(Some(300), None);
        let r = resolve_bandwidth(None, Some(&dest), &global, &Utc, at("2025-01-08T12:00:00Z"));
        assert_eq!(r.upload_kbps, Some(200));
        assert_eq!(r.source, BandwidthSource::Destination);
    }

    #[test]
    fn an_override_replaces_the_level_below_it_wholesale() {
        // The job clears the limit; the global limit must not leak back in.
        let job = settings(None, None);
        let global = settings(Some(300), None);
        let r = resolve_bandwidth(Some(&job), None, &global, &Utc, at("2025-01-08T12:00:00Z"));
        assert_eq!(r.upload_kbps, None);
        assert!(r.is_unlimited());
    }

    #[test]
    fn window_narrows_the_winning_level_in_local_time() {
        let tz = DstZone::EuropeStockholm;
        let global = settings(Some(5000), Some(window(9 * 60, 17 * 60, vec![])));
        // 2025-01-08 12:00 UTC = 13:00 CET, inside the window.
        let inside = resolve_bandwidth(None, None, &global, &tz, at("2025-01-08T12:00:00Z"));
        assert_eq!(inside.upload_kbps, Some(256));
        assert!(inside.window_active);
        // 2025-01-08 20:00 UTC = 21:00 CET, outside it.
        let outside = resolve_bandwidth(None, None, &global, &tz, at("2025-01-08T20:00:00Z"));
        assert_eq!(outside.upload_kbps, Some(5000));
        assert!(!outside.window_active);
    }

    #[test]
    fn empty_window_fields_leave_the_level_value_alone() {
        let mut w = window(9 * 60, 17 * 60, vec![]);
        w.upload_kbps = None;
        w.download_kbps = Some(64);
        let global =
            BandwidthSettings { upload_kbps: Some(5000), download_kbps: None, schedule: Some(w) };
        let r = resolve_bandwidth(None, None, &global, &Utc, at("2025-01-08T12:00:00Z"));
        assert_eq!(r.upload_kbps, Some(5000), "an empty window field must not unlimit");
        assert_eq!(r.download_kbps, Some(64));
    }

    // -- token bucket -------------------------------------------------------

    #[tokio::test]
    async fn bucket_is_instant_while_tokens_remain() {
        let clock = Arc::new(crate::engine::clock::TestClock::at("2025-01-01T00:00:00Z"));
        let bucket = TokenBucket::new(1024 * 1024, clock.clone());
        // Burst capacity is one second of traffic; this fits.
        bucket.consume(512 * 1024).await;
        assert_eq!(clock.now_utc(), at("2025-01-01T00:00:00Z"), "no wait was needed");
    }

    #[tokio::test]
    async fn bucket_charges_for_debt_on_a_virtual_clock() {
        let clock = Arc::new(crate::engine::clock::TestClock::at("2025-01-01T00:00:00Z"));
        let bucket = TokenBucket::new(1024 * 1024, clock.clone());
        bucket.consume(1024 * 1024).await; // drains the bucket
        let c2 = clock.clone();
        let h = tokio::spawn(async move { bucket.consume(1024 * 1024).await });
        c2.wait_for_sleepers(1).await;
        c2.advance(Duration::seconds(1));
        h.await.expect("consume");
    }

    /// The one test in the engine that is allowed to sleep for real: a rate
    /// limiter that only works against a fake clock is not a rate limiter.
    #[tokio::test]
    async fn bucket_holds_real_throughput_near_the_target() {
        let rate = 512 * 1024; // 512 KiB/s
        let bucket = TokenBucket::new(rate, Arc::new(SystemClock::new()));
        let chunk = 32 * 1024u64;
        let chunks = 24u64; // 768 KiB total: one second of burst, then ~0.5s of waiting
        let started = std::time::Instant::now();
        for _ in 0..chunks {
            bucket.consume(chunk).await;
        }
        let elapsed = started.elapsed().as_secs_f64();
        let total = (chunk * chunks) as f64;
        // The first `capacity` bytes are free (that is what a burst is), so the
        // expected floor is (total - capacity) / rate.
        let expected = (total - rate as f64) / rate as f64;
        assert!(
            elapsed >= expected * 0.8,
            "limiter let {total} bytes through in {elapsed:.3}s, expected at least {expected:.3}s"
        );
        assert!(
            elapsed < expected + 1.5,
            "limiter was far slower than the target: {elapsed:.3}s for {total} bytes"
        );
        let effective = total / elapsed.max(0.001);
        assert!(
            effective < rate as f64 * 3.0,
            "effective rate {effective:.0} B/s wildly exceeds the {rate} B/s limit"
        );
    }
}
