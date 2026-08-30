//! End-to-end tests for the kopia driver that do **not** require kopia.
//!
//! A backup product cannot have its most dangerous layer covered only by tests
//! that are skipped on every machine without the tool installed. So these tests
//! build their own kopia: a small Rust program, compiled by `rustc` at test
//! time, which replays recorded real kopia output and records exactly how it was
//! invoked. The driver is then driven against it for the whole repository
//! lifecycle, and the recording is asserted against the flags kopia actually
//! documents.
//!
//! What that buys, specifically:
//!
//! * **argv never carries a secret** — asserted against the recorded command
//!   line of a real `repository create s3`, not against a mock.
//! * **the child environment is built from empty** — asserted by the absence of
//!   an inherited variable.
//! * **cancellation kills the process** — asserted by watching a heartbeat file
//!   the fake writes stop advancing.
//! * **a full event channel cannot deadlock the child** — asserted by flooding
//!   stderr with nobody draining the channel.
//! * **progress parses from recorded kopia output** — the `\r`-delimited frames
//!   and the final `--json --json-verbose` manifest.
//!
//! If `rustc` cannot be found the process-level tests report why and pass; the
//! pure parsing tests in `src/kopia/*` cover the rest unconditionally.

mod kopia_support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use kopia_support::{PathGuard, Scenario};
use superbackup_core::kopia::*;
use superbackup_core::model::*;
use superbackup_core::secret::Secret;

// ---------------------------------------------------------------------------
// Model fixtures
// ---------------------------------------------------------------------------

const PASSPHRASE: &str = "correct-horse-battery-staple-42";
const ACCESS_KEY: &str = "AKIAFAKEACCESSKEY01";
const SECRET_KEY: &str = "s3cr3t/Sup3r+Secret/AccessKey0123456789";

