//! What each list and card actually says.
//!
//! Filtering, sorting, grouping, the human schedule strings and the job-card
//! state machine live here rather than inside a `show()` function, so the rules
//! can be tested against the specification without a rendering context.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use superbackup_core::model::{Destination, Job, Schedule};
use superbackup_core::state::{Event, JobRun, JobSummary, RunStatus, Severity};

use super::copy;
use super::data::Data;
use super::format;

// ---------------------------------------------------------------------------
// Human schedule strings (UX_SPEC §6.4)
// ---------------------------------------------------------------------------

/// Rendered identically in the job list, on cards, and in the wizard.
pub fn schedule_string(schedule: &Schedule) -> String {
    match schedule {
        Schedule::Manual => copy::job::SCHEDULE_MANUAL.to_string(),
        Schedule::Interval { minutes } => {
            if *minutes == 60 {
                "Every hour".to_string()
            } else if *minutes % 60 == 0 && *minutes > 0 {
                format!("Every {} hours", minutes / 60)
            } else {
                format!("Every {minutes} minutes")
            }
        }
        Schedule::Daily { times } => daily_string(times),
        Schedule::Weekly { weekdays, times } => {
            let mut days: Vec<u8> = weekdays.clone();
            days.sort_unstable();
            days.dedup();
            if days.len() >= 7 || days.is_empty() {
                return daily_string(times);
            }
            format!("{} at {}", format::weekdays(&days), times_list(times))
        }
        Schedule::Cron { expression } => format!("Cron: {expression}"),
        Schedule::OnChange { debounce_seconds, min_interval_minutes } => {
            let quiet = if *debounce_seconds >= 60 {
                format!("{} min quiet", debounce_seconds / 60)
            } else {
                format!("{debounce_seconds} s quiet")
            };
            format!("When files change ({quiet}, at most every {min_interval_minutes} min)")
        }
    }
}

fn daily_string(times: &[superbackup_core::model::TimeOfDay]) -> String {
    match times.len() {
        0 => copy::job::SCHEDULE_MANUAL.to_string(),
        1 | 2 => format!("Daily at {}", times_list(times)),
        n => format!("Daily, {n} times a day"),
    }
}

fn times_list(times: &[superbackup_core::model::TimeOfDay]) -> String {
    let mut sorted: Vec<String> = {
        let mut t = times.to_vec();
        t.sort();
        t.iter().map(|t| t.to_string()).collect()
    };
    match sorted.len() {
        0 => String::new(),
        1 => sorted.remove(0),
        2 => format!("{} and {}", sorted[0], sorted[1]),
        _ => sorted.join(", "),
    }
}

/// The next five fire times, for the summary strip under the schedule editor.
/// Interval and cron schedules are computed by the daemon, so this returns
/// what can be worked out locally and says so when it cannot.
pub fn next_runs(schedule: &Schedule, from: DateTime<Utc>, count: usize) -> Vec<DateTime<Utc>> {
    use chrono::{Local, TimeZone, Timelike};
    let mut out = Vec::new();
    match schedule {
        Schedule::Manual | Schedule::OnChange { .. } => {}
        Schedule::Interval { minutes } => {
            let step = Duration::minutes((*minutes).max(1) as i64);
            let mut at = from + step;
            for _ in 0..count {
                out.push(at);
                at += step;
            }
        }
        Schedule::Daily { times } | Schedule::Weekly { times, .. } => {
            let weekdays = match schedule {
                Schedule::Weekly { weekdays, .. } => weekdays.clone(),
                _ => vec![],
            };
            let local_now = from.with_timezone(&Local);
            let mut day = 0i64;
            while out.len() < count && day < 60 {
                let date = (local_now + Duration::days(day)).date_naive();
                let is_wanted = weekdays.is_empty() || {
                    let index = date.format("%u").to_string().parse::<u8>().unwrap_or(1) - 1;
                    weekdays.contains(&index)
                };
                if is_wanted {
                    let mut sorted = times.to_vec();
                    sorted.sort();
                    for t in &sorted {
                        if let Some(naive) =
                            date.and_hms_opt(t.hour as u32, t.minute as u32, 0)
                        {
                            if let Some(local) =
                                Local.from_local_datetime(&naive).earliest()
                            {
                                let utc = local.with_timezone(&Utc);
                                if utc > from && out.len() < count {
                                    out.push(utc);
                                }
                            }
                        }
                    }
                }
                day += 1;
            }
            let _ = local_now.hour();
        }
        // A cron expression needs the daemon's evaluator; the editor shows the
        // parse result instead of inventing an answer here.
        Schedule::Cron { .. } => {}
    }
    out
}

// ---------------------------------------------------------------------------
// Job card state (UX_SPEC §5.4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum CardState {
    Running { fraction: Option<f32>, rate: f64 },
    Queued { behind: String },
    Failed { consecutive: u32 },
    Warnings { skipped: u64 },
    Succeeded,
    Stale,
    NeverRun,
    Disabled { last: Option<RunStatus> },
}

