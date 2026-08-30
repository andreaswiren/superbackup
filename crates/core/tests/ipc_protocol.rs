//! Wire-contract tests.
//!
//! These are the tests that stop the protocol from rotting:
//!
//! * every [`Request`] variant round-trips through JSON;
//! * every [`Reply`] variant round-trips through JSON;
//! * **every `Request` variant appears in the generated schema**, and the
//!   schema's names and parameter names are the ones serde actually accepts —
//!   the check that makes `{"cmd":"schema"}` trustworthy;
//! * a passphrase never appears in `Debug`;
//! * a protocol-version mismatch is rejected with a message that says what to
//!   do about it.
//!
//! [`sample_requests`] is deliberately hand-written and exhaustive. Adding a
//! command to the table without adding it here makes
//! `every_request_variant_is_in_the_schema` fail on the count, which is the
//! drift alarm.

use std::collections::BTreeSet;
use std::path::PathBuf;

use chrono::Utc;
use superbackup_core::error::ErrorCode;
use superbackup_core::ipc::protocol::{
    self, AckReply, BandwidthReply, CheckStatus, ConflictPolicy, DestinationReply,
    DestinationsReply, DoctorCheck, DoctorReply, ErrorPayload, HealthReply, JobReply, JobsReply,
    ListingReply, PauseReply, ProbeReply, ProviderReply, ProvidersReply, RemoteDiffReply,
    RemoteStatusReply, Reply, RepositoryReply, Request, RequestId, RunsReply, SchemaReply,
    SecretRefsReply, SecretString, ServiceReply, SettingsReply, SnapshotsReply, StartedReply,
    StatusReply, StoppedReply, StorageStatsReply, SubscribedReply, UnlockedReply, UsedByReply,
    VersionReply,
};
use superbackup_core::ipc::{
    ClientFrame, ServerFrame, Topic, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
use superbackup_core::model::{
    BandwidthSettings, Destination, DestinationKind, EncryptionSettings, ExclusionSet, Job,
    JobHooks, ProviderKind, RetentionPolicy, S3Credentials, S3Flavour, Schedule, SecretRef,
    Settings, StorageProvider,
};
use superbackup_core::state::{
    DestinationRun, Health, JobRun, Progress, RunStatus, StatusSnapshot, Trigger,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn job() -> Job {
    Job {
        id: Uuid::nil(),
        name: "documents".into(),
        project_id: None,
        description: "Everything under ~/Documents".into(),
        sources: vec![],
        destination_ids: vec![],
        schedule: Schedule::Daily { times: vec![] },
        exclusions: ExclusionSet::default(),
        bandwidth: None,
        retention: None,
        enabled: true,
        timeout_minutes: None,
        hooks: JobHooks::default(),
        continue_on_destination_error: true,
        created_at: Utc::now(),
        tags: vec![],
    }
}

fn destination() -> Destination {
    Destination {
        id: Uuid::nil(),
        name: "external drive".into(),
        kind: DestinationKind::LocalRepository { path: PathBuf::from("/mnt/backup") },
        encryption: Some(EncryptionSettings::default()),
        passphrase_ref: None,
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: Utc::now(),
        last_verified_at: None,
    }
}

fn provider() -> StorageProvider {
    let id = Uuid::nil();
    StorageProvider {
        id,
        name: "StorJ".into(),
        kind: ProviderKind::S3 {
            endpoint: "https://gateway.storjshare.io".into(),
            region: "eu-1".into(),
            credentials: S3Credentials::for_provider(&id),
            tls: true,
            path_style: false,
            flavour: S3Flavour::Storj,
        },
        notes: String::new(),
        created_at: Utc::now(),
        last_verified_at: None,
    }
}

fn snapshot() -> StatusSnapshot {
    StatusSnapshot {
        health: Health::Idle,
        version: "0.1.0".into(),
        machine_label: "test".into(),
        machine_slug: "test".into(),
        unlocked: false,
        paused: false,
        paused_until: None,
        service_installed: false,
        service_running: false,
        kopia_version: None,
        active_runs: vec![],
        jobs: Default::default(),
        next_scheduled: None,
        recent_events: vec![],
        uptime_seconds: 0,
        generated_at: Utc::now(),
    }
}

fn secret(s: &str) -> SecretString {
    SecretString::from_string(s.to_string())
}

/// One value of **every** [`Request`] variant.
///
/// Exhaustiveness is enforced by
/// [`every_request_variant_is_in_the_schema`], which compares this list's
/// length against the generated schema's. Add a command to the table and this
/// test fails until it is added here too — which is the point.
fn sample_requests() -> Vec<Request> {
    vec![
        // status
        Request::Ping {},
        Request::Status {},
        Request::Version {},
        Request::Health {},
        Request::Doctor { fix: true },
        // jobs
        Request::JobList { include_disabled: true },
        Request::JobGet { job: "documents".into() },
        Request::JobCreate { job: Box::new(job()) },
        Request::JobUpdate { job: Box::new(job()) },
        Request::JobDelete { job: "documents".into() },
        Request::JobSetEnabled { job: "documents".into(), enabled: false },
        Request::JobRun { job: "documents".into(), dry_run: true },
        Request::JobStop { run_id: Uuid::nil() },
        Request::JobStopAll {},
        Request::JobHistory { job: Some("documents".into()), limit: 20 },
        // destinations
        Request::DestinationList {},
        Request::DestinationGet { destination: "drive".into() },
        Request::DestinationCreate { destination: Box::new(destination()) },
        Request::DestinationUpdate { destination: Box::new(destination()) },
        Request::DestinationDelete { destination: "drive".into(), force: false },
        Request::DestinationTest { destination: "drive".into() },
        Request::DestinationRepoCreate {
            destination: "drive".into(),
            encryption: Some(EncryptionSettings::default()),
        },
        Request::DestinationRepoConnect { destination: "drive".into() },
        Request::DestinationRepoDisconnect { destination: "drive".into() },
        Request::DestinationStats { destination: "drive".into(), refresh: true },
        // providers
        Request::ProviderList {},
        Request::ProviderGet { provider: "storj".into() },
        Request::ProviderCreate { provider: Box::new(provider()) },
        Request::ProviderUpdate { provider: Box::new(provider()) },
        Request::ProviderDelete { provider: "storj".into(), force: false },
        Request::ProviderTest { provider: "storj".into() },
        Request::ProviderUsedBy { provider: "storj".into() },
        Request::ProviderRotateCredentials {
            provider: "storj".into(),
            access_key_id: secret("AKIAEXAMPLE"),
            secret_access_key: secret("s3cr3t"),
            session_token: None,
        },
        // snapshots
        Request::SnapshotList { destination: "drive".into(), job: None, limit: 50 },
        Request::SnapshotBrowse {
            destination: "drive".into(),
            snapshot: "k1234".into(),
            path: "Documents".into(),
        },
        Request::SnapshotRestore {
            destination: "drive".into(),
            snapshot: "k1234".into(),
            path: String::new(),
            target: PathBuf::from("/tmp/restore"),
            conflict: ConflictPolicy::Skip,
            dry_run: false,
        },
        Request::SnapshotDelete { destination: "drive".into(), snapshot: "k1234".into() },
        // vault
        Request::VaultUnlock { passphrase: secret("correct horse battery staple") },
        Request::VaultLock {},
        Request::VaultIsUnlocked {},
        Request::VaultChangePassphrase {
            current: secret("old one"),
            replacement: secret("new one"),
        },
        Request::VaultSetSecret {
            secret_ref: SecretRef::new("s3-access-key", &Uuid::nil()),
            value: secret("AKIAEXAMPLE"),
        },
        Request::VaultListRefs {},
        // control
        Request::ControlPause { seconds: Some(3600), reason: Some("presenting".into()) },
        Request::ControlResume {},
        Request::ControlPauseState {},
        Request::ControlSetBandwidth { bandwidth: BandwidthSettings::default() },
        Request::ControlReloadConfig {},
        Request::ControlShutdown { stop_runs: true },
        // settings
        Request::SettingsGet {},
        Request::SettingsUpdate { settings: Box::new(Settings::default()) },
        // remote
        Request::RemotePull {},
        Request::RemoteDiff {},
        Request::RemoteApply {},
        Request::RemotePush { message: Some("from the laptop".into()) },
        // service
        Request::ServiceStatus {},
        Request::ServiceInstall {},
        Request::ServiceUninstall {},
        Request::ServiceSetAutostart { enabled: true },
        // transport-answered
        Request::Schema {},
        Request::Subscribe { topics: vec![Topic::Progress] },
    ]
}

/// One value of every [`Reply`] variant.
fn sample_replies() -> Vec<Reply> {
    let run = JobRun {
        run_id: Uuid::nil(),
        job_id: Uuid::nil(),
        job_name: "documents".into(),
        trigger: Trigger::Manual,
        status: RunStatus::Running,
        started_at: Utc::now(),
        finished_at: None,
        destinations: vec![DestinationRun {
            destination_id: Uuid::nil(),
            destination_name: "drive".into(),
            status: RunStatus::Running,
            started_at: None,
            finished_at: None,
            progress: Progress::default(),
            snapshot_id: None,
            error: None,
            warnings: vec![],
        }],
    };

    vec![
        Reply::Ack(AckReply {}),
        Reply::Status(StatusReply { snapshot: Box::new(snapshot()) }),
        Reply::Version(VersionReply {
            version: "0.1.0".into(),
            protocol: PROTOCOL_VERSION,
            min_protocol: MIN_PROTOCOL_VERSION,
            target_os: "windows".into(),
            target_arch: "x86_64".into(),
            kopia_version: None,
            service_scope: false,
        }),
        Reply::Health(HealthReply {
            health: Health::Attention,
            summary: "The vault is locked".into(),
            reasons: vec!["vault.locked".into()],
        }),
        Reply::Doctor(DoctorReply {
            ok: false,
            checks: vec![DoctorCheck {
                id: "kopia.present".into(),
                title: "kopia is installed".into(),
                status: CheckStatus::Fail,
                detail: None,
                hint: Some("Run `superbackup doctor --fix`.".into()),
                fixable: true,
            }],
            fixed: vec![],
        }),
        Reply::Schema(SchemaReply { schema: Box::new(protocol::schema()) }),
        Reply::Jobs(JobsReply { jobs: vec![job()] }),
        Reply::Job(JobReply { job: Box::new(job()) }),
        Reply::Runs(RunsReply { runs: vec![run] }),
        Reply::Started(StartedReply { run_id: Uuid::nil(), started: true, note: None }),
        Reply::Stopped(StoppedReply { stopped: vec![Uuid::nil()] }),
        Reply::Destinations(DestinationsReply { destinations: vec![destination()] }),
        Reply::Destination(DestinationReply { destination: Box::new(destination()) }),
        Reply::Providers(ProvidersReply { providers: vec![provider()] }),
        Reply::Provider(ProviderReply { provider: Box::new(provider()) }),
        Reply::Probe(ProbeReply {
            reachable: true,
            writable: true,
            latency_ms: Some(12),
            detail: None,
        }),
        Reply::Repository(RepositoryReply {
            destination_id: Uuid::nil(),
            connected: true,
            repository_id: Some("abc".into()),
            created: false,
        }),
        Reply::StorageStats(StorageStatsReply {
            destination_id: Uuid::nil(),
            snapshot_count: 7,
            logical_bytes: Some(1024),
            stored_bytes: Some(512),
            last_snapshot_at: None,
            computed_at: Utc::now(),
        }),
        Reply::UsedBy(UsedByReply { destinations: vec![], jobs: vec![] }),
        Reply::Snapshots(SnapshotsReply { snapshots: vec![] }),
        Reply::Listing(ListingReply { path: String::new(), entries: vec![], truncated: false }),
        Reply::Unlocked(UnlockedReply { unlocked: true, auto_lock_at: None }),
        Reply::SecretRefs(SecretRefsReply {
            refs: vec![SecretRef::new("s3-access-key", &Uuid::nil())],
        }),
        Reply::Pause(PauseReply { pause: Default::default() }),
        Reply::Bandwidth(BandwidthReply { bandwidth: BandwidthSettings::default() }),
        Reply::Settings(SettingsReply { settings: Box::new(Settings::default()) }),
        Reply::RemoteStatus(RemoteStatusReply {
            url: None,
            branch: None,
            last_pull_at: None,
            last_known_commit: None,
            local_changes: false,
            remote_changes: false,
            detail: None,
        }),
        Reply::RemoteDiff(RemoteDiffReply { changes: vec![], remote_commit: None }),
        Reply::Service(ServiceReply {
            installed: true,
            running: true,
            autostart: false,
            scope: "system".into(),
            detail: None,
        }),
        Reply::Subscribed(SubscribedReply { subscription: RequestId(7), topics: Topic::all() }),
    ]
}

// ---------------------------------------------------------------------------
// Round trips
// ---------------------------------------------------------------------------

#[test]
fn every_request_round_trips_through_json() {
    for request in sample_requests() {
        let json = serde_json::to_string(&request)
            .unwrap_or_else(|e| panic!("{} failed to serialise: {e}", request.command()));
        let back: Request = serde_json::from_str(&json).unwrap_or_else(|e| {
            panic!("{} failed to deserialise from {json}: {e}", request.command())
        });
        assert_eq!(back.command(), request.command(), "round trip changed the command: {json}");

        // The wire form must carry the command under `cmd`, flat, so that a
        // request can be written by hand.
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            value.get("cmd").and_then(|v| v.as_str()),
            Some(request.command()),
            "the `cmd` discriminator is missing or wrong: {json}"
        );
    }
}

#[test]
fn every_reply_round_trips_through_json() {
    for reply in sample_replies() {
        let json = serde_json::to_string(&reply)
            .unwrap_or_else(|e| panic!("{} failed to serialise: {e}", reply.tag()));
        let back: Reply = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("{} failed to deserialise: {e}", reply.tag()));
        assert_eq!(back.tag(), reply.tag(), "round trip changed the reply tag: {json}");

        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            value.get("reply").and_then(|v| v.as_str()),
            Some(reply.tag()),
            "the `reply` discriminator is missing or wrong: {json}"
        );
    }
}

