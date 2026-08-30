//! Scheduler behaviour: gates, the parallelism queue, catch-up collapsing,
//! config hot-swap, and per-job cancellation.
//!
//! Every test drives a real spawned scheduler through its public handle, on a
//! [`TestClock`], with a [`MockExecutor`]. Waiting on the event stream is
//! wrapped in a real-time timeout so a regression fails the test instead of
//! hanging CI.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use superbackup_core::engine::clock::{Clock, TestClock};
use superbackup_core::engine::testing::{test_job, test_repository, MockBehaviour, MockExecutor};
use superbackup_core::engine::tz::DstZone;
use superbackup_core::engine::{
    EngineBuilder, EngineEvent, SchedulerHandle, SchedulerStatus, SkipReason, StaticEnvironment,
};
use superbackup_core::model::{Config, Destination, Job, PauseState, Schedule, TimeOfDay};
use superbackup_core::state::{JobSummary, PersistedState, RunStatus, Trigger};
use uuid::Uuid;

/// Real-time budget for "the engine should have done this by now". Generous
/// enough for a loaded CI box, short enough that a deadlock is a failure
/// rather than a six-hour job.
const PATIENCE: StdDuration = StdDuration::from_secs(10);

struct Harness {
    handle: SchedulerHandle,
    clock: Arc<TestClock>,
    executor: Arc<MockExecutor>,
    environment: Arc<StaticEnvironment>,
    state: Arc<tokio::sync::Mutex<PersistedState>>,
}

fn build(config: Config, state: PersistedState) -> Harness {
    let clock = Arc::new(TestClock::at("2025-01-08T12:00:00Z"));
    let executor = Arc::new(MockExecutor::new());
    let environment = Arc::new(StaticEnvironment::unlocked());
    let state = Arc::new(tokio::sync::Mutex::new(state));
    let handle = EngineBuilder::new(Arc::new(config), executor.clone())
        .clock(clock.clone())
        .zone(Arc::new(DstZone::EuropeStockholm))
        .environment(environment.clone())
        .state(state.clone())
        .spawn();
    Harness { handle, clock, executor, environment, state }
}

fn config_with(jobs: Vec<Job>, destinations: Vec<Destination>) -> Config {
    Config { jobs, destinations, ..Config::default() }
}

fn job_with(name: &str, destination: &Destination, schedule: Schedule) -> Job {
    let mut job = test_job(name);
    job.destination_ids = vec![destination.id];
    job.schedule = schedule;
    job
}

