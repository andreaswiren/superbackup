//! Turning a [`Schedule`] into instants.
//!
//! Everything here is a pure function of `(schedule, timezone, instant)`. No
//! clock, no I/O, no state. That is deliberate: schedule arithmetic is the part
//! of a backup tool that is hardest to get right and easiest to test, so it is
//! kept completely free of the concurrency machinery that consumes it.
//!
//! # Local time, and what happens when it moves
//!
//! `Daily`, `Weekly` and `Cron` are wall-clock schedules: "02:30 every day"
//! means 02:30 on the user's clock, which is not a fixed number of hours after
//! the previous one. Twice a year that wall-clock time either does not exist or
//! exists twice, and the two classic bugs are silently skipping the run and
//! silently running it twice.
//!
//! The engine's rules, applied uniformly to every wall-clock schedule:
//!
//! * **Spring forward (a gap).** A time that does not exist — 02:30 on a day
//!   where the clock jumps 02:00 → 03:00 — runs at the **first instant that
//!   does exist at or after it**, i.e. 03:00 local. The backup happens, an hour
//!   later than usual. It is never skipped. Skipping is the worse failure: the
//!   user asked for a daily backup and would silently get 364 of them.
//! * **Fall back (a fold).** A time that exists twice — 02:30 on a day where
//!   the clock jumps 03:00 → 02:00 — runs **once, at the earlier of the two
//!   instants** (still on summer time). The second pass is not a new
//!   occurrence: [`next_occurrence_in`] only ever returns an instant strictly
//!   later than the one it was given, and the fold's two instants share a
//!   single wall-clock candidate, so the fold is consumed exactly once.
//! * `Interval` is deliberately *not* a wall-clock schedule. "Every 15
//!   minutes" means every 15 minutes of elapsed time, so it is evaluated on a
//!   UTC grid and is immune to both transitions — an interval schedule neither
//!   loses nor gains a run when the clocks move.
//!
//! # Catch-up
//!
//! [`catch_up_due`] collapses an arbitrary backlog into **at most one** run.
//! A machine that was off for a week with an hourly schedule produces one
//! catch-up run, not 168. The reasoning is that a backup is a snapshot of the
//! current tree: running the same job 168 times back-to-back would produce 167
//! identical snapshots, hammer the destination, and delay the one run that
//! actually matters.

use crate::model::{Schedule, TimeOfDay};
use chrono::{DateTime, Datelike, Duration, Local, MappedLocalTime, NaiveDateTime, TimeZone, Utc};

/// How far ahead the day-walking search looks before giving up. A weekly
/// schedule with an empty weekday list, or a `Cron` expression matching a date
/// that does not occur (31 February), must terminate rather than spin.
const MAX_DAYS_AHEAD: i64 = 400;

/// How far into a DST gap the "snap forward" search looks. Real gaps are 30 or
/// 60 minutes; 180 leaves room for any zone that has ever existed without
/// making a pathological zone loop forever.
const MAX_GAP_MINUTES: i64 = 180;

/// The next moment `schedule` fires strictly after `after`, in the machine's
/// local timezone.
///
/// Returns `None` for schedules that are not driven by the clock:
/// [`Schedule::Manual`] (only a human starts it) and [`Schedule::OnChange`]
/// (the filesystem watcher starts it — see [`crate::engine::watcher`]), and
/// for a [`Schedule::Cron`] whose expression does not parse.
pub fn next_occurrence(schedule: &Schedule, after: DateTime<Local>) -> Option<DateTime<Utc>> {
    next_occurrence_in(schedule, &Local, after.with_timezone(&Utc))
}

/// [`next_occurrence`] against an explicit timezone.
///
/// This generic form is what the engine actually calls, and what the tests use
/// to pin DST behaviour against a zone that really moves its clocks (see
/// [`crate::engine::tz`]). `after` is UTC because "strictly after" is an
/// ordering on instants, not on wall-clock readings — the distinction matters
/// precisely inside a fold, where two instants share one reading.
pub fn next_occurrence_in<Tz: TimeZone>(
    schedule: &Schedule,
    tz: &Tz,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    match schedule {
        Schedule::Manual | Schedule::OnChange { .. } => None,
        Schedule::Interval { minutes } => Some(next_interval(*minutes, after)),
        Schedule::Daily { times } => next_wall_clock(tz, after, times, None),
        Schedule::Weekly { weekdays, times } => next_wall_clock(tz, after, times, Some(weekdays)),
        Schedule::Cron { expression } => {
            let cron = parse_cron(expression).ok()?;
            let start = after.with_timezone(tz);
            cron.find_next_occurrence(&start, false).ok().map(|dt| dt.with_timezone(&Utc))
        }
    }
}

