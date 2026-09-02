//! Test doubles for the engine.
//!
//! Compiled into the library rather than hidden behind `#[cfg(test)]`, because
//! the engine's integration tests live in `crates/core/tests/` and can only
//! reach the public API. The cost is a few hundred bytes of unused code in a
//! release build; the benefit is that the engine can be exercised end-to-end
//! without kopia, without a network, and without a wall clock.
//!
//! [`MockExecutor`] is also the reference for what a real
//! [`BackupExecutor`] must do: honour the cancel token, stream progress, and
//! classify its errors.

use crate::engine::cancel::CancelToken;
use crate::engine::clock::BoxFuture;
use crate::engine::executor::{
    BackupExecutor, ExecutorError, ExecutorResult, PrepareOutcome, PrepareRequest,
    ReplicateOutcome, ReplicateRequest, SnapshotOutcome, SnapshotRequest, VerifyOutcome,
    VerifyRequest,
};
use crate::engine::throttle::ResolvedBandwidth;
use crate::model::{Destination, DestinationKind, ExclusionSet, Job, JobHooks, Schedule, Source};
use crate::state::Progress;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// What the mock does when asked to snapshot a destination.
#[derive(Debug, Clone)]
pub enum MockBehaviour {
    /// Emit some progress, then succeed.
    Succeed { files: u64, bytes: u64, warnings: Vec<String> },
    /// Fail every time with this error.
    Fail(ExecutorError),
    /// Fail `remaining` more times, then succeed. Used to prove that transient
    /// failures are retried and permanent ones are not.
    FailThenSucceed { remaining: u32, error: ExecutorError },
    /// Behave like a well-mannered driver: emit progress, then return as soon
    /// as the cancel token fires.
    BlockUntilCancelled,
    /// Behave like a *badly* mannered driver: ignore the cancel token forever.
    /// Exercises the runner's cancel grace.
    HangForever,
}

impl MockBehaviour {
    /// The default: a small successful snapshot.
    pub fn ok() -> MockBehaviour {
        MockBehaviour::Succeed { files: 10, bytes: 1024, warnings: Vec::new() }
    }
}

/// One recorded snapshot call.
#[derive(Debug, Clone)]
pub struct MockCall {
    pub run_id: Uuid,
    pub job_id: Uuid,
    pub destination_id: Uuid,
    pub attempt: u32,
    pub sources: Vec<PathBuf>,
    pub bandwidth: ResolvedBandwidth,
}

/// One recorded replication call.
#[derive(Debug, Clone)]
pub struct MockReplication {
    pub run_id: Uuid,
    pub job_id: Uuid,
    /// The replica being written.
    pub destination_id: Uuid,
    /// The destination its blobs were copied from.
    pub source_id: Uuid,
    pub attempt: u32,
    pub dry_run: bool,
    pub bandwidth: ResolvedBandwidth,
}

#[derive(Debug, Default)]
struct MockState {
    default: Option<MockBehaviour>,
    per_destination: HashMap<Uuid, MockBehaviour>,
    calls: Vec<MockCall>,
    replications: Vec<MockReplication>,
    prepares: Vec<Uuid>,
    prepare_error: Option<ExecutorError>,
    /// Destination ids in the order any work was started against them, whether
    /// a snapshot or a replication. This is what a chain-ordering test asserts.
    order: Vec<Uuid>,
}

/// A scriptable [`BackupExecutor`].
#[derive(Debug, Clone, Default)]
pub struct MockExecutor {
    state: Arc<Mutex<MockState>>,
}

impl MockExecutor {
    /// A mock that succeeds for every destination.
    pub fn new() -> MockExecutor {
        let mock = MockExecutor::default();
        mock.set_default(MockBehaviour::ok());
        mock
    }

    /// Behaviour for destinations with no specific script.
    pub fn set_default(&self, behaviour: MockBehaviour) {
        if let Ok(mut state) = self.state.lock() {
            state.default = Some(behaviour);
        }
    }

    /// Behaviour for one destination.
    pub fn set_for(&self, destination: Uuid, behaviour: MockBehaviour) {
        if let Ok(mut state) = self.state.lock() {
            state.per_destination.insert(destination, behaviour);
        }
    }

    /// Make `prepare` fail, to exercise the "cannot even connect" path.
    pub fn fail_prepare(&self, error: ExecutorError) {
        if let Ok(mut state) = self.state.lock() {
            state.prepare_error = Some(error);
        }
    }

    /// Every snapshot call made so far, in order.
    pub fn calls(&self) -> Vec<MockCall> {
        self.state.lock().map(|s| s.calls.clone()).unwrap_or_default()
    }

