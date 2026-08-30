//! Executing one job across all of its destinations.
//!
//! The runner is the only place in the engine that produces a [`JobRun`]. It
//! owns the fan-out over destinations, the retry loop, the timeout, the hooks,
//! the progress plumbing, and the write into [`PersistedState`].
//!
//! # Why destinations run one at a time
//!
//! A job typically fans out to a fast local repository, a OneDrive folder and
//! an offsite bucket. Running those concurrently would trade a shorter run for
//! three processes competing for the same disk and the same uplink, and would
//! make the bandwidth ceiling meaningless (three destinations each throttled to
//! the limit is three times the limit). Sequential is slower on paper and
//! faster in practice, and it makes the progress stream comprehensible.
//!
//! # Concurrency invariants
//!
//! * Each destination attempt runs in its own `tokio::spawn`. This is what
//!   lets the runner impose a **grace period** on a driver that ignores its
//!   cancel token: the task is never aborted — aborting is precisely how an
//!   orphaned `kopia` process and a stale repository lock are created — it is
//!   left to finish and reap itself while the run is reported as cancelled.
//! * The shared [`PersistedState`] mutex is taken exactly once per run, at the
//!   end, and is never held across an `await` inside the critical section.
//! * Progress flows one way: executor → unbounded mpsc → forwarder task →
//!   broadcast. The forwarder also keeps the latest snapshot per destination
//!   so that a cancelled or failed destination still records what it had
//!   managed to do.
//! * Nothing here holds the run's `CancelToken` across a lock.

use crate::engine::cancel::{CancelReason, CancelToken};
use crate::engine::clock::Clock;
use crate::engine::executor::{
    BackupExecutor, ExecutorError, ExecutorResult, PrepareRequest, ProgressSink, ProgressUpdate,
    SnapshotOutcome, SnapshotRequest,
};
use crate::engine::hooks::{HookContext, HookKind, HookOutcome, HookRunner};
use crate::engine::mirror::{MirrorEngine, MirrorOptions, MirrorRequest};
use crate::engine::retry::RetryPolicy;
use crate::engine::schedule::Zone;
use crate::engine::EngineEvent;
use crate::error::ErrorCode;
use crate::model::{Destination, DestinationKind, Job, RetentionPolicy, Settings};
use crate::state::{
    DestinationRun, Event, JobRun, PersistedState, Progress, RunStatus, Severity, Trigger,
};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// How long a run waits for a driver to honour its cancel token before
/// reporting the run cancelled and letting the driver finish unwinding in the
/// background.
///
/// The [`BackupExecutor`] contract asks for about a second; ten gives a
/// well-behaved driver room to kill a child and release a lock on a busy
/// machine, while keeping the daemon responsive to a user who pressed Stop.
pub const CANCEL_GRACE_SECONDS: i64 = 10;

/// Everything needed to execute one job once.
#[derive(Debug, Clone)]
pub struct RunRequest {
    pub run_id: Uuid,
    pub job: Arc<Job>,
    /// Resolved from `job.destination_ids`, in order, already filtered to
    /// enabled destinations by the scheduler.
    pub destinations: Vec<Arc<Destination>>,
    pub settings: Arc<Settings>,
    pub trigger: Trigger,
    /// This run's own token — a child of the engine token, so a shutdown stops
    /// it but stopping it touches no other run.
    pub cancel: CancelToken,
}

/// Executes runs. One instance is shared by the scheduler and by every manual
/// trigger; it holds no per-run state.
#[derive(Debug, Clone)]
pub struct Runner {
    executor: Arc<dyn BackupExecutor>,
    mirror: MirrorEngine,
    clock: Arc<dyn Clock>,
    zone: Arc<dyn Zone>,
    hooks: HookRunner,
    events: tokio::sync::broadcast::Sender<EngineEvent>,
    state: Arc<tokio::sync::Mutex<PersistedState>>,
    retry: RetryPolicy,
}

impl Runner {
    pub fn new(
        executor: Arc<dyn BackupExecutor>,
        clock: Arc<dyn Clock>,
        zone: Arc<dyn Zone>,
        events: tokio::sync::broadcast::Sender<EngineEvent>,
        state: Arc<tokio::sync::Mutex<PersistedState>>,
    ) -> Runner {
        Runner {
            mirror: MirrorEngine::new(Arc::clone(&clock)),
            hooks: HookRunner::new(Arc::clone(&clock)),
            executor,
            clock,
            zone,
            events,
            state,
            retry: RetryPolicy::default(),
        }
    }