/// The most recent moment `schedule` fired at or before `at`.
///
/// Used only by [`catch_up_due`]. Searching backwards rather than replaying
/// forwards from the last run is what makes the backlog collapse O(1) in the
/// size of the backlog: a week-long outage costs the same as a one-minute one.
pub fn last_occurrence_in<Tz: TimeZone>(
    schedule: &Schedule,
    tz: &Tz,
    at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    match schedule {
        Schedule::Manual | Schedule::OnChange { .. } => None,
        Schedule::Interval { minutes } => Some(last_interval(*minutes, at)),
        Schedule::Daily { times } => last_wall_clock(tz, at, times, None),
        Schedule::Weekly { weekdays, times } => last_wall_clock(tz, at, times, Some(weekdays)),
        Schedule::Cron { expression } => {
            let cron = parse_cron(expression).ok()?;
            let start = at.with_timezone(tz);
            cron.find_previous_occurrence(&start, true).ok().map(|dt| dt.with_timezone(&Utc))
        }
    }
}

/// The single run owed to a job whose schedule elapsed while the machine was
/// off, asleep, or the daemon was not running.
///
/// Returns `Some(instant)` — the occurrence that was missed most recently —
/// or `None` when nothing is owed. **At most one** instant is ever returned,
/// no matter how long the outage was; see the module docs.
///
/// A job with no recorded previous run gets no catch-up: there is no evidence
/// it ever *should* have run before now, and firing every configured job at
/// once the first time the daemon starts is a denial-of-service on the user's
/// uplink, not a feature.
pub fn catch_up_due<Tz: TimeZone>(
    schedule: &Schedule,
    tz: &Tz,
    last_run: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let last_run = last_run?;
    let missed = last_occurrence_in(schedule, tz, now)?;
    // `missed > last_run` is the whole test: if the newest occurrence in the
    // past is already covered by the last run, nothing was missed.
    (missed > last_run).then_some(missed)
}

// ---------------------------------------------------------------------------
// Object-safe timezone
// ---------------------------------------------------------------------------

/// The timezone, as the running engine sees it.
///
/// The functions above are generic over [`chrono::TimeZone`], which is the
/// right shape for pure schedule arithmetic but the wrong shape for the
/// scheduler and the runner: making those generic would push a `Tz` parameter
/// through every component that touches them, and through the daemon that owns
/// them, purely so the tests can substitute a zone.
///
/// `Zone` is the object-safe projection of everything the engine actually
/// needs from a timezone. Production passes [`chrono::Local`]; the tests pass
/// [`crate::engine::tz::DstZone`]. The blanket implementation means no zone
/// has to know this trait exists.
pub trait Zone: std::fmt::Debug + Send + Sync + 'static {
    /// See [`next_occurrence_in`].
    fn next_occurrence(&self, schedule: &Schedule, after: DateTime<Utc>) -> Option<DateTime<Utc>>;
    /// See [`last_occurrence_in`].
    fn last_occurrence(&self, schedule: &Schedule, at: DateTime<Utc>) -> Option<DateTime<Utc>>;
    /// See [`catch_up_due`].
    fn catch_up(
        &self,
        schedule: &Schedule,
        last_run: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>>;
    /// The local wall-clock reading of `at`, for bandwidth windows.
    fn local_naive(&self, at: DateTime<Utc>) -> NaiveDateTime;
    /// See [`crate::engine::throttle::resolve_bandwidth`]. Lives here rather
    /// than on the throttle module so that `Zone` stays the single object-safe
    /// timezone seam.
    fn resolve_bandwidth(
        &self,
        job: Option<&crate::model::BandwidthSettings>,
        destination: Option<&crate::model::BandwidthSettings>,
        global: &crate::model::BandwidthSettings,
        now: DateTime<Utc>,
    ) -> crate::engine::throttle::ResolvedBandwidth;
}

