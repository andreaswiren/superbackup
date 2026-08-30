//! Plain folder-to-folder mirroring.
//!
//! [`crate::model::DestinationKind::LocalMirror`] is the "I want a readable
//! copy I can open in Explorer" mode. There is no repository, no encryption
//! and no kopia, so the engine does the copy itself.
//!
//! # Layout
//!
//! Each source gets its own subfolder of the mirror root, named after the
//! source's last path component:
//!
//! ```text
//! <mirror root>/
//!   Projects/      <- from C:\Users\me\Projects
//!   Documents/     <- from D:\Documents
//! ```
//!
//! Colliding names are disambiguated with a numeric suffix. Sources are kept
//! apart rather than merged so that deletion (below) can be scoped to exactly
//! the subtree this run is responsible for.
//!
//! # Safety
//!
//! A mirror writes to, and optionally deletes from, an arbitrary user
//! directory. Three failure modes would each be catastrophic, and each has an
//! explicit guard and an explicit test:
//!
//! 1. **Recursion.** A destination nested inside its own source (`C:\data` →
//!    `C:\data\backup`) copies its own output forever until the disk fills.
//!    Both containment directions are rejected before a single byte moves.
//! 2. **Escaping the root.** Every write and every delete is re-checked
//!    against the canonical target root, so a crafted name, a symlink, or a
//!    `..` component cannot reach outside it.
//! 3. **Deletion.** Removing files is off unless
//!    [`MirrorOptions::delete_extraneous`] is explicitly set, never touches
//!    anything above the mirror root, refuses to operate on a filesystem root,
//!    deletes symlinks as links rather than following them, and only removes
//!    entries whose *source* has actually disappeared — never entries that
//!    merely became excluded, because a mistyped glob would otherwise erase
//!    the mirror.
//!
//! # Windows
//!
//! Roots are canonicalised, which on Windows yields `\\?\`-verbatim paths.
//! Every path derived from them inherits the prefix, so trees deeper than
//! `MAX_PATH` work without special-casing each operation. The prefix is
//! stripped again for anything shown to a human.

use crate::engine::cancel::{CancelReason, CancelToken};
use crate::engine::clock::Clock;
use crate::engine::executor::{ExecutorError, ExecutorResult, ProgressSink, SnapshotOutcome};
use crate::engine::throttle::{ResolvedBandwidth, TokenBucket};
use crate::error::ErrorCode;
use crate::model::{Destination, DestinationKind, ExclusionSet, Source};
use crate::state::Progress;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use uuid::Uuid;

/// Copy buffer. Large enough that syscall overhead is irrelevant on spinning
/// disks, small enough that a cancellation is noticed almost immediately even
/// inside a multi-gigabyte file.
const COPY_CHUNK: usize = 1024 * 1024;

/// Warnings kept verbatim before collapsing into a count. A tree with 40 000
/// locked files must not produce a 40 000-line run record.
const MAX_WARNINGS: usize = 100;

/// Filesystems store mtimes at different resolutions (FAT is 2 seconds, and
/// SMB shares round). Comparing exactly would re-copy the whole tree every
/// run on those targets.
const MTIME_TOLERANCE_SECS: i64 = 2;

/// The first 43 bytes of a `CACHEDIR.TAG` file, per the specification.
const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Knobs for one mirror run.
#[derive(Debug, Clone)]
pub struct MirrorOptions {
    /// Remove destination files whose source has disappeared.
    ///
    /// **Off by default and deliberately not (yet) exposed in
    /// [`crate::model::Job`].** A mirror that deletes is a mirror that can
    /// destroy the only remaining copy of a file the user accidentally moved,
    /// so turning it on has to be a conscious act by the layer above.
    pub delete_extraneous: bool,
    /// Skip files larger than this. Taken from the job's [`ExclusionSet`].
    pub max_file_size_mb: Option<u64>,
    /// Honour `CACHEDIR.TAG` markers.
    pub respect_cachedir_tag: bool,
    /// Whether the copy is allowed to leave the source tree through a symlink
    /// or a junction. Taken from [`Source::follow_symlinks`], per source.
    pub follow_symlinks: bool,
}

impl Default for MirrorOptions {
    fn default() -> Self {
        MirrorOptions {
            delete_extraneous: false,
            max_file_size_mb: None,
            respect_cachedir_tag: true,
            follow_symlinks: false,
        }
    }
}

impl MirrorOptions {
    /// Derive the filtering options from a job's exclusion set, leaving the
    /// destructive flag alone.
    pub fn from_exclusions(exclusions: &ExclusionSet) -> MirrorOptions {
        MirrorOptions {
            delete_extraneous: false,
            max_file_size_mb: exclusions.max_file_size_mb,
            respect_cachedir_tag: exclusions.respect_cachedir_tag,
            follow_symlinks: false,
        }
    }
}

