//! Runner behaviour: fan-out, partial failure, cancellation, retry, timeout,
//! hooks and persistence.
//!
//! Everything here runs against [`MockExecutor`] and a [`TestClock`], so the
//! whole file completes in milliseconds and asserts exact instants rather than
//! "roughly a second".

use std::sync::Arc;

use superbackup_core::engine::cancel::{CancelReason, CancelToken};
use superbackup_core::engine::clock::{Clock, TestClock};
use superbackup_core::engine::executor::ExecutorError;
use superbackup_core::engine::testing::{test_job, test_repository, MockBehaviour, MockExecutor};
use superbackup_core::engine::{
    EngineEvent, EVENT_CHANNEL_CAPACITY, RetryPolicy, RunRequest, Runner,
};
use superbackup_core::error::ErrorCode;
use superbackup_core::model::{Destination, Job, JobHooks, Settings};
use superbackup_core::state::{JobRun, PersistedState, RunStatus, Trigger};
use uuid::Uuid;

struct Harness {
    runner: Runner,
    clock: Arc<TestClock>,
    executor: Arc<MockExecutor>,
    state: Arc<tokio::sync::Mutex<PersistedState>>,
    events: tokio::sync::broadcast::Sender<EngineEvent>,
}

fn harness() -> Harness {
    harness_with(RetryPolicy::none())
}

fn harness_with(retry: RetryPolicy) -> Harness {
    let clock = Arc::new(TestClock::at("2025-01-08T12:00:00Z"));
    let executor = Arc::new(MockExecutor::new());
    let state = Arc::new(tokio::sync::Mutex::new(PersistedState::default()));
    let (events, _) = tokio::sync::broadcast::channel(EVENT_CHANNEL_CAPACITY);
    let runner = Runner::new(
        executor.clone(),
        clock.clone(),
        Arc::new(chrono::Utc),
        events.clone(),
        state.clone(),
    )
    .with_retry_policy(retry);
    Harness { runner, clock, executor, state, events }
}

fn request(job: Job, destinations: Vec<Destination>, cancel: CancelToken) -> RunRequest {
    RunRequest {
        run_id: Uuid::new_v4(),
        job: Arc::new(job),
        destinations: destinations.into_iter().map(Arc::new).collect(),
        settings: Arc::new(Settings::default()),
        trigger: Trigger::Manual,
        cancel,
    }
}

fn three_destinations() -> Vec<Destination> {
    vec![
        test_repository("fast-local", "/repos/local"),
        test_repository("onedrive", "/repos/onedrive"),
        test_repository("offsite", "/repos/offsite"),
    ]
}

fn job_for(destinations: &[Destination]) -> Job {
    let mut job = test_job("dev-code");
    job.destination_ids = destinations.iter().map(|d| d.id).collect();
    job
}

#[tokio::test]
async fn one_run_produces_one_destination_run_per_destination() {
    let h = harness();
    let destinations = three_destinations();
    let job = job_for(&destinations);
    let run = h.runner.execute(request(job, destinations.clone(), CancelToken::new())).await;

    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(run.destinations.len(), 3);
    for (dest, expected) in run.destinations.iter().zip(&destinations) {
        assert_eq!(dest.destination_id, expected.id);
        assert_eq!(dest.status, RunStatus::Succeeded);
        assert!(dest.snapshot_id.is_some(), "a successful snapshot records its id");
        assert_eq!(dest.progress.files_processed, 10);
    }
    // Every repository destination is prepared before it is snapshotted.
    assert_eq!(h.executor.prepares().len(), 3);
}

