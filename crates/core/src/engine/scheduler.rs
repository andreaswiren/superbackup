//! The timing wheel, the gates, and the queue.
//!
//! One tokio task owns all scheduling state. Everything else talks to it
//! through [`SchedulerHandle`], which is a channel sender. That is the whole
//! concurrency design, and it is deliberate: a scheduler whose state is shared
//! behind a mutex ends up with "check then act" races (two callers both see a
//! free slot, both start a run) that are impossible when one task decides.
//!
//! # Waking
//!
//! The loop sleeps until the *earliest* due moment across every job, not on a
//! poll interval. A one-second poll would wake 86 400 times a day to do
//! nothing, which on a laptop is a measurable battery cost and prevents the
//! CPU from reaching its deep idle states.
//!
//! Sleeps are nonetheless capped at [`MAX_SLEEP_SECONDS`]. A timer armed for
//! six hours does not survive suspend-to-RAM in any useful sense — the machine
//! wakes and the timer is either late by the suspend duration or fires
//! immediately — so the loop re-derives its deadline at least once a minute
//! and notices that the wall clock has moved.
//!
//! # Gates
//!
//! A scheduled run is checked against [`evaluate_gates`] immediately before it
//! starts, not when it is queued. The difference matters: a job queued behind
//! three others may reach the front twenty minutes later, by which time the
//! laptop has been unplugged. The one exception is "already running", which is
//! checked at *queue* time as well, because letting duplicates accumulate in
//! the queue would mean a job that takes an hour with a fifteen-minute
//! schedule builds a permanent backlog.
//!
//! # Queueing, not dropping
//!
//! `Settings::max_parallel_jobs` bounds concurrency. Jobs beyond it wait in
//! FIFO order; they are never dropped. A dropped run is a backup that silently
//! did not happen, which is the one outcome this application exists to
//! prevent.
//!
//! A skipped run does **not** consume a slot: it is reported and discarded
//! immediately, so a paused or locked machine drains its queue rather than
//! filling it with runs that can never start.
//!
//! # Where skips are recorded
//!
//! A skip produces an [`EngineEvent::RunSkipped`] and an activity-log
//! [`Event`], both carrying a [`SkipReason`] with user-facing text. It is
//! deliberately **not** written into
//! [`crate::state::PersistedState::history`], and it does not touch the job's
//! [`crate::state::JobSummary`].
//!
//! The reason is arithmetic: a job on a fifteen-minute schedule that is paused
//! over a long weekend generates roughly 300 skips. `history` holds
//! [`crate::state::MAX_HISTORY`] = 200 entries, so recording them would evict
//! every real run — including the failure the user needs to see — and would
//! inflate `total_runs` with runs that never ran. The skips are still fully
//! visible in the activity log and in the live event stream, which is where
//! "why didn't it run at 3am?" is actually answered.

use crate::engine::cancel::{CancelReason, CancelToken};
use crate::engine::clock::Clock;
use crate::engine::runner::{destination_is_usable, RunRequest, Runner};
use crate::engine::schedule::Zone;
use crate::engine::watcher::{JobWatcher, WatchTrigger};
use crate::engine::{EngineEvent, Environment};
use crate::error::{Error, Result};
use crate::model::{Config, Destination, Job, Schedule, Settings};
use crate::state::{Event, PersistedState, RunStatus, Severity, Trigger};
use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use uuid::Uuid;

/// Longest the loop will sleep before re-deriving its deadline. See the module
/// docs on suspend/resume.
pub const MAX_SLEEP_SECONDS: i64 = 60;