/// One mirror of one job into one destination.
#[derive(Debug, Clone)]
pub struct MirrorRequest {
    pub run_id: Uuid,
    pub job_id: Uuid,
    pub destination: Arc<Destination>,
    pub sources: Vec<Source>,
    pub exclusions: ExclusionSet,
    pub options: MirrorOptions,
    pub bandwidth: ResolvedBandwidth,
    pub progress: ProgressSink,
    pub cancel: CancelToken,
}

/// The folder mirror. Holds nothing but a clock; one instance serves every
/// mirror destination.
#[derive(Debug, Clone)]
pub struct MirrorEngine {
    clock: Arc<dyn Clock>,
}

impl MirrorEngine {
    pub fn new(clock: Arc<dyn Clock>) -> MirrorEngine {
        MirrorEngine { clock }
    }

    /// Mirror every source of `request` into the destination folder.
    ///
    /// Returns the same [`SnapshotOutcome`] shape a repository destination
    /// produces, so the runner treats both identically.
    pub async fn run(&self, request: MirrorRequest) -> ExecutorResult<SnapshotOutcome> {
        let root = match request.destination.kind {
            DestinationKind::LocalMirror { ref path } => path.clone(),
            _ => {
                return Err(ExecutorError::new(
                    ErrorCode::Internal,
                    "the mirror engine was handed a repository destination",
                )
                .permanent())
            }
        };

        let started = self.clock.now_utc();
        let mut acc = Accumulator::new(request.progress.clone(), Arc::clone(&self.clock), started);

        std::fs::create_dir_all(&root).map_err(|e| {
            ExecutorError::new(
                ErrorCode::Io,
                format!("cannot create the mirror folder {}: {e}", root.display()),
            )
            .permanent()
        })?;
        let root = canonical(&root).map_err(|e| {
            ExecutorError::new(
                ErrorCode::Io,
                format!("cannot resolve the mirror folder {}: {e}", root.display()),
            )
            .permanent()
        })?;

        let matcher = Arc::new(ExclusionMatcher::build(&request.exclusions)?);
        let bucket = TokenBucket::for_upload(&request.bandwidth, Arc::clone(&self.clock)).map(Arc::new);

        let mut used_names: HashSet<String> = HashSet::new();
        for source in &request.sources {
            if request.cancel.is_cancelled() {
                return Err(cancelled_error(&request.cancel));
            }
            let src_root = canonical(&source.path).map_err(|e| {
                ExecutorError::new(
                    ErrorCode::Io,
                    format!("source folder {} is not readable: {e}", source.path.display()),
                )
                // A missing or unreadable source path is a configuration
                // problem: retrying cannot make the folder appear.
                .permanent()
            })?;
            let target = root.join(unique_folder_name(&source.path, &mut used_names));
            guard_containment(&src_root, &target)?;

            let options = MirrorOptions { follow_symlinks: source.follow_symlinks, ..request.options.clone() };
            self.mirror_one(
                &src_root,
                &target,
                &options,
                Arc::clone(&matcher),
                bucket.clone(),
                &request.cancel,
                &mut acc,
            )
            .await?;

            if options.delete_extraneous {
                self.prune(&src_root, &target, &request.cancel, &mut acc).await?;
            }
        }

        if request.cancel.is_cancelled() {
            return Err(cancelled_error(&request.cancel));
        }
        let progress = acc.finish();
        Ok(SnapshotOutcome { snapshot_id: None, progress, warnings: acc.into_warnings() })
    }

