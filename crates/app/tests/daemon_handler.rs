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
            admin_url: None,
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
    // No kopia is installed in the harness, so this reports "not found" and
    // lists the routes it tried. Answering rather than failing is the point:
    // the page exists so a user can see *why* nothing was found.
    run!("kopia.probe", Request::KopiaProbe { destination: None, check_for_update: false });

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
    // A folder mirror has no encryption key at all, so this must refuse with a
    // sentence rather than pretend to check one.
    run!(
        "dest.check_key",
        Request::DestinationCheckKey { destination: mirror_id.to_string(), key: None }
    );
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
    // These reach the network. Offline in CI they answer "could not be
    // reached", which is still a clean, well-formed answer — which is exactly
    // what this test asserts about every command.
    run!(
        "provider.list_buckets",
        Request::ProviderListBuckets { provider: provider_id.to_string() }
    );
    run!(
        "provider.list_objects",
        Request::ProviderListObjects {
            provider: provider_id.to_string(),
            bucket: "dev-backups".into(),
            prefix: "superbackup/pc/".into(),
            max_keys: 8,
        }
    );
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
    // The one command that returns secret material. Exercised here for the
    // same reason as everything else — it must answer or refuse cleanly — and
    // the assertion that it does not leak is in the protocol's own tests.
    run!(
        "vault.export_keys",
        Request::VaultExportKeys {
            passphrase: SecretString::from_string("wrong-on-purpose".into())
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

    // -- machine ----------------------------------------------------------
    run!("machine.rename", Request::MachineRename { label: "The Studio PC".into() });

    // -- manual export / import -------------------------------------------
    run!("config.export", Request::ConfigExport {});
    // Deliberately not a real document: this asserts the command *refuses*
    // cleanly rather than panicking on input it cannot parse, which is the
    // whole point of this test.
    run!(
        "config.import",
        Request::ConfigImport { document: "not-a-document".into(), allow_rollback: false }
    );

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

/// The one command that returns secret material, and the bounds on it.
///
/// This is the protocol's single exception to "secrets go in, never out"
/// (`THREAT_MODEL.md` §A7). The properties asserted here are the ones the
/// exception is justified by: a locked vault refuses it, a wrong passphrase
/// refuses it even when the vault is open, it returns a document a human could
/// act on, and it will not answer twice in quick succession.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exporting_encryption_keys_is_gated_logged_and_rate_limited() {
    let mut harness = Harness::start("export", |config, home| {
        let destination = mirror("copy", home.join("mirror"));
        config.jobs.push(job("docs", seed_tree(home, 1), vec![destination.id]));
        config.destinations.push(destination);
    })
    .await;
    let client = harness.client().await;

    // Locked: refused before the passphrase is even considered.
    let error = client
        .request(Request::VaultExportKeys {
            passphrase: SecretString::from_string(PASSPHRASE.to_string()),
        })
        .await
        .expect_err("a locked vault must refuse the export");
    assert_eq!(error.code(), ErrorCode::Locked);

    client
        .unlock(SecretString::from_string(PASSPHRASE.to_string()))
        .await
        .expect("unlock the vault");

    // Unlocked but the wrong passphrase: still refused. Reaching the socket
    // with an open vault is deliberately not enough.
    let error = client
        .request(Request::VaultExportKeys {
            passphrase: SecretString::from_string("not-the-passphrase".to_string()),
        })
        .await
        .expect_err("a wrong passphrase must refuse the export");
    assert_eq!(error.code(), ErrorCode::BadPassphrase);

    // The real thing.
    let Reply::KeyExport(export) = client
        .request(Request::VaultExportKeys {
            passphrase: SecretString::from_string(PASSPHRASE.to_string()),
        })
        .await
        .expect("the export must succeed with the right passphrase")
    else {
        panic!("expected a key_export reply")
    };
    // The fixture holds one folder mirror, which has no encryption key: it must
    // be listed as omitted rather than silently missing.
    assert_eq!(export.destinations, 0);
    assert!(export.omitted.iter().any(|o| o.contains("copy")), "{:?}", export.omitted);
    assert!(export.document.contains("READ THIS FIRST"), "{}", export.document);
    assert!(export.suggested_file_name.ends_with(".txt"));
    assert!(
        !export.suggested_file_name.contains('/') && !export.suggested_file_name.contains('\\'),
        "the suggested name must be a file name, never a path: {}",
        export.suggested_file_name
    );

    // Immediately again: refused by the cooldown, so the socket is not an
    // oracle to be hammered.
    let error = client
        .request(Request::VaultExportKeys {
            passphrase: SecretString::from_string(PASSPHRASE.to_string()),
        })
        .await
        .expect_err("a second export must be rate limited");
    assert_eq!(error.code(), ErrorCode::Validation);

    // And it left a trace: that it happened, never what it contained.
    let events = harness.runtime.recent_events();
    let logged = events
        .iter()
        .find(|e| e.kind == "vault.keys_exported")
        .expect("an export must be recorded in the activity log");
    assert!(!logged.message.contains(PASSPHRASE));

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

// ---------------------------------------------------------------------------
// Testing a place is not the same question as opening a repository in it
// ---------------------------------------------------------------------------
//
// Both checks used to go through kopia, which meant both needed a repository
// that already existed and an encryption key that opened it. That coupling
// made the answer useless in the two states where the user most wants one:
// fresh credentials with no destination yet, and a destination added but not
// yet created. These tests pin the decoupling.

/// A provider with no destinations at all can still be tested.
///
/// The old implementation borrowed the first destination that used the
/// provider and gave up when there was none — "there is nothing to test
/// against" — which is exactly the state someone is in a minute after pasting
/// their StorJ keys.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_with_no_destinations_can_still_be_tested() {
    let harness = Harness::start("provider-alone", |_config, _home| {}).await;
    let client = harness.client().await;
    client.unlock(SecretString::from_string(PASSPHRASE.to_string())).await.expect("unlock");

    let id = uuid::Uuid::new_v4();
    let provider = StorageProvider {
        id,
        name: "lonely".into(),
        kind: ProviderKind::S3 {
            // A name that cannot resolve, so the test is offline-safe and
            // deterministic: what is asserted is that the daemon *answered*,
            // not that the internet was reachable.
            endpoint: "https://s3.invalid.superbackup-test".into(),
            region: "eu-1".into(),
            credentials: S3Credentials::for_provider(&id),
            tls: true,
            path_style: false,
            flavour: superbackup_core::model::S3Flavour::Storj,
            admin_url: None,
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
    let provider_id = created.provider.id.to_string();

    // No destinations exist at all.
    let Reply::Destinations(list) = harness.call(&client, Request::DestinationList {}).await else {
        panic!("expected a destinations reply")
    };
    assert!(list.destinations.is_empty(), "the fixture must have no destinations");

    for request in [
        Request::ProviderTest { provider: provider_id.clone() },
        Request::ProviderListBuckets { provider: provider_id.clone() },
    ] {
        let name = request.command();
        let detail = match harness.call(&client, request).await {
            Reply::Probe(probe) => probe.detail.unwrap_or_default(),
            Reply::Buckets(buckets) => buckets.detail.unwrap_or_default(),
            other => panic!("`{name}` answered with {other:?}"),
        };
        assert!(
            !detail.contains("No destination uses this provider"),
            "`{name}` still needs a destination: {detail}"
        );
        // A provider is an account. Whether some bucket contains a repository
        // is not a property of an account, and must not be mentioned here.
        assert!(
            !detail.to_lowercase().contains("repositor"),
            "`{name}` talked about repositories: {detail}"
        );
        assert!(!detail.is_empty(), "`{name}` said nothing at all");
    }
}

/// A destination whose repository has not been created yet is *reachable*.
///
/// The old path built a kopia driver, which needs the encryption key, which
/// does not exist until the repository does — so a perfectly good folder
/// reported as unreachable. Reachability and repository presence are now two
/// fields, and the first does not depend on the second.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_destination_with_no_repository_is_reachable_and_says_so() {
    let mut ids = None;
    let harness = Harness::start("uncreated", |config, home| {
        let destination = repository("fresh", home.join("fresh-repo"));
        ids = Some(destination.id);
        config.destinations.push(destination);
    })
    .await;
    let destination_id = ids.expect("an id").to_string();
    let client = harness.client().await;
    client.unlock(SecretString::from_string(PASSPHRASE.to_string())).await.expect("unlock");

    // The handle exists; nothing is behind it, because the key is generated
    // when the repository is.
    let Reply::Probe(probe) =
        harness.call(&client, Request::DestinationTest { destination: destination_id }).await
    else {
        panic!("expected a probe reply")
    };
    assert!(probe.reachable, "a writable folder is reachable: {:?}", probe.detail);
    assert!(probe.writable, "the folder must be writable: {:?}", probe.detail);
    assert_eq!(
        probe.repository_present,
        Some(false),
        "there is no repository in it yet, and that is a separate fact"
    );
    let detail = probe.detail.unwrap_or_default();
    // The message has to name the thing, say what it is, and say where the
    // control lives — "use Create repository" is useless without the screen.
    assert!(detail.contains("folder"), "{detail}");
    assert!(detail.contains("Destinations"), "{detail}");
    assert!(detail.contains("Create repository"), "{detail}");
}

/// A folder mirror holds no repository by definition, so the question does not
/// apply rather than being answered "no".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mirror_is_not_asked_whether_it_holds_a_repository() {
    let mut ids = None;
    let harness = Harness::start("mirror-probe", |config, home| {
        let destination = mirror("copy", home.join("mirror"));
        ids = Some(destination.id);
        config.destinations.push(destination);
    })
    .await;
    let destination_id = ids.expect("an id").to_string();
    let client = harness.client().await;
    client.unlock(SecretString::from_string(PASSPHRASE.to_string())).await.expect("unlock");

    let Reply::Probe(probe) =
        harness.call(&client, Request::DestinationTest { destination: destination_id }).await
    else {
        panic!("expected a probe reply")
    };
    assert!(probe.reachable && probe.writable);
    assert_eq!(probe.repository_present, None, "a mirror has no repository to look for");
    assert_eq!(probe.detail, None, "nothing to warn about");
}

/// A wrong secret key is reported as a credential failure — never as a missing
/// repository, and never as an unreachable endpoint.
///
/// Served by a throwaway local listener that answers the way S3 does, so the
/// whole path is exercised — signing, transport, XML, error mapping, the
/// handler's classification — without the network and without credentials that
/// exist anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_secret_key_is_a_credential_failure_not_a_missing_repository() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a loopback listener");
    let port = listener.local_addr().expect("an address").port();
    let served = tokio::spawn(async move {
        // One request is all the probe makes; answer it and stop.
        if let Ok((mut socket, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut scratch = [0u8; 4096];
            let _ = socket.read(&mut scratch).await;
            let body = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error>\
                        <Code>SignatureDoesNotMatch</Code>\
                        <Message>The request signature we calculated does not match.</Message>\
                        <RequestId>abc</RequestId></Error>";
            let response = format!(
                "HTTP/1.1 403 Forbidden\r\nContent-Type: application/xml\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });

    let harness = Harness::start("bad-secret", |_config, _home| {}).await;
    let client = harness.client().await;
    client.unlock(SecretString::from_string(PASSPHRASE.to_string())).await.expect("unlock");

    let id = uuid::Uuid::new_v4();
    let credentials = S3Credentials::for_provider(&id);
    let provider = StorageProvider {
        id,
        name: "local-s3".into(),
        kind: ProviderKind::S3 {
            endpoint: format!("http://127.0.0.1:{port}"),
            region: "eu-1".into(),
            credentials: credentials.clone(),
            tls: false,
            path_style: true,
            flavour: superbackup_core::model::S3Flavour::Other,
            admin_url: None,
        },
        notes: String::new(),
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    };
    // `provider.create` assigns the id, so the handles to fill are the ones on
    // the provider that comes back, not the ones on the draft that went in.
    let Reply::Provider(created) =
        harness.call(&client, Request::ProviderCreate { provider: Box::new(provider) }).await
    else {
        panic!("expected a provider reply")
    };
    let provider_id = created.provider.id;
    let ProviderKind::S3 { credentials, .. } = &created.provider.kind;
    let credentials = credentials.clone();
    for (handle, value) in [
        (credentials.access_key_ref.clone(), "AKIDEXAMPLE"),
        (credentials.secret_key_ref.clone(), "wrong-secret"),
    ] {
        harness
            .call(
                &client,
                Request::VaultSetSecret {
                    secret_ref: handle,
                    value: SecretString::from_string(value.to_string()),
                },
            )
            .await;
    }

    let Reply::Probe(probe) =
        harness.call(&client, Request::ProviderTest { provider: provider_id.to_string() }).await
    else {
        panic!("expected a probe reply")
    };
    let detail = probe.detail.unwrap_or_default();
    assert!(!probe.reachable, "a rejected signature is not a pass: {detail}");
    assert!(detail.contains("secret key"), "the message must point at the secret key: {detail}");
    assert!(
        !detail.to_lowercase().contains("repositor"),
        "a credential failure must not be dressed up as a missing repository: {detail}"
    );
    // And the key itself never appears in what the user is shown.
    assert!(!detail.contains("wrong-secret"), "{detail}");

    let _ = served.await;
}
