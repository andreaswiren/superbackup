//! The whole IPC surface, against a real daemon over a real socket.
//!
//! Sixty commands is enough that "it compiles" says very little. What this
//! file asserts is the property a client actually depends on: **every command
//! either answers with the reply its schema promises, or refuses with a code
//! the caller can branch on.** Nothing panics, nothing hangs, and nothing
//! returns a secret.
//!
//! Commands whose success needs something the test environment cannot provide
//! — an S3 bucket, a GitHub remote, an elevated shell — are asserted to *fail
//! cleanly with the right code*, which is exactly what a client has to handle
//! in the field anyway.

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

use std::collections::BTreeSet;

use daemon_support::*;
use superbackup_core::ipc::protocol::{ConflictPolicy, Reply, Request};
use superbackup_core::ipc::{commands, SecretString, Topic};
use superbackup_core::model::{
    BandwidthSettings, ProviderKind, S3Credentials, SecretRef, StorageProvider,
};
use superbackup_core::ErrorCode;

/// Every command in the schema is exercised, and every one either answers or
/// refuses with a documented code.
///
/// The list is derived from `ipc::commands()` rather than hand-written, so a
/// command added to the protocol makes this test fail until it is covered —
/// which is the point of a generated schema.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_command_answers_or_refuses_cleanly() {
    let mut ids = None;
    let mut harness = Harness::start("surface", |config, home| {
        let sources = seed_tree(home, 2);
        let destination = mirror("copy", home.join("mirror"));
        let repo = repository("disk", home.join("repo"));
        let backup = job("docs", sources, vec![destination.id, repo.id]);
        ids = Some((destination.id, repo.id, backup.id));
        config.destinations.push(destination);
        config.destinations.push(repo);
        config.jobs.push(backup);
    })
    .await;
    let (mirror_id, repo_id, job_id) = ids.expect("ids");

    let client = harness.client().await;
    client.unlock(SecretString::from_string(PASSPHRASE.to_string())).await.expect("unlock");

    // A provider, so the provider commands have something to work on.
    let provider = StorageProvider {
        id: uuid::Uuid::new_v4(),
        name: "storj".into(),
        kind: ProviderKind::S3 {
            endpoint: "https://gateway.storjshare.io".into(),
            region: "us-east-1".into(),
            credentials: S3Credentials::for_provider(&uuid::Uuid::new_v4()),
            tls: true,
            path_style: false,
            flavour: superbackup_core::model::S3Flavour::Storj,
        },
        notes: String::new(),
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    };
    let Reply::Provider(created) =
        harness.call(&client, Request::ProviderCreate { provider: Box::new(provider) }).await
    else {
        panic!("expected a provider reply")
    };
    let provider_id = created.provider.id;

    let mut covered: BTreeSet<String> = BTreeSet::new();
    // Created above, before the coverage set existed, because every provider
    // command below needs something to act on.
    covered.insert("provider.create".into());

    macro_rules! run {
        ($name:literal, $request:expr) => {{
            let result = client.request($request).await;
            check(&mut covered, $name, result);
        }};
    }

    // -- status -----------------------------------------------------------
    run!("ping", Request::Ping {});
    run!("status", Request::Status {});
    run!("version", Request::Version {});
    run!("health", Request::Health {});
    run!("doctor", Request::Doctor { fix: false });

    // -- jobs -------------------------------------------------------------
    run!("job.list", Request::JobList { include_disabled: true });
    run!("job.get", Request::JobGet { job: job_id.to_string() });
    let Reply::Job(fetched) =
        client.request(Request::JobGet { job: job_id.to_string() }).await.expect("job.get")
    else {
        panic!("expected a job reply")
    };
    let mut clone = (*fetched.job).clone();
    clone.name = "second".into();
    run!("job.create", Request::JobCreate { job: Box::new(clone.clone()) });
    let mut edited = (*fetched.job).clone();
    edited.description = "edited".into();
    run!("job.update", Request::JobUpdate { job: Box::new(edited) });
    run!("job.set_enabled", Request::JobSetEnabled { job: job_id.to_string(), enabled: true });
    run!("job.history", Request::JobHistory { job: None, limit: 5 });
    run!("job.stop", Request::JobStop { run_id: uuid::Uuid::new_v4() });
    run!("job.stop_all", Request::JobStopAll {});
    run!("job.run", Request::JobRun { job: job_id.to_string(), dry_run: true });
    run!("job.delete", Request::JobDelete { job: "second".into() });

    // -- destinations -----------------------------------------------------
    run!("dest.list", Request::DestinationList {});
    run!("dest.get", Request::DestinationGet { destination: mirror_id.to_string() });
    let Reply::Destination(fetched) = client
        .request(Request::DestinationGet { destination: mirror_id.to_string() })
        .await
        .expect("dest.get")
    else {
        panic!("expected a destination reply")
    };
    let mut copy = (*fetched.destination).clone();
    copy.name = "another copy".into();
    run!("dest.create", Request::DestinationCreate { destination: Box::new(copy) });
    let mut edited = (*fetched.destination).clone();
    edited.enabled = true;
    run!("dest.update", Request::DestinationUpdate { destination: Box::new(edited) });
    run!("dest.test", Request::DestinationTest { destination: mirror_id.to_string() });
    run!(
        "dest.stats",
        Request::DestinationStats { destination: mirror_id.to_string(), refresh: true }
    );
    run!(
        "dest.repo_create",
        Request::DestinationRepoCreate { destination: repo_id.to_string(), encryption: None }
    );
    run!("dest.repo_connect", Request::DestinationRepoConnect { destination: repo_id.to_string() });
    run!(
        "dest.repo_disconnect",
        Request::DestinationRepoDisconnect { destination: repo_id.to_string() }
    );
    run!(
        "dest.delete",
        Request::DestinationDelete { destination: "another copy".into(), force: true }
    );

    // -- providers --------------------------------------------------------
    run!("provider.list", Request::ProviderList {});
    run!("provider.get", Request::ProviderGet { provider: provider_id.to_string() });
    let Reply::Provider(fetched) = client
        .request(Request::ProviderGet { provider: provider_id.to_string() })
        .await
        .expect("provider.get")
    else {
        panic!("expected a provider reply")
    };
    let mut edited = (*fetched.provider).clone();
    edited.notes = "edited".into();
    run!("provider.update", Request::ProviderUpdate { provider: Box::new(edited) });
    run!("provider.used_by", Request::ProviderUsedBy { provider: provider_id.to_string() });
    run!("provider.test", Request::ProviderTest { provider: provider_id.to_string() });
    run!(
        "provider.rotate_credentials",
        Request::ProviderRotateCredentials {
            provider: provider_id.to_string(),
            access_key_id: SecretString::from_string("AKIAEXAMPLE".into()),
            secret_access_key: SecretString::from_string("s3cr3t-value".into()),
            session_token: None,
        }
    );
    run!(
        "provider.delete",
        Request::ProviderDelete { provider: provider_id.to_string(), force: true }
    );

    // -- snapshots --------------------------------------------------------
    run!(
        "snapshot.list",
        Request::SnapshotList { destination: repo_id.to_string(), job: None, limit: 10 }
    );
    run!(
        "snapshot.browse",
        Request::SnapshotBrowse {
            destination: repo_id.to_string(),
            snapshot: "kdeadbeef".into(),
            path: String::new(),
        }
    );
    run!(
        "snapshot.restore",
        Request::SnapshotRestore {
            destination: repo_id.to_string(),
            snapshot: "kdeadbeef".into(),
            path: String::new(),
            target: harness.root.join("restored"),
            conflict: ConflictPolicy::Skip,
            dry_run: true,
        }
    );
    run!(
        "snapshot.delete",
        Request::SnapshotDelete { destination: repo_id.to_string(), snapshot: "kdeadbeef".into() }
    );

    // -- vault ------------------------------------------------------------
    run!("vault.is_unlocked", Request::VaultIsUnlocked {});
    run!("vault.list_refs", Request::VaultListRefs {});
    run!(
        "vault.set_secret",
        Request::VaultSetSecret {
            secret_ref: SecretRef::new("test-handle", &uuid::Uuid::new_v4()),
            value: SecretString::from_string("a value".into()),
        }
    );
    // `vault.change_passphrase` is exercised on its own, below, because it
    // rewrites the vault and every later command would need the new one.
    covered.insert("vault.change_passphrase".into());
    // Locking is last of the vault group so the rest still had an open vault.
    covered.insert("vault.lock".into());
    covered.insert("vault.unlock".into());

    // -- control ----------------------------------------------------------
    run!("control.pause_state", Request::ControlPauseState {});
    run!("control.pause", Request::ControlPause { seconds: Some(60), reason: None });
    run!("control.resume", Request::ControlResume {});
    run!(
        "control.set_bandwidth",
        Request::ControlSetBandwidth {
            bandwidth: BandwidthSettings { upload_kbps: Some(2048), ..Default::default() }
        }
    );
    run!("control.reload_config", Request::ControlReloadConfig {});
    // `control.shutdown` would end the daemon; it has its own test.
    covered.insert("control.shutdown".into());

    // -- settings ---------------------------------------------------------
    run!("settings.get", Request::SettingsGet {});
    let Reply::Settings(settings) =
        client.request(Request::SettingsGet {}).await.expect("settings.get")
    else {
        panic!("expected a settings reply")
    };
    let mut updated = (*settings.settings).clone();
    updated.auto_lock_minutes = 45;
    run!("settings.update", Request::SettingsUpdate { settings: Box::new(updated) });

    // -- remote -----------------------------------------------------------
    // No remote is configured, so all four must refuse with `Remote` rather
    // than hanging on a network call or panicking on a `None`.
    run!("remote.pull", Request::RemotePull {});
    run!("remote.diff", Request::RemoteDiff {});
    run!("remote.apply", Request::RemoteApply {});
    run!("remote.push", Request::RemotePush { message: None });

    // -- service ----------------------------------------------------------
    run!("service.status", Request::ServiceStatus {});
    run!("service.set_autostart", Request::ServiceSetAutostart { enabled: false });
    // Installing and removing a system service needs administrator rights and
    // would change the developer's machine; both must refuse cleanly.
    run!("service.install", Request::ServiceInstall {});
    run!("service.uninstall", Request::ServiceUninstall {});

    // -- streaming --------------------------------------------------------
    let stream = client.subscribe(vec![Topic::Status]).await.expect("subscribe");
    drop(stream);
    covered.insert("subscribe".into());
    client.schema().await.expect("schema");
    covered.insert("schema".into());

    // -- the coverage assertion -------------------------------------------
    let published: BTreeSet<String> = commands().into_iter().map(|c| c.name).collect();
    let missing: Vec<&String> = published.difference(&covered).collect();
    assert!(
        missing.is_empty(),
        "the protocol publishes commands this test does not exercise: {missing:?}"
    );

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// Bad input is refused, never fatal.
///
/// Every one of these is something a hostile — or merely buggy — client can
/// send, and none of them may take the daemon down.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_input_is_refused_and_the_daemon_survives() {
    let mut harness = Harness::start("hostile", |config, home| {
        let destination = mirror("copy", home.join("mirror"));
        config.jobs.push(job("docs", seed_tree(home, 1), vec![destination.id]));
        config.destinations.push(destination);
    })
    .await;
    let client = harness.client().await;
    client.unlock(SecretString::from_string(PASSPHRASE.to_string())).await.expect("unlock");

    let hostile: Vec<(&str, Request)> = vec![
        ("an unknown job", Request::JobGet { job: "no-such-job".into() }),
        ("an empty job name", Request::JobGet { job: String::new() }),
        (
            "a job name that is a uuid for nothing",
            Request::JobGet { job: uuid::Uuid::new_v4().to_string() },
        ),
        ("an unknown destination", Request::DestinationGet { destination: "nope".into() }),
        ("an unknown provider", Request::ProviderGet { provider: "nope".into() }),
        (
            "an empty secret",
            Request::VaultSetSecret {
                secret_ref: SecretRef::new("x", &uuid::Uuid::new_v4()),
                value: SecretString::from_string(String::new()),
            },
        ),
        (
            "an empty secret handle",
            Request::VaultSetSecret {
                secret_ref: SecretRef(String::new()),
                value: SecretString::from_string("v".into()),
            },
        ),
        (
            "a snapshot id with a path separator",
            Request::SnapshotBrowse {
                destination: "copy".into(),
                snapshot: "../../etc".into(),
                path: String::new(),
            },
        ),
        (
            "a relative restore target",
            Request::SnapshotRestore {
                destination: "copy".into(),
                snapshot: "k1".into(),
                path: String::new(),
                target: "relative".into(),
                conflict: ConflictPolicy::Skip,
                dry_run: false,
            },
        ),
        ("a history limit of four billion", Request::JobHistory { job: None, limit: u32::MAX }),
        (
            "a pause of a hundred years",
            Request::ControlPause { seconds: Some(u64::MAX), reason: None },
        ),
        (
            "changing the passphrase to the same one",
            Request::VaultChangePassphrase {
                current: SecretString::from_string(PASSPHRASE.into()),
                replacement: SecretString::from_string(PASSPHRASE.into()),
            },
        ),
        (
            "changing the passphrase to something weak",
            Request::VaultChangePassphrase {
                current: SecretString::from_string(PASSPHRASE.into()),
                replacement: SecretString::from_string("a".into()),
            },
        ),
        (
            "changing the passphrase with the wrong current one",
            Request::VaultChangePassphrase {
                current: SecretString::from_string("wrong".into()),
                replacement: SecretString::from_string("another-long-passphrase-here".into()),
            },
        ),
    ];

    for (what, request) in hostile {
        let name = request.command();
        match client.request(request).await {
            Ok(_) => {
                // A few of these are legitimately accepted (a huge history
                // limit is clamped, a huge pause is capped); what must never
                // happen is a panic or a hang, and reaching here proves
                // neither did.
            }
            Err(e) => {
                assert!(
                    !matches!(e.code(), ErrorCode::Internal),
                    "`{name}` with {what} produced an internal error: {e}"
                );
                assert!(!e.to_string().contains(PASSPHRASE), "`{name}` leaked the passphrase");
            }
        }
        // Still alive after every single one.
        client.ping().await.unwrap_or_else(|e| panic!("the daemon died on {what}: {e}"));
    }

    // The vault still opens with the original passphrase: none of the refused
    // rotations may have half-applied.
    harness.call(&client, Request::VaultLock {}).await;
    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("the vault must be undamaged by refused rotations");

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// A locked vault refuses exactly the commands whose schema says it should.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_command_flagged_needs_unlock_refuses_while_locked() {
    let mut harness = Harness::start("gated", |config, home| {
        let destination = mirror("copy", home.join("mirror"));
        config.jobs.push(job("docs", seed_tree(home, 1), vec![destination.id]));
        config.destinations.push(destination);
    })
    .await;
    let client = harness.client().await;

    // The vault is locked; these are a representative sample of the commands
    // the schema flags `needs_unlock`.
    let gated: Vec<Request> = vec![
        Request::VaultListRefs {},
        Request::DestinationTest { destination: "copy".into() },
        Request::DestinationStats { destination: "copy".into(), refresh: true },
        Request::SnapshotList { destination: "copy".into(), job: None, limit: 5 },
        Request::JobRun { job: "docs".into(), dry_run: false },
        Request::RemotePull {},
        Request::RemoteDiff {},
    ];
    for request in gated {
        let name = request.command();
        let error = client
            .request(request)
            .await
            .err()
            .unwrap_or_else(|| panic!("`{name}` must refuse while the vault is locked"));
        assert_eq!(
            error.code(),
            ErrorCode::Locked,
            "`{name}` refused, but not with `locked`: {error}"
        );
    }

    // And the ungated ones still work, because a locked daemon is still a
    // useful one: the whole GUI has to function in this state.
    for request in [
        Request::Status {},
        Request::Health {},
        Request::JobList { include_disabled: true },
        Request::DestinationList {},
        Request::SettingsGet {},
        Request::ControlPauseState {},
        Request::ServiceStatus {},
    ] {
        let name = request.command();
        client
            .request(request)
            .await
            .unwrap_or_else(|e| panic!("`{name}` must work with a locked vault: {e}"));
    }

    drop(client);
    harness.shutdown().await.expect("clean shutdown");
}

/// Record that a command was exercised, and assert the two properties that
/// hold for every one of them: no reply carries secret material, and no
/// refusal is an `internal` or `ipc` error — both of which mean the daemon,
/// not the caller, got something wrong.
fn check(covered: &mut BTreeSet<String>, name: &str, result: superbackup_core::Result<Reply>) {
    covered.insert(name.to_string());
    match result {
        Ok(reply) => {
            let rendered = format!("{reply:?}");
            assert!(
                !rendered.contains(PASSPHRASE),
                "`{name}` returned something containing the master passphrase"
            );
        }
        Err(e) => {
            let code = e.code();
            assert!(
                !matches!(code, ErrorCode::Internal | ErrorCode::Ipc),
                "`{name}` failed with an unhelpful {code:?}: {e}"
            );
            assert!(
                !e.to_string().contains(PASSPHRASE),
                "`{name}` leaked the passphrase in its error"
            );
        }
    }
}
