//! Chained destinations: ordering, skipping, reporting and rehearsal.
//!
//! The product rule these tests exist to pin down is that **the fan-out is
//! never flattened**. A destination whose contents are replicated from another
//! is still its own [`DestinationRun`], with its own status, its own progress
//! and its own error — and when it cannot run because its source did not
//! succeed, it is *skipped with a reason*, not failed.
//!
//! Everything runs against [`MockExecutor`] and a [`TestClock`], so the file
//! completes in milliseconds and never needs kopia.

use std::sync::Arc;

use superbackup_core::engine::cancel::CancelToken;
use superbackup_core::engine::executor::ExecutorError;
use superbackup_core::engine::testing::{
    test_job, test_replica, test_repository, MockBehaviour, MockExecutor,
};
use superbackup_core::engine::{
    plan_destinations, EngineEvent, RetryPolicy, RunRequest, Runner, EVENT_CHANNEL_CAPACITY,
};
use superbackup_core::error::ErrorCode;
use superbackup_core::model::{Destination, Job, Settings};
use superbackup_core::state::{DestinationRun, JobRun, PersistedState, RunStatus, Trigger};
use uuid::Uuid;

struct Harness {
    runner: Runner,
    executor: Arc<MockExecutor>,
    events: tokio::sync::broadcast::Sender<EngineEvent>,
}

fn harness() -> Harness {
    let clock = Arc::new(superbackup_core::engine::clock::TestClock::at("2026-03-01T02:00:00Z"));
    let executor = Arc::new(MockExecutor::new());
    let state = Arc::new(tokio::sync::Mutex::new(PersistedState::default()));
    let (events, _) = tokio::sync::broadcast::channel(EVENT_CHANNEL_CAPACITY);
    let runner = Runner::new(executor.clone(), clock, Arc::new(chrono::Utc), events.clone(), state)
        .with_retry_policy(RetryPolicy::none());
    Harness { runner, executor, events }
}

fn request(job: Job, destinations: Vec<Destination>, cancel: CancelToken) -> RunRequest {
    RunRequest {
        dry_run: false,
        run_id: Uuid::new_v4(),
        job: Arc::new(job),
        destinations: destinations.into_iter().map(Arc::new).collect(),
        settings: Arc::new(Settings::default()),
        trigger: Trigger::Manual,
        cancel,
    }
}

fn job_for(destinations: &[Destination]) -> Job {
    let mut job = test_job("dev-code");
    job.destination_ids = destinations.iter().map(|d| d.id).collect();
    job
}

/// The shape the user asked for: back up to OneDrive, then push that
/// repository onward to StorJ without reading the sources twice.
fn onedrive_then_storj() -> (Destination, Destination) {
    let onedrive = test_repository("onedrive", "/repos/onedrive");
    let storj = test_replica("storj-offsite", "/repos/storj", onedrive.id);
    (onedrive, storj)
}

fn find<'a>(run: &'a JobRun, destination: &Destination) -> &'a DestinationRun {
    run.destinations
        .iter()
        .find(|d| d.destination_id == destination.id)
        .unwrap_or_else(|| panic!("{:?} is missing from the run entirely", destination.name))
}

// ---------------------------------------------------------------------------
// Ordering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_chain_runs_its_source_before_its_replica() {
    let h = harness();
    let (onedrive, storj) = onedrive_then_storj();
    let destinations = vec![onedrive.clone(), storj.clone()];
    let job = job_for(&destinations);

    let run = h.runner.execute(request(job, destinations, CancelToken::new())).await;

    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(h.executor.order(), vec![onedrive.id, storj.id]);
    // The source was snapshotted; the replica was replicated, never
    // snapshotted. Reading the sources a second time is the entire thing this
    // feature exists to avoid.
    assert_eq!(h.executor.calls().len(), 1);
    let replications = h.executor.replications();
    assert_eq!(replications.len(), 1);
    assert_eq!(replications[0].destination_id, storj.id);
    assert_eq!(replications[0].source_id, onedrive.id);
}

#[tokio::test]
async fn a_replica_listed_before_its_source_is_reordered_rather_than_failing() {
    let h = harness();
    let (onedrive, storj) = onedrive_then_storj();
    // Declared the wrong way round, which a job editor makes easy to do.
    let destinations = vec![storj.clone(), onedrive.clone()];
    let job = job_for(&destinations);

    let run = h.runner.execute(request(job, destinations, CancelToken::new())).await;

    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(h.executor.order(), vec![onedrive.id, storj.id]);
    // The run report is in execution order too, so what the user reads matches
    // what happened.
    assert_eq!(run.destinations[0].destination_id, onedrive.id);
    assert_eq!(run.destinations[1].destination_id, storj.id);
}

