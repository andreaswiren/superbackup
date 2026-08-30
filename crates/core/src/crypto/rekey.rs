//! Master-passphrase rotation, and the repository migration it forces.
//!
//! # The hazard
//!
//! A destination whose [`PassphraseSource`] is
//! [`DerivedFromMaster`](PassphraseSource::DerivedFromMaster) has no stored
//! repository password. Its password is *computed*:
//!
//! ```text
//! passphrase + salt --Argon2id--> master key --HKDF--> repository password
//! ```
//!
//! Rotating the master passphrase mints a fresh salt and therefore a fresh
//! master key, so every derived repository password changes at the same
//! instant. Kopia knows nothing about this. The repository on disk, or in the
//! bucket, still expects the *old* password.
//!
//! Left alone, the failure mode is: the user rotates their passphrase, the
//! rotation appears to succeed, and every derived-passphrase repository
//! becomes unopenable — discovered at 03:00 that night, by a scheduled job, in
//! a log nobody reads. That is the worst outcome this application can produce,
//! and a doc comment is not a control.
//!
//! # The mechanism
//!
//! ```text
//!   derived_repositories(&config)      "these five will need migrating"
//!            │                          (shown to the user BEFORE they commit)
//!            ▼
//!   RekeyAcknowledgement::migrate(..)  the caller states what it will do
//!            │
//!            ▼
//!   Store::change_passphrase_migrating  new vault written, backup kept
//!            │
//!            ▼
//!   Rekey  ──► credentials(dest) -> { old, new }   engine runs
//!          ──► mark_migrated(dest) / mark_failed    `kopia repository
//!          ──► report()                              change-password`
//! ```
//!
//! [`Store::change_passphrase`](crate::config::Store::change_passphrase)
//! **refuses outright** when derived repositories exist. The only way through
//! is to name them, which means the caller has necessarily looked at the list.
//!
//! # Why the vault is written *before* the repositories are migrated
//!
//! This ordering is the difference between a recoverable interruption and a
//! permanent loss, and it is not the intuitive one.
//!
//! Migrating repositories first and writing the vault last looks safer — "do
//! not commit until the work is done" — but it is the trap. The new salt is
//! generated during the rotation and exists only in memory until the vault is
//! written. If the process dies after three of five repositories have been
//! moved to the new password, and the new vault was never written, that salt
//! is gone; the new password for those three repositories can never be
//! recomputed, and they are lost.
//!
//! So: the new vault is written first, atomically, after a timestamped backup
//! of the old one. An interruption then leaves
//!
//! * the live vault holding the **new** master key — so the new password for
//!   an already-migrated repository is recomputable, and
//! * a backup holding the **old** master key — so the old password for a
//!   not-yet-migrated repository is recomputable too.
//!
//! Both halves survive on disk, and [`Rekey::resume`] reconstitutes exactly
//! the state the interrupted run had. Nothing is unrecoverable as long as the
//! user still knows both passphrases, which they do: they typed both of them
//! minutes ago.

use super::keys::MasterKeys;
use super::signing;
use super::vault::Vault;
use crate::error::{Error, Result};
use crate::model::{Config, DestinationKind, PassphraseSource};
use crate::secret::Secret;
use serde::Serialize;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// A destination whose repository password is derived from the master key, and
/// which therefore has to be re-passworded when the master passphrase changes.
///
/// Everything here is non-secret and safe to render in a confirmation dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DerivedRepository {
    pub destination_id: Uuid,
    pub destination_name: String,
    /// Where the repository lives, for the confirmation dialog: a filesystem
    /// path, or `bucket/prefix`. Never a credential.
    pub location: String,
}