fn local_destination(path: &Path) -> Destination {
    Destination {
        id: uuid::Uuid::new_v4(),
        name: "Local disk".into(),
        kind: DestinationKind::LocalRepository { path: path.to_path_buf() },
        encryption: Some(EncryptionSettings {
            algorithm: EncryptionAlgorithm::Chacha20Poly1305HmacSha256,
            hash: HashAlgorithm::Blake3256,
            splitter: Splitter::Dynamic2mBuzhash,
            ecc: Some(EccAlgorithm::ReedSolomonCrc32),
            ecc_overhead_percent: 2,
            passphrase_source: PassphraseSource::Generated,
        }),
        passphrase_ref: None,
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

/// The tested target: StorJ's S3 gateway in `eu-1`.
fn storj_provider() -> StorageProvider {
    let id = uuid::Uuid::new_v4();
    StorageProvider {
        id,
        name: "StorJ eu-1".into(),
        kind: ProviderKind::S3 {
            endpoint: "https://gateway.storjshare.io".into(),
            region: "eu-1".into(),
            credentials: S3Credentials::for_provider(&id),
            tls: true,
            path_style: false,
            flavour: S3Flavour::Storj,
        },
        notes: String::new(),
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

fn s3_destination(provider: &StorageProvider) -> Destination {
    Destination {
        id: uuid::Uuid::new_v4(),
        name: "StorJ offsite".into(),
        kind: DestinationKind::S3 {
            provider_id: provider.id,
            bucket: "andreas-backups".into(),
            prefix: normalise_prefix("superbackup/workstation"),
            credential_override: None,
        },
        encryption: Some(EncryptionSettings::default()),
        passphrase_ref: None,
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

fn secrets() -> DestinationSecrets {
    DestinationSecrets::with_passphrase(Secret::from_str(PASSPHRASE))
        .with_s3(Secret::from_str(ACCESS_KEY), Secret::from_str(SECRET_KEY))
}

fn local_driver(s: &Scenario) -> KopiaDriver {
    let paths = s.paths();
    let dest = local_destination(&s.root.join("repo"));
    KopiaDriver::new(s.binary(), &paths, &dest, None, secrets()).expect("driver")
}

fn s3_driver(s: &Scenario) -> KopiaDriver {
    let paths = s.paths();
    let provider = storj_provider();
    let dest = s3_destination(&provider);
    KopiaDriver::new(s.binary(), &paths, &dest, Some(&provider), secrets()).expect("driver")
}

/// Real `snapshot create --json --json-verbose` output, trimmed.
const MANIFEST_JSON: &str = r#"{
  "id": "k9f3a1b2c3d4e5f60718293a4b5c6d7e",
  "source": {"host":"workstation","userName":"andreas","path":"C:\\src\\superbackup"},
  "description": "",
  "startTime": "2026-08-30T09:15:02.331Z",
  "endTime": "2026-08-30T09:17:44.902Z",
  "stats": {
    "totalSize": 6543210987, "excludedTotalSize": 918273645,
    "fileCount": 16517, "cachedFiles": 1201, "nonCachedFiles": 15316,
    "dirCount": 2204, "excludedFileCount": 40311, "excludedDirCount": 812,
    "ignoredErrorCount": 1
  },
  "rootEntry": {
    "name":"superbackup","type":"d","mode":"0755","obj":"kb1f2e3d4c5b6a798",
    "summ":{"size":6543210987,"files":16517,"dirs":2204,"numFailed":0,
            "numIgnoredErrors":1,
            "errors":[{"path":"C:\\src\\target\\lock","error":"access is denied"}]}
  }
}"#;

// ---------------------------------------------------------------------------
// Binary discovery and version handling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn probes_the_version_of_an_explicitly_configured_binary() {
    let _ = fake_or_skip!();
    let s = Scenario::new("probe");
    s.script(&[("version", "0.21.5 build: deadbeef from: kopia/kopia")]);

    let settings = Settings { kopia_path: Some(s.exe.clone()), ..Settings::default() };
    let bin = KopiaBinary::discover(&settings, &s.paths()).await.expect("discovers");
    assert_eq!(bin.version(), &KopiaVersion::new(0, 21, 5));
    assert_eq!(bin.source(), KopiaSource::Configured);
    assert!(bin.banner().contains("deadbeef"));
}

#[tokio::test]
async fn refuses_a_kopia_that_is_too_old_with_a_clear_message() {
    let _ = fake_or_skip!();
    let s = Scenario::new("old");
    s.script(&[("version", "0.9.3 build: ancient from: kopia/kopia")]);

    let settings = Settings { kopia_path: Some(s.exe.clone()), ..Settings::default() };
    let err = KopiaBinary::discover(&settings, &s.paths()).await.expect_err("must refuse");
    let msg = err.to_string();
    assert!(msg.contains("too old"), "unhelpful message: {msg}");
    assert!(msg.contains(&MINIMUM_KOPIA_VERSION.to_string()), "must name the requirement: {msg}");
}

#[tokio::test]
async fn an_explicit_path_wins_over_everything_else() {
    let _ = fake_or_skip!();
    let s = Scenario::new("explicit");
    let paths = s.paths();
    s.script(&[("version", "0.18.0 build: explicit from: kopia/kopia")]);
    // A managed build exists and is newer, and is still not used: a path the
    // user typed is an instruction, not a suggestion.
    s.install_fake_at(
        &paths.bundled_kopia(),
        &[("version", "0.22.0 build: managed from: kopia/kopia")],
    );

    let settings = Settings { kopia_path: Some(s.exe.clone()), ..Settings::default() };
    let bin = KopiaBinary::discover(&settings, &paths).await.expect("discovers");
    assert_eq!(bin.source(), KopiaSource::Configured);
    assert_eq!(bin.version(), &KopiaVersion::new(0, 18, 0));
}

#[tokio::test]
async fn a_system_kopia_is_preferred_when_the_setting_says_so() {
    let _ = fake_or_skip!();
    let s = Scenario::new("prefer-system");
    let paths = s.paths();
    s.script(&[("version", "0.19.0 build: onpath from: kopia/kopia")]);
    s.install_fake_at(
        &paths.bundled_kopia(),
        &[("version", "0.22.0 build: managed from: kopia/kopia")],
    );

    let _path = PathGuard::prepend(&s.bin_dir);
    let bin = KopiaBinary::discover(&Settings::default(), &paths).await.expect("discovers");
    assert_eq!(
        bin.source(),
        KopiaSource::SystemPath,
        "prefer_system_binary defaults to true: a kopia the user installed is the one they expect"
    );
    assert_eq!(bin.version(), &KopiaVersion::new(0, 19, 0));
}

#[tokio::test]
async fn the_managed_build_wins_when_the_user_turns_the_preference_off() {
    let _ = fake_or_skip!();
    let s = Scenario::new("prefer-managed");
    let paths = s.paths();
    s.script(&[("version", "0.19.0 build: onpath from: kopia/kopia")]);
    s.install_fake_at(
        &paths.bundled_kopia(),
        &[("version", "0.22.0 build: managed from: kopia/kopia")],
    );

    let mut settings = Settings::default();
    settings.kopia.prefer_system_binary = false;

    let _path = PathGuard::prepend(&s.bin_dir);
    let bin = KopiaBinary::discover(&settings, &paths).await.expect("discovers");
    assert_eq!(bin.source(), KopiaSource::Bundled);
    assert_eq!(bin.version(), &KopiaVersion::new(0, 22, 0));
    assert_eq!(bin.path(), paths.bundled_kopia().as_path());
}

#[tokio::test]
async fn a_system_kopia_below_the_minimum_is_skipped_for_the_managed_one() {
    let _ = fake_or_skip!();
    let s = Scenario::new("too-old-system");
    let paths = s.paths();
    s.script(&[("version", "0.17.0 build: onpath from: kopia/kopia")]);
    s.install_fake_at(
        &paths.bundled_kopia(),
        &[("version", "0.22.0 build: managed from: kopia/kopia")],
    );

    let mut settings = Settings::default();
    settings.kopia.minimum_version = "0.20.0".into();

    let _path = PathGuard::prepend(&s.bin_dir);
    let bin = KopiaBinary::discover(&settings, &paths).await.expect("discovers");
    assert_eq!(
        bin.source(),
        KopiaSource::Bundled,
        "a system kopia below the floor is stepped over, not fatal"
    );
    assert_eq!(bin.version(), &KopiaVersion::new(0, 22, 0));
}

#[tokio::test]
async fn a_configured_minimum_below_the_hard_floor_cannot_lower_it() {
    let _ = fake_or_skip!();
    let s = Scenario::new("floor");
    let paths = s.paths();
    s.install_fake_at(
        &paths.bundled_kopia(),
        &[("version", "0.12.0 build: ancient from: kopia/kopia")],
    );

    let mut settings = Settings::default();
    settings.kopia.minimum_version = "0.1.0".into();
    settings.kopia.prefer_system_binary = false;

    let _path = PathGuard::empty();
    let err = KopiaBinary::discover(&settings, &paths).await.expect_err("must refuse");
    assert!(err.to_string().contains("too old"), "{err}");
    assert_eq!(configured_floor(&settings), MINIMUM_KOPIA_VERSION);
}

#[tokio::test]
async fn a_missing_kopia_is_reported_as_missing_not_as_a_crash() {
    let s = Scenario::new("missing");
    let settings =
        Settings { kopia_path: Some(s.root.join("nowhere").join("kopia")), ..Settings::default() };
    let err = KopiaBinary::discover(&settings, &s.paths()).await.expect_err("must fail");
    assert!(
        matches!(err.code(), superbackup_core::ErrorCode::KopiaMissing)
            || err.to_string().contains("not a working kopia"),
        "unexpected error: {err}"
    );
}

// ---------------------------------------------------------------------------
// The argv rule
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_repository_passphrase_can_never_reach_argv() {
    let _ = fake_or_skip!();
    let s = Scenario::new("argv");
    let driver = s3_driver(&s);

    driver.create_repository(&RunContext::new()).await.expect("fake kopia exits 0");

    let inv = s.only();
    let line = inv.joined();
    assert!(!line.contains(PASSPHRASE), "the passphrase reached the command line: {line}");
    assert!(!line.contains(SECRET_KEY), "the S3 secret key reached the command line: {line}");
    assert!(!line.contains(ACCESS_KEY), "the S3 access key reached the command line: {line}");
    assert!(!inv.has_flag("--password"), "kopia's --password must never be used");
    assert!(!inv.has_flag("--access-key"));
    assert!(!inv.has_flag("--secret-access-key"));

    // …and it must have arrived, via the environment kopia itself binds.
    assert_eq!(inv.env.get("KOPIA_PASSWORD").map(String::as_str), Some(PASSPHRASE));
    assert_eq!(inv.env.get("AWS_ACCESS_KEY_ID").map(String::as_str), Some(ACCESS_KEY));
    assert_eq!(inv.env.get("AWS_SECRET_ACCESS_KEY").map(String::as_str), Some(SECRET_KEY));
}

#[test]
fn the_argv_audit_refuses_a_command_that_would_leak() {
    let mut cmd = KopiaCommand::new("kopia");
    cmd.command("repository")
        .command("connect")
        .secret_env("KOPIA_PASSWORD", &Secret::from_str(PASSPHRASE))
        .flag("password", PASSPHRASE);
    let err = cmd.audit_argv().expect_err("the audit must refuse this");
    assert_eq!(err.failure, KopiaFailure::Unusable);
    assert!(!format!("{err} {err:?}").contains(PASSPHRASE), "the refusal repeated the secret");
}

#[tokio::test]
async fn the_child_environment_is_built_from_empty() {
    let _ = fake_or_skip!();
    let s = Scenario::new("env");
    let driver = local_driver(&s);
    driver.connect_repository(&RunContext::new()).await.expect("ok");
    let inv = s.only();

    // Set by cargo for the test process and not on the driver's allowlist, so
    // its absence proves the environment was not inherited wholesale.
    if std::env::var_os("CARGO_MANIFEST_DIR").is_some() {
        assert!(
            !inv.env_names.contains("CARGO_MANIFEST_DIR"),
            "the child inherited the parent environment: {:?}",
            inv.env_names
        );
    }
    // But the things kopia genuinely needs are present.
    assert!(inv.env_names.contains("PATH"), "PATH must be passed through");
    // And the determinism pins are set.
    assert_eq!(inv.env.get("KOPIA_CHECK_FOR_UPDATES").map(String::as_str), Some("false"));
    assert_eq!(inv.env.get("KOPIA_BYTES_STRING_BASE_2").map(String::as_str), Some("false"));
}

// ---------------------------------------------------------------------------
// Repository lifecycle: the flags kopia actually documents
// ---------------------------------------------------------------------------

#[tokio::test]
async fn creates_a_filesystem_repository_with_the_full_encryption_settings() {
    let _ = fake_or_skip!();
    let s = Scenario::new("create-fs");
    let driver = local_driver(&s);
    driver.create_repository(&RunContext::new()).await.expect("ok");

    let inv = s.only();
    assert_eq!(inv.words(), vec!["repository", "create", "filesystem"]);
    assert_eq!(inv.flag_value("--encryption").as_deref(), Some("CHACHA20-POLY1305-HMAC-SHA256"));
    assert_eq!(inv.flag_value("--block-hash").as_deref(), Some("BLAKE3-256"));
    assert_eq!(inv.flag_value("--object-splitter").as_deref(), Some("DYNAMIC-2M-BUZHASH"));
    assert_eq!(inv.flag_value("--ecc").as_deref(), Some("REED-SOLOMON-CRC32"));
    assert_eq!(inv.flag_value("--ecc-overhead-percent").as_deref(), Some("2"));
    assert!(inv.flag_value("--path").is_some(), "filesystem storage needs --path");
    assert!(inv.flag_value("--cache-directory").is_some());
    assert_eq!(inv.flag_value("--check-for-updates").as_deref(), Some("false"));
    assert_eq!(inv.flag_value("--persist-credentials").as_deref(), Some("false"));
    // The repository directory is created before kopia is asked to use it.
    assert!(s.root.join("repo").is_dir(), "the repository directory was not prepared");
}

#[tokio::test]
async fn error_correction_is_omitted_when_its_overhead_is_zero() {
    let _ = fake_or_skip!();
    let s = Scenario::new("no-ecc");
    let paths = s.paths();
    let mut dest = local_destination(&s.root.join("repo"));
    dest.encryption = Some(EncryptionSettings { ecc: None, ..EncryptionSettings::default() });
    let driver = KopiaDriver::new(s.binary(), &paths, &dest, None, secrets()).expect("driver");
    driver.create_repository(&RunContext::new()).await.expect("ok");

    let inv = s.only();
    // Kopia ignores --ecc unless the overhead is above zero, so passing it
    // alone would be a lie to the user about what was configured.
    assert!(!inv.has_flag("--ecc"), "{}", inv.joined());
    assert!(!inv.has_flag("--ecc-overhead-percent"));
}

#[tokio::test]
async fn builds_storj_s3_arguments_the_way_kopia_expects_them() {
    let _ = fake_or_skip!();
    let s = Scenario::new("create-s3");
    let driver = s3_driver(&s);
    driver.create_repository(&RunContext::new()).await.expect("ok");

    let inv = s.only();
    assert_eq!(inv.words(), vec!["repository", "create", "s3"]);
    assert_eq!(inv.flag_value("--bucket").as_deref(), Some("andreas-backups"));
    // Bare host: kopia's --endpoint goes straight into minio-go, which rejects
    // a scheme. This is the flag most likely to be got wrong.
    assert_eq!(inv.flag_value("--endpoint").as_deref(), Some("gateway.storjshare.io"));
    assert_eq!(inv.flag_value("--region").as_deref(), Some("eu-1"));
    assert_eq!(inv.flag_value("--prefix").as_deref(), Some("superbackup/workstation/"));
    assert!(!inv.has_flag("--disable-tls"), "an https endpoint must stay on TLS");
}

#[tokio::test]
async fn a_plain_http_endpoint_disables_tls_and_nothing_else_does() {
    let _ = fake_or_skip!();
    let s = Scenario::new("minio");
    let paths = s.paths();
    let id = uuid::Uuid::new_v4();
    let provider = StorageProvider {
        id,
        name: "Local MinIO".into(),
        kind: ProviderKind::S3 {
            endpoint: "http://192.168.1.50:9000".into(),
            region: String::new(),
            credentials: S3Credentials::for_provider(&id),
            tls: true,
            path_style: true,
            flavour: S3Flavour::MinIo,
        },
        notes: String::new(),
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    };
    let mut dest = s3_destination(&provider);
    if let DestinationKind::S3 { provider_id, prefix, .. } = &mut dest.kind {
        *provider_id = provider.id;
        *prefix = String::new();
    }
    let driver =
        KopiaDriver::new(s.binary(), &paths, &dest, Some(&provider), secrets()).expect("driver");

    // Path-style addressing has no kopia flag; the driver says so rather than
    // pretending it applied the setting.
    let unsupported = driver.unsupported_options();
    assert!(
        unsupported.iter().any(|u| u.setting.contains("path-style")),
        "the unsupported path-style option must be surfaced: {unsupported:?}"
    );

    driver.connect_repository(&RunContext::new()).await.expect("ok");
    let inv = s.only();
    assert_eq!(inv.flag_value("--endpoint").as_deref(), Some("192.168.1.50:9000"));
    assert!(inv.has_flag("--disable-tls"), "http:// must select plain HTTP");
    assert!(!inv.has_flag("--region"), "an empty region must be omitted, not sent empty");
    assert!(!inv.has_flag("--prefix"));
}

#[tokio::test]
async fn connect_disconnect_and_status_use_the_documented_commands() {
    let _ = fake_or_skip!();
    let s = Scenario::new("lifecycle");
    let status_json = r#"{
      "configFile":"c:\\x.config","uniqueIDHex":"aa",
      "clientOptions":{"hostname":"workstation","username":"andreas","readonly":false},
      "storage":{"type":"s3"},
      "contentFormat":{"hash":"BLAKE2B-256-128","encryption":"AES256-GCM-HMAC-SHA256","version":3},
      "objectFormat":{"splitter":"DYNAMIC-4M-BUZHASH"}
    }"#;
    let fixture = s.stdout_file("status.json", status_json);
    s.script(&[("mode", "ok"), ("stdout_file", &fixture.display().to_string())]);

    let driver = s3_driver(&s);
    let ctx = RunContext::new();
    driver.connect_repository(&ctx).await.expect("connect");
    let status = driver.repository_status(&ctx).await.expect("status");
    driver.disconnect_repository(&ctx).await.expect("disconnect");

    let rec = s.record();
    assert_eq!(rec[0].words(), vec!["repository", "connect", "s3"]);
    assert_eq!(rec[1].words(), vec!["repository", "status"]);
    assert!(rec[1].has_flag("--json"));
    assert_eq!(rec[2].words(), vec!["repository", "disconnect"]);

    assert_eq!(status.storage_type.as_deref(), Some("s3"));
    assert_eq!(status.encryption.as_deref(), Some("AES256-GCM-HMAC-SHA256"));
    assert_eq!(status.splitter.as_deref(), Some("DYNAMIC-4M-BUZHASH"));
    assert_eq!(status.hostname.as_deref(), Some("workstation"));
}

#[tokio::test]
async fn every_invocation_is_pinned_to_this_destinations_own_config_file() {
    let _ = fake_or_skip!();
    let s = Scenario::new("isolation");
    let paths = s.paths();
    let dest = local_destination(&s.root.join("repo"));
    let expected = paths.kopia_config_for(&dest.id);
    let driver = KopiaDriver::new(s.binary(), &paths, &dest, None, secrets()).expect("driver");

    driver.connect_repository(&RunContext::new()).await.expect("ok");
    let inv = s.only();
    assert_eq!(
        inv.flag_value("--config-file").map(PathBuf::from),
        Some(expected),
        "kopia must never be allowed to fall back to the user's own repository.config"
    );
    assert!(inv.flag_value("--log-dir").is_some());
}

#[tokio::test]
async fn test_connection_treats_an_empty_location_as_success() {
    let _ = fake_or_skip!();
    let s = Scenario::new("testconn-empty");
    s.script(&[
        ("mode", "fail"),
        ("exit", "1"),
        (
            "stderr",
            "kopia: error: unable to connect to repository: unable to read format blob: BLOB not found",
        ),
    ]);
    let driver = s3_driver(&s);
    let result = driver.test_connection(&RunContext::new()).await.expect("not an error");
    assert!(!result.has_repository());
    assert!(result.summary().contains("no backup repository"), "{}", result.summary());
}

#[tokio::test]
async fn test_connection_reports_rejected_credentials_as_such() {
    let _ = fake_or_skip!();
    let s = Scenario::new("testconn-auth");
    s.script(&[
        ("mode", "fail"),
        ("exit", "1"),
        ("stderr", "kopia: error: can't connect to storage: InvalidAccessKeyId: the access key is not valid"),
    ]);
    let driver = s3_driver(&s);
    let err = driver.test_connection(&RunContext::new()).await.expect_err("must fail");
    assert_eq!(err.failure, KopiaFailure::StorageAuth);
    assert!(err.hint.unwrap_or("").contains("access key"), "{:?}", err.hint);
}

#[tokio::test]
async fn changing_the_password_passes_the_new_one_through_the_environment() {
    let _ = fake_or_skip!();
    let s = Scenario::new("chpw");
    let driver = local_driver(&s);
    let new = Secret::from_str("a-brand-new-passphrase-2026");
    driver.change_password(&new, &RunContext::new()).await.expect("ok");

    let inv = s.only();
    assert_eq!(inv.words(), vec!["repository", "change-password"]);
    assert!(!inv.joined().contains("a-brand-new-passphrase-2026"), "{}", inv.joined());
    assert!(!inv.has_flag("--new-password"));
    assert_eq!(
        inv.env.get("KOPIA_NEW_PASSWORD").map(String::as_str),
        Some("a-brand-new-passphrase-2026")
    );
    assert_eq!(inv.env.get("KOPIA_PASSWORD").map(String::as_str), Some(PASSPHRASE));
}

#[tokio::test]
async fn validate_provider_is_available_for_the_deep_compatibility_check() {
    let _ = fake_or_skip!();
    let s = Scenario::new("validate");
    s.script(&[("mode", "ok"), ("stdout", "provider is compatible")]);
    let driver = s3_driver(&s);
    let report = driver.validate_provider(&RunContext::new()).await.expect("ok");
    assert!(report.contains("compatible"));
    assert_eq!(s.only().words(), vec!["repository", "validate-provider"]);
}

#[tokio::test]
async fn throttling_is_expressed_in_the_bytes_per_second_kopia_understands() {
    let _ = fake_or_skip!();
    let s = Scenario::new("throttle");
    let driver = s3_driver(&s);
    driver.set_throttle(Some(2048), None, &RunContext::new()).await.expect("ok");

    let inv = s.only();
    assert_eq!(inv.words(), vec!["repository", "throttle", "set"]);
    assert_eq!(inv.flag_value("--upload-bytes-per-second").as_deref(), Some("2097152"));
    assert_eq!(inv.flag_value("--download-bytes-per-second").as_deref(), Some("unlimited"));
}

#[tokio::test]
async fn blob_stats_gives_the_destination_size() {
    let _ = fake_or_skip!();
    let s = Scenario::new("blobstats");
    let fixture = s.stdout_file(
        "stats.txt",
        "Count: 12043\nTotal: 88123456789\nAverage: 7318231\nHistogram:\n\n",
    );
    s.script(&[("mode", "ok"), ("stdout_file", &fixture.display().to_string())]);

    let driver = s3_driver(&s);
    let stats = driver.blob_stats(&RunContext::new()).await.expect("ok");
    assert_eq!(stats.blob_count, 12043);
    assert_eq!(stats.total_bytes, 88_123_456_789);

    let inv = s.only();
    assert_eq!(inv.words(), vec!["blob", "stats"]);
    assert!(inv.has_flag("--raw"), "without --raw the totals are rounded to one decimal");
}

// ---------------------------------------------------------------------------
// Snapshots, policy, maintenance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snapshot_create_streams_progress_and_parses_the_final_manifest() {
    let _ = fake_or_skip!();
    let s = Scenario::new("snapshot");
    let fixture = s.stdout_file("manifest.json", MANIFEST_JSON);
    s.script(&[("mode", "snapshot"), ("stdout_file", &fixture.display().to_string())]);

    let driver = local_driver(&s);
    let (sink, mut rx) = EventSink::channel(64);
    let ctx = RunContext::new().with_events(sink);
    let source = Source::new(s.root.join("src"));

    let outcome = driver
        .create_snapshot(&source, &SnapshotOptions::default(), &ctx)
        .await
        .expect("snapshot");

    // The flags that make the whole thing work.
    let inv = s.only();
    assert_eq!(inv.words(), vec!["snapshot", "create", &source.path.display().to_string()]);
    assert!(
        inv.has_flag("--progress"),
        "without --progress kopia prints nothing when stdout is a pipe"
    );
    assert!(inv.has_flag("--json"));
    assert!(inv.has_flag("--json-verbose"), "without it the manifest carries no statistics");

    // Live frames reached the consumer as the child ran.
    let mut progress_frames = Vec::new();
    let mut warnings = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        match ev {
            KopiaEvent::Progress(p) => progress_frames.push(p),
            KopiaEvent::Warning(w) => warnings.push(w),
            KopiaEvent::Log(_) => {}
        }
    }
    assert!(progress_frames.len() >= 3, "expected streaming frames, got {}", progress_frames.len());
    assert!(
        progress_frames[0].bytes_processed < progress_frames[2].bytes_processed,
        "progress must advance"
    );
    let mid = &progress_frames[1];
    assert_eq!(mid.bytes_processed, 4_500_000_000, "3.1 GB hashed + 1.4 GB cached");
    assert_eq!(mid.bytes_uploaded, 1_500_000_000, "uploaded is tracked separately");
    assert_eq!(mid.estimated_seconds_remaining, Some(65));
    assert_eq!(mid.current_path.as_deref(), Some(source.path.display().to_string().as_str()));
    assert!(warnings.iter().any(|w| w.contains("access is denied")), "{warnings:?}");

    // The manifest corrects the rounded progress numbers.
    assert_eq!(outcome.snapshot_id.as_deref(), Some("k9f3a1b2c3d4e5f60718293a4b5c6d7e"));
    assert_eq!(outcome.progress.files_processed, 16517);
    assert_eq!(outcome.progress.bytes_processed, 6_543_210_987);
    assert_eq!(outcome.progress.bytes_uploaded, 1_900_000_000);
    assert_eq!(outcome.deduplicated_bytes, 6_543_210_987 - 1_900_000_000);
    assert_eq!(outcome.progress.errors_ignored, 1);
    assert!(!outcome.incomplete);
    assert!(
        outcome.warnings.iter().any(|w| w.contains("1 file") && w.contains("skipped")),
        "{:?}",
        outcome.warnings
    );
    assert!(
        outcome.warnings.iter().any(|w| w.contains("40311")),
        "excluded counts belong in the warnings: {:?}",
        outcome.warnings
    );
}

#[tokio::test]
async fn snapshot_list_delete_and_estimate_use_the_documented_flags() {
    let _ = fake_or_skip!();
    let s = Scenario::new("snaplist");
    let list = format!("[\n {}\n]", MANIFEST_JSON);
    let fixture = s.stdout_file("list.json", &list);
    s.script(&[("mode", "ok"), ("stdout_file", &fixture.display().to_string())]);

    let driver = local_driver(&s);
    let ctx = RunContext::new();
    let snapshots = driver.list_snapshots(None, true, &ctx).await.expect("list");
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].source.host, "workstation");
    assert_eq!(snapshots[0].root_object_id(), Some("kb1f2e3d4c5b6a798"));

    driver.delete_snapshot("k9f3a1b2", true, &ctx).await.expect("delete");

    let rec = s.record();
    assert_eq!(rec[0].words(), vec!["snapshot", "list"]);
    assert!(rec[0].has_flag("--all"));
    assert!(rec[0].has_flag("--json"));
    assert!(rec[0].has_flag("--incomplete"));
    assert_eq!(rec[1].words(), vec!["snapshot", "delete", "k9f3a1b2"]);
    assert!(rec[1].has_flag("--delete"), "kopia requires --delete to actually delete");
}

