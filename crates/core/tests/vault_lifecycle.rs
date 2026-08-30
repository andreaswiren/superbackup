//! The vault's state machine, passphrase rotation, backups, and the
//! deterministic repository-passphrase derivation.

use std::path::PathBuf;

use superbackup_core::crypto::{BackupReason, KdfParams, Vault, VaultFile, BACKUP_KEEP};
use superbackup_core::error::Error;
use superbackup_core::model::SecretRef;
use superbackup_core::paths::Paths;
use superbackup_core::secret::Secret;
use uuid::Uuid;

fn kdf() -> KdfParams {
    KdfParams::insecure_for_tests().expect("test kdf parameters")
}

fn pass(s: &str) -> Secret {
    Secret::from_str(s)
}

fn vault(passphrase: &str) -> Vault {
    Vault::create_unchecked(&pass(passphrase), kdf()).expect("create")
}

/// A throwaway `SUPERBACKUP_HOME`-shaped directory, removed on drop.
struct Home(PathBuf);

impl Home {
    fn new(tag: &str) -> Home {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir()
            .join(format!("sb-it-{tag}-{}-{nanos}", std::process::id()));
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

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[test]
fn a_locked_vault_cannot_be_read_from_by_accident() {
    let mut v = vault("pass");
    v.put(SecretRef("s3.access:1".into()), Secret::from_str("AKIA")).expect("put");
    v.seal().expect("seal");
    v.lock();

    // Every accessor is a `Result`, so there is no shape of this code in which
    // a caller silently gets "no such secret" from a locked vault.
    assert!(matches!(v.get(&SecretRef("s3.access:1".into())), Err(Error::Locked)));
    assert!(matches!(v.list_refs(), Err(Error::Locked)));
    assert!(matches!(v.embedded_config(), Err(Error::Locked)));
    assert!(matches!(v.signer_fingerprint(), Err(Error::Locked)));
    assert!(matches!(v.opened(), Err(Error::Locked)));

    v.unlock_in_place(&pass("pass")).expect("unlock");
    assert_eq!(
        v.get(&SecretRef("s3.access:1".into())).expect("get").expect("present").expose(),
        b"AKIA"
    );
}

#[test]
fn a_failed_unlock_leaves_the_vault_locked() {
    let mut v = vault("right");
    v.put(SecretRef("a:1".into()), Secret::from_str("x")).expect("put");
    let bytes = v.seal().expect("seal");

    let mut locked = Vault::open_locked(&bytes).expect("parse");
    assert!(locked.is_locked());
    assert!(matches!(locked.unlock_in_place(&pass("wrong")), Err(Error::BadPassphrase)));
    assert!(locked.is_locked(), "a failed unlock must not half-open the vault");
    assert!(matches!(locked.get(&SecretRef("a:1".into())), Err(Error::Locked)));

    locked.unlock_in_place(&pass("right")).expect("unlock");
    assert!(!locked.is_locked());
}

#[test]
fn the_lock_screen_can_read_the_header_without_the_passphrase() {
    let mut v = vault("pass");
    let bytes = v.seal().expect("seal");
    let locked = Vault::open_locked(&bytes).expect("parse");

    assert_eq!(locked.id(), v.id());
    assert!(locked.header().kdf.describe().contains("Argon2id"));
    assert!(locked.header().created_at <= chrono::Utc::now());
    assert!(locked.signature().is_none());
}

// ---------------------------------------------------------------------------
// Deterministic repository passphrases
// ---------------------------------------------------------------------------

#[test]
fn derived_repo_passphrases_are_stable_across_machines_that_share_the_vault() {
    let mut origin = vault("shared master passphrase");
    let destination = Uuid::from_u128(0xdeadbeef);
    let expected = origin.derive_repo_passphrase(&destination).expect("derive");
    let bytes = origin.seal().expect("seal");

    // A second machine pulls the sealed vault and opens it. Same bytes, same
    // passphrase, therefore the same repository key — with nothing transmitted
    // between the two beyond this file.
    let elsewhere = Vault::unlock(&bytes, &pass("shared master passphrase")).expect("unlock");
    let there = elsewhere.derive_repo_passphrase(&destination).expect("derive");
    assert!(expected.ct_eq(&there), "the derivation must not depend on the machine");

    // Still stable after a save/reload cycle and after unrelated edits.
    let mut edited = Vault::unlock(&bytes, &pass("shared master passphrase")).expect("unlock");
    edited.put(SecretRef("noise:1".into()), Secret::from_str("noise")).expect("put");
    let bytes = edited.seal().expect("seal");
    let reloaded = Vault::unlock(&bytes, &pass("shared master passphrase")).expect("unlock");
    assert!(expected.ct_eq(&reloaded.derive_repo_passphrase(&destination).expect("derive")));
}

#[test]
fn derived_repo_passphrases_have_the_documented_shape() {
    let v = vault("pass");
    let derived = v.derive_repo_passphrase(&Uuid::from_u128(1)).expect("derive");
    let text = derived.expose_str().expect("printable");

    assert!(text.starts_with("SB1-"), "{text}");
    assert_eq!(text.len(), 68, "{text}");
    assert!(
        text.chars().all(|c| c == '-' || c.is_ascii_uppercase() || c.is_ascii_digit()),
        "a repository passphrase reaches kopia through an environment variable and \
         must not need quoting: {text}"
    );
    for ambiguous in ['I', 'L', 'O', 'U'] {
        assert!(!text.contains(ambiguous), "{ambiguous} is easy to mis-transcribe: {text}");
    }
}

#[test]
fn generated_passphrases_are_unique_and_transcribable() {
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..64 {
        let generated = superbackup_core::crypto::generate_passphrase().expect("generate");
        let text = generated.expose_str().expect("printable").to_string();
        assert_eq!(text.len(), 68, "{text}");
        assert!(text.starts_with("SB1-"));
        assert!(seen.insert(text.clone()), "the CSPRNG repeated itself: {text}");
    }
}

#[test]
fn rotating_the_passphrase_changes_every_derived_repository_key() {
    // This is a deliberate, documented consequence: the repository passphrase
    // is derived from the master key, so re-keying the vault re-keys the
    // derivation. A caller must therefore migrate the affected repositories,
    // and this test exists so nobody "fixes" the surprise by making the
    // derivation independent of the master key — which would defeat the point.
    let mut v = vault("first");
    let destination = Uuid::from_u128(7);
    let before = v.derive_repo_passphrase(&destination).expect("before");
    v.change_passphrase(&pass("first"), &pass("second")).expect("rotate");
    let after = v.derive_repo_passphrase(&destination).expect("after");
    assert!(!before.ct_eq(&after));
}

// ---------------------------------------------------------------------------
// Rotation
// ---------------------------------------------------------------------------

#[test]
fn rotation_preserves_contents_and_identity() {
    let mut v = vault("old");
    v.put(SecretRef("s3.access:1".into()), Secret::from_str("AKIA")).expect("put");
    v.put(SecretRef("s3.secret:1".into()), Secret::from_str("sssh")).expect("put");
    let mut config = superbackup_core::model::Config::default();
    config.machine.label = "workstation".into();
    v.set_embedded_config(Some(config)).expect("embed");
    v.seal().expect("seal");

    let id = v.id();
    let created = v.header().created_at;
    let old_salt = v.header().kdf.salt.clone();

    let bytes = v.change_passphrase(&pass("old"), &pass("new")).expect("rotate");

    assert_eq!(v.id(), id, "rotation must not change the vault's identity");
    assert_eq!(v.header().created_at, created, "creation time survives rotation");
    assert_ne!(v.header().kdf.salt, old_salt, "a new passphrase must get a new salt");

    let reopened = Vault::unlock(&bytes, &pass("new")).expect("open with the new passphrase");
    assert_eq!(
        reopened.get(&SecretRef("s3.access:1".into())).expect("get").expect("present").expose(),
        b"AKIA"
    );
    assert_eq!(
        reopened.embedded_config().expect("config").expect("present").machine.label,
        "workstation"
    );
    assert!(matches!(Vault::unlock(&bytes, &pass("old")), Err(Error::BadPassphrase)));
}

#[test]
fn a_rotation_with_the_wrong_old_passphrase_changes_nothing() {
    let mut v = vault("correct");
    v.put(SecretRef("a:1".into()), Secret::from_str("value")).expect("put");
    let before = v.seal().expect("seal");
    let salt_before = v.header().kdf.salt.clone();

    let err = v.change_passphrase(&pass("guess"), &pass("attacker")).expect_err("must fail");
    assert!(matches!(err, Error::BadPassphrase));

    assert_eq!(v.sealed_bytes(), before.as_slice(), "the sealed bytes must be untouched");
    assert_eq!(v.header().kdf.salt, salt_before, "the salt must be untouched");
    assert!(!v.is_locked(), "a failed rotation must not lock the user out of an open vault");
    assert_eq!(
        v.get(&SecretRef("a:1".into())).expect("get").expect("present").expose(),
        b"value"
    );
    assert!(Vault::unlock(&before, &pass("correct")).is_ok());
    assert!(matches!(Vault::unlock(&before, &pass("attacker")), Err(Error::BadPassphrase)));
}

#[test]
fn rotation_carries_unsaved_edits_rather_than_dropping_them() {
    let mut v = vault("old");
    v.seal().expect("seal");
    // An edit that has not been sealed yet.
    v.put(SecretRef("unsaved:1".into()), Secret::from_str("keep me")).expect("put");

    let bytes = v.change_passphrase(&pass("old"), &pass("new")).expect("rotate");
    let reopened = Vault::unlock(&bytes, &pass("new")).expect("unlock");
    assert_eq!(
        reopened.get(&SecretRef("unsaved:1".into())).expect("get").expect("present").expose(),
        b"keep me",
        "a rotation must not silently discard an unsaved secret"
    );
}

#[test]
fn a_locked_vault_can_still_be_rotated_and_stays_locked() {
    let mut v = vault("old");
    v.put(SecretRef("a:1".into()), Secret::from_str("value")).expect("put");
    let bytes = v.seal().expect("seal");

    let mut locked = Vault::open_locked(&bytes).expect("parse");
    let rotated = locked.change_passphrase(&pass("old"), &pass("new")).expect("rotate");
    assert!(locked.is_locked(), "rotating must not open a vault the caller kept closed");

    let reopened = Vault::unlock(&rotated, &pass("new")).expect("unlock");
    assert_eq!(
        reopened.get(&SecretRef("a:1".into())).expect("get").expect("present").expose(),
        b"value"
    );
}

#[test]
fn rotation_can_raise_the_kdf_parameters_at_the_same_time() {
    let mut v = vault("old");
    let weak = v.header().kdf.memory_kib;

    // Below the floor: refused, and the vault is left exactly as it was.
    let too_weak = KdfParams { memory_kib: 1024, ..kdf() };
    assert!(v.change_passphrase_and_params(&pass("old"), &pass("new"), too_weak).is_err());
    assert_eq!(v.header().kdf.memory_kib, weak, "a refused rotation must not edit the header");
    assert!(Vault::unlock(v.sealed_bytes(), &pass("old")).is_ok());

    // A wrong old passphrase with acceptable parameters: also a no-op. The old
    // passphrase is verified under the *existing* parameters, so this costs
    // one cheap derivation rather than one at the new cost.
    let strong = KdfParams { memory_kib: 64 * 1024, iterations: 3, ..kdf() };
    assert!(matches!(
        v.change_passphrase_and_params(&pass("wrong"), &pass("new"), strong),
        Err(Error::BadPassphrase)
    ));
    assert_eq!(v.header().kdf.memory_kib, weak, "a failed rotation must not touch the header");
    assert!(Vault::unlock(v.sealed_bytes(), &pass("old")).is_ok());
}

// ---------------------------------------------------------------------------
// The file on disk
// ---------------------------------------------------------------------------

#[test]
fn rotation_on_disk_backs_up_first_and_the_backup_still_opens() {
    let home = Home::new("rotate");
    let paths = home.paths();
    let mut file = VaultFile::create_from(&paths, vault("old")).expect("create");
    file.vault_mut()
        .put(SecretRef("s3.access:1".into()), Secret::from_str("AKIA"))
        .expect("put");
    file.save().expect("save");

    file.change_passphrase(&pass("old"), &pass("new")).expect("rotate");

    let live = std::fs::read(file.path()).expect("read live");
    assert!(Vault::unlock(&live, &pass("new")).is_ok());

    let backups = file.list_backups().expect("backups");
    assert_eq!(backups.len(), 1);
    let backup = std::fs::read(&backups[0]).expect("read backup");
    let recovered = Vault::unlock(&backup, &pass("old")).expect("the backup must still open");
    assert_eq!(
        recovered.get(&SecretRef("s3.access:1".into())).expect("get").expect("present").expose(),
        b"AKIA",
        "the backup is worthless if it does not contain the keys"
    );
}

#[test]
fn backups_are_pruned_but_never_below_the_keep_count() {
    let home = Home::new("prune");
    let paths = home.paths();
    let file = VaultFile::create_from(&paths, vault("pass")).expect("create");
    for _ in 0..(BACKUP_KEEP * 2) {
        file.backup(BackupReason::Manual).expect("backup");
    }
    let backups = file.list_backups().expect("list");
    assert_eq!(backups.len(), BACKUP_KEEP);
    for backup in &backups {
        let bytes = std::fs::read(backup).expect("read");
        assert!(Vault::unlock(&bytes, &pass("pass")).is_ok(), "a pruned set must stay usable");
    }
}

#[test]
fn loading_from_disk_yields_a_locked_vault() {
    let home = Home::new("load");
    let paths = home.paths();
    {
        let mut file = VaultFile::create_from(&paths, vault("pass")).expect("create");
        file.vault_mut().put(SecretRef("a:1".into()), Secret::from_str("v")).expect("put");
        file.save().expect("save");
    }
    let mut file = VaultFile::load(&paths).expect("load");
    assert!(file.vault().is_locked(), "loading must never leave a vault open");
    file.vault_mut().unlock_in_place(&pass("pass")).expect("unlock");
    assert_eq!(
        file.vault().get(&SecretRef("a:1".into())).expect("get").expect("present").expose(),
        b"v"
    );
}

#[test]
fn a_corrupt_file_on_disk_is_an_error_not_a_panic() {
    let home = Home::new("corrupt");
    let paths = home.paths();
    {
        let _ = VaultFile::create_from(&paths, vault("pass")).expect("create");
    }
    // Simulate a partially written file from a filesystem that lost power
    // despite the atomic rename, or a bad sector.
    let bytes = std::fs::read(paths.vault_file()).expect("read");
    std::fs::write(paths.vault_file(), &bytes[..bytes.len() / 2]).expect("truncate");

    match VaultFile::load(&paths) {
        Err(Error::VaultCorrupt(_)) => {}
        other => panic!("expected a corruption error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Signing
// ---------------------------------------------------------------------------

#[test]
fn signing_is_unavailable_but_fails_loudly_and_leaves_the_vault_intact() {
    let mut v = vault("pass");
    v.put(SecretRef("a:1".into()), Secret::from_str("value")).expect("put");
    let unsigned = v.seal().expect("seal");

    let err = v.seal_signed().expect_err("this build cannot sign");
    assert!(format!("{err}").contains("unavailable"), "{err}");
    assert_eq!(v.sealed_bytes(), unsigned.as_slice(), "a failed signing must change nothing");
    assert!(v.signature().is_none());
    assert!(Vault::unlock(v.sealed_bytes(), &pass("pass")).is_ok());

    // The fingerprint, which needs only a hash, is available and stable.
    let a = v.signer_fingerprint().expect("fingerprint");
    let b = v.signer_fingerprint().expect("fingerprint");
    assert_eq!(a, b);
    assert_eq!(a.len(), 32);
    assert_ne!(a, vault("pass").signer_fingerprint().expect("other"));
}

#[test]
fn rotating_a_stored_key_keeps_its_label_and_its_creation_time() {
    let mut v = vault("pass");
    let handle = SecretRef("s3.access:1".into());
    v.put_labelled(handle.clone(), Secret::from_str("AKIAOLD"), Some("StorJ eu-1".into()))
        .expect("put");
    let created = {
        let open = v.opened().expect("open");
        open.entry(&handle).expect("entry").created_at()
    };

    // A plain `put` is what the "rotate this key" flow calls.
    v.put(handle.clone(), Secret::from_str("AKIANEW")).expect("rotate");

    let open = v.opened().expect("open");
    let entry = open.entry(&handle).expect("entry");
    assert_eq!(entry.secret().expose(), b"AKIANEW");
    assert_eq!(entry.label(), Some("StorJ eu-1"), "rotating a key must not erase its label");
    assert_eq!(entry.created_at(), created);
}

#[test]
fn an_empty_secret_is_refused_and_a_locked_vault_says_locked_first() {
    let mut v = vault("pass");
    assert!(matches!(
        v.put(SecretRef("a:1".into()), Secret::new(Vec::new())),
        Err(Error::Validation(_))
    ));
    v.lock();
    assert!(
        matches!(v.put(SecretRef("a:1".into()), Secret::new(Vec::new())), Err(Error::Locked)),
        "\"unlock first\" is the actionable message; the empty value is secondary"
    );
}
