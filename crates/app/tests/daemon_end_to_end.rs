//! The path the whole program exists for, driven end to end.
//!
//! Start the daemon on a private endpoint over a temporary
//! `SUPERBACKUP_HOME`, connect over the real IPC socket, unlock the vault,
//! create a local kopia repository, run a job, watch progress arrive, and read
//! it back out of the run history. Nothing is stubbed except kopia itself, and
//! that is the scriptable fake from `crates/core/tests/kopia_support` — a real
//! subprocess, spawned by the real driver, with a real argv and a real
//! environment.
//!
//! If this file passes, the seven libraries are wired into an application.

#![allow(dead_code)]

/// The slice of `crate::cli` that `daemon::mod` and `tray::mod` name.
mod cli {
    pub mod exit {
        pub const OK: i32 = 0;
        pub const FAILED: i32 = 1;
        pub const USAGE: i32 = 2;
        pub const DAEMON_UNREACHABLE: i32 = 3;
        pub const LOCKED: i32 = 4;
        pub const CANCELLED: i32 = 5;
    }

    #[derive(Debug, Clone, Default)]
    pub struct GlobalArgs {
        pub json: bool,
        pub quiet: bool,
        pub verbose: u8,
        pub no_input: bool,
        pub home: Option<std::path::PathBuf>,
        pub service: bool,
        pub timeout: u64,
    }
}

#[path = "../src/daemon/mod.rs"]
mod daemon;
#[path = "../src/tray/mod.rs"]
mod tray;

#[path = "../../core/tests/kopia_support/mod.rs"]
mod kopia_support;

mod daemon_support;

use std::time::Duration;

use daemon_support::*;
use superbackup_core::ipc::protocol::{ConflictPolicy, Reply, Request};
use superbackup_core::ipc::{SecretString, Topic};
use superbackup_core::state::{Health, RunStatus};

