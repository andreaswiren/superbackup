//! The scheduling and job-execution engine.
//!
//! ```text
//!            config (model::Config, replaceable at any time)
//!                     │
//!   clock ──▶  scheduler ──▶ queue ──▶ runner ──▶ executor  (kopia)
//!   zone   ─▶     │                       │    └▶ mirror    (plain copy)
//!   env    ─▶     │                       │
//!               watcher                   ├▶ throttle
//!            (Schedule::OnChange)         ├▶ hooks
//!                                         └▶ state + events
//! ```
//!
//! ## What each piece owns
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`schedule`] | Pure schedule arithmetic: next/previous occurrence, DST, catch-up, human descriptions |
//! | [`scheduler`] | The timing wheel, the skip gates, the parallelism limit, the queue |
//! | [`runner`] | One job across all its destinations: retries, timeout, hooks, progress, persistence |
//! | [`mirror`] | `LocalMirror` destinations, which have no kopia |
//! | [`throttle`] | Effective bandwidth, and a real token bucket for the mirror |
//! | [`watcher`] | `Schedule::OnChange`: debounced filesystem watching |
//! | [`executor`] | The [`BackupExecutor`] seam the kopia driver implements |
//! | [`clock`], [`cancel`], [`hooks`], [`retry`], [`tz`] | Injected time, cooperative cancellation, user commands, backoff policy, DST rule sets |
//!
//! ## Injected dependencies
//!
//! Four things are injected rather than reached for, and all four exist so the
//! engine can be tested without a machine, a network, or a wall clock:
//!
//! * [`clock::Clock`] — time and sleeping.
//! * [`schedule::Zone`] — the local timezone, including its DST rules.
//! * [`Environment`] — vault, power and network state, which really live in
//!   `crate::platform` and `crate::crypto` but must not be depended on here.
//! * [`BackupExecutor`] — the thing that moves bytes.
//!
//! ## Concurrency invariants
//!
//! 1. **One run per job.** The scheduler holds the authoritative
//!    job-id → running-run map; nothing else starts a run.
//! 2. **Cancellation is a tree.** The engine token is the root, each run holds
//!    a child. Stopping a run never touches a sibling; shutting the engine
//!    down stops every run.
//! 3. **No lock is held across an `await`.** Shared state is guarded by
//!    `std::sync::Mutex` where the critical section is synchronous (progress
//!    coalescing, token bucket) and by `tokio::sync::Mutex` only where the
//!    critical section is genuinely async-adjacent (the persisted state,
//!    written once per run).
//! 4. **Config may be replaced at any moment.** In-flight runs hold `Arc`
//!    snapshots of their job and destinations, so a config swap can never
//!    change what a running job is doing halfway through.
//! 5. **Events are lossy by design.** [`EngineEvent`] goes out over a
//!    `broadcast` channel; a slow subscriber lags and misses frames rather
//!    than blocking a backup. Durable truth lives in
//!    [`crate::state::PersistedState`].

pub mod cancel;
pub mod clock;
pub mod executor;
pub mod hooks;
pub mod mirror;
pub mod retry;
pub mod runner;
pub mod schedule;
pub mod scheduler;
pub mod testing;
pub mod throttle;
pub mod tz;
pub mod watcher;

pub use cancel::{CancelReason, CancelToken};
pub use clock::{Clock, SystemClock, TestClock};
pub use executor::{
    BackupExecutor, ExecutorError, ExecutorResult, PrepareOutcome, PrepareRequest, ProgressSink,
    ProgressUpdate, ReplicateOutcome, ReplicateRequest, Retryable, SnapshotOutcome,
    SnapshotRequest, VerifyOutcome, VerifyRequest,
};
pub use mirror::{MirrorEngine, MirrorOptions, MirrorRequest};
pub use retry::RetryPolicy;
pub use runner::{plan_destinations, PlannedDestination, RunRequest, Runner};
pub use schedule::{catch_up_due, describe, next_occurrence, next_occurrence_in, Zone};
pub use scheduler::{evaluate_gates, Scheduler, SchedulerHandle, SchedulerStatus, SkipReason};
pub use throttle::{resolve_bandwidth, BandwidthSource, ResolvedBandwidth, TokenBucket};
pub use watcher::{ChangeDebouncer, WatchTrigger};

