//! Every number, size, duration and timestamp in the interface passes through
//! this module.
//!
//! `DESIGN_SYSTEM.md` §10 makes the rules normative: bytes in binary units with
//! one decimal below ten and none above, durations in the largest two units,
//! relative time that switches to absolute after 48 hours in the past and 24
//! hours in the future. Keeping the rules in one place is what stops the
//! dashboard saying `1.4 GB` where the run detail says `1449 MB`.
//!
//! Nothing here touches egui, so all of it is unit-testable without a context.

use chrono::{DateTime, Datelike, Local, TimeZone, Utc};

// ---------------------------------------------------------------------------
// Bytes and rates
// ---------------------------------------------------------------------------

const UNITS: [&str; 6] = ["B", "kB", "MB", "GB", "TB", "PB"];

/// `842 MB`, `1.4 GB`, `12 GB`. Binary magnitudes, decimal-looking labels,
/// which is what every backup tool a developer already uses does.
pub fn bytes(value: u64) -> String {
    let mut v = value as f64;
    let mut unit = 0usize;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value, UNITS[0])
    } else if v < 10.0 {
        format!("{v:.1} {}", UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

/// `18.2 MB/s`. Negative and non-finite rates render as `0 B/s` rather than
/// letting a malformed daemon reply produce `NaN B/s` on screen.
pub fn rate(bytes_per_second: f64) -> String {
    let v = if bytes_per_second.is_finite() && bytes_per_second > 0.0 {
        bytes_per_second
    } else {
        0.0
    };
    format!("{}/s", bytes(v as u64))
}

/// `~12.4 GB`, used wherever a figure came from a budgeted background walk.
pub fn approx_bytes(value: u64) -> String {
    format!("about {}", bytes(value))
}

// ---------------------------------------------------------------------------
// Durations
// ---------------------------------------------------------------------------

/// Largest two units, and no seconds once an hour is involved: `42s`,
/// `4m 12s`, `1h 06m`, `2d 3h`.
pub fn duration(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        return format!("{s}s");
    }
    if s < 3600 {
        return format!("{}m {:02}s", s / 60, s % 60);
    }
    if s < 86_400 {
        return format!("{}h {:02}m", s / 3600, (s % 3600) / 60);
    }
    format!("{}d {}h", s / 86_400, (s % 86_400) / 3600)
}

/// The same value in the shape the estimated-time-remaining line wants.
pub fn eta(seconds: u64) -> String {
    duration(seconds as i64)
}

/// `27 min`, `2 hours`, `1 day` — a countdown at minute resolution, which is
/// what the auto-lock line wants; `duration` would render `27m 00s`.
pub fn minutes(total: u32) -> String {
    match total {
        0 => "now".to_string(),
        1 => "1 minute".to_string(),
        m if m < 60 => format!("{m} min"),
        m if m < 120 => "1 hour".to_string(),
        m if m < 1440 => format!("{} hours", m / 60),
        m if m < 2880 => "1 day".to_string(),
        m => format!("{} days", m / 1440),
    }
}

// ---------------------------------------------------------------------------
// Times
// ---------------------------------------------------------------------------

/// `12 Mar 02:00`, with the year appended only when it is not this one.
/// Always local time; the model stores UTC.
pub fn absolute(at: DateTime<Utc>) -> String {
    let local = at.with_timezone(&Local);
    let now = Local::now();
    if local.year() == now.year() {
        local.format("%-d %b %H:%M").to_string()
    } else {
        local.format("%-d %b %Y %H:%M").to_string()
    }
}

/// `12 Mar 02:00:04` — the Activity event log wants seconds.
pub fn absolute_seconds(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%-d %b %H:%M:%S").to_string()
}

/// `02:00`.
pub fn clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M").to_string()
}

/// `just now`, `2 minutes ago`, `4 hours ago`, `yesterday 02:00`, then
/// absolute once it is more than 48 hours old.
pub fn relative_past(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - at).num_seconds();
    if secs < 0 {
        // A clock that ran backwards is not worth a special sentence.
        return relative_future(at, now);
    }
    if secs < 60 {
        return "just now".to_string();
    }
    if secs < 3600 {
        let n = secs / 60;
        return if n == 1 { "1 minute ago".into() } else { format!("{n} minutes ago") };
    }
    if secs < 86_400 {
        let n = secs / 3600;
        return if n == 1 { "1 hour ago".into() } else { format!("{n} hours ago") };
    }
    if secs < 172_800 {
        return format!("yesterday {}", clock(at));
    }
    absolute(at)
}