/// Why a scheduled run did not start.
///
/// Every variant produces a [`RunStatus::Skipped`] outcome and a user-facing
/// sentence. Silence is not an option: "my backup did not run and nothing told
/// me why" is the complaint that destroys trust in a backup tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The global pause from the tray or `superbackup pause`.
    GloballyPaused,
    /// `Job::enabled` is false.
    JobDisabled,
    /// The vault is locked, so no destination passphrase can be resolved.
    VaultLocked,
    /// `Settings::skip_on_metered` and the connection is metered.
    MeteredConnection,
    /// `Settings::skip_on_battery` and the machine is on battery.
    OnBattery,
    /// The previous run of this job has not finished.
    AlreadyRunning,
    /// Every destination is disabled, missing, or misconfigured.
    NoUsableDestination,
}

impl SkipReason {
    /// The status value this skip records.
    pub fn status(&self) -> RunStatus {
        RunStatus::Skipped
    }

    /// Short label for a list column.
    pub fn title(&self) -> &'static str {
        match self {
            SkipReason::GloballyPaused => "Paused",
            SkipReason::JobDisabled => "Disabled",
            SkipReason::VaultLocked => "Locked",
            SkipReason::MeteredConnection => "Metered connection",
            SkipReason::OnBattery => "On battery",
            SkipReason::AlreadyRunning => "Already running",
            SkipReason::NoUsableDestination => "No destination",
        }
    }

    /// The sentence shown in the activity log and the tooltip.
    pub fn describe(&self) -> &'static str {
        match self {
            SkipReason::GloballyPaused => {
                "Skipped because backups are paused. Resume them from the tray menu."
            }
            SkipReason::JobDisabled => "Skipped because this job is turned off.",
            SkipReason::VaultLocked => {
                "Skipped because the vault is locked. Unlock superbackup so scheduled backups can run."
            }
            SkipReason::MeteredConnection => {
                "Skipped because this connection is metered. Change this in Settings if you want backups on metered networks."
            }
            SkipReason::OnBattery => {
                "Skipped because the machine is on battery. Plug in, or allow battery runs in Settings."
            }
            SkipReason::AlreadyRunning => {
                "Skipped because the previous run of this job has not finished yet."
            }
            SkipReason::NoUsableDestination => {
                "Skipped because this job has no usable destination. Check that its destinations still exist and are enabled."
            }
        }
    }
}

/// Decide whether a run may start.
///
/// Exposed as a free function, rather than buried in the loop, so the gate
/// policy can be tested exhaustively without a running scheduler — and so the
/// GUI can grey out the Run button for the same reasons the scheduler would
/// skip.
///
/// Manual triggers ([`Trigger::Manual`], [`Trigger::Cli`]) deliberately bypass
/// the *policy* gates. A user who clicks Run Now while paused, on battery and
/// tethered has said what they want; the policy gates exist to stop the
/// machine acting on its own, not to stop its owner. The *physical* gates —
/// the vault being locked, the job already running, the destinations being
/// gone — apply to everybody, because ignoring them cannot work.
pub fn evaluate_gates(
    job: &Job,
    settings: &Settings,
    environment: &dyn Environment,
    already_running: bool,
    usable_destinations: usize,
    trigger: Trigger,
    now: DateTime<Utc>,
) -> Option<SkipReason> {
    if already_running {
        return Some(SkipReason::AlreadyRunning);
    }
    if usable_destinations == 0 {
        return Some(SkipReason::NoUsableDestination);
    }
    if !environment.vault_unlocked() {
        return Some(SkipReason::VaultLocked);
    }
    let user_initiated = matches!(trigger, Trigger::Manual | Trigger::Cli);
    if user_initiated {
        return None;
    }
    if !job.enabled {
        return Some(SkipReason::JobDisabled);
    }
    if settings.pause.is_active(now) {
        return Some(SkipReason::GloballyPaused);
    }
    if settings.skip_on_metered && environment.on_metered_connection() {
        return Some(SkipReason::MeteredConnection);
    }
    if settings.skip_on_battery && environment.on_battery() {
        return Some(SkipReason::OnBattery);
    }
    None
}

