//! Finding Google Drive for Desktop, and being honest about what it is.
//!
//! # Why a folder and not the Drive API
//!
//! kopia ships a `gdrive` backend. We deliberately do not use it, for two
//! reasons that both matter more than the convenience of a native integration:
//!
//! * kopia labels it **`[Not maintained]`** in its own `--help`. So does its
//!   `rclone` backend. Putting someone's only offsite copy behind an
//!   unmaintained storage driver is not a trade worth making.
//! * It authenticates with a **service account**, and files created by a
//!   service account are *owned* by it. A consumer Google account's storage —
//!   the 2 TB or 5 TB someone pays for — is not available to a service
//!   account, which has no Drive quota of its own. Backing up "to Google
//!   Drive" that way fills a quota the user does not have rather than the one
//!   they bought.
//!
//! Google Drive for Desktop mounts the user's own Drive as a filesystem, as
//! themselves, against their own quota. kopia's filesystem backend — the one
//! that *is* maintained, and the one every local and OneDrive destination
//! already uses — then works unchanged.
//!
//! # The hazard this module exists to catch
//!
//! Drive for Desktop defaults to **streaming**: files are placeholders, and
//! opening one fetches it. That is the same failure OneDrive's Files On-Demand
//! causes, and it is worse for a repository than for documents, because kopia
//! reads its index and format blobs on every operation. A repository in a
//! streamed folder is slow at best and stalls at worst.
//!
//! So detection reports the mode, and the interface refuses to pretend a
//! streamed folder is a good place for a repository.
//!
//! # What is *not* claimed
//!
//! Detection was written against Google's documented layouts and could not be
//! exercised against a live client on the machine it was written on, because
//! none was installed. Every route degrades to "not found", which is a correct
//! answer that leaves the user able to type a path by hand.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::onedrive::SyncState;
use crate::model::DestinationKind;

/// Folder created inside the Drive root, mirroring OneDrive's.
pub const SUGGESTED_FOLDER: &str = "Superbackup";

/// Drive for Desktop puts the user's own files under this name inside the
/// mount. The mount root itself also holds "Shared drives" and "Other
/// computers", neither of which is a sane backup target.
///
/// Only the platforms with an official client look for it; Google ships none
/// for Linux.
#[cfg(any(windows, target_os = "macos"))]
const MY_DRIVE: &str = "My Drive";

/// How Drive for Desktop is presenting the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveMode {
    /// Files are real, on this disk. What a repository needs.
    Mirrored,
    /// Files are placeholders fetched on access. Hostile to a repository.
    Streamed,
    /// Could not be determined. Reported as such rather than guessed.
    Unknown,
}

impl DriveMode {
    pub fn is_risky(&self) -> bool {
        matches!(self, DriveMode::Streamed)
    }

    /// Mapped onto the shared vocabulary the destination editor already
    /// speaks, so the placeholder warnings written for OneDrive apply here
    /// without a second set of copy saying the same thing differently.
    pub fn sync_state(&self) -> SyncState {
        match self {
            DriveMode::Mirrored => SyncState::AlwaysAvailable,
            DriveMode::Streamed => SyncState::OnlineOnly,
            DriveMode::Unknown => SyncState::Unknown,
        }
    }
}

