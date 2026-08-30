//! The commands people run every day: status, run, stop, pause, watch.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use superbackup_core::error::ErrorCode;
use superbackup_core::ipc::protocol::{Request, StreamItem};
use superbackup_core::ipc::Topic;
use superbackup_core::model::Job;
use superbackup_core::state::{Health, JobRun, JobSummary, RunStatus, Severity, StatusSnapshot};

use crate::cli::args::{PauseArgs, RunArgs, StatusArgs, StopArgs, WatchArgs};
use crate::cli::client::{reply, Daemon, Start};
use crate::cli::context::Ctx;
use crate::cli::format::{self, Cell, Colour, Column, Table};
use crate::cli::output::{CliError, CliResult, Outcome};
use crate::cli::resolve::{self, Kind};
use crate::cli::timespec;

use super::{jobs, resolve_job};

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

pub fn status(ctx: &mut Ctx, args: StatusArgs) -> CliResult<Outcome> {
    // A query never starts a daemon. Asking a question is not permission to
    // launch a background process.
    let daemon = Daemon::connect(ctx, Start::Never)?;

    if let Some(every) = args.watch {
        return watch_status(ctx, &daemon, &args, every.max(1));
    }

    let (snapshot, all_jobs) = fetch_status(&daemon)?;
    let selected = match &args.job {
        Some(needle) => Some(resolve::one(needle, &all_jobs, Kind::Job)?.clone()),
        None => None,
    };

    render_status(ctx, &snapshot, &all_jobs, selected.as_ref(), &args);

    match selected {
        Some(job) => {
            let summary = snapshot.jobs.get(&job.id).cloned().unwrap_or_default();
            Outcome::data(serde_json::json!({
                "snapshot": snapshot,
                "job": job,
                "summary": summary,
            }))
        }
        // `data` groups two shapes, each serialised straight from `core`: the
        // snapshot answers "what is happening", and the job list is what turns
        // the snapshot's ids into names. Fetching them separately would make
        // every caller do two round trips to render one screen.
        None => Outcome::data(serde_json::json!({ "snapshot": snapshot, "jobs": all_jobs })),
    }
}

fn fetch_status(daemon: &Daemon) -> CliResult<(StatusSnapshot, Vec<Job>)> {
    let snapshot = *reply!(daemon, Request::Status {}, Status)?.snapshot;
    let all_jobs = jobs(daemon)?;
    Ok((snapshot, all_jobs))
}

/// `--watch N`: redraw every N seconds until interrupted.
fn watch_status(
    ctx: &mut Ctx,
    daemon: &Daemon,
    args: &StatusArgs,
    every: u64,
) -> CliResult<Outcome> {
    if ctx.global.json {
        return Err(CliError::usage(
            "--watch repeats a screen for a person and would emit many JSON documents",
        )
        .with_hint("Use `superbackup watch` for a machine-readable stream, or drop --watch."));
    }

    let interval = Duration::from_secs(every);
    loop {
        let (snapshot, all_jobs) = fetch_status(daemon)?;
        let selected = match &args.job {
            Some(needle) => Some(resolve::one(needle, &all_jobs, Kind::Job)?.clone()),
            None => None,
        };
        if ctx.ui.out_is_tty {
            // Home the cursor and clear, rather than printing screens forever.
            ctx.ui.stream_line("\u{1b}[H\u{1b}[2J");
        }
        render_status(ctx, &snapshot, &all_jobs, selected.as_ref(), args);
        ctx.ui.line(format!(
            "\nRefreshing every {}. Press Ctrl-C to stop.",
            format::duration_secs(every as i64)
        ));

        let interrupted = daemon.block_on(async {
            tokio::select! {
                _ = tokio::time::sleep(interval) => false,
                _ = tokio::signal::ctrl_c() => true,
            }
        });
        if interrupted {
            ctx.ui.line("");
            return Ok(Outcome::ok());
        }
    }
}

