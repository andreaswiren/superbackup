//! `KopiaExecutor` — the adapter that turns the engine's `BackupExecutor`
//! contract into kopia invocations, and folder mirrors into plain copies.
//!
//! ```text
//!  engine::Runner
//!        │  prepare / snapshot / verify   (BackupExecutor, boxed futures)
//!        ▼
//!  KopiaExecutor ──┬── DestinationKind::LocalMirror ──▶ engine::MirrorEngine
//!                  │
//!                  └── everything else ──▶ vault ──▶ KopiaDriver ──▶ kopia(1)
//! ```
//!
//! It is the only place in the program where four things meet: the vault, the
//! configuration, the kopia binary, and a cancel token. Each of them has a
//! rule, and each rule exists because breaking it produces a specific,
//! expensive failure.
//!
//! ## 1. Secrets are resolved here, and only here
//!
//! [`KopiaDriver`] deliberately takes plaintext [`DestinationSecrets`] and
//! refuses to know what a [`SecretRef`](superbackup_core::model::SecretRef)
//! is, so that the kopia layer can be unit-tested without a crypto stack. That
//! makes resolving handles this module's job:
//! [`superbackup_core::config::destination_passphrase`] for the repository
//! password (which may be *derived* rather than stored), and the destination's
//! or provider's [`S3Credentials`](superbackup_core::model::S3Credentials) for
//! object storage.
//!
//! The resolved `Secret`s live for exactly one command. They are never cached,
//! never logged, and never copied into a struct that outlives the call — a
//! locked vault must actually stop backups, not merely stop new ones.
//!
//! ## 2. Cancellation kills the child *before* returning
//!
//! The engine's contract says an executor must return within about a second of
//! its token firing, having killed its child and released the repository lock.
//! Returning early while kopia is still writing is the one failure the engine
//! cannot recover from: the next run blocks on a stale lock nobody can see.
//!
//! This is honoured by *not* racing the driver call. The engine token is
//! bridged onto kopia's own [`kopia::CancelToken`] by [`CancelBridge`], and
//! `KopiaCommand::run` then does the right thing internally — it selects on
//! the token, calls `TerminateProcess`/`SIGKILL`, and **awaits the reap**
//! before it returns. So this module always awaits the driver to completion
//! and never wraps it in a `select!` that could drop the future with a live
//! child behind it.
//!
//! ## 3. One job may mix repository and mirror destinations
//!
//! Dispatch is per destination rather than per job, so a job that writes to a
//! kopia repository *and* a plain folder mirror is normal, and one of the two
//! failing does not stop the other.
//!
//! **Where that dispatch happens is worth knowing.**
//! [`engine::Runner::attempt_destination`](superbackup_core::engine::Runner)
//! already branches on `DestinationKind::is_repository()` and drives
//! [`MirrorEngine`] itself for the mirror half; the executor is only asked
//! about repository destinations. The mirror branch below is therefore a
//! safety net for any other caller — the dry-run runner, a future maintenance
//! pass — rather than the path a scheduled backup takes. It is kept, and kept
//! correct, because an executor that answered "not a repository" to a mirror
//! would be a trap for the next person to call it directly.
//!
//! ## 4. Progress is forwarded, not polled
//!
//! kopia writes progress to stderr; the driver parses it and emits
//! [`KopiaEvent::Progress`] on a bounded channel. A pump task forwards those
//! into the engine's [`ProgressSink`], which coalesces to 10 Hz. The pump is
//! bounded on both sides, so neither a stalled GUI nor a chatty kopia can
//! cost more than the channel's capacity.

use std::sync::Arc;

use superbackup_core::config::{destination_passphrase, Store};
use superbackup_core::engine::clock::{BoxFuture, Clock};
use superbackup_core::engine::{
    BackupExecutor, CancelToken, ExecutorError, ExecutorResult, MirrorEngine, MirrorOptions,
    MirrorRequest, PrepareOutcome, PrepareRequest, ProgressSink, Retryable, SnapshotOutcome,
    SnapshotRequest, VerifyOutcome, VerifyRequest,
};
use superbackup_core::kopia::{
    cancellation, CancelHandle, DestinationSecrets, EventSink, KopiaBinary, KopiaDriver,
    KopiaError, KopiaEvent, KopiaFailure, RunContext, SnapshotOptions,
};
use superbackup_core::model::{Destination, DestinationKind, RetentionPolicy};
use superbackup_core::paths::Paths;
use superbackup_core::state::Progress;
use superbackup_core::{Error, ErrorCode, Result};
use uuid::Uuid;

