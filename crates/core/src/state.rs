//! Runtime state: what is running, what it is doing, and what happened last.
//!
//! This is the payload behind the tray icon colour, the dashboard, the
//! progress bars, and `superbackup status --json`. It is kept strictly
//! separate from [`crate::model::Config`] because config is user intent and
//! may be pulled from a shared Git repository, whereas state is local history
//! that must never be overwritten by such a pull.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Aggregate health
// ---------------------------------------------------------------------------

/// The single value that drives the tray icon. Ordered worst-last so that
/// `max()` over every job yields the icon to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// Everything succeeded recently and nothing is scheduled right now.
    Idle,
    /// At least one job is running.
    Running,
    /// Nothing is broken, but something needs attention: a job has not run in
    /// a while, or the vault is locked so schedules cannot fire.
    Attention,
    /// Global pause is in effect. Not an error, but visually distinct so the
    /// user cannot forget they turned backups off.
    Paused,
    /// A job or the service reported a failure.
    Failed,
}

impl Health {
    pub fn title(&self) -> &'static str {
        match self {
            Health::Idle => "Up to date",
            Health::Running => "Backing up",
            Health::Attention => "Needs attention",
            Health::Paused => "Paused",
            Health::Failed => "Backup failed",
        }
    }
    /// Icon asset stem, resolved against `assets/tray/`.
    pub fn icon_stem(&self) -> &'static str {
        match self {
            Health::Idle => "idle",
            Health::Running => "running",
            Health::Attention => "attention",
            Health::Paused => "paused",
            Health::Failed => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Job run state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Preparing,
    Running,
    Finalising,
    Succeeded,
    /// Completed, but kopia reported ignored or unreadable files.
    SucceededWithWarnings,
    Failed,
    Cancelled,
    /// Skipped by policy: paused, metered connection, on battery, disabled.
    Skipped,
}

impl RunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunStatus::Succeeded
                | RunStatus::SucceededWithWarnings
                | RunStatus::Failed
                | RunStatus::Cancelled
                | RunStatus::Skipped
        )
    }
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            RunStatus::Queued | RunStatus::Preparing | RunStatus::Running | RunStatus::Finalising
        )
    }
    pub fn title(&self) -> &'static str {
        match self {
            RunStatus::Queued => "Queued",
            RunStatus::Preparing => "Preparing",
            RunStatus::Running => "Running",
            RunStatus::Finalising => "Finalising",
            RunStatus::Succeeded => "Succeeded",
            RunStatus::SucceededWithWarnings => "Completed with warnings",
            RunStatus::Failed => "Failed",
            RunStatus::Cancelled => "Cancelled",
            RunStatus::Skipped => "Skipped",
        }
    }
}

/// Live progress for one job against one destination.
///
/// Populated from kopia's `--progress=json` stream for repository
/// destinations, and from the mirror engine's own counters for plain copies.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Progress {
    pub files_processed: u64,
    /// Total files, once the estimate phase has produced one.
    pub files_total: Option<u64>,
    pub bytes_processed: u64,
    pub bytes_total: Option<u64>,
    /// Bytes actually uploaded after dedup and compression — the number that
    /// matters for a metered or slow link.
    pub bytes_uploaded: u64,
    pub bytes_per_second: f64,
    pub files_cached: u64,
    pub errors_ignored: u64,
    /// Path currently being read, for the "scanning …" line in the GUI.
    pub current_path: Option<String>,
    pub estimated_seconds_remaining: Option<u64>,
}

impl Progress {
    /// 0.0..=1.0, or `None` while kopia is still estimating.
    pub fn fraction(&self) -> Option<f32> {
        match self.bytes_total {
            Some(total) if total > 0 => {
                Some((self.bytes_processed as f64 / total as f64).clamp(0.0, 1.0) as f32)
            }
            _ => match self.files_total {
                Some(total) if total > 0 => {
                    Some((self.files_processed as f64 / total as f64).clamp(0.0, 1.0) as f32)
                }
                _ => None,
            },
        }
    }
}

/// One job execution against one destination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationRun {
    pub destination_id: Uuid,
    pub destination_name: String,
    pub status: RunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub progress: Progress,
    /// Kopia snapshot id, when one was created.
    #[serde(default)]
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub error: Option<RunError>,
    /// Non-fatal problems: unreadable files, skipped paths.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Set when this destination was filled by **replicating** another
    /// destination's repository rather than by backing up the job's sources.
    ///
    /// The fan-out is never flattened: a replica is still its own
    /// `DestinationRun`, with its own status, progress and error. This field is
    /// what lets a reader tell the two apart, and it carries the source's name
    /// as it stood at run time so a later rename cannot make the history lie.
    #[serde(default)]
    pub replicated_from: Option<ReplicationOrigin>,
    /// Why a [`RunStatus::Skipped`] destination was skipped, in words a user
    /// can act on.
    ///
    /// Separate from `error` on purpose. "Your offsite copy was not made
    /// because the local backup it copies from failed" is not itself a
    /// failure of the offsite destination, and recording it as one would put a
    /// red error on a destination that is fine and hide the destination that
    /// actually broke.
    #[serde(default)]
    pub skipped_reason: Option<String>,
}