fn render_status(
    ctx: &mut Ctx,
    snapshot: &StatusSnapshot,
    all_jobs: &[Job],
    selected: Option<&Job>,
    args: &StatusArgs,
) {
    if ctx.ui.json || ctx.ui.quiet {
        return;
    }
    let now = Utc::now();
    let names: BTreeMap<Uuid, &str> = all_jobs.iter().map(|j| (j.id, j.name.as_str())).collect();

    headline(ctx, snapshot, now);
    ctx.ui.blank();

    let runs: Vec<&JobRun> = match selected {
        Some(job) => snapshot.active_runs.iter().filter(|r| r.job_id == job.id).collect(),
        None => snapshot.active_runs.iter().collect(),
    };
    if !runs.is_empty() {
        ctx.ui.heading("Running now");
        ctx.ui.table(&running_table(&runs));
        ctx.ui.blank();
    }

    match selected {
        Some(job) => job_detail(ctx, job, snapshot, now),
        None => {
            let table = jobs_table(all_jobs, snapshot, now);
            ctx.ui.heading("Jobs");
            ctx.ui.table(&table);
            ctx.ui.blank();
            next_line(ctx, snapshot, &names, now);
        }
    }

    if args.events {
        ctx.ui.blank();
        ctx.ui.heading("Recent activity");
        ctx.ui.table(&events_table(snapshot, args.events_limit));
    }
}

fn headline(ctx: &mut Ctx, snapshot: &StatusSnapshot, now: DateTime<Utc>) {
    let colour = health_colour(snapshot.health);
    let mut line = snapshot.health.title().to_string();

    // The headline has to answer "should I do something?" on its own, so
    // anything that stops backups happening is appended to it rather than
    // hidden three sections down.
    let mut because: Vec<String> = Vec::new();
    if snapshot.paused {
        match snapshot.paused_until {
            Some(until) => because.push(format!("resuming {}", format::relative(until, now))),
            None => because.push("until you run `superbackup resume`".to_string()),
        }
    }
    if !snapshot.unlocked {
        because.push("the vault is locked, so scheduled backups cannot run".to_string());
    }
    let failed = snapshot.jobs.values().filter(|s| s.last_status == Some(RunStatus::Failed)).count();
    if failed > 0 {
        because.push(format!("{} failed", format::plural(failed, "job", "jobs")));
    }
    if !because.is_empty() {
        line.push_str(" - ");
        line.push_str(&because.join("; "));
    }

    ctx.ui.coloured(colour, &line);
    let uptime = if snapshot.uptime_seconds > 0 {
        format!(", up {}", format::duration_secs(snapshot.uptime_seconds as i64))
    } else {
        String::new()
    };
    let kopia = snapshot.kopia_version.as_deref().unwrap_or("kopia not found");
    ctx.ui.line(format!(
        "{} - superbackup {} with kopia {kopia}{uptime}",
        snapshot.machine_label, snapshot.version
    ));
}

fn running_table(runs: &[&JobRun]) -> Table {
    let mut table = Table::new(vec![
        Column::new("job").flex(),
        Column::new("destination").flex(),
        Column::new("progress"),
        Column::new("done").right(),
        Column::new("speed").right(),
        Column::new("left").right(),
    ]);
    for run in runs {
        for dest in &run.destinations {
            let fraction = dest.progress.fraction();
            let bar = match fraction {
                Some(f) => format!("{} {}", format::progress_bar(Some(f), 12), format::percent(f)),
                None => "estimating".to_string(),
            };
            table.push(vec![
                Cell::new(run.job_name.clone()),
                Cell::new(dest.destination_name.clone()),
                Cell::coloured(bar, run_colour(dest.status)),
                Cell::new(format::bytes(dest.progress.bytes_processed)),
                Cell::new(format::rate(dest.progress.bytes_per_second)),
                Cell::new(
                    dest.progress
                        .estimated_seconds_remaining
                        .map(|s| format::duration_secs(s as i64))
                        .unwrap_or_else(|| format::MISSING.to_string()),
                ),
            ]);
        }
        if run.destinations.is_empty() {
            table.push(vec![
                Cell::new(run.job_name.clone()),
                Cell::new(format::MISSING),
                Cell::coloured(run.status.title(), run_colour(run.status)),
                Cell::new(format::MISSING),
                Cell::new(format::MISSING),
                Cell::new(format::MISSING),
            ]);
        }
    }
    table
}