    /// Override the retry policy. Tests use [`RetryPolicy::none`] to assert a
    /// single attempt without waiting for backoff.
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Runner {
        self.retry = policy;
        self
    }

    /// Override the hook runner, e.g. to shorten the hook timeout.
    pub fn with_hooks(mut self, hooks: HookRunner) -> Runner {
        self.hooks = hooks;
        self
    }

    /// Execute one job to completion and record the result.
    ///
    /// Never returns an error: every failure mode is representable in the
    /// returned [`JobRun`], and a caller that had to handle both would end up
    /// inventing a second, worse status enum.
    pub async fn execute(&self, request: RunRequest) -> JobRun {
        let started_at = self.clock.now_utc();
        let mut run = JobRun {
            run_id: request.run_id,
            job_id: request.job.id,
            job_name: request.job.name.clone(),
            trigger: request.trigger,
            status: RunStatus::Running,
            started_at,
            finished_at: None,
            destinations: request
                .destinations
                .iter()
                .map(|d| new_destination_run(d.as_ref()))
                .collect(),
        };

        self.emit(EngineEvent::RunStarted { run: run.clone() });
        self.log(
            Event::info("job.started", format!("{} started", run.job_name))
                .with_job(run.job_id)
                .with_run(run.run_id)
                .with_field("trigger", format!("{:?}", run.trigger)),
        );

        // Progress fan-in. Unbounded because dropping progress under back
        // pressure would stall the executor, and the forwarder only ever moves
        // small structs between two in-process channels.
        let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel::<ProgressUpdate>();
        let latest = Arc::new(std::sync::Mutex::new(HashMap::<Uuid, Progress>::new()));
        // The forwarder is stopped by an explicit signal rather than by the
        // channel closing. A driver that overran its cancel grace still owns a
        // `ProgressSink`, so waiting for the last sender to drop could mean
        // waiting for a wedged process — inside the code path whose whole
        // purpose is not to wait for it.
        let progress_done = CancelToken::new();
        let forwarder = self.spawn_progress_forwarder(
            progress_rx,
            Arc::clone(&latest),
            progress_done.clone(),
        );

        // The timeout is expressed as a cancellation so that the driver is
        // told to stop, rather than having its future dropped from underneath
        // it. That is the difference between a clean abort and a stale lock.
        let watchdog = self.spawn_timeout_watchdog(&request);

        let context = HookContext {
            job_id: request.job.id,
            job_name: request.job.name.clone(),
            run_id: request.run_id,
            status: None,
        };

        let mut aborted_by_hook: Option<HookOutcome> = None;
        if let Some(outcome) =
            self.hooks.run_before(&request.job.hooks, &context, &request.cancel).await
        {
            self.record_hook(&run, &outcome);
            if !outcome.succeeded() && request.job.hooks.abort_on_before_failure {
                aborted_by_hook = Some(outcome);
            }
        }

        if let Some(outcome) = aborted_by_hook {
            // Nothing ran, so `derive_status` has nothing to derive from: an
            // all-`Skipped` run would roll up to "succeeded with warnings",
            // which is exactly the wrong thing to tell someone whose backup
            // did not happen. The status is set explicitly instead.
            for dest in &mut run.destinations {
                dest.status = RunStatus::Skipped;
                dest.finished_at = Some(self.clock.now_utc());
            }
            run.status = RunStatus::Failed;
            if let Some(first) = run.destinations.first_mut() {
                first.error = Some(
                    ExecutorError::new(ErrorCode::Internal, outcome.summary())
                        .with_detail(outcome.output.clone())
                        .to_run_error(),
                );
            }
            return self
                .finalise(
                    run,
                    &request,
                    &context,
                    watchdog,
                    forwarder,
                    progress_tx,
                    progress_done,
                    latest,
                )
                .await;
        }

        for index in 0..run.destinations.len() {
            if request.cancel.is_cancelled() {
                self.mark_remaining(&mut run, index, RunStatus::Cancelled);
                break;
            }
            let destination = Arc::clone(&request.destinations[index]);
            let outcome = self
                .run_destination(&request, &destination, &progress_tx, Arc::clone(&latest))
                .await;
            self.apply_destination_outcome(&mut run, index, outcome);

            let failed = run.destinations[index].status == RunStatus::Failed;
            if failed && !request.job.continue_on_destination_error {
                self.mark_remaining(&mut run, index + 1, RunStatus::Skipped);
                break;
            }
            if run.destinations[index].status == RunStatus::Cancelled {
                self.mark_remaining(&mut run, index + 1, RunStatus::Cancelled);
                break;
            }
        }

        run.status = run.derive_status();
        self.finalise(run, &request, &context, watchdog, forwarder, progress_tx, progress_done, latest)
            .await
    }

