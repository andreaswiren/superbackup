//! Remote configuration sync, end to end, without a network.
//!
//! [`RemoteClient`] is the only part that needs one; everything that decides
//! whether bytes are trustworthy — [`verify_pull`], [`verify_signature`],
//! [`apply_pull`] — is pure, and that is exactly the part worth testing hard.

use std::path::PathBuf;

use superbackup_core::config::{Store, StoreState};
use superbackup_core::crypto::{KdfParams, Vault};
use superbackup_core::error::Error;
use superbackup_core::model::*;
use superbackup_core::paths::Paths;
use superbackup_core::remote::{apply_pull, verify_pull, FetchedVault};
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
            std::env::temp_dir().join(format!("sb-rem-{tag}-{}-{nanos}", std::process::id()));
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

fn remote_source(trusted: Vec<String>) -> RemoteConfigSource {
    RemoteConfigSource {
        url: "https://github.com/andreas/superbackup-config".into(),
        branch: "main".into(),
        path: "config.sbvault".into(),
        auth: RemoteAuth::None,
        auto_pull: false,
        pull_interval_minutes: 60,
        allow_push: false,
        last_pull_at: None,
        last_known_commit: None,
        trusted_signers: trusted,
    }
}

fn job(name: &str, destination_ids: Vec<Uuid>) -> Job {
    Job {
        id: Uuid::new_v4(),
        name: name.into(),
        project_id: None,
        description: String::new(),
        sources: vec![Source::new(abs("code"))],
        destination_ids,
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
    }
}