impl DestinationRun {
    /// True when this destination was replicated from another rather than
    /// backed up from the job's sources.
    pub fn is_replica(&self) -> bool {
        self.replicated_from.is_some()
    }
}

/// The destination a replica was copied from, as it was named at run time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicationOrigin {
    pub destination_id: Uuid,
    pub destination_name: String,
}

/// A failure, in a form safe to show in a notification and to log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunError {
    pub code: crate::error::ErrorCode,
    pub message: String,
    #[serde(default)]
    pub hint: Option<String>,
    /// Trimmed tail of kopia's stderr, with anything resembling a credential
    /// already redacted by the driver.
    #[serde(default)]
    pub detail: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

/// One execution of one job, fanned out over its destinations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRun {
    pub run_id: Uuid,
    pub job_id: Uuid,
    pub job_name: String,
    pub trigger: Trigger,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub destinations: Vec<DestinationRun>,
}

impl JobRun {
    /// Aggregate progress across every destination in this run.
    pub fn overall_fraction(&self) -> Option<f32> {
        let fractions: Vec<f32> =
            self.destinations.iter().filter_map(|d| d.progress.fraction()).collect();
        if fractions.is_empty() {
            return None;
        }
        Some(fractions.iter().sum::<f32>() / fractions.len() as f32)
    }

    /// Roll destination outcomes up into the job's own status. Worst wins,
    /// except that a partial success is still reported as a warning so the
    /// user is never told "succeeded" when a destination was skipped.
    pub fn derive_status(&self) -> RunStatus {
        if self.destinations.iter().any(|d| d.status.is_active()) {
            return RunStatus::Running;
        }
        if self.destinations.iter().any(|d| d.status == RunStatus::Failed) {
            return RunStatus::Failed;
        }
        if self.destinations.iter().any(|d| d.status == RunStatus::Cancelled) {
            return RunStatus::Cancelled;
        }
        if self.destinations.is_empty() {
            return RunStatus::Skipped;
        }
        if self
            .destinations
            .iter()
            .any(|d| matches!(d.status, RunStatus::SucceededWithWarnings | RunStatus::Skipped))
        {
            return RunStatus::SucceededWithWarnings;
        }
        RunStatus::Succeeded
    }

    pub fn duration_seconds(&self) -> Option<i64> {
        self.finished_at.map(|f| (f - self.started_at).num_seconds())
    }
}

/// What caused a run to start. Recorded so the history can answer "why did
/// this fire at 3am?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Schedule,
    Manual,
    Cli,
    FileChange,
    /// A schedule that elapsed while the machine was off or asleep.
    CatchUp,
    /// Retry after a previous failure.
    Retry,
    /// A rehearsal: the run was asked for with `dry_run`, so it reports what it
    /// would have copied and wrote nothing anywhere.
    ///
    /// Carried on the run rather than inferred by the caller because the
    /// distinction outlives the request. A preview lands in the history like
    /// any other run, and a history that cannot tell a rehearsal from a real
    /// backup would let a user believe a destination holds data it does not.
    Preview,
}

impl Trigger {
    /// True when nothing was written by the run this trigger started.
    ///
    /// The one predicate every screen that renders a run should consult before
    /// it says "backed up".
    pub fn is_rehearsal(&self) -> bool {
        matches!(self, Trigger::Preview)
    }
}

// ---------------------------------------------------------------------------
// Persisted per-job summary
// ---------------------------------------------------------------------------

/// The durable summary shown in the job list without loading full history.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobSummary {
    pub last_run: Option<DateTime<Utc>>,
    pub last_success: Option<DateTime<Utc>>,
    pub last_status: Option<RunStatus>,
    pub last_error: Option<RunError>,
    pub next_run: Option<DateTime<Utc>>,
    pub consecutive_failures: u32,
    pub total_runs: u64,
    /// Bytes uploaded on the last successful run, per destination.
    #[serde(default)]
    pub last_uploaded_bytes: u64,
    pub average_duration_seconds: Option<i64>,
}

