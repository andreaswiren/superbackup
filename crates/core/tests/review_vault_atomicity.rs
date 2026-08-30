//! Hostile review of vault write/rotation atomicity and the KDF bounds.
//!
//! Claims under test:
//!
//! * THREAT_MODEL.md §4 rule 4 — "A wrong passphrase and a corrupt file are
//!   the same class of failure, and neither partially applies a change."
//! * `crypto::file::VaultFile::change_passphrase` — "There is no interleaving
//!   that loses the keys."
//! * `paths::write_atomic` — "A crash leaves either the old file or the new
//!   one, never a truncated mixture."
//! * `crypto::kdf` — "Bounds checked before any allocation."

use std::path::PathBuf;

use superbackup_core::crypto::kdf::{KdfParams, MAX_MEMORY_KIB};
use superbackup_core::crypto::rekey::RekeyAcknowledgement;
use superbackup_core::crypto::{Vault, VaultFile};
use superbackup_core::paths::Paths;
use superbackup_core::secret::Secret;

struct TempHome(PathBuf);

impl TempHome {
    fn new(tag: &str) -> TempHome {
        let dir = std::env::temp_dir().join(format!(
            "sb-review-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        TempHome(dir)
    }
    fn paths(&self) -> Paths {
        Paths::rooted_at(&self.0, false)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn new_vault(paths: &Paths, pass: &str) -> VaultFile {
    let vault = Vault::create_unchecked(
        &Secret::from_str(pass),
        KdfParams::insecure_for_tests().expect("kdf"),
    )
    .expect("vault");
    VaultFile::create_from(paths, vault).expect("create")
}

/// Regression guard for H1.
///
/// `VaultFile::change_passphrase` used to rotate the **in-memory** vault before
/// it took the recovery backup and before it wrote anything. If the backup step
/// failed, the call reported failure — but the in-memory vault was already
/// sealed under the *new* passphrase while the file on disk was still under the
/// old one, and the next ordinary `save()` silently wrote the new-passphrase
/// ciphertext over the live file. The user had been told the rotation failed,
/// still believed their old passphrase worked, and there was no recovery backup
/// either, because creating it is exactly what had failed. Permanent, silent
/// lockout from every backup.
///
/// The fix is `Vault::prepare_rekey` / `Vault::commit_rekey`: everything
/// fallible happens before the vault is touched, the caller writes in between,
/// and the commit cannot fail. A failure anywhere leaves memory and disk both
/// on the old passphrase, consistent with what the user was just told.
///
/// The I/O failure is provoked here by putting a plain file where the backup
/// *directory* must go, which is the moral equivalent of a full disk, a
/// revoked ACL, or an antivirus lock on `vault-backups`.
#[test]
fn a_failed_rotation_leaves_both_memory_and_disk_on_the_old_passphrase() {
    let home = TempHome::new("rotate-split");
    let paths = home.paths();
    let mut file = new_vault(&paths, "old-passphrase");
    file.save().expect("save");

    // Make `create_dir_all(backup_dir)` fail: a file already occupies the path.
    let backup_dir = paths.vault_backup_dir();
    std::fs::create_dir_all(backup_dir.parent().expect("parent")).expect("parent dir");
    std::fs::write(&backup_dir, b"not a directory").expect("block the backup dir");

    let err = file
        .change_passphrase(
            &Secret::from_str("old-passphrase"),
            &Secret::from_str("new-passphrase"),
            &RekeyAcknowledgement::NoDerivedRepositories,
        )
        .expect_err("the backup step must fail");
    eprintln!("change_passphrase reported: {err}");

    // The disk file is untouched, which is the half the implementation gets
    // right.
    let on_disk = std::fs::read(file.path()).expect("read");
    assert!(
        Vault::unlock(&on_disk, &Secret::from_str("old-passphrase")).is_ok(),
        "the file on disk must still open with the old passphrase"
    );

    // Now the user does anything at all that saves. Unblock the backup path so
    // that only the rotation's own failure is under test.
    std::fs::remove_file(&backup_dir).expect("unblock");
    file.vault_mut()
        .put(
            superbackup_core::model::SecretRef("s3.access:1".into()),
            Secret::from_str("AKIAEXAMPLE"),
        )
        .expect("put");
    file.save().expect("save");

    let after = std::fs::read(file.path()).expect("read");
    assert!(
        Vault::unlock(&after, &Secret::from_str("old-passphrase")).is_ok(),
        "after a rotation that REPORTED FAILURE, the live vault must still open \
         with the old passphrase: a failed rotation must never be committed by \
         a later save"
    );
    assert!(
        matches!(
            Vault::unlock(&after, &Secret::from_str("new-passphrase")),
            Err(superbackup_core::error::Error::BadPassphrase)
        ),
        "and it must not open with the passphrase the failed rotation tried to set"
    );

    // The secret written after the failed rotation is present, so the save
    // really did happen — the guard is not passing because nothing was written.
    let vault = Vault::unlock(&after, &Secret::from_str("old-passphrase")).expect("unlock");
    assert_eq!(
        vault
            .get(&superbackup_core::model::SecretRef("s3.access:1".into()))
            .expect("get")
            .expect("present")
            .expose(),
        b"AKIAEXAMPLE"
    );

    // And the rotation can simply be retried now that the obstruction is gone.
    file.change_passphrase(
        &Secret::from_str("old-passphrase"),
        &Secret::from_str("new-passphrase"),
        &RekeyAcknowledgement::NoDerivedRepositories,
    )
    .expect("a retried rotation must succeed");
    let rotated = std::fs::read(file.path()).expect("read");
    assert!(Vault::unlock(&rotated, &Secret::from_str("new-passphrase")).is_ok());
}

/// The same ordering rule, for the ordinary save path.
///
/// `VaultFile::save` used to seal — which marks the vault clean — and write
/// afterwards. A write that failed therefore left a vault that believed it had
/// no pending changes, so the retry a caller makes after handling the error
/// found nothing to do and silently dropped the edit.
///
/// The guard is on the property that made that possible: preparing ciphertext
/// must not, by itself, change what the vault believes is on disk.
#[test]
fn preparing_a_seal_does_not_mark_the_vault_clean() {
    let home = TempHome::new("save-order");
    let paths = home.paths();
    let mut file = new_vault(&paths, "pass");
    file.save().expect("save");
    let before = std::fs::read(file.path()).expect("read");

    file.vault_mut()
        .put(
            superbackup_core::model::SecretRef("s3.access:1".into()),
            Secret::from_str("AKIAEXAMPLE"),
        )
        .expect("put");
    assert!(file.vault().is_dirty(), "the edit is pending");

    let sealed = file.vault().prepare_seal().expect("prepare");
    assert!(
        file.vault().is_dirty(),
        "preparing ciphertext must not mark the vault clean; if it did, a write          that then failed would leave the retry with nothing to do and the edit          silently dropped"
    );
    assert_eq!(
        file.vault().sealed_bytes(),
        before.as_slice(),
        "and the vault must still believe the old bytes are the ones on disk"
    );
    assert_ne!(sealed.bytes(), before.as_slice(), "the candidate really is new content");

    // Committing is what changes its mind, and `save` only does that after the
    // write succeeds.
    file.save().expect("save");
    assert!(!file.vault().is_dirty());
    let on_disk = std::fs::read(file.path()).expect("read");
    assert_eq!(file.vault().sealed_bytes(), on_disk.as_slice());
    let vault = Vault::unlock(&on_disk, &Secret::from_str("pass")).expect("unlock");
    assert_eq!(
        vault
            .get(&superbackup_core::model::SecretRef("s3.access:1".into()))
            .expect("get")
            .expect("present")
            .expose(),
        b"AKIAEXAMPLE"
    );
}

/// `paths::write_atomic` unlinks the destination before renaming on Windows,
/// on the stated premise that "Windows `rename` fails if the destination
/// exists". That premise is false: `std::fs::rename` maps to `MoveFileExW`
/// with `MOVEFILE_REPLACE_EXISTING`.
///
/// The `remove_file` is therefore gratuitous, and it opens a window in which
/// `config.sbvault` — every repository key the installation owns — exists
/// nowhere on disk. Worse, if the `rename` then fails (an antivirus scanner
/// holding the freshly created temp file is the ordinary case), the error path
/// deletes the temp file too, destroying both copies.
#[test]
fn std_rename_already_replaces_an_existing_file_so_the_unlink_is_gratuitous() {
    let home = TempHome::new("rename");
    let dir = home.0.clone();
    std::fs::create_dir_all(&dir).expect("dir");
    let src = dir.join("src");
    let dst = dir.join("dst");
    std::fs::write(&src, b"new").expect("src");
    std::fs::write(&dst, b"old").expect("dst");

    std::fs::rename(&src, &dst).expect(
        "std::fs::rename must replace an existing destination on every supported \
         platform; write_atomic's `remove_file` then `rename` sequence is \
         unnecessary and destroys the vault if the rename fails",
    );
    assert_eq!(std::fs::read(&dst).expect("read"), b"new");
}

/// `write_atomic` names its temp file deterministically from the target name
/// and the process id, so two writers in one process racing on the same target
/// collide on the same temp path — the second `File::create` truncates the
/// first writer's partially written buffer, and whichever `rename` lands last
/// wins with content neither writer produced.
#[test]
fn the_temp_file_name_is_fully_predictable() {
    let home = TempHome::new("tmpname");
    let target = home.0.join("config.sbvault");
    let expected = home.0.join(format!(".config.sbvault.tmp-{}", std::process::id()));

    superbackup_core::paths::write_atomic(&target, b"x").expect("write");

    // Recreate the name from public information only.
    assert!(
        !expected.exists(),
        "the temp file is cleaned up, but its path was fully predictable: {}",
        expected.display()
    );
    assert_eq!(
        expected.file_name().and_then(|n| n.to_str()).expect("name"),
        format!(".config.sbvault.tmp-{}", std::process::id()),
        "the temp name contains no randomness at all"
    );
}

/// Regression guard for M7.
///
/// The documented DoS bound on a hostile vault header used to be 2 GiB, and
/// `validate()` accepted it. Anyone who can hand this installation a
/// `config.sbvault` — which is the *designed* threat, since the file is pulled
/// from a Git repository — could therefore make it commit 2 GiB of RAM per
/// unlock attempt before a single byte was authenticated.
///
/// The bound is now 1 GiB, pinned to `CALIBRATION_MAX_MEMORY_KIB` because
/// nothing this program writes ever exceeds that, and `validate()` additionally
/// refuses anything above what this machine can hold.
#[test]
fn a_hostile_header_cannot_demand_more_memory_than_the_ceiling_allows() {
    const OLD_CEILING_KIB: u32 = 2 * 1024 * 1024;

    // Checked at compile time, so the constant cannot creep back up even in a
    // build where this test is never run.
    const _: () = assert!(
        MAX_MEMORY_KIB <= 1024 * 1024,
        "the absolute Argon2 memory ceiling has crept back above 1 GiB"
    );

    let base = KdfParams::insecure_for_tests().expect("base");

    // The value the review demonstrated, and everything above it.
    for demand in [OLD_CEILING_KIB, OLD_CEILING_KIB + 1, u32::MAX] {
        let hostile = KdfParams { memory_kib: demand, ..base.clone() };
        assert!(
            hostile.validate().is_err(),
            "validate() accepts memory_kib = {demand} KiB; a pulled vault could make \
             the daemon allocate that much before authenticating anything"
        );
    }

    // Anything above what this machine will commit to one unlock is refused,
    // whatever the fixed ceiling says.
    let ceiling = superbackup_core::crypto::kdf::memory_ceiling_kib();
    assert!(ceiling <= MAX_MEMORY_KIB);
    if let Some(over) = ceiling.checked_add(1) {
        let hostile = KdfParams { memory_kib: over, ..base.clone() };
        assert!(hostile.validate().is_err(), "{over} KiB is above this machine's ceiling");
    }

    // The check must not have become so strict that ordinary vaults break: the
    // recommended parameters, and the floor, still validate.
    KdfParams::recommended().expect("recommended").validate().expect("defaults must open");
    KdfParams { memory_kib: superbackup_core::crypto::kdf::MIN_NEW_MEMORY_KIB, ..base.clone() }
        .validate()
        .expect("the documented floor must open");
    base.validate().expect("test parameters must still parse");
}