/// Wait for the first event matching `predicate`, or fail.
async fn wait_for<T>(
    events: &mut tokio::sync::broadcast::Receiver<EngineEvent>,
    label: &str,
    mut predicate: impl FnMut(&EngineEvent) -> Option<T>,
) -> T {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for {label}");
        match tokio::time::timeout(remaining, events.recv()).await {
            Ok(Ok(event)) => {
                if let Some(value) = predicate(&event) {
                    return value;
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => panic!("the event stream closed while waiting for {label}"),
            Err(_) => panic!("timed out waiting for {label}"),
        }
    }
}

/// Poll the scheduler's status until `predicate` is satisfied, or fail.
///
/// Bounded by [`PATIENCE`] so a scheduler that never reaches the expected
/// state fails the test instead of spinning forever.
async fn poll_status<T>(
    handle: &SchedulerHandle,
    label: &str,
    mut predicate: impl FnMut(&SchedulerStatus) -> Option<T>,
) -> T {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let status = handle.status().await.expect("the scheduler answers");
        if let Some(value) = predicate(&status) {
            return value;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {label}");
        tokio::time::sleep(StdDuration::from_millis(1)).await;
    }
}

/// Let the engine run to quiescence, for tests asserting that nothing happens.
async fn settle() {
    tokio::time::sleep(StdDuration::from_millis(50)).await;
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
}

fn finished_run(event: &EngineEvent) -> Option<(Uuid, RunStatus, Trigger)> {
    match event {
        EngineEvent::RunFinished { run } => Some((run.job_id, run.status, run.trigger)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_paused_engine_skips_scheduled_runs_with_a_reason() {
    let destination = test_repository("local", "/repos/local");
    // Interval schedules fire on the UTC grid, so this is due within a minute.
    let job = job_with("nightly", &destination, Schedule::Interval { minutes: 1 });
    let job_id = job.id;
    let mut config = config_with(vec![job], vec![destination]);
    config.settings.pause = PauseState { paused: true, until: None, reason: None };

    let h = build(config, PersistedState::default());
    let mut events = h.handle.subscribe();
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance_minutes(2);

    let reason = wait_for(&mut events, "a skip", |e| match e {
        EngineEvent::RunSkipped { job_id: id, reason, .. } if *id == job_id => Some(*reason),
        _ => None,
    })
    .await;
    assert_eq!(reason, SkipReason::GloballyPaused);
    assert_eq!(h.executor.calls().len(), 0, "nothing may reach the driver while paused");
}

#[tokio::test]
async fn a_locked_vault_skips_scheduled_runs() {
    let destination = test_repository("local", "/repos/local");
    let job = job_with("nightly", &destination, Schedule::Interval { minutes: 1 });
    let job_id = job.id;
    let h = build(config_with(vec![job], vec![destination]), PersistedState::default());
    h.environment.set_vault_unlocked(false);

    let mut events = h.handle.subscribe();
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance_minutes(2);

    let reason = wait_for(&mut events, "a skip", |e| match e {
        EngineEvent::RunSkipped { job_id: id, reason, .. } if *id == job_id => Some(*reason),
        _ => None,
    })
    .await;
    assert_eq!(reason, SkipReason::VaultLocked);
}

#[tokio::test]
async fn a_metered_connection_skips_scheduled_runs_but_not_manual_ones() {
    let destination = test_repository("local", "/repos/local");
    let job = job_with("nightly", &destination, Schedule::Interval { minutes: 1 });
    let job_id = job.id;
    let h = build(config_with(vec![job], vec![destination]), PersistedState::default());
    h.environment.set_metered(true);

    let mut events = h.handle.subscribe();
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance_minutes(2);
    let reason = wait_for(&mut events, "a skip", |e| match e {
        EngineEvent::RunSkipped { job_id: id, reason, .. } if *id == job_id => Some(*reason),
        _ => None,
    })
    .await;
    assert_eq!(reason, SkipReason::MeteredConnection);

    // The same job, started by a human, runs anyway.
    h.handle.run_now(job_id, Trigger::Manual).await.expect("accepted");
    let (_, status, trigger) = wait_for(&mut events, "the manual run", |e| {
        finished_run(e).filter(|(id, _, _)| *id == job_id)
    })
    .await;
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(trigger, Trigger::Manual);
}

#[tokio::test]
async fn a_disabled_job_is_skipped_on_a_schedule() {
    let destination = test_repository("local", "/repos/local");
    let mut job = job_with("nightly", &destination, Schedule::Interval { minutes: 1 });
    job.enabled = false;
    let job_id = job.id;
    let h = build(config_with(vec![job], vec![destination]), PersistedState::default());

    let mut events = h.handle.subscribe();
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance_minutes(2);
    let reason = wait_for(&mut events, "a skip", |e| match e {
        EngineEvent::RunSkipped { job_id: id, reason, .. } if *id == job_id => Some(*reason),
        _ => None,
    })
    .await;
    assert_eq!(reason, SkipReason::JobDisabled);
}

#[tokio::test]
async fn a_job_with_no_usable_destination_is_skipped() {
    // The destination the job points at is not in the config at all.
    let mut job = test_job("orphan");
    job.destination_ids = vec![Uuid::new_v4()];
    let job_id = job.id;
    let h = build(config_with(vec![job], vec![]), PersistedState::default());

    let mut events = h.handle.subscribe();
    let _ = h.handle.run_now(job_id, Trigger::Manual).await;
    let reason = wait_for(&mut events, "a skip", |e| match e {
        EngineEvent::RunSkipped { job_id: id, reason, .. } if *id == job_id => Some(*reason),
        _ => None,
    })
    .await;
    assert_eq!(reason, SkipReason::NoUsableDestination);
}

#[tokio::test]
async fn a_job_cannot_be_queued_twice() {
    let destination = test_repository("slow", "/repos/slow");
    let job = job_with("slow", &destination, Schedule::Manual);
    let job_id = job.id;
    let h = build(config_with(vec![job], vec![destination.clone()]), PersistedState::default());
    h.executor.set_for(destination.id, MockBehaviour::BlockUntilCancelled);

    h.handle.run_now(job_id, Trigger::Manual).await.expect("first accepted");
    let mut events = h.handle.subscribe();
    let second = h
        .handle
        .run_now(job_id, Trigger::Manual)
        .await
        .expect_err("a second concurrent run must be refused");
    assert!(matches!(second, superbackup_core::Error::JobRunning(_)), "{second}");
    let reason = wait_for(&mut events, "the already-running skip", |e| match e {
        EngineEvent::RunSkipped { reason, .. } => Some(*reason),
        _ => None,
    })
    .await;
    assert_eq!(reason, SkipReason::AlreadyRunning);
    h.handle.shutdown();
}

// ---------------------------------------------------------------------------
// Queueing and parallelism
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_parallel_jobs_queues_rather_than_drops() {
    let destination = test_repository("local", "/repos/local");
    let jobs: Vec<Job> =
        (0..3).map(|i| job_with(&format!("job-{i}"), &destination, Schedule::Manual)).collect();
    let ids: Vec<Uuid> = jobs.iter().map(|j| j.id).collect();
    let mut config = config_with(jobs, vec![destination.clone()]);
    config.settings.max_parallel_jobs = 1;

    let h = build(config, PersistedState::default());
    h.executor.set_default(MockBehaviour::BlockUntilCancelled);

    for id in &ids {
        h.handle.run_now(*id, Trigger::Manual).await.expect("accepted");
    }

    // Exactly one is running; the other two wait their turn.
    let status = poll_status(&h.handle, "one running and two queued", |s| {
        (s.running.len() == 1 && s.queued.len() == 2).then(|| s.clone())
    })
    .await;
    assert_eq!(status.max_parallel, 1);
    assert_eq!(status.queued.len(), 2, "queued runs must not be dropped");

    // Free the slot one run at a time; the queue drains and nothing is lost.
    let mut events = h.handle.subscribe();
    let mut finished = std::collections::HashSet::new();
    while finished.len() < 3 {
        let running =
            poll_status(&h.handle, "a run to be active", |s| s.running.keys().next().copied())
                .await;
        h.handle.cancel_job(running).expect("cancel");
        let (job_id, _, _) = wait_for(&mut events, "each run to finish", finished_run).await;
        finished.insert(job_id);
    }
    assert_eq!(finished.len(), 3, "every queued job eventually ran, none was dropped");
    assert_eq!(finished, ids.iter().copied().collect::<std::collections::HashSet<_>>());
}

#[tokio::test]
async fn cancelling_one_job_leaves_the_others_running() {
    let destination = test_repository("local", "/repos/local");
    let jobs: Vec<Job> =
        (0..2).map(|i| job_with(&format!("job-{i}"), &destination, Schedule::Manual)).collect();
    let ids: Vec<Uuid> = jobs.iter().map(|j| j.id).collect();
    let mut config = config_with(jobs, vec![destination.clone()]);
    config.settings.max_parallel_jobs = 2;

    let h = build(config, PersistedState::default());
    h.executor.set_default(MockBehaviour::BlockUntilCancelled);
    let mut events = h.handle.subscribe();
    for id in &ids {
        h.handle.run_now(*id, Trigger::Manual).await.expect("accepted");
    }
    poll_status(&h.handle, "both runs to be active", |s| (s.running.len() == 2).then_some(()))
        .await;

    h.handle.cancel_job(ids[0]).expect("cancel");
    let (job_id, status, _) = wait_for(&mut events, "the cancelled run", finished_run).await;
    assert_eq!(job_id, ids[0]);
    assert_eq!(status, RunStatus::Cancelled);

    let running = h.handle.status().await.expect("status").running;
    assert!(running.contains_key(&ids[1]), "the sibling must still be running");
    h.handle.shutdown();
}

// ---------------------------------------------------------------------------
// Schedules and catch-up
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_scheduled_job_fires_at_its_moment() {
    let destination = test_repository("local", "/repos/local");
    // 02:00 Europe/Stockholm = 01:00 UTC in January.
    let job = job_with(
        "nightly",
        &destination,
        Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] },
    );
    let job_id = job.id;
    let h = build(config_with(vec![job], vec![destination]), PersistedState::default());
    let mut events = h.handle.subscribe();

    h.clock.wait_for_sleeps(1).await;
    // Clock starts at 2025-01-08T12:00Z; the next 02:00 local is
    // 2025-01-09T01:00Z, thirteen hours away.
    h.clock.advance_hours(13);

    let (id, status, trigger) = wait_for(&mut events, "the scheduled run", finished_run).await;
    assert_eq!(id, job_id);
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(trigger, Trigger::Schedule);
}

#[tokio::test]
async fn a_week_of_downtime_produces_one_catch_up_run_not_one_per_hour() {
    let destination = test_repository("local", "/repos/local");
    let job = job_with("hourly", &destination, Schedule::Interval { minutes: 60 });
    let job_id = job.id;
    let mut config = config_with(vec![job], vec![destination]);
    config.settings.run_missed_on_start = true;

    // The daemon last ran this job a week ago: 168 hourly occurrences elapsed.
    let mut state = PersistedState::default();
    state.jobs.insert(
        job_id,
        JobSummary {
            last_run: Some("2025-01-01T12:00:00Z".parse().expect("literal")),
            total_runs: 1,
            ..JobSummary::default()
        },
    );

    let h = build(config, state);
    let mut events = h.handle.subscribe();

    let (id, status, trigger) = wait_for(&mut events, "the catch-up run", finished_run).await;
    assert_eq!(id, job_id);
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(trigger, Trigger::CatchUp);

    // Let the scheduler settle, then confirm there was exactly one.
    settle().await;
    assert_eq!(
        h.executor.calls().len(),
        1,
        "a week off must not queue 168 runs; it queues exactly one"
    );
    let history = h.state.lock().await.history.len();
    assert_eq!(history, 1);
}

#[tokio::test]
async fn a_job_that_never_ran_gets_no_catch_up_storm_on_first_start() {
    let destination = test_repository("local", "/repos/local");
    let job = job_with("hourly", &destination, Schedule::Interval { minutes: 60 });
    let mut config = config_with(vec![job], vec![destination]);
    config.settings.run_missed_on_start = true;

    let h = build(config, PersistedState::default());
    settle().await;
    assert_eq!(h.executor.calls().len(), 0, "a fresh install must not back up everything at once");
}

#[tokio::test]
async fn catch_up_is_off_when_the_setting_is_off() {
    let destination = test_repository("local", "/repos/local");
    let job = job_with("hourly", &destination, Schedule::Interval { minutes: 60 });
    let job_id = job.id;
    let mut config = config_with(vec![job], vec![destination]);
    config.settings.run_missed_on_start = false;

    let mut state = PersistedState::default();
    state.jobs.insert(
        job_id,
        JobSummary {
            last_run: Some("2025-01-01T12:00:00Z".parse().expect("literal")),
            total_runs: 1,
            ..JobSummary::default()
        },
    );

    let h = build(config, state);
    settle().await;
    assert_eq!(h.executor.calls().len(), 0);
}

// ---------------------------------------------------------------------------
// Config replacement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn replacing_the_config_does_not_disturb_an_in_flight_run() {
    // Two destinations so the two jobs can be scripted independently: one
    // blocks forever, the other completes.
    let blocking = test_repository("blocking", "/repos/blocking");
    let quick = test_repository("quick", "/repos/quick");
    let job = job_with("long", &blocking, Schedule::Manual);
    let job_id = job.id;
    // Two slots, so the newly added job is not merely queued behind the one
    // that is deliberately blocked.
    let mut config = config_with(vec![job.clone()], vec![blocking.clone(), quick.clone()]);
    config.settings.max_parallel_jobs = 2;

    let h = build(config, PersistedState::default());
    h.executor.set_for(blocking.id, MockBehaviour::BlockUntilCancelled);
    let mut events = h.handle.subscribe();
    h.handle.run_now(job_id, Trigger::Manual).await.expect("accepted");
    poll_status(&h.handle, "the run to be active", |s| {
        s.running.contains_key(&job_id).then_some(())
    })
    .await;

    // The user edits the job — renames it, changes its schedule — and adds a
    // second one, all while the first is still running.
    let mut edited = job.clone();
    edited.name = "renamed".into();
    edited.schedule = Schedule::Daily { times: vec![TimeOfDay { hour: 3, minute: 0 }] };
    let extra = job_with("second", &quick, Schedule::Manual);
    let extra_id = extra.id;
    let mut replacement = config_with(vec![edited, extra], vec![blocking, quick]);
    replacement.settings.max_parallel_jobs = 2;
    h.handle.replace_config(Arc::new(replacement)).expect("replaced");

    // The in-flight run is untouched: still running, under its original job.
    let status = h.handle.status().await.expect("status");
    assert!(status.running.contains_key(&job_id), "a config swap must not drop a running job");

    // The new job is immediately schedulable.
    h.handle.run_now(extra_id, Trigger::Manual).await.expect("the new job is known");
    let (_, run_status, _) = wait_for(&mut events, "the new job", |e| {
        finished_run(e).filter(|(id, _, _)| *id == extra_id)
    })
    .await;
    assert_eq!(run_status, RunStatus::Succeeded);

    // The edited job picked up its new schedule without disturbing the run.
    poll_status(&h.handle, "the edited schedule", |s| {
        s.next_runs.contains_key(&job_id).then_some(())
    })
    .await;
    assert!(h.handle.status().await.expect("status").running.contains_key(&job_id));

    // And the original run still ends on its own terms.
    h.handle.cancel_job(job_id).expect("cancel");
    let (_, run_status, _) = wait_for(&mut events, "the original job", |e| {
        finished_run(e).filter(|(id, _, _)| *id == job_id)
    })
    .await;
    assert_eq!(run_status, RunStatus::Cancelled);
}

#[tokio::test]
async fn deleting_a_job_removes_its_schedule() {
    let destination = test_repository("local", "/repos/local");
    let job = job_with(
        "nightly",
        &destination,
        Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] },
    );
    let job_id = job.id;
    let h = build(config_with(vec![job], vec![destination.clone()]), PersistedState::default());
    poll_status(&h.handle, "the job to be scheduled", |s| {
        s.next_runs.contains_key(&job_id).then_some(())
    })
    .await;

    h.handle.replace_config(Arc::new(config_with(vec![], vec![destination]))).expect("replaced");
    poll_status(&h.handle, "the schedule to be dropped", |s| s.next_runs.is_empty().then_some(()))
        .await;
    assert!(h.handle.status().await.expect("status").next_scheduled.is_none());
}

#[tokio::test]
async fn shutdown_cancels_in_flight_runs() {
    let destination = test_repository("local", "/repos/local");
    let job = job_with("long", &destination, Schedule::Manual);
    let job_id = job.id;
    let h = build(config_with(vec![job], vec![destination]), PersistedState::default());
    h.executor.set_default(MockBehaviour::BlockUntilCancelled);

    let mut events = h.handle.subscribe();
    h.handle.run_now(job_id, Trigger::Manual).await.expect("accepted");
    poll_status(&h.handle, "the run to be active", |s| {
        s.running.contains_key(&job_id).then_some(())
    })
    .await;

    h.handle.shutdown();
    let (id, status, _) = wait_for(&mut events, "the run to stop", finished_run).await;
    assert_eq!(id, job_id);
    assert_eq!(status, RunStatus::Cancelled);
    assert_eq!(
        h.clock.now_utc(),
        "2025-01-08T12:00:00Z".parse::<chrono::DateTime<chrono::Utc>>().expect("literal")
    );
}
