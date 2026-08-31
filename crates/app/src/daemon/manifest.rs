//! Writing the machine manifest next to the backups.
//!
//! [`superbackup_core::platform::identity`] knows how to write
//! `<root>/_superbackup/machines/<uuid>.json` and the human-readable
//! `README.txt` beside it. This module decides *when*, which is the part with
//! the product judgement in it.
//!
//! ## Why it exists
//!
//! At recovery time a person is looking at a drive or a share holding several
//! opaque folders named `studio-1a2b3c4d`, `laptop-9f8e7d6c`, and kopia blobs
//! with meaningless names. The manifest is the only thing on that medium that
//! says which computer each folder belongs to, what it was called, and when it
//! last wrote anything. It costs a few hundred bytes and it is the difference
//! between a recoverable situation and a guessing game.
//!
//! ## The three rules
//!
//! 1. **Refreshed on every run, not only the first.** `last_seen` is the most
//!    useful field on the record and a stale one is worse than no record: it
//!    would tell somebody a machine is still backing up here when it stopped
//!    six months ago. [`write_manifest`] is idempotent and preserves
//!    `first_seen`, so calling it every time is both correct and cheap.
//! 2. **It can never fail a backup.** This is a convenience for a future
//!    human, not part of the data path. Every failure becomes a warning on the
//!    destination and the run carries on. A read-only medium, a full disk, or
//!    a permission problem must not cost the user their backup.
//! 3. **It is optional.** The record contains no secret and no file name, but
//!    it *is* identifying — a label, a hostname, an OS version — and a shared
//!    bucket or a family drive is exactly where somebody might not want that.
//!    [`Settings::write_machine_manifest`] defaults to on and can be switched
//!    off.
//!
//! ## What it cannot do, stated rather than hidden
//!
//! [`write_manifest`] takes a filesystem path. An S3 or StorJ destination has
//! no local root, and kopia exposes no way to put an arbitrary object into the
//! bucket it manages — `kopia repository` commands write only the blobs kopia
//! itself owns. So **an object-storage destination gets no manifest**, and
//! [`ManifestOutcome::Unsupported`] is what the interface renders next to that
//! destination. It is not skipped silently and it is not faked.

use superbackup_core::model::{Destination, DestinationKind, MachineIdentity, Settings};
use superbackup_core::platform::identity;

/// What happened when a run tried to leave its calling card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestOutcome {
    /// Written or refreshed at this path.
    Written(String),
    /// The user switched the setting off.
    Disabled,
    /// A rehearsal writes nothing anywhere, including this.
    Rehearsal,
    /// This destination kind has no filesystem root to write to.
    Unsupported,
    /// Tried and failed. The backup is unaffected; the string is a warning.
    Failed(String),
}

impl ManifestOutcome {
    /// The warning to attach to the destination's run, when there is one.
    ///
    /// Only a genuine failure produces one: "switched off" and "not supported
    /// here" are answers, not problems, and a warning per destination per run
    /// for either would be noise that trains users to ignore warnings.
    pub fn warning(&self) -> Option<String> {
        match self {
            ManifestOutcome::Failed(reason) => Some(reason.clone()),
            _ => None,
        }
    }
}

/// Write or refresh this machine's record at a destination.
///
/// Blocking filesystem work, so callers on an async executor should wrap it in
/// `spawn_blocking`; [`write_for_destination`] does.
pub fn write_now(
    destination: &Destination,
    identity: &MachineIdentity,
    settings: &Settings,
    rehearsal: bool,
) -> ManifestOutcome {
    if rehearsal {
        return ManifestOutcome::Rehearsal;
    }
    if !settings.write_machine_manifest {
        return ManifestOutcome::Disabled;
    }
    let root = match &destination.kind {
        DestinationKind::LocalRepository { path }
        | DestinationKind::OneDrive { path, .. }
        | DestinationKind::LocalMirror { path } => path.clone(),
        DestinationKind::S3 { .. } => return ManifestOutcome::Unsupported,
    };
    match identity::write_manifest(&root, identity) {
        Ok(_) => ManifestOutcome::Written(
            identity::record_path(&root, &identity.id).display().to_string(),
        ),
        Err(e) => ManifestOutcome::Failed(format!(
            "Could not write the machine manifest at \"{}\": {e}. The backup itself is \
             unaffected.",
            destination.name
        )),
    }
}

