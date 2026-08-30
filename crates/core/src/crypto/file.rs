//! The vault on disk: atomic writes, timestamped backups, and pruning.
//!
//! Everything in [`super::vault`] is pure: bytes in, bytes out. This module is
//! the only place that touches the filesystem, which keeps the cryptography
//! testable without a temp directory and keeps the "did we lose the file?"
//! reasoning in one readable place.
//!
//! # The rules
//!
//! Losing `config.sbvault` loses every repository passphrase, and therefore
//! every backup, permanently. So:
//!
//! 1. **Never overwrite the vault without first copying the current bytes into
//!    [`crate::paths::Paths::vault_backup_dir`]**, and never write the live
//!    file non-atomically.
//! 2. **Never mutate the in-memory vault before the write has succeeded.**
//!
//! The second rule is the subtler one and it is worth being explicit about
//! why. If a rotation re-keys `self.vault` and the backup or the write then
//! fails, the call returns an error while memory holds ciphertext under the
//! *new* passphrase and the file still holds the *old* one. The user is told
//! the rotation failed and carries on using the old passphrase — until the
//! next ordinary `save()` writes the new-passphrase ciphertext over the live
//! file, with no recovery backup, because creating the backup is exactly what
//! failed. A reported failure silently becomes a permanent lockout from every
//! backup the installation owns.
//!
//! Every method here therefore follows: **prepare (fallible) -> write ->
//! commit (infallible)**. [`crate::crypto::Vault::prepare_seal`] and
//! [`crate::crypto::Vault::prepare_rekey`] exist to make that shape the only
//! one expressible; there is no way to commit without first holding a value
//! that proves the bytes were computed, and the caller writes them in between.

use super::rekey::{DerivedRepository, Rekey, RekeyAcknowledgement};
use super::vault::Vault;
use crate::error::{Error, IoContext, Result};
use crate::paths::{self, Paths};
use crate::secret::Secret;
use chrono::Utc;
use std::path::{Path, PathBuf};

/// How many rotation backups to keep.
///
/// Each is a few kilobytes, so the cost of keeping them is nil, and the value
/// of the oldest one is "the user rotated their passphrase four times last
/// month and can no longer remember which one they actually committed to".
/// Ten is deep enough to survive a bad week and shallow enough that the
/// directory stays readable.
pub const BACKUP_KEEP: usize = 10;

/// Prefix of every backup file name.
const BACKUP_PREFIX: &str = "config.sbvault.";

/// Why a backup was taken. Ends up in the file name, so a user browsing the
/// directory can tell a routine save from a passphrase rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupReason {
    /// Before a passphrase rotation.
    Rekey,
    /// Before replacing the local vault with one pulled from a remote.
    RemotePull,
    /// Explicitly requested by the user.
    Manual,
}

impl BackupReason {
    fn tag(&self) -> &'static str {
        match self {
            BackupReason::Rekey => "rekey",
            BackupReason::RemotePull => "pull",
            BackupReason::Manual => "manual",
        }
    }
}

/// A vault bound to a location on disk.
#[derive(Debug)]
pub struct VaultFile {
    path: PathBuf,
    backup_dir: PathBuf,
    vault: Vault,
}

impl VaultFile {
    /// Load the vault from `paths`, without unlocking it.
    pub fn load(paths: &Paths) -> Result<VaultFile> {
        let path = paths.vault_file();
        let bytes = std::fs::read(&path).ctx(format!("reading the vault at {}", path.display()))?;
        Ok(VaultFile {
            vault: Vault::open_locked(&bytes)?,
            path,
            backup_dir: paths.vault_backup_dir(),
        })
    }

    /// Whether a vault exists at this location. Drives first-run detection.
    pub fn exists(paths: &Paths) -> bool {
        paths.vault_file().is_file()
    }

    /// Create a new vault and write it. Refuses to clobber an existing file:
    /// "create" must never be a way to destroy every key on the machine.
    pub fn create(paths: &Paths, passphrase: &Secret) -> Result<VaultFile> {
        Self::create_from(paths, Vault::create(passphrase)?)
    }

    /// [`VaultFile::create`] from an already-built vault, so the settings
    /// screen can hand in calibrated KDF parameters.
    pub fn create_from(paths: &Paths, vault: Vault) -> Result<VaultFile> {
        let mut vault = vault;
        let path = paths.vault_file();
        if path.exists() {
            return Err(Error::Path {
                path: path.clone(),
                reason: "a vault already exists here; refusing to overwrite it".into(),
            });
        }
        paths.ensure()?;
        let sealed = vault.prepare_seal()?;
        paths::write_atomic(&path, sealed.bytes())?;
        vault.commit_seal(sealed);
        Ok(VaultFile { path, backup_dir: paths.vault_backup_dir(), vault })
    }

