//! The git inventory against real repositories on the machine running the
//! tests, rather than against a mock.
//!
//! Every part of this feature has unit tests over captured output, and those
//! would all pass while the scan found nothing at all: the parsing is not the
//! part that breaks, the wiring is. So this builds actual repositories with
//! actual `git`, in a temporary folder, and asserts on what comes back.
//!
//! Skipped entirely where git is not installed. A contributor without git
//! should get a passing suite and a clear reason, not a wall of failures about
//! a program they were never asked to have.

use std::path::{Path, PathBuf};
use std::process::Command;

use superbackup_core::git::{self, RepoState, ScanOptions};

fn git_available() -> bool {
    git::cli::Git::available()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sb-git-{tag}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).expect("create scratch");
    dir
}

/// Run git for the *fixture*, not through the driver: a test that built its
/// repositories with the code under test could not tell a broken driver from a
/// broken scan.
fn git_do(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} in {}: {e}", dir.display()));
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository with one commit and an identity that does not depend on
/// whatever the machine happens to have configured.
fn make_repo(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).expect("create repo dir");
    git_do(&dir, &["init", "--initial-branch=main"]);
    git_do(&dir, &["config", "user.name", "Test"]);
    git_do(&dir, &["config", "user.email", "test@example.invalid"]);
    git_do(&dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("README.md"), b"hello\n").expect("write");
    git_do(&dir, &["add", "."]);
    git_do(&dir, &["commit", "-m", "first"]);
    dir
}

#[tokio::test]
async fn the_scan_finds_real_repositories_and_reads_their_actual_state() {
    if !git_available() {
        eprintln!("git is not installed; skipping");
        return;
    }
    let root = scratch("scan");

    // Clean: one commit, nothing outstanding.
    make_repo(&root, "clean");

    // Uncommitted: an edit that was never committed.
    let dirty = make_repo(&root, "dirty");
    std::fs::write(dirty.join("README.md"), b"changed\n").expect("write");

    // Untracked only — the state a folder of new work is actually in, and the
    // one a naive `git diff` check misses entirely.
    let fresh = make_repo(&root, "untracked");
    std::fs::write(fresh.join("new-idea.txt"), b"nothing here yet\n").expect("write");

    // Nested one level down, to prove the walk descends.
    let nested_parent = root.join("clients").join("acme");
    std::fs::create_dir_all(&nested_parent).expect("create");
    make_repo(&nested_parent, "deep");

    // Not a repository at all.
    std::fs::create_dir_all(root.join("just-a-folder")).expect("create");
    std::fs::write(root.join("just-a-folder").join("notes.txt"), b"x").expect("write");

    let options = ScanOptions { check_remotes: false, ..Default::default() };
    let inventory =
        git::inventory(std::slice::from_ref(&root), &options).await.expect("the scan ran");

    assert!(inventory.git_available);
    assert!(!inventory.truncated, "nothing here should hit a limit");

    let names: Vec<&str> = inventory.repos.iter().map(|r| r.name.as_str()).collect();
    for expected in ["clean", "dirty", "untracked", "deep"] {
        assert!(names.contains(&expected), "{expected} was not found in {names:?}");
    }
    assert!(!names.contains(&"just-a-folder"), "a plain folder is not a repository");
    assert_eq!(inventory.repos.len(), 4, "found {names:?}");

    let by_name = |want: &str| {
        inventory.repos.iter().find(|r| r.name == want).unwrap_or_else(|| panic!("{want}"))
    };

    let clean = by_name("clean");
    assert_eq!(clean.branch.as_deref(), Some("main"));
    assert!(clean.head.is_some(), "a committed repository has a head");
    assert!(clean.last_commit_at.is_some(), "and a commit date");
    assert_eq!(clean.last_commit_summary.as_deref(), Some("first"));
    assert_eq!(clean.last_commit_author.as_deref(), Some("Test"));
    // No remote configured, so its whole history is only on this disk — which
    // is the finding this feature exists to surface.
    assert_eq!(clean.state(), RepoState::NoRemote);

    let dirty = by_name("dirty");
    assert_eq!(dirty.unstaged, 1, "one edited file");
    assert_eq!(dirty.state(), RepoState::Uncommitted);

    let untracked = by_name("untracked");
    assert_eq!(untracked.untracked, 1, "one new file git has never seen");
    assert_eq!(untracked.state(), RepoState::Uncommitted);

    // And the summary the screen leads with.
    let at_risk = inventory.at_risk();
    assert_eq!(at_risk.len(), 4, "all four are unpushed work");

    let _ = std::fs::remove_dir_all(&root);
}

