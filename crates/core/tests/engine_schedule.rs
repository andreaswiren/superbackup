//! Schedule semantics, end to end through a running scheduler.
//!
//! The arithmetic itself is unit-tested inside
//! `superbackup_core::engine::schedule`. What is tested here is that the
//! *engine* honours it: that a 02:30 job on the spring-forward day actually
//! produces a run, that the same job on the autumn day produces exactly one,
//! and that a machine which was off for a week produces one catch-up rather
//! than a backlog.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use superbackup_core::engine::clock::TestClock;
use superbackup_core::engine::schedule::{catch_up_due, describe, next_occurrence_in, Zone};
use superbackup_core::engine::testing::{test_job, test_repository, MockExecutor};
use superbackup_core::engine::tz::DstZone;
use superbackup_core::engine::{EngineBuilder, EngineEvent, SchedulerHandle, StaticEnvironment};
use superbackup_core::model::{Config, Destination, Job, Schedule, TimeOfDay};
use superbackup_core::state::{PersistedState, RunStatus, Trigger};
use uuid::Uuid;

const PATIENCE: StdDuration = StdDuration::from_secs(10);

struct Harness {
    handle: SchedulerHandle,
    clock: Arc<TestClock>,
    executor: Arc<MockExecutor>,
}

fn build(start: &str, jobs: Vec<Job>, destinations: Vec<Destination>) -> Harness {
    let clock = Arc::new(TestClock::at(start));
    let executor = Arc::new(MockExecutor::new());
    let config = Config { jobs, destinations, ..Config::default() };
    let handle = EngineBuilder::new(Arc::new(config), executor.clone())
        .clock(clock.clone())
        .zone(Arc::new(DstZone::EuropeStockholm))
        .environment(Arc::new(StaticEnvironment::unlocked()))
        .state(Arc::new(tokio::sync::Mutex::new(PersistedState::default())))
        .spawn();
    Harness { handle, clock, executor }
}

fn daily_job(destination: &Destination, hour: u8, minute: u8) -> Job {
    let mut job = test_job("nightly");
    job.destination_ids = vec![destination.id];
    job.schedule = Schedule::Daily { times: vec![TimeOfDay { hour, minute }] };
    job
}

async fn wait_for_finished(
    events: &mut tokio::sync::broadcast::Receiver<EngineEvent>,
    label: &str,
) -> (Uuid, RunStatus, Trigger) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for {label}");
        match tokio::time::timeout(remaining, events.recv()).await {
            Ok(Ok(EngineEvent::RunFinished { run })) => {
                return (run.job_id, run.status, run.trigger)
            }
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => panic!("the event stream closed while waiting for {label}"),
            Err(_) => panic!("timed out waiting for {label}"),
        }
    }
}

