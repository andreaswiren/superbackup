//! The `--schedule` mini-language.
//!
//! `manual`, `hourly`, `daily@02:00`, `weekly@mon,thu@09:00`, `every 30m`,
//! `on-change`, or a five-field cron expression — exactly the forms the help
//! text promises, and nothing else. A schedule that was misread is a job that
//! runs at the wrong time or never, so anything unrecognised is refused with
//! the list of accepted shapes rather than falling back to a default.

use superbackup_core::engine::schedule as engine_schedule;
use superbackup_core::model::{Schedule, TimeOfDay};

use super::output::{CliError, CliResult};

/// Debounce for `on-change`, matching the wizard's default: long enough that a
/// build writing a thousand files is one run, short enough to feel live.
const ON_CHANGE_DEBOUNCE_SECONDS: u32 = 60;
const ON_CHANGE_MIN_INTERVAL_MINUTES: u32 = 15;

pub fn parse(spec: &str) -> CliResult<Schedule> {
    let text = spec.trim();
    let lower = text.to_lowercase();

    match lower.as_str() {
        "manual" | "never" => return Ok(Schedule::Manual),
        "hourly" => return Ok(Schedule::Interval { minutes: 60 }),
        "daily" => return Ok(Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] }),
        "on-change" | "onchange" => {
            return Ok(Schedule::OnChange {
                debounce_seconds: ON_CHANGE_DEBOUNCE_SECONDS,
                min_interval_minutes: ON_CHANGE_MIN_INTERVAL_MINUTES,
            })
        }
        _ => {}
    }

    if let Some(rest) = lower.strip_prefix("every") {
        let every = rest.trim_start_matches([' ', ':', '=']);
        let duration = super::timespec::parse_duration(every).map_err(|_| bad(spec))?;
        let minutes = duration.num_minutes();
        if minutes < 1 {
            return Err(CliError::usage(format!(
                "`{spec}` asks for a run more often than once a minute"
            )));
        }
        return Ok(Schedule::Interval { minutes: minutes.min(u32::MAX as i64) as u32 });
    }

    if let Some(rest) = lower.strip_prefix("daily@") {
        return Ok(Schedule::Daily { times: parse_times(rest, spec)? });
    }

    if let Some(rest) = lower.strip_prefix("weekly@") {
        let (days, times) = rest.split_once('@').ok_or_else(|| bad(spec))?;
        return Ok(Schedule::Weekly {
            weekdays: parse_weekdays(days, spec)?,
            times: parse_times(times, spec)?,
        });
    }

    // Five whitespace-separated fields is a cron expression. Validated with
    // the same parser the scheduler uses, so a spec the CLI accepts is one the
    // daemon can actually run.
    if text.split_whitespace().count() == 5 {
        engine_schedule::parse_cron(text).map_err(|e| {
            CliError::usage(format!("`{spec}` is not a valid cron expression: {e}"))
        })?;
        return Ok(Schedule::Cron { expression: text.to_string() });
    }

    Err(bad(spec))
}

/// One line describing a schedule, from the scheduler's own vocabulary so the
/// CLI and the interface cannot describe the same schedule differently.
pub fn describe(schedule: &Schedule) -> String {
    engine_schedule::describe(schedule)
}

fn parse_times(text: &str, spec: &str) -> CliResult<Vec<TimeOfDay>> {
    let mut out = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        let (hour, minute) = part.split_once(':').ok_or_else(|| bad(spec))?;
        let hour: u8 = hour.parse().map_err(|_| bad(spec))?;
        let minute: u8 = minute.parse().map_err(|_| bad(spec))?;
        if hour > 23 || minute > 59 {
            return Err(CliError::usage(format!("`{part}` is not a time of day")));
        }
        out.push(TimeOfDay { hour, minute });
    }
    if out.is_empty() {
        return Err(bad(spec));
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn parse_weekdays(text: &str, spec: &str) -> CliResult<Vec<u8>> {
    let mut out = Vec::new();
    for part in text.split(',') {
        let day = match part.trim() {
            "mon" | "monday" => 0,
            "tue" | "tues" | "tuesday" => 1,
            "wed" | "weds" | "wednesday" => 2,
            "thu" | "thur" | "thurs" | "thursday" => 3,
            "fri" | "friday" => 4,
            "sat" | "saturday" => 5,
            "sun" | "sunday" => 6,
            _ => return Err(bad(spec)),
        };
        out.push(day);
    }
    if out.is_empty() {
        return Err(bad(spec));
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn bad(spec: &str) -> CliError {
    CliError::usage(format!("`{spec}` is not a schedule superbackup understands")).with_hint(
        "Use manual, hourly, daily@02:00, weekly@mon,thu@09:00, `every 30m`, on-change, or a \
         five-field cron expression.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_form_parses() {
        assert_eq!(parse("manual").expect("manual"), Schedule::Manual);
        assert_eq!(parse("hourly").expect("hourly"), Schedule::Interval { minutes: 60 });
        assert_eq!(
            parse("daily@02:00").expect("daily"),
            Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] }
        );
        assert_eq!(
            parse("weekly@mon,thu@09:00").expect("weekly"),
            Schedule::Weekly {
                weekdays: vec![0, 3],
                times: vec![TimeOfDay { hour: 9, minute: 0 }]
            }
        );
        assert_eq!(parse("every 30m").expect("interval"), Schedule::Interval { minutes: 30 });
        assert!(matches!(parse("on-change").expect("on-change"), Schedule::OnChange { .. }));
        assert_eq!(
            parse("0 2 * * *").expect("cron"),
            Schedule::Cron { expression: "0 2 * * *".to_string() }
        );
    }

    #[test]
    fn several_times_of_day_are_sorted_and_deduplicated() {
        assert_eq!(
            parse("daily@14:00,02:00,02:00").expect("multi"),
            Schedule::Daily {
                times: vec![TimeOfDay { hour: 2, minute: 0 }, TimeOfDay { hour: 14, minute: 0 }]
            }
        );
    }

    #[test]
    fn nonsense_is_refused_with_the_accepted_forms() {
        for bad in ["", "sometimes", "daily@25:00", "weekly@funday@09:00", "every 0m", "* * *"] {
            let error = parse(bad).err().unwrap_or_else(|| panic!("`{bad}` must be refused"));
            assert_eq!(error.exit_code(), crate::cli::exit::USAGE);
        }
    }

    #[test]
    fn a_cron_expression_is_validated_by_the_scheduler_not_by_counting_fields() {
        // Five fields of nonsense must not become a schedule that only fails
        // later, inside the daemon, at two in the morning.
        assert!(parse("99 99 99 99 99").is_err());
    }
}