/// `in 12 minutes`, `in 4 hours`, `tomorrow 02:00`, then absolute past a day.
pub fn relative_future(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (at - now).num_seconds();
    if secs <= 0 {
        return "any moment now".to_string();
    }
    if secs < 60 {
        return "in less than a minute".to_string();
    }
    if secs < 3600 {
        let n = secs / 60;
        return if n == 1 { "in 1 minute".into() } else { format!("in {n} minutes") };
    }
    if secs < 86_400 {
        let n = secs / 3600;
        return if n == 1 { "in 1 hour".into() } else { format!("in {n} hours") };
    }
    if secs < 172_800 {
        return format!("tomorrow {}", clock(at));
    }
    absolute(at)
}

/// Relative in whichever direction the timestamp actually lies.
pub fn relative(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    if at <= now {
        relative_past(at, now)
    } else {
        relative_future(at, now)
    }
}

/// Local-time offset, shown in tooltips where UTC versus local could matter.
pub fn offset_note(at: DateTime<Utc>) -> String {
    let local = Local.from_utc_datetime(&at.naive_utc());
    format!("{} (UTC{})", local.format("%Y-%m-%d %H:%M:%S"), local.format("%:z"))
}

// ---------------------------------------------------------------------------
// Counts
// ---------------------------------------------------------------------------

/// `1,204,882`. A thousands separator, because an unbroken eight-digit file
/// count is unreadable in a table cell.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `42%`, clamped, integer, never a decimal.
pub fn percent(fraction: f32) -> String {
    let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    format!("{}%", (f * 100.0).round() as i64)
}

/// `2nd`, `3rd`, `11th`. Used for `<n>th consecutive failure`.
pub fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// `1 job` / `4 jobs`, and the same shape for anything else countable.
pub fn plural(n: usize, singular: &str, plural_form: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural_form}")
    }
}

// ---------------------------------------------------------------------------
// Paths and ids
// ---------------------------------------------------------------------------

/// Middle elision that keeps the head and the last two segments, which is what
/// makes `C:\Users\andreas\…\web\src` readable at a glance (`DESIGN_SYSTEM.md`
/// L3). `max_chars` is a character budget; the pixel-aware variant lives in
/// `widgets::elide_to_width`.
pub fn elide_middle(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars || max_chars < 6 {
        return text.to_string();
    }

    // Prefer cutting on a separator so the result still looks like a path.
    let sep = if text.contains('\\') { '\\' } else { '/' };
    let segments: Vec<&str> = text.split(sep).collect();
    if segments.len() > 3 {
        let head = segments[0];
        let tail = segments[segments.len().saturating_sub(2)..].join(&sep.to_string());
        let candidate = format!("{head}{sep}…{sep}{tail}");
        if candidate.chars().count() <= max_chars {
            return candidate;
        }
        // Even the tail is too long: fall through to a plain character cut.
    }

    let keep = max_chars - 1;
    let head = keep / 2;
    let tail = keep - head;
    let head_s: String = chars[..head].iter().collect();
    let tail_s: String = chars[chars.len() - tail..].iter().collect();
    format!("{head_s}…{tail_s}")
}

/// Elide from the left, keeping the end. Used for the `Scanning …` line, where
/// the interesting part of a path is always the tail.
pub fn elide_left(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars || max_chars < 2 {
        return text.to_string();
    }
    let tail: String = chars[chars.len() - (max_chars - 1)..].iter().collect();
    format!("…{tail}")
}

/// First eight characters of a UUID; the full value lives in the tooltip.
pub fn short_uuid(id: &uuid::Uuid) -> String {
    let s = id.to_string();
    format!("{}…", &s[..8.min(s.len())])
}

/// First twelve characters of a kopia snapshot id.
pub fn short_snapshot(id: &str) -> String {
    if id.chars().count() <= 12 {
        id.to_string()
    } else {
        let head: String = id.chars().take(12).collect();
        format!("{head}…")
    }
}

/// `2,000 kB/s up, unlimited down` — the sentence the bandwidth override needs
/// so an override is comparable with the thing it overrides.
pub fn kbps(value: Option<u32>) -> String {
    match value {
        Some(v) => format!("{} kB/s", count(v as u64)),
        None => "unlimited".to_string(),
    }
}

/// `≈ 16 Mbit/s`, the reassurance line beside a kB/s field.
pub fn kbps_as_mbit(kbps_value: u32) -> String {
    let mbit = (kbps_value as f64) * 8.0 / 1000.0;
    if mbit < 10.0 {
        format!("≈ {mbit:.1} Mbit/s")
    } else {
        format!("≈ {mbit:.0} Mbit/s")
    }
}

/// `09:00` from minutes past local midnight, for the bandwidth window.
pub fn minutes_of_day(minutes: u32) -> String {
    let m = minutes % 1440;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Weekday initials in the model's order, 0 = Monday.
pub const WEEKDAY_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
pub const WEEKDAY_INITIAL: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];

