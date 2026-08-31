//! The engine event pump, and the event log writer.
//!
//! ```text
//!   engine ──broadcast<EngineEvent>──▶ pump ──┬──▶ Runtime live-run tracking
//!                                             ├──▶ broadcast<StreamItem>  (IPC, tray)
//!                                             ├──▶ events.ndjson          (history)
//!                                             └──▶ platform::notify       (toasts)
//! ```
//!
//! One subscriber, four consequences. The alternative — every consumer
//! subscribing to the engine directly — means four places that each have to
//! remember to update the live-run map, and four subscribers that can lag
//! independently and disagree about what is running.
//!
//! ## Lag is expected and handled
//!
//! The engine's channel is lossy by design: a slow consumer misses frames
//! rather than throttling a backup. When this pump lags it says so on the IPC
//! stream ([`StreamItem::Lagged`]) and then *resynchronises* by publishing a
//! full status snapshot, because the correct reaction to "you missed some
//! progress" is to redraw from the truth, not to try to recover the lost
//! frames.

use std::sync::Arc;

use superbackup_core::engine::{EngineEvent, SkipReason};
use superbackup_core::ipc::StreamItem;
use superbackup_core::platform::{Notification, NotificationKind, NotifyOutcome};
use superbackup_core::state::{Event, JobRun, RunStatus, Severity};
use tokio::sync::broadcast::error::RecvError;

use super::runtime::Runtime;

/// Notifications are suppressed for this long after start-up.
///
/// A cold start with three stale jobs must not carpet-bomb the user with
/// toasts before the window has even drawn. Specified in `UX_SPEC.md` §15.3.
const NOTIFICATION_GRACE_SECONDS: u64 = 30;

/// Subscribe to an engine event channel and fan it out. Returns the task
/// handle so the daemon can wait for it during shutdown.
pub fn pump_engine_events(
    runtime: Arc<Runtime>,
    mut events: tokio::sync::broadcast::Receiver<EngineEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => handle(&runtime, event).await,
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "the engine event pump fell behind");
                    runtime.publish(StreamItem::Lagged { missed });
                    // Resynchronise rather than pretend: a client that missed
                    // a RunFinished would otherwise show a run as live for ever.
                    runtime.publish_status().await;
                }
                // The engine has gone; so should we.
                Err(RecvError::Closed) => return,
            }
        }
    })
}

/// Refresh this machine's record at a destination, off the pump's thread.
///
/// Hooked to `DestinationStarted` rather than to the executor because that is
/// the one signal every destination kind produces: the runner drives folder
/// mirrors and replicas itself and never calls `BackupExecutor::prepare` for
/// them, so an executor-side hook would quietly cover only kopia repositories
/// — which is exactly the drive a user is most likely to be looking at with a
/// torch during a recovery.
///
/// Fire-and-forget on purpose. This pump also delivers progress and terminal
/// events for live backups, and a slow or stalled network share must not be
/// able to hold it up. The manifest is a convenience for a future human, never
/// part of the data path, so a failure becomes one activity-log line and
/// nothing else. See [`super::manifest`].
fn leave_machine_manifest(runtime: &Arc<Runtime>, run_id: uuid::Uuid, destination_id: uuid::Uuid) {
    let runtime = Arc::clone(runtime);
    tokio::spawn(async move {
        // A rehearsal writes nothing anywhere, including this.
        let rehearsal = runtime
            .active_runs()
            .iter()
            .find(|r| r.run_id == run_id)
            .map(|r| r.trigger.is_rehearsal())
            .unwrap_or(false);

        let (destination, identity, settings) = {
            let store = runtime.store.lock().await;
            let config = store.config();
            let Some(destination) = config.destination(&destination_id).cloned() else {
                return;
            };
            (destination, config.machine.clone(), config.settings.clone())
        };

        let outcome =
            super::manifest::write_for_destination(&destination, &identity, &settings, rehearsal)
                .await;
        if let Some(warning) = outcome.warning() {
            runtime.record_event(
                Event::new(Severity::Warning, "dest.manifest_failed", warning)
                    .with_destination(destination_id),
            );
        }
    });
}