    /// Run one destination, including the retry loop.
    async fn run_destination(
        &self,
        request: &RunRequest,
        destination: &Arc<Destination>,
        progress_tx: &tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
        latest: Arc<std::sync::Mutex<HashMap<Uuid, Progress>>>,
    ) -> DestinationOutcome {
        self.emit(EngineEvent::DestinationStarted {
            run_id: request.run_id,
            job_id: request.job.id,
            destination_id: destination.id,
        });

        let mut attempt = 1u32;
        loop {
            if request.cancel.is_cancelled() {
                return DestinationOutcome::Cancelled(self.snapshot_latest(&latest, destination.id));
            }
            // Re-resolved per attempt: a run that crosses into a bandwidth
            // window is throttled from that attempt onwards rather than
            // keeping the ceiling it started with.
            let bandwidth = self.zone.resolve_bandwidth(
                request.job.bandwidth.as_ref(),
                destination.bandwidth.as_ref(),
                &request.settings.bandwidth,
                self.clock.now_utc(),
            );
            let sink = ProgressSink::new(
                progress_tx.clone(),
                request.run_id,
                request.job.id,
                destination.id,
                Arc::clone(&self.clock),
            );

            let result = self
                .attempt_destination(request, destination, bandwidth, sink, attempt)
                .await;

            match result {
                Ok(outcome) => return DestinationOutcome::Succeeded(Box::new(outcome)),
                Err(error) if error.is_cancellation() || request.cancel.is_cancelled() => {
                    return DestinationOutcome::Cancelled(
                        self.snapshot_latest(&latest, destination.id),
                    )
                }
                Err(error) => {
                    if self.retry.should_retry(attempt, &error) {
                        let delay = self.retry.delay_after(attempt);
                        self.log(
                            Event::warn(
                                "job.retry",
                                format!(
                                    "{} failed against {} (attempt {attempt}): {}; retrying in {}s",
                                    request.job.name,
                                    destination.name,
                                    error.message,
                                    delay.num_seconds()
                                ),
                            )
                            .with_job(request.job.id)
                            .with_run(request.run_id)
                            .with_destination(destination.id),
                        );
                        self.emit(EngineEvent::DestinationRetrying {
                            run_id: request.run_id,
                            job_id: request.job.id,
                            destination_id: destination.id,
                            attempt,
                            retry_in_seconds: delay.num_seconds(),
                        });
                        // Sleeping in a `select!` against the token keeps a
                        // Stop pressed during backoff instantaneous, instead
                        // of waiting out 80 seconds of politeness.
                        tokio::select! {
                            _ = self.clock.sleep(delay) => {}
                            _ = request.cancel.cancelled() => {
                                return DestinationOutcome::Cancelled(
                                    self.snapshot_latest(&latest, destination.id),
                                );
                            }
                        }
                        attempt += 1;
                        continue;
                    }
                    return DestinationOutcome::Failed(
                        Box::new(error),
                        self.snapshot_latest(&latest, destination.id),
                    );
                }
            }
        }
    }