#[tokio::test]
async fn snapshot_delete_without_confirmation_stays_a_dry_run() {
    let _ = fake_or_skip!();
    let s = Scenario::new("snapdel");
    let driver = local_driver(&s);
    driver.delete_snapshot("k1", false, &RunContext::new()).await.expect("ok");
    assert!(
        !s.only().has_flag("--delete"),
        "an unconfirmed delete must not carry kopia's confirmation flag"
    );
}

#[tokio::test]
async fn snapshot_estimate_is_parsed_from_kopias_prose() {
    let _ = fake_or_skip!();
    let s = Scenario::new("estimate");
    let fixture = s.stdout_file(
        "estimate.txt",
        "Snapshot includes 16517 file(s), total size 6.5 GB\n\
         Snapshot excludes 40311 file(s), total size 918.3 MB\n\
         Snapshot excludes 812 directories. Examples:\n - x\n\
         \nEstimated upload time: 1h27m6s at 10 Mbit/s\n",
    );
    s.script(&[("mode", "ok"), ("stdout_file", &fixture.display().to_string())]);

    let driver = local_driver(&s);
    let est = driver
        .estimate_snapshot(&s.root.join("src"), &RunContext::new())
        .await
        .expect("estimate");
    assert_eq!(est.included_files, 16517);
    assert_eq!(est.included_bytes, 6_500_000_000);
    assert_eq!(est.excluded_files, 40311);
    assert_eq!(est.excluded_directories, 812);
    assert!(s.only().has_flag("--quiet"), "one stderr line per directory is not useful");
}

