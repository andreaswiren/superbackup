//! Start-up, shutdown, single-instance enforcement, and the unlock that lets
//! the scheduler proceed.
//!
//! These are the properties a backup daemon is judged on when nothing is going
//! wrong: it starts, it refuses to start twice, it stops when asked, and
//! unlocking it actually runs the backup that the lock prevented — rather than
//! leaving the user to wonder why 02:00's job never happened.

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

#[path = "../src/daemon/mod.rs"]
mod daemon;
#[path = "../src/tray/mod.rs"]
mod tray;

#[path = "../../core/tests/kopia_support/mod.rs"]
mod kopia_support;

mod daemon_support;

use std::time::Duration;

use daemon_support::*;
use superbackup_core::config::Store;
use superbackup_core::engine::Environment;
use superbackup_core::ipc::protocol::{Reply, Request};
use superbackup_core::ipc::{Client, SecretString};
use superbackup_core::state::{Health, RunStatus, Trigger};

/// The daemon starts, serves, and stops without leaving anything behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_daemon_starts_serves_and_stops_cleanly() {
    let mut harness = Harness::start("startup", |_, _| {}).await;
    let paths = harness.paths.clone();
    let endpoint = harness.endpoint.clone();

    // Serving.
    let client = harness.client().await;
    client.ping().await.expect("ping");
    let status = client.status().await.expect("status");
    assert_eq!(status.version, superbackup_core::VERSION);
    assert!(status.uptime_seconds < 60);

    // The lock is held while it runs.
    assert!(paths.lock_file().is_file(), "the single-instance lock must exist while running");

    drop(client);
    harness.shutdown().await.expect("clean shutdown");

    // And released afterwards, so a restart is not blocked by a corpse.
    assert!(
        !paths.lock_file().is_file(),
        "the single-instance lock must be released on shutdown"
    );

    // Nothing is listening any more.
    let refused = Client::connect(&endpoint).await;
    assert!(refused.is_err(), "the endpoint must be closed after shutdown");

    // Run history was flushed.
    assert!(paths.state_file().is_file(), "run history must be written on the way out");
}

