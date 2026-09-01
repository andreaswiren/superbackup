//! One job, several destinations of different kinds, and cancellation that
//! leaves no kopia behind.
//!
//! Two properties are checked here, and both are the kind that only show up in
//! integration:
//!
//! 1. **A job may mix a kopia repository and a folder mirror**, and one of them
//!    failing must not take the other with it. The executor dispatches per
//!    destination; the runner isolates them. If either half of that were
//!    wrong, a user with a local repo and a plain copy would silently lose one
//!    of them.
//! 2. **Cancelling kills the kopia child before the run unwinds.** Returning
//!    early while kopia is still writing leaves a stale repository lock that
//!    nobody can see and that blocks every future run. The fake kopia's `hang`
//!    mode makes this observable: it appends to `heartbeat.txt` every 50 ms for
//!    as long as it is alive, so a file that stops growing is a process that
//!    actually died.

#![allow(dead_code)]

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

// `daemon::handler` reports which build it is, so the module that
// answers that has to exist in this synthetic crate too.
#[path = "../src/build.rs"]
mod build;
#[path = "../src/daemon/mod.rs"]
mod daemon;
#[path = "../src/tray/mod.rs"]
mod tray;

#[path = "../../core/tests/kopia_support/mod.rs"]
mod kopia_support;

mod daemon_support;

use std::time::Duration;

use daemon_support::*;
use superbackup_core::ipc::protocol::{Reply, Request};
use superbackup_core::ipc::SecretString;
use superbackup_core::state::RunStatus;