    /// One attempt against one destination: prepare, then snapshot (or mirror).
    ///
    /// The work is spawned so that a driver ignoring its cancel token cannot
    /// wedge the run; see [`CANCEL_GRACE_SECONDS`].
    async fn attempt_destination(
        &self,
        request: &RunRequest,
        destination: &Arc<Destination>,
        bandwidth: crate::engine::throttle::ResolvedBandwidth,
        sink: ProgressSink,
        attempt: u32,
    ) -> ExecutorResult<SnapshotOutcome> {
        let cancel = request.cancel.clone();
        let job = Arc::clone(&request.job);
        let destination = Arc::clone(destination);
        let run_id = request.run_id;
        let executor = Arc::clone(&self.executor);
        let mirror = self.mirror.clone();
        let retention = effective_retention(&job, &destination);

        let mut handle = tokio::spawn(async move {
            if destination.kind.is_repository() {
                let prepared = executor
                    .prepare(PrepareRequest {
                        run_id,
                        destination: Arc::clone(&destination),
                        retention,
                        create_if_missing: true,
                        cancel: cancel.clone(),
                    })
                    .await?;
                let mut outcome = executor
                    .snapshot(SnapshotRequest {
                        run_id,
                        job_id: job.id,
                        job_name: job.name.clone(),
                        destination: Arc::clone(&destination),
                        sources: job.sources.clone(),
                        exclusions: job.exclusions.clone(),
                        bandwidth,
                        progress: sink,
                        cancel,
                        attempt,
                    })
                    .await?;
                outcome.warnings.extend(prepared.warnings);
                Ok(outcome)
            } else {
                let options = MirrorOptions::from_exclusions(&job.exclusions);
                mirror
                    .run(MirrorRequest {
                        run_id,
                        job_id: job.id,
                        destination: Arc::clone(&destination),
                        sources: job.sources.clone(),
                        exclusions: job.exclusions.clone(),
                        options,
                        bandwidth,
                        progress: sink,
                        cancel,
                    })
                    .await
            }
        });

        let grace = Duration::seconds(CANCEL_GRACE_SECONDS);
        let clock = Arc::clone(&self.clock);
        let cancel = request.cancel.clone();
        tokio::select! {
            joined = &mut handle => match joined {
                Ok(result) => result,
                Err(join) => Err(ExecutorError::new(
                    ErrorCode::Internal,
                    format!("the backup worker stopped unexpectedly: {join}"),
                )),
            },
            _ = async move {
                cancel.cancelled().await;
                clock.sleep(grace).await;
            } => {
                // Deliberately *not* `handle.abort()`. The task owns a child
                // process and a repository lock; killing its future would
                // strand both. It is left running to unwind on its own, and
                // the run is reported as cancelled now.
                Err(ExecutorError::new(
                    ErrorCode::JobCancelled,
                    format!(
                        "the backup for {} did not stop within {CANCEL_GRACE_SECONDS}s and is still shutting down",
                        request.job.name
                    ),
                )
                .permanent())
            }
        }
    }

    /// Fold one destination's outcome into the run.
    fn apply_destination_outcome(
        &self,
        run: &mut JobRun,
        index: usize,
        outcome: DestinationOutcome,
    ) {
        let now = self.clock.now_utc();
        let Some(dest) = run.destinations.get_mut(index) else { return };
        dest.finished_at = Some(now);
        match outcome {
            DestinationOutcome::Succeeded(result) => {
                let result = *result;
                dest.progress = result.progress;
                dest.snapshot_id = result.snapshot_id;
                dest.warnings = result.warnings;
                dest.status = if dest.warnings.is_empty() {
                    RunStatus::Succeeded
                } else {
                    RunStatus::SucceededWithWarnings
                };
            }
            DestinationOutcome::Failed(error, progress) => {
                dest.progress = progress;
                dest.error = Some(error.to_run_error());
                dest.status = RunStatus::Failed;
            }
            DestinationOutcome::Cancelled(progress) => {
                dest.progress = progress;
                dest.status = RunStatus::Cancelled;
            }
        }
        let severity = match dest.status {
            RunStatus::Failed => Severity::Error,
            RunStatus::SucceededWithWarnings | RunStatus::Cancelled => Severity::Warning,
            _ => Severity::Info,
        };
        let message = match &dest.error {
            Some(e) => format!("{} → {}: {}", run.job_name, dest.destination_name, e.message),
            None => format!(
                "{} → {}: {}",
                run.job_name,
                dest.destination_name,
                dest.status.title().to_lowercase()
            ),
        };
        self.log(
            Event::new(severity, "job.destination", message)
                .with_job(run.job_id)
                .with_run(run.run_id)
                .with_destination(dest.destination_id),
        );
        self.emit(EngineEvent::DestinationFinished {
            run_id: run.run_id,
            job_id: run.job_id,
            destination: Box::new(dest.clone()),
        });
    }