/// Every destination in `config` whose repository password is derived from the
/// master key.
///
/// This is the list the GUI must show *before* the user commits to a rotation:
/// "changing your master passphrase will re-password these 5 repositories;
/// this will take a while and must not be interrupted."
///
/// Folder mirrors are excluded: they have no repository and no password.
/// Destinations with no explicit [`crate::model::EncryptionSettings`] are also
/// excluded, because the model's default source is
/// [`PassphraseSource::Generated`] — a stored secret, unaffected by rotation.
pub fn derived_repositories(config: &Config) -> Vec<DerivedRepository> {
    config
        .destinations
        .iter()
        .filter(|destination| destination.kind.is_repository())
        .filter(|destination| {
            destination
                .encryption
                .as_ref()
                .is_some_and(|e| e.passphrase_source == PassphraseSource::DerivedFromMaster)
        })
        .map(|destination| DerivedRepository {
            destination_id: destination.id,
            destination_name: destination.name.clone(),
            location: describe_location(&destination.kind),
        })
        .collect()
}

fn describe_location(kind: &DestinationKind) -> String {
    match kind {
        DestinationKind::LocalRepository { path }
        | DestinationKind::OneDrive { path, .. }
        | DestinationKind::LocalMirror { path } => path.display().to_string(),
        DestinationKind::S3 { bucket, prefix, .. } => format!("{bucket}/{prefix}"),
    }
}

/// The caller's statement about how it will handle derived repositories.
///
/// A required argument to [`Vault::change_passphrase`], so that "I forgot
/// about the repositories" is not a thing anyone can express by accident. The
/// two variants are the only two coherent positions, and both have to be
/// typed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RekeyAcknowledgement {
    /// There is nothing to migrate.
    ///
    /// [`crate::config::Store`] checks this against the real configuration and
    /// refuses a rotation that contradicts it. A caller driving [`Vault`]
    /// directly is asserting it, and the vault will still cross-check its own
    /// embedded configuration when it has one.
    NoDerivedRepositories,
    /// The caller will migrate these repositories with the returned
    /// [`Rekey`], and understands that skipping one makes it unopenable.
    Migrate(Vec<DerivedRepository>),
}

impl RekeyAcknowledgement {
    /// Build the right acknowledgement for a configuration.
    pub fn for_config(config: &Config) -> RekeyAcknowledgement {
        let derived = derived_repositories(config);
        if derived.is_empty() {
            RekeyAcknowledgement::NoDerivedRepositories
        } else {
            RekeyAcknowledgement::Migrate(derived)
        }
    }

    pub fn repositories(&self) -> &[DerivedRepository] {
        match self {
            RekeyAcknowledgement::NoDerivedRepositories => &[],
            RekeyAcknowledgement::Migrate(repositories) => repositories,
        }
    }
}

/// Where one repository has got to in a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationState {
    /// Still on the old password.
    Pending,
    /// Successfully moved to the new password.
    Migrated,
    /// The attempt failed. The repository may be on either password; the
    /// engine must probe with both, which is why
    /// [`RepositoryCredentials`] carries both.
    Failed,
}

/// One repository's slot in the migration, with no secret material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryMigration {
    pub destination_id: Uuid,
    pub destination_name: String,
    pub location: String,
    pub state: MigrationState,
    /// Redacted failure text from the last attempt.
    pub last_error: Option<String>,
}

/// The old and the new password for one repository.
///
/// # Note for the caller
///
/// Both are supplied, not just the new one, and the engine should use both:
/// the correct operation is "try to open with `new`; if that works this
/// repository is already migrated, do nothing; otherwise open with `old` and
/// change the password to `new`". That makes each step idempotent, which is
/// what makes the whole migration safely re-runnable after a crash, a network
/// blip, or a laptop lid closing halfway through.
#[derive(Debug)]
pub struct RepositoryCredentials {
    pub destination_id: Uuid,
    /// The password the repository has now, if it has not been migrated.
    pub old: Secret,
    /// The password it must end up with.
    pub new: Secret,
}

