//! Mirror engine: incremental copying, exclusions, deletion guards, traversal
//! guards, locked files and cancellation.
//!
//! These are the only engine tests that touch a real filesystem, because the
//! things being tested — mtime preservation, path traversal, junctions, long
//! paths, sharing violations — are properties of the filesystem, and a faked
//! one would test nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use superbackup_core::engine::cancel::{CancelReason, CancelToken};
use superbackup_core::engine::clock::{Clock, SystemClock};
use superbackup_core::engine::executor::{ProgressSink, ProgressUpdate};
use superbackup_core::engine::testing::test_mirror;
use superbackup_core::engine::{MirrorEngine, MirrorOptions, MirrorRequest, ResolvedBandwidth};
use superbackup_core::model::{ExclusionPreset, ExclusionSet, Source};
use superbackup_core::state::Progress;
use uuid::Uuid;

/// A scratch directory that removes itself.
///
/// `tempfile` is not a dependency of this workspace, so this is hand-rolled;
/// it is deliberately placed under the OS temp directory with a unique name so
/// concurrent test binaries cannot collide.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Scratch {
        let root = std::env::temp_dir()
            .join("superbackup-engine-tests")
            .join(format!("{label}-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&root).expect("create scratch");
        Scratch { root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&path, contents).expect("write file");
        path
    }

    fn dir(&self, rel: &str) -> PathBuf {
        let path = self.path(rel);
        fs::create_dir_all(&path).expect("create dir");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn sink() -> (ProgressSink, tokio::sync::mpsc::UnboundedReceiver<ProgressUpdate>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (
        ProgressSink::new(
            tx,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Arc::new(SystemClock::new()),
        ),
        rx,
    )
}

fn request(
    source: &Path,
    mirror_root: &Path,
    exclusions: ExclusionSet,
    options: MirrorOptions,
    cancel: CancelToken,
) -> (MirrorRequest, tokio::sync::mpsc::UnboundedReceiver<ProgressUpdate>) {
    let (progress, rx) = sink();
    let destination = test_mirror("mirror", mirror_root);
    (
        MirrorRequest {
            run_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            destination: Arc::new(destination),
            sources: vec![Source::new(source)],
            exclusions,
            options,
            bandwidth: ResolvedBandwidth::default(),
            progress,
            cancel,
        },
        rx,
    )
}

fn engine() -> MirrorEngine {
    MirrorEngine::new(Arc::new(SystemClock::new()))
}

/// The mirror places each source in a subfolder named after it.
fn mirrored(mirror_root: &Path, source: &Path, rel: &str) -> PathBuf {
    let name = source.file_name().expect("source has a name");
    mirror_root.join(name).join(rel)
}

#[tokio::test]
async fn a_first_run_copies_everything() {
    let scratch = Scratch::new("copy");
    let source = scratch.dir("src");
    scratch.write("src/a.txt", "alpha");
    scratch.write("src/nested/b.txt", "beta");
    let mirror_root = scratch.dir("mirror");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    let outcome = engine().run(req).await.expect("mirror succeeds");

    assert_eq!(fs::read_to_string(mirrored(&mirror_root, &source, "a.txt")).unwrap(), "alpha");
    assert_eq!(
        fs::read_to_string(mirrored(&mirror_root, &source, "nested/b.txt")).unwrap(),
        "beta"
    );
    assert_eq!(outcome.progress.files_processed, 2);
    assert_eq!(outcome.progress.files_cached, 0);
    assert!(outcome.snapshot_id.is_none(), "a mirror has no snapshot id");
}

#[tokio::test]
async fn a_second_run_copies_nothing_and_preserves_mtimes() {
    let scratch = Scratch::new("incremental");
    let source = scratch.dir("src");
    scratch.write("src/a.txt", "alpha");
    scratch.write("src/b.txt", "beta");
    let mirror_root = scratch.dir("mirror");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    engine().run(req).await.expect("first run");

    let source_mtime = fs::metadata(scratch.path("src/a.txt")).unwrap().modified().unwrap();
    let copy = mirrored(&mirror_root, &source, "a.txt");
    let copy_mtime = fs::metadata(&copy).unwrap().modified().unwrap();
    let delta = copy_mtime
        .duration_since(source_mtime)
        .or_else(|e| Ok::<_, std::time::SystemTimeError>(e.duration()))
        .unwrap();
    assert!(delta.as_secs() <= 2, "the copy must keep the source's modification time");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    let outcome = engine().run(req).await.expect("second run");
    assert_eq!(outcome.progress.files_cached, 2, "an unchanged tree must re-copy nothing");
    assert_eq!(outcome.progress.bytes_uploaded, 0);
}

#[tokio::test]
async fn a_changed_file_is_recopied() {
    let scratch = Scratch::new("changed");
    let source = scratch.dir("src");
    scratch.write("src/a.txt", "alpha");
    let mirror_root = scratch.dir("mirror");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    engine().run(req).await.expect("first run");

    scratch.write("src/a.txt", "alpha plus more content");
    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    let outcome = engine().run(req).await.expect("second run");
    assert_eq!(outcome.progress.files_cached, 0);
    assert_eq!(
        fs::read_to_string(mirrored(&mirror_root, &source, "a.txt")).unwrap(),
        "alpha plus more content"
    );
}

#[tokio::test]
async fn exclusions_prune_whole_subtrees() {
    let scratch = Scratch::new("exclusions");
    let source = scratch.dir("src");
    scratch.write("src/app.js", "code");
    scratch.write("src/node_modules/react/index.js", "vendor");
    scratch.write("src/node_modules/.bin/next", "vendor");
    scratch.write("src/debug.log", "noise");
    let mirror_root = scratch.dir("mirror");

    let exclusions = ExclusionSet {
        presets: vec![ExclusionPreset::NodeModules, ExclusionPreset::LogsAndTemp],
        ..ExclusionSet::default()
    };
    let (req, _rx) =
        request(&source, &mirror_root, exclusions, MirrorOptions::default(), CancelToken::new());
    let outcome = engine().run(req).await.expect("mirror");

    assert!(mirrored(&mirror_root, &source, "app.js").exists());
    assert!(!mirrored(&mirror_root, &source, "node_modules").exists(), "the tree must be pruned");
    assert!(!mirrored(&mirror_root, &source, "debug.log").exists());
    assert_eq!(outcome.progress.files_processed, 1);
}

#[tokio::test]
async fn max_file_size_is_honoured() {
    let scratch = Scratch::new("maxsize");
    let source = scratch.dir("src");
    scratch.write("src/small.txt", "x");
    scratch.write("src/big.bin", &"x".repeat(2 * 1024 * 1024));
    let mirror_root = scratch.dir("mirror");

    let options = MirrorOptions { max_file_size_mb: Some(1), ..MirrorOptions::default() };
    let (req, _rx) =
        request(&source, &mirror_root, ExclusionSet::default(), options, CancelToken::new());
    engine().run(req).await.expect("mirror");

    assert!(mirrored(&mirror_root, &source, "small.txt").exists());
    assert!(!mirrored(&mirror_root, &source, "big.bin").exists());
}

#[tokio::test]
async fn cachedir_tagged_folders_are_skipped() {
    let scratch = Scratch::new("cachedir");
    let source = scratch.dir("src");
    scratch.write("src/keep.txt", "keep");
    scratch.write(
        "src/cache/CACHEDIR.TAG",
        "Signature: 8a477f597d28d172789f06886806bc55\nthis is a cache directory",
    );
    scratch.write("src/cache/blob.bin", "junk");
    let mirror_root = scratch.dir("mirror");

    let options = MirrorOptions { respect_cachedir_tag: true, ..MirrorOptions::default() };
    let (req, _rx) =
        request(&source, &mirror_root, ExclusionSet::default(), options, CancelToken::new());
    engine().run(req).await.expect("mirror");

    assert!(mirrored(&mirror_root, &source, "keep.txt").exists());
    assert!(!mirrored(&mirror_root, &source, "cache").exists());
}

// ---------------------------------------------------------------------------
// Deletion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn extraneous_files_survive_unless_deletion_is_requested() {
    let scratch = Scratch::new("nodelete");
    let source = scratch.dir("src");
    scratch.write("src/a.txt", "alpha");
    let mirror_root = scratch.dir("mirror");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    engine().run(req).await.expect("first run");

    // Something else appears in the mirror, and the source file disappears.
    let stray = mirrored(&mirror_root, &source, "stray.txt");
    fs::write(&stray, "not from the source").expect("write stray");
    fs::remove_file(scratch.path("src/a.txt")).expect("remove source file");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    engine().run(req).await.expect("second run");

    assert!(stray.exists(), "deletion is off by default");
    assert!(
        mirrored(&mirror_root, &source, "a.txt").exists(),
        "a vanished source file is not removed unless asked"
    );
}

#[tokio::test]
async fn deletion_removes_only_what_the_source_lost() {
    let scratch = Scratch::new("delete");
    let source = scratch.dir("src");
    scratch.write("src/keep.txt", "keep");
    scratch.write("src/gone.txt", "gone");
    scratch.write("src/sub/also-gone.txt", "gone");
    let mirror_root = scratch.dir("mirror");

    let options = MirrorOptions { delete_extraneous: true, ..MirrorOptions::default() };
    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        options.clone(),
        CancelToken::new(),
    );
    engine().run(req).await.expect("first run");
    assert!(mirrored(&mirror_root, &source, "gone.txt").exists());

    fs::remove_file(scratch.path("src/gone.txt")).expect("remove");
    fs::remove_dir_all(scratch.path("src/sub")).expect("remove");

    let (req, _rx) =
        request(&source, &mirror_root, ExclusionSet::default(), options, CancelToken::new());
    engine().run(req).await.expect("second run");

    assert!(mirrored(&mirror_root, &source, "keep.txt").exists(), "surviving files stay");
    assert!(!mirrored(&mirror_root, &source, "gone.txt").exists());
    assert!(!mirrored(&mirror_root, &source, "sub").exists());
    // Nothing above the mirror root was touched.
    assert!(scratch.path("src").exists());
    assert!(mirror_root.exists());
}