fn destination(name: &str) -> Destination {
    Destination {
        id: Uuid::new_v4(),
        name: name.into(),
        kind: DestinationKind::LocalRepository { path: abs("backups/repo") },
        encryption: Some(EncryptionSettings::default()),
        passphrase_ref: Some(SecretRef("repo.passphrase:0".into())),
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

fn store(home: &Home, passphrase: &str) -> Store {
    Store::initialise_with(
        home.paths(),
        Vault::create_unchecked(&Secret::from_str(passphrase), kdf()).expect("vault"),
    )
    .expect("initialise")
}

/// What the publishing machine would upload.
fn published(passphrase: &str, config: &Config, secrets: &[(&str, &str)]) -> FetchedVault {
    let mut vault =
        Vault::create_unchecked(&Secret::from_str(passphrase), kdf()).expect("vault");
    for (handle, value) in secrets {
        vault
            .put(SecretRef((*handle).into()), Secret::from_str(value))
            .expect("put");
    }
    vault.set_embedded_config(Some(config.clone())).expect("embed");
    FetchedVault {
        bytes: vault.seal().expect("seal"),
        source_url: "https://raw.githubusercontent.com/andreas/cfg/main/config.sbvault".into(),
        sha: Some("abc123".into()),
    }
}

// ---------------------------------------------------------------------------

#[test]
fn a_full_pull_shows_a_diff_and_then_applies_it() {
    let home = Home::new("pull");
    let mut local = store(&home, "shared master");

    // The local machine has one job.
    let existing_destination = destination("Local repo");
    let existing_destination_id = existing_destination.id;
    let mut config = local.config().clone();
    config.machine.slug = "pc-1".into();
    config.destinations.push(existing_destination);
    config.jobs.push(job("dev-code", vec![existing_destination_id]));
    local.set_config(config.clone()).expect("set config");
    let local_vault_bytes_before = std::fs::read(home.paths().vault_file()).expect("read");

    // The other machine publishes the same config plus a second job and a
    // credential the local machine has never seen.
    let mut theirs = config.clone();
    theirs.jobs.push(job("documents", vec![existing_destination_id]));
    theirs.jobs[0].description = "renamed and re-described".into();
    let fetched = published("shared master", &theirs, &[("s3.access:9", "AKIAFROMTHEOTHERPC")]);

    let source = remote_source(vec![]);
    let plan = verify_pull(
        &fetched,
        local.config(),
        local.vault(),
        &source,
        &Secret::from_str("shared master"),
    )
    .expect("verify");

    assert_eq!(plan.diff.jobs.added.len(), 1);
    assert_eq!(plan.diff.jobs.added[0].name, "documents");
    assert_eq!(plan.diff.jobs.modified.len(), 1);
    assert_eq!(plan.diff.jobs.modified[0].name, "dev-code");
    assert!(plan.diff.jobs.removed.is_empty());
    assert!(plan.diff.destinations.is_empty(), "{:?}", plan.diff.destinations);
    assert_eq!(plan.secrets_added, vec!["s3.access:9".to_string()]);
    assert!(plan.secrets_removed.is_empty());
    assert!(plan.different_vault);

    // Nothing has been written yet: the user is still looking at the diff.
    assert_eq!(std::fs::read(home.paths().vault_file()).expect("read"), local_vault_bytes_before);
    assert_eq!(local.config().jobs.len(), 1);

    apply_pull(&mut local, &plan).expect("apply");

    assert_eq!(local.config().jobs.len(), 2);
    assert!(local.config().jobs.iter().any(|j| j.name == "documents"));

    // The local vault was backed up before being replaced.
    let backups = local.vault_file().list_backups().expect("backups");
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read(&backups[0]).expect("read"), local_vault_bytes_before);

    // And the new secret is there once the replaced vault is unlocked.
    assert_eq!(local.state(), StoreState::ConfigOnly, "a replaced vault arrives locked");
    local.unlock(&Secret::from_str("shared master")).expect("unlock");
    assert_eq!(local.state(), StoreState::Unlocked);
    assert_eq!(
        local
            .secret(&SecretRef("s3.access:9".into()))
            .expect("get")
            .expect("present")
            .expose(),
        b"AKIAFROMTHEOTHERPC"
    );
}

#[test]
fn a_pull_with_the_wrong_passphrase_never_reaches_the_disk() {
    let home = Home::new("pullwrong");
    let mut local = store(&home, "my master");
    let before = std::fs::read(home.paths().vault_file()).expect("read");

    let fetched = published("their master", &Config::default(), &[]);
    let err = verify_pull(
        &fetched,
        local.config(),
        local.vault(),
        &remote_source(vec![]),
        &Secret::from_str("my master"),
    )
    .expect_err("must not accept a vault it cannot open");
    assert!(matches!(err, Error::BadPassphrase));

    assert_eq!(std::fs::read(home.paths().vault_file()).expect("read"), before);
    assert!(local.vault_file().list_backups().expect("backups").is_empty());
    local.unlock(&Secret::from_str("my master")).expect("the local vault still opens");
}

#[test]
fn a_tampered_remote_vault_is_rejected_before_anything_is_written() {
    let home = Home::new("pulltampered");
    let local = store(&home, "shared");
    let before = std::fs::read(home.paths().vault_file()).expect("read");

    let mut fetched = published("shared", &Config::default(), &[("a:1", "value")]);
    // Someone with write access to the repository flips a bit.
    let mut document: serde_json::Value =
        serde_json::from_slice(&fetched.bytes).expect("json");
    let ciphertext = document["ciphertext"].as_str().expect("ct").to_string();
    let mut chars: Vec<char> = ciphertext.chars().collect();
    let middle = chars.len() / 2;
    chars[middle] = if chars[middle] == 'A' { 'B' } else { 'A' };
    document["ciphertext"] = serde_json::json!(chars.into_iter().collect::<String>());
    fetched.bytes = serde_json::to_vec(&document).expect("serialise");

    assert!(verify_pull(
        &fetched,
        local.config(),
        local.vault(),
        &remote_source(vec![]),
        &Secret::from_str("shared"),
    )
    .is_err());
    assert_eq!(std::fs::read(home.paths().vault_file()).expect("read"), before);
}

#[test]
fn garbage_served_instead_of_a_vault_is_rejected() {
    let home = Home::new("pullgarbage");
    let local = store(&home, "shared");

    for body in [
        &b"<!DOCTYPE html><html><body>404 Not Found</body></html>"[..],
        &b"version https://git-lfs.github.com/spec/v1\noid sha256:deadbeef\nsize 1\n"[..],
        &b""[..],
        &b"{}"[..],
    ] {
        let fetched = FetchedVault {
            bytes: body.to_vec(),
            source_url: "https://example.com/config.sbvault".into(),
            sha: None,
        };
        assert!(
            verify_pull(
                &fetched,
                local.config(),
                local.vault(),
                &remote_source(vec![]),
                &Secret::from_str("shared"),
            )
            .is_err(),
            "{:?} must be rejected",
            String::from_utf8_lossy(body)
        );
    }
}

#[test]
fn pinning_a_signer_fails_closed_in_a_build_that_cannot_verify() {
    // This is the point of the whole exercise: a security control that cannot
    // be evaluated must reject, not wave things through. If Ed25519 is ever
    // linked in, this test should start failing with "signature is valid" —
    // and that is the signal to update it, not to delete the pinning.
    let home = Home::new("pullsigned");
    let local = store(&home, "shared");
    let fetched = published("shared", &Config::default(), &[]);

    let err = verify_pull(
        &fetched,
        local.config(),
        local.vault(),
        &remote_source(vec!["0123456789abcdef0123456789abcdef".into()]),
        &Secret::from_str("shared"),
    )
    .expect_err("an unsigned vault must not satisfy a pinned signer list");
    assert!(format!("{err}").contains("not signed"), "{err}");

    // With no pinning configured, the same vault is accepted.
    assert!(verify_pull(
        &fetched,
        local.config(),
        local.vault(),
        &remote_source(vec![]),
        &Secret::from_str("shared"),
    )
    .is_ok());
}

#[test]
fn a_published_config_that_does_not_validate_is_refused() {
    let home = Home::new("pullinvalid");
    let mut local = store(&home, "shared");
    let before = std::fs::read(home.paths().vault_file()).expect("read");

    // A configuration referring to a destination that does not exist. The
    // publishing machine could only produce this by hand or by a bug, and
    // applying it would leave the local machine with a new vault and an
    // unusable configuration.
    let mut broken = Config::default();
    broken.jobs.push(job("dev", vec![Uuid::new_v4()]));

    let fetched = published("shared", &broken, &[]);
    let err = verify_pull(
        &fetched,
        local.config(),
        local.vault(),
        &remote_source(vec![]),
        &Secret::from_str("shared"),
    )
    .expect_err("must refuse an invalid published configuration");
    assert!(format!("{err}").contains("not valid"), "{err}");

    assert_eq!(std::fs::read(home.paths().vault_file()).expect("read"), before);
    assert_eq!(local.config().jobs.len(), 0);
    local.unlock(&Secret::from_str("shared")).expect("local vault untouched");
}

#[test]
fn a_vault_with_no_published_config_updates_only_the_secrets() {
    let home = Home::new("secretsonly");
    let mut local = store(&home, "shared");

    let destination = destination("Local repo");
    let destination_id = destination.id;
    let mut config = local.config().clone();
    config.destinations.push(destination);
    config.jobs.push(job("keep-me", vec![destination_id]));
    local.set_config(config).expect("set config");

    // The remote vault carries credentials but no configuration: the sharing
    // machine chose to share keys, not schedules.
    let mut remote_vault =
        Vault::create_unchecked(&Secret::from_str("shared"), kdf()).expect("vault");
    remote_vault
        .put(SecretRef("s3.access:1".into()), Secret::from_str("SHAREDKEY"))
        .expect("put");
    let fetched = FetchedVault {
        bytes: remote_vault.seal().expect("seal"),
        source_url: "https://example.com/config.sbvault".into(),
        sha: None,
    };

    let plan = verify_pull(
        &fetched,
        local.config(),
        local.vault(),
        &remote_source(vec![]),
        &Secret::from_str("shared"),
    )
    .expect("verify");
    assert!(plan.incoming_config.is_none());
    assert!(plan.diff.is_empty());

    apply_pull(&mut local, &plan).expect("apply");
    assert_eq!(local.config().jobs.len(), 1, "local job definitions must survive");
    assert_eq!(local.config().jobs[0].name, "keep-me");

    local.unlock(&Secret::from_str("shared")).expect("unlock");
    assert_eq!(
        local.secret(&SecretRef("s3.access:1".into())).expect("get").expect("present").expose(),
        b"SHAREDKEY"
    );
}

#[test]
fn a_pull_plan_never_renders_secret_material() {
    let home = Home::new("plandebug");
    let local = store(&home, "shared");
    let fetched = published("shared", &Config::default(), &[("s3.secret:1", "TOPSECRETCANARY")]);
    let plan = verify_pull(
        &fetched,
        local.config(),
        local.vault(),
        &remote_source(vec![]),
        &Secret::from_str("shared"),
    )
    .expect("verify");

    let rendered = format!("{plan:?}");
    assert!(!rendered.contains("TOPSECRETCANARY"), "{rendered}");
    // The plan holds the sealed bytes, which are ciphertext, and the handle
    // names, which are not secret.
    assert!(rendered.contains("s3.secret:1"));
}

#[test]
fn a_publication_payload_round_trips_to_a_second_machine() {
    // The full loop, both directions: machine A stages and publishes, machine
    // B pulls the same bytes and ends up with A's jobs and A's keys.
    let a_home = Home::new("pubA");
    let mut a = store(&a_home, "shared");
    let destination = destination("Local repo");
    let destination_id = destination.id;
    let mut config = a.config().clone();
    config.destinations.push(destination);
    config.jobs.push(job("nightly", vec![destination_id]));
    a.set_config(config).expect("set config");
    a.put_secret(SecretRef("s3.access:1".into()), Secret::from_str("AKIAFROMA"))
        .expect("put");

    let payload = a.publication_payload().expect("payload");
    let request = superbackup_core::remote::PushRequest::new(payload.clone(), "publish");
    assert!(!request.is_confirmed(), "building a payload must not authorise a push");

    let b_home = Home::new("pubB");
    let mut b = store(&b_home, "unrelated local");
    let fetched = FetchedVault {
        bytes: payload,
        source_url: "https://example.com/config.sbvault".into(),
        sha: Some("sha1".into()),
    };
    let plan = verify_pull(
        &fetched,
        b.config(),
        b.vault(),
        &remote_source(vec![]),
        &Secret::from_str("shared"),
    )
    .expect("verify");
    assert_eq!(plan.diff.jobs.added.len(), 1);
    assert_eq!(plan.diff.destinations.added.len(), 1);
    assert!(plan.incoming_warnings.iter().all(|w| !w.message.is_empty()));

    apply_pull(&mut b, &plan).expect("apply");
    b.unlock(&Secret::from_str("shared")).expect("unlock with the publisher passphrase");
    assert_eq!(b.config().jobs[0].name, "nightly");
    assert_eq!(
        b.secret(&SecretRef("s3.access:1".into())).expect("get").expect("present").expose(),
        b"AKIAFROMA"
    );
}