    /// Adopt bytes that came from somewhere other than this file — a remote
    /// pull, or a restore from backup — after they have already been verified.
    ///
    /// The caller is responsible for having decrypted the bytes successfully
    /// first; see [`crate::remote`]. A backup of the current file is taken
    /// before anything is replaced.
    pub fn replace_with(&mut self, bytes: &[u8], reason: BackupReason) -> Result<()> {
        // Parse first, so a garbage payload is refused before a backup is
        // taken; then write; then adopt. `self.vault` changes only once the
        // bytes it describes are the bytes on disk.
        let replacement = Vault::open_locked(bytes)?;
        self.backup(reason)?;
        paths::write_atomic(&self.path, replacement.sealed_bytes())?;
        self.vault = replacement;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn backup_dir(&self) -> &Path {
        &self.backup_dir
    }

    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    pub fn vault_mut(&mut self) -> &mut Vault {
        &mut self.vault
    }

    /// Seal and write, if anything changed. A no-op when the vault is clean,
    /// so an idle daemon does not rewrite the file every minute.
    ///
    /// The vault is marked clean only after the write succeeds. Sealing first
    /// and writing second would leave a failed save looking like a completed
    /// one, and the retry the caller makes after handling the error would find
    /// nothing to do and silently drop the change.
    pub fn save(&mut self) -> Result<()> {
        if !self.vault.is_dirty() {
            return Ok(());
        }
        let sealed = self.vault.prepare_seal()?;
        paths::write_atomic(&self.path, sealed.bytes())?;
        self.vault.commit_seal(sealed);
        Ok(())
    }

    /// Rotate the master passphrase: prepare, back up, write, commit, prune.
    ///
    /// The ordering is the whole point.
    ///
    /// 1. [`Vault::prepare_rekey`] verifies the old passphrase, audits the
    ///    acknowledgement, derives both key hierarchies and produces the
    ///    complete new ciphertext — all of it or none of it, and **without
    ///    touching the vault**. Doing this first also means a mistyped
    ///    passphrase does not litter the backup directory on every attempt.
    /// 2. Copy the *current* file into the backup directory. This is the
    ///    recovery anchor for the repository migration that follows, and its
    ///    path is recorded on the returned [`Rekey`].
    /// 3. Write the new bytes atomically over the live file.
    /// 4. Only now [`Vault::commit_rekey`], which cannot fail.
    ///
    /// A failure at step 1, 2 or 3 leaves the in-memory vault and the file
    /// both on the old passphrase — consistent with each other and with what
    /// the user has just been told. A crash during step 3 leaves either the
    /// old file or the new one, because [`crate::paths::write_atomic`] renames
    /// rather than truncates. There is no interleaving that loses the keys and
    /// none that leaves memory and disk disagreeing about the passphrase.
    ///
    /// # Then what
    ///
    /// The returned [`Rekey`] is not optional bookkeeping. If it lists any
    /// repositories, they are on the *old* password and the vault now holds
    /// the *new* master key; until the engine migrates each one, they cannot
    /// be opened. See [`super::rekey`] for the sequence and for why the vault
    /// is written before the repositories are moved.
    pub fn change_passphrase(
        &mut self,
        old: &Secret,
        new: &Secret,
        ack: &RekeyAcknowledgement,
    ) -> Result<Rekey> {
        let plan = self.vault.prepare_rekey(old, new, ack)?;
        let backup = self.backup_without_pruning(BackupReason::Rekey)?;
        paths::write_atomic(&self.path, plan.sealed_bytes())?;
        let mut rekey = self.vault.commit_rekey(plan);
        // Prune only after the new file is safely in place, and never below
        // the backup this rotation just took — pruning it away mid-migration
        // would destroy the only on-disk copy of the old key hierarchy.
        self.prune_backups(BACKUP_KEEP)?;
        rekey.set_recovery_backup(backup);
        Ok(rekey)
    }

    /// Rebuild the plan for a rotation that was interrupted part-way through
    /// its repository migration.
    ///
    /// `backup` is [`crate::crypto::MigrationReport::recovery_backup`] from the
    /// interrupted run — or, if that was lost with the process, the newest
    /// `*.rekey` entry in [`VaultFile::list_backups`]. Both passphrases are
    /// needed, because both key hierarchies have to be reconstructed.
    pub fn resume_rekey(
        &self,
        backup: &Path,
        old: &Secret,
        new: &Secret,
        repositories: &[DerivedRepository],
    ) -> Result<Rekey> {
        let old_bytes = std::fs::read(backup)
            .ctx(format!("reading the pre-rotation backup {}", backup.display()))?;
        let new_bytes = std::fs::read(&self.path)
            .ctx(format!("reading the vault at {}", self.path.display()))?;
        let mut rekey = Rekey::resume(&new_bytes, new, &old_bytes, old, repositories)?;
        rekey.set_recovery_backup(Some(backup.to_path_buf()));
        Ok(rekey)
    }

    /// The most recent backup taken by a passphrase rotation, if any.
    ///
    /// The fallback for [`VaultFile::resume_rekey`] when the interrupted run's
    /// report did not survive.
    pub fn latest_rekey_backup(&self) -> Result<Option<PathBuf>> {
        Ok(self.list_backups()?.into_iter().find(|p| p.extension().is_some_and(|e| e == "rekey")))
    }

    /// Copy the current on-disk vault into the backup directory.
    ///
    /// Returns `Ok(None)` when there is nothing to back up yet.
    pub fn backup(&self, reason: BackupReason) -> Result<Option<PathBuf>> {
        let written = self.backup_without_pruning(reason)?;
        self.prune_backups(BACKUP_KEEP)?;
        Ok(written)
    }

    /// [`VaultFile::backup`] without the prune, for callers that must not
    /// discard an old backup until a later step has succeeded.
    fn backup_without_pruning(&self, reason: BackupReason) -> Result<Option<PathBuf>> {
        if !self.path.exists() {
            return Ok(None);
        }
        std::fs::create_dir_all(&self.backup_dir)
            .ctx(format!("creating {}", self.backup_dir.display()))?;
        paths::harden_dir(&self.backup_dir)?;

        let bytes =
            std::fs::read(&self.path).ctx(format!("reading {} for backup", self.path.display()))?;

        // Names are `config.sbvault.<UTC timestamp>-<counter>.<reason>`.
        //
        // The timestamp is fixed-width, big-endian and second-resolution; the
        // three-digit counter disambiguates the several rotations a test — or
        // an impatient user — can perform inside one second. Both fields are
        // fixed width so that a plain lexicographic sort is a chronological
        // sort, which is what [`VaultFile::list_backups`] relies on. The
        // counter continues from the highest one still present rather than
        // restarting at zero, so pruning cannot cause a later backup to sort
        // before an earlier one.
        let stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let prefix = format!("{BACKUP_PREFIX}{stamp}-");
        let next = self
            .list_backups()?
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .filter_map(|n| n.strip_prefix(&prefix))
            .filter_map(|rest| rest.split('.').next())
            .filter_map(|counter| counter.parse::<u32>().ok())
            .max()
            .map(|max| max + 1)
            .unwrap_or(0);
        if next > 999 {
            return Err(Error::Path {
                path: self.backup_dir.clone(),
                reason: "more than 1000 vault backups in a single second".into(),
            });
        }

        let candidate = self.backup_dir.join(format!("{prefix}{next:03}.{}", reason.tag()));
        paths::write_atomic(&candidate, &bytes)?;
        Ok(Some(candidate))
    }

    /// Every backup, newest first.
    pub fn list_backups(&self) -> Result<Vec<PathBuf>> {
        if !self.backup_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut found: Vec<PathBuf> = Vec::new();
        let entries = std::fs::read_dir(&self.backup_dir)
            .ctx(format!("listing {}", self.backup_dir.display()))?;
        for entry in entries {
            let entry = entry.ctx("reading a backup directory entry")?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with(BACKUP_PREFIX) && entry.path().is_file() {
                found.push(entry.path());
            }
        }
        // The timestamp is fixed-width and lexicographically ordered, so a
        // plain sort is a chronological sort — and unlike mtime it survives a
        // copy, a restore, or a filesystem with coarse timestamps.
        found.sort();
        found.reverse();
        Ok(found)
    }

    /// Delete all but the newest `keep` backups.
    ///
    /// Returns what was deleted, so the caller can log it. Failures to delete
    /// an individual file are reported rather than ignored, but pruning never
    /// touches the newest `keep` entries, so a partial failure can never
    /// remove the backup you were about to need.
    pub fn prune_backups(&self, keep: usize) -> Result<Vec<PathBuf>> {
        let backups = self.list_backups()?;
        let mut removed = Vec::new();
        for stale in backups.into_iter().skip(keep) {
            std::fs::remove_file(&stale)
                .ctx(format!("removing stale backup {}", stale.display()))?;
            removed.push(stale);
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::kdf::KdfParams;
    use crate::model::SecretRef;

    struct TempHome(PathBuf);

    impl TempHome {
        fn new(tag: &str) -> TempHome {
            let dir = std::env::temp_dir().join(format!(
                "sb-vaultfile-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
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

    #[test]
    fn create_refuses_to_clobber_an_existing_vault() {
        let home = TempHome::new("clobber");
        let paths = home.paths();
        let _first = new_vault(&paths, "one");
        let vault = Vault::create_unchecked(
            &Secret::from_str("two"),
            KdfParams::insecure_for_tests().expect("kdf"),
        )
        .expect("vault");
        assert!(
            VaultFile::create_from(&paths, vault).is_err(),
            "creating over an existing vault would destroy every key on the machine"
        );
    }

    #[test]
    fn rotation_backs_up_before_it_overwrites() {
        let home = TempHome::new("rotate");
        let paths = home.paths();
        let mut file = new_vault(&paths, "old-passphrase");
        file.vault_mut()
            .put(SecretRef("s3.access:1".into()), Secret::from_str("AKIA"))
            .expect("put");
        file.save().expect("save");
        let before = std::fs::read(file.path()).expect("read");

        let rekey = file
            .change_passphrase(
                &Secret::from_str("old-passphrase"),
                &Secret::from_str("new-one"),
                &RekeyAcknowledgement::NoDerivedRepositories,
            )
            .expect("rotate");
        assert!(rekey.repositories().is_empty());

        let backups = file.list_backups().expect("list");
        assert_eq!(backups.len(), 1, "exactly one backup for one rotation");
        assert_eq!(
            std::fs::read(&backups[0]).expect("read backup"),
            before,
            "the backup must be the pre-rotation bytes"
        );

        // The live file opens with the new passphrase and not the old one.
        let bytes = std::fs::read(file.path()).expect("read");
        assert!(Vault::unlock(&bytes, &Secret::from_str("new-one")).is_ok());
        assert!(matches!(
            Vault::unlock(&bytes, &Secret::from_str("old-passphrase")),
            Err(Error::BadPassphrase)
        ));
        // And the backup still opens with the old one, which is the entire
        // reason it exists.
        let backup = std::fs::read(&backups[0]).expect("read backup");
        assert!(Vault::unlock(&backup, &Secret::from_str("old-passphrase")).is_ok());
    }

    #[test]
    fn a_failed_rotation_writes_nothing_at_all() {
        let home = TempHome::new("failrotate");
        let paths = home.paths();
        let mut file = new_vault(&paths, "correct");
        let before = std::fs::read(file.path()).expect("read");

        let err = file
            .change_passphrase(
                &Secret::from_str("wrong"),
                &Secret::from_str("whatever"),
                &RekeyAcknowledgement::NoDerivedRepositories,
            )
            .expect_err("must fail");
        assert!(matches!(err, Error::BadPassphrase));

        assert_eq!(std::fs::read(file.path()).expect("read"), before, "file must be untouched");
        assert!(
            file.list_backups().expect("list").is_empty(),
            "a mistyped passphrase must not litter the backup directory"
        );
        assert!(Vault::unlock(&before, &Secret::from_str("correct")).is_ok());
    }

    #[test]
    fn backups_are_pruned_newest_first() {
        let home = TempHome::new("prune");
        let paths = home.paths();
        let file = new_vault(&paths, "pass");
        for _ in 0..(BACKUP_KEEP + 5) {
            file.backup(BackupReason::Manual).expect("backup");
        }
        let backups = file.list_backups().expect("list");
        assert_eq!(backups.len(), BACKUP_KEEP);
        // Newest first, and the newest is the one with the highest suffix.
        let mut sorted = backups.clone();
        sorted.sort();
        sorted.reverse();
        assert_eq!(backups, sorted);
    }

    #[test]
    fn save_is_a_no_op_when_nothing_changed() {
        let home = TempHome::new("nosave");
        let paths = home.paths();
        let mut file = new_vault(&paths, "pass");
        let before = std::fs::read(file.path()).expect("read");
        file.save().expect("save");
        assert_eq!(std::fs::read(file.path()).expect("read"), before);

        file.vault_mut().put(SecretRef("a:1".into()), Secret::from_str("x")).expect("put");
        file.save().expect("save");
        assert_ne!(std::fs::read(file.path()).expect("read"), before);
    }

    #[test]
    fn replace_with_backs_up_first() {
        let home = TempHome::new("replace");
        let paths = home.paths();
        let mut file = new_vault(&paths, "local");
        let original = std::fs::read(file.path()).expect("read");

        let mut other = Vault::create_unchecked(
            &Secret::from_str("remote"),
            KdfParams::insecure_for_tests().expect("kdf"),
        )
        .expect("vault");
        let incoming = other.seal().expect("seal");

        file.replace_with(&incoming, BackupReason::RemotePull).expect("replace");
        assert_eq!(std::fs::read(file.path()).expect("read"), incoming);
        let backups = file.list_backups().expect("list");
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).expect("read"), original);
    }

    #[test]
    fn replace_with_rejects_garbage_before_touching_the_file() {
        let home = TempHome::new("replacebad");
        let paths = home.paths();
        let mut file = new_vault(&paths, "local");
        let original = std::fs::read(file.path()).expect("read");
        assert!(file.replace_with(b"definitely not a vault", BackupReason::RemotePull).is_err());
        assert_eq!(std::fs::read(file.path()).expect("read"), original);
        assert!(file.list_backups().expect("list").is_empty());
    }
}