    /// Every replication call made so far, in order.
    pub fn replications(&self) -> Vec<MockReplication> {
        self.state.lock().map(|s| s.replications.clone()).unwrap_or_default()
    }

    /// Destination ids in the order work actually started against them,
    /// snapshots and replications together. A chained job asserts on this.
    pub fn order(&self) -> Vec<Uuid> {
        self.state.lock().map(|s| s.order.clone()).unwrap_or_default()
    }

    /// How many times a destination was attempted, by either route.
    pub fn attempts(&self, destination: Uuid) -> usize {
        self.state
            .lock()
            .map(|s| {
                s.calls.iter().filter(|c| c.destination_id == destination).count()
                    + s.replications.iter().filter(|r| r.destination_id == destination).count()
            })
            .unwrap_or(0)
    }

    /// Destination ids passed to `prepare`, in order.
    pub fn prepares(&self) -> Vec<Uuid> {
        self.state.lock().map(|s| s.prepares.clone()).unwrap_or_default()
    }

    /// Take the behaviour for a destination, decrementing a
    /// `FailThenSucceed` counter as a side effect.
    fn next_behaviour(&self, destination: Uuid) -> MockBehaviour {
        let Ok(mut state) = self.state.lock() else { return MockBehaviour::ok() };
        let entry = state.per_destination.get_mut(&destination);
        if let Some(behaviour) = entry {
            if let MockBehaviour::FailThenSucceed { remaining, error } = behaviour {
                if *remaining > 0 {
                    *remaining -= 1;
                    return MockBehaviour::Fail(error.clone());
                }
                return MockBehaviour::ok();
            }
            return behaviour.clone();
        }
        match &mut state.default {
            Some(MockBehaviour::FailThenSucceed { remaining, error }) => {
                if *remaining > 0 {
                    *remaining -= 1;
                    let error = error.clone();
                    return MockBehaviour::Fail(error);
                }
                MockBehaviour::ok()
            }
            Some(other) => other.clone(),
            None => MockBehaviour::ok(),
        }
    }
}