#[tokio::test]
async fn deletion_never_reaches_above_the_mirror_root() {
    let scratch = Scratch::new("delete-scope");
    let source = scratch.dir("src");
    scratch.write("src/a.txt", "alpha");
    let mirror_root = scratch.dir("mirror");
    // A sibling of the mirror root that must survive.
    let sibling = scratch.write("precious.txt", "do not delete me");

    let options = MirrorOptions { delete_extraneous: true, ..MirrorOptions::default() };
    let (req, _rx) =
        request(&source, &mirror_root, ExclusionSet::default(), options, CancelToken::new());
    engine().run(req).await.expect("mirror");

    assert!(sibling.exists(), "deletion must be confined to the mirror subtree");
    assert!(scratch.path("src/a.txt").exists(), "the source is never deleted from");
}

// ---------------------------------------------------------------------------
// Traversal guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_mirror_inside_its_own_source_is_refused() {
    let scratch = Scratch::new("recursion");
    let source = scratch.dir("src");
    scratch.write("src/a.txt", "alpha");
    // The classic mistake: back up C:\data into C:\data\backup.
    let mirror_root = scratch.dir("src/backup");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    let error = engine().run(req).await.expect_err("must be refused");
    assert!(
        error.message.contains("inside its own source"),
        "the message must explain the loop: {}",
        error.message
    );
    assert_eq!(error.retryable, superbackup_core::engine::Retryable::Permanent);
}