fn jobs_table(all_jobs: &[Job], snapshot: &StatusSnapshot, now: DateTime<Utc>) -> Table {
    let mut table = Table::new(vec![
        Column::new("job").flex(),
        Column::new("last run"),
        Column::new("result"),
        Column::new("uploaded").right(),
        Column::new("next run"),
    ])
    .empty_note("No jobs yet. Add one with `superbackup job add --name NAME --source PATH`.");

    let running: Vec<Uuid> = snapshot.active_runs.iter().map(|r| r.job_id).collect();
    for job in all_jobs {
        let summary = snapshot.jobs.get(&job.id).cloned().unwrap_or_default();
        table.push(vec![
            Cell::new(job.name.clone()),
            Cell::new(format::opt_relative(summary.last_run, now)),
            result_cell(job, &summary, running.contains(&job.id)),
            Cell::new(if summary.last_uploaded_bytes > 0 {
                format::bytes(summary.last_uploaded_bytes)
            } else {
                format::MISSING.to_string()
            }),
            Cell::new(next_run_text(job, &summary, now)),
        ]);
    }
    table
}

fn result_cell(job: &Job, summary: &JobSummary, running: bool) -> Cell {
    if running {
        return Cell::coloured("Running", Colour::Cyan);
    }
    if !job.enabled {
        return Cell::coloured("Disabled", Colour::Dim);
    }
    match summary.last_status {
        Some(status) => {
            let mut text = status.title().to_string();
            if summary.consecutive_failures > 1 {
                text = format!("{text} x{}", summary.consecutive_failures);
            }
            Cell::coloured(text, run_colour(status))
        }
        None => Cell::coloured("Never run", Colour::Dim),
    }
}

fn next_run_text(job: &Job, summary: &JobSummary, now: DateTime<Utc>) -> String {
    if !job.enabled {
        return format::MISSING.to_string();
    }
    match summary.next_run {
        Some(next) => format::relative(next, now),
        None if !job.schedule.is_automatic() => "manual".to_string(),
        None => format::MISSING.to_string(),
    }
}

fn job_detail(ctx: &mut Ctx, job: &Job, snapshot: &StatusSnapshot, now: DateTime<Utc>) {
    let summary = snapshot.jobs.get(&job.id).cloned().unwrap_or_default();
    ctx.ui.heading(&job.name);
    let pad = 16;
    ctx.ui.field("Enabled", if job.enabled { "yes" } else { "no" }, pad);
    ctx.ui.field("Sources", format::plural(job.sources.len(), "folder", "folders"), pad);
    ctx.ui.field(
        "Destinations",
        format::plural(job.destination_ids.len(), "destination", "destinations"),
        pad,
    );
    ctx.ui.field("Last run", format::opt_relative(summary.last_run, now), pad);
    ctx.ui.field(
        "Last result",
        summary.last_status.map(|s| s.title().to_string()).unwrap_or_else(|| "never run".into()),
        pad,
    );
    ctx.ui.field("Last success", format::opt_relative(summary.last_success, now), pad);
    ctx.ui.field("Next run", next_run_text(job, &summary, now), pad);
    ctx.ui.field("Total runs", summary.total_runs.to_string(), pad);
    ctx.ui.field(
        "Average duration",
        format::opt_duration_secs(summary.average_duration_seconds),
        pad,
    );
    if let Some(error) = &summary.last_error {
        ctx.ui.blank();
        ctx.ui.coloured(Colour::Red, &format!("Last failure: {}", error.message));
        if let Some(hint) = &error.hint {
            ctx.ui.line(format!("  {hint}"));
        }
    }
}

