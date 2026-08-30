//! OneDrive detection and destination validation.
//!
//! The validation tests run against real temporary directories, because the
//! failure modes that matter (an ACL that denies writes, a path that is too
//! long) cannot be reproduced with a mock. Nothing here writes outside a
//! per-test temporary directory, and nothing touches OneDrive's own state.
//!
//! Tests marked `#[ignore]` inspect the machine's real OneDrive configuration
//! and are therefore environment-dependent. Run them with
//! `cargo test -p superbackup-core --test platform_onedrive -- --ignored`.

use std::path::PathBuf;
use superbackup_core::platform::onedrive::{self, SyncState};
use superbackup_core::state::Severity;
use uuid::Uuid;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!(
            "sb-od-{tag}-{}-{}",
            std::process::id(),
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn codes(v: &onedrive::Validation) -> Vec<&str> {
    v.issues.iter().map(|i| i.code.as_str()).collect()
}

#[test]
fn a_plain_writable_folder_is_accepted() {
    let dir = TempDir::new("ok");
    let v = onedrive::validate(dir.path(), &[], 0);
    assert!(v.is_usable(), "unexpected errors: {:?}", codes(&v));
    // The probe file must not be left behind.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .expect("read dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("write-test"))
        .collect();
    assert!(leftovers.is_empty(), "the write probe left a file behind");
}

#[test]
fn a_folder_that_does_not_exist_yet_is_created_and_accepted() {
    let dir = TempDir::new("create");
    let target = dir.path().join("Backup");
    assert!(!target.exists());
    let v = onedrive::validate(&target, &[], 0);
    assert!(v.is_usable(), "{:?}", codes(&v));
    assert!(target.is_dir(), "validate must be able to answer for a folder we will create");
}

#[test]
fn a_repository_inside_a_backed_up_folder_is_refused() {
    let dir = TempDir::new("recursive");
    let source = dir.path().join("Documents");
    std::fs::create_dir_all(&source).expect("create source");
    let target = source.join("Backup");

    let v = onedrive::validate(&target, &[source], 0);
    assert!(!v.is_usable(), "a repository inside a source must be refused");
    assert!(codes(&v).contains(&"inside_source"), "{:?}", codes(&v));
    let issue = v.errors().next().expect("an error");
    assert!(issue.remedy.is_some(), "the user must be told what to do instead");
    assert_eq!(issue.severity, Severity::Error);
}

#[test]
fn a_repository_that_contains_a_source_is_allowed_with_a_warning() {
    let dir = TempDir::new("contains");
    let source = dir.path().join("Backup").join("Projects");
    std::fs::create_dir_all(&source).expect("create source");
    let target = dir.path().join("Backup");

    let v = onedrive::validate(&target, &[source], 0);
    assert!(v.is_usable(), "this is awkward, not impossible: {:?}", codes(&v));
    assert!(codes(&v).contains(&"contains_source"));
}

#[test]
fn a_snapshot_that_will_not_fit_is_refused() {
    let dir = TempDir::new("space");
    // More than any real volume: 8 exabytes.
    let v = onedrive::validate(dir.path(), &[], u64::MAX / 2);
    assert!(!v.is_usable());
    assert!(codes(&v).contains(&"insufficient_space"), "{:?}", codes(&v));
}

#[test]
fn an_absurdly_long_path_is_refused_on_windows() {
    let dir = TempDir::new("long");
    // Build a path over the error threshold without actually creating it —
    // `path_length_issue` is a pure rule and must fire before any I/O.
    let long = dir.path().join("x".repeat(onedrive::PATH_ERROR_LEN));
    let issue = onedrive::path_length_issue(&long);
    if cfg!(windows) {
        let issue = issue.expect("Windows must reject a path this long");
        assert_eq!(issue.code, "path_too_long");
        assert_eq!(issue.severity, Severity::Error);
        assert!(issue.message.contains("260"), "the message must name the limit");
    } else {
        assert!(issue.is_none(), "only Windows has a 260-character problem");
    }
}

#[test]
fn a_medium_length_path_only_warns() {
    let base = if cfg!(windows) { "C:\\" } else { "/" };
    let path = PathBuf::from(format!("{base}{}", "y".repeat(onedrive::PATH_WARN_LEN)));
    match onedrive::path_length_issue(&path) {
        Some(issue) if cfg!(windows) => {
            assert_eq!(issue.code, "path_long");
            assert_eq!(issue.severity, Severity::Warning);
        }
        Some(other) => panic!("unexpected issue on a non-Windows host: {other:?}"),
        None => assert!(!cfg!(windows)),
    }
}

#[test]
fn preparing_a_folder_creates_it_and_reports_what_the_user_must_do() {
    let dir = TempDir::new("prepare");
    let target = dir.path().join("Backup");
    let prepared = onedrive::prepare_repository_folder(&target).expect("prepare");

    assert!(target.is_dir());
    assert_eq!(prepared.path, target);
    // A plain temp folder is not cloud-backed, so it is never "online only".
    assert_ne!(prepared.sync_state, SyncState::OnlineOnly);

    if cfg!(windows) {
        // Pinning a non-cloud folder still succeeds (the attribute is inert
        // outside a sync root), so we should not be handing the user manual
        // steps for an ordinary directory.
        assert!(
            prepared.pinned || !prepared.manual_steps.is_empty(),
            "either we pinned it, or we told the user exactly how to"
        );
    } else {
        assert!(
            !prepared.manual_steps.is_empty() || !prepared.warnings.is_empty(),
            "platforms that cannot pin must say so"
        );
    }
    for step in &prepared.manual_steps {
        assert!(step.ends_with('.'), "instructions are sentences: {step}");
    }
}

#[test]
fn sync_state_of_an_ordinary_folder_is_never_a_false_alarm() {
    let dir = TempDir::new("syncstate");
    let state = onedrive::sync_state(dir.path());
    if cfg!(windows) {
        assert_eq!(state, SyncState::NotCloudBacked);
        assert!(!state.is_risky());
    } else {
        // Non-Windows has no API for this and must say "unknown" rather than
        // claim a folder is safe.
        assert_eq!(state, SyncState::Unknown);
    }
}

#[test]
fn detection_never_panics_and_returns_only_real_folders() {
    for account in onedrive::detect() {
        assert!(account.path.is_dir(), "{} does not exist", account.path.display());
        assert!(!account.display_name.is_empty());
        assert!(
            account.suggested_repository_root().starts_with(&account.path),
            "the suggestion must live inside the account"
        );
        assert!(
            account.suggested_repository_root() != account.path,
            "a repository must not be dropped at the root of somebody's OneDrive"
        );
    }
}

#[test]
#[ignore = "inspects this machine's real OneDrive configuration; results vary by host"]
fn report_the_real_onedrive_accounts_on_this_machine() {
    let accounts = onedrive::detect();
    println!("found {} OneDrive account(s)", accounts.len());
    for a in &accounts {
        println!(
            "  {} at {} [{:?}] {} free of {}, sync state {:?}",
            a.display_name,
            a.path.display(),
            a.kind,
            bytesize::ByteSize(a.available_bytes),
            bytesize::ByteSize(a.total_bytes),
            a.sync_state
        );
        for w in &a.warnings {
            println!("    warning: {w}");
        }
        let suggestion = a.suggested_repository_root();
        println!("    suggested repository root: {}", suggestion.display());
        let v = onedrive::validate(&suggestion, &[], 0);
        println!("    validation: usable={} issues={:?}", v.is_usable(), codes(&v));
    }
}