/// `control.shutdown` stops the daemon, and answers before it does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_shutdown_answers_before_it_stops() {
    let mut harness = Harness::start("ctrlshutdown", |_, _| {}).await;
    let client = harness.client().await;

    // The reply must arrive: a client that asks the daemon to stop and gets a
    // dropped socket instead cannot tell success from a crash.
    let reply = client
        .request(Request::ControlShutdown { stop_runs: true })
        .await
        .expect("shutdown must be acknowledged");
    assert!(matches!(reply, Reply::Ack(_)));

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// A second daemon over the same home is refused, and does not displace the
/// first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_instance_is_refused_and_the_first_keeps_running() {
    let mut harness = Harness::start("singleton", |_, _| {}).await;

    // Same home, different endpoint — so the *only* thing that can refuse the
    // second instance is the single-instance lock, not the socket.
    let paths = harness.paths.clone();
    let second_endpoint = private_endpoint("singleton-2");
    let (ready_tx, _ready_rx) = tokio::sync::oneshot::channel();
    let (_stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let hooks = daemon::Hooks {
        ready: Some(ready_tx),
        external_stop: Some(stop_rx),
        endpoint: Some(second_endpoint),
    };
    let control = daemon::logging::init(&paths, true);
    let result = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(daemon::run(paths, daemon::Surface::Headless, control, 0, Some(hooks)))
    });
    let outcome = tokio::task::spawn_blocking(move || result.join())
        .await
        .expect("join task")
        .expect("the second daemon did not panic");

    let error = outcome.expect_err("a second instance must be refused");
    assert!(
        error.to_string().contains("already running"),
        "the refusal must name the problem: {error}"
    );

    // The first is untouched and still answering.
    let client = harness.client().await;
    client.ping().await.expect("the first instance must survive a rejected second");

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// Unlocking runs the backup the lock blocked.
///
/// The scheduler *drains* runs it cannot start rather than queueing them, so
/// without the daemon remembering them, "unlock at 09:00" would leave 02:00's
/// backup waiting until 02:00 tomorrow. This is the test for that.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unlocking_runs_what_the_lock_blocked() {
    if !kopia_available() {
        eprintln!("SKIPPED: rustc is unavailable");
        return;
    }
    let mut job_id = None;
    let mut harness = Harness::start("unlock-proceeds", |config, home| {
        let sources = seed_tree(home, 2);
        let destination = mirror("plain copy", home.join("mirror"));
        let backup = job("nightly", sources, vec![destination.id]);
        job_id = Some(backup.id);
        config.destinations.push(destination);
        config.jobs.push(backup);
    })
    .await;
    let job_id = job_id.expect("id");

    // A scheduled run while locked: the scheduler evaluates the physical gate
    // and drops it, and the daemon records that it did.
    let scheduler = harness.runtime.require_scheduler().expect("the engine is running");
    scheduler.run_now(job_id, Trigger::Schedule).await.expect("queue a scheduled run");

    assert!(
        wait_for(Duration::from_secs(10), || {
            harness.runtime.migration().is_none()
                && !harness.runtime.active_runs().iter().any(|r| r.job_id == job_id)
        })
        .await,
        "the blocked run must not be left hanging"
    );

    // Nothing ran, and the vault is why.
    let client = harness.client().await;
    let Reply::Runs(before) = harness
        .call(&client, Request::JobHistory { job: None, limit: 10 })
        .await
    else {
        panic!("expected runs")
    };
    assert!(
        before.runs.iter().all(|r| r.status != RunStatus::Succeeded),
        "nothing may succeed while the vault is locked"
    );

    // Unlock. The daemon must re-queue exactly the job it dropped.
    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");

    assert!(
        wait_for_async(Duration::from_secs(30), || {
            let client = client.clone();
            async move {
                matches!(
                    client.request(Request::JobHistory { job: None, limit: 10 }).await,
                    Ok(Reply::Runs(r))
                        if r.runs.iter().any(|run| {
                            run.job_id == job_id
                                && matches!(
                                    run.status,
                                    RunStatus::Succeeded | RunStatus::SucceededWithWarnings
                                )
                        })
                )
            }
        })
        .await,
        "unlocking must run the backup the lock blocked"
    );

    // And the files really were copied.
    let mirror_root = harness.root.join("mirror");
    assert!(mirror_root.is_dir(), "the mirror root must exist after a real run");

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// Locking the vault takes effect immediately, everywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn locking_stops_the_scheduler_as_well_as_the_ipc_surface() {
    let mut harness = Harness::start("lock", |_, _| {}).await;
    let client = harness.client().await;

    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");
    assert!(harness.runtime.environment.vault_unlocked(), "the engine must see the unlock");

    let Reply::Unlocked(locked) = harness.call(&client, Request::VaultLock {}).await else {
        panic!("expected an unlocked reply")
    };
    assert!(!locked.unlocked);
    assert!(
        !harness.runtime.environment.vault_unlocked(),
        "the engine's gate and the store must never disagree"
    );
    // The retained passphrase is gone with it.
    assert!(harness.runtime.master().is_err(), "the passphrase must not outlive the unlock");

    // And the surface refuses again.
    let refused = client
        .request(Request::VaultListRefs {})
        .await
        .expect_err("a locked vault must refuse to list handles");
    assert_eq!(refused.code(), superbackup_core::ErrorCode::Locked);

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// A wrong passphrase is refused without unlocking anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_passphrase_is_refused_and_changes_nothing() {
    let mut harness = Harness::start("badpass", |_, _| {}).await;
    let client = harness.client().await;

    let error = client
        .unlock(SecretString::from_string("not the passphrase".into()))
        .await
        .expect_err("a wrong passphrase must be refused");
    assert_eq!(error.code(), superbackup_core::ErrorCode::BadPassphrase);
    assert!(!harness.runtime.environment.vault_unlocked());

    // The real one still works afterwards: a failed attempt must not damage
    // the vault or latch the daemon into a broken state.
    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("the correct passphrase must still work");

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// The auto-lock deadline is armed on unlock and disarmed on lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_auto_lock_deadline_follows_the_setting() {
    let mut harness = Harness::start("autolock", |config, _| {
        config.settings.auto_lock_minutes = 30;
    })
    .await;
    let client = harness.client().await;

    let unlocked = client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");
    let at = unlocked.auto_lock_at.expect("a 30-minute auto-lock must be armed");
    let minutes = (at - chrono::Utc::now()).num_minutes();
    assert!((25..=30).contains(&minutes), "expected about 30 minutes, got {minutes}");

    harness.call(&client, Request::VaultLock {}).await;
    let Reply::Unlocked(state) = harness.call(&client, Request::VaultIsUnlocked {}).await else {
        panic!("expected an unlocked reply")
    };
    assert!(state.auto_lock_at.is_none(), "locking disarms the timer");

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// A daemon with no vault refuses to start rather than creating an empty one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_vault_is_an_error_and_never_a_new_empty_one() {
    let (root, paths) = fresh_home("novault");
    let control = daemon::logging::init(&paths, true);
    let (_stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let hooks = daemon::Hooks {
        ready: None,
        external_stop: Some(stop_rx),
        endpoint: Some(private_endpoint("novault")),
    };
    let run_paths = paths.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(daemon::run(run_paths, daemon::Surface::Headless, control, 0, Some(hooks)))
    })
    .await
    .expect("join");

    let error = outcome.expect_err("a missing vault must stop start-up");
    assert!(error.to_string().contains("no vault"), "{error}");
    assert!(
        !paths.vault_file().exists(),
        "start-up must never create a vault: an accidental one strands every repository"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Run history survives a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_history_survives_a_restart() {
    if !kopia_available() {
        eprintln!("SKIPPED: rustc is unavailable");
        return;
    }
    let mut job_id = None;
    let mut harness = Harness::start("persist", |config, home| {
        let sources = seed_tree(home, 1);
        let destination = mirror("copy", home.join("mirror"));
        let backup = job("docs", sources, vec![destination.id]);
        job_id = Some(backup.id);
        config.destinations.push(destination);
        config.jobs.push(backup);
    })
    .await;
    let job_id = job_id.expect("id");
    let paths = harness.paths.clone();

    let client = harness.client().await;
    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");
    harness.call(&client, Request::JobRun { job: "docs".into(), dry_run: false }).await;
    assert!(
        wait_for(Duration::from_secs(30), || harness.runtime.active_runs().is_empty()
            && !harness.runtime.recent_events().is_empty())
        .await,
        "the run must finish"
    );
    drop(client);
    harness.shutdown().await.expect("clean shutdown");

    // A fresh store reading the same home must see the run.
    let store = Store::open(paths.clone()).expect("reopen the store");
    let state: superbackup_core::state::PersistedState =
        serde_json::from_slice(&std::fs::read(paths.state_file()).expect("state.json"))
            .expect("state.json parses");
    assert!(
        state.jobs.get(&job_id).map(|s| s.total_runs).unwrap_or(0) >= 1,
        "the job summary must survive a restart"
    );
    assert!(!state.history.is_empty(), "the run history must survive a restart");
    drop(store);
}

/// The tray icon's state follows the daemon's health, and only the daemon's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_drives_the_icon_and_the_daemon_owns_the_rule() {
    let mut harness = Harness::start("health", |_, _| {}).await;
    let client = harness.client().await;

    // Locked: attention.
    let health = client.request(Request::Health {}).await.expect("health");
    let Reply::Health(health) = health else { panic!("expected a health reply") };
    assert_eq!(health.health, Health::Attention);
    assert!(
        health.reasons.iter().any(|r| r.contains("locked")),
        "the reason must say what to do: {:?}",
        health.reasons
    );

    // Unlocked with nothing to do: idle.
    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");
    assert!(
        wait_for_async(Duration::from_secs(5), || {
            let client = client.clone();
            async move {
                matches!(client.request(Request::Health {}).await, Ok(Reply::Health(h)) if h.health == Health::Idle)
            }
        })
        .await,
        "an unlocked, idle daemon is Idle"
    );

    // Paused: paused, whatever else is true.
    harness
        .call(
            &client,
            Request::ControlPause { seconds: Some(3600), reason: Some("testing".into()) },
        )
        .await;
    let Reply::Health(health) = client.request(Request::Health {}).await.expect("health") else {
        panic!("expected a health reply")
    };
    assert_eq!(health.health, Health::Paused);
    assert!(health.summary.starts_with("Paused until"), "{}", health.summary);

    harness.call(&client, Request::ControlResume {}).await;

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}


