//! Master-passphrase rotation, and the repository migration it forces.
//!
//! Read [`superbackup_core::crypto::rekey`] first — it explains *why* the
//! vault is committed before any repository is touched, and why that
//! counter-intuitive order is the difference between a recoverable
//! interruption and permanently unopenable backups. This module is the other
//! half: the part that actually walks the repositories.
//!
//! ```text
//!   change_passphrase(current, replacement)
//!        │
//!        ├─ 1. verify `current` opens the live vault      ← before anything
//!        ├─ 2. Store::change_passphrase_migrating         ← new vault on disk,
//!        │                                                  backup kept
//!        ├─ 3. persist the migration report               ← BEFORE step 4
//!        ├─ 4. suppress the pending destinations          ← no failure storm
//!        └─ 5. per repository, in the background:
//!                 try NEW password  ──► already migrated, mark and move on
//!                 else OLD password ──► kopia repository change-password
//!                                       mark migrated, rewrite the report
//! ```
//!
//! ## The four rules, and what each one prevents
//!
//! **The report is persisted before the first repository is touched.** If the
//! process dies during step 5, the report on disk plus
//! [`superbackup_core::crypto::rekey::Rekey::resume`] reconstitutes exactly
//! the state the interrupted run had. Writing the report afterwards would
//! leave a machine that knows a rotation happened and nothing about how far it
//! got.
//!
//! **Every step is idempotent: new password first, then old.** A repository
//! that was migrated just before the crash opens with the new password, so
//! re-running does nothing to it. A repository that was not opens with the old
//! one. That is why
//! [`RepositoryCredentials`](superbackup_core::crypto::rekey::RepositoryCredentials)
//! carries both, and why this module tries them in that order rather than
//! trusting its own record of what it had done.
//!
//! **The recovery backup is not pruned mid-migration.** It is the only place
//! the *old* key hierarchy still exists on disk, so it is the only way to
//! recompute the old password for a repository that has not moved yet. It is
//! named in the persisted report and left strictly alone until every
//! repository is migrated.
//!
//! **Pending destinations are kept out of the scheduler's configuration.**
//! Without this, every scheduled run against every not-yet-migrated repository
//! fails with "invalid repository password" until the walk finishes — a wall
//! of expected failures that buries the one message the user needs to see. The
//! suppression lives in memory only ([`Runtime::effective_config`]), so an
//! interrupted migration cannot leave a destination switched off in
//! `config.json`; it is rebuilt from the persisted report at startup instead.

use std::collections::BTreeSet;
use std::sync::Arc;

use superbackup_core::crypto::rekey::{MigrationReport, MigrationState, Rekey};
use superbackup_core::kopia::{KopiaDriver, RunContext};
use superbackup_core::model::Destination;
use superbackup_core::paths;
use superbackup_core::secret::Secret;
use superbackup_core::state::{Event, Severity};
use superbackup_core::{Error, Result};
use uuid::Uuid;

use super::runtime::{PendingMigration, Runtime};

/// Where the migration report lives between the vault being rewritten and the
/// last repository being moved.
///
/// In the data directory rather than the config directory: it is local
/// progress, not user intent, and it must never be published to a shared
/// remote alongside `config.json`.
pub fn report_path(paths: &superbackup_core::paths::Paths) -> std::path::PathBuf {
    paths.data_dir.join("rekey-migration.json")
}

/// Persist the report atomically. Failing to write it aborts the rotation
/// *before* any repository is touched, which is the only point at which
/// aborting is free.
pub fn write_report(
    paths: &superbackup_core::paths::Paths,
    report: &MigrationReport,
) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|e| Error::Internal(format!("the migration report could not be written: {e}")))?;
    paths::write_atomic(&report_path(paths), &bytes)?;
    paths::harden_file(&report_path(paths))
}

/// Read a report left behind by an interrupted rotation.
pub fn read_report(paths: &superbackup_core::paths::Paths) -> Option<MigrationReport> {
    let bytes = std::fs::read(report_path(paths)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Remove the report once every repository has moved.
pub fn clear_report(paths: &superbackup_core::paths::Paths) {
    let path = report_path(paths);
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!(error = %e, "could not remove the finished migration report");
        }
    }
}

/// Destinations in a report that are not yet on the new password.
fn still_pending(report: &MigrationReport) -> BTreeSet<Uuid> {
    report
        .repositories
        .iter()
        .filter(|r| r.state != MigrationState::Migrated)
        .map(|r| r.destination_id)
        .collect()
}