    /// Copy one source tree into one target folder.
    ///
    /// The directory walk runs on a blocking worker and streams entries over a
    /// bounded channel; the async side does the copying. The bound is what
    /// keeps a ten-million-file tree from being materialised in memory, and
    /// dropping the receiver (on cancellation) makes the walker's next send
    /// fail, which unwinds it without a second signalling mechanism.
    #[allow(clippy::too_many_arguments)]
    async fn mirror_one(
        &self,
        src_root: &Path,
        target: &Path,
        options: &MirrorOptions,
        matcher: Arc<ExclusionMatcher>,
        bucket: Option<Arc<TokenBucket>>,
        cancel: &CancelToken,
        acc: &mut Accumulator,
    ) -> ExecutorResult<()> {
        std::fs::create_dir_all(target).map_err(|e| {
            ExecutorError::new(
                ErrorCode::Io,
                format!("cannot create {}: {e}", display_path(target)),
            )
            .permanent()
        })?;
        let target = canonical(target).map_err(|e| {
            ExecutorError::new(ErrorCode::Io, format!("cannot resolve the mirror subfolder: {e}"))
                .permanent()
        })?;
        // Re-check after canonicalisation: the target may itself be a symlink
        // pointing back inside the source.
        guard_containment(src_root, &target)?;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<ScanItem>(256);
        let walk_root = src_root.to_path_buf();
        let walk_opts = options.clone();
        let walk_matcher = Arc::clone(&matcher);
        let walk_cancel = cancel.clone();
        let walker = tokio::task::spawn_blocking(move || {
            scan(&walk_root, &walk_opts, &walk_matcher, &walk_cancel, &tx);
        });

        while let Some(item) = rx.recv().await {
            if cancel.is_cancelled() {
                break;
            }
            match item {
                ScanItem::Warning(text) => acc.warn(text),
                ScanItem::Dir(rel) => {
                    let Some(dest) = safe_join(&target, &rel) else {
                        acc.warn(format!("refusing to create {} outside the mirror", rel.display()));
                        continue;
                    };
                    if let Err(e) = std::fs::create_dir_all(&dest) {
                        acc.warn(format!("cannot create {}: {e}", display_path(&dest)));
                    }
                }
                ScanItem::File { rel, len, mtime } => {
                    let Some(dest) = safe_join(&target, &rel) else {
                        acc.warn(format!("refusing to write {} outside the mirror", rel.display()));
                        continue;
                    };
                    let src = src_root.join(&rel);
                    acc.set_current(&rel);
                    if is_up_to_date(&dest, len, mtime) {
                        acc.cached(len);
                        continue;
                    }
                    match self
                        .copy_file(src, dest, len, mtime, bucket.clone(), cancel.clone())
                        .await
                    {
                        Ok(copied) => acc.copied(copied),
                        Err(CopyError::Cancelled) => break,
                        Err(CopyError::Io(text)) => acc.ignored(text),
                    }
                }
            }
        }
        // Drop the receiver so a walker still trying to send unwinds, then
        // join it. Joining rather than detaching is what guarantees no worker
        // thread outlives the run and keeps writing after it "finished".
        drop(rx);
        let _ = walker.await;

        if cancel.is_cancelled() {
            return Err(cancelled_error(cancel));
        }
        Ok(())
    }

    /// Copy one file, preserving its modification time.
    ///
    /// The whole copy happens on a blocking worker: chunked, cancellation
    /// checked between chunks, and throttled through the token bucket's
    /// blocking path. Doing it this way (rather than `tokio::fs` per chunk)
    /// keeps a large file at disk speed while still reacting to a Stop within
    /// one chunk.
    async fn copy_file(
        &self,
        src: PathBuf,
        dest: PathBuf,
        len: u64,
        mtime: Option<SystemTime>,
        bucket: Option<Arc<TokenBucket>>,
        cancel: CancelToken,
    ) -> Result<u64, CopyError> {
        let handle = tokio::task::spawn_blocking(move || {
            copy_file_blocking(&src, &dest, len, mtime, bucket.as_deref(), &cancel)
        });
        match handle.await {
            Ok(result) => result,
            // A panicking copy must not take the daemon down with it.
            Err(join) => Err(CopyError::Io(format!("copy worker failed: {join}"))),
        }
    }