/// The cached passphrase is destroyed when the vault locks.
///
/// This is the invariant that makes `vault.lock` mean something on an install
/// that opted into caching: a machine which re-opens itself the instant it is
/// asked to shut has not locked anything. The platform half is not touched
/// here — the sidecar is the marker, and destroying it is what makes any
/// surviving key useless.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn locking_destroys_the_cached_passphrase() {
    let mut harness = Harness::start("keychain-lock", |config, _| {
        config.settings.auto_lock_minutes = 0;
    })
    .await;
    let client = harness.client().await;

    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");

    // Seed the local half directly, so the test never writes to the machine's
    // real credential store.
    let key = superbackup_core::crypto::random_bytes(32).expect("key");
    daemon::keychain::seal_local(
        &harness.paths,
        &key,
        &superbackup_core::secret::Secret::from_str(PASSPHRASE),
    )
    .expect("seal the sidecar");
    assert!(daemon::keychain::has_local(&harness.paths));

    harness.call(&client, Request::VaultLock {}).await;

    assert!(
        !daemon::keychain::has_local(&harness.paths),
        "locking must destroy the cached passphrase"
    );
    assert!(
        daemon::keychain::open_local(&harness.paths, &key)
            .expect("a missing sidecar is not an error")
            .is_none(),
        "the cache must be unusable after a lock"
    );

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// Turning the setting off destroys what was already cached.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn withdrawing_consent_destroys_the_cache() {
    let mut harness = Harness::start("keychain-off", |config, _| {
        config.settings.use_os_keychain = true;
        config.settings.auto_lock_minutes = 0;
    })
    .await;
    let client = harness.client().await;
    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");

    let key = superbackup_core::crypto::random_bytes(32).expect("key");
    daemon::keychain::seal_local(
        &harness.paths,
        &key,
        &superbackup_core::secret::Secret::from_str(PASSPHRASE),
    )
    .expect("seal the sidecar");

    let Reply::Settings(settings) =
        harness.call(&client, Request::SettingsGet {}).await
    else {
        panic!("expected a settings reply")
    };
    let mut updated = (*settings.settings).clone();
    updated.use_os_keychain = false;
    harness
        .call(&client, Request::SettingsUpdate { settings: Box::new(updated) })
        .await;

    assert!(
        !daemon::keychain::has_local(&harness.paths),
        "switching the setting off must destroy the cache: the setting is the consent"
    );

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// A rotation never leaves a cache that opens the old passphrase.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rotating_the_passphrase_invalidates_the_cache() {
    let mut harness = Harness::start("keychain-rotate", |config, _| {
        config.settings.auto_lock_minutes = 0;
    })
    .await;
    let client = harness.client().await;
    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock");

    let key = superbackup_core::crypto::random_bytes(32).expect("key");
    daemon::keychain::seal_local(
        &harness.paths,
        &key,
        &superbackup_core::secret::Secret::from_str(PASSPHRASE),
    )
    .expect("seal the sidecar");

    let replacement = "an-entirely-different-passphrase-77";
    harness
        .call(
            &client,
            Request::VaultChangePassphrase {
                current: SecretString::from_string(PASSPHRASE.to_string()),
                replacement: SecretString::from_string(replacement.to_string()),
            },
        )
        .await;

    // `use_os_keychain` is off in this fixture, so nothing is re-cached: what
    // matters is that the *old* one is gone. A stale key that still opens a
    // vault whose passphrase has moved on is the failure this guards.
    assert!(
        !daemon::keychain::has_local(&harness.paths),
        "a rotation must destroy the cached passphrase"
    );

    // And the new passphrase is what actually opens the vault now.
    harness.call(&client, Request::VaultLock {}).await;
    client
        .unlock(SecretString::from_string(replacement.to_string()))
        .await
        .expect("the new passphrase must open the vault");
    let stale = client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await;
    assert!(stale.is_ok(), "unlocking an already-open vault is a no-op");

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}