fn next_line(
    ctx: &mut Ctx,
    snapshot: &StatusSnapshot,
    names: &BTreeMap<Uuid, &str>,
    now: DateTime<Utc>,
) {
    match &snapshot.next_scheduled {
        Some((job_id, at)) => {
            let name = names.get(job_id).copied().unwrap_or("a job");
            ctx.ui.line(format!("Next: {name} {}", format::relative(*at, now)));
        }
        None if snapshot.paused => {
            ctx.ui.line("Next: nothing, because backups are paused.");
        }
        None => ctx.ui.line("Next: nothing scheduled."),
    }
}

fn events_table(snapshot: &StatusSnapshot, limit: usize) -> Table {
    let mut table = Table::new(vec![
        Column::new("when"),
        Column::new("severity"),
        Column::new("kind"),
        Column::new("message").flex(),
    ])
    .empty_note("No recent activity.");

    // The most recent `limit` lines, oldest first — a log tail, where the
    // newest line is the one nearest the prompt. Sorting rather than trusting
    // the snapshot's order means this reads correctly whichever way round the
    // daemon happens to send them.
    let mut recent: Vec<&superbackup_core::state::Event> = snapshot.recent_events.iter().collect();
    recent.sort_by_key(|e| e.at);
    let start = recent.len().saturating_sub(limit);
    for event in &recent[start..] {
        table.push(vec![
            Cell::new(format::timestamp_local(event.at)),
            Cell::coloured(severity_text(event.severity), severity_colour(event.severity)),
            Cell::new(event.kind.clone()),
            Cell::new(event.message.clone()),
        ]);
    }
    table
}

pub fn health_colour(health: Health) -> Colour {
    match health {
        Health::Idle => Colour::Green,
        Health::Running => Colour::Cyan,
        Health::Attention => Colour::Yellow,
        Health::Paused => Colour::Blue,
        Health::Failed => Colour::Red,
    }
}

pub fn run_colour(status: RunStatus) -> Colour {
    match status {
        RunStatus::Succeeded => Colour::Green,
        RunStatus::SucceededWithWarnings => Colour::Yellow,
        RunStatus::Failed => Colour::Red,
        RunStatus::Cancelled | RunStatus::Skipped => Colour::Dim,
        _ => Colour::Cyan,
    }
}

