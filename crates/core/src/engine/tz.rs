//! Real DST rule sets, without a timezone database.
//!
//! Production code evaluates schedules against [`chrono::Local`], which reads
//! the operating system's timezone database. That is correct, but it is
//! useless for testing: the DST behaviour of a schedule then depends on where
//! the machine running `cargo test` happens to be, and CI runs in UTC — a zone
//! with no transitions at all, where every DST bug passes.
//!
//! `chrono-tz` is not a dependency of this workspace, so this module hard-codes
//! the two rule sets the engine is tested against. They are the *actual*
//! current rules, not an approximation:
//!
//! * **Europe/Stockholm** (EU Directive 2000/84/EC): CET = UTC+1; summer time
//!   runs from 01:00 UTC on the last Sunday of March to 01:00 UTC on the last
//!   Sunday of October, when the offset is CEST = UTC+2. Locally that is a gap
//!   over `02:00–03:00` in spring and a fold over `02:00–03:00` in autumn.
//! * **America/New_York** (US Energy Policy Act 2005): EST = UTC−5; daylight
//!   time runs from 02:00 local standard time on the second Sunday of March to
//!   02:00 local daylight time on the first Sunday of November, when the offset
//!   is EDT = UTC−4. Locally: a gap over `02:00–03:00` and a fold over
//!   `01:00–02:00`.
//!
//! Both rules are valid for every year the test-suite uses (2007 onwards for
//! the US rule, 1996 onwards for the EU rule). They are deliberately *not*
//! historically accurate outside that range, and this type is therefore not
//! offered as a general timezone implementation — it exists so the DST tests
//! can assert against a zone that really does move its clocks.

use chrono::{
    Datelike, Duration, FixedOffset, MappedLocalTime, NaiveDate, NaiveDateTime, Offset, TimeZone,
    Weekday,
};

/// A timezone with a hard-coded DST rule. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DstZone {
    /// CET/CEST, EU transition rule.
    EuropeStockholm,
    /// EST/EDT, US transition rule.
    AmericaNewYork,
}

impl DstZone {
    /// IANA name, for messages and test failure output.
    pub fn name(&self) -> &'static str {
        match self {
            DstZone::EuropeStockholm => "Europe/Stockholm",
            DstZone::AmericaNewYork => "America/New_York",
        }
    }

    fn standard(&self) -> i32 {
        match self {
            DstZone::EuropeStockholm => 3600,
            DstZone::AmericaNewYork => -5 * 3600,
        }
    }

    fn daylight(&self) -> i32 {
        self.standard() + 3600
    }

    /// The two transition instants for `year`, expressed in UTC.
    ///
    /// Returning UTC instants (rather than local ones) is what keeps the
    /// local-time resolution below free of circular reasoning: deciding which
    /// offset applies never needs to already know the offset.
    fn transitions_utc(&self, year: i32) -> Option<(NaiveDateTime, NaiveDateTime)> {
        match self {
            DstZone::EuropeStockholm => {
                let start = last_weekday_of(year, 3, Weekday::Sun)?.and_hms_opt(1, 0, 0)?;
                let end = last_weekday_of(year, 10, Weekday::Sun)?.and_hms_opt(1, 0, 0)?;
                Some((start, end))
            }
            DstZone::AmericaNewYork => {
                // 02:00 EST = 07:00 UTC; 02:00 EDT = 06:00 UTC.
                let start = nth_weekday_of(year, 3, Weekday::Sun, 2)?.and_hms_opt(7, 0, 0)?;
                let end = nth_weekday_of(year, 11, Weekday::Sun, 1)?.and_hms_opt(6, 0, 0)?;
                Some((start, end))
            }
        }
    }

    fn is_daylight(&self, utc: NaiveDateTime) -> bool {
        match self.transitions_utc(utc.year()) {
            // Both modelled zones are northern-hemisphere, so summer time is a
            // single contiguous interval inside one calendar year.
            Some((start, end)) => utc >= start && utc < end,
            None => false,
        }
    }

    fn offset_seconds_for_utc(&self, utc: NaiveDateTime) -> i32 {
        if self.is_daylight(utc) {
            self.daylight()
        } else {
            self.standard()
        }
    }

    fn wrap(&self, seconds: i32) -> DstOffset {
        DstOffset { zone: *self, seconds }
    }
}

/// The offset half of [`DstZone`]. Carries the zone so that
/// `DateTime::timezone()` round-trips, which `croner` relies on when it walks
/// a cron expression forward through local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DstOffset {
    zone: DstZone,
    /// Seconds east of UTC. Held as an integer so that construction is
    /// infallible: `FixedOffset::east_opt` returns an `Option`, and a trait
    /// method that must return an offset has nowhere to put the error.
    seconds: i32,
}

impl DstOffset {
    fn fixed(&self) -> FixedOffset {
        // Only `|seconds| < 86_400` is accepted; every value this module
        // produces is within ±6 hours, so the fallback is unreachable. It is
        // spelled out rather than unwrapped because the engine forbids panics
        // outside tests.
        FixedOffset::east_opt(self.seconds).unwrap_or_else(|| chrono::Utc.fix())
    }
}

impl Offset for DstOffset {
    fn fix(&self) -> FixedOffset {
        self.fixed()
    }
}

impl std::fmt::Display for DstOffset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.fixed(), f)
    }
}

impl TimeZone for DstZone {
    type Offset = DstOffset;

    fn from_offset(offset: &DstOffset) -> DstZone {
        offset.zone
    }