/// The async wrapper a run uses. Never returns an error: see rule 2.
pub async fn write_for_destination(
    destination: &Destination,
    identity: &MachineIdentity,
    settings: &Settings,
    rehearsal: bool,
) -> ManifestOutcome {
    let destination = destination.clone();
    let identity = identity.clone();
    let settings = settings.clone();
    tokio::task::spawn_blocking(move || write_now(&destination, &identity, &settings, rehearsal))
        .await
        .unwrap_or_else(|_| {
            ManifestOutcome::Failed("The machine manifest write did not complete.".to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn destination(kind: DestinationKind) -> Destination {
        Destination {
            id: Uuid::new_v4(),
            name: "Archive".into(),
            kind,
            encryption: None,
            passphrase_ref: None,
            retention: Default::default(),
            enabled: true,
            auto_discovered: false,
            bandwidth: None,
            replicate_from: None,
            created_at: Utc::now(),
            last_verified_at: None,
        }
    }

    fn identity() -> MachineIdentity {
        MachineIdentity { label: "Studio".into(), ..MachineIdentity::default() }
    }

    #[test]
    fn a_run_leaves_a_record_and_refreshing_it_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("sb-manifest-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let dest = destination(DestinationKind::LocalRepository { path: dir.clone() });
        let id = identity();
        let settings = Settings::default();

        let first = write_now(&dest, &id, &settings, false);
        assert!(matches!(first, ManifestOutcome::Written(_)), "{first:?}");
        let second = write_now(&dest, &id, &settings, false);
        assert!(matches!(second, ManifestOutcome::Written(_)), "{second:?}");

        let machines = identity::list_machines_for(&dest).expect("list");
        assert_eq!(machines.len(), 1, "refreshing must not add a second record");
        assert_eq!(machines[0].label, "Studio");
        assert_eq!(machines[0].first_seen, machines[0].first_seen.min(machines[0].last_seen));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rehearsal_and_a_switched_off_setting_both_write_nothing() {
        let dir = std::env::temp_dir().join(format!("sb-manifest-{}", Uuid::new_v4().simple()));
        let dest = destination(DestinationKind::LocalMirror { path: dir.clone() });
        let id = identity();

        assert_eq!(write_now(&dest, &id, &Settings::default(), true), ManifestOutcome::Rehearsal);
        let off = Settings { write_machine_manifest: false, ..Settings::default() };
        assert_eq!(write_now(&dest, &id, &off, false), ManifestOutcome::Disabled);
        assert!(!dir.exists(), "nothing may be created by either path");
    }

    #[test]
    fn object_storage_is_reported_as_unsupported_not_skipped() {
        let dest = destination(DestinationKind::S3 {
            provider_id: Uuid::new_v4(),
            bucket: "backups".into(),
            prefix: String::new(),
            credential_override: None,
        });
        let outcome = write_now(&dest, &identity(), &Settings::default(), false);
        assert_eq!(outcome, ManifestOutcome::Unsupported);
        // And it is not a warning: it is a permanent property of the kind.
        assert!(outcome.warning().is_none());
        assert!(identity::list_machines_for(&dest).expect("list").is_empty());
    }

    #[test]
    fn only_a_real_failure_becomes_a_warning() {
        let failed = ManifestOutcome::Failed("disk full".into());
        assert_eq!(failed.warning().as_deref(), Some("disk full"));
        assert!(ManifestOutcome::Disabled.warning().is_none());
        assert!(ManifestOutcome::Rehearsal.warning().is_none());
    }
}