#[tokio::test]
async fn restore_defaults_are_safe_and_the_flags_are_the_real_ones() {
    let _ = fake_or_skip!();
    let s = Scenario::new("restore");
    let driver = local_driver(&s);
    let target = s.root.join("restored");
    driver
        .restore("k9f3a1b2/src/main.rs", &target, &RestoreOptions::default(), &RunContext::new())
        .await
        .expect("restore");

    let inv = s.only();
    assert_eq!(
        inv.words(),
        vec!["restore", "k9f3a1b2/src/main.rs", &target.display().to_string()]
    );
    assert_eq!(inv.flag_value("--overwrite-files").as_deref(), Some("false"));
    assert_eq!(inv.flag_value("--overwrite-directories").as_deref(), Some("false"));
    assert_eq!(inv.flag_value("--write-files-atomically").as_deref(), Some("true"));
    assert!(!inv.has_flag("--delete-extra"), "a restore must never delete by default");
    assert!(inv.has_flag("--progress"));
}

#[tokio::test]
async fn restore_progress_is_streamed_too() {
    let _ = fake_or_skip!();
    let s = Scenario::new("restore-progress");
    let driver = local_driver(&s);

    // Drive the tracker directly with a recorded restore frame: the renderer is
    // different from the upload one and both must reach `Progress`.
    let mut tracker = ProgressTracker::new();
    assert!(tracker.feed(
        "Processed 812 (1.9 GB) of 4021 (6.5 GB), skipped 3 (1 KB), ignored 2 errors 41.2 MB/s (29.2%) remaining 1m50s."
    ));
    assert_eq!(tracker.progress().files_total, Some(4021));
    assert_eq!(tracker.progress().bytes_processed, 1_900_000_000);
    assert_eq!(tracker.progress().errors_ignored, 2);

    // And the command itself still runs cleanly against the fake.
    driver
        .restore("k1", &s.root.join("out"), &RestoreOptions::default(), &RunContext::new())
        .await
        .expect("restore");
}