    /// Close out a run: after-hooks, persistence, events.
    #[allow(clippy::too_many_arguments)]
    async fn finalise(
        &self,
        mut run: JobRun,
        request: &RunRequest,
        context: &HookContext,
        watchdog: Option<tokio::task::JoinHandle<()>>,
        forwarder: tokio::task::JoinHandle<()>,
        progress_tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
        progress_done: CancelToken,
        latest: Arc<std::sync::Mutex<HashMap<Uuid, Progress>>>,
    ) -> JobRun {
        // Stop the watchdog before the after-hooks, or a job that finished at
        // 05:59:59 would have its `after_success` hook cancelled at 06:00:00.
        if let Some(watchdog) = watchdog {
            watchdog.abort();
        }
        // A timeout expresses itself as a cancellation, so translate it back
        // into the status the user needs to see.
        if run.status == RunStatus::Cancelled
            && request.cancel.reason() == Some(CancelReason::Timeout)
        {
            run.status = RunStatus::Failed;
            let timeout_error = ExecutorError::new(
                ErrorCode::JobCancelled,
                format!(
                    "{} exceeded its {}-minute time limit and was stopped",
                    run.job_name,
                    request.job.timeout_minutes.unwrap_or_default()
                ),
            )
            .with_hint("Raise the time limit for this job, or narrow what it backs up.");
            for dest in run.destinations.iter_mut().filter(|d| d.status == RunStatus::Cancelled) {
                dest.status = RunStatus::Failed;
                dest.error = Some(timeout_error.to_run_error());
            }
        }

        run.finished_at = Some(self.clock.now_utc());

        let succeeded =
            matches!(run.status, RunStatus::Succeeded | RunStatus::SucceededWithWarnings);
        let after_context = HookContext { status: Some(run.status.title().to_string()), ..context.clone() };
        let hook = if succeeded {
            request.job.hooks.after_success.as_ref().map(|c| (HookKind::AfterSuccess, c))
        } else {
            request.job.hooks.after_failure.as_ref().map(|c| (HookKind::AfterFailure, c))
        };
        if let Some((kind, command)) = hook {
            // The after-hooks run on a *fresh* token: the run's own token may
            // already have fired (Stop, timeout), and an `after_failure` hook
            // that is skipped precisely when the job failed is worse than no
            // hook at all.
            let hook_cancel = CancelToken::new();
            let outcome = self.hooks.run(kind, command, &after_context, &hook_cancel).await;
            self.record_hook(&run, &outcome);
        }

        // Stop the forwarder and wait for it to drain. Awaiting it is what
        // guarantees every emitted update reaches the broadcast before the
        // run-finished event does, so a subscriber never sees "finished"
        // ahead of the final progress frame.
        drop(progress_tx);
        progress_done.cancel(CancelReason::Shutdown);
        let _ = forwarder.await;
        for dest in run.destinations.iter_mut() {
            if dest.progress.files_processed == 0 && dest.progress.bytes_processed == 0 {
                if let Some(p) = latest.lock().ok().and_then(|m| m.get(&dest.destination_id).cloned())
                {
                    dest.progress = p;
                }
            }
        }

        {
            let mut state = self.state.lock().await;
            state.record(run.clone());
        }

        let severity = match run.status {
            RunStatus::Failed => Severity::Error,
            RunStatus::SucceededWithWarnings | RunStatus::Cancelled => Severity::Warning,
            _ => Severity::Info,
        };
        self.log(
            Event::new(
                severity,
                "job.finished",
                format!("{} {}", run.job_name, run.status.title().to_lowercase()),
            )
            .with_job(run.job_id)
            .with_run(run.run_id)
            .with_field("status", format!("{:?}", run.status))
            .with_field("seconds", run.duration_seconds().unwrap_or_default()),
        );
        self.emit(EngineEvent::RunFinished { run: run.clone() });
        run
    }

    // -- plumbing -----------------------------------------------------------

    /// Forward coalesced progress to the broadcast channel and keep the latest
    /// snapshot per destination.
    fn spawn_progress_forwarder(
        &self,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<ProgressUpdate>,
        latest: Arc<std::sync::Mutex<HashMap<Uuid, Progress>>>,
        done: CancelToken,
    ) -> tokio::task::JoinHandle<()> {
        let events = self.events.clone();
        tokio::spawn(async move {
            let publish = |update: ProgressUpdate| {
                if let Ok(mut map) = latest.lock() {
                    map.insert(update.destination_id, update.progress.clone());
                }
                // A broadcast with no subscribers, or a lagging one, is not an
                // error: the tray may simply not be running.
                let _ = events.send(EngineEvent::Progress(update));
            };
            loop {
                tokio::select! {
                    received = rx.recv() => match received {
                        Some(update) => publish(update),
                        None => break,
                    },
                    _ = done.cancelled() => break,
                }
            }
            // Drain whatever was already queued, so the final frame of a
            // destination is never lost to the stop signal racing it.
            while let Ok(update) = rx.try_recv() {
                publish(update);
            }
        })
    }