#[tokio::test]
async fn a_two_hop_chain_runs_root_first_then_each_link() {
    let h = harness();
    let local = test_repository("fast-local", "/repos/local");
    let onedrive = test_replica("onedrive", "/repos/onedrive", local.id);
    let storj = test_replica("storj-offsite", "/repos/storj", onedrive.id);
    // Deliberately shuffled.
    let destinations = vec![storj.clone(), onedrive.clone(), local.clone()];
    let job = job_for(&destinations);

    let run = h.runner.execute(request(job, destinations, CancelToken::new())).await;

    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(h.executor.order(), vec![local.id, onedrive.id, storj.id]);
    // Only the root reads the sources; each further hop copies blobs.
    assert_eq!(h.executor.calls().len(), 1);
    assert_eq!(h.executor.replications().len(), 2);
}

#[test]
fn planning_preserves_the_declared_order_when_it_already_works() {
    let a = Arc::new(test_repository("a", "/a"));
    let b = Arc::new(test_repository("b", "/b"));
    let c = Arc::new(test_repository("c", "/c"));
    let plan = plan_destinations(&[Arc::clone(&a), Arc::clone(&b), Arc::clone(&c)]);
    let ids: Vec<Uuid> = plan.iter().map(|p| p.destination.id).collect();
    assert_eq!(ids, vec![a.id, b.id, c.id]);
    assert!(plan.iter().all(|p| p.depends_on.is_none() && p.blocked.is_none()));
}

#[test]
fn planning_marks_a_cycle_blocked_instead_of_looping_forever() {
    // Two destinations replicating from each other. Validation rejects this,
    // but a daemon started on a config the user is still repairing must still
    // terminate, and must still report on every destination.
    let mut a = test_repository("a", "/a");
    let mut b = test_repository("b", "/b");
    a.replicate_from = Some(b.id);
    b.replicate_from = Some(a.id);
    let plan = plan_destinations(&[Arc::new(a), Arc::new(b)]);

    assert_eq!(plan.len(), 2, "every destination must still appear");
    assert!(plan.iter().all(|p| p.blocked.is_some()));
    for entry in &plan {
        let reason = entry.blocked.as_deref().unwrap_or_default();
        assert!(reason.contains("loop"), "the reason must say what is wrong: {reason}");
    }
}

#[test]
fn planning_blocks_a_replica_whose_source_is_not_in_the_run() {
    let absent = Uuid::new_v4();
    let orphan = Arc::new(test_replica("storj-offsite", "/repos/storj", absent));
    let plan = plan_destinations(&[orphan]);
    assert_eq!(plan.len(), 1);
    let reason = plan[0].blocked.clone().expect("blocked");
    assert!(reason.contains("does not back up"), "unhelpful reason: {reason}");
}

// ---------------------------------------------------------------------------
// A failed source skips its dependants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_failed_source_skips_its_replica_with_a_readable_reason() {
    let h = harness();
    let (onedrive, storj) = onedrive_then_storj();
    h.executor.set_for(
        onedrive.id,
        MockBehaviour::Fail(
            ExecutorError::new(ErrorCode::Kopia, "the OneDrive folder is not there").permanent(),
        ),
    );
    let destinations = vec![onedrive.clone(), storj.clone()];
    let job = job_for(&destinations);

    let run = h.runner.execute(request(job, destinations, CancelToken::new())).await;

    let source = find(&run, &onedrive);
    let replica = find(&run, &storj);

    assert_eq!(source.status, RunStatus::Failed);
    // Skipped, *not* failed: nothing is wrong with StorJ, and marking it failed
    // would put a red mark on a healthy destination and bury the real fault.
    assert_eq!(replica.status, RunStatus::Skipped);
    assert!(replica.error.is_none(), "a skipped replica has no failure of its own");

    let reason = replica.skipped_reason.clone().expect("a skip must carry a reason");
    assert!(reason.contains("onedrive"), "the reason must name the source: {reason}");
    assert!(reason.contains("failed"), "the reason must say what happened: {reason}");

    // Nothing was attempted against the replica at all.
    assert_eq!(h.executor.attempts(storj.id), 0);
    // The run as a whole is a failure, because a destination failed.
    assert_eq!(run.status, RunStatus::Failed);
}

#[tokio::test]
async fn a_skipped_replica_still_gets_its_own_finished_event() {
    let h = harness();
    let mut subscriber = h.events.subscribe();
    let (onedrive, storj) = onedrive_then_storj();
    h.executor.set_for(
        onedrive.id,
        MockBehaviour::Fail(ExecutorError::new(ErrorCode::Io, "disk").permanent()),
    );
    let destinations = vec![onedrive.clone(), storj.clone()];
    let job = job_for(&destinations);

    let _ = h.runner.execute(request(job, destinations, CancelToken::new())).await;

    let mut finished_for_replica = None;
    while let Ok(event) = subscriber.try_recv() {
        if let EngineEvent::DestinationFinished { destination, .. } = event {
            if destination.destination_id == storj.id {
                finished_for_replica = Some(destination);
            }
        }
    }
    let reported = finished_for_replica.expect("a skipped destination is still reported");
    assert_eq!(reported.status, RunStatus::Skipped);
    assert!(reported.skipped_reason.is_some());
}