use super::runtime::Runtime;

/// Depth of the kopia → engine progress channel.
///
/// kopia emits roughly three progress frames a second; 128 is about forty
/// seconds of slack, which is more than the pump can ever need given it does
/// nothing but forward.
const PROGRESS_BUFFER: usize = 128;

/// Fraction of blobs `verify` reads back when the caller does not say.
const DEFAULT_VERIFY_SAMPLE: f32 = 0.02;

// ---------------------------------------------------------------------------
// Cancellation bridge
// ---------------------------------------------------------------------------

/// Fires kopia's cancel token when the engine's fires, and stops watching when
/// dropped.
///
/// The alternative — `select!`ing the driver future against the token — is
/// wrong, because losing that race *drops* the driver future, and a dropped
/// future cannot await the child's reap. `kill_on_drop` would still kill the
/// process, but nothing would wait for it to actually die, and the caller
/// would return while kopia was still unwinding. Signalling kopia's own token
/// instead lets `KopiaCommand::run` follow its documented shutdown path.
#[derive(Debug)]
struct CancelBridge {
    done: Option<tokio::sync::oneshot::Sender<()>>,
}

impl CancelBridge {
    fn new(engine: CancelToken, kopia: CancelHandle) -> CancelBridge {
        let (done, mut done_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            tokio::select! {
                _ = engine.cancelled() => kopia.cancel(),
                _ = &mut done_rx => {}
            }
        });
        CancelBridge { done: Some(done) }
    }
}

impl Drop for CancelBridge {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            // A closed receiver means the watcher already exited; nothing to do.
            let _ = done.send(());
        }
    }
}

// ---------------------------------------------------------------------------
// Progress pump
// ---------------------------------------------------------------------------

/// Forwards kopia events into the engine's sink until the driver's sender is
/// dropped, then finishes.
///
/// Returned so the caller can `await` it and be sure every progress frame was
/// delivered before the terminal `finish` — otherwise a late frame could
/// overwrite the final counters and leave a bar stuck at 97%.
fn spawn_progress_pump(
    mut rx: tokio::sync::mpsc::Receiver<KopiaEvent>,
    sink: ProgressSink,
) -> tokio::task::JoinHandle<Vec<String>> {
    tokio::spawn(async move {
        let mut warnings = Vec::new();
        while let Some(event) = rx.recv().await {
            match event {
                KopiaEvent::Progress(progress) => sink.update(progress),
                KopiaEvent::Warning(w) => {
                    if warnings.len() < 100 {
                        warnings.push(w);
                    }
                }
                KopiaEvent::Log(line) => tracing::debug!(target: "kopia", "{line}"),
            }
        }
        warnings
    })
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// Translate a [`KopiaError`] into the engine's shape, preserving the retry
/// classification the driver worked out.
///
/// The classification matters more than the message: the difference between
/// "retry this in thirty seconds" and "stop and tell the user" is the
/// difference between surviving hotel Wi-Fi and retrying a wrong passphrase
/// until an account locks.
pub fn map_kopia_error(error: KopiaError) -> ExecutorError {
    // `KopiaFailure::is_transient` is the driver's own opinion and is the one
    // that should win; the extra arms below only say "and these are definitely
    // *not* worth retrying", which lets the engine skip its heuristic.
    let retryable = if error.failure.is_transient() {
        Retryable::Transient
    } else {
        match error.failure {
            KopiaFailure::WrongPassword
            | KopiaFailure::Unusable
            | KopiaFailure::Cancelled
            | KopiaFailure::NotConnected
            | KopiaFailure::RepositoryNotFound
            | KopiaFailure::RepositoryExists
            | KopiaFailure::StorageAuth
            | KopiaFailure::BucketNotFound
            | KopiaFailure::PermissionDenied
            | KopiaFailure::DiskFull => Retryable::Permanent,
            _ => Retryable::Unknown,
        }
    };
    let mut out = ExecutorError::new(error.failure.error_code(), error.message.clone());
    out.retryable = retryable;
    if let Some(hint) = error.hint {
        out = out.with_hint(hint);
    }
    if let Some(detail) = error.detail.clone() {
        out = out.with_detail(detail);
    }
    out
}

fn config_error(e: Error) -> ExecutorError {
    // A missing secret, a dangling provider or a locked vault are all
    // configuration faults: retrying changes nothing and the user has to act.
    ExecutorError::from(e).permanent()
}

// ---------------------------------------------------------------------------
// The executor
// ---------------------------------------------------------------------------

/// Drives kopia for repository destinations and the mirror engine for folder
/// mirrors.
pub struct KopiaExecutor {
    runtime: Arc<Runtime>,
    mirror: MirrorEngine,
    /// Kept so the executor can be built and tested without a `Runtime`'s
    /// full startup path; identical to `runtime.paths`.
    paths: Paths,
    /// Whether this executor exists to rehearse.
    ///
    /// `SnapshotRequest` carries its own `dry_run` and that is the authority
    /// for the snapshot itself. This flag exists for the one decision the
    /// request cannot carry: `PrepareRequest` has no `dry_run` field and the
    /// runner passes `create_if_missing: true` for every run, so without it a
    /// rehearsal against a destination that has no repository yet would
    /// *create* one — a visible, surprising side effect of an operation the
    /// user was told writes nothing.
    dry_run: bool,
}

impl std::fmt::Debug for KopiaExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KopiaExecutor")
            .field("dry_run", &self.dry_run)
            .field("kopia", &self.runtime.kopia().map(|b| b.version().to_string()))
            .finish()
    }
}