/// Everything the dashboard card and the jobs row need, in one value.
#[derive(Debug, Clone, PartialEq)]
pub struct JobView {
    pub state: CardState,
    /// The badge word. Never colour alone.
    pub badge: String,
    /// Row 2 of the card.
    pub meta: String,
    /// The status the spine and the badge colour come from.
    pub status: RunStatus,
}

pub fn job_view(data: &Data, job: &Job, now: DateTime<Utc>) -> JobView {
    let summary = data.summary_for(&job.id).unwrap_or_default();
    let active = data.active_run_for(&job.id);

    if !job.enabled {
        let meta = match (summary.last_status, summary.last_run) {
            (Some(status), Some(at)) => {
                copy::card_meta_disabled(status.title(), &format::relative_past(at, now))
            }
            _ => copy::badge::DISABLED.to_string(),
        };
        return JobView {
            state: CardState::Disabled { last: summary.last_status },
            badge: copy::badge::DISABLED.to_string(),
            meta,
            status: summary.last_status.unwrap_or(RunStatus::Skipped),
        };
    }

    if let Some(run) = active {
        if run.status == RunStatus::Queued
            || run.destinations.iter().all(|d| d.status == RunStatus::Queued)
        {
            let behind = data
                .active_runs()
                .iter()
                .find(|r| r.run_id != run.run_id)
                .map(|r| r.job_name.clone())
                .unwrap_or_else(|| copy::state::UNKNOWN.to_string());
            return JobView {
                state: CardState::Queued { behind: behind.clone() },
                badge: RunStatus::Queued.title().to_string(),
                meta: copy::card_meta_queued(&behind),
                status: RunStatus::Queued,
            };
        }
        let rate: f64 = run.destinations.iter().map(|d| d.progress.bytes_per_second).sum();
        return JobView {
            state: CardState::Running { fraction: run.overall_fraction(), rate },
            badge: RunStatus::Running.title().to_string(),
            meta: copy::card_meta_running(
                &format::relative_past(run.started_at, now),
                copy::trigger(run.trigger),
            ),
            status: RunStatus::Running,
        };
    }

    let stale_days = data.settings.notifications.stale_after_days;
    match summary.last_status {
        None => {
            let next = summary
                .next_run
                .map(|t| format::relative_future(t, now))
                .unwrap_or_else(|| copy::state::UNKNOWN.to_string());
            JobView {
                state: CardState::NeverRun,
                badge: copy::badge::NEVER_RUN.to_string(),
                meta: if job.schedule.is_automatic() {
                    copy::card_meta_never(job.sources.len(), &next)
                } else {
                    copy::card_meta_never_manual(job.sources.len())
                },
                status: RunStatus::Skipped,
            }
        }
        Some(RunStatus::Failed) => {
            let when = summary
                .last_run
                .map(|t| format::relative_past(t, now))
                .unwrap_or_else(|| copy::state::UNKNOWN.to_string());
            let meta = if summary.consecutive_failures > 1 {
                copy::card_meta_failed(&when, &format::ordinal(summary.consecutive_failures))
            } else {
                copy::card_meta_failed_first(&when)
            };
            JobView {
                state: CardState::Failed { consecutive: summary.consecutive_failures },
                badge: RunStatus::Failed.title().to_string(),
                meta,
                status: RunStatus::Failed,
            }
        }
        Some(status) => {
            // A stale job takes the badge even though its last run succeeded:
            // "succeeded a fortnight ago" is not a healthy backup.
            if summary.is_stale(stale_days, now) {
                let when = summary
                    .last_success
                    .map(|t| format::relative_past(t, now))
                    .unwrap_or_else(|| copy::state::NEVER.to_string());
                return JobView {
                    state: CardState::Stale,
                    badge: copy::badge::WARNINGS_SHORT.to_string(),
                    meta: copy::card_meta_stale(&when),
                    status: RunStatus::SucceededWithWarnings,
                };
            }
            let when = summary
                .last_run
                .map(|t| format::relative_past(t, now))
                .unwrap_or_else(|| copy::state::UNKNOWN.to_string());
            let duration = summary
                .average_duration_seconds
                .map(format::duration)
                .unwrap_or_else(|| copy::state::UNKNOWN.to_lowercase());
            if status == RunStatus::SucceededWithWarnings {
                let skipped = data
                    .history
                    .iter()
                    .find(|r| r.job_id == job.id)
                    .map(|r| {
                        r.destinations.iter().map(|d| d.progress.errors_ignored).sum::<u64>()
                    })
                    .unwrap_or(0);
                return JobView {
                    state: CardState::Warnings { skipped },
                    badge: copy::badge::WARNINGS_SHORT.to_string(),
                    meta: copy::card_meta_warnings(&when, &duration, skipped),
                    status,
                };
            }
            JobView {
                state: CardState::Succeeded,
                badge: RunStatus::Succeeded.title().to_string(),
                meta: copy::card_meta_succeeded(&when, &duration, summary.last_uploaded_bytes),
                status,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Job list filtering, sorting and grouping (UX_SPEC §6.1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobFilter {
    All,
    Enabled,
    Disabled,
    Failing,
    Stale,
}

impl JobFilter {
    pub const ALL: [JobFilter; 5] = [
        JobFilter::All,
        JobFilter::Enabled,
        JobFilter::Disabled,
        JobFilter::Failing,
        JobFilter::Stale,
    ];
    pub fn title(self) -> &'static str {
        match self {
            JobFilter::All => copy::jobs::FILTER_ALL,
            JobFilter::Enabled => copy::jobs::FILTER_ENABLED,
            JobFilter::Disabled => copy::jobs::FILTER_DISABLED,
            JobFilter::Failing => copy::jobs::FILTER_FAILING,
            JobFilter::Stale => copy::jobs::FILTER_STALE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    None,
    Project,
    Schedule,
}

impl GroupBy {
    pub const ALL: [GroupBy; 3] = [GroupBy::None, GroupBy::Project, GroupBy::Schedule];
    pub fn title(self) -> &'static str {
        match self {
            GroupBy::None => copy::jobs::GROUP_NONE,
            GroupBy::Project => copy::jobs::GROUP_PROJECT,
            GroupBy::Schedule => copy::jobs::GROUP_SCHEDULE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Status,
    Name,
    Sources,
    Destinations,
    Schedule,
    LastRun,
    NextRun,
    Uploaded,
}

/// Problems first. This is the order the default sort uses, and it is why the
/// jobs list is worth opening at all.
fn status_rank(view: &JobView) -> u8 {
    match view.state {
        CardState::Failed { .. } => 0,
        CardState::Warnings { .. } | CardState::Stale => 1,
        CardState::Running { .. } | CardState::Queued { .. } => 2,
        CardState::NeverRun => 3,
        CardState::Succeeded => 4,
        CardState::Disabled { .. } => 5,
    }
}

/// Does this job match the search box? Name, description, tags and source
/// paths, because "which job backs up ~/dev" is a real question.
pub fn matches_search(job: &Job, needle: &str) -> bool {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    if job.name.to_lowercase().contains(&needle)
        || job.description.to_lowercase().contains(&needle)
    {
        return true;
    }
    if job.tags.iter().any(|t| t.to_lowercase().contains(&needle)) {
        return true;
    }
    job.sources
        .iter()
        .any(|s| s.path.to_string_lossy().to_lowercase().contains(&needle))
}

/// The visible, ordered job list.
pub fn visible_jobs<'a>(
    data: &'a Data,
    search: &str,
    filter: JobFilter,
    sort: SortKey,
    descending: bool,
    now: DateTime<Utc>,
) -> Vec<(&'a Job, JobView)> {
    let stale_days = data.settings.notifications.stale_after_days;
    let mut rows: Vec<(&Job, JobView)> = data
        .jobs
        .iter()
        .filter(|job| matches_search(job, search))
        .filter(|job| match filter {
            JobFilter::All => true,
            JobFilter::Enabled => job.enabled,
            JobFilter::Disabled => !job.enabled,
            JobFilter::Failing => {
                data.summary_for(&job.id).map(|s| s.last_status) == Some(Some(RunStatus::Failed))
            }
            JobFilter::Stale => data
                .summary_for(&job.id)
                .map(|s| s.is_stale(stale_days, now))
                .unwrap_or(false),
        })
        .map(|job| {
            let view = job_view(data, job, now);
            (job, view)
        })
        .collect();

    rows.sort_by(|(a, av), (b, bv)| {
        let sa = data.summary_for(&a.id).unwrap_or_default();
        let sb = data.summary_for(&b.id).unwrap_or_default();
        let ordering = match sort {
            SortKey::Status => status_rank(av)
                .cmp(&status_rank(bv))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Sources => a.sources.len().cmp(&b.sources.len()),
            SortKey::Destinations => a.destination_ids.len().cmp(&b.destination_ids.len()),
            SortKey::Schedule => schedule_string(&a.schedule).cmp(&schedule_string(&b.schedule)),
            SortKey::LastRun => sa.last_run.cmp(&sb.last_run),
            SortKey::NextRun => sa.next_run.cmp(&sb.next_run),
            SortKey::Uploaded => sa.last_uploaded_bytes.cmp(&sb.last_uploaded_bytes),
        };
        // Status already sorts worst-first, so "descending" must not undo it.
        if descending && sort != SortKey::Status {
            ordering.reverse()
        } else {
            ordering
        }
    });
    rows
}

/// Group headings, in the order they are drawn. Jobs with no project are last,
/// under `Ungrouped`.
pub fn group_jobs<'a>(
    rows: Vec<(&'a Job, JobView)>,
    group: GroupBy,
) -> Vec<(String, Vec<(&'a Job, JobView)>)> {
    match group {
        GroupBy::None => vec![(String::new(), rows)],
        GroupBy::Project => {
            let mut named: Vec<(String, Vec<(&Job, JobView)>)> = Vec::new();
            let mut ungrouped: Vec<(&Job, JobView)> = Vec::new();
            for (job, view) in rows {
                match job.project_id {
                    // No `project.list` command exists on the IPC surface, so a
                    // project's name cannot be resolved by any client. The id
                    // is shown rather than a fabricated name.
                    Some(id) => {
                        let key = format!("Project {}", format::short_uuid(&id));
                        match named.iter_mut().find(|(k, _)| k == &key) {
                            Some((_, list)) => list.push((job, view)),
                            None => named.push((key, vec![(job, view)])),
                        }
                    }
                    None => ungrouped.push((job, view)),
                }
            }
            named.sort_by(|(a, _), (b, _)| a.cmp(b));
            if !ungrouped.is_empty() {
                named.push((copy::jobs::UNGROUPED.to_string(), ungrouped));
            }
            named
        }
        GroupBy::Schedule => {
            let mut groups: Vec<(String, Vec<(&Job, JobView)>)> = Vec::new();
            for (job, view) in rows {
                let key = schedule_string(&job.schedule);
                match groups.iter_mut().find(|(k, _)| k == &key) {
                    Some((_, list)) => list.push((job, view)),
                    None => groups.push((key, vec![(job, view)])),
                }
            }
            groups.sort_by(|(a, _), (b, _)| a.cmp(b));
            groups
        }
    }
}

// ---------------------------------------------------------------------------
// Table columns (DESIGN_SYSTEM L5, UX_SPEC §4.4)
// ---------------------------------------------------------------------------

/// One column of a table: a fixed width, and how willing it is to be dropped.
///
/// `egui_extras::TableBuilder` cannot size a column to its content, so every
/// width is declared. The specification's own column sets are wider than the
/// content column they sit in — the jobs table's nine columns sum to 998px
/// inside 819px — so rather than squeezing them (which L5 forbids) the
/// interface drops whole columns in the documented priority order until the
/// set fits, at whatever width the window happens to be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnSpec {
    pub key: &'static str,
    pub width: f32,
    /// `None` for a column that is never dropped; otherwise lower goes first.
    pub drop_order: Option<u8>,
}

impl ColumnSpec {
    pub const fn keep(key: &'static str, width: f32) -> ColumnSpec {
        ColumnSpec { key, width, drop_order: None }
    }
    pub const fn droppable(key: &'static str, width: f32, order: u8) -> ColumnSpec {
        ColumnSpec { key, width, drop_order: Some(order) }
    }
}

/// The columns that fit, in their original order.
///
/// `spacing` is the gap the table puts between columns, which is part of the
/// width whether or not anyone remembers it.
pub fn fit_columns(
    available: f32,
    remainder_min: f32,
    spacing: f32,
    specs: &[ColumnSpec],
) -> Vec<&'static str> {
    let mut kept: Vec<&ColumnSpec> = specs.iter().collect();
    loop {
        let gaps = spacing * kept.len() as f32;
        let total: f32 = kept.iter().map(|c| c.width).sum::<f32>() + remainder_min + gaps;
        if total <= available {
            break;
        }
        // Drop the most expendable column that is still present.
        let victim = kept
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.drop_order.map(|o| (o, i)))
            .max_by_key(|(order, _)| std::cmp::Reverse(*order))
            .map(|(_, i)| i);
        match victim {
            Some(index) => {
                kept.remove(index);
            }
            // Nothing left to drop: the remaining columns are all essential,
            // and the table scrolls horizontally rather than lying about them.
            None => break,
        }
    }
    kept.into_iter().map(|c| c.key).collect()
}

// ---------------------------------------------------------------------------
// Destination status (UX_SPEC §8.1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationStatus {
    Ready,
    NotConnected,
    Unreachable,
    Disabled,
}

impl DestinationStatus {
    pub fn title(self) -> &'static str {
        match self {
            DestinationStatus::Ready => copy::dest::STATUS_READY,
            DestinationStatus::NotConnected => copy::dest::STATUS_NOT_CONNECTED,
            DestinationStatus::Unreachable => copy::dest::STATUS_UNREACHABLE,
            DestinationStatus::Disabled => copy::badge::DISABLED,
        }
    }
}

pub fn destination_status(
    destination: &Destination,
    last_probe_failed: bool,
) -> DestinationStatus {
    if !destination.enabled {
        return DestinationStatus::Disabled;
    }
    if last_probe_failed {
        return DestinationStatus::Unreachable;
    }
    match destination.last_verified_at {
        Some(_) => DestinationStatus::Ready,
        None => DestinationStatus::NotConnected,
    }
}

/// The verification badge on a destination row: how long ago, and whether that
/// is recent enough to be reassuring.
#[derive(Debug, Clone, PartialEq)]
pub enum Verification {
    Recent(String),
    Old(String),
    Never,
    Unreachable,
}

pub fn verification(
    destination: &Destination,
    now: DateTime<Utc>,
    failed: bool,
) -> Verification {
    if failed {
        return Verification::Unreachable;
    }
    match destination.last_verified_at {
        None => Verification::Never,
        Some(at) => {
            let label = copy::job_dest_verified(&format::relative_past(at, now));
            if now - at <= Duration::days(7) {
                Verification::Recent(label)
            } else {
                Verification::Old(label)
            }
        }
    }
}

/// The order the job editor lists destinations in: ticked first, then by kind,
/// then by name. Re-ordering happens only on save, so ticking a box does not
/// make the list jump under the cursor.
pub fn order_destinations<'a>(
    destinations: &'a [Destination],
    ticked: &[Uuid],
) -> Vec<&'a Destination> {
    let mut out: Vec<&Destination> = destinations.iter().collect();
    out.sort_by_key(|d| {
        let kind_rank = match d.kind {
            superbackup_core::model::DestinationKind::LocalRepository { .. } => 0,
            superbackup_core::model::DestinationKind::OneDrive { .. } => 1,
            superbackup_core::model::DestinationKind::S3 { .. } => 2,
            superbackup_core::model::DestinationKind::LocalMirror { .. } => 3,
        };
        (
            if ticked.contains(&d.id) { 0 } else { 1 },
            kind_rank,
            d.name.to_lowercase(),
        )
    });
    out
}

