//! The seam between the engine and whatever actually moves the bytes.
//!
//! The engine never spawns `kopia`, never parses its JSON, and never knows
//! that it exists. It knows only [`BackupExecutor`]: "prepare this
//! destination", "snapshot these sources into it", "verify it". The real
//! implementation lives in [`crate::kopia`]; the tests use
//! [`crate::engine::testing::MockExecutor`].
//!
//! ## Why a trait and not a concrete driver
//!
//! Three reasons, in order of how much they hurt when ignored:
//!
//! 1. Every interesting behaviour of the runner — retry classification,
//!    cancellation promptness, partial-destination failure, progress
//!    coalescing, timeout — is a behaviour *around* the driver. Testing them
//!    through a real subprocess means either a kopia binary in CI or no tests.
//! 2. `LocalMirror` destinations have no kopia at all
//!    ([`crate::engine::mirror`] implements them), and the runner must treat
//!    both kinds identically.
//! 3. The driver is written by a different workstream on a different
//!    schedule.
//!
//! ## Contract an implementation must honour
//!
//! * **Cancellation.** Every method takes a [`CancelToken`]. When it fires the
//!   implementation must abandon its work and return within roughly one
//!   second, and before returning it must have *killed its child process* and
//!   *released any repository lock it holds*. Returning while a `kopia`
//!   process is still writing is the one failure the engine cannot recover
//!   from: the next run will block on a stale lock. Returning
//!   `Err(ExecutorError::cancelled())` is the expected outcome; returning
//!   `Ok` after a cancellation is also accepted (the work finished first) and
//!   the runner will still mark the run cancelled.
//! * **Progress.** Send [`Progress`] snapshots to [`ProgressSink`] as often as
//!   is convenient — the sink coalesces, so a 10 kHz stream costs nothing.
//!   Each snapshot is absolute, not a delta.
//! * **Errors.** Classify failures with [`Retryable`] where the driver knows
//!   better than the engine (a 503 from S3 is transient; a bad passphrase is
//!   not). [`Retryable::Unknown`] is fine: the engine falls back to a
//!   heuristic over [`ErrorCode`] and the message.
//! * **Secrets.** Requests carry [`SecretRef`] handles, never plaintext. The
//!   driver resolves them against the unlocked vault itself and passes them to
//!   kopia via environment or stdin, never argv.
//! * **Blocking.** Methods are async and must not block the runtime; wrap
//!   synchronous work in `spawn_blocking`.

use crate::engine::cancel::CancelToken;
use crate::engine::clock::BoxFuture;
use crate::engine::throttle::ResolvedBandwidth;
use crate::error::ErrorCode;
use crate::model::{Destination, ExclusionSet, RetentionPolicy, Source};
use crate::state::{Progress, RunError};
use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

/// Whether a failure is worth trying again.
///
/// The distinction is the difference between a backup tool that survives a
/// flaky hotel Wi-Fi and one that retries a wrong password five times and then
/// locks the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retryable {
    /// Network, rate-limit, lock contention, transient storage error.
    Transient,
    /// Wrong passphrase, missing path, invalid configuration, cancelled.
    /// Retrying produces the same failure and wastes the user's time.
    Permanent,
    /// The driver has no opinion; the engine classifies it heuristically.
    Unknown,
}

/// A failure from an executor, in the shape the run history stores.
///
/// This is a distinct type from [`crate::Error`] because it carries the retry
/// classification and because it must be `Clone` — the same failure is written
/// to a [`crate::state::DestinationRun`], emitted as an event, and possibly
/// shown in a notification.
#[derive(Debug, Clone)]
pub struct ExecutorError {
    pub code: ErrorCode,
    /// One sentence, safe for a notification. Must not contain credentials.
    pub message: String,
    /// What the user can do about it.
    pub hint: Option<String>,
    /// Trimmed, already-redacted tail of the driver's stderr.
    pub detail: Option<String>,
    pub retryable: Retryable,
}