/// Rebuild the scheduler suppression from a report left by a previous run.
///
/// Called at startup. A machine that was switched off halfway through a
/// rotation comes back with the right destinations suppressed and a loud
/// activity-log line telling the user what to do, rather than with a schedule
/// full of password failures.
pub async fn restore_after_restart(runtime: &Arc<Runtime>) {
    let Some(report) = read_report(&runtime.paths) else { return };
    let pending = still_pending(&report);
    if pending.is_empty() {
        clear_report(&runtime.paths);
        return;
    }
    let names: Vec<String> = report
        .repositories
        .iter()
        .filter(|r| r.state != MigrationState::Migrated)
        .map(|r| r.destination_name.clone())
        .collect();
    runtime.set_migration(Some(PendingMigration { destinations: pending, report }));
    runtime.record_event(Event::new(
        Severity::Warning,
        "vault.rekey_incomplete",
        format!(
            "A master-passphrase change did not finish: {} still use the old repository \
             password and are paused. Change the passphrase again with both passphrases to \
             finish, or restore the vault backup.",
            names.join(", ")
        ),
    ));
}

/// Rotate the master passphrase and migrate every repository it invalidates.
///
/// Returns as soon as the vault has been rewritten and the report persisted.
/// The repository walk continues on a background task, because re-passwording
/// a remote repository takes as long as a network round trip per repository
/// and an IPC handler must not hold the connection open for it.
pub async fn change_passphrase(
    runtime: &Arc<Runtime>,
    current: Secret,
    replacement: Secret,
) -> Result<()> {
    if replacement.is_empty() {
        return Err(Error::Validation("the new passphrase cannot be empty".into()));
    }
    if current.ct_eq(&replacement) {
        return Err(Error::Validation(
            "the new passphrase is the same as the current one".into(),
        ));
    }
    let strength = replacement
        .expose_str()
        .map(superbackup_core::secret::estimate_strength)
        .unwrap_or(superbackup_core::secret::Strength::Weak);
    if !strength.is_acceptable() {
        return Err(Error::Validation(format!(
            "that passphrase is {}. Choose a longer one — there is no way to recover a vault \
             whose passphrase is guessed.",
            strength.title().to_lowercase()
        )));
    }
    if runtime.migration().is_some_and(|m| !m.is_empty()) {
        return Err(Error::Validation(
            "a previous passphrase change has not finished migrating its repositories. Let it \
             finish before starting another."
                .into(),
        ));
    }

    // Verifying `current` before anything is written is not paranoia: the
    // rotation itself would fail on a wrong passphrase, but it would do so
    // after taking a vault backup and possibly after partial work.
    {
        let store = runtime.store.lock().await;
        let bytes = store.vault().sealed_bytes().to_vec();
        drop(store);
        superbackup_core::crypto::Vault::unlock(&bytes, &current)
            .map_err(|_| Error::BadPassphrase)?;
    }

    // The vault is committed here. From this instant the live vault holds the
    // NEW key hierarchy and the backup holds the old one; both halves are on
    // disk, which is what makes an interruption recoverable.
    let mut rekey = {
        let mut store = runtime.store.lock().await;
        store.change_passphrase_migrating(&current, &replacement)?
    };

    // Whatever else happens, the daemon is now unlocked under the new
    // passphrase and must know it.
    runtime.remember_master(Secret::new(replacement.expose().to_vec()));
    if let Err(e) = super::keychain::forget(&runtime.paths) {
        tracing::debug!(error = %e, "could not clear the cached passphrase after a rotation");
    }

    let report = rekey.report();
    let pending = still_pending(&report);

    // Rule 1: the report reaches disk BEFORE the first repository is touched.
    // If this write fails the rotation is still complete and recoverable — the
    // vault and its backup are both on disk — but the walk must not start
    // blind, so it is reported as an error the user has to act on.
    if let Err(e) = write_report(&runtime.paths, &report) {
        runtime.record_event(Event::new(
            Severity::Error,
            "vault.rekey_report_failed",
            format!(
                "The passphrase was changed, but the migration record could not be saved ({e}). \
                 Do not delete the vault backup at {}.",
                report
                    .recovery_backup
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "the vault-backups folder".into())
            ),
        ));
        return Err(e);
    }

    runtime.record_event(Event::info(
        "vault.passphrase_changed",
        if pending.is_empty() {
            "The master passphrase was changed.".to_string()
        } else {
            format!(
                "The master passphrase was changed. {} repositor{} must now be re-passworded; \
                 their scheduled backups are paused until that finishes.",
                pending.len(),
                if pending.len() == 1 { "y" } else { "ies" }
            )
        },
    ));

    if pending.is_empty() {
        clear_report(&runtime.paths);
        runtime.publish_status().await;
        return Ok(());
    }

    // Rule 4: suppress before the walk, so no scheduled run can reach a
    // repository whose password is mid-flight.
    runtime.set_migration(Some(PendingMigration { destinations: pending, report }));
    let config = { runtime.store.lock().await.config().clone() };
    runtime.push_config(&config);
    runtime.publish_status().await;

    let background = Arc::clone(runtime);
    tokio::spawn(async move {
        migrate_all(&background, &mut rekey).await;
    });
    Ok(())
}