#[tokio::test]
async fn a_mirror_that_lands_on_its_own_source_is_refused() {
    // The source sits directly under the mirror root, so the folder the
    // mirror would write into *is* the source.
    let scratch = Scratch::new("same");
    let mirror_root = scratch.dir("mirror");
    let source = scratch.dir("mirror/src");
    scratch.write("mirror/src/a.txt", "alpha");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    let error = engine().run(req).await.expect_err("must be refused");
    assert!(error.message.contains("same folder"), "{}", error.message);
    assert!(scratch.path("mirror/src/a.txt").exists(), "the source is untouched");
}

#[tokio::test]
async fn a_source_inside_its_own_mirror_target_is_refused() {
    // A colliding basename makes the target an ancestor of the source:
    // <mirror>/Projects would be written from <mirror>/Projects/Projects.
    let scratch = Scratch::new("inverted");
    let mirror_root = scratch.dir("mirror");
    let source = scratch.dir("mirror/Projects/Projects");
    scratch.write("mirror/Projects/Projects/a.txt", "alpha");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    let error = engine().run(req).await.expect_err("must be refused");
    assert!(error.message.contains("overwrite the original"), "{}", error.message);
    assert!(scratch.path("mirror/Projects/Projects/a.txt").exists(), "the source is untouched");
}