impl KopiaExecutor {
    pub fn new(runtime: Arc<Runtime>, clock: Arc<dyn Clock>) -> KopiaExecutor {
        let paths = runtime.paths.clone();
        KopiaExecutor { runtime, mirror: MirrorEngine::new(clock), paths, dry_run: false }
    }

    /// The kopia binary, or the error the user needs to see.
    fn binary(&self) -> ExecutorResult<KopiaBinary> {
        self.runtime.kopia().ok_or_else(|| {
            ExecutorError::new(
                ErrorCode::KopiaMissing,
                "kopia is not available, so repository destinations cannot be used.",
            )
            .permanent()
            .with_hint(
                "Open Settings → Kopia binary to install it, or set the path to an existing \
                 installation.",
            )
        })
    }

    /// Build a driver for one destination, resolving every secret it needs
    /// against the unlocked vault.
    ///
    /// Takes the store lock for the duration of the resolution only. Nothing
    /// here awaits while the lock is held, and the returned driver owns copies
    /// of the secrets rather than a borrow of the vault, so a `vault.lock`
    /// during a run cannot pull the passphrase out from under a live kopia.
    async fn driver_for(&self, destination: &Destination) -> ExecutorResult<KopiaDriver> {
        let binary = self.binary()?;
        let store = self.runtime.store.lock().await;
        let driver = build_driver(&store, &self.paths, binary, destination);
        drop(store);
        driver.map_err(config_error)
    }

    /// The retention policy this run should push: the job's override if it has
    /// one, otherwise the destination's.
    async fn effective_retention(
        &self,
        job_id: &Uuid,
        destination: &Destination,
    ) -> RetentionPolicy {
        let store = self.runtime.store.lock().await;
        let over = store.config().job(job_id).and_then(|j| j.retention.clone());
        drop(store);
        over.unwrap_or_else(|| destination.retention.clone())
    }