    /// Turn `Job::timeout_minutes` into a cancellation at the deadline.
    fn spawn_timeout_watchdog(&self, request: &RunRequest) -> Option<tokio::task::JoinHandle<()>> {
        let minutes = request.job.timeout_minutes.filter(|m| *m > 0)?;
        let deadline = self.clock.now_utc() + Duration::minutes(minutes as i64);
        let clock = Arc::clone(&self.clock);
        let cancel = request.cancel.clone();
        Some(tokio::spawn(async move {
            clock.sleep_until(deadline).await;
            cancel.cancel(CancelReason::Timeout);
        }))
    }

    fn snapshot_latest(
        &self,
        latest: &Arc<std::sync::Mutex<HashMap<Uuid, Progress>>>,
        destination_id: Uuid,
    ) -> Progress {
        latest
            .lock()
            .ok()
            .and_then(|m| m.get(&destination_id).cloned())
            .unwrap_or_default()
    }

    fn mark_remaining(&self, run: &mut JobRun, from: usize, status: RunStatus) {
        let now = self.clock.now_utc();
        for dest in run.destinations.iter_mut().skip(from) {
            if dest.status.is_active() {
                dest.status = status;
                dest.finished_at = Some(now);
            }
        }
    }

    fn record_hook(&self, run: &JobRun, outcome: &HookOutcome) {
        let severity = if outcome.succeeded() { Severity::Info } else { Severity::Warning };
        self.log(
            Event::new(severity, "job.hook", outcome.summary())
                .with_job(run.job_id)
                .with_run(run.run_id)
                .with_field("output", outcome.output.clone()),
        );
        self.emit(EngineEvent::HookFinished {
            run_id: run.run_id,
            job_id: run.job_id,
            outcome: Box::new(outcome.clone()),
        });
    }

    fn emit(&self, event: EngineEvent) {
        let _ = self.events.send(event);
    }

    fn log(&self, event: Event) {
        match event.severity {
            Severity::Error => tracing::error!(kind = %event.kind, "{}", event.message),
            Severity::Warning => tracing::warn!(kind = %event.kind, "{}", event.message),
            _ => tracing::info!(kind = %event.kind, "{}", event.message),
        }
        self.emit(EngineEvent::Log(Box::new(event)));
    }
}

/// The three ways one destination can end.
#[derive(Debug)]
enum DestinationOutcome {
    // Boxed to keep the enum small; `SnapshotOutcome` carries a whole
    // `Progress` and a warning list.
    Succeeded(Box<SnapshotOutcome>),
    Failed(Box<ExecutorError>, Progress),
    Cancelled(Progress),
}

fn new_destination_run(destination: &Destination) -> DestinationRun {
    DestinationRun {
        destination_id: destination.id,
        destination_name: destination.name.clone(),
        status: RunStatus::Queued,
        started_at: None,
        finished_at: None,
        progress: Progress::default(),
        snapshot_id: None,
        error: None,
        warnings: Vec::new(),
    }
}

/// Job override beats destination policy, matching the precedence used for
/// bandwidth so the two settings behave the same way.
fn effective_retention(job: &Job, destination: &Destination) -> RetentionPolicy {
    job.retention.clone().unwrap_or_else(|| destination.retention.clone())
}

/// True for destinations the runner can actually write to. Exposed because the
/// scheduler filters with it before building a [`RunRequest`].
pub fn destination_is_usable(destination: &Destination) -> bool {
    destination.enabled
        && match &destination.kind {
            DestinationKind::LocalMirror { path }
            | DestinationKind::LocalRepository { path }
            | DestinationKind::OneDrive { path, .. } => !path.as_os_str().is_empty(),
            DestinationKind::S3 { bucket, .. } => !bucket.is_empty(),
        }
}

/// Time at which a run started plus its timeout, for the GUI's countdown.
pub fn deadline_for(job: &Job, started_at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    job.timeout_minutes.filter(|m| *m > 0).map(|m| started_at + Duration::minutes(m as i64))
}