/// One Google Drive for Desktop account mounted on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleDriveAccount {
    /// The writable root — the `My Drive` folder, not the mount point.
    pub path: PathBuf,
    /// What a picker shows, e.g. `Google Drive (andreas@example.com)`.
    pub display_name: String,
    #[serde(default)]
    pub email: Option<String>,
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub mode: DriveMode,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl GoogleDriveAccount {
    /// One obvious folder inside Drive, never the root: a repository at the
    /// root would mix opaque blobs into the user's own files and make
    /// "delete the backup" a dangerous operation.
    pub fn suggested_repository_root(&self) -> PathBuf {
        self.path.join(SUGGESTED_FOLDER)
    }

    /// Stored as a plain local repository.
    ///
    /// Deliberately **not** a new `DestinationKind`. What a destination needs
    /// to know is where the folder is and that kopia's filesystem backend
    /// drives it — which is exactly `LocalRepository`. Adding a variant would
    /// be a schema change, would need a migration, and would buy nothing that
    /// the detection warnings do not already provide.
    pub fn to_destination_kind(&self, path: PathBuf) -> DestinationKind {
        DestinationKind::LocalRepository { path }
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Every Google Drive for Desktop mount this user has.
///
/// Never fails. A machine without the client returns an empty vector, which is
/// a correct answer and must not block anything.
pub fn detect() -> Vec<GoogleDriveAccount> {
    let mut found = detect_raw();
    found.retain(|a| a.path.is_dir());
    dedupe_by_path(&mut found);
    for account in &mut found {
        enrich(account);
    }
    found
}

fn detect_raw() -> Vec<GoogleDriveAccount> {
    #[cfg(windows)]
    {
        let mut found = detect_windows_mounts();
        found.extend(detect_legacy_folder());
        found
    }
    #[cfg(target_os = "macos")]
    {
        let mut found = detect_macos_cloudstorage();
        found.extend(detect_legacy_folder());
        found
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        // Google ships no Linux client. A third-party mount (rclone, ocamlfuse)
        // is a plain folder the user can point at directly, and guessing at
        // one would risk naming something that is not Drive at all.
        Vec::new()
    }
}

/// Drive for Desktop mounts a virtual volume, by default `G:`, whose label is
/// "Google Drive". Walk the fixed and removable roots and look for it.
///
/// The registry is not used: the mount point lives in DriveFS's own
/// `root_preference_sqlite.db`, and reading another program's SQLite file
/// while it is running is a good way to get a locked or half-written answer.
/// The volume label is a documented, stable, read-only signal.
#[cfg(windows)]
fn detect_windows_mounts() -> Vec<GoogleDriveAccount> {
    let mut found = Vec::new();
    // Only drives that exist, and only the kinds a Drive mount can be.
    //
    // Walking `A:` to `Z:` and stat-ing each one froze the interface for
    // several seconds: probing an empty floppy or optical drive blocks until
    // the device times out, and `A:`/`B:` are the worst offenders. The
    // bitmask says which letters exist without touching any of them, and the
    // drive type rules out removable media before anything is opened.
    for root in super::mounted_drive_roots() {
        let Some(label) = super::volume_label(&root) else { continue };
        if !label.eq_ignore_ascii_case("Google Drive") {
            continue;
        }
        // The mount root holds "My Drive", "Shared drives" and "Other
        // computers". Only the first is the user's own writable storage.
        let my_drive = root.join(MY_DRIVE);
        if my_drive.is_dir() {
            found.push(new_account(my_drive, None));
        }
    }
    found
}

/// The current macOS layout: `~/Library/CloudStorage/GoogleDrive-<email>/My Drive`.
/// The older `/Volumes/GoogleDrive` is covered too, for machines that have not
/// migrated.
#[cfg(target_os = "macos")]
fn detect_macos_cloudstorage() -> Vec<GoogleDriveAccount> {
    let mut found = Vec::new();
    if let Some(home) = dirs_home() {
        let cloud = home.join("Library/CloudStorage");
        if let Ok(entries) = std::fs::read_dir(&cloud) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(email) = name.strip_prefix("GoogleDrive-") else { continue };
                let my_drive = entry.path().join(MY_DRIVE);
                if my_drive.is_dir() {
                    found.push(new_account(my_drive, Some(email.to_string())));
                }
            }
        }
    }
    let legacy = PathBuf::from("/Volumes/GoogleDrive").join(MY_DRIVE);
    if legacy.is_dir() {
        found.push(new_account(legacy, None));
    }
    found
}