/// A serialisable snapshot of migration progress, safe to persist and to send
/// over IPC.
///
/// Contains no key material — only identities and states — so the engine can
/// write it into its own state file and pick the migration back up after a
/// restart, at which point [`Rekey::resume`] rebuilds the secret half from the
/// two vault files and the two passphrases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MigrationReport {
    pub vault_id: Uuid,
    pub total: usize,
    pub migrated: usize,
    pub failed: usize,
    pub pending: usize,
    pub repositories: Vec<RepositoryMigration>,
    /// The vault backup taken immediately before the rotation. This file plus
    /// the old passphrase is the recovery anchor; do not prune it until the
    /// migration is complete.
    pub recovery_backup: Option<PathBuf>,
    /// Signer fingerprint before and after. A rotation changes the signing
    /// identity, so anyone pinning this machine in `trusted_signers` has to
    /// re-pin.
    pub old_signer_fingerprint: String,
    pub new_signer_fingerprint: String,
}

/// A master-passphrase rotation that has been committed to the vault, together
/// with everything needed to migrate the repositories it invalidated.
///
/// Holding one of these is the only moment at which both the old and the new
/// key hierarchies exist at once, which is the only moment at which a
/// repository can be moved from one to the other.
pub struct Rekey {
    vault_id: Uuid,
    old_keys: MasterKeys,
    new_keys: MasterKeys,
    sealed_bytes: Vec<u8>,
    repositories: Vec<RepositoryMigration>,
    recovery_backup: Option<PathBuf>,
    old_signer: String,
    new_signer: String,
}

impl std::fmt::Debug for Rekey {
    /// Metadata only. A `{:?}` of this must never print a repository password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rekey")
            .field("vault_id", &self.vault_id)
            .field("repositories", &self.repositories.len())
            .field("pending", &self.pending().count())
            .field("recovery_backup", &self.recovery_backup)
            .finish_non_exhaustive()
    }
}

impl Rekey {
    /// Assemble a rekey from two already-derived key hierarchies.
    pub(crate) fn new(
        vault_id: Uuid,
        old_keys: MasterKeys,
        new_keys: MasterKeys,
        sealed_bytes: Vec<u8>,
        repositories: &[DerivedRepository],
    ) -> Result<Rekey> {
        Ok(Rekey {
            vault_id,
            old_signer: signing::seed_fingerprint(old_keys.signing_seed())?,
            new_signer: signing::seed_fingerprint(new_keys.signing_seed())?,
            old_keys,
            new_keys,
            sealed_bytes,
            repositories: repositories
                .iter()
                .map(|repository| RepositoryMigration {
                    destination_id: repository.destination_id,
                    destination_name: repository.destination_name.clone(),
                    location: repository.location.clone(),
                    state: MigrationState::Pending,
                    last_error: None,
                })
                .collect(),
            recovery_backup: None,
        })
    }

    /// Rebuild a rekey from the two vault files, to resume an interrupted
    /// migration.
    ///
    /// `new_vault_bytes` is the live `config.sbvault` (already rotated);
    /// `old_vault_bytes` is the backup taken immediately before the rotation,
    /// named by [`MigrationReport::recovery_backup`]. Both passphrases are
    /// required, because both key hierarchies have to be reconstructed.
    ///
    /// Every repository comes back as [`MigrationState::Pending`]. That is
    /// deliberate: this function cannot know which ones already moved, and
    /// guessing would be worse than re-running. Re-running is safe precisely
    /// because each step is idempotent — see [`RepositoryCredentials`].
    ///
    /// # Errors
    ///
    /// Refuses when the two files are not the same vault. Rotating with a
    /// backup belonging to some *other* vault would compute repository
    /// passwords from an unrelated key and change every repository to a
    /// password nothing can reproduce.
    pub fn resume(
        new_vault_bytes: &[u8],
        new_passphrase: &Secret,
        old_vault_bytes: &[u8],
        old_passphrase: &Secret,
        repositories: &[DerivedRepository],
    ) -> Result<Rekey> {
        let new_vault = Vault::unlock(new_vault_bytes, new_passphrase)?;
        let old_vault = Vault::unlock(old_vault_bytes, old_passphrase)?;
        if new_vault.id() != old_vault.id() {
            return Err(Error::Validation(format!(
                "the backup is vault {} but the live file is vault {}; these are different \
                 vaults, and resuming across them would set every repository to a password \
                 nothing can reproduce",
                old_vault.id(),
                new_vault.id()
            )));
        }
        if new_vault.header().kdf.salt == old_vault.header().kdf.salt {
            return Err(Error::Validation(
                "the backup and the live vault share a salt, so no rotation happened between \
                 them; there is nothing to resume"
                    .into(),
            ));
        }
        let vault_id = new_vault.id();
        Rekey::new(
            vault_id,
            old_vault.into_keys()?,
            new_vault.into_keys()?,
            new_vault_bytes.to_vec(),
            repositories,
        )
    }

