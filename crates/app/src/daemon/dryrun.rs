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
//! ## Folder mirrors cannot be rehearsed, and are left out rather than faked
//!
//! [`Runner::attempt_destination`](superbackup_core::engine::Runner) branches
//! on `DestinationKind` *itself* and drives
//! [`MirrorEngine`](superbackup_core::engine::MirrorEngine) directly for a
//! mirror — the executor is never consulted. A dry-run executor therefore
//! cannot suppress a mirror copy: the runner would copy the files for real
//! and the "rehearsal" would have written to the user's disk.
//!
//! Rather than lie about that, mirror destinations are excluded from the
//! rehearsal and the reply says how many were skipped. Reporting a dry run
//! that quietly copied gigabytes would be far worse than reporting one that
//! covered fewer destinations than the real thing will.

use std::sync::Arc;

use superbackup_core::engine::runner::{destination_is_usable, RunRequest, Runner};
use superbackup_core::engine::EVENT_CHANNEL_CAPACITY;
use superbackup_core::ipc::protocol::StartedReply;
use superbackup_core::model::{DestinationKind, Job};
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

    let (config, destinations, skipped_mirrors) = {
        let store = runtime.store.lock().await;
        let config = store.config().clone();
        let usable: Vec<&superbackup_core::model::Destination> = job
            .destination_ids
            .iter()
            .filter_map(|id| config.destination(id))
            .filter(|d| destination_is_usable(d))
            .collect();
        let skipped: Vec<String> = usable
            .iter()
            .filter(|d| matches!(d.kind, DestinationKind::LocalMirror { .. }))
            .map(|d| d.name.clone())
            .collect();
        let destinations: Vec<Arc<superbackup_core::model::Destination>> = usable
            .into_iter()
            .filter(|d| !matches!(d.kind, DestinationKind::LocalMirror { .. }))
            .map(|d| Arc::new(d.clone()))
            .collect();
        (config, destinations, skipped)
    };
    if destinations.is_empty() {
        return Err(Error::Validation(if skipped_mirrors.is_empty() {
            format!("\"{}\" has no usable destination to rehearse against", job.name)
        } else {
            format!(
                "\"{}\" writes only to folder mirrors ({}), which cannot be rehearsed without \
                 copying the files for real. Run it normally instead.",
                job.name,
                skipped_mirrors.join(", ")
            )
        }));
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
        trigger: Trigger::Manual,
        // A child of the engine's token where there is one, so shutting the
        // daemon down stops a rehearsal too.
        cancel: runtime
            .scheduler()
            .map(|s| s.cancel_token().child())
            .unwrap_or_default(),
    };

    let name = job.name.clone();
    tokio::spawn(async move {
        let run = runner.execute(request).await;
        tracing::info!(job = %name, status = ?run.status, "dry run finished");
        // Keep the channel alive until the run is over; dropping the sender
        // early would end the pump before the terminal event.
        drop(events);
    });

    let mut note = format!(
        "Dry run: \"{}\" will report what it would copy and write nothing.",
        job.name
    );
    if !skipped_mirrors.is_empty() {
        note.push_str(&format!(
            " {} folder mirror{} ({}) cannot be rehearsed and {} left out.",
            skipped_mirrors.len(),
            if skipped_mirrors.len() == 1 { "" } else { "s" },
            skipped_mirrors.join(", "),
            if skipped_mirrors.len() == 1 { "was" } else { "were" }
        ));
    }
    Ok(StartedReply { run_id, started: true, note: Some(note) })
}