// ---------------------------------------------------------------------------
// Activity (UX_SPEC §10)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRange {
    Day,
    Week,
    Month,
    All,
}

impl TimeRange {
    pub const ALL: [TimeRange; 4] =
        [TimeRange::Day, TimeRange::Week, TimeRange::Month, TimeRange::All];
    pub fn title(self) -> &'static str {
        match self {
            TimeRange::Day => copy::activity::RANGE_24H,
            TimeRange::Week => copy::activity::RANGE_7D,
            TimeRange::Month => copy::activity::RANGE_30D,
            TimeRange::All => copy::activity::RANGE_ALL,
        }
    }
    pub fn cutoff(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            TimeRange::Day => Some(now - Duration::hours(24)),
            TimeRange::Week => Some(now - Duration::days(7)),
            TimeRange::Month => Some(now - Duration::days(30)),
            TimeRange::All => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunFilters {
    pub search: String,
    pub range: Option<TimeRange>,
    pub job: Option<Uuid>,
    pub status: Option<RunStatus>,
    pub destination: Option<Uuid>,
}

impl RunFilters {
    pub fn any(&self) -> bool {
        !self.search.trim().is_empty()
            || self.job.is_some()
            || self.status.is_some()
            || self.destination.is_some()
    }
}

pub fn visible_runs<'a>(
    runs: &'a [JobRun],
    filters: &RunFilters,
    range: TimeRange,
    now: DateTime<Utc>,
) -> Vec<&'a JobRun> {
    let cutoff = range.cutoff(now);
    let needle = filters.search.trim().to_lowercase();
    runs.iter()
        .filter(|r| cutoff.map(|c| r.started_at >= c).unwrap_or(true))
        .filter(|r| filters.job.map(|j| r.job_id == j).unwrap_or(true))
        .filter(|r| filters.status.map(|s| r.status == s).unwrap_or(true))
        .filter(|r| {
            filters
                .destination
                .map(|d| r.destinations.iter().any(|dr| dr.destination_id == d))
                .unwrap_or(true)
        })
        .filter(|r| {
            needle.is_empty()
                || r.job_name.to_lowercase().contains(&needle)
                || r.destinations.iter().any(|d| {
                    d.destination_name.to_lowercase().contains(&needle)
                        || d.error
                            .as_ref()
                            .map(|e| e.message.to_lowercase().contains(&needle))
                            .unwrap_or(false)
                })
        })
        .collect()
}