impl ExecutorError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ExecutorError {
        ExecutorError {
            code,
            message: message.into(),
            hint: None,
            detail: None,
            retryable: Retryable::Unknown,
        }
    }

    /// Mark a failure as worth retrying.
    pub fn transient(mut self) -> ExecutorError {
        self.retryable = Retryable::Transient;
        self
    }

    /// Mark a failure as not worth retrying.
    pub fn permanent(mut self) -> ExecutorError {
        self.retryable = Retryable::Permanent;
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> ExecutorError {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> ExecutorError {
        self.detail = Some(detail.into());
        self
    }

    /// The canonical "the token fired" error.
    pub fn cancelled() -> ExecutorError {
        ExecutorError::new(ErrorCode::JobCancelled, "cancelled").permanent()
    }

    pub fn is_cancellation(&self) -> bool {
        self.code == ErrorCode::JobCancelled
    }

    /// Convert into the persisted shape.
    pub fn to_run_error(&self) -> RunError {
        RunError {
            code: self.code,
            message: self.message.clone(),
            hint: self.hint.clone(),
            detail: self.detail.clone(),
            occurred_at: Utc::now(),
        }
    }
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ExecutorError {}

impl From<crate::Error> for ExecutorError {
    fn from(e: crate::Error) -> ExecutorError {
        let hint = e.hint().map(|h| h.to_string());
        ExecutorError {
            code: e.code(),
            message: e.to_string(),
            hint,
            detail: None,
            retryable: Retryable::Unknown,
        }
    }
}

pub type ExecutorResult<T> = std::result::Result<T, ExecutorError>;

// ---------------------------------------------------------------------------
// Progress plumbing
// ---------------------------------------------------------------------------

/// Where an executor pushes live progress.
///
/// The sink is *coalescing*: it forwards at most [`ProgressSink::MAX_HZ`]
/// updates per second and silently drops the ones in between, because a kopia
/// snapshot of a source tree emits progress far faster than any GUI can
/// render, and a broadcast channel full of stale progress is how a tray icon
/// ends up several minutes behind reality.
///
/// The final state is never dropped: [`ProgressSink::finish`] always emits,
/// regardless of the rate limit, and the runner calls it for every destination
/// on every path out — success, failure, and cancellation alike.
#[derive(Clone)]
pub struct ProgressSink {
    inner: Arc<ProgressSinkInner>,
}

struct ProgressSinkInner {
    tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
    run_id: Uuid,
    job_id: Uuid,
    destination_id: Uuid,
    clock: Arc<dyn crate::engine::clock::Clock>,
    /// Guarded by a `std::sync::Mutex` deliberately: the critical section is a
    /// timestamp comparison with no `await` in it, so a std mutex is both
    /// faster and impossible to hold across a yield point.
    gate: std::sync::Mutex<RateGate>,
}

#[derive(Debug)]
struct RateGate {
    last_emit: Option<chrono::DateTime<Utc>>,
}

/// One coalesced progress observation, addressed to a destination within a run.
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub run_id: Uuid,
    pub job_id: Uuid,
    pub destination_id: Uuid,
    pub progress: Progress,
    /// True for the terminal update of a destination. Consumers may use it to
    /// stop animating without waiting for the run-finished event.
    pub final_update: bool,
}

impl std::fmt::Debug for ProgressSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressSink")
            .field("run_id", &self.inner.run_id)
            .field("destination_id", &self.inner.destination_id)
            .finish()
    }
}

impl ProgressSink {
    /// Maximum forwarded updates per second, per destination.
    pub const MAX_HZ: u32 = 10;

    const MIN_INTERVAL_MS: i64 = 1000 / Self::MAX_HZ as i64;

    /// Build a sink over a channel of updates.
    ///
    /// Public because [`crate::engine::mirror::MirrorEngine`] can be driven
    /// directly (and is, by the tests), not only through the runner.
    pub fn new(
        tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
        run_id: Uuid,
        job_id: Uuid,
        destination_id: Uuid,
        clock: Arc<dyn crate::engine::clock::Clock>,
    ) -> ProgressSink {
        ProgressSink {
            inner: Arc::new(ProgressSinkInner {
                tx,
                run_id,
                job_id,
                destination_id,
                clock,
                gate: std::sync::Mutex::new(RateGate { last_emit: None }),
            }),
        }
    }

    /// Offer a progress snapshot. Cheap, non-blocking, and safe to call from a
    /// tight loop; most calls do nothing but read a clock.
    pub fn update(&self, progress: Progress) {
        let now = self.inner.clock.now_utc();
        let should_emit = match self.inner.gate.lock() {
            Ok(mut gate) => {
                let due = match gate.last_emit {
                    None => true,
                    Some(prev) => (now - prev).num_milliseconds() >= Self::MIN_INTERVAL_MS,
                };
                if due {
                    gate.last_emit = Some(now);
                }
                due
            }
            // A poisoned gate means a caller panicked mid-update. Losing the
            // rate limit is strictly better than losing progress entirely.
            Err(_) => true,
        };
        if should_emit {
            self.emit(progress, false);
        }
    }

    /// Emit the terminal state, bypassing the rate limit.
    pub fn finish(&self, progress: Progress) {
        if let Ok(mut gate) = self.inner.gate.lock() {
            gate.last_emit = Some(self.inner.clock.now_utc());
        }
        self.emit(progress, true);
    }

    fn emit(&self, progress: Progress, final_update: bool) {
        // A closed receiver means the run is being torn down; dropping the
        // update is correct and must not fail the backup.
        let _ = self.inner.tx.send(ProgressUpdate {
            run_id: self.inner.run_id,
            job_id: self.inner.job_id,
            destination_id: self.inner.destination_id,
            progress,
            final_update,
        });
    }
}