    /// The sealed vault produced by the rotation — what is (or is about to be)
    /// on disk.
    pub fn sealed_bytes(&self) -> &[u8] {
        &self.sealed_bytes
    }

    pub fn vault_id(&self) -> Uuid {
        self.vault_id
    }

    /// Every repository this rotation invalidated, in configuration order.
    pub fn repositories(&self) -> &[RepositoryMigration] {
        &self.repositories
    }

    /// Those not yet migrated. Failed entries are included: a failure is not a
    /// reason to stop trying.
    pub fn pending(&self) -> impl Iterator<Item = &RepositoryMigration> {
        self.repositories.iter().filter(|r| r.state != MigrationState::Migrated)
    }

    /// True when every repository has been moved to the new password.
    pub fn is_complete(&self) -> bool {
        self.repositories.iter().all(|r| r.state == MigrationState::Migrated)
    }

    /// The old and new repository password for one destination.
    ///
    /// # Errors
    ///
    /// [`Error::Validation`] when the destination is not part of this
    /// rotation. Deriving a password for a destination nobody asked about
    /// would be a silent way to re-password the wrong repository.
    pub fn credentials(&self, destination_id: &Uuid) -> Result<RepositoryCredentials> {
        if !self.repositories.iter().any(|r| &r.destination_id == destination_id) {
            return Err(Error::Validation(format!(
                "destination {destination_id} is not part of this passphrase rotation"
            )));
        }
        Ok(RepositoryCredentials {
            destination_id: *destination_id,
            old: self.old_keys.repo_passphrase(destination_id)?,
            new: self.new_keys.repo_passphrase(destination_id)?,
        })
    }

    /// Record that a repository now uses the new password.
    pub fn mark_migrated(&mut self, destination_id: &Uuid) -> Result<()> {
        self.slot(destination_id).map(|slot| {
            slot.state = MigrationState::Migrated;
            slot.last_error = None;
        })
    }

    /// Record a failure, with text that has already been through
    /// [`crate::redact::scrub`] at the call site if it came from a subprocess.
    pub fn mark_failed(&mut self, destination_id: &Uuid, reason: impl Into<String>) -> Result<()> {
        let reason = reason.into();
        self.slot(destination_id).map(|slot| {
            slot.state = MigrationState::Failed;
            slot.last_error = Some(crate::redact::scrub(&reason).into_owned());
        })
    }

    fn slot(&mut self, destination_id: &Uuid) -> Result<&mut RepositoryMigration> {
        self.repositories
            .iter_mut()
            .find(|r| &r.destination_id == destination_id)
            .ok_or_else(|| {
                Error::Validation(format!(
                    "destination {destination_id} is not part of this passphrase rotation"
                ))
            })
    }

    /// The backup taken immediately before the rotation. Keep it until the
    /// migration completes; it is the only place the old key hierarchy still
    /// exists on disk.
    pub fn recovery_backup(&self) -> Option<&Path> {
        self.recovery_backup.as_deref()
    }