#[tokio::test]
async fn browsing_a_snapshot_directory_yields_structured_entries() {
    let _ = fake_or_skip!();
    let s = Scenario::new("browse");
    let dir_json = r#"{"stream":"kopia:directory","entries":[
        {"name":"zeta.txt","type":"f","size":774,"mtime":"2026-08-30T21:17:00Z","obj":"Ic1d2"},
        {"name":"crates","type":"d","mtime":"2026-08-30T21:16:00Z","obj":"kf00d",
         "summ":{"size":123,"files":9,"dirs":3,"numFailed":0}},
        {"name":"Alpha.md","type":"f","size":10,"obj":"Iaaa"}
      ],"summary":{"size":907,"files":10,"dirs":4,"numFailed":0}}"#;
    let fixture = s.stdout_file("dir.json", dir_json);
    s.script(&[("mode", "ok"), ("stdout_file", &fixture.display().to_string())]);

    let driver = local_driver(&s);
    let entries =
        driver.list_directory("k9f3a1b2/crates", &RunContext::new()).await.expect("browse");

    assert_eq!(s.only().words(), vec!["show", "k9f3a1b2/crates"]);
    // Directories first, then case-insensitive by name.
    assert_eq!(
        entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        vec!["crates", "Alpha.md", "zeta.txt"]
    );
    assert!(entries[0].entry_type.is_dir());
    assert_eq!(entries[0].object_id, "kf00d");
    assert_eq!(entries[2].size, 774);
}

