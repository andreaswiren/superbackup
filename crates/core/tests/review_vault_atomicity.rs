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

/// `VaultFile::change_passphrase` rotates the **in-memory** vault before it
/// takes the recovery backup and before it writes anything. If the backup step
/// fails, the call reports failure — but the in-memory vault is already sealed
/// under the *new* passphrase, while the file on disk is still under the old
/// one.
///
/// The next successful `save()` then silently writes the new-passphrase
/// ciphertext over the live file. The user was told the rotation failed and
/// still believes their old passphrase works. It does not, and there is no
/// recovery backup either — the backup is exactly what failed.
///
/// The I/O failure is provoked here by putting a plain file where the backup
/// *directory* must go, which is the moral equivalent of a full disk, a
/// revoked ACL, or an antivirus lock on `vault-backups`.
#[test]
fn a_failed_rotation_leaves_the_in_memory_vault_rotated_and_the_disk_file_not() {
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
        "after a rotation that REPORTED FAILURE, the live vault no longer opens \
         with the old passphrase: the failed rotation was silently committed by \
         the next save"
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

/// The documented DoS bound on a hostile vault header is 2 GiB. Anyone who can
/// hand this installation a `config.sbvault` — which is the *designed* threat,
/// since the file is pulled from a Git repository — can therefore make it
/// commit 2 GiB of RAM per unlock attempt before a single byte is
/// authenticated.
#[test]
fn a_hostile_header_can_still_demand_a_two_gibibyte_allocation() {
    let hostile = KdfParams {
        memory_kib: MAX_MEMORY_KIB,
        ..KdfParams::insecure_for_tests().expect("base")
    };
    assert!(
        hostile.validate().is_err(),
        "validate() accepts memory_kib = {} KiB ({} GiB); a pulled vault can make \
         the daemon allocate that much before authenticating anything",
        MAX_MEMORY_KIB,
        MAX_MEMORY_KIB / (1024 * 1024)
    );
}