#[test]
fn frames_round_trip_through_json() {
    let frames = vec![
        ServerFrame::Hello {
            protocol: PROTOCOL_VERSION,
            min_protocol: MIN_PROTOCOL_VERSION,
            version: "0.1.0".into(),
            service_scope: false,
        },
        ServerFrame::Ok { id: RequestId(1), body: Box::new(Reply::Ack(AckReply {})) },
        ServerFrame::Error {
            id: RequestId(2),
            body: ErrorPayload::new(ErrorCode::Locked, "the vault is locked"),
        },
        ServerFrame::End { id: RequestId(3) },
        ServerFrame::Bye { reason: "shutting down".into() },
    ];
    for frame in frames {
        let json = serde_json::to_string(&frame).expect("serialise");
        let _: ServerFrame = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("frame did not round trip: {json}: {e}"));
    }

    let client = ClientFrame::Request {
        id: RequestId(4),
        protocol: PROTOCOL_VERSION,
        body: Request::Status {},
    };
    let json = serde_json::to_string(&client).expect("serialise");
    let _: ClientFrame = serde_json::from_str(&json).expect("round trip");

    // `protocol` may be omitted by a hand-written client.
    let terse: ClientFrame =
        serde_json::from_str(r#"{"type":"request","id":9,"body":{"cmd":"status"}}"#)
            .expect("a request without an explicit protocol must be accepted");
    match terse {
        ClientFrame::Request { protocol, .. } => assert_eq!(protocol, PROTOCOL_VERSION),
        other => panic!("expected a request frame, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Schema discovery
// ---------------------------------------------------------------------------

#[test]
fn every_request_variant_is_in_the_schema() {
    let schema = protocol::schema();
    let names: BTreeSet<&str> = schema.commands.iter().map(|c| c.name.as_str()).collect();

    for request in sample_requests() {
        assert!(
            names.contains(request.command()),
            "`{}` is a Request variant but is missing from the schema",
            request.command()
        );
    }

    // The count check is the drift alarm in the other direction: a command
    // added to the table but not to `sample_requests` fails here, which forces
    // whoever adds a command to also prove it round-trips.
    assert_eq!(
        sample_requests().len(),
        schema.commands.len(),
        "the schema has {} commands but only {} are covered by `sample_requests`; \
         add the new command to the test fixture",
        schema.commands.len(),
        sample_requests().len()
    );
    assert_eq!(names.len(), schema.commands.len(), "duplicate command names in the schema");
}

#[test]
fn schema_parameter_names_are_the_ones_serde_accepts() {
    // The check that makes `{"cmd":"schema"}` trustworthy: for every command,
    // build a request out of the schema's own description of it and prove the
    // deserialiser accepts it. A renamed field, a missing `#[serde(rename)]`,
    // or a stale hand-written entry all fail here.
    for command in protocol::schema().commands {
        let mut object = serde_json::Map::new();
        object.insert("cmd".into(), serde_json::Value::String(command.name.clone()));
        for param in &command.params {
            if param.optional {
                continue; // absence is legal by definition
            }
            object.insert(param.name.clone(), placeholder(&param.ty));
        }
        let json = serde_json::Value::Object(object);
        let parsed: Result<Request, _> = serde_json::from_value(json.clone());
        let request = parsed.unwrap_or_else(|e| {
            panic!("the schema entry for `{}` does not deserialise: {e}\n{json}", command.name)
        });
        assert_eq!(request.command(), command.name);
    }
}

/// A minimal legal value for a parameter type named in the schema.
///
/// Only the types the command table actually uses; an unhandled one is a
/// deliberate failure, because a new parameter type should be a considered
/// addition to the discovery story rather than silently untested.
fn placeholder(ty: &str) -> serde_json::Value {
    use serde_json::json;
    match ty {
        "bool" => json!(false),
        "u32" | "u64" => json!(0),
        "String" | "SecretString" => json!(""),
        "Uuid" => json!(Uuid::nil()),
        "PathBuf" => json!("/tmp/x"),
        "SecretRef" => json!("s3-access-key:00000000-0000-0000-0000-000000000000"),
        "ConflictPolicy" => json!("skip"),
        "Vec<Topic>" => json!([]),
        "BandwidthSettings" => serde_json::to_value(BandwidthSettings::default()).expect("ser"),
        "Job" => serde_json::to_value(job()).expect("ser"),
        "Destination" => serde_json::to_value(destination()).expect("ser"),
        "StorageProvider" => serde_json::to_value(provider()).expect("ser"),
        "Settings" => serde_json::to_value(Settings::default()).expect("ser"),
        other => panic!(
            "no schema placeholder for parameter type `{other}`; add one so the new type is \
             covered by discovery testing"
        ),
    }
}

#[test]
fn schema_describes_replies_error_codes_topics_and_limits() {
    let schema = protocol::schema();

    // Every command's declared reply tag must be a reply the schema also
    // describes, or a client cannot know what shape to expect.
    let replies: BTreeSet<&str> = schema.replies.iter().map(|r| r.name.as_str()).collect();
    for command in &schema.commands {
        assert!(
            replies.contains(command.reply.as_str()),
            "`{}` claims to return `{}`, which is not a described reply",
            command.name,
            command.reply
        );
    }

    // Every reply the schema describes must be produced by something.
    let produced: BTreeSet<&str> = schema.commands.iter().map(|c| c.reply.as_str()).collect();
    for reply in &schema.replies {
        assert!(
            produced.contains(reply.name.as_str()),
            "reply `{}` is described but no command produces it",
            reply.name
        );
    }

    // The error-code list must match what `ErrorCode` actually serialises to.
    for code in &schema.error_codes {
        let json = format!("\"{code}\"");
        serde_json::from_str::<ErrorCode>(&json)
            .unwrap_or_else(|e| panic!("`{code}` is not a real ErrorCode: {e}"));
    }

    assert_eq!(schema.protocol, PROTOCOL_VERSION);
    assert_eq!(schema.min_protocol, MIN_PROTOCOL_VERSION);
    assert!(schema.limits.max_line_bytes > 0);
    assert_eq!(schema.topics.len(), Topic::all().len());
}

#[test]
fn schema_marks_secrets_and_mutations() {
    let schema = protocol::schema();

    let unlock =
        schema.commands.iter().find(|c| c.name == "vault.unlock").expect("vault.unlock must exist");
    let passphrase =
        unlock.params.iter().find(|p| p.name == "passphrase").expect("passphrase parameter");
    assert!(passphrase.secret, "the passphrase parameter must be flagged secret");

    let status = schema.commands.iter().find(|c| c.name == "status").expect("status must exist");
    assert!(!status.is_mutating(), "`status` must not be flagged mutating");

    let run = schema.commands.iter().find(|c| c.name == "job.run").expect("job.run must exist");
    assert!(run.is_mutating());
    assert!(run.needs_unlock(), "a run needs repository credentials");

    // A `Box<T>` parameter must be published as `T`: boxing is a layout
    // decision with no meaning to a client in another language.
    let create =
        schema.commands.iter().find(|c| c.name == "job.create").expect("job.create must exist");
    assert_eq!(create.params[0].ty, "Job", "Box<Job> must be published as Job");

    // Optional parameters must be marked, or a client will send `null` where
    // the key should be absent.
    let history =
        schema.commands.iter().find(|c| c.name == "job.history").expect("job.history must exist");
    let job_param = history.params.iter().find(|p| p.name == "job").expect("job parameter");
    assert!(job_param.optional);
}

#[test]
fn the_schema_itself_survives_json() {
    // An agent or another language's client generator consumes this. If it
    // cannot be parsed back, discovery is worthless.
    let json = serde_json::to_string(&protocol::schema()).expect("schema serialises");
    let back: superbackup_core::ipc::Schema =
        serde_json::from_str(&json).expect("schema deserialises");
    assert_eq!(back.commands.len(), protocol::schema().commands.len());
}

// ---------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------

#[test]
fn a_passphrase_never_appears_in_debug_output() {
    const PASSPHRASE: &str = "correct-horse-battery-staple-9271";

    let request = Request::VaultUnlock { passphrase: secret(PASSPHRASE) };
    let rendered = format!("{request:?}");
    assert!(!rendered.contains(PASSPHRASE), "Debug for Request leaked the passphrase: {rendered}");
    assert!(rendered.contains("redacted"), "expected a redaction marker: {rendered}");

    // The same must hold once it is wrapped in a frame, which is what a
    // tracing span would actually print.
    let frame = ClientFrame::Request {
        id: RequestId(1),
        protocol: PROTOCOL_VERSION,
        body: Request::VaultUnlock { passphrase: secret(PASSPHRASE) },
    };
    let rendered = format!("{frame:?}");
    assert!(!rendered.contains(PASSPHRASE), "Debug for ClientFrame leaked it: {rendered}");

    // And for the other two secret-carrying commands.
    let change = Request::VaultChangePassphrase {
        current: secret(PASSPHRASE),
        replacement: secret("something else entirely"),
    };
    assert!(!format!("{change:?}").contains(PASSPHRASE));

    let rotate = Request::ProviderRotateCredentials {
        provider: "storj".into(),
        access_key_id: secret(PASSPHRASE),
        secret_access_key: secret(PASSPHRASE),
        session_token: Some(secret(PASSPHRASE)),
    };
    assert!(!format!("{rotate:?}").contains(PASSPHRASE));
}

#[test]
fn a_passphrase_deserialises_into_a_secret_and_still_serialises_for_the_client() {
    let json = r#"{"cmd":"vault.unlock","passphrase":"hunter2"}"#;
    let request: Request = serde_json::from_str(json).expect("deserialise");
    match &request {
        Request::VaultUnlock { passphrase } => {
            assert_eq!(passphrase.expose().expose_str(), Some("hunter2"));
        }
        other => panic!("wrong variant: {other:?}"),
    }
    // A client must be able to send it, or unlocking would be impossible.
    let round = serde_json::to_string(&request).expect("serialise");
    assert!(round.contains("hunter2"));
}

#[test]
fn there_is_no_request_that_reads_a_secret_back() {
    // The load-bearing security property of this protocol, asserted rather
    // than merely documented: no command returns plaintext secret material.
    let schema = protocol::schema();
    for command in &schema.commands {
        assert_ne!(command.reply, "secret", "a reply returning a secret was added");
    }
    assert!(
        schema.commands.iter().any(|c| c.name == "vault.set_secret"),
        "secrets must still be settable"
    );
    assert!(
        !schema.commands.iter().any(|c| c.name.contains("get_secret")),
        "a get_secret command was added; see the module documentation for why there is none"
    );

    // `vault.list_refs` returns handles, never values.
    let refs = schema
        .commands
        .iter()
        .find(|c| c.name == "vault.list_refs")
        .expect("vault.list_refs must exist");
    assert_eq!(refs.reply, "secret_refs");
}

// ---------------------------------------------------------------------------
// Version negotiation
// ---------------------------------------------------------------------------

#[test]
fn protocol_version_mismatch_is_rejected_with_an_actionable_message() {
    protocol::check_protocol(PROTOCOL_VERSION).expect("the current version must be accepted");

    let too_new =
        protocol::check_protocol(PROTOCOL_VERSION + 1).expect_err("a newer client must be refused");
    assert_eq!(too_new.code(), ErrorCode::Ipc);
    let message = too_new.to_string();
    assert!(message.contains("upgrade the daemon"), "unhelpful message: {message}");
    assert!(
        message.contains(&(PROTOCOL_VERSION + 1).to_string()),
        "the message must name the client's version: {message}"
    );

    if MIN_PROTOCOL_VERSION > 0 {
        let too_old = protocol::check_protocol(MIN_PROTOCOL_VERSION - 1)
            .expect_err("an older client must be refused");
        assert!(too_old.to_string().contains("upgrade the client"), "unhelpful message: {too_old}");
    }
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

#[test]
fn outbound_frames_are_scrubbed() {
    let leaky = "fatal: could not read https://ghp_AbC123DeadBeef@github.com/me/cfg.git";

    let frame = ServerFrame::Error {
        id: RequestId(1),
        body: ErrorPayload {
            code: ErrorCode::Remote,
            message: leaky.into(),
            hint: None,
            detail: None,
        },
    }
    .sanitise();
    let json = serde_json::to_string(&frame).expect("serialise");
    assert!(!json.contains("ghp_AbC123DeadBeef"), "a token survived sanitisation: {json}");

    let probe = ServerFrame::Ok {
        id: RequestId(2),
        body: Box::new(Reply::Probe(ProbeReply {
            reachable: false,
            writable: false,
            latency_ms: None,
            detail: Some("AWS_SECRET_ACCESS_KEY=abc/def+123 rejected".into()),
        })),
    }
    .sanitise();
    let json = serde_json::to_string(&probe).expect("serialise");
    assert!(!json.contains("abc/def+123"), "a key survived sanitisation: {json}");
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

#[test]
fn error_payloads_preserve_their_code_across_the_wire() {
    use superbackup_core::error::Error;

    for error in [
        Error::Locked,
        Error::BadPassphrase,
        Error::KopiaMissing,
        Error::DaemonUnreachable,
        Error::JobNotFound("documents".into()),
        Error::Validation("nope".into()),
        Error::Kopia { status: 1, stderr: "boom".into() },
    ] {
        let payload = ErrorPayload::from_error(&error);
        let json = serde_json::to_string(&payload).expect("serialise");
        let back: ErrorPayload = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.code, error.code(), "the error code must survive: {json}");

        // Reconstructing an `Error` is documented as lossy for the variants
        // that carry non-JSON fields; they collapse to `Internal`. Everything
        // else must come back with the same code, because that is what
        // callers are told to branch on.
        let lossy = matches!(
            error.code(),
            ErrorCode::Io | ErrorCode::VaultVersion | ErrorCode::Kopia | ErrorCode::Internal
        );
        if !lossy {
            assert_eq!(
                back.clone().into_error().code(),
                error.code(),
                "code changed on the way back: {json}"
            );
        }
    }
}