impl<Tz> Zone for Tz
where
    Tz: TimeZone + std::fmt::Debug + Send + Sync + 'static,
    Tz::Offset: Send + Sync,
{
    fn next_occurrence(&self, schedule: &Schedule, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        next_occurrence_in(schedule, self, after)
    }

    fn last_occurrence(&self, schedule: &Schedule, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
        last_occurrence_in(schedule, self, at)
    }

    fn catch_up(
        &self,
        schedule: &Schedule,
        last_run: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        catch_up_due(schedule, self, last_run, now)
    }

    fn local_naive(&self, at: DateTime<Utc>) -> NaiveDateTime {
        at.with_timezone(self).naive_local()
    }

    fn resolve_bandwidth(
        &self,
        job: Option<&crate::model::BandwidthSettings>,
        destination: Option<&crate::model::BandwidthSettings>,
        global: &crate::model::BandwidthSettings,
        now: DateTime<Utc>,
    ) -> crate::engine::throttle::ResolvedBandwidth {
        crate::engine::throttle::resolve_bandwidth(job, destination, global, self, now)
    }
}

// ---------------------------------------------------------------------------
// Interval
// ---------------------------------------------------------------------------

/// Interval schedules run on a fixed UTC grid anchored at the Unix epoch, so
/// "every 15 minutes" lands on :00/:15/:30/:45 regardless of when the daemon
/// started, survives a daemon restart without drifting, and is unaffected by
/// DST. A zero-minute interval is clamped to one minute rather than rejected:
/// treating it as "never" would silently disable the job, and treating it
/// literally would be a busy loop.
fn interval_seconds(minutes: u32) -> i64 {
    (minutes.max(1) as i64) * 60
}

fn next_interval(minutes: u32, after: DateTime<Utc>) -> DateTime<Utc> {
    let step = interval_seconds(minutes);
    let secs = after.timestamp();
    // `div_euclid` keeps the grid aligned for pre-epoch timestamps too, which
    // a badly restored clock can produce.
    let next = (secs.div_euclid(step) + 1) * step;
    DateTime::from_timestamp(next, 0).unwrap_or(after)
}

fn last_interval(minutes: u32, at: DateTime<Utc>) -> DateTime<Utc> {
    let step = interval_seconds(minutes);
    let prev = at.timestamp().div_euclid(step) * step;
    DateTime::from_timestamp(prev, 0).unwrap_or(at)
}

// ---------------------------------------------------------------------------
// Wall-clock (Daily / Weekly)
// ---------------------------------------------------------------------------

/// Sorted, de-duplicated, validity-filtered times. An out-of-range
/// `TimeOfDay` (which JSON can express) is dropped rather than panicking.
fn normalise_times(times: &[TimeOfDay]) -> Vec<TimeOfDay> {
    let mut out: Vec<TimeOfDay> =
        times.iter().copied().filter(|t| t.hour < 24 && t.minute < 60).collect();
    out.sort();
    out.dedup();
    out
}

fn weekday_matches(date: chrono::NaiveDate, weekdays: Option<&Vec<u8>>) -> bool {
    match weekdays {
        None => true,
        // An empty weekday list means "no day", not "every day": the GUI
        // cannot produce it, but a hand-edited config can, and inventing days
        // the user did not ask for is worse than not running.
        Some(days) => days.contains(&(date.weekday().num_days_from_monday() as u8)),
    }
}

fn next_wall_clock<Tz: TimeZone>(
    tz: &Tz,
    after: DateTime<Utc>,
    times: &[TimeOfDay],
    weekdays: Option<&Vec<u8>>,
) -> Option<DateTime<Utc>> {
    let times = normalise_times(times);
    if times.is_empty() {
        return None;
    }
    let mut day = after.with_timezone(tz).date_naive();
    for _ in 0..MAX_DAYS_AHEAD {
        if weekday_matches(day, weekdays) {
            for t in &times {
                if let Some(instant) = resolve_local(tz, day, *t) {
                    // Strictly greater is what makes a fold fire exactly once:
                    // the second pass through 02:30 is not later than the
                    // first pass's instant plus the run itself, so the search
                    // moves on to the next day instead of repeating.
                    if instant > after {
                        return Some(instant);
                    }
                }
            }
        }
        day = day.succ_opt()?;
    }
    None
}

