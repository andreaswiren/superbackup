//! Loading, migration, atomic saving, the combined `Store`, and vault garbage
//! collection.

use std::path::PathBuf;

use superbackup_core::config::{migrate, plan_gc, ConfigStore, Store, StoreState};
use superbackup_core::crypto::{KdfParams, Vault};
use superbackup_core::error::Error;
use superbackup_core::model::*;
use superbackup_core::paths::Paths;
use superbackup_core::secret::Secret;
use uuid::Uuid;

struct Home(PathBuf);

impl Home {
    fn new(tag: &str) -> Home {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir =
            std::env::temp_dir().join(format!("sb-cfg-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Home(dir)
    }
    fn paths(&self) -> Paths {
        Paths::rooted_at(&self.0, false)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn abs(tail: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!(r"C:\{}", tail.replace('/', "\\")))
    } else {
        PathBuf::from(format!("/{tail}"))
    }
}

fn kdf() -> KdfParams {
    KdfParams::insecure_for_tests().expect("kdf")
}

fn write_raw(paths: &Paths, json: &str) {
    std::fs::create_dir_all(&paths.config_dir).expect("config dir");
    std::fs::write(paths.config_file(), json).expect("write config");
}

// ---------------------------------------------------------------------------
// Load and save
// ---------------------------------------------------------------------------

#[test]
fn a_missing_config_is_a_first_run_not_an_error() {
    let home = Home::new("firstrun");
    let store = ConfigStore::new(home.paths());
    assert!(!store.exists());
    let (config, outcome) = store.load().expect("load");
    assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
    assert!(config.jobs.is_empty());
    assert!(!outcome.migrated);
}

#[test]
fn a_config_that_is_present_but_broken_is_never_silently_replaced() {
    let home = Home::new("broken");
    let paths = home.paths();
    write_raw(&paths, "{ this is not json");
    let store = ConfigStore::new(paths.clone());

    let err = store.load().expect_err("must not pretend it is a first run");
    assert!(matches!(err, Error::Config(_)), "{err:?}");

    // The file is still exactly where the user left it, so it can be repaired.
    assert_eq!(
        std::fs::read_to_string(paths.config_file()).expect("read"),
        "{ this is not json"
    );
}

#[test]
fn save_then_load_round_trips_and_normalises() {
    let home = Home::new("roundtrip");
    let store = ConfigStore::new(home.paths());

    let provider_id = Uuid::new_v4();
    let mut config = Config::default();
    config.machine.slug = "pc-1".into();
    config.providers.push(StorageProvider {
        id: provider_id,
        name: "  StorJ  ".into(),
        kind: ProviderKind::S3 {
            endpoint: "https://gateway.storjshare.io".into(),
            region: "eu-1".into(),
            credentials: S3Credentials::for_provider(&provider_id),
            tls: true,
            path_style: false,
            flavour: S3Flavour::Storj,
        },
        notes: String::new(),
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    });
    let destination_id = Uuid::new_v4();
    config.destinations.push(Destination {
        id: destination_id,
        name: "Offsite".into(),
        kind: DestinationKind::S3 {
            provider_id,
            bucket: "backups".into(),
            // Deliberately un-normalised.
            prefix: "/superbackup//pc-1/".into(),
            credential_override: None,
        },
        encryption: Some(EncryptionSettings::default()),
        passphrase_ref: Some(SecretRef(format!("repo.passphrase:{destination_id}"))),
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    });
    config.jobs.push(Job {
        id: Uuid::new_v4(),
        name: "dev".into(),
        project_id: None,
        description: String::new(),
        sources: vec![Source::new(abs("code"))],
        destination_ids: vec![destination_id],
        schedule: Schedule::Cron { expression: "0 3 * * *".into() },
        exclusions: ExclusionSet::developer_defaults(),
        bandwidth: None,
        retention: None,
        enabled: true,
        timeout_minutes: None,
        hooks: JobHooks::default(),
        continue_on_destination_error: true,
        created_at: chrono::Utc::now(),
        tags: Vec::new(),
    });

    store.save(&config).expect("save");
    let (loaded, outcome) = store.load().expect("load");

    assert_eq!(loaded.providers[0].name, "StorJ", "names should be trimmed on save");
    match &loaded.destinations[0].kind {
        DestinationKind::S3 { prefix, .. } => assert_eq!(prefix, "superbackup/pc-1/"),
        other => panic!("unexpected kind {other:?}"),
    }
    assert_eq!(loaded.jobs.len(), 1);
    assert_eq!(loaded.schema_version, CONFIG_SCHEMA_VERSION);
    assert!(loaded.updated_at.is_some(), "save must stamp updated_at");
    assert!(!outcome.migrated);
}

#[test]
fn saving_an_invalid_config_writes_nothing() {
    let home = Home::new("invalidsave");
    let paths = home.paths();
    let store = ConfigStore::new(paths.clone());

    let mut good = Config::default();
    good.machine.slug = "pc-1".into();
    store.save(&good).expect("save the good one");
    let before = std::fs::read(paths.config_file()).expect("read");

    good.jobs.push(Job {
        id: Uuid::new_v4(),
        name: "broken".into(),
        project_id: None,
        description: String::new(),
        sources: vec![Source::new("relative")],
        destination_ids: vec![Uuid::new_v4()],
        schedule: Schedule::Manual,
        exclusions: ExclusionSet::default(),
        bandwidth: None,
        retention: None,
        enabled: true,
        timeout_minutes: None,
        hooks: JobHooks::default(),
        continue_on_destination_error: true,
        created_at: chrono::Utc::now(),
        tags: Vec::new(),
    });
    assert!(matches!(store.save(&good), Err(Error::Validation(_))));
    assert_eq!(
        std::fs::read(paths.config_file()).expect("read"),
        before,
        "a rejected save must leave the previous configuration intact"
    );
}

#[test]
fn an_oversized_config_is_refused_before_parsing() {
    let home = Home::new("huge");
    let paths = home.paths();
    std::fs::create_dir_all(&paths.config_dir).expect("dir");
    let filler = "x".repeat(1024);
    let mut giant = String::from("{\"schema_version\":1,\"pad\":\"");
    while giant.len() < (superbackup_core::config::MAX_CONFIG_BYTES as usize) + 1024 {
        giant.push_str(&filler);
    }
    giant.push_str("\"}");
    std::fs::write(paths.config_file(), &giant).expect("write");

    let err = ConfigStore::new(paths).load().expect_err("must refuse");
    assert!(format!("{err}").contains("maximum"), "{err}");
}

// ---------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------

#[test]
fn a_pre_versioning_document_is_migrated_forward() {
    // No `schema_version`, no `machine`, an un-normalised prefix, and missing
    // collections: exactly what a development build wrote.
    let provider_id = Uuid::new_v4();
    let raw = serde_json::json!({
        "destinations": [{
            "id": Uuid::new_v4(),
            "name": "Offsite",
            "kind": {
                "type": "s3",
                "provider_id": provider_id,
                "bucket": "backups",
                "prefix": "/superbackup//pc-1/../x/"
            },
            "created_at": "2024-01-01T00:00:00Z"
        }]
    });

    let (migrated, notes) = migrate(raw).expect("migrate");
    assert_eq!(migrated["schema_version"], CONFIG_SCHEMA_VERSION);
    assert!(migrated["machine"].is_object(), "a missing machine identity must be minted");
    assert!(migrated["jobs"].is_array());
    assert!(migrated["providers"].is_array());
    assert!(migrated["projects"].is_array());
    assert_eq!(migrated["destinations"][0]["kind"]["prefix"], "superbackup/pc-1/x/");
    assert!(notes.iter().any(|n| n.contains("machine identities")), "{notes:?}");
    assert!(notes.iter().any(|n| n.contains("normalised")), "{notes:?}");

    // And the migrated document actually deserialises, which is the whole
    // point — the original would not, because `machine` has no serde default.
    let config: Config = serde_json::from_value(migrated).expect("deserialise");
    assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
    assert_eq!(config.destinations.len(), 1);
}

#[test]
fn loading_migrates_and_reports_it() {
    let home = Home::new("migrate");
    let paths = home.paths();
    write_raw(&paths, r#"{"jobs":[],"destinations":[],"providers":[]}"#);

    let (config, outcome) = ConfigStore::new(paths).load().expect("load");
    assert_eq!(outcome.found_version, 0);
    assert!(outcome.migrated);
    assert!(!outcome.notes.is_empty());
    assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
}

#[test]
fn a_document_from_the_future_is_refused_and_left_alone() {
    let home = Home::new("future");
    let paths = home.paths();
    let raw = format!(r#"{{"schema_version": {}, "jobs": []}}"#, CONFIG_SCHEMA_VERSION + 1);
    write_raw(&paths, &raw);

    let err = ConfigStore::new(paths.clone()).load().expect_err("must refuse");
    let message = format!("{err}");
    assert!(message.contains("newer"), "{message}");
    assert!(message.contains("NOT been modified"), "the user must be told it is safe: {message}");
    assert_eq!(std::fs::read_to_string(paths.config_file()).expect("read"), raw);
}

#[test]
fn migration_rejects_structurally_impossible_documents() {
    assert!(migrate(serde_json::json!([])).is_err(), "a top-level array is not a config");
    assert!(migrate(serde_json::json!("hello")).is_err());
    assert!(migrate(serde_json::json!({"schema_version": "one"})).is_err());
    assert!(migrate(serde_json::json!({"jobs": {"not": "an array"}})).is_err());
}

#[test]
fn migrating_an_already_current_document_is_a_no_op() {
    let mut config = Config::default();
    config.machine.slug = "pc-1".into();
    let value = serde_json::to_value(&config).expect("serialise");
    let (migrated, notes) = migrate(value.clone()).expect("migrate");
    assert_eq!(migrated, value);
    assert!(notes.is_empty());
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

fn initialise(home: &Home, passphrase: &str) -> Store {
    Store::initialise_with(
        home.paths(),
        Vault::create_unchecked(&Secret::from_str(passphrase), kdf()).expect("vault"),
    )
    .expect("initialise")
}

#[test]
fn a_fresh_store_is_unlocked_and_a_reopened_one_is_locked() {
    let home = Home::new("store");
    {
        let mut store = initialise(&home, "master");
        assert!(!store.is_locked());
        store
            .put_secret(SecretRef("s3.access:1".into()), Secret::from_str("AKIA"))
            .expect("put");
    }

    let mut store = Store::open(home.paths()).expect("open");
    assert!(store.is_locked(), "reopening must not carry key material across a restart");
    assert!(matches!(store.secret(&SecretRef("s3.access:1".into())), Err(Error::Locked)));

    assert_eq!(store.state(), StoreState::ConfigOnly);
    assert!(!store.state().can_run_jobs(), "a locked vault cannot resolve repository keys");

    store.unlock(&Secret::from_str("master")).expect("unlock");
    assert_eq!(store.state(), StoreState::Unlocked);
    assert!(store.state().can_run_jobs());
    assert_eq!(
        store.secret(&SecretRef("s3.access:1".into())).expect("get").expect("present").expose(),
        b"AKIA"
    );

    store.lock();
    assert!(store.is_locked());
    assert_eq!(store.state(), StoreState::ConfigOnly);
}

#[test]
fn opening_without_a_vault_refuses_rather_than_creating_one() {
    let home = Home::new("novault");
    let err = Store::open(home.paths()).expect_err("must refuse");
    assert!(format!("{err}").contains("superbackup init"), "{err}");
    assert!(
        !home.paths().vault_file().exists(),
        "a failed open must never create a vault; that would destroy a recoverable one"
    );
}

#[test]
fn require_secret_names_the_missing_handle() {
    let home = Home::new("missing");
    let store = initialise(&home, "master");
    let err = store.require_secret(&SecretRef("s3.access:404".into())).expect_err("missing");
    assert!(format!("{err}").contains("s3.access:404"), "{err}");
}

#[test]
fn set_config_validates_before_it_persists() {
    let home = Home::new("setconfig");
    let mut store = initialise(&home, "master");

    let mut config = store.config().clone();
    config.jobs.push(Job {
        id: Uuid::new_v4(),
        name: "bad".into(),
        project_id: None,
        description: String::new(),
        sources: vec![Source::new("relative")],
        destination_ids: vec![],
        schedule: Schedule::Manual,
        exclusions: ExclusionSet::default(),
        bandwidth: None,
        retention: None,
        enabled: true,
        timeout_minutes: None,
        hooks: JobHooks::default(),
        continue_on_destination_error: true,
        created_at: chrono::Utc::now(),
        tags: Vec::new(),
    });
    assert!(store.set_config(config).is_err());
    assert!(store.config().jobs.is_empty(), "a rejected config must not be adopted in memory");
}

#[test]
fn staging_for_publication_puts_the_config_inside_the_ciphertext() {
    let home = Home::new("publish");
    let mut store = initialise(&home, "master");

    let mut config = store.config().clone();
    config.machine.label = "PUBLISHCANARY".into();
    store.set_config(config).expect("set");
    store.stage_for_publication().expect("stage");

    let bytes = std::fs::read(home.paths().vault_file()).expect("read");
    let text = String::from_utf8(bytes.clone()).expect("utf8");
    assert!(!text.contains("PUBLISHCANARY"), "the published config must be encrypted");

    let vault = Vault::unlock(&bytes, &Secret::from_str("master")).expect("unlock");
    assert_eq!(
        vault.embedded_config().expect("config").expect("present").machine.label,
        "PUBLISHCANARY"
    );
}

// ---------------------------------------------------------------------------
// Garbage collection
// ---------------------------------------------------------------------------

/// Build a config with one provider and one destination, and a vault holding
/// the four handles they own plus one belonging to nothing.
fn gc_fixture(home: &Home) -> (Store, Uuid, Uuid) {
    let mut store = initialise(home, "master");

    let provider_id = Uuid::new_v4();
    let destination_id = Uuid::new_v4();
    let mut config = store.config().clone();
    config.machine.slug = "pc-1".into();
    config.providers.push(StorageProvider {
        id: provider_id,
        name: "StorJ".into(),
        kind: ProviderKind::S3 {
            endpoint: "https://gateway.storjshare.io".into(),
            region: "eu-1".into(),
            credentials: S3Credentials::for_provider(&provider_id),
            tls: true,
            path_style: false,
            flavour: S3Flavour::Storj,
        },
        notes: String::new(),
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    });
    config.destinations.push(Destination {
        id: destination_id,
        name: "Offsite".into(),
        kind: DestinationKind::S3 {
            provider_id,
            bucket: "backups".into(),
            prefix: "superbackup/pc-1/".into(),
            credential_override: None,
        },
        encryption: Some(EncryptionSettings::default()),
        passphrase_ref: Some(SecretRef(format!("repo.passphrase:{destination_id}"))),
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    });
    store.set_config(config).expect("set config");

    let credentials = S3Credentials::for_provider(&provider_id);
    store.put_secret(credentials.access_key_ref.clone(), Secret::from_str("AKIA")).expect("put");
    store.put_secret(credentials.secret_key_ref.clone(), Secret::from_str("sssh")).expect("put");
    store
        .put_secret(
            SecretRef(format!("repo.passphrase:{destination_id}")),
            Secret::from_str("repo"),
        )
        .expect("put");
    (store, provider_id, destination_id)
}

#[test]
fn nothing_is_collected_while_everything_is_referenced() {
    let home = Home::new("gcclean");
    let (store, _, _) = gc_fixture(&home);
    let report = store.gc_dry_run().expect("dry run");
    assert!(report.is_empty(), "{report:?}");
    assert_eq!(report.live, 3);
    assert!(report.unrecognised.is_empty());
}

#[test]
fn deleting_a_provider_orphans_its_credentials() {
    let home = Home::new("gcprovider");
    let (mut store, provider_id, _) = gc_fixture(&home);

    let mut config = store.config().clone();
    config.providers.clear();
    config.destinations.clear();
    store.set_config(config).expect("set config");

    let dry = store.gc_dry_run().expect("dry run");
    assert_eq!(dry.orphans.len(), 3, "{dry:?}");
    assert!(dry.orphans.contains(&S3Credentials::for_provider(&provider_id).access_key_ref));
    assert_eq!(dry.live, 0);

    // The dry run really is dry.
    assert_eq!(store.vault().list_refs().expect("refs").len(), 3);

    let done = store.collect_garbage().expect("gc");
    assert_eq!(done.orphans, dry.orphans);
    assert!(store.vault().list_refs().expect("refs").is_empty());

    // And a backup was taken, because "the GC was wrong" must be survivable.
    assert!(!store.vault_file().list_backups().expect("backups").is_empty());
}

#[test]
fn gc_never_touches_handles_it_does_not_understand() {
    let home = Home::new("gcunknown");
    let (mut store, _, _) = gc_fixture(&home);

    // A handle from a future release, and one whose UUID still names a live
    // destination even though nothing references the handle itself.
    let destination_id = store.config().destinations[0].id;
    store
        .put_secret(SecretRef("some.future.thing".into()), Secret::from_str("x"))
        .expect("put");
    store
        .put_secret(
            SecretRef(format!("unknown.kind:{destination_id}")),
            Secret::from_str("y"),
        )
        .expect("put");

    let report = store.gc_dry_run().expect("dry run");
    assert!(report.orphans.is_empty(), "conservative GC must leave these alone: {report:?}");
    assert_eq!(report.unrecognised.len(), 2);

    let done = store.collect_garbage().expect("gc");
    assert!(done.is_empty());
    assert_eq!(store.vault().list_refs().expect("refs").len(), 5);
    assert!(
        store.vault_file().list_backups().expect("backups").is_empty(),
        "a no-op GC must not take a backup"
    );
}

#[test]
fn gc_requires_an_unlocked_vault() {
    let home = Home::new("gclocked");
    let (mut store, _, _) = gc_fixture(&home);
    store.lock();
    assert!(matches!(store.gc_dry_run(), Err(Error::Locked)));
    assert!(matches!(store.collect_garbage(), Err(Error::Locked)));
}

#[test]
fn plan_gc_keeps_a_remote_token_that_the_remote_still_uses() {
    let home = Home::new("gctoken");
    let (mut store, _, _) = gc_fixture(&home);

    let token_ref = SecretRef(format!("github.token:{}", Uuid::new_v4()));
    let mut config = store.config().clone();
    config.remote = Some(RemoteConfigSource {
        url: "https://github.com/me/cfg".into(),
        branch: "main".into(),
        path: "config.sbvault".into(),
        auth: RemoteAuth::Token { token_ref: token_ref.clone() },
        auto_pull: false,
        pull_interval_minutes: 60,
        allow_push: false,
        last_pull_at: None,
        last_known_commit: None,
        trusted_signers: vec![],
    });
    store.set_config(config.clone()).expect("set config");
    store.put_secret(token_ref.clone(), Secret::from_str("ghp_example")).expect("put");

    let report = plan_gc(store.config(), store.vault()).expect("plan");
    assert!(!report.orphans.contains(&token_ref), "{report:?}");

    // Detach the remote and it becomes collectable.
    config.remote = None;
    store.set_config(config).expect("set config");
    let report = plan_gc(store.config(), store.vault()).expect("plan");
    assert!(report.orphans.contains(&token_ref), "{report:?}");
}

// ---------------------------------------------------------------------------
// Repairing a configuration that does not validate
// ---------------------------------------------------------------------------

#[test]
fn a_config_that_does_not_validate_can_still_be_opened_for_repair() {
    let home = Home::new("repair");
    let paths = home.paths();
    {
        let _ = initialise(&home, "master");
    }

    // Hand-edit the file into something that parses but does not validate: a
    // job pointing at a destination that no longer exists. This happens for
    // real when a config is edited by hand or merged badly.
    let ghost = Uuid::new_v4();
    let raw = serde_json::json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "machine": serde_json::to_value(MachineIdentity::default()).expect("machine"),
        "jobs": [{
            "id": Uuid::new_v4(),
            "name": "orphaned",
            "sources": [{ "path": abs("code") }],
            "destination_ids": [ghost],
            "created_at": "2026-01-01T00:00:00Z"
        }]
    });
    write_raw(&paths, &serde_json::to_string_pretty(&raw).expect("json"));

    // The strict path refuses, so nothing schedules a run against it.
    let err = Store::open(paths.clone()).expect_err("strict open must refuse");
    assert!(matches!(err, Error::Validation(_)), "{err:?}");

    // The repair path gets in, and reports exactly what is wrong.
    let (mut store, report) = Store::open_for_repair(paths.clone()).expect("repair open");
    assert!(!report.is_ok());
    assert!(
        report.errors.iter().any(|e| e.to_string().contains(&ghost.to_string())),
        "{:#?}",
        report.errors
    );
    assert_eq!(store.config().jobs.len(), 1, "the broken document must still be readable");

    // And the fix can be saved.
    let mut fixed = store.config().clone();
    fixed.jobs[0].destination_ids.clear();
    store.set_config(fixed).expect("save the repair");
    assert!(Store::open(paths).is_ok(), "the repaired config must open strictly");
}