pub fn visible_events<'a>(
    events: &'a [Event],
    search: &str,
    min_severity: Severity,
    range: TimeRange,
    now: DateTime<Utc>,
) -> Vec<&'a Event> {
    let cutoff = range.cutoff(now);
    let needle = search.trim().to_lowercase();
    events
        .iter()
        .filter(|e| cutoff.map(|c| e.at >= c).unwrap_or(true))
        .filter(|e| e.severity >= min_severity)
        .filter(|e| {
            needle.is_empty()
                || e.message.to_lowercase().contains(&needle)
                || e.kind.to_lowercase().contains(&needle)
        })
        .collect()
}

/// `2 succeeded, 1 failed` — the destination summary an activity row owes the
/// user. A run that partly failed never renders as a plain success.
pub fn destination_summary(run: &JobRun) -> String {
    let total = run.destinations.len();
    let ok = run
        .destinations
        .iter()
        .filter(|d| {
            matches!(d.status, RunStatus::Succeeded | RunStatus::SucceededWithWarnings)
        })
        .count();
    copy::activity_dest_summary(ok, total)
}

/// The `JobSummary` a job would have if the given run were its latest. Used by
/// the run detail's "retry" affordance and by tests of the fan-out rule.
pub fn run_outcome(run: &JobRun) -> RunStatus {
    run.derive_status()
}