/// A snapshot of what the scheduler is doing, for `superbackup status`.
#[derive(Debug, Clone, Default)]
pub struct SchedulerStatus {
    /// job id → run id, for runs executing right now.
    pub running: BTreeMap<Uuid, Uuid>,
    /// Job ids waiting for a slot, in order.
    pub queued: Vec<Uuid>,
    /// job id → the next moment it is due.
    pub next_runs: BTreeMap<Uuid, DateTime<Utc>>,
    /// The soonest of `next_runs`.
    pub next_scheduled: Option<(Uuid, DateTime<Utc>)>,
    pub max_parallel: u32,
}

/// Messages accepted by the scheduler task.
#[derive(Debug)]
enum Command {
    Run { job: Uuid, trigger: Trigger, reply: tokio::sync::oneshot::Sender<Result<Uuid>> },
    Cancel { job: Uuid },
    ReplaceConfig { config: Arc<Config> },
    Status { reply: tokio::sync::oneshot::Sender<SchedulerStatus> },
    FileChange { job: Uuid, trigger: WatchTrigger },
    Shutdown,
}

/// The public face of a running engine. Cheap to clone.
#[derive(Debug, Clone)]
pub struct SchedulerHandle {
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    events: tokio::sync::broadcast::Sender<EngineEvent>,
    cancel: CancelToken,
}

impl SchedulerHandle {
    /// Subscribe to the engine's event stream.
    ///
    /// Subscribers that fall behind lag and lose frames rather than blocking a
    /// backup; see [`crate::engine::EVENT_CHANNEL_CAPACITY`].
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    /// Start a job now, bypassing the *policy* gates (pause, metered, battery,
    /// disabled). Returns the new run id.
    ///
    /// `Ok` means the run was queued, not that it will execute: the physical
    /// gates still apply, so a locked vault or a vanished destination turns it
    /// into a [`EngineEvent::RunSkipped`] when it reaches the front of the
    /// queue. Callers that need the outcome should watch the event stream for
    /// the returned run id.
    pub async fn run_now(&self, job: Uuid, trigger: Trigger) -> Result<Uuid> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.commands
            .send(Command::Run { job, trigger, reply })
            .map_err(|_| Error::Internal("the scheduler has stopped".into()))?;
        response.await.map_err(|_| Error::Internal("the scheduler has stopped".into()))?
    }

    /// Stop a running job. Other jobs are untouched.
    pub fn cancel_job(&self, job: Uuid) -> Result<()> {
        self.commands
            .send(Command::Cancel { job })
            .map_err(|_| Error::Internal("the scheduler has stopped".into()))
    }

    /// Swap in a new configuration.
    ///
    /// In-flight runs keep the job and destination snapshots they started
    /// with; only future runs see the new config.
    pub fn replace_config(&self, config: Arc<Config>) -> Result<()> {
        self.commands
            .send(Command::ReplaceConfig { config })
            .map_err(|_| Error::Internal("the scheduler has stopped".into()))
    }

    /// Report a filesystem change for an `OnChange` job. Called by the
    /// watcher, and available to tests that would rather not touch a disk.
    pub fn notify_change(&self, job: Uuid, trigger: WatchTrigger) -> Result<()> {
        self.commands
            .send(Command::FileChange { job, trigger })
            .map_err(|_| Error::Internal("the scheduler has stopped".into()))
    }

    /// What the scheduler is doing right now.
    pub async fn status(&self) -> Result<SchedulerStatus> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.commands
            .send(Command::Status { reply })
            .map_err(|_| Error::Internal("the scheduler has stopped".into()))?;
        response.await.map_err(|_| Error::Internal("the scheduler has stopped".into()))
    }

    /// Stop the engine: cancel every in-flight run and end the loop.
    ///
    /// Returns once the request has been posted. In-flight runs unwind on
    /// their own; the runner's cancel grace bounds how long that takes.
    pub fn shutdown(&self) {
        self.cancel.cancel(CancelReason::Shutdown);
        let _ = self.commands.send(Command::Shutdown);
    }

    /// The engine-wide cancellation token, for callers that want to observe
    /// shutdown without owning it.
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }
}