/// Legacy Backup and Sync, and mirrored mode, both put a real folder at
/// `~/Google Drive`.
#[cfg(any(windows, target_os = "macos"))]
fn detect_legacy_folder() -> Vec<GoogleDriveAccount> {
    let Some(home) = dirs_home() else { return Vec::new() };
    let path = home.join("Google Drive");
    if path.is_dir() {
        vec![new_account(path, None)]
    } else {
        Vec::new()
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

fn new_account(path: PathBuf, email: Option<String>) -> GoogleDriveAccount {
    let display_name = match &email {
        Some(e) => format!("Google Drive ({e})"),
        None => "Google Drive".to_string(),
    };
    GoogleDriveAccount {
        path,
        display_name,
        email,
        available_bytes: 0,
        total_bytes: 0,
        mode: DriveMode::Unknown,
        warnings: Vec::new(),
    }
}

fn dedupe_by_path(accounts: &mut Vec<GoogleDriveAccount>) {
    let mut seen: Vec<PathBuf> = Vec::new();
    accounts.retain(|a| {
        let key = a.path.clone();
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
}

/// Free space, streaming mode, and the warnings that follow from both.
fn enrich(account: &mut GoogleDriveAccount) {
    if let Some((available, total)) = super::disk_space(&account.path) {
        account.available_bytes = available;
        account.total_bytes = total;
    }
    account.mode = detect_mode(&account.path);

    if account.mode == DriveMode::Streamed {
        account.warnings.push(
            "This Drive is in streaming mode, so files here are placeholders that are fetched \
             when opened. A backup repository is read on every operation, so it would be slow \
             and can stall entirely. Switch Drive for Desktop to mirroring, or choose another \
             destination."
                .to_string(),
        );
    }
    if account.mode == DriveMode::Unknown {
        account.warnings.push(
            "superbackup could not tell whether this Drive mirrors files locally or streams \
             them. If Drive for Desktop is set to stream, a repository here will be slow and \
             may stall."
                .to_string(),
        );
    }
    // Drive's quota is the account's, not the disk's, and in streaming mode
    // the reported volume size is a virtual figure rather than real free
    // space. Saying so is better than quoting a number that means nothing.
    if account.mode == DriveMode::Streamed {
        account.warnings.push(
            "Free space shown for a streamed Drive is a virtual figure and does not reflect \
             your Google storage quota. Check your quota in Drive before backing up."
                .to_string(),
        );
    }
}

/// Is this folder mirrored to real local files, or streamed?
///
/// A streamed Drive is a virtual filesystem: on Windows it is not `NTFS`, and
/// on both platforms its reported volume size is synthetic. The check is
/// therefore "does this path live on a normal local volume", which is exactly
/// the question that matters for putting a repository on it.
fn detect_mode(path: &Path) -> DriveMode {
    #[cfg(windows)]
    {
        match super::filesystem_name(path).as_deref() {
            // Drive for Desktop's virtual volume.
            Some(name) if name.eq_ignore_ascii_case("DriveFS") => DriveMode::Streamed,
            Some(name)
                if name.eq_ignore_ascii_case("NTFS") || name.eq_ignore_ascii_case("ReFS") =>
            {
                DriveMode::Mirrored
            }
            _ => DriveMode::Unknown,
        }
    }
    #[cfg(not(windows))]
    {
        // `~/Library/CloudStorage/...` is always the streaming client on
        // macOS; a plain `~/Google Drive` is the mirrored legacy layout.
        let text = path.to_string_lossy();
        if text.contains("CloudStorage") || text.starts_with("/Volumes/GoogleDrive") {
            DriveMode::Streamed
        } else if path.is_dir() {
            DriveMode::Mirrored
        } else {
            DriveMode::Unknown
        }
    }
}

#[cfg(test)]
mod tests {

    /// Detection runs on the interface thread when the destination editor
    /// opens, so it has to be cheap.
    ///
    /// The first version walked `A:` to `Z:` and stat-ed every letter, which
    /// froze the window for seconds: opening an empty floppy or optical drive
    /// blocks until the device times out. The mounted-letters bitmask answers
    /// which drives exist without touching any of them.
    #[test]
    fn detection_is_fast_enough_to_run_while_drawing_a_frame() {
        let started = std::time::Instant::now();
        let _ = detect();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(400),
            "detection took {elapsed:?}, which is long enough for the user to see it"
        );
    }
    use super::*;

    #[test]
    fn detection_never_panics_and_never_invents_an_account() {
        // The machine running this may or may not have Drive for Desktop. The
        // only universal truth is that every reported account must exist on
        // disk and have a usable root — a picker entry that points at nothing
        // is worse than no entry at all.
        for account in detect() {
            assert!(account.path.is_dir(), "reported a path that is not a directory");
            assert!(!account.display_name.is_empty());
        }
    }

    #[test]
    fn the_suggested_root_is_inside_drive_and_never_the_root_itself() {
        let account = new_account(PathBuf::from("/tmp/gdrive/My Drive"), None);
        let root = account.suggested_repository_root();
        assert!(root.starts_with(&account.path));
        assert_ne!(root, account.path, "a repository must never be written at the Drive root");
        assert!(root.ends_with(SUGGESTED_FOLDER));
    }

    #[test]
    fn a_streamed_drive_is_reported_as_risky_and_explains_itself() {
        assert!(DriveMode::Streamed.is_risky());
        assert!(!DriveMode::Mirrored.is_risky());
        // Unknown is deliberately *not* risky: refusing a folder we could not
        // classify would block every third-party and unusual setup.
        assert!(!DriveMode::Unknown.is_risky());
    }

    #[test]
    fn a_streamed_drive_maps_onto_the_placeholder_vocabulary_the_editor_speaks() {
        // The destination editor already refuses to put a repository on an
        // online-only folder. Streaming is that same hazard, so it maps onto
        // the same value rather than needing a parallel set of warnings.
        assert_eq!(DriveMode::Streamed.sync_state(), SyncState::OnlineOnly);
        assert_eq!(DriveMode::Mirrored.sync_state(), SyncState::AlwaysAvailable);
        assert!(DriveMode::Streamed.sync_state().is_risky());
    }

    #[test]
    fn a_drive_is_stored_as_an_ordinary_local_repository() {
        // No new DestinationKind: what matters is that kopia's filesystem
        // backend drives it, which is what LocalRepository already means.
        let account = new_account(PathBuf::from("/tmp/gdrive/My Drive"), None);
        let kind = account.to_destination_kind(account.suggested_repository_root());
        assert!(matches!(kind, DestinationKind::LocalRepository { .. }));
        assert!(kind.is_repository());
    }

    #[test]
    fn duplicate_mounts_are_reported_once() {
        let mut accounts = vec![
            new_account(PathBuf::from("/tmp/g/My Drive"), None),
            new_account(PathBuf::from("/tmp/g/My Drive"), Some("a@b.c".into())),
            new_account(PathBuf::from("/tmp/other/My Drive"), None),
        ];
        dedupe_by_path(&mut accounts);
        assert_eq!(accounts.len(), 2);
    }

    #[test]
    fn an_account_with_an_email_says_which_one_it_is() {
        // With two Drives mounted, "Google Drive" twice is useless.
        let a = new_account(PathBuf::from("/tmp/x"), Some("andreas@example.com".into()));
        assert!(a.display_name.contains("andreas@example.com"));
    }
}