#[tokio::test]
async fn browsing_a_file_fails_with_a_message_a_human_can_read() {
    let _ = fake_or_skip!();
    let s = Scenario::new("browse-file");
    let fixture = s.stdout_file("blob.bin", "this is a text file, not a directory manifest");
    s.script(&[("mode", "ok"), ("stdout_file", &fixture.display().to_string())]);

    let driver = local_driver(&s);
    let err = driver.list_directory("k1/readme.txt", &RunContext::new()).await.expect_err("fails");
    assert!(err.message.contains("only directories"), "{}", err.message);
}

#[tokio::test]
async fn applying_a_policy_clears_stale_ignores_in_a_separate_pass() {
    let _ = fake_or_skip!();
    let s = Scenario::new("policy");
    let driver = local_driver(&s);
    let source = Source { one_filesystem: true, ..Source::new(s.root.join("src")) };
    let exclusions = ExclusionSet {
        presets: vec![ExclusionPreset::NodeModules],
        patterns: vec!["*.iso".into()],
        use_gitignore: true,
        max_file_size_mb: Some(2048),
        respect_cachedir_tag: true,
    };
    let retention = RetentionPolicy {
        keep_latest: 7,
        keep_hourly: 12,
        keep_daily: 30,
        keep_weekly: 4,
        keep_monthly: 6,
        keep_annual: 2,
        maintenance_every_n_runs: 20,
    };

    driver
        .apply_source_policy(&source, &retention, &exclusions, &RunContext::new())
        .await
        .expect("policy");

    let rec = s.record();
    assert_eq!(rec.len(), 2, "clearing and setting must be separate invocations: {rec:#?}");

    // Pass 1 clears only. Kopia's applyPolicyStringList returns early on
    // --clear-ignore, so combining the two would silently discard every rule.
    assert!(rec[0].has_flag("--clear-ignore"));
    assert!(rec[0].has_flag("--clear-dot-ignore"));
    assert!(!rec[0].has_flag("--add-ignore"), "a combined pass would wipe the ignore list");

    // Pass 2 sets everything.
    let set = &rec[1];
    assert!(!set.has_flag("--clear-ignore"));
    assert_eq!(set.flag_value("--keep-latest").as_deref(), Some("7"));
    assert_eq!(set.flag_value("--keep-daily").as_deref(), Some("30"));
    assert_eq!(set.flag_value("--keep-annual").as_deref(), Some("2"));
    let ignores = set.flag_values("--add-ignore");
    assert_eq!(ignores, exclusions.effective_patterns());
    assert!(ignores.iter().any(|p| p.contains("node_modules")), "{ignores:?}");
    assert!(ignores.iter().any(|p| p == "*.iso"));
    assert_eq!(set.flag_value("--add-dot-ignore").as_deref(), Some(".gitignore"));
    assert_eq!(set.flag_value("--ignore-cache-dirs").as_deref(), Some("true"));
    assert_eq!(set.flag_value("--one-file-system").as_deref(), Some("true"));
    // Kopia parses --max-file-size with ParseInt: plain bytes, no unit suffix.
    assert_eq!(set.flag_value("--max-file-size").as_deref(), Some("2147483648"));
}