/// The bound that makes this usable on the machines it was written for. A
/// `node_modules` on a real project holds hundreds of thousands of folders,
/// and a scan that walks one has effectively hung.
#[tokio::test]
async fn a_node_modules_is_never_walked_however_deep_the_scan_goes() {
    if !git_available() {
        return;
    }
    let root = scratch("skip");
    make_repo(&root, "project");

    // A repository inside node_modules is a dependency, not the user's work.
    let vendored = root.join("project").join("node_modules").join("some-package");
    std::fs::create_dir_all(&vendored).expect("create");
    make_repo(vendored.parent().expect("parent"), "some-package");

    let options = ScanOptions { max_depth: 8, check_remotes: false, ..Default::default() };
    let inventory = git::inventory(std::slice::from_ref(&root), &options).await.expect("scanned");

    let names: Vec<&str> = inventory.repos.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["project"], "only the user's own repository");

    let _ = std::fs::remove_dir_all(&root);
}

/// A repository found is not descended into: its submodules and vendored
/// checkouts are part of it, and listing them separately buries the projects
/// the user actually works in.
#[tokio::test]
async fn a_repository_inside_a_repository_is_not_listed_twice() {
    if !git_available() {
        return;
    }
    let root = scratch("nested");
    let outer = make_repo(&root, "outer");
    make_repo(&outer, "inner");

    let options = ScanOptions { max_depth: 8, check_remotes: false, ..Default::default() };
    let inventory = git::inventory(std::slice::from_ref(&root), &options).await.expect("scanned");
    let names: Vec<&str> = inventory.repos.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["outer"], "the walk stops at a repository boundary");

    let _ = std::fs::remove_dir_all(&root);
}

/// Commit and push are the two buttons on the screen. The commit half can be
/// tested for real; the push half needs a remote, which the next test builds.
#[tokio::test]
async fn committing_from_superbackup_produces_a_real_commit() {
    if !git_available() {
        return;
    }
    let root = scratch("commit");
    let repo = make_repo(&root, "work");
    std::fs::write(repo.join("new.txt"), b"work in progress\n").expect("write");
    std::fs::write(repo.join("README.md"), b"edited\n").expect("write");

    let before = git::inventory(std::slice::from_ref(&root), &ScanOptions::default()).await.expect("scan");
    assert_eq!(before.repos[0].state(), RepoState::Uncommitted);

    let outcome = git::commit(&repo, "save the work", true).await.expect("commit ran");
    assert!(outcome.ok, "{}", outcome.detail);

    let after = git::inventory(std::slice::from_ref(&root), &ScanOptions::default()).await.expect("scan");
    let repo_after = &after.repos[0];
    assert_eq!(repo_after.untracked, 0, "the new file was included");
    assert_eq!(repo_after.unstaged, 0, "and the edit");
    assert_eq!(repo_after.last_commit_summary.as_deref(), Some("save the work"));
    // No remote, so it is still only on this disk.
    assert_eq!(repo_after.state(), RepoState::NoRemote);

    // Committing again with nothing to commit is a success, not a failure:
    // the caller asked for a clean tree and has one.
    let again = git::commit(&repo, "nothing changed", true).await.expect("ran");
    assert!(again.ok, "{}", again.detail);
    assert!(again.detail.contains("Nothing to commit"), "{}", again.detail);

    let _ = std::fs::remove_dir_all(&root);
}