#[tokio::test]
async fn a_replica_of_a_healthy_source_still_runs_when_a_sibling_failed() {
    let h = harness();
    let (onedrive, storj) = onedrive_then_storj();
    let mirrorless = test_repository("unrelated", "/repos/other");
    h.executor.set_for(
        mirrorless.id,
        MockBehaviour::Fail(ExecutorError::new(ErrorCode::Io, "unrelated fault").permanent()),
    );
    let destinations = vec![mirrorless.clone(), onedrive.clone(), storj.clone()];
    let job = job_for(&destinations);

    let run = h.runner.execute(request(job, destinations, CancelToken::new())).await;

    // The chain is independent of the destination that broke, so it ran.
    assert_eq!(find(&run, &storj).status, RunStatus::Succeeded);
    assert_eq!(h.executor.replications().len(), 1);
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replica_is_marked_as_replicated_from_its_source() {
    let h = harness();
    let (onedrive, storj) = onedrive_then_storj();
    let destinations = vec![onedrive.clone(), storj.clone()];
    let job = job_for(&destinations);

    let run = h.runner.execute(request(job, destinations, CancelToken::new())).await;

    let source = find(&run, &onedrive);
    let replica = find(&run, &storj);

    assert!(!source.is_replica(), "a destination written from the sources is not a replica");
    assert!(source.replicated_from.is_none());

    let origin = replica.replicated_from.clone().expect("a replica records where it came from");
    assert_eq!(origin.destination_id, onedrive.id);
    // The name is captured at run time so a later rename cannot make the
    // history lie about what was copied.
    assert_eq!(origin.destination_name, "onedrive");

    // The fan-out is not flattened: the replica has its own progress, and a
    // replication creates no snapshot of its own.
    assert!(replica.progress.bytes_processed > 0);
    assert!(replica.snapshot_id.is_none());
    assert!(source.snapshot_id.is_some());
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancelling_during_a_replication_stops_it_and_leaves_nothing_behind() {
    let h = harness();
    let (onedrive, storj) = onedrive_then_storj();
    h.executor.set_for(storj.id, MockBehaviour::BlockUntilCancelled);
    let trailing = test_repository("third", "/repos/third");
    let destinations = vec![onedrive.clone(), storj.clone(), trailing.clone()];
    let job = job_for(&destinations);

    let cancel = CancelToken::new();
    let watcher = cancel.clone();
    let executor = Arc::clone(&h.executor);
    let stopper = tokio::spawn(async move {
        // Wait until the replication is genuinely under way, so this asserts
        // cancellation *during* the sync rather than before it.
        for _ in 0..2000 {
            if !executor.replications().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        watcher.cancel(superbackup_core::engine::CancelReason::Requested);
    });

    let run = h.runner.execute(request(job, destinations, cancel)).await;
    stopper.await.expect("stopper");

    assert_eq!(find(&run, &onedrive).status, RunStatus::Succeeded);
    assert_eq!(find(&run, &storj).status, RunStatus::Cancelled);
    // Nothing after the cancelled destination was started: a stopped run must
    // not quietly carry on spending the user's uplink.
    assert_eq!(find(&run, &trailing).status, RunStatus::Cancelled);
    assert_eq!(h.executor.attempts(trailing.id), 0);
    assert_eq!(run.status, RunStatus::Cancelled);
    // Every destination reached a terminal state; none was left `Running`,
    // which is what "nothing orphaned" means in the run report.
    assert!(run.destinations.iter().all(|d| d.status.is_terminal()));
    assert!(run.finished_at.is_some());
}

// ---------------------------------------------------------------------------
// Dry run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_dry_run_replicates_nothing() {
    let h = harness();
    let (onedrive, storj) = onedrive_then_storj();
    let destinations = vec![onedrive.clone(), storj.clone()];
    let job = job_for(&destinations);
    let mut req = request(job, destinations, CancelToken::new());
    req.dry_run = true;

    let run = h.runner.execute(req).await;

    let replications = h.executor.replications();
    assert_eq!(replications.len(), 1, "the rehearsal still reports on the replica");
    assert!(replications[0].dry_run, "the executor must be told this is a rehearsal");

    let replica = find(&run, &storj);
    assert_eq!(replica.progress.bytes_processed, 0, "a dry run copies nothing");
    assert!(
        replica.warnings.iter().any(|w| w.contains("Dry run")),
        "the user must be told nothing was written: {:?}",
        replica.warnings
    );
}