/// Give the engine time to do anything it was going to do.
async fn settle() {
    tokio::time::sleep(StdDuration::from_millis(50)).await;
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn a_job_scheduled_inside_the_spring_forward_gap_still_runs() {
    // Europe/Stockholm, 2025-03-30: the clock jumps 02:00 -> 03:00, so 02:30
    // does not exist. The engine must run the job at 03:00 local (01:00 UTC),
    // not skip the day entirely.
    let destination = test_repository("local", "/repos/local");
    let job = daily_job(&destination, 2, 30);
    let job_id = job.id;
    // 00:00 UTC on the transition day is 01:00 CET, an hour before the gap.
    let h = build("2025-03-30T00:00:00Z", vec![job], vec![destination]);
    let mut events = h.handle.subscribe();

    h.clock.wait_for_sleeps(1).await;
    h.clock.set("2025-03-30T01:00:01Z".parse().expect("literal"));

    let (id, status, trigger) = wait_for_finished(&mut events, "the snapped-forward run").await;
    assert_eq!(id, job_id);
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(trigger, Trigger::Schedule);
    assert_eq!(h.executor.calls().len(), 1, "exactly one run, not none and not two");

    // And it is back to its usual 02:30 CEST = 00:30 UTC the next day.
    let next = h.handle.status().await.expect("status").next_runs.get(&job_id).copied();
    assert_eq!(
        next,
        Some("2025-03-31T00:30:00Z".parse().expect("literal")),
        "the day after a gap is an ordinary day"
    );
}

#[tokio::test]
async fn a_job_scheduled_inside_the_fall_back_fold_runs_exactly_once() {
    // Europe/Stockholm, 2025-10-26: the clock jumps 03:00 -> 02:00, so 02:30
    // happens twice — at 00:30 UTC and again at 01:30 UTC. The engine must run
    // the job once, on the first pass.
    let destination = test_repository("local", "/repos/local");
    let job = daily_job(&destination, 2, 30);
    let job_id = job.id;
    // 00:00 UTC is 02:00 CEST, half an hour before the first pass.
    let h = build("2025-10-26T00:00:00Z", vec![job], vec![destination]);
    let mut events = h.handle.subscribe();

    h.clock.wait_for_sleeps(1).await;
    h.clock.set("2025-10-26T00:30:01Z".parse().expect("literal"));
    let (id, status, _) = wait_for_finished(&mut events, "the first pass").await;
    assert_eq!(id, job_id);
    assert_eq!(status, RunStatus::Succeeded);

    // Now walk past the *second* 02:30 of the day.
    h.clock.set("2025-10-26T02:00:00Z".parse().expect("literal"));
    settle().await;
    assert_eq!(
        h.executor.calls().len(),
        1,
        "the second pass through 02:30 must not produce a second backup"
    );

    let next = h.handle.status().await.expect("status").next_runs.get(&job_id).copied();
    assert_eq!(
        next,
        Some("2025-10-27T01:30:00Z".parse().expect("literal")),
        "02:30 CET the following day"
    );
}

#[tokio::test]
async fn an_interval_schedule_is_unaffected_by_a_transition() {
    // An hourly job across the spring-forward instant: elapsed time, not wall
    // clock, so it must fire once per hour with no gap and no duplicate.
    let destination = test_repository("local", "/repos/local");
    let mut job = test_job("hourly");
    job.destination_ids = vec![destination.id];
    job.schedule = Schedule::Interval { minutes: 60 };
    let h = build("2025-03-30T00:30:00Z", vec![job], vec![destination]);
    let mut events = h.handle.subscribe();

    h.clock.wait_for_sleeps(1).await;
    h.clock.set("2025-03-30T01:00:01Z".parse().expect("literal"));
    wait_for_finished(&mut events, "the run at the transition instant").await;

    h.clock.set("2025-03-30T01:59:00Z".parse().expect("literal"));
    settle().await;
    assert_eq!(h.executor.calls().len(), 1, "no extra run inside the same hour");

    h.clock.set("2025-03-30T02:00:01Z".parse().expect("literal"));
    wait_for_finished(&mut events, "the next hourly run").await;
    assert_eq!(h.executor.calls().len(), 2);
}

// ---------------------------------------------------------------------------
// The pure API, exercised the way the GUI and CLI will
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_public_schedule_api_agrees_with_the_engine() {
    let tz = DstZone::EuropeStockholm;
    let schedule = Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 30 }] };
    let before_gap = "2025-03-29T23:00:00Z".parse().expect("literal");
    assert_eq!(
        next_occurrence_in(&schedule, &tz, before_gap),
        Some("2025-03-30T01:00:00Z".parse().expect("literal"))
    );

    // The object-safe projection the engine actually holds must agree.
    let zone: Arc<dyn Zone> = Arc::new(tz);
    assert_eq!(
        zone.next_occurrence(&schedule, before_gap),
        next_occurrence_in(&schedule, &tz, before_gap)
    );

    // A week of downtime collapses to one owed run.
    let last_run = "2025-01-01T01:00:00Z".parse().expect("literal");
    let now = "2025-01-08T12:00:00Z".parse().expect("literal");
    let hourly = Schedule::Interval { minutes: 60 };
    assert_eq!(
        catch_up_due(&hourly, &tz, Some(last_run), now),
        Some("2025-01-08T12:00:00Z".parse().expect("literal"))
    );
    assert_eq!(zone.catch_up(&hourly, Some(last_run), now), Some(now));
}

#[test]
fn describe_is_the_sentence_the_gui_shows() {
    assert_eq!(
        describe(&Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] }),
        "Every day at 02:00"
    );
    assert_eq!(
        describe(&Schedule::Weekly {
            weekdays: vec![0, 1, 2, 3, 4],
            times: vec![TimeOfDay { hour: 9, minute: 0 }, TimeOfDay { hour: 18, minute: 0 }],
        }),
        "Weekdays at 09:00 and 18:00"
    );
    assert_eq!(
        describe(&Schedule::OnChange { debounce_seconds: 900, min_interval_minutes: 0 }),
        "15 minutes after changes stop"
    );
}

#[tokio::test]
async fn a_manual_job_is_never_scheduled() {
    let destination = test_repository("local", "/repos/local");
    let mut job = test_job("manual");
    job.destination_ids = vec![destination.id];
    job.schedule = Schedule::Manual;
    let job_id = job.id;
    let h = build("2025-01-08T12:00:00Z", vec![job], vec![destination]);

    settle().await;
    let status = h.handle.status().await.expect("status");
    assert!(!status.next_runs.contains_key(&job_id));
    assert!(status.next_scheduled.is_none());

    // Advancing a year changes nothing.
    h.clock.advance(chrono::Duration::days(365));
    settle().await;
    assert_eq!(h.executor.calls().len(), 0);
}