fn last_wall_clock<Tz: TimeZone>(
    tz: &Tz,
    at: DateTime<Utc>,
    times: &[TimeOfDay],
    weekdays: Option<&Vec<u8>>,
) -> Option<DateTime<Utc>> {
    let times = normalise_times(times);
    if times.is_empty() {
        return None;
    }
    let mut day = at.with_timezone(tz).date_naive();
    for _ in 0..MAX_DAYS_AHEAD {
        if weekday_matches(day, weekdays) {
            for t in times.iter().rev() {
                if let Some(instant) = resolve_local(tz, day, *t) {
                    if instant <= at {
                        return Some(instant);
                    }
                }
            }
        }
        day = day.pred_opt()?;
    }
    None
}

/// Map one wall-clock reading onto a real instant, applying the DST policy
/// documented at the top of this module.
fn resolve_local<Tz: TimeZone>(
    tz: &Tz,
    day: chrono::NaiveDate,
    time: TimeOfDay,
) -> Option<DateTime<Utc>> {
    let naive = day.and_hms_opt(time.hour as u32, time.minute as u32, 0)?;
    resolve_naive(tz, naive)
}

fn resolve_naive<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    match tz.from_local_datetime(&naive) {
        MappedLocalTime::Single(dt) => Some(dt.with_timezone(&Utc)),
        // Fold: take the earlier pass, and only that one.
        MappedLocalTime::Ambiguous(first, _second) => Some(first.with_timezone(&Utc)),
        // Gap: walk forward to the first reading that exists.
        MappedLocalTime::None => {
            for step in 1..=MAX_GAP_MINUTES {
                let probe = naive.checked_add_signed(Duration::minutes(step))?;
                match tz.from_local_datetime(&probe) {
                    MappedLocalTime::Single(dt) => return Some(dt.with_timezone(&Utc)),
                    MappedLocalTime::Ambiguous(first, _) => {
                        return Some(first.with_timezone(&Utc))
                    }
                    MappedLocalTime::None => continue,
                }
            }
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Cron
// ---------------------------------------------------------------------------

/// Parse a cron expression, mapping `croner`'s error into the crate error type
/// so the GUI can validate the field the user typed into.
///
/// Five fields (minute hour day-of-month month day-of-week) is the documented
/// shape; `croner` also accepts an optional leading seconds field, which is a
/// superset and costs nothing to allow.
pub fn parse_cron(expression: &str) -> crate::Result<croner::Cron> {
    use std::str::FromStr;
    croner::Cron::from_str(expression)
        .map_err(|e| crate::Error::Schedule(format!("{expression:?}: {e}")))
}

/// Validate a schedule the user just edited, returning the reason it is
/// unusable. Called by the config layer and by the GUI's live validation.
pub fn validate(schedule: &Schedule) -> crate::Result<()> {
    match schedule {
        Schedule::Manual | Schedule::OnChange { .. } => Ok(()),
        Schedule::Interval { minutes } => {
            if *minutes == 0 {
                Err(crate::Error::Schedule("an interval of 0 minutes is not a schedule".into()))
            } else {
                Ok(())
            }
        }
        Schedule::Daily { times } => {
            if normalise_times(times).is_empty() {
                Err(crate::Error::Schedule("a daily schedule needs at least one valid time".into()))
            } else {
                Ok(())
            }
        }
        Schedule::Weekly { weekdays, times } => {
            if weekdays.is_empty() || weekdays.iter().any(|d| *d > 6) {
                return Err(crate::Error::Schedule(
                    "a weekly schedule needs at least one weekday, numbered 0 (Monday) to 6".into(),
                ));
            }
            if normalise_times(times).is_empty() {
                Err(crate::Error::Schedule(
                    "a weekly schedule needs at least one valid time".into(),
                ))
            } else {
                Ok(())
            }
        }
        Schedule::Cron { expression } => parse_cron(expression).map(|_| ()),
    }
}

// ---------------------------------------------------------------------------
// Human description
// ---------------------------------------------------------------------------

/// The sentence the GUI shows under a job's name, e.g. `Every day at 02:00`.
///
/// Kept here rather than in the GUI so that the tray, the GUI and
/// `superbackup list` can never disagree about what a schedule means.
pub fn describe(schedule: &Schedule) -> String {
    match schedule {
        Schedule::Manual => "Only when you start it".to_string(),
        Schedule::Interval { minutes } => format!("Every {}", plural_minutes(*minutes)),
        Schedule::Daily { times } => {
            let times = normalise_times(times);
            if times.is_empty() {
                "Never (no valid time set)".to_string()
            } else {
                format!("Every day at {}", join_times(&times))
            }
        }
        Schedule::Weekly { weekdays, times } => {
            let times = normalise_times(times);
            if times.is_empty() || weekdays.is_empty() {
                return "Never (incomplete weekly schedule)".to_string();
            }
            format!("{} at {}", describe_weekdays(weekdays), join_times(&times))
        }
        Schedule::Cron { expression } => match parse_cron(expression) {
            Ok(cron) => capitalise(&cron.describe()),
            Err(_) => format!("Custom schedule ({expression})"),
        },
        Schedule::OnChange { debounce_seconds, min_interval_minutes } => {
            let base = format!("{} after changes stop", plural_seconds(*debounce_seconds));
            if *min_interval_minutes == 0 {
                base
            } else {
                format!("{base}, at most once every {}", plural_minutes(*min_interval_minutes))
            }
        }
    }
}

fn describe_weekdays(weekdays: &[u8]) -> String {
    let mut days: Vec<u8> = weekdays.iter().copied().filter(|d| *d <= 6).collect();
    days.sort_unstable();
    days.dedup();
    match days.as_slice() {
        [] => "Never".to_string(),
        [0, 1, 2, 3, 4] => "Weekdays".to_string(),
        [5, 6] => "Weekends".to_string(),
        [0, 1, 2, 3, 4, 5, 6] => "Every day".to_string(),
        [one] => format!("Every {}", weekday_name(*one)),
        many => {
            let names: Vec<&str> = many.iter().map(|d| weekday_name(*d)).collect();
            format!("Every {}", join_words(&names))
        }
    }
}

fn weekday_name(day: u8) -> &'static str {
    match day {
        0 => "Monday",
        1 => "Tuesday",
        2 => "Wednesday",
        3 => "Thursday",
        4 => "Friday",
        5 => "Saturday",
        _ => "Sunday",
    }
}

fn join_times(times: &[TimeOfDay]) -> String {
    let rendered: Vec<String> = times.iter().map(|t| t.to_string()).collect();
    let refs: Vec<&str> = rendered.iter().map(|s| s.as_str()).collect();
    join_words(&refs)
}

/// `a`, `a and b`, `a, b and c` — the Oxford-comma-free form the rest of the
/// UI copy uses.
fn join_words(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [a] => (*a).to_string(),
        [a, b] => format!("{a} and {b}"),
        _ => {
            let (last, rest) = items.split_last().unwrap_or((&"", &[]));
            format!("{} and {}", rest.join(", "), last)
        }
    }
}