#[tokio::test]
async fn policy_show_reads_the_stored_policy_back() {
    let _ = fake_or_skip!();
    let s = Scenario::new("policy-show");
    let fixture = s.stdout_file(
        "policy.json",
        r#"{"retention":{"keepDaily":30},"files":{"ignore":["node_modules/"]}}"#,
    );
    s.script(&[("mode", "ok"), ("stdout_file", &fixture.display().to_string())]);

    let driver = local_driver(&s);
    let policy =
        driver.show_policy(Some(&s.root.join("src")), &RunContext::new()).await.expect("show");
    assert_eq!(policy.keep_daily, Some(30));
    assert_eq!(policy.ignore_rules, vec!["node_modules/"]);
    assert!(s.only().has_flag("--json"));
}

#[tokio::test]
async fn maintenance_runs_quick_and_full() {
    let _ = fake_or_skip!();
    let s = Scenario::new("maintenance");
    let driver = local_driver(&s);
    let ctx = RunContext::new();

    driver.run_maintenance(MaintenanceMode::Quick, &ctx).await.expect("quick");
    driver.run_maintenance(MaintenanceMode::Full, &ctx).await.expect("full");
    driver
        .configure_maintenance(
            &MaintenanceSettings {
                owner: Some("andreas@workstation".into()),
                enable_quick: Some(true),
                enable_full: Some(true),
                quick_interval: Some(Duration::from_secs(3600)),
                full_interval: Some(Duration::from_secs(24 * 3600)),
            },
            &ctx,
        )
        .await
        .expect("set");

    let rec = s.record();
    assert_eq!(rec[0].words(), vec!["maintenance", "run"]);
    assert!(!rec[0].has_flag("--full"));
    assert!(rec[1].has_flag("--full"));
    assert_eq!(rec[2].words(), vec!["maintenance", "set"]);
    assert_eq!(rec[2].flag_value("--owner").as_deref(), Some("andreas@workstation"));
    assert_eq!(rec[2].flag_value("--quick-interval").as_deref(), Some("1h"));
    assert_eq!(rec[2].flag_value("--full-interval").as_deref(), Some("24h"));
    assert_eq!(rec[2].flag_value("--enable-full").as_deref(), Some("true"));
}

// ---------------------------------------------------------------------------
// Cancellation, timeouts, and the pipe deadlock
// ---------------------------------------------------------------------------

/// Wait until `path` exists, or give up.
async fn wait_for(path: &Path, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

fn size_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[tokio::test]
async fn cancelling_kills_the_child_promptly_and_leaves_nothing_running() {
    let _ = fake_or_skip!();
    let s = Scenario::new("cancel");
    s.script(&[("mode", "hang")]);
    let driver = local_driver(&s);
    let heartbeat = s.heartbeat();

    let (handle, token) = cancellation();
    let ctx = RunContext::new().with_cancel(token);
    let d = driver.clone();
    let task = tokio::spawn(async move { d.create_repository(&ctx).await });

    assert!(wait_for(&heartbeat, Duration::from_secs(20)).await, "the fake kopia never started");
    handle.cancel();

    let result = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("cancellation must not hang")
        .expect("task must not panic");
    let err = result.expect_err("a cancelled command must not report success");
    assert_eq!(err.failure, KopiaFailure::Cancelled);

    // The heartbeat stopping is the proof that the process is actually gone
    // rather than merely detached from us.
    let after_kill = size_of(&heartbeat);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        size_of(&heartbeat),
        after_kill,
        "the child was still running after cancellation returned"
    );
}

#[tokio::test]
async fn a_timeout_stops_the_child_and_says_so() {
    let _ = fake_or_skip!();
    let s = Scenario::new("timeout");
    s.script(&[("mode", "hang")]);
    let driver = local_driver(&s);
    let heartbeat = s.heartbeat();

    let ctx = RunContext::new().with_timeout(Duration::from_millis(700));
    let err = tokio::time::timeout(Duration::from_secs(20), driver.create_repository(&ctx))
        .await
        .expect("the timeout must fire on its own")
        .expect_err("a timed-out command must not report success");
    assert_eq!(err.failure, KopiaFailure::Timeout);
    assert!(err.hint.is_some(), "a timeout must tell the user what to do");

    assert!(heartbeat.exists(), "the fake should have run at all");
    let after = size_of(&heartbeat);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(size_of(&heartbeat), after, "the child survived its timeout");
}

#[tokio::test]
async fn a_stalled_consumer_cannot_deadlock_the_child() {
    let _ = fake_or_skip!();
    let s = Scenario::new("flood");
    s.script(&[("mode", "flood")]);
    let driver = local_driver(&s);

    // Capacity 1, and the receiver is never polled: if the stderr pump awaited
    // on send, kopia's pipe would fill and neither side would ever move again.
    let (sink, _never_drained) = EventSink::channel(1);
    let ctx = RunContext::new().with_events(sink);

    tokio::time::timeout(Duration::from_secs(60), driver.create_repository(&ctx))
        .await
        .expect("a stalled progress consumer must not deadlock the child")
        .expect("the command itself should still succeed");
}