impl JobSummary {
    /// A job that has not succeeded within `stale_after_days` deserves the
    /// attention badge even though nothing has actually errored.
    pub fn is_stale(&self, stale_after_days: u32, now: DateTime<Utc>) -> bool {
        if stale_after_days == 0 {
            return false;
        }
        match self.last_success {
            None => self.total_runs > 0,
            Some(t) => (now - t).num_days() > stale_after_days as i64,
        }
    }
}

// ---------------------------------------------------------------------------
// Whole-application snapshot
// ---------------------------------------------------------------------------

/// Everything the tray, the GUI and `superbackup status` need, in one message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub health: Health,
    pub version: String,
    /// The editable friendly name. Defaults to the hostname but need not stay
    /// equal to it.
    pub machine_label: String,
    /// The machine's actual hostname.
    ///
    /// Carried separately from the label because the sidebar must be able to
    /// show what the computer *is* even after someone renames the label, and
    /// "which machine am I looking at" is the question that matters when
    /// several of them write to one destination.
    #[serde(default)]
    pub machine_hostname: String,
    pub machine_slug: String,
    /// `false` when the vault is locked, which blocks every scheduled run.
    pub unlocked: bool,
    pub paused: bool,
    pub paused_until: Option<DateTime<Utc>>,
    pub service_installed: bool,
    pub service_running: bool,
    pub kopia_version: Option<String>,
    pub active_runs: Vec<JobRun>,
    /// Keyed by job id.
    pub jobs: BTreeMap<Uuid, JobSummary>,
    pub next_scheduled: Option<(Uuid, DateTime<Utc>)>,
    pub recent_events: Vec<Event>,
    pub uptime_seconds: u64,
    pub generated_at: DateTime<Utc>,
}

impl StatusSnapshot {
    /// The tray icon to show, derived from the parts. Kept as a function so
    /// the daemon and the GUI can never disagree about the rule.
    pub fn derive_health(
        unlocked: bool,
        paused: bool,
        active: bool,
        any_failed: bool,
        any_stale: bool,
    ) -> Health {
        if any_failed {
            Health::Failed
        } else if active {
            Health::Running
        } else if paused {
            Health::Paused
        } else if !unlocked || any_stale {
            Health::Attention
        } else {
            Health::Idle
        }
    }
}

// ---------------------------------------------------------------------------
// Event log
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Debug,
    Info,
    Warning,
    Error,
}

/// One line in the append-only activity log. Rendered in the GUI's Activity
/// tab and streamed by `superbackup watch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    pub severity: Severity,
    /// Short machine-readable kind, e.g. `job.started`, `repo.created`,
    /// `vault.unlocked`, `service.error`.
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub job_id: Option<Uuid>,
    #[serde(default)]
    pub destination_id: Option<Uuid>,
    #[serde(default)]
    pub run_id: Option<Uuid>,
    /// Structured extras, never containing secret material.
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

impl Event {
    pub fn new(severity: Severity, kind: impl Into<String>, message: impl Into<String>) -> Event {
        Event {
            id: Uuid::new_v4(),
            at: Utc::now(),
            severity,
            kind: kind.into(),
            message: message.into(),
            job_id: None,
            destination_id: None,
            run_id: None,
            fields: BTreeMap::new(),
        }
    }
    pub fn info(kind: impl Into<String>, message: impl Into<String>) -> Event {
        Event::new(Severity::Info, kind, message)
    }
    pub fn warn(kind: impl Into<String>, message: impl Into<String>) -> Event {
        Event::new(Severity::Warning, kind, message)
    }
    pub fn error(kind: impl Into<String>, message: impl Into<String>) -> Event {
        Event::new(Severity::Error, kind, message)
    }
    pub fn with_job(mut self, job_id: Uuid) -> Event {
        self.job_id = Some(job_id);
        self
    }
    pub fn with_run(mut self, run_id: Uuid) -> Event {
        self.run_id = Some(run_id);
        self
    }
    pub fn with_destination(mut self, destination_id: Uuid) -> Event {
        self.destination_id = Some(destination_id);
        self
    }
    pub fn with_field(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Event {
        self.fields.insert(key.into(), value.into());
        self
    }
}

/// The persisted half of runtime state, written to `state.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedState {
    pub jobs: BTreeMap<Uuid, JobSummary>,
    /// Bounded run history, newest first.
    pub history: Vec<JobRun>,
    /// Successful snapshot count per destination, for maintenance scheduling.
    pub runs_since_maintenance: BTreeMap<Uuid, u32>,
}

/// How many completed runs are kept in `state.json`. Older ones survive only
/// in the event log, which rotates on its own schedule.
pub const MAX_HISTORY: usize = 200;