fn severity_text(severity: Severity) -> &'static str {
    match severity {
        Severity::Debug => "debug",
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

fn severity_colour(severity: Severity) -> Colour {
    match severity {
        Severity::Debug => Colour::Dim,
        Severity::Info => Colour::Blue,
        Severity::Warning => Colour::Yellow,
        Severity::Error => Colour::Red,
    }
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

pub fn run(ctx: &mut Ctx, args: RunArgs) -> CliResult<Outcome> {
    // `job.run` takes a job and a dry-run flag and nothing else. Rather than
    // accept a filter and quietly write everywhere anyway, say so: a backup
    // that went somewhere the user excluded is not a smaller problem than a
    // backup that did not happen.
    if !args.destinations.is_empty() {
        return Err(CliError::unsupported(
            "--destination",
            "the running instance takes no destination filter on `job.run`",
        )
        .with_hint(
            "Detach the destination from the job with `superbackup job edit`, or run the job \
             in full.",
        ));
    }
    if args.force {
        return Err(CliError::unsupported(
            "--force",
            "the running instance has no way to override pause, battery or metered policy for \
             a single run",
        )
        .with_hint("Run `superbackup resume` first, then run the job."));
    }

    let daemon = Daemon::connect(ctx, Start::IfNeeded)?;
    let all_jobs = jobs(&daemon)?;

    let targets: Vec<Job> = match (&args.job, args.all) {
        (Some(needle), _) => vec![resolve::one(needle, &all_jobs, Kind::Job)?.clone()],
        (None, true) => {
            let enabled: Vec<Job> = all_jobs.iter().filter(|j| j.enabled).cloned().collect();
            if enabled.is_empty() {
                return Err(CliError::usage("there are no enabled jobs to run")
                    .with_hint("Enable one with `superbackup job enable NAME`."));
            }
            enabled
        }
        // clap enforces this; the arm exists so a future edit cannot make it
        // reachable silently.
        (None, false) => {
            return Err(CliError::usage("name a job to run, or pass --all"));
        }
    };

    let mut started = Vec::new();
    for job in &targets {
        let reply = reply!(
            daemon,
            Request::JobRun { job: job.id.to_string(), dry_run: args.dry_run },
            Started
        )?;
        started.push((job.clone(), reply));
    }

    if !args.wait {
        for (job, reply) in &started {
            let verb = if reply.started { "started" } else { "queued" };
            ctx.ui.line(format!("{}: {verb} ({})", job.name, reply.run_id));
            if let Some(note) = &reply.note {
                ctx.ui.line(format!("  {note}"));
            }
        }
        // A user who expected this to block and got silence assumes nothing
        // happened. Say what it did and how to follow it.
        ctx.ui.line(format!(
            "\nNot waiting for {} to finish. Follow with `superbackup status`, or add --wait.",
            if started.len() == 1 { "it" } else { "them" }
        ));
        return Outcome::data(
            started.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(),
        );
    }

    let mut results = Vec::new();
    let mut worst_ok = true;
    for (job, reply) in &started {
        let outcome = follow_run(ctx, &daemon, reply.run_id, &job.name)?;
        if !outcome.succeeded() {
            worst_ok = false;
        }
        results.push(outcome);
    }

    let value: Vec<serde_json::Value> = results.iter().map(|r| r.as_json()).collect();
    if worst_ok {
        Outcome::data(value)
    } else {
        // The command ran and the answer was negative: `ok` stays true because
        // the CLI did what was asked, and the exit code says the backup did
        // not succeed.
        Outcome::negative(value)
    }
}

// ---------------------------------------------------------------------------
// Following a run to its end
// ---------------------------------------------------------------------------

/// How a followed run ended.
pub enum Followed {
    /// The daemon recorded a terminal status for it.
    Finished(Box<JobRun>),
    /// It left the active list but no history entry could be found. Reported
    /// rather than assumed successful: "probably fine" is not an answer a
    /// backup tool may give.
    Unknown { run_id: Uuid },
}

impl Followed {
    pub fn succeeded(&self) -> bool {
        match self {
            Followed::Finished(run) => matches!(
                run.status,
                RunStatus::Succeeded | RunStatus::SucceededWithWarnings | RunStatus::Skipped
            ),
            Followed::Unknown { .. } => false,
        }
    }

    pub fn as_json(&self) -> serde_json::Value {
        match self {
            Followed::Finished(run) => {
                serde_json::to_value(run).unwrap_or(serde_json::Value::Null)
            }
            Followed::Unknown { run_id } => {
                serde_json::json!({ "run_id": run_id, "status": "unknown" })
            }
        }
    }
}

/// Stream progress until the run reaches a terminal state.
///
/// Driven by the subscription, with a slow poll behind it: an event stream is
/// the fast path, but a client that has silently missed the one item that said
/// "finished" would otherwise wait forever.
pub fn follow_run(
    ctx: &mut Ctx,
    daemon: &Daemon,
    run_id: Uuid,
    job_name: &str,
) -> CliResult<Followed> {
    let mut subscription = daemon.subscribe(vec![Topic::Progress, Topic::Events, Topic::Status])?;
    ctx.ui.note(format!("Waiting for {job_name} to finish. Press Ctrl-C to stop waiting."));

    let mut seen_active = false;
    let mut interrupted = false;
    let mut finished: Option<RunStatus> = None;
    let mut gone_from_status = 0u32;
    let started_at = std::time::Instant::now();

    daemon.block_on(async {
        let mut poll = tokio::time::interval(Duration::from_secs(2));
        poll.tick().await; // the first tick is immediate
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    interrupted = true;
                    return;
                }
                item = subscription.next() => {
                    let Some(item) = item else { return };
                    match item {
                        StreamItem::Progress { run_id: id, status, progress, .. } if id == run_id => {
                            seen_active = true;
                            if status.is_terminal() {
                                finished = Some(status);
                                return;
                            }
                            ctx.ui.progress_line(&progress_text(job_name, &progress, status));
                        }
                        StreamItem::Event { event } if event.run_id == Some(run_id) => {
                            if event.severity >= Severity::Warning {
                                ctx.ui.clear_transient();
                                ctx.ui.warn(&event.message);
                            }
                        }
                        StreamItem::Status { snapshot } => {
                            if snapshot.active_runs.iter().any(|r| r.run_id == run_id) {
                                seen_active = true;
                            } else if seen_active {
                                return;
                            }
                        }
                        StreamItem::Lagged { missed } => {
                            // Never dropped silently: the display is now stale
                            // and the user has to be told before they read it.
                            ctx.ui.clear_transient();
                            ctx.ui.warn(format!(
                                "{missed} progress updates were missed; the figures below \
                                 resynchronise on the next one"
                            ));
                        }
                        _ => {}
                    }
                }
                _ = poll.tick() => {
                    let Ok(reply) = daemon.client().request(Request::Status {}).await else {
                        continue;
                    };
                    let superbackup_core::ipc::protocol::Reply::Status(status) = reply else {
                        continue;
                    };
                    if status.snapshot.active_runs.iter().any(|r| r.run_id == run_id) {
                        seen_active = true;
                        gone_from_status = 0;
                    } else {
                        gone_from_status += 1;
                        // Two consecutive polls without it, and at least a
                        // moment for a just-accepted run to appear.
                        if gone_from_status >= 2 && started_at.elapsed() >= Duration::from_secs(2) {
                            return;
                        }
                    }
                }
            }
        }
    });

    ctx.ui.clear_transient();

    if interrupted {
        return Err(CliError::new(
            ErrorCode::JobCancelled,
            format!("stopped waiting for {job_name}; the run continues in the background"),
        )
        .with_hint("Follow it with `superbackup status`, or stop it with `superbackup stop`."));
    }

    // Whatever the stream said, the history is the record. Read the outcome
    // from it rather than from the last item that happened to arrive.
    if let Some(run) = find_run(daemon, run_id)? {
        report_run(ctx, &run);
        return Ok(Followed::Finished(Box::new(run)));
    }
    if let Some(status) = finished {
        ctx.ui.line(format!("{job_name}: {}", status.title()));
        return Ok(Followed::Unknown { run_id });
    }
    ctx.ui.warn(format!(
        "{job_name} is no longer running, but no record of run {run_id} was found"
    ));
    Ok(Followed::Unknown { run_id })
}