#[tokio::test]
async fn a_failing_destination_does_not_abort_the_others() {
    let h = harness();
    let destinations = three_destinations();
    let mut job = job_for(&destinations);
    job.continue_on_destination_error = true;
    h.executor.set_for(
        destinations[1].id,
        MockBehaviour::Fail(ExecutorError::new(ErrorCode::Kopia, "bucket unreachable").permanent()),
    );

    let run = h.runner.execute(request(job, destinations.clone(), CancelToken::new())).await;

    assert_eq!(run.destinations[0].status, RunStatus::Succeeded);
    assert_eq!(run.destinations[1].status, RunStatus::Failed);
    assert_eq!(run.destinations[2].status, RunStatus::Succeeded, "the third must still run");
    assert_eq!(run.status, RunStatus::Failed, "derive_status: any failure fails the run");
    assert_eq!(
        run.destinations[1].error.as_ref().map(|e| e.code),
        Some(ErrorCode::Kopia),
        "the failure is recorded against the destination that produced it"
    );
}

#[tokio::test]
async fn continue_on_destination_error_false_stops_at_the_first_failure() {
    let h = harness();
    let destinations = three_destinations();
    let mut job = job_for(&destinations);
    job.continue_on_destination_error = false;
    h.executor.set_for(
        destinations[0].id,
        MockBehaviour::Fail(ExecutorError::new(ErrorCode::Kopia, "disk full").permanent()),
    );

    let run = h.runner.execute(request(job, destinations.clone(), CancelToken::new())).await;

    assert_eq!(run.destinations[0].status, RunStatus::Failed);
    assert_eq!(run.destinations[1].status, RunStatus::Skipped);
    assert_eq!(run.destinations[2].status, RunStatus::Skipped);
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(h.executor.attempts(destinations[1].id), 0, "the second was never attempted");
}

#[tokio::test]
async fn warnings_downgrade_success_without_failing_the_run() {
    let h = harness();
    let destinations = vec![test_repository("local", "/repos/local")];
    let job = job_for(&destinations);
    h.executor.set_for(
        destinations[0].id,
        MockBehaviour::Succeed {
            files: 3,
            bytes: 30,
            warnings: vec!["2 files were locked and skipped".into()],
        },
    );

    let run = h.runner.execute(request(job, destinations, CancelToken::new())).await;
    assert_eq!(run.status, RunStatus::SucceededWithWarnings);
    assert_eq!(run.destinations[0].warnings.len(), 1);
}