#[tokio::test]
async fn a_command_cancelled_before_it_starts_never_launches_kopia() {
    let _ = fake_or_skip!();
    let s = Scenario::new("precancel");
    s.script(&[("mode", "hang")]);
    let driver = local_driver(&s);

    let (handle, token) = cancellation();
    handle.cancel();
    let err = driver
        .create_repository(&RunContext::new().with_cancel(token))
        .await
        .expect_err("must not run");
    assert_eq!(err.failure, KopiaFailure::Cancelled);
    assert!(s.record().is_empty(), "kopia must not have been launched at all");
}

// ---------------------------------------------------------------------------
// Error translation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kopias_real_failure_modes_all_map_to_actionable_errors() {
    let _ = fake_or_skip!();

    // Left: verbatim kopia stderr. Right: what the user must be told.
    let cases: &[(&str, KopiaFailure)] = &[
        (
            "kopia: error: unable to open repository: invalid repository password",
            KopiaFailure::WrongPassword,
        ),
        (
            "kopia: error: unable to connect to repository: unable to read format blob: BLOB not found",
            KopiaFailure::RepositoryNotFound,
        ),
        (
            "kopia: error: unable to get repository storage: found existing data in storage location",
            KopiaFailure::RepositoryExists,
        ),
        (
            "kopia: error: repository is not connected. See https://kopia.io/docs/repositories/",
            KopiaFailure::NotConnected,
        ),
        (
            "kopia: error: can't connect to storage: SignatureDoesNotMatch: The request signature we calculated does not match",
            KopiaFailure::StorageAuth,
        ),
        (
            "kopia: error: can't connect to storage: NoSuchBucket: the specified bucket does not exist",
            KopiaFailure::BucketNotFound,
        ),
        (
            "kopia: error: can't connect to storage: Get \"https://gateway.storjshare.io\": dial tcp: lookup gateway.storjshare.io: no such host",
            KopiaFailure::StorageUnreachable,
        ),
        ("kopia: error: repository upgrade in progress", KopiaFailure::Locked),
        (
            "kopia: error: error writing blob: write /mnt/backup/p1.f: no space left on device",
            KopiaFailure::DiskFull,
        ),
        (
            "kopia: error: unable to read directory: open /root/private: permission denied",
            KopiaFailure::PermissionDenied,
        ),
        (
            "kopia: error: unable to open file: The process cannot access the file because it is being used by another process.",
            KopiaFailure::Locked,
        ),
    ];

    for (stderr, expected) in cases {
        let s = Scenario::new("errors");
        s.script(&[("mode", "fail"), ("exit", "1"), ("stderr", stderr)]);
        let driver = s3_driver(&s);
        let err = driver.connect_repository(&RunContext::new()).await.expect_err("must fail");
        assert_eq!(err.failure, *expected, "misclassified: {stderr}");
        assert!(!err.message.is_empty());
        assert!(
            !err.message.starts_with("kopia:"),
            "the headline must be our words, not a stderr dump: {}",
            err.message
        );
        // Everything except a plain cancellation deserves a next step.
        assert!(err.hint.is_some(), "no hint for {expected:?}");
        // The raw text is still available for the details disclosure.
        assert!(err.detail.is_some());
        // And it converts into the crate error type with a stable code.
        let converted: superbackup_core::Error = err.clone().into();
        assert_eq!(converted.code(), err.failure.error_code());
    }
}

#[tokio::test]
async fn a_credential_echoed_back_by_the_provider_is_redacted_before_it_is_stored() {
    let _ = fake_or_skip!();
    let s = Scenario::new("redact");
    s.script(&[
        ("mode", "fail"),
        ("exit", "1"),
        (
            "stderr",
            "kopia: error: can't connect to storage: request failed for https://andreas:s3cr3t-Sup3r-Secret@gateway.storjshare.io/andreas-backups",
        ),
    ]);
    let driver = s3_driver(&s);
    let err = driver.connect_repository(&RunContext::new()).await.expect_err("must fail");
    let everything = format!("{err} {err:?} {:?}", err.to_run_error());
    assert!(!everything.contains("s3cr3t-Sup3r-Secret"), "credential survived: {everything}");
    assert!(everything.contains("gateway.storjshare.io"), "the useful part was destroyed");
}

#[tokio::test]
async fn an_unclassifiable_failure_still_shows_kopias_own_words() {
    let _ = fake_or_skip!();
    let s = Scenario::new("unknown");
    s.script(&[
        ("mode", "fail"),
        ("exit", "3"),
        ("stderr", "kopia: error: something nobody has seen before"),
    ]);
    let driver = local_driver(&s);
    let err = driver.connect_repository(&RunContext::new()).await.expect_err("must fail");
    assert_eq!(err.failure, KopiaFailure::Unknown);
    assert_eq!(err.status, Some(3));
    assert!(err.detail.unwrap_or_default().contains("nobody has seen before"));
}

#[tokio::test]
async fn a_missing_passphrase_is_refused_before_kopia_is_launched() {
    let _ = fake_or_skip!();
    let s = Scenario::new("nopass");
    let paths = s.paths();
    let dest = local_destination(&s.root.join("repo"));
    let driver = KopiaDriver::new(s.binary(), &paths, &dest, None, DestinationSecrets::default())
        .expect("driver");

    let err = driver.create_repository(&RunContext::new()).await.expect_err("must refuse");
    assert_eq!(err.failure, KopiaFailure::WrongPassword);
    assert!(err.message.contains("Unlock the vault"), "{}", err.message);
    assert!(s.record().is_empty(), "kopia must not be launched without a passphrase");
}

#[test]
fn a_mirror_destination_is_refused_at_construction() {
    let s = Scenario::new("mirror");
    let paths = s.paths();
    let dest = Destination {
        kind: DestinationKind::LocalMirror { path: s.root.join("mirror") },
        ..local_destination(&s.root.join("repo"))
    };
    let err = KopiaDriver::new(s.binary(), &paths, &dest, None, secrets()).expect_err("refuses");
    assert!(err.message.contains("folder mirror"), "{}", err.message);
}