fn plural_minutes(minutes: u32) -> String {
    match minutes {
        0 => "minute".to_string(),
        1 => "minute".to_string(),
        60 => "hour".to_string(),
        m if m % 1440 == 0 && m / 1440 == 1 => "day".to_string(),
        m if m % 1440 == 0 => format!("{} days", m / 1440),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{m} minutes"),
    }
}

fn plural_seconds(seconds: u32) -> String {
    match seconds {
        1 => "1 second".to_string(),
        s if s % 60 == 0 && s / 60 == 1 => "1 minute".to_string(),
        s if s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{s} seconds"),
    }
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tz::DstZone;

    fn utc(s: &str) -> DateTime<Utc> {
        s.parse::<DateTime<Utc>>().expect("utc literal")
    }

    fn tod(hour: u8, minute: u8) -> TimeOfDay {
        TimeOfDay { hour, minute }
    }

    fn daily(hour: u8, minute: u8) -> Schedule {
        Schedule::Daily { times: vec![tod(hour, minute)] }
    }

    // -- Interval -----------------------------------------------------------

    #[test]
    fn interval_lands_on_the_utc_grid() {
        let s = Schedule::Interval { minutes: 15 };
        let next = next_occurrence_in(&s, &Utc, utc("2025-01-01T10:07:31Z")).expect("next");
        assert_eq!(next, utc("2025-01-01T10:15:00Z"));
        // Exactly on a grid point returns the *following* point, never the
        // same instant, or the scheduler would spin.
        let next = next_occurrence_in(&s, &Utc, utc("2025-01-01T10:15:00Z")).expect("next");
        assert_eq!(next, utc("2025-01-01T10:30:00Z"));
    }

    #[test]
    fn interval_is_immune_to_dst() {
        let s = Schedule::Interval { minutes: 60 };
        let tz = DstZone::EuropeStockholm;
        // Straddles the spring-forward instant (01:00 UTC).
        let next = next_occurrence_in(&s, &tz, utc("2025-03-30T00:30:00Z")).expect("next");
        assert_eq!(next, utc("2025-03-30T01:00:00Z"));
        let next = next_occurrence_in(&s, &tz, utc("2025-03-30T01:00:00Z")).expect("next");
        assert_eq!(next, utc("2025-03-30T02:00:00Z"));
    }

    #[test]
    fn zero_interval_is_clamped_not_a_busy_loop() {
        let s = Schedule::Interval { minutes: 0 };
        let next = next_occurrence_in(&s, &Utc, utc("2025-01-01T10:00:30Z")).expect("next");
        assert_eq!(next, utc("2025-01-01T10:01:00Z"));
        assert!(validate(&s).is_err(), "the GUI must still reject it");
    }

    // -- Daily / DST --------------------------------------------------------

    #[test]
    fn daily_fires_once_a_day_in_a_stable_zone() {
        let tz = DstZone::EuropeStockholm;
        // 02:00 CET = 01:00 UTC in January.
        let next = next_occurrence_in(&daily(2, 0), &tz, utc("2025-01-10T05:00:00Z")).expect("n");
        assert_eq!(next, utc("2025-01-11T01:00:00Z"));
    }

    #[test]
    fn spring_forward_does_not_skip_the_run() {
        // Europe/Stockholm 2025-03-30: 02:00 -> 03:00. A 02:30 job has no
        // wall-clock slot, and must run at 03:00 local = 01:00 UTC.
        let tz = DstZone::EuropeStockholm;
        let s = daily(2, 30);
        let next = next_occurrence_in(&s, &tz, utc("2025-03-29T12:00:00Z")).expect("n");
        assert_eq!(
            next,
            utc("2025-03-30T01:00:00Z"),
            "a 02:30 job on the spring-forward day must snap to 03:00 local, not vanish"
        );
        // And the day after, it is back to its normal 02:30 CEST = 00:30 UTC.
        let after = next_occurrence_in(&s, &tz, next).expect("n");
        assert_eq!(after, utc("2025-03-31T00:30:00Z"));
    }

    #[test]
    fn spring_forward_new_york_does_not_skip_the_run() {
        let tz = DstZone::AmericaNewYork;
        // 2025-03-09: 02:00 EST -> 03:00 EDT. 02:30 snaps to 03:00 EDT = 07:00 UTC.
        let next = next_occurrence_in(&daily(2, 30), &tz, utc("2025-03-08T12:00:00Z")).expect("n");
        assert_eq!(next, utc("2025-03-09T07:00:00Z"));
    }

    #[test]
    fn fall_back_runs_exactly_once() {
        // Europe/Stockholm 2025-10-26: 03:00 CEST -> 02:00 CET, so 02:30
        // happens at 00:30 UTC and again at 01:30 UTC.
        let tz = DstZone::EuropeStockholm;
        let s = daily(2, 30);
        let first = next_occurrence_in(&s, &tz, utc("2025-10-25T12:00:00Z")).expect("n");
        assert_eq!(first, utc("2025-10-26T00:30:00Z"), "must take the earlier pass");
        // Asking again from the instant it fired must jump to the next day —
        // not to the second pass through the same wall-clock time.
        let second = next_occurrence_in(&s, &tz, first).expect("n");
        assert_eq!(second, utc("2025-10-27T01:30:00Z"), "must not fire twice inside the fold");
        // Even asking from *inside* the fold does not resurrect it.
        let inside = next_occurrence_in(&s, &tz, utc("2025-10-26T01:00:00Z")).expect("n");
        assert_eq!(inside, utc("2025-10-27T01:30:00Z"));
    }

    #[test]
    fn fall_back_new_york_runs_exactly_once() {
        let tz = DstZone::AmericaNewYork;
        let s = daily(1, 30);
        let first = next_occurrence_in(&s, &tz, utc("2025-11-01T12:00:00Z")).expect("n");
        assert_eq!(first, utc("2025-11-02T05:30:00Z"), "01:30 EDT");
        let second = next_occurrence_in(&s, &tz, first).expect("n");
        assert_eq!(second, utc("2025-11-03T06:30:00Z"), "next day, not 01:30 EST");
    }

    #[test]
    fn two_daily_times_collapsing_into_one_instant_fire_once() {
        // 02:30 snaps forward to 03:00, which is also a configured time.
        let tz = DstZone::EuropeStockholm;
        let s = Schedule::Daily { times: vec![tod(2, 30), tod(3, 0)] };
        let first = next_occurrence_in(&s, &tz, utc("2025-03-29T23:00:00Z")).expect("n");
        assert_eq!(first, utc("2025-03-30T01:00:00Z"));
        let second = next_occurrence_in(&s, &tz, first).expect("n");
        assert_eq!(second, utc("2025-03-31T00:30:00Z"), "no duplicate run at the same instant");
    }

    // -- Weekly -------------------------------------------------------------

    #[test]
    fn weekly_only_fires_on_listed_days() {
        let tz = DstZone::EuropeStockholm;
        // 2025-01-06 is a Monday.
        let s = Schedule::Weekly { weekdays: vec![0, 4], times: vec![tod(9, 0)] };
        let next = next_occurrence_in(&s, &tz, utc("2025-01-06T12:00:00Z")).expect("n");
        // Friday 2025-01-10 09:00 CET = 08:00 UTC.
        assert_eq!(next, utc("2025-01-10T08:00:00Z"));
        let next = next_occurrence_in(&s, &tz, next).expect("n");
        assert_eq!(next, utc("2025-01-13T08:00:00Z"), "back to Monday");
    }

    #[test]
    fn weekly_with_no_days_never_fires() {
        let s = Schedule::Weekly { weekdays: vec![], times: vec![tod(9, 0)] };
        assert!(next_occurrence_in(&s, &Utc, utc("2025-01-01T00:00:00Z")).is_none());
        assert!(validate(&s).is_err());
    }

    // -- Cron ---------------------------------------------------------------

    #[test]
    fn cron_is_evaluated_in_local_time() {
        let tz = DstZone::EuropeStockholm;
        let s = Schedule::Cron { expression: "0 3 * * *".into() };
        let next = next_occurrence_in(&s, &tz, utc("2025-01-10T05:00:00Z")).expect("n");
        assert_eq!(next, utc("2025-01-11T02:00:00Z"), "03:00 CET = 02:00 UTC");
    }

    #[test]
    fn cron_across_spring_forward_does_not_skip() {
        let tz = DstZone::EuropeStockholm;
        let s = Schedule::Cron { expression: "30 2 * * *".into() };
        let next = next_occurrence_in(&s, &tz, utc("2025-03-29T23:00:00Z")).expect("n");
        assert_eq!(next, utc("2025-03-30T01:00:00Z"));
    }

    #[test]
    fn invalid_cron_never_fires_and_fails_validation() {
        let s = Schedule::Cron { expression: "not a cron".into() };
        assert!(next_occurrence_in(&s, &Utc, utc("2025-01-01T00:00:00Z")).is_none());
        assert!(validate(&s).is_err());
    }

    // -- Manual / OnChange --------------------------------------------------

    #[test]
    fn clockless_schedules_have_no_next_occurrence() {
        assert!(next_occurrence_in(&Schedule::Manual, &Utc, utc("2025-01-01T00:00:00Z")).is_none());
        let s = Schedule::OnChange { debounce_seconds: 30, min_interval_minutes: 60 };
        assert!(next_occurrence_in(&s, &Utc, utc("2025-01-01T00:00:00Z")).is_none());
    }

    // -- Catch-up -----------------------------------------------------------

    #[test]
    fn a_week_of_downtime_collapses_to_one_run() {
        let tz = DstZone::EuropeStockholm;
        let s = Schedule::Interval { minutes: 60 };
        let last_run = utc("2025-01-01T00:00:00Z");
        let now = utc("2025-01-08T00:00:00Z"); // 168 hourly occurrences missed
        let due = catch_up_due(&s, &tz, Some(last_run), now).expect("owed one run");
        assert_eq!(due, utc("2025-01-08T00:00:00Z"));
        // Once that run is recorded, nothing more is owed.
        assert!(catch_up_due(&s, &tz, Some(due), now).is_none());
    }

    #[test]
    fn a_week_of_downtime_collapses_for_daily_too() {
        let tz = DstZone::EuropeStockholm;
        let s = daily(2, 0);
        let due = catch_up_due(&s, &tz, Some(utc("2025-01-01T01:00:00Z")), utc("2025-01-08T12:00:00Z"))
            .expect("owed one run");
        assert_eq!(due, utc("2025-01-08T01:00:00Z"), "the most recent missed 02:00 CET only");
    }

    #[test]
    fn nothing_is_owed_when_the_last_run_covers_the_last_occurrence() {
        let tz = DstZone::EuropeStockholm;
        let s = daily(2, 0);
        let due = catch_up_due(
            &s,
            &tz,
            Some(utc("2025-01-08T01:00:00Z")),
            utc("2025-01-08T09:00:00Z"),
        );
        assert!(due.is_none());
    }

    #[test]
    fn a_job_that_never_ran_gets_no_catch_up() {
        let tz = DstZone::EuropeStockholm;
        assert!(catch_up_due(&daily(2, 0), &tz, None, utc("2025-06-01T12:00:00Z")).is_none());
    }

    #[test]
    fn manual_jobs_are_never_owed_a_catch_up() {
        assert!(catch_up_due(
            &Schedule::Manual,
            &Utc,
            Some(utc("2020-01-01T00:00:00Z")),
            utc("2025-01-01T00:00:00Z")
        )
        .is_none());
    }

    // -- describe -----------------------------------------------------------

    #[test]
    fn describe_reads_like_english() {
        assert_eq!(describe(&daily(2, 0)), "Every day at 02:00");
        assert_eq!(
            describe(&Schedule::Daily { times: vec![tod(9, 0), tod(18, 0)] }),
            "Every day at 09:00 and 18:00"
        );
        assert_eq!(
            describe(&Schedule::Weekly {
                weekdays: vec![0, 1, 2, 3, 4],
                times: vec![tod(9, 0), tod(18, 0)]
            }),
            "Weekdays at 09:00 and 18:00"
        );
        assert_eq!(
            describe(&Schedule::Weekly { weekdays: vec![5, 6], times: vec![tod(11, 30)] }),
            "Weekends at 11:30"
        );
        assert_eq!(
            describe(&Schedule::Weekly { weekdays: vec![0], times: vec![tod(7, 5)] }),
            "Every Monday at 07:05"
        );
        assert_eq!(
            describe(&Schedule::Weekly { weekdays: vec![0, 4], times: vec![tod(7, 5)] }),
            "Every Monday and Friday at 07:05"
        );
        assert_eq!(describe(&Schedule::Interval { minutes: 15 }), "Every 15 minutes");
        assert_eq!(describe(&Schedule::Interval { minutes: 60 }), "Every hour");
        assert_eq!(describe(&Schedule::Interval { minutes: 180 }), "Every 3 hours");
        assert_eq!(describe(&Schedule::Manual), "Only when you start it");
        assert_eq!(
            describe(&Schedule::OnChange { debounce_seconds: 900, min_interval_minutes: 0 }),
            "15 minutes after changes stop"
        );
        assert_eq!(
            describe(&Schedule::OnChange { debounce_seconds: 30, min_interval_minutes: 60 }),
            "30 seconds after changes stop, at most once every hour"
        );
    }

    #[test]
    fn describe_never_panics_on_hostile_config() {
        // Hand-edited config: out-of-range times, bogus weekdays, empty lists.
        let hostile = [
            Schedule::Daily { times: vec![] },
            Schedule::Daily { times: vec![tod(99, 99)] },
            Schedule::Weekly { weekdays: vec![9], times: vec![] },
            Schedule::Cron { expression: String::new() },
            Schedule::Interval { minutes: 0 },
        ];
        for s in &hostile {
            let text = describe(s);
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn three_times_join_with_commas() {
        assert_eq!(
            describe(&Schedule::Daily { times: vec![tod(1, 0), tod(2, 0), tod(3, 0)] }),
            "Every day at 01:00, 02:00 and 03:00"
        );
    }
}