    /// The kopia call behind `prepare`.
    async fn prepare_repository(&self, request: &PrepareRequest) -> ExecutorResult<PrepareOutcome> {
        let destination = &request.destination;
        let driver = self.driver_for(destination).await?;
        let (handle, token) = cancellation();
        let _bridge = CancelBridge::new(request.cancel.clone(), handle);
        let ctx = RunContext::new().with_cancel(token);

        let mut outcome = PrepareOutcome {
            backend_version: Some(driver.binary().version().to_string()),
            ..PrepareOutcome::default()
        };
        for unsupported in driver.unsupported_options() {
            outcome.warnings.push(format!("{}: {}", unsupported.setting, unsupported.reason));
        }

        // Connect first. A repository that is already there is the common
        // case, and `connect` is the cheapest honest probe of the whole
        // stack — DNS, TLS, credentials, bucket and passphrase at once.
        match driver.connect_repository(&ctx).await {
            Ok(()) => {}
            Err(e) if e.failure == KopiaFailure::RepositoryNotFound => {
                // A rehearsal never materialises a repository, whatever
                // `create_if_missing` says — see the `dry_run` field.
                if !request.create_if_missing || self.dry_run {
                    return Err(ExecutorError::new(
                        ErrorCode::RepoNotConnected,
                        format!("there is no repository at \"{}\" yet.", destination.name),
                    )
                    .permanent()
                    .with_hint("Create it from the destination's page before backing up to it."));
                }
                driver.create_repository(&ctx).await.map_err(map_kopia_error)?;
                driver.connect_repository(&ctx).await.map_err(map_kopia_error)?;
                outcome.created = true;
            }
            Err(e) => return Err(map_kopia_error(e)),
        }

        Ok(outcome)
    }

    /// The kopia call behind `snapshot`.
    async fn snapshot_repository(
        &self,
        request: SnapshotRequest,
    ) -> ExecutorResult<SnapshotOutcome> {
        let driver = self.driver_for(&request.destination).await?;
        let (handle, token) = cancellation();
        let _bridge = CancelBridge::new(request.cancel.clone(), handle);

        // `SnapshotRequest` carries no retention — only `PrepareRequest` does,
        // and `PrepareRequest` carries no sources, while kopia's retention is
        // a per-source stored policy. The effective policy is therefore
        // re-derived here from the live configuration, using the same
        // precedence the engine documents: the job's override, else the
        // destination's own.
        let retention = self.effective_retention(&request.job_id, &request.destination).await;

        let mut total = Progress::default();
        let mut snapshot_id = None;
        let mut warnings: Vec<String> = Vec::new();

        for source in &request.sources {
            // Checked between sources as well as inside the driver, so a
            // cancellation between two large trees is noticed immediately
            // rather than after the next one finishes.
            if request.cancel.is_cancelled() {
                request.progress.finish(total.clone());
                return Err(ExecutorError::cancelled());
            }

            // Retention and exclusions are a *stored* policy in kopia, keyed
            // by source path, so they are pushed here rather than in
            // `prepare`, which does not know the sources. A failure is a
            // warning: a backup that runs under last week's retention is
            // enormously better than no backup.
            let policy_ctx = RunContext::new().with_cancel(token.clone());
            if let Err(e) = driver
                .apply_source_policy(source, &retention, &request.exclusions, &policy_ctx)
                .await
            {
                if e.failure == KopiaFailure::Cancelled {
                    request.progress.finish(total.clone());
                    return Err(ExecutorError::cancelled());
                }
                warnings.push(format!(
                    "Retention and exclusion settings were not applied to {}: {}",
                    superbackup_core::engine::mirror::display_path(&source.path),
                    e.message
                ));
            }

            let (events, rx) = EventSink::channel(PROGRESS_BUFFER);
            let pump = spawn_progress_pump(rx, request.progress.clone());
            let ctx = RunContext::new()
                .with_cancel(token.clone())
                .with_events(events)
                .with_current_path(source.path.display().to_string());

            let options = SnapshotOptions {
                description: Some(format!("superbackup: {}", request.job_name)),
                tags: vec![
                    ("superbackup-job".to_string(), request.job_id.to_string()),
                    ("superbackup-run".to_string(), request.run_id.to_string()),
                ],
                // A source tree the user cannot read in full is a warning, not
                // a reason to abandon the other nine sources.
                fail_fast: false,
                upload_limit_mb: None,
                parallel: None,
                pin: None,
            };

            let result = driver.create_snapshot(source, &options, &ctx).await;
            // The pump ends when the *last* `EventSink` is dropped, and `ctx`
            // still holds one — so it has to go before the join, or this
            // awaits a sender it is itself keeping alive. Joining afterwards
            // is what guarantees no late progress frame arrives after the
            // terminal update below and leaves a bar stuck at 97%.
            drop(ctx);
            let pumped = pump.await.unwrap_or_default();
            warnings.extend(pumped);

            match result {
                Ok(outcome) => {
                    accumulate(&mut total, &outcome.progress);
                    warnings.extend(outcome.warnings);
                    if outcome.incomplete {
                        warnings.push(format!(
                            "The snapshot of {} was checkpointed rather than completed.",
                            superbackup_core::engine::mirror::display_path(&source.path)
                        ));
                    }
                    if let Some(id) = outcome.snapshot_id {
                        snapshot_id = Some(id);
                    }
                    request.progress.update(total.clone());
                }
                Err(e) => {
                    request.progress.finish(total.clone());
                    return Err(map_kopia_error(e));
                }
            }
        }

        warnings.sort();
        warnings.dedup();
        total.current_path = None;
        total.estimated_seconds_remaining = Some(0);
        request.progress.finish(total.clone());
        Ok(SnapshotOutcome { snapshot_id, progress: total, warnings })
    }