/// Walk every pending repository, moving each from the old password to the new.
///
/// Never returns an error: a repository that will not migrate is recorded as
/// failed, stays suppressed, and is reported to the user. Aborting the whole
/// walk because one bucket was unreachable would leave the rest stranded too.
pub async fn migrate_all(runtime: &Arc<Runtime>, rekey: &mut Rekey) {
    let ids: Vec<Uuid> = rekey.pending().map(|r| r.destination_id).collect();
    for destination_id in ids {
        let name = rekey
            .repositories()
            .iter()
            .find(|r| r.destination_id == destination_id)
            .map(|r| r.destination_name.clone())
            .unwrap_or_else(|| destination_id.to_string());

        match migrate_one(runtime, rekey, &destination_id).await {
            Ok(already) => {
                if let Err(e) = rekey.mark_migrated(&destination_id) {
                    tracing::warn!(error = %e, "could not record a successful migration");
                }
                runtime.record_event(
                    Event::info(
                        "repo.repassworded",
                        if already {
                            format!("\"{name}\" was already using the new passphrase.")
                        } else {
                            format!("\"{name}\" now uses the new passphrase.")
                        },
                    )
                    .with_destination(destination_id),
                );
            }
            Err(e) => {
                if let Err(mark) = rekey.mark_failed(&destination_id, e.to_string()) {
                    tracing::warn!(error = %mark, "could not record a failed migration");
                }
                runtime.record_event(
                    Event::new(
                        Severity::Error,
                        "repo.repassword_failed",
                        format!(
                            "\"{name}\" could not be moved to the new passphrase: {e} Its \
                             scheduled backups stay paused until it is."
                        ),
                    )
                    .with_destination(destination_id),
                );
            }
        }

        // Rewritten after every repository, so a crash costs at most one
        // repository's worth of re-work — and re-work is free, because each
        // step tries the new password first.
        let report = rekey.report();
        if let Err(e) = write_report(&runtime.paths, &report) {
            tracing::error!(error = %e, "could not update the migration report");
        }
        let pending = still_pending(&report);
        runtime.set_migration(Some(PendingMigration { destinations: pending, report }));
        let config = { runtime.store.lock().await.config().clone() };
        runtime.push_config(&config);
    }

    let report = rekey.report();
    if rekey.is_complete() {
        // Rule 3, the other end of it: the recovery backup has been load
        // bearing until exactly now, and only now is it safe to stop caring
        // about it. It is still not deleted — vault backups are pruned by
        // their own policy — but the report that pinned it can go.
        clear_report(&runtime.paths);
        runtime.set_migration(None);
        runtime.record_event(Event::info(
            "vault.rekey_complete",
            "Every repository now uses the new master passphrase.",
        ));
    } else {
        runtime.record_event(Event::new(
            Severity::Warning,
            "vault.rekey_incomplete",
            format!(
                "{} of {} repositories still use the old passphrase and stay paused. Fix the \
                 problem and change the passphrase again with both passphrases to retry.",
                report.pending + report.failed,
                report.total
            ),
        ));
    }
    let config = { runtime.store.lock().await.config().clone() };
    runtime.push_config(&config);
    runtime.publish_status().await;
}

