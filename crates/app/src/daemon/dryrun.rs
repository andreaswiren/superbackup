//! `job.run --dry-run`: report what would be copied, write nothing.
//!
//! ## Why this does not go through the scheduler
//!
//! [`SchedulerHandle::run_now`](superbackup_core::engine::SchedulerHandle::run_now)
//! takes a job and a trigger and nothing else — the engine has no concept of a
//! rehearsal, and the executor is chosen once when the engine is built. So a
//! dry run cannot be expressed as a scheduled run without either giving the
//! scheduler a flag it does not have, or making the *live* executor
//! stateful about whether the current run is a rehearsal, which would be a
//! race waiting to be lost.
//!
//! Instead a dry run builds its own [`Runner`] over a dry-run
//! [`KopiaExecutor`](super::executor::KopiaExecutor) and drives it directly.
//! That reuses everything that makes a real run trustworthy — retry
//! classification, per-destination isolation, progress coalescing, the run
//! record — while the executor underneath refuses to write.
//!
//! ## What it still respects
//!
//! The engine's "one run per job" invariant is the scheduler's to enforce, and
//! a run started outside it would break that. So this refuses to start when
//! the job is already running, which is the same answer the scheduler gives.
//! It also refuses while the vault is locked, because a dry run against a
//! repository still has to connect to it.
//!
//! ## Every destination kind is rehearsed, including folder mirrors
//!
//! `RunRequest::dry_run` is the authority, not the choice of executor, and
//! that distinction is load-bearing.
//! [`Runner`](superbackup_core::engine::Runner) branches on
//! `DestinationKind` itself and drives
//! [`MirrorEngine`](superbackup_core::engine::MirrorEngine) directly for a
//! mirror, so an executor-level flag could never have suppressed a mirror
//! copy — a "rehearsal" would have written the files for real. With the flag
//! on the request, the runner forwards it to
//! `MirrorOptions::dry_run` and to `SnapshotRequest::dry_run`, and the
//! guarantee holds for every destination kind: no directory created, no file
//! copied, nothing pruned, and the counts still reported.

use std::sync::Arc;

use superbackup_core::engine::runner::{destination_is_usable, RunRequest, Runner};
use superbackup_core::engine::EVENT_CHANNEL_CAPACITY;
use superbackup_core::ipc::protocol::StartedReply;
use superbackup_core::model::Job;
use superbackup_core::state::Trigger;
use superbackup_core::{Error, Result};
use uuid::Uuid;

use super::events::pump_engine_events;
use super::runtime::Runtime;

/// Start a dry run and return as soon as it is accepted.
pub async fn start(runtime: &Arc<Runtime>, job: Job) -> Result<StartedReply> {
    if runtime.active_runs().iter().any(|r| r.job_id == job.id) {
        return Err(Error::JobRunning(job.name.clone()));
    }

    let (config, destinations) = {
        let store = runtime.store.lock().await;
        let config = store.config().clone();
        // Every kind, mirrors included: `RunRequest::dry_run` reaches the
        // mirror engine, so there is nothing left to exclude.
        let destinations: Vec<Arc<superbackup_core::model::Destination>> = job
            .destination_ids
            .iter()
            .filter_map(|id| config.destination(id))
            .filter(|d| destination_is_usable(d))
            .map(|d| Arc::new(d.clone()))
            .collect();
        (config, destinations)
    };
    if destinations.is_empty() {
        return Err(Error::Validation(format!(
            "\"{}\" has no usable destination to rehearse against",
            job.name
        )));
    }

    let clock = Arc::new(superbackup_core::engine::SystemClock::new());
    let executor = Arc::new(super::executor::dry_run(Arc::clone(runtime), clock.clone()));

    // Its own event channel, pumped into the same places the engine's is, so a
    // dry run shows up in the GUI's progress view exactly like a real one.
    let (events, receiver) = tokio::sync::broadcast::channel(EVENT_CHANNEL_CAPACITY);
    pump_engine_events(Arc::clone(runtime), receiver);

    let runner = Runner::new(
        executor,
        clock,
        Arc::new(chrono::Local),
        events.clone(),
        Arc::clone(&runtime.persisted),
    );

    let run_id = Uuid::new_v4();
    let request = RunRequest {
        run_id,
        job: Arc::new(job.clone()),
        destinations,
        settings: Arc::new(config.settings.clone()),
        // `Preview`, not `Manual`. The run lands in the history like any
        // other, and a history that cannot tell a rehearsal from a real backup
        // would let a user believe a destination holds data it does not.
        trigger: Trigger::Preview,
        dry_run: true,
        // A child of the engine's token where there is one, so shutting the
        // daemon down stops a rehearsal too.
        cancel: runtime.scheduler().map(|s| s.cancel_token().child()).unwrap_or_default(),
    };

    let name = job.name.clone();
    tokio::spawn(async move {
        let run = runner.execute(request).await;
        tracing::info!(job = %name, status = ?run.status, "dry run finished");
        // Keep the channel alive until the run is over; dropping the sender
        // early would end the pump before the terminal event.
        drop(events);
    });

    Ok(StartedReply {
        run_id,
        started: true,
        note: Some(format!(
            "Dry run: \"{}\" will report what it would copy and write nothing.",
            job.name
        )),
    })
}