fn find_run(daemon: &Daemon, run_id: Uuid) -> CliResult<Option<JobRun>> {
    let history = reply!(daemon, Request::JobHistory { job: None, limit: 50 }, Runs)?;
    Ok(history.runs.into_iter().find(|r| r.run_id == run_id))
}

fn report_run(ctx: &mut Ctx, run: &JobRun) {
    let colour = run_colour(run.status);
    let took = match run.duration_seconds() {
        Some(0) => " in under a second".to_string(),
        Some(seconds) => format!(" in {}", format::duration_secs(seconds)),
        None => String::new(),
    };
    let uploaded: u64 = run.destinations.iter().map(|d| d.progress.bytes_uploaded).sum();
    let files: u64 = run.destinations.iter().map(|d| d.progress.files_processed).sum();
    ctx.ui.coloured(
        colour,
        &format!(
            "{}: {}{took} - {}, {} uploaded",
            run.job_name,
            run.status.title(),
            format::plural(files as usize, "file", "files"),
            format::bytes(uploaded)
        ),
    );
    for dest in &run.destinations {
        if let Some(error) = &dest.error {
            ctx.ui.line(format!("  {}: {}", dest.destination_name, error.message));
            if let Some(hint) = &error.hint {
                ctx.ui.line(format!("    {hint}"));
            }
        }
        for warning in &dest.warnings {
            ctx.ui.line(format!("  {}: {warning}", dest.destination_name));
        }
    }
}