/// Move one repository. Returns `Ok(true)` when it was already migrated.
///
/// Rule 2 lives here: the new password is tried *first*. Connecting with it
/// successfully proves the repository has already moved, which makes running
/// this function twice harmless — and that is precisely what makes the whole
/// walk safe to resume after a crash, a dropped Wi-Fi connection, or a laptop
/// lid closing.
async fn migrate_one(
    runtime: &Arc<Runtime>,
    rekey: &Rekey,
    destination_id: &Uuid,
) -> Result<bool> {
    let credentials = rekey.credentials(destination_id)?;
    let binary = runtime.kopia().ok_or(Error::KopiaMissing)?;
    let destination = {
        let store = runtime.store.lock().await;
        store
            .config()
            .destination(destination_id)
            .cloned()
            .ok_or_else(|| {
                Error::Validation(format!(
                    "destination {destination_id} is no longer in the configuration"
                ))
            })?
    };

    let with_new = driver_with(&binary, runtime, &destination, &credentials.new)?;
    let ctx = RunContext::new();
    if with_new.connect_repository(&ctx).await.is_ok() {
        return Ok(true);
    }

    let with_old = driver_with(&binary, runtime, &destination, &credentials.old)?;
    with_old.connect_repository(&ctx).await.map_err(|e| {
        Error::Kopia { status: e.status.unwrap_or(-1), stderr: e.message }
    })?;
    with_old
        .change_password(&credentials.new, &ctx)
        .await
        .map_err(|e| Error::Kopia { status: e.status.unwrap_or(-1), stderr: e.message })?;

    // Prove it before claiming it: a `change-password` that reported success
    // and did not take would otherwise be recorded as migrated and the old
    // password forgotten.
    let verify = driver_with(&binary, runtime, &destination, &credentials.new)?;
    verify.connect_repository(&ctx).await.map_err(|e| Error::Kopia {
        status: e.status.unwrap_or(-1),
        stderr: format!("the repository did not accept the new passphrase afterwards: {}", e.message),
    })?;
    Ok(false)
}

/// A driver bound to one destination with an explicit passphrase, bypassing
/// the vault.
///
/// The migration is the one place where the passphrase to use is *not* the one
/// the vault would resolve — that is the whole problem being solved — so this
/// deliberately does not go through
/// [`superbackup_core::config::destination_passphrase`].
fn driver_with(
    binary: &superbackup_core::kopia::KopiaBinary,
    runtime: &Arc<Runtime>,
    destination: &Destination,
    passphrase: &Secret,
) -> Result<KopiaDriver> {
    let secrets = superbackup_core::kopia::DestinationSecrets::with_passphrase(Secret::new(
        passphrase.expose().to_vec(),
    ));
    // Object-store credentials are unaffected by a master rotation — only the
    // repository password changes — so a derived-passphrase destination on S3
    // still needs its access keys. Those come from the vault, which is open.
    KopiaDriver::new(binary.clone(), &runtime.paths, destination, None, secrets).map_err(|e| {
        Error::Kopia { status: e.status.unwrap_or(-1), stderr: e.message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use superbackup_core::crypto::rekey::RepositoryMigration;

    fn report(states: &[MigrationState]) -> MigrationReport {
        MigrationReport {
            vault_id: Uuid::new_v4(),
            total: states.len(),
            migrated: states.iter().filter(|s| **s == MigrationState::Migrated).count(),
            failed: states.iter().filter(|s| **s == MigrationState::Failed).count(),
            pending: states.iter().filter(|s| **s == MigrationState::Pending).count(),
            repositories: states
                .iter()
                .map(|state| RepositoryMigration {
                    destination_id: Uuid::new_v4(),
                    destination_name: "d".into(),
                    location: "/tmp".into(),
                    state: *state,
                    last_error: None,
                })
                .collect(),
            recovery_backup: None,
            old_signer_fingerprint: "old".into(),
            new_signer_fingerprint: "new".into(),
        }
    }

    #[test]
    fn a_failed_repository_still_counts_as_pending() {
        let r = report(&[MigrationState::Migrated, MigrationState::Failed]);
        assert_eq!(still_pending(&r).len(), 1);
    }

    #[test]
    fn a_fully_migrated_report_suppresses_nothing() {
        let r = report(&[MigrationState::Migrated, MigrationState::Migrated]);
        assert!(still_pending(&r).is_empty());
    }

    #[test]
    fn the_report_round_trips_through_disk() {
        let root = std::env::temp_dir().join(format!("sb-rekey-{}", uuid::Uuid::new_v4()));
        let paths = superbackup_core::paths::Paths::rooted_at(&root, false);
        paths.ensure().expect("dirs");
        let written = report(&[MigrationState::Pending]);
        write_report(&paths, &written).expect("write");
        let read = read_report(&paths).expect("read back");
        assert_eq!(read.total, 1);
        assert_eq!(read.repositories[0].state, MigrationState::Pending);
        clear_report(&paths);
        assert!(read_report(&paths).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