/// A one-line, redacted summary of a run for pasting into a bug report.
pub fn run_summary_text(run: &JobRun, now: DateTime<Utc>) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} — {}\n", run.job_name, run.status.title()));
    out.push_str(&format!(
        "started {} ({})\n",
        format::absolute(run.started_at),
        format::relative_past(run.started_at, now)
    ));
    if let Some(secs) = run.duration_seconds() {
        out.push_str(&format!("duration {}\n", format::duration(secs)));
    }
    out.push_str(&format!("trigger {}\n", copy::trigger(run.trigger)));
    for d in &run.destinations {
        out.push_str(&format!(
            "  {} — {} · {} uploaded",
            d.destination_name,
            d.status.title(),
            format::bytes(d.progress.bytes_uploaded)
        ));
        if let Some(e) = &d.error {
            out.push_str(&format!(" · {:?}: {}", e.code, e.message));
        }
        out.push('\n');
    }
    out
}

/// The summaries a job list row needs, defaulted so a job the daemon has never
/// run still renders.
pub fn summary_or_default(data: &Data, job: &Uuid) -> JobSummary {
    data.summary_for(job).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use superbackup_core::model::TimeOfDay;
    use superbackup_core::state::{DestinationRun, Health, Progress, StatusSnapshot, Trigger};

    fn t(h: u8, m: u8) -> TimeOfDay {
        TimeOfDay { hour: h, minute: m }
    }

    #[test]
    fn schedule_strings_match_the_specification_table() {
        assert_eq!(schedule_string(&Schedule::Manual), "Manual only");
        assert_eq!(
            schedule_string(&Schedule::Interval { minutes: 30 }),
            "Every 30 minutes"
        );
        assert_eq!(schedule_string(&Schedule::Interval { minutes: 60 }), "Every hour");
        assert_eq!(
            schedule_string(&Schedule::Interval { minutes: 240 }),
            "Every 4 hours"
        );
        assert_eq!(
            schedule_string(&Schedule::Daily { times: vec![t(2, 0)] }),
            "Daily at 02:00"
        );
        assert_eq!(
            schedule_string(&Schedule::Daily { times: vec![t(9, 0), t(18, 0)] }),
            "Daily at 09:00 and 18:00"
        );
        assert_eq!(
            schedule_string(&Schedule::Daily {
                times: vec![t(1, 0), t(7, 0), t(13, 0), t(19, 0)]
            }),
            "Daily, 4 times a day"
        );
        assert_eq!(
            schedule_string(&Schedule::Weekly {
                weekdays: vec![0, 2, 4],
                times: vec![t(2, 0)]
            }),
            "Mon, Wed, Fri at 02:00"
        );
        assert_eq!(
            schedule_string(&Schedule::Cron { expression: "0 2 * * *".into() }),
            "Cron: 0 2 * * *"
        );
        assert_eq!(
            schedule_string(&Schedule::OnChange {
                debounce_seconds: 120,
                min_interval_minutes: 30
            }),
            "When files change (2 min quiet, at most every 30 min)"
        );
    }

    #[test]
    fn a_weekly_schedule_on_every_day_reads_as_daily() {
        assert_eq!(
            schedule_string(&Schedule::Weekly {
                weekdays: vec![0, 1, 2, 3, 4, 5, 6],
                times: vec![t(2, 0)]
            }),
            "Daily at 02:00"
        );
    }

    #[test]
    fn next_runs_are_in_the_future_and_in_order() {
        let now = Utc::now();
        let runs = next_runs(&Schedule::Daily { times: vec![t(2, 0), t(14, 0)] }, now, 5);
        assert_eq!(runs.len(), 5);
        assert!(runs.iter().all(|r| *r > now));
        assert!(runs.windows(2).all(|w| w[0] < w[1]));

        assert!(next_runs(&Schedule::Manual, now, 5).is_empty());
        assert_eq!(next_runs(&Schedule::Interval { minutes: 30 }, now, 3).len(), 3);
    }

    fn job(name: &str) -> Job {
        Job {
            id: Uuid::new_v4(),
            name: name.into(),
            project_id: None,
            description: String::new(),
            sources: vec![],
            destination_ids: vec![Uuid::new_v4()],
            schedule: Schedule::Daily { times: vec![t(2, 0)] },
            exclusions: Default::default(),
            bandwidth: None,
            retention: None,
            enabled: true,
            timeout_minutes: None,
            hooks: Default::default(),
            continue_on_destination_error: true,
            created_at: Utc::now(),
            tags: vec![],
        }
    }

    fn data_with(jobs: Vec<Job>, summaries: BTreeMap<Uuid, JobSummary>) -> Data {
        let mut d = Data::new();
        d.loading = false;
        d.link_up = true;
        d.snapshot = Some(StatusSnapshot {
            health: Health::Idle,
            version: "0.1.0".into(),
            machine_label: "M".into(),
            machine_slug: "m".into(),
            unlocked: true,
            paused: false,
            paused_until: None,
            service_installed: true,
            service_running: true,
            kopia_version: Some("0.17.0".into()),
            active_runs: vec![],
            jobs: summaries,
            next_scheduled: None,
            recent_events: vec![],
            uptime_seconds: 1,
            generated_at: Utc::now(),
        });
        d.jobs = jobs;
        d
    }

    #[test]
    fn a_never_run_job_says_so_rather_than_showing_a_status() {
        let j = job("Fresh");
        let d = data_with(vec![j.clone()], BTreeMap::new());
        let view = job_view(&d, &d.jobs[0], Utc::now());
        assert_eq!(view.state, CardState::NeverRun);
        assert_eq!(view.badge, "Never run");
    }

    #[test]
    fn a_disabled_job_keeps_its_last_result_in_the_meta_line() {
        let mut j = job("Scratch VM");
        j.enabled = false;
        let mut summaries = BTreeMap::new();
        summaries.insert(
            j.id,
            JobSummary {
                last_status: Some(RunStatus::Succeeded),
                last_run: Some(Utc::now() - Duration::hours(3)),
                ..Default::default()
            },
        );
        let d = data_with(vec![j], summaries);
        let view = job_view(&d, &d.jobs[0], Utc::now());
        assert!(matches!(view.state, CardState::Disabled { .. }));
        assert_eq!(view.badge, "Disabled");
        assert!(view.meta.contains("Succeeded"));
    }

    #[test]
    fn a_stale_job_takes_the_warning_badge_even_though_it_succeeded() {
        let j = job("Photos");
        let mut summaries = BTreeMap::new();
        summaries.insert(
            j.id,
            JobSummary {
                last_status: Some(RunStatus::Succeeded),
                last_run: Some(Utc::now() - Duration::days(5)),
                last_success: Some(Utc::now() - Duration::days(5)),
                total_runs: 4,
                ..Default::default()
            },
        );
        let mut d = data_with(vec![j], summaries);
        d.settings.notifications.stale_after_days = 3;
        let view = job_view(&d, &d.jobs[0], Utc::now());
        assert_eq!(view.state, CardState::Stale);
        assert_eq!(view.badge, "Warnings");
    }

    #[test]
    fn consecutive_failures_appear_as_an_ordinal() {
        let j = job("Dev code");
        let mut summaries = BTreeMap::new();
        summaries.insert(
            j.id,
            JobSummary {
                last_status: Some(RunStatus::Failed),
                last_run: Some(Utc::now() - Duration::minutes(20)),
                consecutive_failures: 3,
                ..Default::default()
            },
        );
        let d = data_with(vec![j], summaries);
        let view = job_view(&d, &d.jobs[0], Utc::now());
        assert!(view.meta.contains("3rd failure in a row"), "{}", view.meta);
    }

    #[test]
    fn the_default_sort_puts_problems_first() {
        let failing = job("Zebra");
        let fine = job("Alpha");
        let mut summaries = BTreeMap::new();
        summaries.insert(
            failing.id,
            JobSummary { last_status: Some(RunStatus::Failed), ..Default::default() },
        );
        summaries.insert(
            fine.id,
            JobSummary {
                last_status: Some(RunStatus::Succeeded),
                last_run: Some(Utc::now()),
                last_success: Some(Utc::now()),
                ..Default::default()
            },
        );
        let d = data_with(vec![fine, failing], summaries);
        let rows = visible_jobs(&d, "", JobFilter::All, SortKey::Status, false, Utc::now());
        assert_eq!(rows[0].0.name, "Zebra", "the failing job must sort first");
    }

    #[test]
    fn search_matches_names_tags_and_source_paths() {
        let mut j = job("Dev code");
        j.tags = vec!["work".into()];
        j.sources = vec![superbackup_core::model::Source::new("/home/andreas/dev")];
        assert!(matches_search(&j, "dev"));
        assert!(matches_search(&j, "work"));
        assert!(matches_search(&j, "andreas"));
        assert!(!matches_search(&j, "photos"));
        assert!(matches_search(&j, "   "));
    }

    #[test]
    fn filters_narrow_the_list_without_reordering_it() {
        let mut disabled = job("Off");
        disabled.enabled = false;
        let on = job("On");
        let d = data_with(vec![on, disabled], BTreeMap::new());
        let rows = visible_jobs(&d, "", JobFilter::Disabled, SortKey::Name, false, Utc::now());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.name, "Off");
    }

    #[test]
    fn grouping_puts_projectless_jobs_last() {
        let mut grouped = job("Grouped");
        grouped.project_id = Some(Uuid::new_v4());
        let loose = job("Loose");
        let d = data_with(vec![grouped, loose], BTreeMap::new());
        let rows = visible_jobs(&d, "", JobFilter::All, SortKey::Name, false, Utc::now());
        let groups = group_jobs(rows, GroupBy::Project);
        assert_eq!(groups.last().expect("a group").0, "Ungrouped");
    }

    fn run(statuses: Vec<RunStatus>) -> JobRun {
        let mut run = JobRun {
            run_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            job_name: "Dev code".into(),
            trigger: Trigger::Schedule,
            status: RunStatus::Succeeded,
            started_at: Utc::now() - Duration::minutes(10),
            finished_at: Some(Utc::now()),
            destinations: statuses
                .into_iter()
                .enumerate()
                .map(|(i, status)| DestinationRun {
                    destination_id: Uuid::new_v4(),
                    destination_name: format!("d{i}"),
                    status,
                    started_at: None,
                    finished_at: None,
                    progress: Progress::default(),
                    snapshot_id: None,
                    error: None,
                    warnings: vec![],
                })
                .collect(),
        };
        // Never hand-written: the roll-up is the core's rule, not the test's.
        run.status = run.derive_status();
        run
    }

    #[test]
    fn columns_drop_in_the_documented_priority_order() {
        let specs = [
            ColumnSpec::keep("status", 32.0),
            ColumnSpec::keep("name", 190.0),
            ColumnSpec::droppable("sources", 52.0, 4),
            ColumnSpec::keep("destinations", 124.0),
            ColumnSpec::keep("schedule", 118.0),
            ColumnSpec::keep("last", 104.0),
            ColumnSpec::droppable("next", 104.0, 3),
            ColumnSpec::droppable("uploaded", 84.0, 1),
        ];
        // Everything fits in a very wide window.
        assert_eq!(fit_columns(2000.0, 84.0, 8.0, &specs).len(), specs.len());

        // `Uploaded` is always the first to go, then `Next run`, then
        // `Folders` — the specification's own order.
        let mut dropped: Vec<&str> = Vec::new();
        let mut width = 1000.0;
        while width > 300.0 {
            let kept = fit_columns(width, 84.0, 8.0, &specs);
            for spec in &specs {
                if !kept.contains(&spec.key) && !dropped.contains(&spec.key) {
                    dropped.push(spec.key);
                }
            }
            width -= 20.0;
        }
        assert_eq!(dropped, vec!["uploaded", "next", "sources"]);
        assert!(
            fit_columns(400.0, 84.0, 8.0, &specs).contains(&"name"),
            "the name column is never dropped"
        );

        // The gaps between columns count: the same set does not fit twice.
        assert!(
            fit_columns(830.0, 84.0, 0.0, &specs).len()
                >= fit_columns(830.0, 84.0, 8.0, &specs).len()
        );

        // A column with no drop order survives even when nothing fits.
        let cramped = fit_columns(100.0, 84.0, 8.0, &specs);
        assert!(cramped.contains(&"status"));
        assert!(cramped.contains(&"name"));
        assert!(!cramped.contains(&"sources"));
    }

    #[test]
    fn a_partial_run_is_never_summarised_as_a_success() {
        let r = run(vec![RunStatus::Succeeded, RunStatus::Succeeded, RunStatus::Failed]);
        assert_eq!(destination_summary(&r), "2 of 3 succeeded");
        assert_eq!(run_outcome(&r), RunStatus::Failed);
    }

    #[test]
    fn activity_filters_compose() {
        let now = Utc::now();
        let mut old = run(vec![RunStatus::Succeeded]);
        old.started_at = now - Duration::days(20);
        let recent = run(vec![RunStatus::Failed]);
        let runs = vec![recent.clone(), old];
        let filters = RunFilters::default();
        assert_eq!(visible_runs(&runs, &filters, TimeRange::Week, now).len(), 1);
        assert_eq!(visible_runs(&runs, &filters, TimeRange::All, now).len(), 2);

        let filters = RunFilters { status: Some(RunStatus::Failed), ..Default::default() };
        let out = visible_runs(&runs, &filters, TimeRange::All, now);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].run_id, recent.run_id);
    }

    #[test]
    fn events_respect_the_severity_floor() {
        let now = Utc::now();
        let events = vec![
            Event::new(Severity::Debug, "a", "debug line"),
            Event::info("b", "info line"),
            Event::error("c", "error line"),
        ];
        assert_eq!(
            visible_events(&events, "", Severity::Info, TimeRange::All, now).len(),
            2
        );
        assert_eq!(
            visible_events(&events, "", Severity::Error, TimeRange::All, now).len(),
            1
        );
        assert_eq!(
            visible_events(&events, "", Severity::Debug, TimeRange::All, now).len(),
            3
        );
    }
}