    /// Delete destination entries whose source has disappeared.
    async fn prune(
        &self,
        src_root: &Path,
        target: &Path,
        cancel: &CancelToken,
        acc: &mut Accumulator,
    ) -> ExecutorResult<()> {
        let src_root = src_root.to_path_buf();
        let target = target.to_path_buf();
        let cancel = cancel.clone();
        let handle = tokio::task::spawn_blocking(move || prune_blocking(&src_root, &target, &cancel));
        match handle.await {
            Ok(Ok(report)) => {
                for w in report.warnings {
                    acc.warn(w);
                }
                acc.deleted(report.deleted);
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(join) => Err(ExecutorError::new(
                ErrorCode::Internal,
                format!("prune worker failed: {join}"),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// Reject a source and target that contain one another.
///
/// `dest inside source` is the infinite-recursion case: the copy would feed on
/// its own output. `source inside dest` is the self-overwrite case: the target
/// subtree *is* the source subtree, and the "delete what is gone" pass would
/// then be operating on the original data. Neither can be made safe by being
/// careful during the walk, so both are refused up front.
fn guard_containment(source: &Path, target: &Path) -> ExecutorResult<()> {
    if target == source {
        return Err(ExecutorError::new(
            ErrorCode::Validation,
            format!(
                "the mirror folder {} is the same folder as its source — there is nothing to copy",
                display_path(target)
            ),
        )
        .permanent()
        .with_hint("Choose a mirror folder outside the folder you are backing up."));
    }
    if target.starts_with(source) {
        return Err(ExecutorError::new(
            ErrorCode::Validation,
            format!(
                "the mirror folder {} is inside its own source {} — that would copy forever",
                display_path(target),
                display_path(source)
            ),
        )
        .permanent()
        .with_hint("Choose a mirror folder outside the folder you are backing up."));
    }
    if source.starts_with(target) {
        return Err(ExecutorError::new(
            ErrorCode::Validation,
            format!(
                "the source {} is inside the mirror folder {} — the mirror would overwrite the original",
                display_path(source),
                display_path(target)
            ),
        )
        .permanent()
        .with_hint("Choose a mirror folder outside the folder you are backing up."));
    }
    Ok(())
}

/// Join `rel` onto `root`, refusing anything that escapes.
///
/// `rel` always comes from `walkdir`'s own relative paths, which contain no
/// `..`, but this is the last line of defence before a write and costs
/// nothing: a rejected join is a warning, not a corrupted filesystem.
fn safe_join(root: &Path, rel: &Path) -> Option<PathBuf> {
    for component in rel.components() {
        match component {
            Component::Normal(_) => {}
            // Anything else — `..`, a root, a drive prefix — would relocate
            // the join.
            _ => return None,
        }
    }
    let joined = root.join(rel);
    joined.starts_with(root).then_some(joined)
}

/// True when `path` has no parent inside the filesystem, i.e. it is `C:\`,
/// `\\?\C:\`, `/`, or a bare UNC share. Deleting recursively from such a root
/// is never what the user meant.
fn is_filesystem_root(path: &Path) -> bool {
    match path.parent() {
        None => true,
        Some(parent) => {
            // `\\?\C:\`.parent() is `\\?\C:` on Windows, which is still a
            // prefix-only path with no normal components.
            !parent.components().any(|c| matches!(c, Component::Normal(_)))
                && !path.components().any(|c| matches!(c, Component::Normal(_)))
        }
    }
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum ScanItem {
    Dir(PathBuf),
    File { rel: PathBuf, len: u64, mtime: Option<SystemTime> },
    Warning(String),
}

/// Walk `root`, applying exclusions, and stream what should be mirrored.
///
/// Runs on a blocking worker. Every send is a backpressure point, and a failed
/// send (receiver dropped) ends the walk immediately — that is the
/// cancellation path for a walk that is stuck in a huge directory.
fn scan(
    root: &Path,
    options: &MirrorOptions,
    matcher: &ExclusionMatcher,
    cancel: &CancelToken,
    tx: &tokio::sync::mpsc::Sender<ScanItem>,
) {
    let max_bytes = options.max_file_size_mb.map(|mb| mb.saturating_mul(1024 * 1024));
    let walker = walkdir::WalkDir::new(root)
        .follow_links(options.follow_symlinks)
        .min_depth(1)
        .into_iter();

    let it = walker.filter_entry(|entry| {
        if entry.file_type().is_dir() {
            let Ok(rel) = entry.path().strip_prefix(root) else { return false };
            if matcher.matches_dir(rel) {
                return false;
            }
            if options.respect_cachedir_tag && has_cachedir_tag(entry.path()) {
                return false;
            }
        }
        true
    });

    for next in it {
        if cancel.is_cancelled() {
            return;
        }
        let entry = match next {
            Ok(e) => e,
            Err(e) => {
                // walkdir surfaces permission errors and symlink loops here.
                // Neither should fail the whole run.
                if tx.blocking_send(ScanItem::Warning(format!("skipped: {e}"))).is_err() {
                    return;
                }
                continue;
            }
        };
        let Ok(rel) = entry.path().strip_prefix(root) else { continue };
        let rel = rel.to_path_buf();

        // `file_type` on a `follow_links(false)` walk describes the link
        // itself, which is exactly what we want to detect here.
        if entry.file_type().is_symlink() {
            let msg = format!(
                "skipped the link {} (enable \"follow symlinks\" on the source to include it)",
                rel.display()
            );
            if tx.blocking_send(ScanItem::Warning(msg)).is_err() {
                return;
            }
            continue;
        }

        if entry.file_type().is_dir() {
            if tx.blocking_send(ScanItem::Dir(rel)).is_err() {
                return;
            }
            continue;
        }

        if matcher.matches_file(&rel) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                if tx
                    .blocking_send(ScanItem::Warning(format!("cannot read {}: {e}", rel.display())))
                    .is_err()
                {
                    return;
                }
                continue;
            }
        };
        let len = metadata.len();
        if let Some(max) = max_bytes {
            if len > max {
                continue;
            }
        }
        let mtime = metadata.modified().ok();
        if tx.blocking_send(ScanItem::File { rel, len, mtime }).is_err() {
            return;
        }
    }
}

/// Does this directory carry a valid `CACHEDIR.TAG`?
fn has_cachedir_tag(dir: &Path) -> bool {
    use std::io::Read;
    let tag = dir.join("CACHEDIR.TAG");
    let Ok(mut file) = std::fs::File::open(tag) else { return false };
    let mut buf = [0u8; 43];
    match file.read_exact(&mut buf) {
        Ok(()) => buf == CACHEDIR_SIGNATURE,
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Copying
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum CopyError {
    Cancelled,
    /// Recorded as a warning and stepped over — a single locked file must not
    /// fail an otherwise good backup.
    Io(String),
}

/// Is the destination already a byte-for-byte-plausible copy?
///
/// Size plus modification time, which is what every incremental copier uses:
/// hashing every file would make an incremental run cost the same as a full
/// one, defeating the point.
fn is_up_to_date(dest: &Path, len: u64, mtime: Option<SystemTime>) -> bool {
    let Ok(meta) = std::fs::metadata(dest) else { return false };
    if meta.len() != len {
        return false;
    }
    match (meta.modified().ok(), mtime) {
        (Some(a), Some(b)) => within_tolerance(a, b),
        // Without a usable timestamp on either side, fall back to size alone
        // rather than re-copying the tree on every run.
        _ => true,
    }
}

fn within_tolerance(a: SystemTime, b: SystemTime) -> bool {
    let delta = match a.duration_since(b) {
        Ok(d) => d,
        Err(e) => e.duration(),
    };
    delta.as_secs() as i64 <= MTIME_TOLERANCE_SECS
}

fn copy_file_blocking(
    src: &Path,
    dest: &Path,
    len: u64,
    mtime: Option<SystemTime>,
    bucket: Option<&TokenBucket>,
    cancel: &CancelToken,
) -> Result<u64, CopyError> {
    use std::io::{Read, Write};

    if cancel.is_cancelled() {
        return Err(CopyError::Cancelled);
    }
    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Err(CopyError::Io(format!("cannot create {}: {e}", display_path(parent))));
        }
    }
    let mut input = std::fs::File::open(src).map_err(|e| CopyError::Io(describe_open(src, &e)))?;
    // Write through a temporary sibling and rename into place, so an
    // interrupted copy never leaves a half-written file that the next run
    // would consider "up to date" because its size happens to match.
    let temp = temp_sibling(dest);
    let mut output = std::fs::File::create(&temp)
        .map_err(|e| CopyError::Io(format!("cannot write {}: {e}", display_path(dest))))?;

    let mut buf = vec![0u8; COPY_CHUNK.min(len.max(1) as usize).max(64 * 1024)];
    let mut written: u64 = 0;
    loop {
        if cancel.is_cancelled() {
            drop(output);
            let _ = std::fs::remove_file(&temp);
            return Err(CopyError::Cancelled);
        }
        let read = match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                drop(output);
                let _ = std::fs::remove_file(&temp);
                return Err(CopyError::Io(describe_open(src, &e)));
            }
        };
        if let Some(bucket) = bucket {
            bucket.consume_blocking(read as u64, cancel);
        }
        if let Err(e) = output.write_all(&buf[..read]) {
            drop(output);
            let _ = std::fs::remove_file(&temp);
            return Err(CopyError::Io(format!("cannot write {}: {e}", display_path(dest))));
        }
        written += read as u64;
    }

    // Preserve the modification time so the next run's incremental check sees
    // a match. `File::set_modified` is portable; the `filetime` crate is not a
    // dependency of this workspace.
    if let Some(t) = mtime {
        if let Err(e) = output.set_modified(t) {
            tracing::debug!(path = %display_path(dest), error = %e, "could not preserve mtime");
        }
    }
    if let Err(e) = output.sync_all() {
        tracing::debug!(path = %display_path(dest), error = %e, "sync failed");
    }
    drop(output);

    if let Err(e) = std::fs::rename(&temp, dest) {
        let _ = std::fs::remove_file(&temp);
        return Err(CopyError::Io(format!("cannot replace {}: {e}", display_path(dest))));
    }
    Ok(written)
}

/// A temporary name beside the destination, on the same filesystem so the
/// rename is atomic.
fn temp_sibling(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".sbtmp-{}", Uuid::new_v4().simple()));
    match dest.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// Turn an open failure into a message that says what the user should do.
///
/// A file locked by another process is the single most common non-fatal
/// problem in a Windows backup (Outlook's PST, a running VM, a database), and
/// it must read as "one file was busy", not as a crash.
fn describe_open(path: &Path, error: &std::io::Error) -> String {
    let busy = matches!(error.kind(), std::io::ErrorKind::PermissionDenied)
        || matches!(error.raw_os_error(), Some(32) | Some(33));
    if busy {
        format!("{} is in use by another program and was skipped", display_path(path))
    } else {
        format!("cannot read {}: {error}", display_path(path))
    }
}

// ---------------------------------------------------------------------------
// Pruning
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct PruneReport {
    deleted: u64,
    warnings: Vec<String>,
}

/// Remove entries under `target` whose counterpart under `src_root` is gone.
///
/// Runs bottom-up so directories are empty by the time they are considered.
/// Every candidate is re-validated against `target` immediately before the
/// `remove_*` call — the check is not hoisted out of the loop, because the
/// point of it is to be the last thing that happens before deletion.
fn prune_blocking(src_root: &Path, target: &Path, cancel: &CancelToken) -> ExecutorResult<PruneReport> {
    if is_filesystem_root(target) {
        return Err(ExecutorError::new(
            ErrorCode::Validation,
            format!("refusing to delete files directly under {}", display_path(target)),
        )
        .permanent());
    }
    let mut report = PruneReport::default();
    let walker = walkdir::WalkDir::new(target)
        .follow_links(false)
        .min_depth(1)
        .contents_first(true)
        .into_iter();

    for next in walker {
        if cancel.is_cancelled() {
            return Ok(report);
        }
        let entry = match next {
            Ok(e) => e,
            Err(e) => {
                report.warnings.push(format!("cannot scan for deletions: {e}"));
                continue;
            }
        };
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(target) else { continue };
        // Leftover temporaries from an interrupted copy are always removable.
        let is_temp = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.contains(".sbtmp-"))
            .unwrap_or(false);
        if !is_temp && std::fs::symlink_metadata(src_root.join(rel)).is_ok() {
            continue;
        }
        // Final guard: never step outside the mirror subtree, and never follow
        // a link to decide what to delete.
        let Some(validated) = safe_join(target, rel) else {
            report.warnings.push(format!("refusing to delete {} — outside the mirror", rel.display()));
            continue;
        };
        if validated != path || !validated.starts_with(target) || validated == target {
            report.warnings.push(format!("refusing to delete {}", display_path(path)));
            continue;
        }
        let meta = match std::fs::symlink_metadata(&validated) {
            Ok(m) => m,
            Err(e) => {
                report.warnings.push(format!("cannot inspect {}: {e}", display_path(&validated)));
                continue;
            }
        };
        // `is_dir()` on symlink metadata is false for a link *to* a directory,
        // so a junction is removed as a link and never recursed into.
        let result = if meta.is_dir() {
            std::fs::remove_dir(&validated)
        } else {
            std::fs::remove_file(&validated)
        };
        match result {
            Ok(()) => report.deleted += 1,
            Err(e) => {
                report.warnings.push(format!("cannot delete {}: {e}", display_path(&validated)))
            }
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// Exclusions
// ---------------------------------------------------------------------------

/// Compiled form of an [`ExclusionSet`]'s patterns.
///
/// `.gitignore` syntax is translated into two globsets: one tested against
/// directories (patterns written with a trailing `/`) so whole subtrees are
/// pruned during the walk rather than filtered file by file, and one tested
/// against files. Pruning matters enormously here — the entire reason this
/// application exists is that `node_modules` makes naive copiers unusable.
#[derive(Debug)]
pub struct ExclusionMatcher {
    dirs: GlobSet,
    files: GlobSet,
}

impl ExclusionMatcher {
    /// Compile the effective patterns of `set`.
    pub fn build(set: &ExclusionSet) -> ExecutorResult<ExclusionMatcher> {
        Self::from_patterns(&set.effective_patterns())
    }

    /// Compile a raw pattern list.
    pub fn from_patterns(patterns: &[String]) -> ExecutorResult<ExclusionMatcher> {
        let mut dirs = GlobSetBuilder::new();
        let mut files = GlobSetBuilder::new();
        for raw in patterns {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let directory_only = trimmed.ends_with('/');
            let body = trimmed.trim_end_matches('/');
            // A leading `/` anchors to the source root; anything else may match
            // at any depth, which is what `.gitignore` means by a bare name.
            let normalised = match body.strip_prefix('/') {
                Some(rest) => rest.to_string(),
                None => format!("**/{body}"),
            };
            // `literal_separator` makes `*` stop at a path separator, which is
            // what `.gitignore` means and what keeps `/build/` anchored.
            // Matching is case-insensitive on Windows because NTFS is.
            let glob = GlobBuilder::new(&normalised)
                .literal_separator(true)
                .case_insensitive(cfg!(windows))
                .build()
                .map_err(|e| {
                    ExecutorError::new(
                        ErrorCode::Validation,
                        format!("exclusion pattern {raw:?} is not valid: {e}"),
                    )
                    .permanent()
                })?;
            if directory_only {
                dirs.add(glob);
            } else {
                // A pattern without a trailing slash can match either, so it
                // goes into both sets.
                dirs.add(glob.clone());
                files.add(glob);
            }
        }
        let build = |b: GlobSetBuilder| -> ExecutorResult<GlobSet> {
            b.build().map_err(|e| {
                ExecutorError::new(ErrorCode::Validation, format!("cannot compile exclusions: {e}"))
                    .permanent()
            })
        };
        Ok(ExclusionMatcher { dirs: build(dirs)?, files: build(files)? })
    }

    /// Should this directory (relative to the source root) be pruned?
    pub fn matches_dir(&self, rel: &Path) -> bool {
        self.dirs.is_match(normalise_for_match(rel))
    }

    /// Should this file (relative to the source root) be skipped?
    pub fn matches_file(&self, rel: &Path) -> bool {
        self.files.is_match(normalise_for_match(rel))
    }
}

/// Glob patterns are written with forward slashes regardless of platform, so
/// Windows paths are translated before matching. Without this, every
/// `**/node_modules/` pattern silently matches nothing on Windows.
fn normalise_for_match(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

// ---------------------------------------------------------------------------
// Progress accounting
// ---------------------------------------------------------------------------

/// Accumulates counters and pushes coalesced [`Progress`] to the sink.
#[derive(Debug)]
struct Accumulator {
    sink: ProgressSink,
    clock: Arc<dyn Clock>,
    started: chrono::DateTime<chrono::Utc>,
    progress: Progress,
    warnings: Vec<String>,
    suppressed: u64,
}

impl Accumulator {
    fn new(sink: ProgressSink, clock: Arc<dyn Clock>, started: chrono::DateTime<chrono::Utc>) -> Accumulator {
        Accumulator {
            sink,
            clock,
            started,
            progress: Progress::default(),
            warnings: Vec::new(),
            suppressed: 0,
        }
    }

    fn set_current(&mut self, rel: &Path) {
        self.progress.current_path = Some(rel.to_string_lossy().to_string());
    }

    fn copied(&mut self, bytes: u64) {
        self.progress.files_processed += 1;
        self.progress.bytes_processed += bytes;
        self.progress.bytes_uploaded += bytes;
        self.push();
    }

    fn cached(&mut self, bytes: u64) {
        self.progress.files_processed += 1;
        self.progress.files_cached += 1;
        self.progress.bytes_processed += bytes;
        self.push();
    }

    fn deleted(&mut self, count: u64) {
        if count > 0 {
            self.warnings.push(format!("removed {count} file(s) no longer present in the source"));
        }
    }

    fn ignored(&mut self, text: String) {
        self.progress.errors_ignored += 1;
        self.warn(text);
        self.push();
    }

    fn warn(&mut self, text: String) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(text);
        } else {
            self.suppressed += 1;
        }
    }

    fn push(&mut self) {
        self.recompute_rate();
        self.sink.update(self.progress.clone());
    }

    fn recompute_rate(&mut self) {
        let elapsed = (self.clock.now_utc() - self.started).num_milliseconds();
        if elapsed > 0 {
            self.progress.bytes_per_second =
                self.progress.bytes_uploaded as f64 * 1000.0 / elapsed as f64;
        }
    }

    fn finish(&mut self) -> Progress {
        self.recompute_rate();
        self.progress.current_path = None;
        self.progress.files_total = Some(self.progress.files_processed);
        self.progress.bytes_total = Some(self.progress.bytes_processed);
        self.sink.finish(self.progress.clone());
        self.progress.clone()
    }

    fn into_warnings(mut self) -> Vec<String> {
        if self.suppressed > 0 {
            self.warnings.push(format!("... and {} more similar problems", self.suppressed));
        }
        self.warnings
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Canonicalise, which on Windows produces a `\\?\`-verbatim path and thereby
/// lifts the 260-character limit for every path derived from it.
fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// Strip the verbatim prefix for anything a human will read. Users do not
/// recognise `\\?\C:\Users\...` as their own folder.
pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_string(),
    }
}

/// Folder name for a source inside the mirror root, unique within one run.
fn unique_folder_name(source: &Path, used: &mut HashSet<String>) -> String {
    let base = source
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        // A drive root (`D:\`) has no file name; name it after the drive.
        .unwrap_or_else(|| crate::model::slugify(&source.to_string_lossy()));
    let mut candidate = base.clone();
    let mut n = 2;
    while !used.insert(candidate.clone()) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    candidate
}

fn cancelled_error(cancel: &CancelToken) -> ExecutorError {
    let reason = cancel.reason().unwrap_or(CancelReason::Requested);
    ExecutorError::new(ErrorCode::JobCancelled, format!("mirror {}", reason.describe())).permanent()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_traversal() {
        let root = Path::new("/mirror/root");
        assert!(safe_join(root, Path::new("a/b.txt")).is_some());
        assert!(safe_join(root, Path::new("../escape")).is_none());
        assert!(safe_join(root, Path::new("a/../../escape")).is_none());
        assert!(safe_join(root, Path::new("/absolute")).is_none());
    }

    #[test]
    fn containment_guard_rejects_both_directions() {
        let src = Path::new("/data/projects");
        assert!(guard_containment(src, Path::new("/data/projects/backup")).is_err());
        assert!(guard_containment(src, Path::new("/data/projects")).is_err());
        assert!(guard_containment(Path::new("/data/projects/sub"), Path::new("/data/projects"))
            .is_err());
        // Each direction gets its own explanation, because the fix differs.
        let same = guard_containment(src, Path::new("/data/projects")).expect_err("same");
        assert!(same.message.contains("same folder"), "{}", same.message);
        let nested = guard_containment(src, Path::new("/data/projects/backup")).expect_err("in");
        assert!(nested.message.contains("copy forever"), "{}", nested.message);
        let inverted = guard_containment(Path::new("/mirror/p/p"), Path::new("/mirror/p"))
            .expect_err("inverted");
        assert!(inverted.message.contains("overwrite the original"), "{}", inverted.message);
        assert!(guard_containment(src, Path::new("/mirror")).is_ok());
        // A sibling whose name is a string prefix must not be confused with a
        // child: `starts_with` is component-wise, so this is fine.
        assert!(guard_containment(src, Path::new("/data/projects-mirror")).is_ok());
    }

    #[test]
    fn filesystem_roots_are_recognised() {
        assert!(is_filesystem_root(Path::new("/")));
        assert!(!is_filesystem_root(Path::new("/mirror")));
        if cfg!(windows) {
            assert!(is_filesystem_root(Path::new(r"C:\")));
            assert!(!is_filesystem_root(Path::new(r"C:\mirror")));
        }
    }

    #[test]
    fn exclusion_patterns_translate_from_gitignore_syntax() {
        let matcher = ExclusionMatcher::from_patterns(&[
            "/**/node_modules/".to_string(),
            "/**/*.log".to_string(),
            "/build/".to_string(),
        ])
        .expect("compile");
        assert!(matcher.matches_dir(Path::new("node_modules")));
        assert!(matcher.matches_dir(Path::new("packages/app/node_modules")));
        assert!(matcher.matches_file(Path::new("logs/run.log")));
        assert!(!matcher.matches_file(Path::new("logs/run.txt")));
        // Anchored: only the root-level `build`.
        assert!(matcher.matches_dir(Path::new("build")));
        assert!(!matcher.matches_dir(Path::new("sub/build")));
    }

    #[test]
    fn windows_paths_are_normalised_before_matching() {
        let matcher =
            ExclusionMatcher::from_patterns(&["/**/node_modules/".to_string()]).expect("compile");
        assert!(matcher.matches_dir(Path::new(r"packages\app\node_modules")));
    }

    #[test]
    fn developer_defaults_prune_the_usual_suspects() {
        let matcher =
            ExclusionMatcher::build(&crate::model::ExclusionSet::developer_defaults()).expect("c");
        assert!(matcher.matches_dir(Path::new("web/node_modules")));
        assert!(matcher.matches_dir(Path::new("web/.next/cache")));
        assert!(matcher.matches_file(Path::new("a/b/Thumbs.db")));
        assert!(!matcher.matches_file(Path::new("src/main.rs")));
    }

    #[test]
    fn verbatim_prefixes_are_hidden_from_humans() {
        assert_eq!(display_path(Path::new(r"\\?\C:\Users\me")), r"C:\Users\me");
        assert_eq!(display_path(Path::new(r"\\?\UNC\server\share\x")), r"\\server\share\x");
        assert_eq!(display_path(Path::new("/home/me")), "/home/me");
    }

    #[test]
    fn source_folder_names_are_deduplicated() {
        let mut used = HashSet::new();
        assert_eq!(unique_folder_name(Path::new("/a/Projects"), &mut used), "Projects");
        assert_eq!(unique_folder_name(Path::new("/b/Projects"), &mut used), "Projects-2");
        assert_eq!(unique_folder_name(Path::new("/c/Projects"), &mut used), "Projects-3");
    }

    #[test]
    fn mtime_tolerance_absorbs_filesystem_rounding() {
        let base = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        assert!(within_tolerance(base, base + std::time::Duration::from_secs(2)));
        assert!(within_tolerance(base + std::time::Duration::from_secs(2), base));
        assert!(!within_tolerance(base, base + std::time::Duration::from_secs(5)));
    }
}