fn progress_text(
    job_name: &str,
    progress: &superbackup_core::state::Progress,
    status: RunStatus,
) -> String {
    match progress.fraction() {
        Some(f) => format!(
            "{job_name}  {} {}  {}  {}",
            format::progress_bar(Some(f), 20),
            format::percent(f),
            format::bytes(progress.bytes_processed),
            format::rate(progress.bytes_per_second),
        ),
        None => format!(
            "{job_name}  {}  {}, {}",
            status.title(),
            format::plural(progress.files_processed as usize, "file", "files"),
            format::bytes(progress.bytes_processed)
        ),
    }
}

// ---------------------------------------------------------------------------
// stop
// ---------------------------------------------------------------------------

pub fn stop(ctx: &mut Ctx, args: StopArgs) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;

    if args.all {
        let stopped = reply!(daemon, Request::JobStopAll {}, Stopped)?;
        report_stopped(ctx, &stopped.stopped);
        return Outcome::data(stopped);
    }

    let Some(needle) = args.job.as_deref() else {
        return Err(CliError::usage("name a job or a run to stop, or pass --all"));
    };

    // The argument is documented as "job or run id", so a run id is tried
    // first: it is unambiguous, and a user pasting one from `status` should
    // not be told there is no job by that name.
    let snapshot = *reply!(daemon, Request::Status {}, Status)?.snapshot;
    let run_id = match Uuid::parse_str(needle) {
        Ok(id) if snapshot.active_runs.iter().any(|r| r.run_id == id) => id,
        _ => {
            let job = resolve_job(&daemon, needle)?;
            match snapshot.active_runs.iter().find(|r| r.job_id == job.id) {
                Some(run) => run.run_id,
                None => {
                    // Stopping something that is not running is not a failure;
                    // it is the state the caller wanted.
                    ctx.ui.line(format!("{} is not running.", job.name));
                    return Outcome::data(serde_json::json!({ "stopped": [] }));
                }
            }
        }
    };

    let stopped = reply!(daemon, Request::JobStop { run_id }, Stopped)?;
    report_stopped(ctx, &stopped.stopped);
    Outcome::data(stopped)
}

fn report_stopped(ctx: &mut Ctx, stopped: &[Uuid]) {
    if stopped.is_empty() {
        ctx.ui.line("Nothing was running.");
    } else {
        ctx.ui.line(format!("Stopping {}.", format::plural(stopped.len(), "run", "runs")));
    }
}

// ---------------------------------------------------------------------------
// pause / resume
// ---------------------------------------------------------------------------

pub fn pause(ctx: &mut Ctx, args: PauseArgs) -> CliResult<Outcome> {
    let seconds = match (&args.duration, args.until_resumed) {
        (Some(text), _) => Some(timespec::parse_duration(text)?.num_seconds().max(1) as u64),
        (None, true) => None,
        (None, false) => {
            return Err(CliError::usage("say how long to pause, or pass --until-resumed")
                .with_hint("For example: superbackup pause 4h"))
        }
    };

    let daemon = Daemon::connect(ctx, Start::Never)?;
    let paused = reply!(
        daemon,
        Request::ControlPause { seconds, reason: args.reason.clone() },
        Pause
    )?;

    match paused.pause.until {
        Some(until) => ctx.ui.line(format!(
            "Backups are paused until {} ({}).",
            format::absolute_local(until),
            format::relative(until, Utc::now())
        )),
        None => ctx.ui.line("Backups are paused until you run `superbackup resume`."),
    }
    if let Some(reason) = &paused.pause.reason {
        ctx.ui.line(format!("Reason: {reason}"));
    }
    Outcome::data(paused)
}