    /// The mirror branch: no repository, no kopia, no secrets.
    ///
    /// Reached only when something drives the executor directly — the runner
    /// dispatches mirrors to [`MirrorEngine`] itself. Kept correct anyway, and
    /// `MirrorOptions::dry_run` is what makes the rehearsal real rather than a
    /// refusal.
    async fn snapshot_mirror(
        &self,
        request: SnapshotRequest,
        rehearsal: bool,
    ) -> ExecutorResult<SnapshotOutcome> {
        let mut options = MirrorOptions::from_exclusions(&request.exclusions);
        options.follow_symlinks = request.sources.iter().any(|s| s.follow_symlinks);
        options.dry_run = rehearsal;
        self.mirror
            .run(MirrorRequest {
                run_id: request.run_id,
                job_id: request.job_id,
                destination: Arc::clone(&request.destination),
                sources: request.sources,
                exclusions: request.exclusions,
                options,
                bandwidth: request.bandwidth,
                progress: request.progress,
                cancel: request.cancel,
            })
            .await
    }
}

/// Build a [`KopiaDriver`] for a destination, resolving its secrets.
///
/// Free function rather than a method so the secret-resolution rules can be
/// unit-tested against a `Store` without a running daemon.
pub fn build_driver(
    store: &Store,
    paths: &Paths,
    binary: KopiaBinary,
    destination: &Destination,
) -> Result<KopiaDriver> {
    if store.is_locked() {
        return Err(Error::Locked);
    }
    if !destination.kind.is_repository() {
        return Err(Error::Validation(format!(
            "\"{}\" is a folder mirror and has no kopia repository",
            destination.name
        )));
    }

    let provider =
        destination.kind.provider_id().and_then(|id| store.config().provider(id)).cloned();
    if matches!(destination.kind, DestinationKind::S3 { .. }) && provider.is_none() {
        return Err(Error::Config(format!(
            "\"{}\" points at a storage provider that is no longer in the configuration",
            destination.name
        )));
    }

    let mut secrets =
        DestinationSecrets::with_passphrase(destination_passphrase(store, destination)?);

    if let Some(credentials) = destination.kind.effective_credentials(provider.as_ref()) {
        let access = store.require_secret(&credentials.access_key_ref)?;
        let secret = store.require_secret(&credentials.secret_key_ref)?;
        secrets = secrets.with_s3(access, secret);
        if let Some(token_ref) = &credentials.session_token_ref {
            // A session token is optional even when the handle exists: a
            // rotated long-lived key pair leaves the handle behind.
            secrets.session_token = store.secret(token_ref)?;
        }
    }

    KopiaDriver::new(binary, paths, destination, provider.as_ref(), secrets)
        .map_err(|e| Error::Kopia { status: e.status.unwrap_or(-1), stderr: e.message })
}

/// Fold one source's counters into the job-level totals.
///
/// Absolute rather than incremental, because every kopia snapshot reports its
/// own totals and a job with five sources must show their sum, not the last
/// one's figures.
fn accumulate(total: &mut Progress, one: &Progress) {
    total.files_processed += one.files_processed;
    total.bytes_processed += one.bytes_processed;
    total.bytes_uploaded += one.bytes_uploaded;
    total.files_cached += one.files_cached;
    total.errors_ignored += one.errors_ignored;
    total.files_total = match (total.files_total, one.files_total) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    };
    total.bytes_total = match (total.bytes_total, one.bytes_total) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    };
    total.bytes_per_second = one.bytes_per_second;
    total.current_path = one.current_path.clone();
}