async fn handle(runtime: &Arc<Runtime>, event: EngineEvent) {
    match event {
        EngineEvent::RunQueued { run_id, job_id, job_name } => {
            // Tracked from the moment it is queued, so `job.stop` on a run
            // that has not started yet still finds its job.
            runtime.set_active(JobRun {
                run_id,
                job_id,
                job_name,
                trigger: superbackup_core::state::Trigger::Schedule,
                status: RunStatus::Queued,
                started_at: chrono::Utc::now(),
                finished_at: None,
                destinations: Vec::new(),
            });
            runtime.publish_status().await;
        }

        EngineEvent::RunStarted { run } => {
            runtime.set_active(run.clone());
            runtime.publish_status().await;
        }

        EngineEvent::DestinationStarted { run_id, job_id, destination_id } => {
            runtime.publish(StreamItem::Progress {
                run_id,
                job_id,
                destination_id,
                status: RunStatus::Preparing,
                progress: Box::default(),
            });
            leave_machine_manifest(runtime, run_id, destination_id);
        }

        EngineEvent::DestinationRetrying {
            job_id,
            destination_id,
            attempt,
            retry_in_seconds,
            ..
        } => {
            runtime.record_event(
                Event::new(
                    Severity::Warning,
                    "destination.retrying",
                    format!(
                        "A destination failed and will be retried in {retry_in_seconds} seconds \
                         (attempt {attempt})."
                    ),
                )
                .with_job(job_id)
                .with_destination(destination_id),
            );
        }

        EngineEvent::DestinationFinished { run_id, job_id, destination } => {
            runtime.publish(StreamItem::Progress {
                run_id,
                job_id,
                destination_id: destination.destination_id,
                status: destination.status,
                progress: Box::new(destination.progress.clone()),
            });
            runtime.apply_destination_finished(&run_id, *destination);
        }

        EngineEvent::Progress(update) => {
            runtime.apply_progress(
                &update.run_id,
                &update.destination_id,
                &update.progress,
                update.final_update,
            );
            runtime.publish(StreamItem::Progress {
                run_id: update.run_id,
                job_id: update.job_id,
                destination_id: update.destination_id,
                status: if update.final_update {
                    RunStatus::Finalising
                } else {
                    RunStatus::Running
                },
                progress: Box::new(update.progress),
            });
        }

        EngineEvent::HookFinished { job_id, outcome, .. } => {
            if !outcome.succeeded() {
                runtime.record_event(
                    Event::new(
                        Severity::Warning,
                        "hook.failed",
                        format!("A job hook did not succeed: {}", outcome.summary()),
                    )
                    .with_job(job_id),
                );
            }
        }

        EngineEvent::RunFinished { run } => {
            runtime.clear_active(&run.run_id);
            finish(runtime, &run).await;
            runtime.publish_status().await;
        }

        EngineEvent::RunSkipped { job_id, job_name, reason } => {
            // The scheduler drains blocked runs rather than queueing them, so
            // the daemon has to remember this one itself if the vault is the
            // reason. See `Runtime::blocked_by_lock`.
            if reason == SkipReason::VaultLocked {
                runtime.note_blocked_by_lock(job_id);
                notify(
                    runtime,
                    Notification::new(
                        NotificationKind::ServiceError,
                        "A backup was skipped",
                        format!("\"{job_name}\" was due. Unlock superbackup to run it."),
                    )
                    .with_job(job_id),
                )
                .await;
            }
            // Everything else is activity-log material; the scheduler already
            // emits an `EngineEvent::Log` for it, so nothing is added here.
            runtime.publish_status().await;
        }

        EngineEvent::NextRunChanged { .. } => {
            runtime.publish_status().await;
        }

        EngineEvent::Log(event) => {
            runtime.record_event(*event);
        }
    }
}