#[tokio::test]
async fn a_missing_source_fails_permanently_rather_than_retrying_forever() {
    let scratch = Scratch::new("missing");
    let mirror_root = scratch.dir("mirror");
    let source = scratch.path("does-not-exist");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    let error = engine().run(req).await.expect_err("must fail");
    assert_eq!(error.retryable, superbackup_core::engine::Retryable::Permanent);
}

// ---------------------------------------------------------------------------
// Symlinks and junctions
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn make_dir_link(link: &Path, target: &Path) -> bool {
    std::os::windows::fs::symlink_dir(target, link).is_ok()
}

#[cfg(not(windows))]
fn make_dir_link(link: &Path, target: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[tokio::test]
async fn links_out_of_the_tree_are_not_followed_by_default() {
    let scratch = Scratch::new("symlink");
    let source = scratch.dir("src");
    scratch.write("src/a.txt", "alpha");
    let outside = scratch.dir("outside");
    scratch.write("outside/secret.txt", "must not be copied");
    let mirror_root = scratch.dir("mirror");

    if !make_dir_link(&scratch.path("src/escape"), &outside) {
        // Creating symlinks needs Developer Mode or elevation on Windows.
        // Skipping is better than a test that fails on a normal workstation.
        eprintln!("skipping: this machine cannot create directory links");
        return;
    }

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    let outcome = engine().run(req).await.expect("mirror");

    assert!(mirrored(&mirror_root, &source, "a.txt").exists());
    assert!(
        !mirrored(&mirror_root, &source, "escape/secret.txt").exists(),
        "a link out of the tree must not be followed"
    );
    assert!(
        outcome.warnings.iter().any(|w| w.contains("escape")),
        "the skipped link must be reported: {:?}",
        outcome.warnings
    );
}

// ---------------------------------------------------------------------------
// Robustness
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg(windows)]
async fn a_locked_file_is_a_warning_not_a_failure() {
    use std::os::windows::fs::OpenOptionsExt;

    let scratch = Scratch::new("locked");
    let source = scratch.dir("src");
    scratch.write("src/open.dat", "in use");
    scratch.write("src/fine.txt", "readable");
    let mirror_root = scratch.dir("mirror");

    // Open with no sharing at all: any other open fails with a sharing
    // violation, which is exactly what Outlook and a running VM do.
    let _held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(scratch.path("src/open.dat"))
        .expect("hold the file open");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    let outcome = engine().run(req).await.expect("a locked file must not fail the run");

    assert!(mirrored(&mirror_root, &source, "fine.txt").exists(), "other files still copy");
    assert_eq!(outcome.progress.errors_ignored, 1);
    assert!(
        outcome.warnings.iter().any(|w| w.contains("in use")),
        "the busy file must be reported: {:?}",
        outcome.warnings
    );
}