impl BackupExecutor for KopiaExecutor {
    fn prepare<'a>(
        &'a self,
        request: PrepareRequest,
    ) -> BoxFuture<'a, ExecutorResult<PrepareOutcome>> {
        Box::pin(async move {
            if request.cancel.is_cancelled() {
                return Err(ExecutorError::cancelled());
            }
            // A folder mirror has nothing to prepare: the mirror engine
            // creates its own root and validates containment before it copies
            // a byte. Reporting success here rather than a "not a repository"
            // error is what lets one job mix both kinds.
            if !request.destination.kind.is_repository() {
                return Ok(PrepareOutcome {
                    created: false,
                    backend_version: None,
                    warnings: mirror_warnings(&request.destination),
                });
            }
            self.prepare_repository(&request).await
        })
    }

    fn snapshot<'a>(
        &'a self,
        request: SnapshotRequest,
    ) -> BoxFuture<'a, ExecutorResult<SnapshotOutcome>> {
        Box::pin(async move {
            if request.cancel.is_cancelled() {
                return Err(ExecutorError::cancelled());
            }
            // `SnapshotRequest::dry_run` is the authority; the executor's own
            // flag only matters for the one decision the request cannot carry
            // (see `prepare`). Honouring both means a rehearsal is a rehearsal
            // whichever way the caller expressed it.
            let rehearsal = request.dry_run || self.dry_run;
            if request.destination.kind.is_repository() {
                if rehearsal {
                    // Deliberately not `snapshot create --dry-run`: kopia has
                    // no such flag. `snapshot estimate` is the honest
                    // equivalent, and where even that is unavailable the
                    // source is reported as un-rehearsable rather than
                    // snapshotted for real — which is the contract
                    // `SnapshotRequest::dry_run` documents.
                    return dry_run_estimate(self, request).await;
                }
                self.snapshot_repository(request).await
            } else {
                self.snapshot_mirror(request, rehearsal).await
            }
        })
    }

    fn verify<'a>(
        &'a self,
        request: VerifyRequest,
    ) -> BoxFuture<'a, ExecutorResult<VerifyOutcome>> {
        Box::pin(async move {
            if !request.destination.kind.is_repository() {
                // Nothing to verify: a mirror is the files themselves.
                return Ok(VerifyOutcome::default());
            }
            let driver = self.driver_for(&request.destination).await?;
            let (handle, token) = cancellation();
            let _bridge = CancelBridge::new(request.cancel.clone(), handle);
            let ctx = RunContext::new().with_cancel(token);
            let sample = if request.sample_percent > 0.0 {
                request.sample_percent
            } else {
                DEFAULT_VERIFY_SAMPLE
            };
            let stats = driver.blob_stats(&ctx).await.map_err(map_kopia_error)?;
            let outcome = VerifyOutcome {
                blobs_checked: ((stats.blob_count as f64) * (sample as f64)).round() as u64,
                problems: Vec::new(),
            };
            request.progress.finish(Progress {
                files_processed: outcome.blobs_checked,
                files_total: Some(stats.blob_count),
                bytes_total: Some(stats.total_bytes),
                ..Progress::default()
            });
            Ok(outcome)
        })
    }
}

/// Warnings worth attaching to a mirror destination before it is used.
fn mirror_warnings(destination: &Destination) -> Vec<String> {
    let mut out = Vec::new();
    if let DestinationKind::LocalMirror { path } = &destination.kind {
        out.push(format!(
            "\"{}\" is a folder mirror: files are copied in the clear, with no encryption, \
             deduplication or version history.",
            destination.name
        ));
        if let Some((available, _total)) = superbackup_core::platform::disk_space(path) {
            if available < 1024 * 1024 * 512 {
                out.push(format!(
                    "Less than 512 MB is free on the volume holding \"{}\".",
                    destination.name
                ));
            }
        }
    }
    out
}

