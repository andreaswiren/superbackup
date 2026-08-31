//! Parsing the times and durations a person actually types.
//!
//! Two grammars, both small and both strict about what they refuse. A restore
//! that silently picks the wrong snapshot because `--at yesterday` was
//! misunderstood is the worst kind of bug this program could have, so anything
//! not understood is an error naming the accepted forms — never a guess, and
//! never a silent fall back to "the most recent".

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

use super::format;
use super::output::{CliError, CliResult};

/// `30m`, `4h`, `2h30m`, `90s`, `1d`, `1d12h`.
///
/// Accepts a run of `<number><unit>` pairs. A bare number is refused: `pause
/// 30` could reasonably mean seconds or minutes, and picking one silently is
/// how somebody's backups stay off for a day.
pub fn parse_duration(input: &str) -> CliResult<Duration> {
    let text = input.trim().to_lowercase();
    if text.is_empty() {
        return Err(bad_duration(input));
    }

    let mut total: i64 = 0;
    let mut digits = String::new();
    let mut saw_unit = false;

    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        if digits.is_empty() {
            return Err(bad_duration(input));
        }
        let value: i64 = digits.parse().map_err(|_| bad_duration(input))?;
        digits.clear();
        let seconds = match ch {
            's' => 1,
            'm' => 60,
            'h' => 3_600,
            'd' => 86_400,
            'w' => 604_800,
            _ => return Err(bad_duration(input)),
        };
        total =
            total.checked_add(value.saturating_mul(seconds)).ok_or_else(|| bad_duration(input))?;
        saw_unit = true;
    }

    if !digits.is_empty() || !saw_unit || total <= 0 {
        return Err(bad_duration(input));
    }
    Ok(Duration::seconds(total))
}

fn bad_duration(input: &str) -> CliError {
    CliError::usage(format!("`{input}` is not a duration"))
        .with_hint("Use a number and a unit, like 30m, 4h, 2h30m or 1d.")
}

/// A point in time, in the shapes the help text promises: an ISO timestamp,
/// `yesterday`, or `3 days ago`.
///
/// Bare timestamps are read as **local** time. A user typing `14:00` means
/// their own two o'clock; forcing them to think in UTC to restore a file is a
/// tax with no benefit. An explicit offset or a trailing `Z` is honoured as
/// given.
pub fn parse_at(input: &str) -> CliResult<DateTime<Utc>> {
    parse_at_from(input, Utc::now())
}

/// The same, with "now" injected, so the behaviour is testable.
pub fn parse_at_from(input: &str, now: DateTime<Utc>) -> CliResult<DateTime<Utc>> {
    let text = input.trim();
    if text.is_empty() {
        return Err(bad_time(input));
    }
    let lower = text.to_lowercase();

    match lower.as_str() {
        "now" => return Ok(now),
        // Midnight rather than "24 hours ago": somebody saying "yesterday"
        // means the day, and the latest snapshot from that day is what a
        // restore should land on.
        "today" => return start_of_day_local(now, 0),
        "yesterday" => return start_of_day_local(now, 1),
        _ => {}
    }

    // `3 days ago`, `2 hours ago`, `1 week ago`.
    if let Some(rest) = lower.strip_suffix(" ago") {
        return parse_ago(rest, now).ok_or_else(|| bad_time(input));
    }

    // A full RFC 3339 timestamp, offset and all.
    if let Ok(t) = DateTime::parse_from_rfc3339(text) {
        return Ok(t.with_timezone(&Utc));
    }

    // Local date and time, with or without the `T`, with or without seconds.
    for pattern in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, pattern) {
            return format::local_to_utc(naive).ok_or_else(|| ambiguous_local(input));
        }
    }

    // A bare date means the end of that day, so `--at 2026-08-29` finds the
    // last snapshot taken on the 29th rather than the one from just after
    // midnight.
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let naive = date.and_hms_opt(23, 59, 59).ok_or_else(|| bad_time(input))?;
        return format::local_to_utc(naive).ok_or_else(|| ambiguous_local(input));
    }

    Err(bad_time(input))
}

fn parse_ago(rest: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let mut parts = rest.split_whitespace();
    let count: i64 = match parts.next()? {
        "a" | "an" => 1,
        n => n.parse().ok()?,
    };
    let unit = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let seconds = match unit.trim_end_matches('s') {
        "second" | "sec" => 1,
        "minute" | "min" => 60,
        "hour" => 3_600,
        "day" => 86_400,
        "week" => 604_800,
        // Calendar months are not a fixed number of seconds; rather than
        // approximate one, say so and let the user give a date.
        _ => return None,
    };
    now.checked_sub_signed(Duration::seconds(count.checked_mul(seconds)?))
}