impl BackupExecutor for MockExecutor {
    fn prepare<'a>(
        &'a self,
        request: PrepareRequest,
    ) -> BoxFuture<'a, ExecutorResult<PrepareOutcome>> {
        Box::pin(async move {
            let error = {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| ExecutorError::new(crate::ErrorCode::Internal, "poisoned"))?;
                state.prepares.push(request.destination.id);
                state.prepare_error.clone()
            };
            match error {
                Some(e) => Err(e),
                None => Ok(PrepareOutcome {
                    created: false,
                    backend_version: Some("mock 1.0".into()),
                    warnings: Vec::new(),
                }),
            }
        })
    }

    fn snapshot<'a>(
        &'a self,
        request: SnapshotRequest,
    ) -> BoxFuture<'a, ExecutorResult<SnapshotOutcome>> {
        Box::pin(async move {
            if let Ok(mut state) = self.state.lock() {
                state.order.push(request.destination.id);
                state.calls.push(MockCall {
                    run_id: request.run_id,
                    job_id: request.job_id,
                    destination_id: request.destination.id,
                    attempt: request.attempt,
                    sources: request.sources.iter().map(|s| s.path.clone()).collect(),
                    bandwidth: request.bandwidth,
                });
            }
            match self.next_behaviour(request.destination.id) {
                MockBehaviour::Succeed { files, bytes, warnings } => {
                    // A real driver emits far more than this; the sink
                    // coalesces either way.
                    for i in 0..files {
                        request.progress.update(Progress {
                            files_processed: i + 1,
                            bytes_processed: bytes * (i + 1) / files.max(1),
                            ..Default::default()
                        });
                    }
                    let progress = Progress {
                        files_processed: files,
                        files_total: Some(files),
                        bytes_processed: bytes,
                        bytes_total: Some(bytes),
                        bytes_uploaded: bytes,
                        ..Default::default()
                    };
                    request.progress.finish(progress.clone());
                    Ok(SnapshotOutcome {
                        snapshot_id: Some(format!("mock-{}", Uuid::new_v4().simple())),
                        progress,
                        warnings,
                        notes: Vec::new(),
                    })
                }
                MockBehaviour::Fail(error) => Err(error),
                // Already resolved by `next_behaviour`; reaching here means the
                // counter ran out.
                MockBehaviour::FailThenSucceed { .. } => Ok(SnapshotOutcome::default()),
                MockBehaviour::BlockUntilCancelled => {
                    request.progress.update(Progress {
                        files_processed: 1,
                        bytes_processed: 512,
                        ..Default::default()
                    });
                    request.cancel.cancelled().await;
                    Err(ExecutorError::cancelled())
                }
                MockBehaviour::HangForever => std::future::pending().await,
            }
        })
    }

    fn replicate<'a>(
        &'a self,
        request: ReplicateRequest,
    ) -> BoxFuture<'a, ExecutorResult<ReplicateOutcome>> {
        Box::pin(async move {
            if let Ok(mut state) = self.state.lock() {
                state.order.push(request.destination.id);
                state.replications.push(MockReplication {
                    run_id: request.run_id,
                    job_id: request.job_id,
                    destination_id: request.destination.id,
                    source_id: request.source.id,
                    attempt: request.attempt,
                    dry_run: request.dry_run,
                    bandwidth: request.bandwidth,
                });
            }
            // A rehearsal copies nothing, and says so, because the one thing a
            // dry run must never do is look like a real one.
            if request.dry_run {
                let progress = Progress::default();
                request.progress.finish(progress.clone());
                return Ok(ReplicateOutcome {
                    blobs_copied: 0,
                    bytes_copied: 0,
                    progress,
                    warnings: vec![format!(
                        "Dry run: nothing was copied to \"{}\".",
                        request.destination.name
                    )],
                });
            }
            match self.next_behaviour(request.destination.id) {
                MockBehaviour::Succeed { files, bytes, warnings } => {
                    let progress = Progress {
                        files_processed: files,
                        files_total: Some(files),
                        bytes_processed: bytes,
                        bytes_total: Some(bytes),
                        bytes_uploaded: bytes,
                        ..Default::default()
                    };
                    request.progress.finish(progress.clone());
                    Ok(ReplicateOutcome {
                        blobs_copied: files,
                        bytes_copied: bytes,
                        progress,
                        warnings,
                    })
                }
                MockBehaviour::Fail(error) => Err(error),
                MockBehaviour::FailThenSucceed { .. } => Ok(ReplicateOutcome::default()),
                MockBehaviour::BlockUntilCancelled => {
                    request.progress.update(Progress {
                        files_processed: 1,
                        bytes_processed: 512,
                        ..Default::default()
                    });
                    request.cancel.cancelled().await;
                    Err(ExecutorError::cancelled())
                }
                MockBehaviour::HangForever => std::future::pending().await,
            }
        })
    }

    fn verify<'a>(
        &'a self,
        _request: VerifyRequest,
    ) -> BoxFuture<'a, ExecutorResult<VerifyOutcome>> {
        Box::pin(async move { Ok(VerifyOutcome { blobs_checked: 1, problems: Vec::new() }) })
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A manual job with one source and no destinations.
///
/// Every field is set explicitly rather than through `..Default::default()`,
/// because [`Job`] has no `Default` and because a fixture that silently picks
/// up new fields is a fixture that stops testing them.
pub fn test_job(name: &str) -> Job {
    Job {
        id: Uuid::new_v4(),
        name: name.to_string(),
        project_id: None,
        description: String::new(),
        sources: vec![Source::new("/tmp/source")],
        destination_ids: Vec::new(),
        schedule: Schedule::Manual,
        exclusions: ExclusionSet::default(),
        bandwidth: None,
        retention: None,
        enabled: true,
        timeout_minutes: None,
        hooks: JobHooks::default(),
        continue_on_destination_error: true,
        created_at: chrono::Utc::now(),
        tags: Vec::new(),
    }
}

/// A local-repository destination, which routes through [`BackupExecutor`].
pub fn test_repository(name: &str, path: impl Into<PathBuf>) -> Destination {
    Destination {
        id: Uuid::new_v4(),
        name: name.to_string(),
        kind: DestinationKind::LocalRepository { path: path.into() },
        encryption: None,
        passphrase_ref: None,
        retention: Default::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        replicate_from: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

/// A repository destination that is a replica of `source`.
///
/// Carries no `encryption` and no `passphrase_ref`, because a replica has
/// neither: it is the same kopia repository as its source, reached at a second
/// address. See [`crate::model::Destination::replicate_from`].
pub fn test_replica(name: &str, path: impl Into<PathBuf>, source: Uuid) -> Destination {
    Destination { replicate_from: Some(source), ..test_repository(name, path) }
}

/// A folder-mirror destination, which routes through
/// [`crate::engine::mirror`].
pub fn test_mirror(name: &str, path: impl Into<PathBuf>) -> Destination {
    Destination {
        kind: DestinationKind::LocalMirror { path: path.into() },
        ..test_repository(name, "")
    }
}

/// A [`CancelToken`] that is already cancelled, for tests that need one.
pub fn cancelled_token() -> CancelToken {
    let token = CancelToken::new();
    token.cancel(crate::engine::cancel::CancelReason::Requested);
    token
}