/// `Mon, Wed, Fri`, or `every day` when the set is empty or complete.
pub fn weekdays(days: &[u8]) -> String {
    if days.is_empty() || days.len() >= 7 {
        return "every day".to_string();
    }
    let mut sorted: Vec<u8> = days.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
        .iter()
        .filter_map(|d| WEEKDAY_SHORT.get(*d as usize).copied())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn bytes_follow_the_one_decimal_rule() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(842), "842 B");
        assert_eq!(bytes(842 * 1024 * 1024), "842 MB");
        assert_eq!(bytes(1024 * 1024 * 1024 + 400 * 1024 * 1024), "1.4 GB");
        assert_eq!(bytes(12 * 1024 * 1024 * 1024), "12 GB");
    }

    #[test]
    fn a_broken_rate_never_renders_as_nan() {
        assert_eq!(rate(f64::NAN), "0 B/s");
        assert_eq!(rate(-1.0), "0 B/s");
        assert_eq!(rate(19_000_000.0), "18 MB/s");
    }

    #[test]
    fn durations_use_the_largest_two_units() {
        assert_eq!(duration(42), "42s");
        assert_eq!(duration(252), "4m 12s");
        assert_eq!(duration(3960), "1h 06m");
        assert_eq!(duration(183_600), "2d 3h");
        assert_eq!(duration(-5), "0s");
    }

    #[test]
    fn relative_past_switches_to_absolute_after_two_days() {
        let now = Utc::now();
        assert_eq!(relative_past(now - Duration::seconds(10), now), "just now");
        assert_eq!(relative_past(now - Duration::minutes(2), now), "2 minutes ago");
        assert_eq!(relative_past(now - Duration::hours(4), now), "4 hours ago");
        assert!(relative_past(now - Duration::hours(30), now).starts_with("yesterday"));
        let old = relative_past(now - Duration::days(9), now);
        assert!(!old.contains("ago"), "{old} should have become absolute");
    }

    #[test]
    fn relative_future_reads_forwards() {
        let now = Utc::now();
        assert_eq!(relative_future(now + Duration::minutes(12), now), "in 12 minutes");
        assert_eq!(relative_future(now + Duration::hours(4), now), "in 4 hours");
        assert!(relative_future(now + Duration::hours(30), now).starts_with("tomorrow"));
    }

    #[test]
    fn the_lock_countdown_is_minute_resolution() {
        assert_eq!(minutes(0), "now");
        assert_eq!(minutes(1), "1 minute");
        assert_eq!(minutes(27), "27 min");
        assert_eq!(minutes(90), "1 hour");
        assert_eq!(minutes(240), "4 hours");
        assert_eq!(minutes(2_880), "2 days");
    }

    #[test]
    fn counts_get_separators() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_204_882), "1,204,882");
    }

    #[test]
    fn percent_is_an_integer_and_clamped() {
        assert_eq!(percent(0.4242), "42%");
        assert_eq!(percent(1.5), "100%");
        assert_eq!(percent(f32::NAN), "0%");
    }

    #[test]
    fn ordinals_handle_the_teens() {
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(4), "4th");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(22), "22nd");
    }

    #[test]
    fn middle_elision_keeps_the_drive_and_the_tail() {
        let p = r"C:\Users\andreas\projects\web\src\components";
        let out = elide_middle(p, 30);
        assert!(out.starts_with("C:"), "{out}");
        assert!(out.ends_with(r"src\components"), "{out}");
        assert!(out.contains('…'));
        assert_eq!(elide_middle("short", 30), "short");
    }

    #[test]
    fn left_elision_keeps_the_tail() {
        let out = elide_left("/home/andreas/dev/web/src/components/App.tsx", 20);
        assert!(out.starts_with('…'));
        assert!(out.ends_with("App.tsx"));
        assert_eq!(out.chars().count(), 20);
    }

    #[test]
    fn weekday_rendering_collapses_the_full_week() {
        assert_eq!(weekdays(&[]), "every day");
        assert_eq!(weekdays(&[0, 1, 2, 3, 4, 5, 6]), "every day");
        assert_eq!(weekdays(&[4, 0, 2]), "Mon, Wed, Fri");
    }

    #[test]
    fn bandwidth_helpers_say_unlimited_rather_than_zero() {
        assert_eq!(kbps(None), "unlimited");
        assert_eq!(kbps(Some(2000)), "2,000 kB/s");
        assert_eq!(kbps_as_mbit(2000), "≈ 16 Mbit/s");
        assert_eq!(minutes_of_day(540), "09:00");
        assert_eq!(minutes_of_day(1_080), "18:00");
    }
}