/// A run waiting for a slot.
#[derive(Debug, Clone)]
struct Pending {
    run_id: Uuid,
    job_id: Uuid,
    trigger: Trigger,
}

#[derive(Debug)]
struct Active {
    run_id: Uuid,
    cancel: CancelToken,
}

/// The scheduler task's state. Never shared: only the owning task touches it.
#[derive(Debug)]
pub struct Scheduler {
    config: Arc<Config>,
    runner: Runner,
    clock: Arc<dyn Clock>,
    zone: Arc<dyn Zone>,
    environment: Arc<dyn Environment>,
    state: Arc<tokio::sync::Mutex<PersistedState>>,
    events: tokio::sync::broadcast::Sender<EngineEvent>,
    cancel: CancelToken,

    queue: VecDeque<Pending>,
    active: HashMap<Uuid, Active>,
    /// job id → the next instant it is due.
    next_fire: BTreeMap<Uuid, DateTime<Utc>>,
    /// The schedule each entry in `next_fire` was derived from, so a config
    /// swap can tell "the user edited this job's schedule" from "the user
    /// renamed it".
    known_schedules: HashMap<Uuid, Schedule>,
    /// Jobs whose one-shot catch-up has already been considered.
    caught_up: HashSet<Uuid>,
    /// Live filesystem watchers, keyed by job.
    watchers: HashMap<Uuid, JobWatcher>,

    completions: tokio::sync::mpsc::UnboundedSender<Uuid>,
}