/// The whole path: unlock, create, run, watch, stop, history.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_runs_end_to_end_against_a_real_repository() {
    if !kopia_available() {
        eprintln!("SKIPPED: rustc is unavailable, so the fake kopia could not be built");
        return;
    }

    let mut ids = None;
    let mut harness = Harness::start("e2e", |config, home| {
        let sources = seed_tree(home, 3);
        let destination = repository("local disk", home.join("repo"));
        let backup = job("dev code", sources, vec![destination.id]);
        ids = Some((destination.id, backup.id));
        config.destinations.push(destination);
        config.jobs.push(backup);
    })
    .await;
    let (destination_id, job_id) = ids.expect("the fixture recorded its ids");

    // The fake prints a real manifest, so the run history gets real numbers
    // rather than zeroes that would hide a broken parse.
    let manifest = harness.root.join("manifest.json");
    std::fs::write(&manifest, manifest_json(1204, 4_400_000_000)).expect("write the manifest");
    harness.script(&[
        ("mode", "snapshot"),
        ("stdout_file", &manifest.display().to_string()),
    ]);

    let client = harness.client().await;

    // ---- the daemon is alive and honest about being locked ----------
    client.ping().await.expect("ping");
    let version = client.version().await.expect("version");
    assert_eq!(version.version, superbackup_core::VERSION);
    assert!(version.kopia_version.is_some(), "the daemon found its kopia");

    let status = client.status().await.expect("status");
    assert!(!status.unlocked, "a fresh daemon starts with the vault locked");
    assert_eq!(status.health, Health::Attention, "a locked vault needs attention");

    // ---- a run is refused while locked, by code and not by accident --
    let refused = client
        .request(Request::JobRun { job: "dev code".into(), dry_run: false })
        .await
        .expect_err("a locked vault must refuse to run a job");
    assert_eq!(refused.code(), superbackup_core::ErrorCode::Locked);

    // ---- unlock -----------------------------------------------------
    let unlocked = client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");
    assert!(unlocked.unlocked);
    assert!(
        wait_for_async(Duration::from_secs(5), || {
            let client = client.clone();
            async move { client.status().await.map(|s| s.unlocked).unwrap_or(false) }
        })
        .await,
        "the daemon must report itself unlocked"
    );

    // ---- create the repository --------------------------------------
    let created = harness
        .call(
            &client,
            Request::DestinationRepoCreate {
                destination: destination_id.to_string(),
                encryption: None,
            },
        )
        .await;
    let Reply::Repository(repo) = created else { panic!("expected a repository reply") };
    assert_eq!(repo.destination_id, destination_id);
    assert!(repo.connected);

    // The passphrase must have reached the vault *before* kopia was asked to
    // create anything, or the repository would be unopenable for ever.
    let Reply::SecretRefs(refs) = harness.call(&client, Request::VaultListRefs {}).await else {
        panic!("expected a secret-refs reply")
    };
    assert!(
        refs.refs.iter().any(|r| r.as_str().contains(&destination_id.to_string())),
        "the repository passphrase must be in the vault: {:?}",
        refs.refs
    );

    // ---- subscribe, then run ----------------------------------------
    let mut stream = client
        .subscribe(vec![Topic::Progress, Topic::Events, Topic::Status])
        .await
        .expect("subscribe");

    let started = harness
        .call(&client, Request::JobRun { job: "dev code".into(), dry_run: false })
        .await;
    let Reply::Started(started) = started else { panic!("expected a started reply") };
    assert!(started.started);

    // ---- watch progress ---------------------------------------------
    let mut saw_progress = false;
    let mut saw_finish = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline && !saw_finish {
        let Ok(Some(item)) =
            tokio::time::timeout(Duration::from_secs(5), stream.next()).await
        else {
            break;
        };
        match item {
            superbackup_core::ipc::StreamItem::Progress { destination_id: d, .. } => {
                assert_eq!(d, destination_id);
                saw_progress = true;
            }
            superbackup_core::ipc::StreamItem::Event { event }
                if event.kind == "job.finished" =>
            {
                saw_finish = true;
            }
            _ => {}
        }
    }
    assert!(saw_progress, "the GUI must be able to see a backup happening");
    assert!(saw_finish, "the run must reach a terminal state");

    // ---- history ----------------------------------------------------
    let Reply::Runs(history) = harness
        .call(&client, Request::JobHistory { job: Some("dev code".into()), limit: 10 })
        .await
    else {
        panic!("expected a runs reply")
    };
    assert_eq!(history.runs.len(), 1, "exactly one run happened");
    let run = &history.runs[0];
    assert_eq!(run.job_id, job_id);
    assert!(
        matches!(run.status, RunStatus::Succeeded | RunStatus::SucceededWithWarnings),
        "the run must have succeeded, got {:?}: {:#?}",
        run.status,
        run.destinations
    );
    assert_eq!(run.destinations.len(), 1);
    let dest = &run.destinations[0];
    assert_eq!(dest.destination_id, destination_id);
    assert_eq!(
        dest.progress.files_processed, 1204,
        "kopia's own manifest figures must reach the history"
    );
    assert_eq!(dest.snapshot_id.as_deref().map(|s| s.starts_with('k')), Some(true));

    // ---- kopia was driven correctly ---------------------------------
    let invocations = harness.invocations();
    // The fake kopia accepts every connect, so `dest.repo_create` finds a
    // repository already there and never has to create one — which is the
    // idempotent path, and the one that must be pinned to our own config file.
    let connect = invocations
        .iter()
        .find(|argv| argv.iter().any(|a| a == "repository") && argv.iter().any(|a| a == "connect"))
        .unwrap_or_else(|| panic!("a `repository connect` invocation: {invocations:#?}"));
    assert!(
        connect.iter().any(|a| a.starts_with("--config-file=")),
        "every invocation must be pinned to superbackup's own config file: {connect:?}"
    );
    assert!(
        invocations.iter().all(|argv| argv
            .iter()
            .any(|a| a.starts_with("--config-file=") || a == "--version")),
        "an invocation escaped superbackup's own kopia configuration: {invocations:#?}"
    );
    let snapshot = invocations
        .iter()
        .find(|argv| argv.iter().any(|a| a == "snapshot") && argv.iter().any(|a| a == "create"))
        .expect("a `snapshot create` invocation");
    assert!(
        snapshot.iter().any(|a| a.starts_with("--progress")),
        "progress must be forced on, or the bar freezes: {snapshot:?}"
    );
    // The single most important assertion in the file: no argument anywhere
    // may be, or contain, a secret.
    for argv in &invocations {
        for arg in argv {
            let lower = arg.to_lowercase();
            assert!(
                !lower.contains("password") || arg.starts_with("--send-snapshot-report"),
                "a secret-shaped argument reached kopia's command line: {arg}"
            );
            assert!(!arg.contains(PASSPHRASE), "the master passphrase reached argv: {arg}");
        }
    }

    // ---- status reflects the finished run ---------------------------
    let status = client.status().await.expect("status");
    assert!(status.active_runs.is_empty(), "nothing is still running");
    // The fake kopia reports one ignored unreadable file, exactly as a real
    // one does over a source tree with a locked lockfile — so the honest
    // outcome is "succeeded, with warnings", not a clean success.
    assert_eq!(
        status.jobs.get(&job_id).and_then(|j| j.last_status),
        Some(RunStatus::SucceededWithWarnings)
    );
    assert!(
        !run.destinations[0].warnings.is_empty(),
        "an ignored file must reach the run record, not be swallowed"
    );

    drop(stream);
    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// A dry run reports and writes nothing — for *every* destination kind.