#[tokio::test]
async fn cancellation_stops_the_copy() {
    let scratch = Scratch::new("cancel");
    let source = scratch.dir("src");
    for i in 0..400 {
        scratch.write(&format!("src/file-{i:04}.txt",), &"x".repeat(4096));
    }
    let mirror_root = scratch.dir("mirror");

    let cancel = CancelToken::new();
    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        cancel.clone(),
    );
    // Cancel before the engine even starts: the result must be a cancellation,
    // not a partial success reported as success.
    cancel.cancel(CancelReason::Requested);
    let error = engine().run(req).await.expect_err("cancelled");
    assert!(error.is_cancellation(), "{error:?}");
}

#[tokio::test]
async fn progress_is_streamed_while_copying() {
    let scratch = Scratch::new("progress");
    let source = scratch.dir("src");
    for i in 0..5 {
        scratch.write(&format!("src/f{i}.txt"), "content");
    }
    let mirror_root = scratch.dir("mirror");

    let (req, mut rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    engine().run(req).await.expect("mirror");

    let mut updates: Vec<Progress> = Vec::new();
    let mut saw_final = false;
    while let Ok(update) = rx.try_recv() {
        saw_final |= update.final_update;
        updates.push(update.progress);
    }
    assert!(saw_final, "the terminal frame must always be emitted");
    let last = updates.last().expect("at least one update");
    assert_eq!(last.files_processed, 5);
    assert_eq!(last.files_total, Some(5));
    assert!(last.current_path.is_none(), "the final frame clears the current path");
}

#[tokio::test]
async fn several_sources_get_separate_folders() {
    let scratch = Scratch::new("multi");
    let first = scratch.dir("one/Projects");
    let second = scratch.dir("two/Projects");
    scratch.write("one/Projects/a.txt", "first");
    scratch.write("two/Projects/b.txt", "second");
    let mirror_root = scratch.dir("mirror");

    let (progress, _rx) = sink();
    let req = MirrorRequest {
        run_id: Uuid::new_v4(),
        job_id: Uuid::new_v4(),
        destination: Arc::new(test_mirror("mirror", &mirror_root)),
        sources: vec![Source::new(&first), Source::new(&second)],
        exclusions: ExclusionSet::default(),
        options: MirrorOptions::default(),
        bandwidth: ResolvedBandwidth::default(),
        progress,
        cancel: CancelToken::new(),
    };
    engine().run(req).await.expect("mirror");

    assert!(mirror_root.join("Projects").join("a.txt").exists());
    assert!(
        mirror_root.join("Projects-2").join("b.txt").exists(),
        "colliding source names must not merge into one folder"
    );
}

#[tokio::test]
async fn a_long_path_round_trips() {
    let scratch = Scratch::new("longpath");
    let source = scratch.dir("src");
    // Well past the legacy 260-character MAX_PATH once the mirror prefix is
    // added on top.
    let deep: String = (0..12).map(|i| format!("a-very-long-directory-name-{i:02}/")).collect();
    scratch.write(&format!("src/{deep}leaf.txt"), "deep");
    let mirror_root = scratch.dir("mirror");

    let (req, _rx) = request(
        &source,
        &mirror_root,
        ExclusionSet::default(),
        MirrorOptions::default(),
        CancelToken::new(),
    );
    let outcome = engine().run(req).await.expect("a deep tree must copy");
    assert_eq!(outcome.progress.files_processed, 1, "{:?}", outcome.warnings);

    let expected = mirrored(&mirror_root, &source, &format!("{deep}leaf.txt"));
    assert!(expected.exists() || expected.metadata().is_ok(), "the deep copy must exist");
}

#[tokio::test]
async fn the_mirror_clock_is_the_injected_one() {
    // Guards against a regression to `Utc::now()` inside the mirror: with a
    // frozen clock the rate is zero rather than a division-by-elapsed panic.
    let scratch = Scratch::new("clock");
    let source = scratch.dir("src");
    scratch.write("src/a.txt", "alpha");
    let mirror_root = scratch.dir("mirror");

    let clock = Arc::new(superbackup_core::engine::clock::TestClock::at("2025-01-01T00:00:00Z"));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let progress =
        ProgressSink::new(tx, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), clock.clone());
    let req = MirrorRequest {
        run_id: Uuid::new_v4(),
        job_id: Uuid::new_v4(),
        destination: Arc::new(test_mirror("mirror", &mirror_root)),
        sources: vec![Source::new(&source)],
        exclusions: ExclusionSet::default(),
        options: MirrorOptions::default(),
        bandwidth: ResolvedBandwidth::default(),
        progress,
        cancel: CancelToken::new(),
    };
    let outcome = MirrorEngine::new(clock.clone()).run(req).await.expect("mirror");
    assert_eq!(outcome.progress.bytes_per_second, 0.0);
    assert_eq!(clock.now_utc().to_rfc3339(), "2025-01-01T00:00:00+00:00");
}