/// The whole point of the remote check: `.git` reports what the last fetch
/// saw, and this asks the remote instead. Built against a bare repository on
/// disk, so it needs no network and no credentials.
#[tokio::test]
async fn the_remote_check_notices_commits_that_no_fetch_has_seen() {
    if !git_available() {
        return;
    }
    let root = scratch("remote");
    let bare = root.join("origin.git");
    std::fs::create_dir_all(&bare).expect("create");
    git_do(&bare, &["init", "--bare", "--initial-branch=main"]);

    let work = root.join("work");
    std::fs::create_dir_all(&work).expect("create");
    let repo = make_repo(&work, "project");
    git_do(&repo, &["remote", "add", "origin", &bare.display().to_string()]);
    git_do(&repo, &["push", "--set-upstream", "origin", "main"]);

    let options = ScanOptions { check_remotes: true, ..Default::default() };
    let inventory = git::inventory(std::slice::from_ref(&work), &options).await.expect("scanned");
    let project = &inventory.repos[0];
    let check = project.remote_check.as_ref().expect("the remote was asked");
    assert_eq!(check.error, None, "a local bare repository is always reachable");
    assert_eq!(check.relation, Some(git::Relation::Same));
    assert_eq!(project.state(), RepoState::Clean, "committed and pushed");

    // Now move the remote on behind this clone's back — exactly what a
    // colleague pushing does — without fetching.
    let other = root.join("other");
    std::fs::create_dir_all(&other).expect("create");
    git_do(&root, &["clone", &bare.display().to_string(), &other.display().to_string()]);
    git_do(&other, &["config", "user.name", "Other"]);
    git_do(&other, &["config", "user.email", "other@example.invalid"]);
    git_do(&other, &["config", "commit.gpgsign", "false"]);
    std::fs::write(other.join("theirs.txt"), b"from someone else\n").expect("write");
    git_do(&other, &["add", "."]);
    git_do(&other, &["commit", "-m", "their work"]);
    git_do(&other, &["push"]);

    // `.git` in the original clone still believes it is level.
    let stale = git::inventory(std::slice::from_ref(&work), &ScanOptions::default()).await.expect("scanned");
    assert_eq!(stale.repos[0].behind, 0, "the cached view has not noticed");
    assert_eq!(stale.repos[0].state(), RepoState::Clean, "and would say everything is fine");

    // The live check does notice, which is the entire feature.
    let live = git::inventory(std::slice::from_ref(&work), &options).await.expect("scanned");
    assert_eq!(
        live.repos[0].state(),
        RepoState::PullRecommended,
        "asking the remote finds what the last fetch missed"
    );

    // And pulling resolves it.
    let pulled = git::pull(&repo).await.expect("pull ran");
    assert!(pulled.ok, "{}", pulled.detail);
    let after = git::inventory(std::slice::from_ref(&work), &options).await.expect("scanned");
    assert_eq!(after.repos[0].state(), RepoState::Clean);

    let _ = std::fs::remove_dir_all(&root);
}

/// Committing in a folder that is not a repository root would walk up to
/// whichever repository contains it — so a mistyped path could commit in a
/// parent tree the user never named.
#[tokio::test]
async fn an_action_refuses_a_folder_that_is_not_a_repository_root() {
    if !git_available() {
        return;
    }
    let root = scratch("guard");
    let repo = make_repo(&root, "project");
    let inside = repo.join("src");
    std::fs::create_dir_all(&inside).expect("create");

    let err = git::commit(&inside, "should not happen", true).await.expect_err("refused");
    assert!(err.to_string().contains("root"), "{err}");

    let plain = root.join("not-a-repo");
    std::fs::create_dir_all(&plain).expect("create");
    let err = git::commit(&plain, "should not happen", true).await.expect_err("refused");
    assert!(err.to_string().contains("not a git repository"), "{err}");

    let _ = std::fs::remove_dir_all(&root);
}