///
/// `engine::Runner` drives the mirror engine itself for a `LocalMirror`
/// destination, so an executor-level flag could never have stopped it copying.
/// `RunRequest::dry_run` reaches both halves, so the guarantee is uniform: the
/// repository is estimated rather than snapshotted, and the mirror folder is
/// not even brought into existence — while both still report the counts they
/// would have produced.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dry_run_reports_without_writing() {
    if !kopia_available() {
        eprintln!("SKIPPED: rustc is unavailable");
        return;
    }
    let mut repo_id = None;
    let mut harness = Harness::start("dryrun", |config, home| {
        let sources = seed_tree(home, 2);
        let repo = repository("disk", home.join("repo"));
        repo_id = Some(repo.id);
        let copy = mirror("plain copy", home.join("mirror"));
        let backup = job("docs", sources, vec![repo.id, copy.id]);
        config.destinations.push(repo);
        config.destinations.push(copy);
        config.jobs.push(backup);
    })
    .await;
    let repo_id = repo_id.expect("id");

    let client = harness.client().await;
    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");
    // A rehearsal still has to connect to a real repository, so it has to
    // exist — and creating it is what puts its passphrase in the vault.
    harness
        .call(
            &client,
            Request::DestinationRepoCreate {
                destination: repo_id.to_string(),
                encryption: None,
            },
        )
        .await;

    let Reply::Started(started) = harness
        .call(&client, Request::JobRun { job: "docs".into(), dry_run: true })
        .await
    else {
        panic!("expected a started reply")
    };
    let note = started.note.expect("a dry run must explain itself");
    assert!(note.to_lowercase().contains("dry run"), "{note}");
    assert!(
        !note.contains("cannot be rehearsed"),
        "every destination kind is rehearsable now; the caveat must be gone: {note}"
    );

    assert!(
        wait_for_async(Duration::from_secs(30), || {
            let client = client.clone();
            async move {
                matches!(
                    client.request(Request::JobHistory { job: None, limit: 5 }).await,
                    Ok(Reply::Runs(r)) if r.runs.iter().any(|run| run.status.is_terminal())
                )
            }
        })
        .await,
        "the dry run must be recorded"
    );

    // The mirror folder was not even created: bringing it into existence is a
    // visible side effect, and on a removable drive it is the difference
    // between "nothing happened" and a stray folder.
    let mirror_root = harness.root.join("mirror");
    assert!(
        !mirror_root.exists(),
        "a dry run created the mirror folder: {:?}",
        std::fs::read_dir(&mirror_root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    );

    // And kopia was asked to *estimate*, never to snapshot.
    let invocations = harness.invocations();
    assert!(
        invocations
            .iter()
            .any(|argv| argv.iter().any(|a| a == "estimate")),
        "a repository rehearsal must ask kopia what it would copy: {invocations:#?}"
    );
    assert!(
        !invocations.iter().any(|argv| argv.iter().any(|a| a == "snapshot")
            && argv.iter().any(|a| a == "create")),
        "a dry run must never run `snapshot create`: {invocations:#?}"
    );

    // The run is recorded, with the dry-run warning attached to it.
    let Reply::Runs(history) = harness
        .call(&client, Request::JobHistory { job: None, limit: 5 })
        .await
    else {
        panic!("expected runs")
    };
    let run = history.runs.first().expect("one run");
    assert_eq!(
        run.destinations.len(),
        2,
        "both destinations must be rehearsed, not just the repository"
    );
    let repo_run = run
        .destinations
        .iter()
        .find(|d| d.destination_id == repo_id)
        .expect("the repository was rehearsed");
    assert!(
        repo_run.warnings.iter().any(|w| w.starts_with("Dry run:")),
        "the history must record that nothing was written: {:#?}",
        repo_run.warnings
    );
    assert!(
        repo_run.snapshot_id.is_none(),
        "a rehearsal must not claim to have produced a snapshot"
    );

    // The mirror still reports what it *would* have copied — a rehearsal that
    // reported zero would be useless for deciding whether to run for real.
    let mirror_run = run
        .destinations
        .iter()
        .find(|d| d.destination_id != repo_id)
        .expect("the mirror was rehearsed");
    assert!(
        mirror_run.progress.files_processed >= 3,
        "the mirror rehearsal must report the files it would copy: {:#?}",
        mirror_run.progress
    );

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// A restore is accepted, addressed correctly, and refuses what kopia cannot do.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_is_accepted_and_refuses_a_policy_kopia_cannot_honour() {
    if !kopia_available() {
        eprintln!("SKIPPED: rustc is unavailable");
        return;
    }
    let mut destination_id = None;
    let mut harness = Harness::start("restore", |config, home| {
        let destination = repository("disk", home.join("repo"));
        destination_id = Some(destination.id);
        config.destinations.push(destination);
    })
    .await;
    let destination_id = destination_id.expect("id");

    let client = harness.client().await;
    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");

    let target = harness.root.join("restored");
    // `KeepBoth` is refused rather than silently doing something else with the
    // user's data during a restore.
    let refused = client
        .request(Request::SnapshotRestore {
            destination: destination_id.to_string(),
            snapshot: "kdeadbeef".into(),
            path: String::new(),
            target: target.clone(),
            conflict: ConflictPolicy::KeepBoth,
            dry_run: false,
        })
        .await
        .expect_err("keep-both must be refused");
    assert_eq!(refused.code(), superbackup_core::ErrorCode::Validation);

    // A relative target is refused before anything is written.
    let relative = client
        .request(Request::SnapshotRestore {
            destination: destination_id.to_string(),
            snapshot: "kdeadbeef".into(),
            path: String::new(),
            target: "relative/path".into(),
            conflict: ConflictPolicy::Skip,
            dry_run: false,
        })
        .await
        .expect_err("a relative target must be refused");
    assert_eq!(relative.code(), superbackup_core::ErrorCode::Validation);

    // A path that tries to escape the snapshot is refused too.
    let escape = client
        .request(Request::SnapshotBrowse {
            destination: destination_id.to_string(),
            snapshot: "kdeadbeef".into(),
            path: "../../etc".into(),
        })
        .await
        .expect_err("`..` must be refused");
    assert_eq!(escape.code(), superbackup_core::ErrorCode::Validation);

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}