/// A dry run must not create, write, or delete anything at all.
///
/// The runner dispatches on `DestinationKind::is_repository()` and drives the
/// mirror engine directly, so swapping in a rehearsal executor never reached a
/// mirror — a "dry run" against a mirror destination copied every byte for
/// real. That is the one destination kind where the mistake writes gigabytes to
/// the user's disk, so the guarantee is pinned here.
#[tokio::test]
async fn a_dry_run_writes_absolutely_nothing() {
    let scratch = Scratch::new("mirror-dryrun");
    let source = scratch.root.join("src");
    let mirror_root = scratch.root.join("mirror");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("a.txt"), vec![b'a'; 4096]).unwrap();
    fs::write(source.join("nested/b.bin"), vec![b'b'; 16_384]).unwrap();

    let options = MirrorOptions { dry_run: true, ..MirrorOptions::default() };

    let cancel = CancelToken::new();
    let (req, _rx) =
        request(&source, &mirror_root, ExclusionSet::default(), options, cancel.clone());
    let engine = MirrorEngine::new(Arc::new(SystemClock::new()));
    let outcome = engine.run(req).await.expect("a dry run should succeed");

    // Nothing on disk. Not even the mirror root, because on a removable drive
    // a stray empty folder is the difference between "nothing happened" and
    // "something happened".
    assert!(
        !mirror_root.exists(),
        "the dry run created {} — it must not touch the filesystem",
        mirror_root.display()
    );

    // But it must still report what it *would* have done, or the rehearsal is
    // useless.
    assert!(
        outcome.progress.files_processed >= 2,
        "a dry run must still count the files it would copy, saw {}",
        outcome.progress.files_processed
    );
    assert!(
        outcome.progress.bytes_processed >= 20_480,
        "a dry run must still total the bytes it would copy, saw {}",
        outcome.progress.bytes_processed
    );
}

/// The same guarantee when the destination already exists and pruning is on:
/// a rehearsal must not delete a file whose source has gone.
#[tokio::test]
async fn a_dry_run_never_prunes() {
    let scratch = Scratch::new("mirror-dryrun-prune");
    let source = scratch.root.join("src");
    let mirror_root = scratch.root.join("mirror");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("kept.txt"), b"kept").unwrap();

    // A file in the mirror whose source has disappeared: the prune candidate.
    let job_dir = mirror_root.join("mirror");
    fs::create_dir_all(&job_dir).unwrap();
    let orphan = job_dir.join("gone.txt");
    fs::write(&orphan, b"precious").unwrap();

    let options =
        MirrorOptions { delete_extraneous: true, dry_run: true, ..MirrorOptions::default() };

    let cancel = CancelToken::new();
    let (req, _rx) =
        request(&source, &mirror_root, ExclusionSet::default(), options, cancel.clone());
    let engine = MirrorEngine::new(Arc::new(SystemClock::new()));
    engine.run(req).await.expect("a dry run should succeed");

    assert!(
        orphan.exists(),
        "the dry run deleted {} — a rehearsal must never destroy data",
        orphan.display()
    );
    assert_eq!(fs::read(&orphan).unwrap(), b"precious");
}