use crate::model::Config;
use crate::state::{DestinationRun, Event, JobRun, PersistedState};
use std::sync::Arc;
use uuid::Uuid;

/// Capacity of the engine's event broadcast.
///
/// Sized for a GUI that redraws at 60 Hz and a coalescer that emits at 10 Hz
/// per destination: a subscriber that stalls for several seconds still catches
/// up without lagging. A subscriber that stalls for longer *should* lag — the
/// alternative is throttling the backup to the speed of the slowest window.
pub const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Everything the engine broadcasts. Subscribers are the tray, the GUI, the
/// IPC layer, and the notifier.
///
/// Large payloads are boxed so that the enum stays small enough to be cheap to
/// clone for every subscriber on every frame.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// A run was accepted and is waiting for a slot.
    RunQueued { run_id: Uuid, job_id: Uuid, job_name: String },
    /// A run started executing.
    RunStarted { run: JobRun },
    /// A destination began.
    DestinationStarted { run_id: Uuid, job_id: Uuid, destination_id: Uuid },
    /// A destination failed transiently and will be retried.
    DestinationRetrying {
        run_id: Uuid,
        job_id: Uuid,
        destination_id: Uuid,
        attempt: u32,
        retry_in_seconds: i64,
    },
    /// A destination reached a terminal state.
    DestinationFinished { run_id: Uuid, job_id: Uuid, destination: Box<DestinationRun> },
    /// Coalesced live progress. See [`ProgressSink`].
    Progress(ProgressUpdate),
    /// A hook finished, successfully or otherwise.
    HookFinished { run_id: Uuid, job_id: Uuid, outcome: Box<hooks::HookOutcome> },
    /// A run reached a terminal state and has been recorded.
    RunFinished { run: JobRun },
    /// A scheduled run did not start, and why.
    RunSkipped { job_id: Uuid, job_name: String, reason: SkipReason },
    /// The scheduler recomputed when it will next wake.
    NextRunChanged { job_id: Uuid, next_run: Option<chrono::DateTime<chrono::Utc>> },
    /// An activity-log line.
    Log(Box<Event>),
}

/// Machine state the engine must consult but must not implement.
///
/// The real answers live in `crate::platform` (power and network) and
/// `crate::crypto` (the vault), both of which are owned by other workstreams.
/// Depending on this trait instead keeps the engine buildable and testable on
/// its own, and keeps "is the vault unlocked" a single question with a single
/// answer rather than a flag copied into three places.
///
/// Implementations must be cheap and non-blocking: these are called on the
/// scheduler's hot path, once per gate evaluation.
pub trait Environment: std::fmt::Debug + Send + Sync + 'static {
    /// False blocks every scheduled run: without the vault, no destination
    /// passphrase can be resolved.
    fn vault_unlocked(&self) -> bool;
    /// True when the active network connection is metered (tethering, a
    /// capped mobile broadband dongle).
    fn on_metered_connection(&self) -> bool;
    /// True when running on battery rather than mains.
    fn on_battery(&self) -> bool;
}

/// An [`Environment`] whose answers are set programmatically.
///
/// Used by the tests, by `--dry-run`, and as the default before the platform
/// layer is wired in. Values are plain atomics so a caller can flip one from
/// any thread without taking a lock on the scheduler's hot path.
#[derive(Debug)]
pub struct StaticEnvironment {
    unlocked: std::sync::atomic::AtomicBool,
    metered: std::sync::atomic::AtomicBool,
    battery: std::sync::atomic::AtomicBool,
}

impl Default for StaticEnvironment {
    fn default() -> Self {
        StaticEnvironment::unlocked()
    }
}