    pub(crate) fn set_recovery_backup(&mut self, path: Option<PathBuf>) {
        self.recovery_backup = path;
    }

    /// The signer fingerprint before the rotation, for the "your published
    /// vaults were signed as X" line in the GUI.
    pub fn old_signer_fingerprint(&self) -> &str {
        &self.old_signer
    }

    /// The signer fingerprint after the rotation. Anyone pinning this machine
    /// in `trusted_signers` must be given this value, or their next pull will
    /// be rejected — which is the correct behaviour, and exactly why the value
    /// is surfaced here rather than left to be discovered.
    pub fn new_signer_fingerprint(&self) -> &str {
        &self.new_signer
    }

    /// A persistable, secret-free snapshot of progress.
    pub fn report(&self) -> MigrationReport {
        MigrationReport {
            vault_id: self.vault_id,
            total: self.repositories.len(),
            migrated: self
                .repositories
                .iter()
                .filter(|r| r.state == MigrationState::Migrated)
                .count(),
            failed: self.repositories.iter().filter(|r| r.state == MigrationState::Failed).count(),
            pending: self
                .repositories
                .iter()
                .filter(|r| r.state == MigrationState::Pending)
                .count(),
            repositories: self.repositories.clone(),
            recovery_backup: self.recovery_backup.clone(),
            old_signer_fingerprint: self.old_signer.clone(),
            new_signer_fingerprint: self.new_signer.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::KdfParams;
    use crate::model::*;

    fn kdf() -> KdfParams {
        KdfParams::insecure_for_tests().expect("kdf")
    }

    fn destination(name: &str, source: Option<PassphraseSource>) -> Destination {
        Destination {
            id: Uuid::new_v4(),
            name: name.into(),
            kind: DestinationKind::LocalRepository { path: "/backups".into() },
            encryption: source.map(|passphrase_source| EncryptionSettings {
                passphrase_source,
                ..EncryptionSettings::default()
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

    #[test]
    fn enumeration_picks_out_exactly_the_derived_destinations() {
        let mut config = Config::default();
        config.destinations.push(destination("derived", Some(PassphraseSource::DerivedFromMaster)));
        config.destinations.push(destination("generated", Some(PassphraseSource::Generated)));
        config.destinations.push(destination("typed", Some(PassphraseSource::UserSupplied)));
        config.destinations.push(destination("no-encryption-settings", None));

        // A mirror has no repository, so it cannot have a repository password
        // even if somebody set the field.
        let mut mirror = destination("mirror", Some(PassphraseSource::DerivedFromMaster));
        mirror.kind = DestinationKind::LocalMirror { path: "/mirror".into() };
        config.destinations.push(mirror);

        let derived = derived_repositories(&config);
        assert_eq!(derived.len(), 1, "{derived:#?}");
        assert_eq!(derived[0].destination_name, "derived");
        assert_eq!(derived[0].location, "/backups");
    }

    #[test]
    fn s3_locations_are_described_without_credentials() {
        let mut config = Config::default();
        let mut s3 = destination("offsite", Some(PassphraseSource::DerivedFromMaster));
        s3.kind = DestinationKind::S3 {
            provider_id: Uuid::new_v4(),
            bucket: "backups".into(),
            prefix: "superbackup/pc-1/".into(),
            credential_override: None,
        };
        config.destinations.push(s3);
        assert_eq!(derived_repositories(&config)[0].location, "backups/superbackup/pc-1/");
    }

    #[test]
    fn the_acknowledgement_follows_the_configuration() {
        let empty = Config::default();
        assert_eq!(
            RekeyAcknowledgement::for_config(&empty),
            RekeyAcknowledgement::NoDerivedRepositories
        );

        let mut config = Config::default();
        config.destinations.push(destination("derived", Some(PassphraseSource::DerivedFromMaster)));
        let ack = RekeyAcknowledgement::for_config(&config);
        assert_eq!(ack.repositories().len(), 1);
    }

    #[test]
    fn credentials_are_refused_for_a_destination_outside_the_rotation() {
        let mut vault =
            Vault::create_unchecked(&Secret::from_str("old"), kdf()).expect("vault");
        let inside = DerivedRepository {
            destination_id: Uuid::from_u128(1),
            destination_name: "in".into(),
            location: "/backups".into(),
        };
        let rekey = vault
            .change_passphrase(
                &Secret::from_str("old"),
                &Secret::from_str("new"),
                &RekeyAcknowledgement::Migrate(vec![inside]),
            )
            .expect("rotate");

        assert!(rekey.credentials(&Uuid::from_u128(1)).is_ok());
        assert!(
            rekey.credentials(&Uuid::from_u128(2)).is_err(),
            "deriving a password for a destination nobody listed would re-password the wrong repository"
        );
    }

    #[test]
    fn progress_tracking_and_reporting() {
        let mut vault =
            Vault::create_unchecked(&Secret::from_str("old"), kdf()).expect("vault");
        let repositories: Vec<DerivedRepository> = (1..=3)
            .map(|n| DerivedRepository {
                destination_id: Uuid::from_u128(n),
                destination_name: format!("repo-{n}"),
                location: "/backups".into(),
            })
            .collect();
        let mut rekey = vault
            .change_passphrase(
                &Secret::from_str("old"),
                &Secret::from_str("new"),
                &RekeyAcknowledgement::Migrate(repositories),
            )
            .expect("rotate");

        assert_eq!(rekey.pending().count(), 3);
        assert!(!rekey.is_complete());

        rekey.mark_migrated(&Uuid::from_u128(1)).expect("mark");
        rekey
            .mark_failed(&Uuid::from_u128(2), "KOPIA_PASSWORD=hunter2 refused")
            .expect("mark");

        let report = rekey.report();
        assert_eq!((report.total, report.migrated, report.failed, report.pending), (3, 1, 1, 1));
        assert!(!rekey.is_complete());
        assert_eq!(rekey.pending().count(), 2, "a failure stays in the queue");

        // Failure text is scrubbed, because it comes from a subprocess.
        let failed = report.repositories.iter().find(|r| r.destination_id == Uuid::from_u128(2));
        let message = failed.expect("failed slot").last_error.clone().expect("message");
        assert!(!message.contains("hunter2"), "{message}");

        rekey.mark_migrated(&Uuid::from_u128(2)).expect("retry");
        rekey.mark_migrated(&Uuid::from_u128(3)).expect("mark");
        assert!(rekey.is_complete());
        assert_eq!(rekey.pending().count(), 0);
        assert!(rekey.report().repositories.iter().all(|r| r.last_error.is_none()));

        assert!(rekey.mark_migrated(&Uuid::from_u128(99)).is_err());
    }

    #[test]
    fn rekey_debug_never_prints_a_repository_password() {
        let mut vault =
            Vault::create_unchecked(&Secret::from_str("old"), kdf()).expect("vault");
        let rekey = vault
            .change_passphrase(
                &Secret::from_str("old"),
                &Secret::from_str("new"),
                &RekeyAcknowledgement::Migrate(vec![DerivedRepository {
                    destination_id: Uuid::from_u128(1),
                    destination_name: "repo".into(),
                    location: "/backups".into(),
                }]),
            )
            .expect("rotate");
        let credentials = rekey.credentials(&Uuid::from_u128(1)).expect("credentials");
        let secret = credentials.new.expose_str().expect("printable").to_string();
        assert!(!format!("{rekey:?}").contains(&secret));
        assert!(!format!("{:?}", rekey.report()).contains(&secret));
        // `RepositoryCredentials` holds `Secret`s, whose own Debug is redacted.
        assert!(!format!("{credentials:?}").contains(&secret));
    }
}