#[tokio::test]
async fn cancellation_is_prompt_and_isolated() {
    let h = harness();
    let destinations = vec![test_repository("local", "/repos/local")];
    let job = job_for(&destinations);
    h.executor.set_for(destinations[0].id, MockBehaviour::BlockUntilCancelled);

    let cancel = CancelToken::new();
    let other_job = CancelToken::new();
    let request = request(job, destinations, cancel.clone());

    let runner = h.runner.clone();
    let handle = tokio::spawn(async move { runner.execute(request).await });

    // Let the mock reach its await, then stop it.
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    cancel.cancel(CancelReason::Requested);
    let run = handle.await.expect("run task");

    assert_eq!(run.status, RunStatus::Cancelled);
    assert_eq!(run.destinations[0].status, RunStatus::Cancelled);
    assert_eq!(
        run.destinations[0].progress.files_processed, 1,
        "progress accumulated before the stop is preserved"
    );
    assert!(!other_job.is_cancelled(), "cancelling one job must not touch another token");
    // The clock never moved: nothing waited out a real timeout.
    assert_eq!(h.clock.now_utc(), "2025-01-08T12:00:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap());
}

#[tokio::test]
async fn a_driver_that_ignores_cancellation_is_left_to_unwind() {
    let h = harness();
    let destinations = vec![test_repository("wedged", "/repos/wedged")];
    let job = job_for(&destinations);
    h.executor.set_for(destinations[0].id, MockBehaviour::HangForever);

    let cancel = CancelToken::new();
    let request = request(job, destinations, cancel.clone());
    let runner = h.runner.clone();
    let handle = tokio::spawn(async move { runner.execute(request).await });

    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    cancel.cancel(CancelReason::Requested);
    // The runner now waits out its cancel grace on the injected clock.
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(chrono::Duration::seconds(
        superbackup_core::engine::runner::CANCEL_GRACE_SECONDS + 1,
    ));

    let run = handle.await.expect("run task");
    assert_eq!(run.status, RunStatus::Cancelled, "the run reports promptly");
    assert_eq!(run.destinations[0].status, RunStatus::Cancelled);
}

#[tokio::test]
async fn transient_failures_are_retried_with_backoff() {
    let h = harness_with(RetryPolicy::default());
    let destinations = vec![test_repository("s3", "/repos/s3")];
    let job = job_for(&destinations);
    h.executor.set_for(
        destinations[0].id,
        MockBehaviour::FailThenSucceed {
            remaining: 2,
            error: ExecutorError::new(ErrorCode::Kopia, "503 Slow Down"),
        },
    );

    let request = request(job, destinations.clone(), CancelToken::new());
    let runner = h.runner.clone();
    let clock = h.clock.clone();
    let handle = tokio::spawn(async move { runner.execute(request).await });

    // Two backoff sleeps: 5s then 20s.
    clock.wait_for_sleeps(1).await;
    clock.advance(chrono::Duration::seconds(5));
    clock.wait_for_sleeps(2).await;
    clock.advance(chrono::Duration::seconds(20));

    let run = handle.await.expect("run task");
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(h.executor.attempts(destinations[0].id), 3, "two retries then success");
}

#[tokio::test]
async fn deterministic_failures_are_not_retried() {
    let h = harness_with(RetryPolicy::default());
    let destinations = vec![test_repository("local", "/repos/local")];
    let job = job_for(&destinations);
    h.executor.set_for(
        destinations[0].id,
        MockBehaviour::Fail(ExecutorError::new(ErrorCode::BadPassphrase, "wrong passphrase")),
    );

    let run = h.runner.execute(request(job, destinations.clone(), CancelToken::new())).await;
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(
        h.executor.attempts(destinations[0].id),
        1,
        "a wrong passphrase must be reported once, not four times"
    );
}

#[tokio::test]
async fn retry_is_bounded() {
    let h = harness_with(RetryPolicy::default());
    let destinations = vec![test_repository("s3", "/repos/s3")];
    let job = job_for(&destinations);
    h.executor.set_for(
        destinations[0].id,
        MockBehaviour::Fail(ExecutorError::new(ErrorCode::Kopia, "connection reset by peer")),
    );

    let request = request(job, destinations.clone(), CancelToken::new());
    let runner = h.runner.clone();
    let clock = h.clock.clone();
    let handle = tokio::spawn(async move { runner.execute(request).await });
    for n in 1..=2 {
        clock.wait_for_sleeps(n).await;
        clock.advance(chrono::Duration::seconds(120));
    }
    let run = handle.await.expect("run task");
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(h.executor.attempts(destinations[0].id), 3, "max_attempts is a hard bound");
}

#[tokio::test]
async fn the_timeout_stops_the_run_and_reports_it_as_a_failure() {
    let h = harness();
    let destinations = vec![test_repository("slow", "/repos/slow")];
    let mut job = job_for(&destinations);
    job.timeout_minutes = Some(30);
    h.executor.set_for(destinations[0].id, MockBehaviour::BlockUntilCancelled);

    let request = request(job, destinations, CancelToken::new());
    let runner = h.runner.clone();
    let clock = h.clock.clone();
    let handle = tokio::spawn(async move { runner.execute(request).await });

    clock.wait_for_sleeps(1).await; // the watchdog is armed
    clock.advance(chrono::Duration::minutes(31));

    let run = handle.await.expect("run task");
    assert_eq!(run.status, RunStatus::Failed, "a timeout is a failure, not a quiet cancellation");
    assert_eq!(run.destinations[0].status, RunStatus::Failed);
    let error = run.destinations[0].error.as_ref().expect("an error");
    assert!(error.message.contains("time limit"), "{}", error.message);
}

#[tokio::test]
async fn a_run_inside_its_timeout_is_untouched() {
    let h = harness();
    let destinations = vec![test_repository("local", "/repos/local")];
    let mut job = job_for(&destinations);
    job.timeout_minutes = Some(30);
    let run = h.runner.execute(request(job, destinations, CancelToken::new())).await;
    assert_eq!(run.status, RunStatus::Succeeded);
}

#[tokio::test]
async fn a_failing_before_hook_aborts_only_when_asked_to() {
    let failing = "exit 1";

    // Without the flag, the run proceeds.
    let h = harness();
    let destinations = vec![test_repository("local", "/repos/local")];
    let mut job = job_for(&destinations);
    job.hooks = JobHooks {
        before: Some(failing.to_string()),
        after_success: None,
        after_failure: None,
        abort_on_before_failure: false,
    };
    let run = h.runner.execute(request(job, destinations.clone(), CancelToken::new())).await;
    assert_eq!(run.status, RunStatus::Succeeded);

    // With it, nothing runs and the run fails.
    let h = harness();
    let mut job = job_for(&destinations);
    job.hooks = JobHooks {
        before: Some(failing.to_string()),
        after_success: None,
        after_failure: None,
        abort_on_before_failure: true,
    };
    let run = h.runner.execute(request(job, destinations.clone(), CancelToken::new())).await;
    assert_eq!(
        run.status,
        RunStatus::Failed,
        "an aborted run must never be reported as a partial success"
    );
    assert_eq!(run.destinations[0].status, RunStatus::Skipped);
    assert_eq!(h.executor.attempts(destinations[0].id), 0);
}

#[tokio::test]
async fn hook_output_is_captured() {
    let h = harness();
    let destinations = vec![test_repository("local", "/repos/local")];
    let mut job = job_for(&destinations);
    job.hooks =
        JobHooks { before: Some("echo before-ran".into()), ..JobHooks::default() };
    let mut events = h.events.subscribe();
    let _ = h.runner.execute(request(job, destinations, CancelToken::new())).await;

    let mut saw_hook = false;
    while let Ok(event) = events.try_recv() {
        if let EngineEvent::HookFinished { outcome, .. } = event {
            saw_hook = true;
            assert!(outcome.succeeded());
            assert!(outcome.output.contains("before-ran"), "{:?}", outcome.output);
        }
    }
    assert!(saw_hook, "the hook outcome must be broadcast");
}

#[tokio::test]
async fn the_finished_run_is_recorded_in_persisted_state() {
    let h = harness();
    let destinations = vec![test_repository("local", "/repos/local")];
    let job = job_for(&destinations);
    let job_id = job.id;
    let run = h.runner.execute(request(job, destinations, CancelToken::new())).await;

    let state = h.state.lock().await;
    assert_eq!(state.history.len(), 1);
    assert_eq!(state.history[0].run_id, run.run_id);
    let summary = state.jobs.get(&job_id).expect("a summary");
    assert_eq!(summary.total_runs, 1);
    assert_eq!(summary.last_status, Some(RunStatus::Succeeded));
    assert_eq!(summary.consecutive_failures, 0);
    assert_eq!(summary.last_uploaded_bytes, 1024);
}

#[tokio::test]
async fn progress_reaches_subscribers_before_the_finished_event() {
    let h = harness();
    let destinations = vec![test_repository("local", "/repos/local")];
    let job = job_for(&destinations);
    let mut events = h.events.subscribe();
    let _ = h.runner.execute(request(job, destinations, CancelToken::new())).await;

    let mut seen_final_progress = false;
    let mut order_ok = false;
    while let Ok(event) = events.try_recv() {
        match event {
            EngineEvent::Progress(update) if update.final_update => seen_final_progress = true,
            EngineEvent::RunFinished { .. } => order_ok = seen_final_progress,
            _ => {}
        }
    }
    assert!(seen_final_progress, "the terminal progress frame must be emitted");
    assert!(order_ok, "RunFinished must not overtake the final progress frame");
}

#[tokio::test]
async fn a_run_with_no_destinations_is_skipped_not_succeeded() {
    let h = harness();
    let job = test_job("orphan");
    let run: JobRun = h.runner.execute(request(job, Vec::new(), CancelToken::new())).await;
    assert_eq!(run.status, RunStatus::Skipped);
}