pub fn resume(ctx: &mut Ctx) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;
    let paused = reply!(daemon, Request::ControlResume {}, Pause)?;
    ctx.ui.line("Backups will run on their schedules again.");
    Outcome::data(paused)
}

// ---------------------------------------------------------------------------
// watch
// ---------------------------------------------------------------------------

/// Stream events as NDJSON, one object per line, flushed as they arrive.
///
/// Always NDJSON, with or without `--json`: that is what the command promises
/// in its help text, and a stream is not a document. Anything that is not an
/// event — a note, a lag warning — goes to stderr so a `| jq` on the other end
/// sees only objects.
pub fn watch(ctx: &mut Ctx, args: WatchArgs) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;

    let job = match &args.job {
        Some(needle) => Some(resolve_job(&daemon, needle)?),
        None => None,
    };

    let mut topics = vec![Topic::Events];
    if args.progress {
        topics.push(Topic::Progress);
    }
    let mut subscription = daemon.subscribe(topics)?;

    // Say exactly what is being watched: a filter that quietly matched
    // nothing looks identical to a daemon that has gone quiet.
    let subject = match (&job, args.kinds.is_empty()) {
        (Some(j), true) => format!("events for {}", j.name),
        (Some(j), false) => format!("{} events for {}", args.kinds.join(", "), j.name),
        (None, true) => "every event".to_string(),
        (None, false) => format!("{} events", args.kinds.join(", ")),
    };
    let also = if args.progress { " and live progress" } else { "" };
    ctx.ui.note(format!("Watching {subject}{also}. Press Ctrl-C to stop."));

    let mut emitted = 0usize;
    let limit = args.limit.unwrap_or(usize::MAX);

    daemon.block_on(async {
        loop {
            let item = tokio::select! {
                // Ctrl-C is an ordinary way to end a stream, not a crash.
                _ = tokio::signal::ctrl_c() => return,
                item = subscription.next() => item,
            };
            let Some(item) = item else { return };

            if let StreamItem::Lagged { missed } = &item {
                // Surfaced, never swallowed: a consumer that silently loses
                // events believes it has seen everything.
                ctx.ui.warn(format!(
                    "{missed} events were dropped because this client could not keep up"
                ));
            }
            if !matches(&item, job.as_ref(), &args.kinds) {
                continue;
            }
            match serde_json::to_string(&item) {
                Ok(line) => ctx.ui.stream_line(&line),
                Err(e) => ctx.ui.warn(format!("an event could not be rendered: {e}")),
            }
            emitted += 1;
            if emitted >= limit {
                return;
            }
        }
    });

    Ok(Outcome::streamed())
}

/// Client-side filtering. The subscription is by topic; job and kind filters
/// are applied here because the protocol has no per-job subscription.
fn matches(item: &StreamItem, job: Option<&Job>, kinds: &[String]) -> bool {
    match item {
        StreamItem::Lagged { .. } => true,
        StreamItem::Event { event } => {
            if let Some(job) = job {
                if event.job_id != Some(job.id) {
                    return false;
                }
            }
            if kinds.is_empty() {
                return true;
            }
            // `--kind job` matches `job.started`, so a caller can follow a
            // family without listing every member.
            kinds
                .iter()
                .any(|k| event.kind == *k || event.kind.starts_with(&format!("{k}.")))
        }
        StreamItem::Progress { job_id, .. } => {
            // A progress item has no kind, so an explicit --kind filter is a
            // request for events only.
            kinds.is_empty() && job.map(|j| j.id == *job_id).unwrap_or(true)
        }
        StreamItem::Status { .. } => kinds.is_empty() && job.is_none(),
    }
}