/// The activity line and the notification a finished run deserves.
async fn finish(runtime: &Arc<Runtime>, run: &JobRun) {
    let failed: Vec<&superbackup_core::state::DestinationRun> =
        run.destinations.iter().filter(|d| d.status == RunStatus::Failed).collect();
    let succeeded = run.destinations.len() - failed.len();

    let severity = match run.status {
        RunStatus::Failed => Severity::Error,
        RunStatus::SucceededWithWarnings | RunStatus::Cancelled => Severity::Warning,
        _ => Severity::Info,
    };
    let uploaded: u64 = run.destinations.iter().map(|d| d.progress.bytes_uploaded).sum();
    let files: u64 = run.destinations.iter().map(|d| d.progress.files_processed).max().unwrap_or(0);
    let message = match run.status {
        RunStatus::Succeeded => format!(
            "\"{}\" finished: {files} files, {} uploaded{}.",
            run.job_name,
            bytes(uploaded),
            duration_suffix(run)
        ),
        RunStatus::SucceededWithWarnings => format!(
            "\"{}\" finished with warnings: {files} files, {} uploaded.",
            run.job_name,
            bytes(uploaded)
        ),
        RunStatus::Cancelled => format!("\"{}\" was stopped.", run.job_name),
        RunStatus::Failed => match failed.first().and_then(|d| d.error.as_ref()) {
            Some(error) => format!("\"{}\" failed: {}", run.job_name, error.message),
            None => format!("\"{}\" failed.", run.job_name),
        },
        _ => format!("\"{}\" ended as {}.", run.job_name, run.status.title()),
    };
    runtime.record_event(
        Event::new(severity, "job.finished", message)
            .with_job(run.job_id)
            .with_run(run.run_id)
            .with_field("status", run.status.title()),
    );

    // Recovery is news even when success notifications are off, so the dedupe
    // history for this job is cleared on any success — see `UX_SPEC.md` §15.3.
    if matches!(run.status, RunStatus::Succeeded | RunStatus::SucceededWithWarnings) {
        runtime.notifier.subject_recovered(&run.job_id);
    }

    let notification = match run.status {
        RunStatus::Failed => {
            let error = failed.first().and_then(|d| d.error.as_ref());
            let mut n = Notification::new(
                NotificationKind::Failure,
                format!("Backup failed: {}", run.job_name),
                error
                    .map(|e| truncate(&e.message, 120))
                    .unwrap_or_else(|| "The backup did not complete.".into()),
            )
            .with_job(run.job_id);
            if let Some(code) = error.map(|e| e.code) {
                n = n.with_error_code(code);
            }
            Some(n)
        }
        // "Succeeded to 2 of 3 destinations" is a failure the user must see
        // even though the run as a whole is recorded as a success.
        RunStatus::SucceededWithWarnings if !failed.is_empty() => Some(
            Notification::new(
                NotificationKind::Failure,
                format!("Backup finished with problems: {}", run.job_name),
                format!(
                    "Succeeded to {succeeded} of {} destinations. {} failed.",
                    run.destinations.len(),
                    failed
                        .iter()
                        .map(|d| d.destination_name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with_job(run.job_id),
        ),
        RunStatus::Succeeded | RunStatus::SucceededWithWarnings => Some(
            Notification::new(
                NotificationKind::Success,
                format!("Backup finished: {}", run.job_name),
                format!("{files} files · {} uploaded{}", bytes(uploaded), duration_suffix(run)),
            )
            .with_job(run.job_id),
        ),
        // Stopped, skipped and queued are deliberately silent: the user asked
        // for it, or policy did, and both belong in Activity.
        _ => None,
    };
    if let Some(notification) = notification {
        notify(runtime, notification).await;
    }
}

/// Show a notification, honouring the start-up grace period.
async fn notify(runtime: &Arc<Runtime>, notification: Notification) {
    if runtime.uptime_seconds() < NOTIFICATION_GRACE_SECONDS {
        tracing::debug!(
            title = %notification.title,
            "notification suppressed during the start-up grace period"
        );
        return;
    }
    let outcome = runtime.notifier.notify(&notification);
    if let NotifyOutcome::Deduped { seconds_remaining } = outcome {
        tracing::debug!(seconds_remaining, "notification deduplicated");
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    // By characters, not bytes: slicing a multi-byte codepoint would panic,
    // and this text comes from kopia.
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn bytes(n: u64) -> String {
    bytesize_like(n)
}

/// Human byte sizes without pulling a formatting crate into this crate.
fn bytesize_like(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn duration_suffix(run: &JobRun) -> String {
    match run.duration_seconds() {
        Some(secs) if secs >= 60 => format!(" in {}m {}s", secs / 60, secs % 60),
        Some(secs) => format!(" in {secs}s"),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// The event log
// ---------------------------------------------------------------------------

/// Rotate `events.ndjson` once it passes this size.
const EVENT_LOG_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Spawn the writer for `events.ndjson` and register its sender.
///
/// Behind a channel rather than written inline because [`Runtime::record_event`]
/// is called from the engine's hot path and from IPC handlers, and neither may
/// be made to wait on a disk write. A dropped line under extreme pressure is
/// acceptable; a stalled backup is not.
pub fn spawn_event_log(runtime: Arc<Runtime>) -> tokio::task::JoinHandle<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    runtime.set_event_log(tx);
    let path = runtime.paths.event_log();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let path = path.clone();
            let written = tokio::task::spawn_blocking(move || append(&path, &event)).await;
            if let Ok(Err(e)) = written {
                tracing::debug!(error = %e, "could not append to the event log");
            }
        }
    })
}

fn append(path: &std::path::Path, event: &Event) -> std::io::Result<()> {
    use std::io::Write;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > EVENT_LOG_MAX_BYTES {
            // One generation only. The activity log is a convenience; the
            // durable record is `state.json`, which is bounded by its own
            // history cap.
            let _ = std::fs::rename(path, path.with_extension("ndjson.1"));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_vec(event)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push(b'\n');
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_sizes_read_like_a_file_manager() {
        assert_eq!(bytesize_like(512), "512 B");
        assert_eq!(bytesize_like(1024), "1.0 KB");
        assert_eq!(bytesize_like(1024 * 1024 * 3 / 2), "1.5 MB");
    }

    #[test]
    fn truncation_never_splits_a_codepoint() {
        let text = "é".repeat(200);
        let out = truncate(&text, 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
        assert_eq!(truncate("short", 10), "short");
    }

    #[test]
    fn the_event_log_rotates_once_it_is_too_big() {
        let dir = std::env::temp_dir().join(format!("sb-eventlog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("events.ndjson");
        std::fs::write(&path, vec![b'x'; (EVENT_LOG_MAX_BYTES + 1) as usize]).expect("seed");
        append(&path, &Event::info("test", "hello")).expect("append");
        assert!(path.with_extension("ndjson.1").exists(), "the old log must be kept");
        let fresh = std::fs::read_to_string(&path).expect("read");
        assert!(fresh.contains("hello"));
        assert!(fresh.len() < 1024, "the new log starts empty");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
