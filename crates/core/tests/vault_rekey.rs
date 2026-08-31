//! The passphrase-rotation hazard: derived repository passwords.
//!
//! Rotating the master passphrase silently invalidates every repository whose
//! password is [`PassphraseSource::DerivedFromMaster`]. These tests exist to
//! make sure that cannot happen by accident, and that a rotation interrupted
//! halfway through its migration is recoverable rather than terminal.

use std::path::PathBuf;

use superbackup_core::config::Store;
use superbackup_core::crypto::{
    derived_repositories, DerivedRepository, KdfParams, MigrationState, Rekey,
    RekeyAcknowledgement, Vault, VaultFile,
};
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
            std::env::temp_dir().join(format!("sb-rekey-{tag}-{}-{nanos}", std::process::id()));
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

fn kdf() -> KdfParams {
    KdfParams::insecure_for_tests().expect("kdf")
}

fn pass(s: &str) -> Secret {
    Secret::from_str(s)
}

fn abs(tail: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!(r"C:\{}", tail.replace('/', "\\")))
    } else {
        PathBuf::from(format!("/{tail}"))
    }
}

fn destination(name: &str, source: PassphraseSource) -> Destination {
    Destination {
        id: Uuid::new_v4(),
        name: name.into(),
        kind: DestinationKind::LocalRepository { path: abs(&format!("backups/{name}")) },
        encryption: Some(EncryptionSettings { passphrase_source: source, ..Default::default() }),
        passphrase_ref: match source {
            PassphraseSource::DerivedFromMaster => None,
            _ => Some(SecretRef(format!("repo.passphrase:{name}"))),
        },
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        replicate_from: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

/// A store whose configuration has `derived` derived-passphrase repositories
/// and one that stores its own password.
fn store_with_derived(home: &Home, passphrase: &str, derived: usize) -> Store {
    let mut store = Store::initialise_with(
        home.paths(),
        Vault::create_unchecked(&pass(passphrase), kdf()).expect("vault"),
    )
    .expect("initialise");

    let mut config = store.config().clone();
    config.machine.slug = "pc-1".into();
    for n in 0..derived {
        config
            .destinations
            .push(destination(&format!("derived-{n}"), PassphraseSource::DerivedFromMaster));
    }
    config.destinations.push(destination("stored", PassphraseSource::Generated));
    store.set_config(config).expect("set config");
    store
}

// ---------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------

#[test]
fn enumeration_finds_exactly_the_repositories_a_rotation_would_break() {
    let home = Home::new("enumerate");
    let store = store_with_derived(&home, "master", 3);

    let derived = store.derived_repositories();
    assert_eq!(derived.len(), 3, "{derived:#?}");
    let mut names: Vec<&str> = derived.iter().map(|r| r.destination_name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["derived-0", "derived-1", "derived-2"]);
    assert!(
        !names.contains(&"stored"),
        "a destination with a stored password is unaffected by rotation"
    );

    // Every entry carries what a confirmation dialog needs, and no secrets.
    for repository in &derived {
        assert!(!repository.location.is_empty());
        assert!(!repository.destination_name.is_empty());
        assert!(store.config().destination(&repository.destination_id).is_some());
    }
    assert_eq!(derived, derived_repositories(store.config()));
}

#[test]
fn a_config_with_nothing_derived_enumerates_empty() {
    let home = Home::new("enumempty");
    let store = store_with_derived(&home, "master", 0);
    assert!(store.derived_repositories().is_empty());
}

// ---------------------------------------------------------------------------
// The refusal
// ---------------------------------------------------------------------------

#[test]
fn plain_rotation_is_refused_when_derived_repositories_exist() {
    let home = Home::new("refuse");
    let mut store = store_with_derived(&home, "master", 2);
    let before = std::fs::read(home.paths().vault_file()).expect("read");

    let err = store
        .change_passphrase(&pass("master"), &pass("new master"))
        .expect_err("this must not be possible by accident");

    assert!(matches!(err, Error::Validation(_)), "{err:?}");
    let message = format!("{err}");
    assert!(message.contains("derived-0"), "the user must be told which ones: {message}");
    assert!(message.contains("derived-1"), "{message}");
    assert!(
        message.contains("change_passphrase_migrating"),
        "the message must name the way forward: {message}"
    );

    // And absolutely nothing happened.
    assert_eq!(std::fs::read(home.paths().vault_file()).expect("read"), before);
    assert!(store.vault_file().list_backups().expect("backups").is_empty());
    assert!(Vault::unlock(&before, &pass("master")).is_ok());
    assert!(matches!(Vault::unlock(&before, &pass("new master")), Err(Error::BadPassphrase)));
}

#[test]
fn plain_rotation_is_allowed_when_nothing_is_derived() {
    let home = Home::new("allow");
    let mut store = store_with_derived(&home, "master", 0);
    let rekey = store.change_passphrase(&pass("master"), &pass("new master")).expect("rotate");
    assert!(rekey.repositories().is_empty());
    assert!(rekey.is_complete(), "a rotation with nothing to migrate is already complete");

    let live = std::fs::read(home.paths().vault_file()).expect("read");
    assert!(Vault::unlock(&live, &pass("new master")).is_ok());
}

/// The same guard one level down, for a caller driving `Vault` directly.
#[test]
fn the_vault_refuses_an_acknowledgement_its_own_config_contradicts() {
    let mut config = Config::default();
    let derived = destination("derived", PassphraseSource::DerivedFromMaster);
    let derived_id = derived.id;
    config.destinations.push(derived);

    let mut vault = Vault::create_unchecked(&pass("old"), kdf()).expect("vault");
    vault.set_embedded_config(Some(config)).expect("embed");
    vault.seal().expect("seal");

    let err = vault
        .change_passphrase(&pass("old"), &pass("new"), &RekeyAcknowledgement::NoDerivedRepositories)
        .expect_err("the embedded config contradicts the claim");
    assert!(format!("{err}").contains("derived"), "{err}");

    // Listing *some* of them is the subtle failure, and is refused too.
    let mut config = Config::default();
    let listed = destination("listed", PassphraseSource::DerivedFromMaster);
    let listed_id = listed.id;
    config.destinations.push(listed);
    config.destinations.push(destination("forgotten", PassphraseSource::DerivedFromMaster));
    let mut vault = Vault::create_unchecked(&pass("old"), kdf()).expect("vault");
    vault.set_embedded_config(Some(config)).expect("embed");
    vault.seal().expect("seal");

    let partial = RekeyAcknowledgement::Migrate(vec![DerivedRepository {
        destination_id: listed_id,
        destination_name: "listed".into(),
        location: "/backups".into(),
    }]);
    let err = vault
        .change_passphrase(&pass("old"), &pass("new"), &partial)
        .expect_err("an incomplete plan must be refused");
    assert!(format!("{err}").contains("forgotten"), "{err}");
    assert!(!format!("{err}").contains("listed"), "only the omissions matter: {err}");

    let _ = derived_id;
}

// ---------------------------------------------------------------------------
// Old and new derivation
// ---------------------------------------------------------------------------

#[test]
fn the_plan_derives_the_old_and_the_new_password_for_each_destination() {
    let home = Home::new("derive");
    let mut store = store_with_derived(&home, "master", 2);

    // Capture what the repositories are actually using right now, so the test
    // is checking against reality rather than against itself.
    store.unlock(&pass("master")).expect("unlock");
    let ids: Vec<Uuid> = store.derived_repositories().iter().map(|r| r.destination_id).collect();
    let before: Vec<Secret> =
        ids.iter().map(|id| store.vault().derive_repo_passphrase(id).expect("derive")).collect();

    let rekey =
        store.change_passphrase_migrating(&pass("master"), &pass("new master")).expect("rotate");
    assert_eq!(rekey.repositories().len(), 2);

    store.unlock(&pass("new master")).expect("unlock after rotation");
    for (i, id) in ids.iter().enumerate() {
        let credentials = rekey.credentials(id).expect("credentials");
        assert_eq!(&credentials.destination_id, id);

        // `old` is what the repository has now.
        assert!(
            credentials.old.ct_eq(&before[i]),
            "the old password must be the one the repository is actually using"
        );
        // `new` is what the rotated vault will derive from now on.
        let after = store.vault().derive_repo_passphrase(id).expect("derive");
        assert!(
            credentials.new.ct_eq(&after),
            "the new password must be what the vault derives after the rotation"
        );
        assert!(!credentials.old.ct_eq(&credentials.new));

        // Both are transcribable repository passwords in the documented shape.
        for secret in [&credentials.old, &credentials.new] {
            let text = secret.expose_str().expect("printable");
            assert!(text.starts_with("SB1-"), "{text}");
            assert_eq!(text.len(), 68);
        }
    }
}

#[test]
fn credentials_are_refused_for_a_destination_outside_the_plan() {
    let home = Home::new("outside");
    let mut store = store_with_derived(&home, "master", 1);
    let stored_id = store
        .config()
        .destinations
        .iter()
        .find(|d| d.name == "stored")
        .expect("stored destination")
        .id;

    let rekey = store.change_passphrase_migrating(&pass("master"), &pass("new")).expect("rotate");

    assert!(
        rekey.credentials(&stored_id).is_err(),
        "a destination with a stored password is not part of the rotation, and deriving a \
         password for it would re-password the wrong repository"
    );
    assert!(rekey.credentials(&Uuid::new_v4()).is_err());
}

// ---------------------------------------------------------------------------
// Ordering and resume
// ---------------------------------------------------------------------------

#[test]
fn the_new_vault_is_on_disk_before_the_migration_starts() {
    // The ordering that makes an interruption survivable: if the vault were
    // written last, a crash mid-migration would lose the new salt, and every
    // already-migrated repository would be on a password nothing could
    // recompute.
    let home = Home::new("ordering");
    let mut store = store_with_derived(&home, "master", 3);

    let rekey = store.change_passphrase_migrating(&pass("master"), &pass("new")).expect("rotate");

    let live = std::fs::read(home.paths().vault_file()).expect("read");
    assert_eq!(live, rekey.sealed_bytes(), "the rotated vault must already be committed");
    assert!(Vault::unlock(&live, &pass("new")).is_ok());

    // Nothing has been migrated yet, and the plan says so.
    assert_eq!(rekey.pending().count(), 3);
    assert!(!rekey.is_complete());

    // The recovery anchor exists and still holds the old key hierarchy.
    let backup = rekey.recovery_backup().expect("recovery backup");
    let backup_bytes = std::fs::read(backup).expect("read backup");
    assert!(Vault::unlock(&backup_bytes, &pass("master")).is_ok());
}

#[test]
fn a_migration_interrupted_halfway_can_be_resumed() {
    let home = Home::new("resume");
    let mut store = store_with_derived(&home, "master", 5);

    let mut rekey =
        store.change_passphrase_migrating(&pass("master"), &pass("new")).expect("rotate");
    let ids: Vec<Uuid> = rekey.repositories().iter().map(|r| r.destination_id).collect();
    let expected: Vec<(Uuid, String)> = ids
        .iter()
        .map(|id| {
            let c = rekey.credentials(id).expect("credentials");
            (*id, c.new.expose_str().expect("printable").to_string())
        })
        .collect();

    // Three of five succeed, then the process dies.
    for id in ids.iter().take(3) {
        rekey.mark_migrated(id).expect("mark");
    }
    let report = rekey.report();
    assert_eq!((report.migrated, report.pending), (3, 2));
    let backup = report.recovery_backup.clone().expect("recovery backup");
    drop(rekey);

    // A later run reopens the store and picks the migration back up from the
    // two files on disk plus the two passphrases the user still knows.
    let store = Store::open(home.paths()).expect("reopen");
    let resumed = store.resume_rekey(&backup, &pass("master"), &pass("new")).expect("resume");

    assert_eq!(resumed.repositories().len(), 5);
    assert_eq!(
        resumed.pending().count(),
        5,
        "resume cannot know what already moved, so everything is re-attempted; that is safe \
         because each step is idempotent"
    );

    // Crucially, the passwords it derives are identical to the ones the
    // interrupted run was using — otherwise the three already-migrated
    // repositories would be orphaned.
    for (id, new_password) in &expected {
        let credentials = resumed.credentials(id).expect("credentials");
        assert_eq!(
            credentials.new.expose_str().expect("printable"),
            new_password,
            "a resumed plan must derive the same new password as the interrupted one"
        );
    }
}

#[test]
fn resuming_across_two_different_vaults_is_refused() {
    let home = Home::new("resumewrong");
    let mut store = store_with_derived(&home, "master", 1);
    let rekey = store.change_passphrase_migrating(&pass("master"), &pass("new")).expect("rotate");
    let live = std::fs::read(home.paths().vault_file()).expect("read");
    let _ = rekey;

    // A backup belonging to some completely different vault. Resuming against
    // it would compute repository passwords from an unrelated key hierarchy
    // and set every repository to a password nothing can reproduce.
    let other_home = Home::new("resumeother");
    let other = VaultFile::create_from(
        &other_home.paths(),
        Vault::create_unchecked(&pass("master"), kdf()).expect("vault"),
    )
    .expect("create");
    let stranger = std::fs::read(other.path()).expect("read");

    let repositories = store.derived_repositories();
    let err = Rekey::resume(&live, &pass("new"), &stranger, &pass("master"), &repositories)
        .expect_err("must refuse");
    assert!(format!("{err}").contains("different vaults"), "{err}");
}

#[test]
fn resuming_when_no_rotation_happened_is_refused() {
    let home = Home::new("resumenoop");
    let store = store_with_derived(&home, "master", 1);
    let live = std::fs::read(home.paths().vault_file()).expect("read");

    let err = Rekey::resume(
        &live,
        &pass("master"),
        &live,
        &pass("master"),
        &store.derived_repositories(),
    )
    .expect_err("the same file twice is not a rotation");
    assert!(format!("{err}").contains("nothing to resume"), "{err}");
}

#[test]
fn a_completed_migration_reports_itself_complete() {
    let home = Home::new("complete");
    let mut store = store_with_derived(&home, "master", 2);
    let mut rekey =
        store.change_passphrase_migrating(&pass("master"), &pass("new")).expect("rotate");

    let ids: Vec<Uuid> = rekey.repositories().iter().map(|r| r.destination_id).collect();
    rekey.mark_failed(&ids[0], "kopia exited with status 1").expect("mark");
    assert!(!rekey.is_complete());
    assert_eq!(rekey.report().failed, 1);
    assert_eq!(rekey.pending().count(), 2, "a failure stays in the queue for retry");

    for id in &ids {
        rekey.mark_migrated(id).expect("mark");
    }
    assert!(rekey.is_complete());
    let report = rekey.report();
    assert_eq!((report.total, report.migrated, report.failed, report.pending), (2, 2, 0, 0));
    assert!(report.repositories.iter().all(|r| r.state == MigrationState::Migrated));
}

#[test]
fn the_report_is_serialisable_and_carries_no_secrets() {
    let home = Home::new("report");
    let mut store = store_with_derived(&home, "master", 2);
    let rekey = store.change_passphrase_migrating(&pass("master"), &pass("new")).expect("rotate");

    let id = rekey.repositories()[0].destination_id;
    let password = rekey
        .credentials(&id)
        .expect("credentials")
        .new
        .expose_str()
        .expect("printable")
        .to_string();

    let json = serde_json::to_string(&rekey.report()).expect("serialise");
    assert!(!json.contains(&password), "the persisted report must not contain a password");
    assert!(json.contains("derived-"), "but it must name the destinations");
    assert!(json.contains("recovery_backup"));
    assert!(!format!("{rekey:?}").contains(&password));
}