// ---------------------------------------------------------------------------
// Requests and outcomes
// ---------------------------------------------------------------------------

/// Everything an executor needs to make one destination ready to receive a
/// snapshot: connect to the repository, creating it if it does not exist, and
/// apply the retention policy.
#[derive(Debug, Clone)]
pub struct PrepareRequest {
    pub run_id: Uuid,
    pub destination: Arc<Destination>,
    /// The effective policy for this run: the job's override if it has one,
    /// otherwise the destination's.
    pub retention: RetentionPolicy,
    /// Create the repository if the destination is empty. False for a dry run
    /// or a verification pass, where materialising a repository would be a
    /// surprising side effect.
    pub create_if_missing: bool,
    pub cancel: CancelToken,
}

/// What `prepare` learned about the destination.
#[derive(Debug, Clone, Default)]
pub struct PrepareOutcome {
    /// True when this call created the repository.
    pub created: bool,
    /// Driver version string, for the About screen and for bug reports.
    pub backend_version: Option<String>,
    pub warnings: Vec<String>,
}

/// One snapshot of one job's sources into one destination.
#[derive(Debug, Clone)]
pub struct SnapshotRequest {
    pub run_id: Uuid,
    pub job_id: Uuid,
    pub job_name: String,
    pub destination: Arc<Destination>,
    pub sources: Vec<Source>,
    pub exclusions: ExclusionSet,
    /// Already resolved through job → destination → global precedence and
    /// through any active time window. See [`crate::engine::throttle`].
    pub bandwidth: ResolvedBandwidth,
    /// Absolute progress snapshots; coalesced downstream.
    pub progress: ProgressSink,
    /// Fires when the run must stop. See the cancellation contract above.
    pub cancel: CancelToken,
    /// Which attempt this is, starting at 1. Drivers may use it to widen a
    /// timeout or to log; the engine owns the retry decision.
    pub attempt: u32,
    /// Report what would be backed up without creating a snapshot.
    ///
    /// An implementation that cannot rehearse must return an error rather than
    /// quietly performing the real thing: a dry run that writes is worse than
    /// one that refuses, because the user believed nothing would happen.
    pub dry_run: bool,
}

/// The result of a successful snapshot.
#[derive(Debug, Clone, Default)]
pub struct SnapshotOutcome {
    /// Kopia snapshot id, recorded in [`crate::state::DestinationRun`].
    pub snapshot_id: Option<String>,
    /// Final absolute counters. The runner stores these verbatim, so they must
    /// be the true totals rather than the last sampled value.
    pub progress: Progress,
    /// Non-fatal problems: unreadable files, skipped paths, ignored errors.
    /// A non-empty list turns the destination into `SucceededWithWarnings`.
    pub warnings: Vec<String>,
}

/// Replicate one destination's repository into another destination.
///
/// This is the chained-backup step: the sources were read, chunked and
/// encrypted once into `source`, and `destination` is filled from the
/// resulting blobs instead of from the user's disk a second time.
///
/// The runner only ever issues this after `source` has finished successfully
/// **in the same run**, so an implementation may assume the source repository
/// is current. It must *not* assume the destination repository exists: it may
/// be an empty bucket on the first run, and creating it is the replication's
/// own business — but creating it as a *new* repository is not, because
/// `kopia repository sync-to` refuses a destination whose format blob does not
/// match the source's.
#[derive(Debug, Clone)]
pub struct ReplicateRequest {
    pub run_id: Uuid,
    pub job_id: Uuid,
    pub job_name: String,
    /// The replica being written.
    pub destination: Arc<Destination>,
    /// The repository being copied. Always a repository destination, never a
    /// [`crate::model::DestinationKind::LocalMirror`].
    pub source: Arc<Destination>,
    /// Resolved exactly as for [`SnapshotRequest::bandwidth`], through job →
    /// destination → global precedence and any active time window.
    pub bandwidth: ResolvedBandwidth,
    pub progress: ProgressSink,
    /// Fires when the run must stop. Same contract as everywhere else: return
    /// within about a second, having killed the child.
    pub cancel: CancelToken,
    pub attempt: u32,
    /// Report what would be copied without copying it.
    ///
    /// An implementation that cannot rehearse without writing must return an
    /// error rather than replicating for real. Note that this is a sharper
    /// requirement than it sounds: kopia's own `--dry-run` will still
    /// initialise an empty destination unless `--must-exist` is passed with it.
    pub dry_run: bool,
}