impl Scheduler {
    /// Spawn the scheduler task. Use [`crate::engine::EngineBuilder`] rather
    /// than calling this directly unless you are assembling the parts by hand.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        config: Arc<Config>,
        runner: Runner,
        clock: Arc<dyn Clock>,
        zone: Arc<dyn Zone>,
        environment: Arc<dyn Environment>,
        state: Arc<tokio::sync::Mutex<PersistedState>>,
        events: tokio::sync::broadcast::Sender<EngineEvent>,
    ) -> SchedulerHandle {
        let (commands, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (completions, completion_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = CancelToken::new();
        let handle = SchedulerHandle {
            commands: commands.clone(),
            events: events.clone(),
            cancel: cancel.clone(),
        };

        let scheduler = Scheduler {
            config,
            runner,
            clock,
            zone,
            environment,
            state,
            events,
            cancel,
            queue: VecDeque::new(),
            active: HashMap::new(),
            next_fire: BTreeMap::new(),
            known_schedules: HashMap::new(),
            caught_up: HashSet::new(),
            watchers: HashMap::new(),
            completions,
        };
        tokio::spawn(scheduler.run(command_rx, completion_rx, commands));
        handle
    }

    /// The main loop.
    async fn run(
        mut self,
        mut commands: tokio::sync::mpsc::UnboundedReceiver<Command>,
        mut completions: tokio::sync::mpsc::UnboundedReceiver<Uuid>,
        command_sender: tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        self.resync(&command_sender).await;
        loop {
            self.pump(self.clock.now_utc());
            let deadline = self.next_deadline();

            tokio::select! {
                // Biased so that commands and completions are always handled
                // before a timer that is due in the same poll. A Stop must not
                // wait behind a scheduled start.
                biased;
                _ = self.cancel.cancelled() => break,
                Some(command) = commands.recv() => {
                    if self.handle_command(command, &command_sender).await {
                        break;
                    }
                }
                Some(job_id) = completions.recv() => {
                    self.active.remove(&job_id);
                }
                _ = self.clock.sleep_until(deadline) => {
                    self.enqueue_due(self.clock.now_utc());
                }
            }
        }
        self.shutdown_runs();
    }

    /// Cancel every in-flight run. Called once, on the way out.
    fn shutdown_runs(&mut self) {
        for (_, active) in self.active.drain() {
            active.cancel.cancel(CancelReason::Shutdown);
        }
        self.watchers.clear();
    }

    /// Returns true when the loop should end.
    async fn handle_command(
        &mut self,
        command: Command,
        command_sender: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) -> bool {
        match command {
            Command::Shutdown => return true,
            Command::Cancel { job } => {
                if let Some(active) = self.active.get(&job) {
                    active.cancel.cancel(CancelReason::Requested);
                } else {
                    // Not running: drop it from the queue instead, so a Stop
                    // pressed while a job waits behind another actually stops
                    // it rather than letting it start a minute later.
                    self.queue.retain(|p| p.job_id != job);
                }
            }
            Command::Run { job, trigger, reply } => {
                let result = self.enqueue(job, trigger);
                let _ = reply.send(result);
            }
            Command::ReplaceConfig { config } => {
                self.config = config;
                self.resync(command_sender).await;
            }
            Command::Status { reply } => {
                let _ = reply.send(self.status());
            }
            Command::FileChange { job, trigger } => {
                let job_trigger = match trigger {
                    WatchTrigger::Debounced | WatchTrigger::Rescan => Trigger::FileChange,
                };
                if let Err(e) = self.enqueue(job, job_trigger) {
                    tracing::debug!("file-change trigger ignored: {e}");
                }
            }
        }
        false
    }

    /// Recompute schedules and watchers after a config change.
    ///
    /// Everything in-flight is untouched: `active` is keyed by job id and the
    /// runs themselves hold `Arc` snapshots taken when they started.
    async fn resync(&mut self, command_sender: &tokio::sync::mpsc::UnboundedSender<Command>) {
        let now = self.clock.now_utc();
        let present: HashSet<Uuid> = self.config.jobs.iter().map(|j| j.id).collect();
        self.next_fire.retain(|id, _| present.contains(id));
        self.known_schedules.retain(|id, _| present.contains(id));
        self.watchers.retain(|id, _| present.contains(id));
        // A run already queued for a job that has been deleted would panic the
        // "resolve the job" step later; drop it now.
        self.queue.retain(|p| present.contains(&p.job_id));

        // `last_run` for every job, read in one pass so the state lock is
        // taken once and never while anything else is awaited.
        let last_runs: BTreeMap<Uuid, Option<DateTime<Utc>>> = {
            let state = self.state.lock().await;
            self.config
                .jobs
                .iter()
                .map(|j| (j.id, state.jobs.get(&j.id).and_then(|s| s.last_run)))
                .collect()
        };

        let jobs: Vec<Job> = self.config.jobs.clone();
        for job in &jobs {
            match &job.schedule {
                Schedule::OnChange { debounce_seconds, min_interval_minutes } => {
                    self.next_fire.remove(&job.id);
                    let changed = self.known_schedules.get(&job.id) != Some(&job.schedule);
                    if changed || !self.watchers.contains_key(&job.id) {
                        // Rebuilding the watcher is the only way to pick up an
                        // edited debounce, source list or exclusion set.
                        let watcher = JobWatcher::spawn(
                            job.clone(),
                            *debounce_seconds,
                            *min_interval_minutes,
                            Arc::clone(&self.clock),
                            command_sender.clone(),
                            self.cancel.child(),
                            |job_id, trigger| Command::FileChange { job: job_id, trigger },
                        );
                        self.watchers.insert(job.id, watcher);
                    }
                }
                schedule if schedule.is_automatic() => {
                    self.watchers.remove(&job.id);
                    let changed = self.known_schedules.get(&job.id) != Some(schedule);
                    if changed || !self.next_fire.contains_key(&job.id) {
                        if self.config.settings.run_missed_on_start && self.caught_up.insert(job.id)
                        {
                            let last_run = last_runs.get(&job.id).copied().flatten();
                            if self.zone.catch_up(schedule, last_run, now).is_some() {
                                // Exactly one catch-up run, no matter how many
                                // occurrences elapsed while the machine was
                                // off. See `schedule::catch_up_due`.
                                let _ = self.enqueue(job.id, Trigger::CatchUp);
                            }
                        }
                        let next = self.zone.next_occurrence(schedule, now);
                        match next {
                            Some(at) => {
                                self.next_fire.insert(job.id, at);
                            }
                            None => {
                                self.next_fire.remove(&job.id);
                            }
                        }
                        let _ = self
                            .events
                            .send(EngineEvent::NextRunChanged { job_id: job.id, next_run: next });
                    }
                }
                _ => {
                    // Manual, or a schedule that cannot be evaluated.
                    self.next_fire.remove(&job.id);
                    self.watchers.remove(&job.id);
                }
            }
            self.known_schedules.insert(job.id, job.schedule.clone());
        }
    }

    /// The instant the loop should next wake.
    fn next_deadline(&self) -> DateTime<Utc> {
        let now = self.clock.now_utc();
        let capped = now + Duration::seconds(MAX_SLEEP_SECONDS);
        match self.next_fire.values().min() {
            Some(at) if *at < capped => *at,
            _ => capped,
        }
    }

    /// Queue every job whose moment has arrived, and re-arm it.
    fn enqueue_due(&mut self, now: DateTime<Utc>) {
        let due: Vec<Uuid> =
            self.next_fire.iter().filter(|(_, at)| **at <= now).map(|(id, _)| *id).collect();
        for job_id in due {
            let schedule = self.config.job(&job_id).map(|j| j.schedule.clone());
            let _ = self.enqueue(job_id, Trigger::Schedule);
            // Re-arm from `now`, not from the moment that just fired: a
            // machine that woke up hours late must not then replay every
            // occurrence it slept through one loop iteration at a time.
            let next = schedule.as_ref().and_then(|s| self.zone.next_occurrence(s, now));
            match next {
                Some(at) => {
                    self.next_fire.insert(job_id, at);
                }
                None => {
                    self.next_fire.remove(&job_id);
                }
            }
            let _ = self.events.send(EngineEvent::NextRunChanged { job_id, next_run: next });
        }
    }

    /// Accept a run into the queue.
    fn enqueue(&mut self, job_id: Uuid, trigger: Trigger) -> Result<Uuid> {
        let Some(job) = self.config.job(&job_id) else {
            return Err(Error::JobNotFound(job_id.to_string()));
        };
        if self.active.contains_key(&job_id) || self.queue.iter().any(|p| p.job_id == job_id) {
            // Refused before a run id existed, so there is nothing to retire.
            self.emit_skip_for(None, job, SkipReason::AlreadyRunning);
            return Err(Error::JobRunning(job.name.clone()));
        }
        let run_id = Uuid::new_v4();
        let job_name = job.name.clone();
        self.queue.push_back(Pending { run_id, job_id, trigger });
        let _ = self.events.send(EngineEvent::RunQueued { run_id, job_id, job_name });
        Ok(run_id)
    }

    /// Start as many queued runs as the parallelism limit allows.
    ///
    /// Skipped runs are removed from the queue and reported; they do not
    /// consume a slot, so a paused machine drains its queue immediately rather
    /// than holding runs that will never start.
    fn pump(&mut self, now: DateTime<Utc>) {
        let max_parallel = self.config.settings.max_parallel_jobs.max(1) as usize;
        while self.active.len() < max_parallel {
            let Some(pending) = self.queue.pop_front() else { break };
            let Some(job) = self.config.job(&pending.job_id).cloned() else { continue };
            let destinations = self.resolve_destinations(&job);
            let skip = evaluate_gates(
                &job,
                &self.config.settings,
                self.environment.as_ref(),
                self.active.contains_key(&job.id),
                destinations.len(),
                pending.trigger,
                now,
            );
            match skip {
                // The queued run's id travels with the skip. Without it the
                // daemon cannot retire the active entry `RunQueued` created,
                // and every locked-vault tick left one behind for ever.
                Some(reason) => self.emit_skip_for(Some(pending.run_id), &job, reason),
                None => self.start(pending, job, destinations),
            }
        }
    }

    /// The destinations a job can actually be written to right now.
    fn resolve_destinations(&self, job: &Job) -> Vec<Arc<Destination>> {
        job.destination_ids
            .iter()
            .filter_map(|id| self.config.destination(id))
            .filter(|d| destination_is_usable(d))
            .map(|d| Arc::new(d.clone()))
            .collect()
    }

    /// Spawn the run.
    fn start(&mut self, pending: Pending, job: Job, destinations: Vec<Arc<Destination>>) {
        // A child of the engine token: shutting the engine down stops this
        // run, stopping this run does not touch the engine or its siblings.
        let cancel = self.cancel.child();
        self.active.insert(job.id, Active { run_id: pending.run_id, cancel: cancel.clone() });

        let request = RunRequest {
            // The scheduler never rehearses; a dry run is always explicit.
            dry_run: false,
            run_id: pending.run_id,
            job: Arc::new(job),
            destinations,
            settings: Arc::new(self.config.settings.clone()),
            trigger: pending.trigger,
            cancel,
        };
        let runner = self.runner.clone();
        let completions = self.completions.clone();
        tokio::spawn(async move {
            let job_id = request.job.id;
            let _ = runner.execute(request).await;
            // Freeing the slot is the last thing that happens, and it happens
            // on every path including a panic-free early return, because the
            // runner never returns an error.
            let _ = completions.send(job_id);
        });
    }

    fn emit_skip_for(&self, run_id: Option<Uuid>, job: &Job, reason: SkipReason) {
        let _ = self.events.send(EngineEvent::RunSkipped {
            run_id,
            job_id: job.id,
            job_name: job.name.clone(),
            reason,
        });
        let event = Event::new(
            Severity::Info,
            "job.skipped",
            format!("{} skipped: {}", job.name, reason.describe()),
        )
        .with_job(job.id)
        .with_field("reason", reason.title());
        tracing::info!(job = %job.name, reason = reason.title(), "scheduled run skipped");
        let _ = self.events.send(EngineEvent::Log(Box::new(event)));
    }

    fn status(&self) -> SchedulerStatus {
        let next_scheduled =
            self.next_fire.iter().min_by_key(|(_, at)| **at).map(|(id, at)| (*id, *at));
        SchedulerStatus {
            running: self.active.iter().map(|(job, a)| (*job, a.run_id)).collect(),
            queued: self.queue.iter().map(|p| p.job_id).collect(),
            next_runs: self.next_fire.clone(),
            next_scheduled,
            max_parallel: self.config.settings.max_parallel_jobs.max(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::StaticEnvironment;
    use crate::model::{PauseState, Source};

    fn job(enabled: bool) -> Job {
        Job {
            id: Uuid::new_v4(),
            name: "test".into(),
            project_id: None,
            description: String::new(),
            sources: vec![Source::new("/tmp/x")],
            destination_ids: vec![Uuid::new_v4()],
            schedule: Schedule::Manual,
            exclusions: Default::default(),
            bandwidth: None,
            retention: None,
            enabled,
            timeout_minutes: None,
            hooks: Default::default(),
            continue_on_destination_error: true,
            created_at: Utc::now(),
            tags: vec![],
        }
    }

    fn now() -> DateTime<Utc> {
        "2025-01-08T12:00:00Z".parse().expect("literal")
    }

    #[test]
    fn happy_path_has_no_gate() {
        let env = StaticEnvironment::unlocked();
        let settings = Settings::default();
        assert_eq!(
            evaluate_gates(&job(true), &settings, &env, false, 1, Trigger::Schedule, now()),
            None
        );
    }

    #[test]
    fn every_gate_reports_its_own_reason() {
        let settings = Settings { skip_on_battery: true, ..Settings::default() };
        let env = StaticEnvironment::unlocked();

        assert_eq!(
            evaluate_gates(&job(true), &settings, &env, true, 1, Trigger::Schedule, now()),
            Some(SkipReason::AlreadyRunning)
        );
        assert_eq!(
            evaluate_gates(&job(true), &settings, &env, false, 0, Trigger::Schedule, now()),
            Some(SkipReason::NoUsableDestination)
        );
        assert_eq!(
            evaluate_gates(&job(false), &settings, &env, false, 1, Trigger::Schedule, now()),
            Some(SkipReason::JobDisabled)
        );

        let paused = Settings {
            pause: PauseState { paused: true, until: None, reason: None },
            ..settings.clone()
        };
        assert_eq!(
            evaluate_gates(&job(true), &paused, &env, false, 1, Trigger::Schedule, now()),
            Some(SkipReason::GloballyPaused)
        );

        env.set_metered(true);
        assert_eq!(
            evaluate_gates(&job(true), &settings, &env, false, 1, Trigger::Schedule, now()),
            Some(SkipReason::MeteredConnection)
        );
        env.set_metered(false);

        env.set_on_battery(true);
        assert_eq!(
            evaluate_gates(&job(true), &settings, &env, false, 1, Trigger::Schedule, now()),
            Some(SkipReason::OnBattery)
        );
        env.set_on_battery(false);

        env.set_vault_unlocked(false);
        assert_eq!(
            evaluate_gates(&job(true), &settings, &env, false, 1, Trigger::Schedule, now()),
            Some(SkipReason::VaultLocked)
        );
    }

    #[test]
    fn an_expired_pause_does_not_gate() {
        let settings = Settings {
            pause: PauseState {
                paused: true,
                until: Some(now() - Duration::hours(1)),
                reason: None,
            },
            ..Settings::default()
        };
        let env = StaticEnvironment::unlocked();
        assert_eq!(
            evaluate_gates(&job(true), &settings, &env, false, 1, Trigger::Schedule, now()),
            None
        );
    }

    #[test]
    fn manual_runs_bypass_policy_gates_but_not_physical_ones() {
        let settings = Settings {
            skip_on_battery: true,
            pause: PauseState { paused: true, until: None, reason: None },
            ..Settings::default()
        };
        let env = StaticEnvironment::unlocked();
        env.set_metered(true);
        env.set_on_battery(true);
        // Paused, metered, on battery, and the job is switched off: a human
        // pressing Run Now still gets their backup.
        assert_eq!(
            evaluate_gates(&job(false), &settings, &env, false, 1, Trigger::Manual, now()),
            None
        );
        assert_eq!(
            evaluate_gates(&job(false), &settings, &env, false, 1, Trigger::Cli, now()),
            None
        );
        // But not while it is already running, and not with the vault locked.
        assert_eq!(
            evaluate_gates(&job(true), &settings, &env, true, 1, Trigger::Manual, now()),
            Some(SkipReason::AlreadyRunning)
        );
        env.set_vault_unlocked(false);
        assert_eq!(
            evaluate_gates(&job(true), &settings, &env, false, 1, Trigger::Manual, now()),
            Some(SkipReason::VaultLocked)
        );
    }

    #[test]
    fn skip_reasons_all_have_user_facing_text() {
        for reason in [
            SkipReason::GloballyPaused,
            SkipReason::JobDisabled,
            SkipReason::VaultLocked,
            SkipReason::MeteredConnection,
            SkipReason::OnBattery,
            SkipReason::AlreadyRunning,
            SkipReason::NoUsableDestination,
        ] {
            assert!(!reason.title().is_empty());
            assert!(reason.describe().len() > 20, "{reason:?} needs a real explanation");
            assert_eq!(reason.status(), RunStatus::Skipped);
        }
    }
}
