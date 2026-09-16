//! Running a job again because the last run did not get everything.
//!
//! # Why this is not [`crate::engine::retry`]
//!
//! That module retries one *operation* seconds after it failed: a 503 from S3,
//! a lock held for a moment, a socket that dropped. It is measured in seconds
//! and it happens inside a single run.
//!
//! This is about the run as a whole, and about a different kind of miss. A
//! folder somebody is working in always has a few files another program has
//! open — an editor's lock file, a build's temporary output, a game engine's
//! `Temp` directory. Those are skipped so the snapshot gets made at all; the
//! snapshot is then genuinely missing them, and no amount of retrying five
//! seconds later helps, because the editor is still open.
//!
//! What does help is trying again in half an hour. The build finished, the
//! editor was closed, the file moved on. So a run that left files unread earns
//! another attempt on a timer measured in minutes, until it comes back clean or
//! the attempts run out.
//!
//! # Why it gives up
//!
//! Some files are never readable — a lock file that lives as long as the
//! application does, a pagefile, a socket. "Retry until the files are backed
//! up" would mean retrying that one for ever, running a full scan of the source
//! tree every half hour, day and night, for a file that cannot be read by
//! design. So the attempts are bounded, and the bound is a setting: a run that
//! is still missing files after the last attempt waits for the job's ordinary
//! schedule like any other.

use chrono::Duration;

use crate::model::RetrySettings;
use crate::state::{DestinationRun, JobRun, RunStatus};

/// What is wrong with the last run, if anything worth another attempt is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shortfall {
    /// A destination failed outright.
    DestinationFailed { name: String },
    /// The snapshot was made, but files could not be read and were left out.
    FilesUnread { destination: String, count: u64 },
}

impl Shortfall {
    /// One sentence, for the event log and the job's history.
    pub fn summary(&self) -> String {
        match self {
            Shortfall::DestinationFailed { name } => {
                format!("{name:?} failed")
            }
            Shortfall::FilesUnread { destination, count } => {
                let files = if *count == 1 { "file" } else { "files" };
                format!("{count} {files} could not be read into {destination:?}")
            }
        }
    }
}

/// Everything about the last run that another attempt might fix.
///
/// Empty means the run got everything it was asked for, which is also what
/// resets the attempt counter.
pub fn shortfalls(run: &JobRun) -> Vec<Shortfall> {
    let mut out = Vec::new();
    for destination in &run.destinations {
        match destination.status {
            RunStatus::Failed => out
                .push(Shortfall::DestinationFailed { name: destination.destination_name.clone() }),
            _ => {
                if let Some(unread) = unread_files(destination) {
                    out.push(Shortfall::FilesUnread {
                        destination: destination.destination_name.clone(),
                        count: unread,
                    });
                }
            }
        }
    }
    out
}

/// Files this destination's snapshot is missing because they could not be read.
///
/// `errors_ignored` is kopia's count of exactly that, and it is only non-zero
/// because superbackup asks kopia to carry on past them rather than abandon the
/// snapshot. A run of 110,052 files that kept 110,049 is a good run and a
/// successful one — and it is still missing three files, which is the whole
/// reason to come back later.
fn unread_files(destination: &DestinationRun) -> Option<u64> {
    match destination.progress.errors_ignored {
        0 => None,
        n => Some(n),
    }
}