impl PersistedState {
    pub fn record(&mut self, run: JobRun) {
        let summary = self.jobs.entry(run.job_id).or_default();
        summary.total_runs += 1;
        summary.last_run = Some(run.started_at);
        summary.last_status = Some(run.status);
        match run.status {
            RunStatus::Succeeded | RunStatus::SucceededWithWarnings => {
                summary.last_success = run.finished_at.or(Some(run.started_at));
                summary.consecutive_failures = 0;
                summary.last_error = None;
                summary.last_uploaded_bytes =
                    run.destinations.iter().map(|d| d.progress.bytes_uploaded).sum();
            }
            RunStatus::Failed => {
                summary.consecutive_failures += 1;
                summary.last_error = run.destinations.iter().find_map(|d| d.error.clone());
            }
            _ => {}
        }
        if let Some(secs) = run.duration_seconds() {
            summary.average_duration_seconds = Some(match summary.average_duration_seconds {
                // Exponential moving average; recent runs dominate, which is
                // what makes the "about 4 minutes" estimate useful.
                Some(prev) => (prev * 3 + secs) / 4,
                None => secs,
            });
        }

        self.history.insert(0, run);
        self.history.truncate(MAX_HISTORY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(status: RunStatus) -> DestinationRun {
        DestinationRun {
            destination_id: Uuid::new_v4(),
            destination_name: "d".into(),
            status,
            started_at: None,
            finished_at: None,
            progress: Progress::default(),
            snapshot_id: None,
            error: None,
            warnings: vec![],
            replicated_from: None,
            skipped_reason: None,
        }
    }

    fn run(destinations: Vec<DestinationRun>) -> JobRun {
        JobRun {
            run_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            job_name: "j".into(),
            trigger: Trigger::Manual,
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            destinations,
        }
    }

    #[test]
    fn one_failed_destination_fails_the_run() {
        let r = run(vec![dest(RunStatus::Succeeded), dest(RunStatus::Failed)]);
        assert_eq!(r.derive_status(), RunStatus::Failed);
    }

    #[test]
    fn partial_success_is_never_reported_as_clean_success() {
        let r = run(vec![dest(RunStatus::Succeeded), dest(RunStatus::Skipped)]);
        assert_eq!(r.derive_status(), RunStatus::SucceededWithWarnings);
    }

    #[test]
    fn all_succeeded_is_success() {
        let r = run(vec![dest(RunStatus::Succeeded), dest(RunStatus::Succeeded)]);
        assert_eq!(r.derive_status(), RunStatus::Succeeded);
    }

    #[test]
    fn health_precedence_puts_failure_first() {
        assert_eq!(StatusSnapshot::derive_health(true, true, true, true, true), Health::Failed);
        assert_eq!(StatusSnapshot::derive_health(true, true, false, false, false), Health::Paused);
        assert_eq!(
            StatusSnapshot::derive_health(false, false, false, false, false),
            Health::Attention,
            "a locked vault blocks schedules and must be visible"
        );
        assert_eq!(StatusSnapshot::derive_health(true, false, false, false, false), Health::Idle);
    }

    #[test]
    fn history_is_bounded() {
        let mut st = PersistedState::default();
        let job_id = Uuid::new_v4();
        for _ in 0..(MAX_HISTORY + 50) {
            let mut r = run(vec![dest(RunStatus::Succeeded)]);
            r.job_id = job_id;
            r.status = RunStatus::Succeeded;
            st.record(r);
        }
        assert_eq!(st.history.len(), MAX_HISTORY);
        assert_eq!(st.jobs[&job_id].total_runs as usize, MAX_HISTORY + 50);
        assert_eq!(st.jobs[&job_id].consecutive_failures, 0);
    }

    #[test]
    fn failures_increment_then_reset() {
        let mut st = PersistedState::default();
        let job_id = Uuid::new_v4();
        for _ in 0..3 {
            let mut r = run(vec![dest(RunStatus::Failed)]);
            r.job_id = job_id;
            r.status = RunStatus::Failed;
            st.record(r);
        }
        assert_eq!(st.jobs[&job_id].consecutive_failures, 3);
        let mut ok = run(vec![dest(RunStatus::Succeeded)]);
        ok.job_id = job_id;
        ok.status = RunStatus::Succeeded;
        st.record(ok);
        assert_eq!(st.jobs[&job_id].consecutive_failures, 0);
    }

    #[test]
    fn stale_detection_respects_zero_as_disabled() {
        let s = JobSummary { last_success: None, total_runs: 5, ..Default::default() };
        assert!(s.is_stale(3, Utc::now()));
        assert!(!s.is_stale(0, Utc::now()));
    }
}