impl StaticEnvironment {
    /// Vault unlocked, mains power, unmetered connection — the happy path.
    pub fn unlocked() -> StaticEnvironment {
        use std::sync::atomic::AtomicBool;
        StaticEnvironment {
            unlocked: AtomicBool::new(true),
            metered: AtomicBool::new(false),
            battery: AtomicBool::new(false),
        }
    }

    pub fn set_vault_unlocked(&self, value: bool) {
        self.unlocked.store(value, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_metered(&self, value: bool) {
        self.metered.store(value, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_on_battery(&self, value: bool) {
        self.battery.store(value, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Environment for StaticEnvironment {
    fn vault_unlocked(&self) -> bool {
        self.unlocked.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn on_metered_connection(&self) -> bool {
        self.metered.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn on_battery(&self) -> bool {
        self.battery.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Assembles a running engine.
///
/// Everything has a default except the executor and the config, so wiring the
/// real daemon is a two-line call and a test fixture is a four-line one.
#[derive(Debug)]
pub struct EngineBuilder {
    config: Arc<Config>,
    executor: Arc<dyn BackupExecutor>,
    clock: Arc<dyn Clock>,
    zone: Arc<dyn Zone>,
    environment: Arc<dyn Environment>,
    state: Arc<tokio::sync::Mutex<PersistedState>>,
    retry: RetryPolicy,
    hooks: Option<hooks::HookRunner>,
}

impl EngineBuilder {
    /// Start from a config and an executor, with production defaults: the
    /// system clock, the machine's local timezone, and an environment that
    /// reports "unlocked, mains, unmetered" until the platform layer replaces
    /// it.
    pub fn new(config: Arc<Config>, executor: Arc<dyn BackupExecutor>) -> EngineBuilder {
        EngineBuilder {
            config,
            executor,
            clock: Arc::new(SystemClock::new()),
            zone: Arc::new(chrono::Local),
            environment: Arc::new(StaticEnvironment::unlocked()),
            state: Arc::new(tokio::sync::Mutex::new(PersistedState::default())),
            retry: RetryPolicy::default(),
            hooks: None,
        }
    }

    pub fn clock(mut self, clock: Arc<dyn Clock>) -> EngineBuilder {
        self.clock = clock;
        self
    }

    /// Override the timezone schedules are evaluated in. Production leaves
    /// this at [`chrono::Local`]; tests pass [`tz::DstZone`].
    pub fn zone(mut self, zone: Arc<dyn Zone>) -> EngineBuilder {
        self.zone = zone;
        self
    }

    pub fn environment(mut self, environment: Arc<dyn Environment>) -> EngineBuilder {
        self.environment = environment;
        self
    }

    /// Continue from previously loaded state, so that "when did this job last
    /// run" survives a restart and catch-up works on the first tick.
    pub fn state(mut self, state: Arc<tokio::sync::Mutex<PersistedState>>) -> EngineBuilder {
        self.state = state;
        self
    }

    pub fn retry_policy(mut self, retry: RetryPolicy) -> EngineBuilder {
        self.retry = retry;
        self
    }

    pub fn hook_runner(mut self, hooks: hooks::HookRunner) -> EngineBuilder {
        self.hooks = Some(hooks);
        self
    }

    /// Spawn the scheduler task and return a handle to it.
    ///
    /// The returned [`scheduler::SchedulerHandle`] is the only way to talk to
    /// a running engine: it is cheap to clone, safe to hold across the whole
    /// process lifetime, and shuts the engine down when
    /// [`scheduler::SchedulerHandle::shutdown`] is called.
    pub fn spawn(self) -> scheduler::SchedulerHandle {
        let (events, _) = tokio::sync::broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let mut runner = Runner::new(
            Arc::clone(&self.executor),
            Arc::clone(&self.clock),
            Arc::clone(&self.zone),
            events.clone(),
            Arc::clone(&self.state),
        )
        .with_retry_policy(self.retry);
        if let Some(hooks) = self.hooks {
            runner = runner.with_hooks(hooks);
        }
        Scheduler::spawn(
            self.config,
            runner,
            self.clock,
            self.zone,
            self.environment,
            self.state,
            events,
        )
    }
}