/// How long to wait before running this job again, if it should be.
///
/// `attempts_so_far` counts the retries already made since the last clean run,
/// not the runs. Zero means the run that just finished was the job's ordinary
/// one.
///
/// `None` covers every reason not to: retrying switched off, nothing missing, a
/// run the user cancelled, a rehearsal that wrote nothing anyway, or the
/// attempts exhausted.
pub fn delay_after(
    settings: &RetrySettings,
    run: &JobRun,
    attempts_so_far: u32,
) -> Option<Duration> {
    if !settings.enabled {
        return None;
    }
    // A run nobody let finish says nothing about whether the files are
    // readable, and starting it again is the opposite of what stopping it
    // asked for.
    if matches!(run.status, RunStatus::Cancelled | RunStatus::Skipped) {
        return None;
    }
    // A rehearsal wrote nothing anywhere, so there is nothing missing from
    // anything.
    if run.trigger.is_rehearsal() {
        return None;
    }
    if shortfalls(run).is_empty() {
        return None;
    }
    if settings.max_attempts > 0 && attempts_so_far >= settings.max_attempts {
        return None;
    }
    Some(Duration::minutes(settings.after_minutes.max(1) as i64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Progress, Trigger};
    use uuid::Uuid;

    fn destination(name: &str, status: RunStatus, errors_ignored: u64) -> DestinationRun {
        DestinationRun {
            destination_id: Uuid::new_v4(),
            destination_name: name.to_string(),
            status,
            started_at: None,
            finished_at: None,
            progress: Progress { errors_ignored, ..Progress::default() },
            snapshot_id: None,
            error: None,
            warnings: Vec::new(),
            notes: Vec::new(),
            replicated_from: None,
            skipped_reason: None,
        }
    }

    fn run(status: RunStatus, destinations: Vec<DestinationRun>) -> JobRun {
        JobRun {
            run_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            job_name: "Workspace".into(),
            trigger: Trigger::Schedule,
            status,
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            destinations,
            skipped_because: None,
        }
    }

    /// The case this exists for: a good run that is still missing files.
    ///
    /// 110,052 files walked, 110,049 kept, three held open by an editor. The
    /// run succeeded — the snapshot is real and restorable — and the snapshot
    /// does not contain those three, which no amount of retrying in five
    /// seconds fixes and which closing the editor does.
    #[test]
    fn a_successful_run_that_left_files_unread_is_tried_again() {
        let run = run(
            RunStatus::SucceededWithWarnings,
            vec![destination("OneDrive", RunStatus::SucceededWithWarnings, 3)],
        );
        let settings = RetrySettings::default();

        let found = shortfalls(&run);
        assert_eq!(
            found,
            vec![Shortfall::FilesUnread { destination: "OneDrive".into(), count: 3 }]
        );
        assert!(found[0].summary().contains("3 files"), "{}", found[0].summary());

        let delay = delay_after(&settings, &run, 0).expect("another attempt");
        assert_eq!(delay, Duration::minutes(settings.after_minutes as i64));
    }

    /// A clean run is left alone, and resets nothing to argue about.
    #[test]
    fn a_clean_run_is_not_repeated() {
        let run = run(RunStatus::Succeeded, vec![destination("OneDrive", RunStatus::Succeeded, 0)]);
        assert!(shortfalls(&run).is_empty());
        assert_eq!(delay_after(&RetrySettings::default(), &run, 0), None);
    }

    /// A failed destination earns an attempt too.
    #[test]
    fn a_failed_destination_is_tried_again() {
        let run = run(RunStatus::Failed, vec![destination("StorJ", RunStatus::Failed, 0)]);
        let found = shortfalls(&run);
        assert_eq!(found, vec![Shortfall::DestinationFailed { name: "StorJ".into() }]);
        assert!(delay_after(&RetrySettings::default(), &run, 0).is_some());
    }

    /// Stopping a run means stopping it.
    #[test]
    fn a_cancelled_run_is_never_restarted_on_a_timer() {
        let run = run(RunStatus::Cancelled, vec![destination("OneDrive", RunStatus::Cancelled, 7)]);
        assert_eq!(
            delay_after(&RetrySettings::default(), &run, 0),
            None,
            "a run the user stopped must not start itself again"
        );
    }

    /// A rehearsal wrote nothing, so nothing is missing from anything.
    #[test]
    fn a_preview_is_never_retried() {
        let mut run = run(
            RunStatus::SucceededWithWarnings,
            vec![destination("OneDrive", RunStatus::SucceededWithWarnings, 3)],
        );
        run.trigger = Trigger::Preview;
        assert_eq!(delay_after(&RetrySettings::default(), &run, 0), None);
    }

    /// Some files are never readable, and the attempts have to stop.
    ///
    /// A lock file that lives as long as the application does would otherwise
    /// mean a full scan of the source tree every half hour, for ever, for a
    /// file that cannot be read by design.
    #[test]
    fn the_attempts_are_bounded() {
        let run = run(
            RunStatus::SucceededWithWarnings,
            vec![destination("OneDrive", RunStatus::SucceededWithWarnings, 1)],
        );
        let settings = RetrySettings { max_attempts: 2, ..RetrySettings::default() };
        assert!(delay_after(&settings, &run, 0).is_some(), "first extra attempt");
        assert!(delay_after(&settings, &run, 1).is_some(), "second extra attempt");
        assert_eq!(delay_after(&settings, &run, 2), None, "and then it waits for the schedule");

        // Zero means "until the next scheduled run comes round", which is its
        // own bound: the schedule resets the count.
        let unbounded = RetrySettings { max_attempts: 0, ..RetrySettings::default() };
        assert!(delay_after(&unbounded, &run, 99).is_some());
    }

    /// Switched off is switched off, whatever the run says.
    #[test]
    fn retrying_can_be_turned_off() {
        let run = run(RunStatus::Failed, vec![destination("OneDrive", RunStatus::Failed, 12)]);
        let settings = RetrySettings { enabled: false, ..RetrySettings::default() };
        assert_eq!(delay_after(&settings, &run, 0), None);
    }

    /// The interval is never zero, however it was configured.
    ///
    /// A zero-minute retry is a job that starts again the instant it finishes,
    /// which on a fifteen-gigabyte source is a machine that never stops
    /// scanning.
    #[test]
    fn the_interval_has_a_floor() {
        let run = run(RunStatus::Failed, vec![destination("OneDrive", RunStatus::Failed, 0)]);
        let settings = RetrySettings { after_minutes: 0, ..RetrySettings::default() };
        assert_eq!(delay_after(&settings, &run, 0), Some(Duration::minutes(1)));
    }
}