/// A job fanning out to a repository and a mirror, with the repository failing.
///
/// The mirror must still be written, the run must be recorded as failed
/// overall, and the failure must name the destination that broke rather than
/// the job.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_fans_out_to_a_repository_and_a_mirror_and_survives_one_failing() {
    if !kopia_available() {
        eprintln!("SKIPPED: rustc is unavailable");
        return;
    }

    let mut ids = None;
    let mut harness = Harness::start("fanout", |config, home| {
        let sources = seed_tree(home, 4);
        let repo = repository("offsite", home.join("repo"));
        let copy = mirror("plain copy", home.join("mirror"));
        let backup = job("everything", sources, vec![repo.id, copy.id]);
        ids = Some((repo.id, copy.id, backup.id));
        config.destinations.push(repo);
        config.destinations.push(copy);
        config.jobs.push(backup);
    })
    .await;
    let (repo_id, mirror_id, job_id) = ids.expect("ids");

    // kopia refuses everything with a permanent, non-retryable failure, so the
    // test does not spend the retry policy's backoff waiting.
    harness.script(&[
        ("mode", "fail"),
        ("exit", "1"),
        ("stderr", "ERROR invalid repository password"),
    ]);

    let client = harness.client().await;
    client.unlock(SecretString::from_string(PASSPHRASE.to_string())).await.expect("unlock");

    harness.call(&client, Request::JobRun { job: "everything".into(), dry_run: false }).await;

    assert!(
        wait_for_async(Duration::from_secs(60), || {
            let client = client.clone();
            async move {
                matches!(
                    client.request(Request::JobHistory { job: None, limit: 5 }).await,
                    Ok(Reply::Runs(r)) if r.runs.iter().any(|run| run.status.is_terminal())
                )
            }
        })
        .await,
        "the run must finish"
    );

    let Reply::Runs(history) =
        harness.call(&client, Request::JobHistory { job: None, limit: 5 }).await
    else {
        panic!("expected runs")
    };
    let run = history.runs.first().expect("one run");
    assert_eq!(run.job_id, job_id);
    assert_eq!(run.destinations.len(), 2, "both destinations must be attempted");

    let repo_run = run
        .destinations
        .iter()
        .find(|d| d.destination_id == repo_id)
        .expect("the repository destination");
    let mirror_run = run
        .destinations
        .iter()
        .find(|d| d.destination_id == mirror_id)
        .expect("the mirror destination");

    assert_eq!(repo_run.status, RunStatus::Failed, "the repository must fail");
    assert!(
        repo_run.error.as_ref().is_some_and(|e| !e.message.is_empty()),
        "a failed destination must carry a reason"
    );
    // The one destination that could work, did. This is the property that
    // makes several destinations worth having.
    assert_eq!(
        mirror_run.status,
        RunStatus::Succeeded,
        "a kopia failure must not stop a plain folder copy: {:#?}",
        mirror_run
    );

    // The files really are on disk.
    let mirror_root = harness.root.join("mirror");
    let copied = walk_count(&mirror_root);
    assert!(copied >= 4, "the mirror must hold the source files, found {copied}");

    // And the run as a whole is a failure, because one destination did not get
    // the backup — reporting this as a success is the failure mode that
    // destroys trust.
    assert_eq!(run.status, RunStatus::Failed);

    // Nothing in the recorded error may contain the passphrase.
    let rendered = format!("{:#?}", run.destinations);
    assert!(!rendered.contains(PASSPHRASE), "a secret reached the run history");

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// Stopping a run kills the kopia child before the run is recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_a_run_leaves_no_kopia_behind() {
    if !kopia_available() {
        eprintln!("SKIPPED: rustc is unavailable");
        return;
    }

    let mut destination_id = None;
    let mut harness = Harness::start("cancel", |config, home| {
        let sources = seed_tree(home, 2);
        let destination = repository("slow disk", home.join("repo"));
        destination_id = Some(destination.id);
        let backup = job("big", sources, vec![destination.id]);
        config.destinations.push(destination);
        config.jobs.push(backup);
    })
    .await;
    let destination_id = destination_id.expect("id");

    let client = harness.client().await;
    client.unlock(SecretString::from_string(PASSPHRASE.to_string())).await.expect("unlock");
    // The repository has to exist before a run can reach kopia at all: that
    // is what puts its passphrase in the vault.
    harness
        .call(
            &client,
            Request::DestinationRepoCreate {
                destination: destination_id.to_string(),
                encryption: None,
            },
        )
        .await;

    // From here on, every kopia invocation hangs until it is killed.
    let heartbeat = harness.kopia_dir.join("heartbeat.txt");
    let _ = std::fs::remove_file(&heartbeat);
    harness.script(&[("mode", "hang")]);

    let Reply::Started(started) =
        harness.call(&client, Request::JobRun { job: "big".into(), dry_run: false }).await
    else {
        panic!("expected a started reply")
    };

    // Wait until kopia is genuinely running: the heartbeat file proves it.
    assert!(
        wait_for(Duration::from_secs(30), || heartbeat
            .metadata()
            .map(|m| m.len() > 0)
            .unwrap_or(false))
        .await,
        "the fake kopia must actually be running before it can be cancelled"
    );

    let stopped_at = std::time::Instant::now();
    let Reply::Stopped(stopped) =
        harness.call(&client, Request::JobStop { run_id: started.run_id }).await
    else {
        panic!("expected a stopped reply")
    };
    assert_eq!(stopped.stopped, vec![started.run_id]);

    // The run must reach a terminal state promptly. The executor's contract is
    // "about one second"; the runner adds its own grace, so fifteen seconds is
    // a generous ceiling that still fails loudly on a hang.
    assert!(
        wait_for(Duration::from_secs(15), || harness.runtime.active_runs().is_empty()).await,
        "a stopped run must unwind promptly, not when kopia feels like it"
    );
    let elapsed = stopped_at.elapsed();
    assert!(elapsed < Duration::from_secs(15), "cancellation took {elapsed:?}");

    // The child is dead: its heartbeat stopped growing. This is the assertion
    // that matters — a kopia still running would hold the repository lock and
    // every future run would block on it.
    let size_now = heartbeat.metadata().map(|m| m.len()).unwrap_or(0);
    tokio::time::sleep(Duration::from_millis(600)).await;
    let size_later = heartbeat.metadata().map(|m| m.len()).unwrap_or(0);
    assert_eq!(
        size_now, size_later,
        "the kopia child was still alive {}s after the run was cancelled",
        0.6
    );

    // And it is recorded as cancelled, not as a failure — the user asked.
    let Reply::Runs(history) =
        harness.call(&client, Request::JobHistory { job: None, limit: 5 }).await
    else {
        panic!("expected runs")
    };
    let run = history.runs.first().expect("the cancelled run was recorded");
    assert_eq!(run.status, RunStatus::Cancelled, "{:#?}", run.destinations);

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// Shutdown with a run in flight cancels it rather than abandoning kopia.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutting_down_mid_backup_stops_kopia_first() {
    if !kopia_available() {
        eprintln!("SKIPPED: rustc is unavailable");
        return;
    }

    let mut destination_id = None;
    let mut harness = Harness::start("shutdown-mid", |config, home| {
        let sources = seed_tree(home, 2);
        let destination = repository("disk", home.join("repo"));
        destination_id = Some(destination.id);
        let backup = job("long", sources, vec![destination.id]);
        config.destinations.push(destination);
        config.jobs.push(backup);
    })
    .await;
    let destination_id = destination_id.expect("id");

    let client = harness.client().await;
    client.unlock(SecretString::from_string(PASSPHRASE.to_string())).await.expect("unlock");
    harness
        .call(
            &client,
            Request::DestinationRepoCreate {
                destination: destination_id.to_string(),
                encryption: None,
            },
        )
        .await;

    let heartbeat = harness.kopia_dir.join("heartbeat.txt");
    let _ = std::fs::remove_file(&heartbeat);
    harness.script(&[("mode", "hang")]);
    harness.call(&client, Request::JobRun { job: "long".into(), dry_run: false }).await;
    assert!(
        wait_for(Duration::from_secs(30), || heartbeat
            .metadata()
            .map(|m| m.len() > 0)
            .unwrap_or(false))
        .await,
        "kopia must be running before shutdown is tested"
    );

    drop(client);
    let started = std::time::Instant::now();
    harness.shutdown().await.expect("shutdown must complete even mid-backup");
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "shutdown took {:?}; the SCM would have killed the service",
        started.elapsed()
    );

    // The child died with the daemon, rather than being orphaned onto a
    // repository nobody is watching any more.
    let size_now = heartbeat.metadata().map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(600));
    let size_later = heartbeat.metadata().map(|m| m.len()).unwrap_or(0);
    assert_eq!(size_now, size_later, "shutdown orphaned a kopia process");
}

/// Stopping something that is not running is a success, not an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_nothing_is_idempotent() {
    let mut harness = Harness::start("stop-nothing", |_, _| {}).await;
    let client = harness.client().await;

    let Reply::Stopped(stopped) =
        harness.call(&client, Request::JobStop { run_id: uuid::Uuid::new_v4() }).await
    else {
        panic!("expected a stopped reply")
    };
    assert!(stopped.stopped.is_empty());

    let Reply::Stopped(all) = harness.call(&client, Request::JobStopAll {}).await else {
        panic!("expected a stopped reply")
    };
    assert!(all.stopped.is_empty());

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// Count the files under a directory tree.
fn walk_count(root: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else { return 0 };
    let mut count = 0;
    for entry in entries.flatten() {
        match entry.file_type() {
            Ok(t) if t.is_dir() => count += walk_count(&entry.path()),
            Ok(_) => count += 1,
            Err(_) => {}
        }
    }
    count
}