/// Local midnight, `days_back` days ago.
fn start_of_day_local(now: DateTime<Utc>, days_back: i64) -> CliResult<DateTime<Utc>> {
    let local = now.with_timezone(&Local) - Duration::days(days_back);
    let naive = local.date_naive().and_hms_opt(0, 0, 0).ok_or_else(|| bad_time("yesterday"))?;
    Local
        .from_local_datetime(&naive)
        .single()
        // A midnight that does not exist because the clocks went forward:
        // step to the next hour rather than failing on a technicality.
        .or_else(|| Local.from_local_datetime(&(naive + Duration::hours(1))).single())
        .map(|t| t.with_timezone(&Utc))
        .ok_or_else(|| bad_time("yesterday"))
}

fn bad_time(input: &str) -> CliError {
    CliError::usage(format!("`{input}` is not a time superbackup understands"))
        .with_hint("Use 2026-08-29T14:00, 2026-08-29, yesterday, today, or `3 days ago`.")
}

fn ambiguous_local(input: &str) -> CliError {
    CliError::usage(format!(
        "`{input}` is ambiguous in this time zone: the clocks changed and that local time \
         happened twice, or not at all"
    ))
    .with_hint("Give the time with an offset, like 2026-10-25T02:30+02:00.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_accept_the_forms_the_help_text_promises() {
        assert_eq!(parse_duration("30m").expect("30m").num_seconds(), 1_800);
        assert_eq!(parse_duration("4h").expect("4h").num_seconds(), 14_400);
        assert_eq!(parse_duration("2h30m").expect("2h30m").num_seconds(), 9_000);
        assert_eq!(parse_duration("1d").expect("1d").num_seconds(), 86_400);
        assert_eq!(parse_duration(" 90S ").expect("case and space").num_seconds(), 90);
    }

    #[test]
    fn a_bare_number_is_refused_rather_than_assumed() {
        for bad in ["30", "", "h", "4x", "4h30", "-2h"] {
            let error = parse_duration(bad).err().unwrap_or_else(|| panic!("`{bad}` must fail"));
            assert_eq!(error.exit_code(), crate::cli::exit::USAGE);
        }
    }

    #[test]
    fn an_iso_timestamp_with_an_offset_is_taken_exactly() {
        let now = Utc::now();
        let t = parse_at_from("2026-08-29T14:00:00+00:00", now).expect("rfc3339");
        assert_eq!(t.to_rfc3339(), "2026-08-29T14:00:00+00:00");
    }

    #[test]
    fn a_bare_timestamp_is_local_time() {
        let now = Utc::now();
        let t = parse_at_from("2026-08-29T14:00", now).expect("local");
        let local = t.with_timezone(&Local);
        assert_eq!(local.format("%Y-%m-%d %H:%M").to_string(), "2026-08-29 14:00");
    }

    #[test]
    fn a_bare_date_means_the_end_of_that_day() {
        // Otherwise `--at 2026-08-29` finds the snapshot from 00:00 and the
        // user quietly restores a day earlier than they asked for.
        let t = parse_at_from("2026-08-29", Utc::now()).expect("date");
        let local = t.with_timezone(&Local);
        assert_eq!(local.format("%Y-%m-%d %H:%M").to_string(), "2026-08-29 23:59");
    }

    #[test]
    fn yesterday_is_a_day_not_twenty_four_hours() {
        let now = Utc::now();
        let t = parse_at_from("yesterday", now).expect("yesterday");
        let local = t.with_timezone(&Local);
        assert_eq!(local.format("%H:%M").to_string(), "00:00");
        assert!(t < now);
    }

    #[test]
    fn three_days_ago_is_three_days_ago() {
        let now = Utc::now();
        let t = parse_at_from("3 days ago", now).expect("relative");
        assert_eq!((now - t).num_days(), 3);
        let hour = parse_at_from("2 hours ago", now).expect("relative");
        assert_eq!((now - hour).num_hours(), 2);
        assert!(parse_at_from("an hour ago", now).is_ok());
    }

    #[test]
    fn an_unparseable_time_names_the_accepted_forms() {
        for bad in ["last tuesday", "3 fortnights ago", "29/08/2026", "", "2 months ago"] {
            let error = parse_at_from(bad, Utc::now())
                .err()
                .unwrap_or_else(|| panic!("`{bad}` must be refused"));
            assert_eq!(error.exit_code(), crate::cli::exit::USAGE);
            let hint = error.hint.unwrap_or_default();
            assert!(hint.contains("yesterday"), "the hint must show the way: {hint}");
        }
    }
}