/// The result of a successful replication.
#[derive(Debug, Clone, Default)]
pub struct ReplicateOutcome {
    /// Blobs actually transferred. Zero is the normal, healthy result when the
    /// replica was already up to date.
    pub blobs_copied: u64,
    pub bytes_copied: u64,
    /// Final absolute counters, stored verbatim by the runner. Blob counts
    /// occupy the file fields, because every consumer of
    /// [`crate::state::Progress`] would otherwise show an empty destination.
    pub progress: Progress,
    /// Non-fatal problems. A non-empty list turns the destination into
    /// `SucceededWithWarnings`, exactly as for a snapshot.
    pub warnings: Vec<String>,
}

/// A consistency check of a destination, run by `superbackup verify` and by
/// the periodic maintenance pass.
#[derive(Debug, Clone)]
pub struct VerifyRequest {
    pub run_id: Uuid,
    pub destination: Arc<Destination>,
    /// Fraction of blobs to read back, 0.0..=1.0. Full verification of a large
    /// remote repository costs real money in egress, so it is sampled.
    pub sample_percent: f32,
    pub progress: ProgressSink,
    pub cancel: CancelToken,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyOutcome {
    pub blobs_checked: u64,
    pub problems: Vec<String>,
}

/// Drives one backend. See the module docs for the full contract.
///
/// Deliberately dyn-compatible (boxed futures rather than `async fn`) so the
/// daemon can hold one `Arc<dyn BackupExecutor>` chosen at startup — real
/// driver, mock, or dry-run — without the choice leaking into the type of
/// every component that touches it.
pub trait BackupExecutor: std::fmt::Debug + Send + Sync + 'static {
    /// Connect to (or create) the repository behind `destination`.
    ///
    /// Called once per destination per run, before any snapshot. Idempotent:
    /// re-preparing an already-connected repository must succeed cheaply.
    fn prepare<'a>(
        &'a self,
        request: PrepareRequest,
    ) -> BoxFuture<'a, ExecutorResult<PrepareOutcome>>;

    /// Snapshot `request.sources` into `request.destination`.
    ///
    /// Must stream progress, must honour the cancel token within about a
    /// second, and must leave no lock or child process behind on any exit
    /// path.
    fn snapshot<'a>(
        &'a self,
        request: SnapshotRequest,
    ) -> BoxFuture<'a, ExecutorResult<SnapshotOutcome>>;

    /// Copy `request.source`'s repository into `request.destination`.
    ///
    /// Required rather than defaulted: an executor that silently answered
    /// "not supported" would turn a configured offsite copy into a run that
    /// looks fine and produces nothing, which is the failure mode this whole
    /// feature exists to avoid. An implementation that genuinely cannot
    /// replicate should say so with an error the user can read.
    fn replicate<'a>(
        &'a self,
        request: ReplicateRequest,
    ) -> BoxFuture<'a, ExecutorResult<ReplicateOutcome>>;

    /// Check the integrity of a destination.
    fn verify<'a>(&'a self, request: VerifyRequest)
        -> BoxFuture<'a, ExecutorResult<VerifyOutcome>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::TestClock;

    fn sink(
        clock: Arc<dyn crate::engine::clock::Clock>,
    ) -> (ProgressSink, tokio::sync::mpsc::UnboundedReceiver<ProgressUpdate>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (ProgressSink::new(tx, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), clock), rx)
    }

    #[tokio::test]
    async fn progress_is_coalesced_to_the_rate_limit() {
        let clock = Arc::new(TestClock::at("2025-01-01T00:00:00Z"));
        let (sink, mut rx) = sink(clock.clone());
        // 10_000 updates inside one instant must collapse to a single emit.
        for i in 0..10_000u64 {
            sink.update(Progress { files_processed: i, ..Default::default() });
        }
        let mut seen = 0;
        while rx.try_recv().is_ok() {
            seen += 1;
        }
        assert_eq!(seen, 1, "a 10k/s stream must not become 10k events");
    }

    #[tokio::test]
    async fn progress_resumes_after_the_window() {
        let clock = Arc::new(TestClock::at("2025-01-01T00:00:00Z"));
        let (sink, mut rx) = sink(clock.clone());
        sink.update(Progress::default());
        clock.advance(chrono::Duration::milliseconds(150));
        sink.update(Progress::default());
        let mut seen = 0;
        while rx.try_recv().is_ok() {
            seen += 1;
        }
        assert_eq!(seen, 2);
    }

    #[tokio::test]
    async fn final_state_is_never_dropped() {
        let clock = Arc::new(TestClock::at("2025-01-01T00:00:00Z"));
        let (sink, mut rx) = sink(clock.clone());
        sink.update(Progress { files_processed: 1, ..Default::default() });
        // Same instant: the rate limit would normally suppress this.
        sink.finish(Progress { files_processed: 999, ..Default::default() });
        let mut last = None;
        while let Ok(u) = rx.try_recv() {
            last = Some(u);
        }
        let last = last.expect("an update");
        assert_eq!(last.progress.files_processed, 999);
        assert!(last.final_update);
    }
}