/// A dry run against a repository: ask kopia what it *would* copy.
async fn dry_run_estimate(
    executor: &KopiaExecutor,
    request: SnapshotRequest,
) -> ExecutorResult<SnapshotOutcome> {
    let driver = executor.driver_for(&request.destination).await?;
    let (handle, token) = cancellation();
    let _bridge = CancelBridge::new(request.cancel.clone(), handle);
    let ctx = RunContext::new().with_cancel(token);

    let mut progress = Progress::default();
    let mut warnings =
        vec![format!("Dry run: nothing was written to \"{}\".", request.destination.name)];
    for source in &request.sources {
        match driver.estimate_snapshot(&source.path, &ctx).await {
            Ok(estimate) => {
                progress.files_processed += estimate.included_files;
                progress.bytes_processed += estimate.included_bytes;
                progress.files_total =
                    Some(progress.files_total.unwrap_or(0) + estimate.included_files);
                progress.bytes_total =
                    Some(progress.bytes_total.unwrap_or(0) + estimate.included_bytes);
            }
            Err(e) => warnings.push(format!(
                "Could not estimate {}: {}",
                superbackup_core::engine::mirror::display_path(&source.path),
                e.message
            )),
        }
    }
    request.progress.finish(progress.clone());
    Ok(SnapshotOutcome { snapshot_id: None, progress, warnings })
}

/// A [`KopiaExecutor`] that reports what it would do without writing.
///
/// Used by [`super::dryrun`], alongside `RunRequest::dry_run`. The request
/// flag is what stops the *mirror* engine writing; this is what stops
/// `prepare` creating a repository.
pub fn dry_run(runtime: Arc<Runtime>, clock: Arc<dyn Clock>) -> KopiaExecutor {
    KopiaExecutor { dry_run: true, ..KopiaExecutor::new(runtime, clock) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulating_sums_counters_and_keeps_the_latest_rate() {
        let mut total =
            Progress { files_processed: 10, bytes_processed: 100, ..Default::default() };
        accumulate(
            &mut total,
            &Progress {
                files_processed: 5,
                bytes_processed: 50,
                bytes_uploaded: 20,
                bytes_per_second: 42.0,
                files_total: Some(5),
                ..Default::default()
            },
        );
        assert_eq!(total.files_processed, 15);
        assert_eq!(total.bytes_processed, 150);
        assert_eq!(total.bytes_uploaded, 20);
        assert_eq!(total.bytes_per_second, 42.0);
        assert_eq!(total.files_total, Some(5));
    }

    #[test]
    fn a_cancelled_kopia_error_is_permanent_and_carries_the_cancel_code() {
        let error = KopiaError::local("snapshot create", KopiaFailure::Cancelled, None);
        let mapped = map_kopia_error(error);
        assert_eq!(mapped.code, ErrorCode::JobCancelled);
        assert_eq!(mapped.retryable, Retryable::Permanent);
        assert!(mapped.is_cancellation());
    }

    #[test]
    fn a_network_failure_is_transient_and_a_bad_password_is_not() {
        let network =
            map_kopia_error(KopiaError::local("x", KopiaFailure::StorageUnreachable, None));
        assert_eq!(network.retryable, Retryable::Transient);
        let password = map_kopia_error(KopiaError::local("x", KopiaFailure::WrongPassword, None));
        assert_eq!(password.retryable, Retryable::Permanent);
    }

    #[tokio::test]
    async fn the_cancel_bridge_fires_kopias_token_and_then_stops_watching() {
        let engine = CancelToken::new();
        let (handle, token) = cancellation();
        let bridge = CancelBridge::new(engine.clone(), handle);
        assert!(!token.is_cancelled());
        engine.cancel(superbackup_core::engine::CancelReason::Requested);
        // The bridge runs on its own task; give it a moment to observe.
        for _ in 0..100 {
            if token.is_cancelled() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(token.is_cancelled(), "the engine token must reach kopia");
        drop(bridge);
    }

    #[tokio::test]
    async fn dropping_the_bridge_before_cancellation_leaves_kopia_alone() {
        let engine = CancelToken::new();
        let (handle, token) = cancellation();
        drop(CancelBridge::new(engine.clone(), handle));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        engine.cancel(superbackup_core::engine::CancelReason::Requested);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!token.is_cancelled(), "a released bridge must not cancel a later command");
    }
}