    fn offset_from_local_date(&self, local: &NaiveDate) -> MappedLocalTime<DstOffset> {
        match local.and_hms_opt(0, 0, 0) {
            Some(dt) => self.offset_from_local_datetime(&dt),
            None => MappedLocalTime::None,
        }
    }

    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> MappedLocalTime<DstOffset> {
        let std_secs = self.standard();
        let dst_secs = self.daylight();
        // A local time is valid under a candidate offset only if the UTC
        // instant it implies actually uses that offset. Exactly one candidate
        // is valid normally, two inside a fold, and none inside a gap.
        let as_std = local.checked_sub_signed(Duration::seconds(std_secs as i64));
        let as_dst = local.checked_sub_signed(Duration::seconds(dst_secs as i64));
        let std_ok = as_std.map(|u| !self.is_daylight(u)).unwrap_or(false);
        let dst_ok = as_dst.map(|u| self.is_daylight(u)).unwrap_or(false);
        match (std_ok, dst_ok) {
            (true, false) => MappedLocalTime::Single(self.wrap(std_secs)),
            (false, true) => MappedLocalTime::Single(self.wrap(dst_secs)),
            // Fold. `Ambiguous` is ordered (earliest, latest); the earliest
            // UTC instant is the one reached with the *larger* offset, i.e.
            // still on daylight time.
            (true, true) => MappedLocalTime::Ambiguous(self.wrap(dst_secs), self.wrap(std_secs)),
            (false, false) => MappedLocalTime::None,
        }
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> DstOffset {
        // `and_hms_opt(0, 0, 0)` cannot fail for a valid `NaiveDate`; falling
        // back to the epoch keeps this panic-free either way.
        let dt = utc.and_hms_opt(0, 0, 0).unwrap_or_default();
        self.offset_from_utc_datetime(&dt)
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> DstOffset {
        self.wrap(self.offset_seconds_for_utc(*utc))
    }
}

/// The `nth` (1-based) `weekday` of `month` in `year`.
fn nth_weekday_of(year: i32, month: u32, weekday: Weekday, nth: u32) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let shift = (7 + weekday.num_days_from_monday() - first.weekday().num_days_from_monday()) % 7;
    let day = 1 + shift + (nth - 1) * 7;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// The last `weekday` of `month` in `year`.
fn last_weekday_of(year: i32, month: u32, weekday: Weekday) -> Option<NaiveDate> {
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let first_of_next = NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    let last = first_of_next.pred_opt()?;
    let back = (7 + last.weekday().num_days_from_monday() - weekday.num_days_from_monday()) % 7;
    last.checked_sub_signed(Duration::days(back as i64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn naive(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").expect("literal")
    }

    #[test]
    fn eu_transition_dates_are_correct() {
        // Last Sundays: 2025-03-30 and 2025-10-26.
        let (start, end) =
            DstZone::EuropeStockholm.transitions_utc(2025).expect("transitions");
        assert_eq!(start, naive("2025-03-30 01:00:00"));
        assert_eq!(end, naive("2025-10-26 01:00:00"));
    }

    #[test]
    fn us_transition_dates_are_correct() {
        // Second Sunday of March 2025 = the 9th; first Sunday of November = the 2nd.
        let (start, end) = DstZone::AmericaNewYork.transitions_utc(2025).expect("transitions");
        assert_eq!(start, naive("2025-03-09 07:00:00"));
        assert_eq!(end, naive("2025-11-02 06:00:00"));
    }

    #[test]
    fn spring_forward_produces_a_gap() {
        let tz = DstZone::EuropeStockholm;
        assert!(matches!(
            tz.from_local_datetime(&naive("2025-03-30 02:30:00")),
            MappedLocalTime::None
        ));
        // 01:59 is still CET, 03:00 is already CEST.
        let before = tz.from_local_datetime(&naive("2025-03-30 01:59:00")).single().expect("cet");
        assert_eq!(before.with_timezone(&Utc), Utc.with_ymd_and_hms(2025, 3, 30, 0, 59, 0).unwrap());
        let after = tz.from_local_datetime(&naive("2025-03-30 03:00:00")).single().expect("cest");
        assert_eq!(after.with_timezone(&Utc), Utc.with_ymd_and_hms(2025, 3, 30, 1, 0, 0).unwrap());
    }

    #[test]
    fn fall_back_produces_a_fold_ordered_earliest_first() {
        let tz = DstZone::EuropeStockholm;
        match tz.from_local_datetime(&naive("2025-10-26 02:30:00")) {
            MappedLocalTime::Ambiguous(first, second) => {
                assert_eq!(
                    first.with_timezone(&Utc),
                    Utc.with_ymd_and_hms(2025, 10, 26, 0, 30, 0).unwrap()
                );
                assert_eq!(
                    second.with_timezone(&Utc),
                    Utc.with_ymd_and_hms(2025, 10, 26, 1, 30, 0).unwrap()
                );
                assert!(first < second, "Ambiguous must be (earliest, latest)");
            }
            other => panic!("expected a fold, got {other:?}"),
        }
    }

    #[test]
    fn new_york_gap_and_fold() {
        let tz = DstZone::AmericaNewYork;
        assert!(matches!(
            tz.from_local_datetime(&naive("2025-03-09 02:30:00")),
            MappedLocalTime::None
        ));
        assert!(matches!(
            tz.from_local_datetime(&naive("2025-11-02 01:30:00")),
            MappedLocalTime::Ambiguous(_, _)
        ));
    }

    #[test]
    fn timezone_round_trips_through_its_offset() {
        let tz = DstZone::AmericaNewYork;
        let dt = tz.from_local_datetime(&naive("2025-06-01 12:00:00")).single().expect("summer");
        assert_eq!(dt.timezone(), DstZone::AmericaNewYork);
        assert_eq!(dt.offset().fix().local_minus_utc(), -4 * 3600);
    }
}
