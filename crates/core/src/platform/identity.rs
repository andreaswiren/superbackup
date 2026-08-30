//! Machine identity, and the self-describing manifest written into every
//! destination root.
//!
//! # Why a random UUID and not a hardware serial
//!
//! The obvious way to identify a PC is to hash the motherboard UUID, the disk
//! serial, or the MAC address. We deliberately do not. Those values are stable
//! across reinstalls and *across applications*, which makes them a
//! cross-context tracking identifier — a fingerprint. A backup tool has no
//! business minting one. A random v4 UUID, generated once and stored next to
//! the configuration, is exactly as useful to us (it names a folder) and
//! useless to anybody correlating users across products. It also degrades
//! honestly: replacing the motherboard does not silently orphan the backup
//! folder, and cloning a VM produces a *new* identity the first time the clone
//! runs rather than two machines fighting over one folder.
//!
//! # Where the identity lives
//!
//! The UUID is stored on its own in `<config-dir>/machine-id`, **not** only in
//! `config.json`. `config.json` can be replaced wholesale by a pull from the
//! shared Git configuration repository (see [`crate::model::RemoteConfigSource`]),
//! and if the machine id travelled inside it, pulling a colleague's config
//! would silently rename this PC's destination folder and orphan every
//! existing snapshot. The id file is local, tiny, and never synced.
//!
//! # The on-destination layout
//!
//! ```text
//! <destination-root>/
//!   _superbackup/
//!     machines/<machine-uuid>.json   <- one record per PC writing here
//!     README.txt                     <- plain English, for a human browsing
//!   <machine-slug>/                  <- this PC's repository and mirrors
//! ```
//!
//! The point of the manifest is the moment someone opens a shared drive, a NAS
//! folder, or an S3 bucket and sees four opaque `<name>-a1b2c3d4` directories.
//! The README and the JSON records tell them whose is whose without needing
//! this application installed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, IoContext, Result};
use crate::model::{slugify, Destination, DestinationKind, MachineIdentity, MANIFEST_DIR};
use crate::paths::{write_atomic, Paths};
use crate::state::{Event, Severity};

/// Filename of the local, never-synced machine id.
pub const MACHINE_ID_FILE: &str = "machine-id";

/// Schema version of [`MachineRecord`]. Readers refuse nothing — unknown
/// fields are preserved via [`MachineRecord::extra`] — but a newer major
/// version is surfaced to the user rather than silently misread.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Number of hex characters of the UUID appended to a slug. Eight characters
/// of a v4 UUID is 32 bits: with a hundred machines sharing one destination the
/// chance of a collision is about one in a million, and a collision only
/// matters when the *labels* also match.
const SLUG_ID_CHARS: usize = 8;

// ---------------------------------------------------------------------------
// Building the identity
// ---------------------------------------------------------------------------

/// The on-disk folder name for a machine: `<slugified-label>-<id prefix>`.
///
/// The UUID suffix is what makes two PCs both called "Laptop" land in two
/// different folders. It is derived from the id, never from the label, so it
/// survives a rename.
pub fn slug_for(label: &str, id: &Uuid) -> String {
    let simple = id.simple().to_string();
    format!("{}-{}", slugify(label), &simple[..SLUG_ID_CHARS])
}

/// Read the persisted machine id, generating and storing one on first run.
///
/// A malformed or empty file is replaced rather than treated as fatal: losing
/// the id costs one orphaned folder, refusing to start costs every backup.
pub fn load_or_create_id(paths: &Paths) -> Result<Uuid> {
    let file = paths.config_dir.join(MACHINE_ID_FILE);
    if let Ok(text) = std::fs::read_to_string(&file) {
        if let Ok(id) = Uuid::parse_str(text.trim()) {
            return Ok(id);
        }
        tracing::warn!(path = %file.display(), "machine-id file is unreadable; minting a new id");
    }
    let id = Uuid::new_v4();
    std::fs::create_dir_all(&paths.config_dir)
        .ctx(format!("creating {}", paths.config_dir.display()))?;
    write_atomic(&file, id.to_string().as_bytes())?;
    Ok(id)
}

/// Build a complete [`MachineIdentity`] for this PC, persisting the id.
///
/// `label` defaults to the hostname, which is what almost every user wants and
/// what makes the destination folder recognisable without any setup.
pub fn detect(paths: &Paths) -> Result<MachineIdentity> {
    let id = load_or_create_id(paths)?;
    let hostname = detect_hostname();
    let label = hostname.clone();
    Ok(MachineIdentity {
        id,
        slug: slug_for(&label, &id),
        label,
        hostname,
        os: std::env::consts::OS.to_string(),
        os_version: detect_os_version(),
        arch: std::env::consts::ARCH.to_string(),
        username: detect_username(),
        created_at: Utc::now(),
    })
}

/// Refresh the volatile facts (hostname, OS build, logged-in user) on an
/// identity loaded from `config.json`, leaving id, label and slug alone.
///
/// This runs at every start: a Windows feature update changes the build
/// number, and a renamed PC changes its hostname, but neither may move the
/// destination folder.
pub fn refresh(identity: &mut MachineIdentity) -> Vec<Event> {
    let mut events = Vec::new();
    let hostname = detect_hostname();
    if hostname != identity.hostname {
        events.push(
            Event::info(
                "machine.hostname_changed",
                format!("Hostname changed from {} to {}", identity.hostname, hostname),
            )
            .with_field("previous", identity.hostname.clone())
            .with_field("current", hostname.clone()),
        );
        identity.hostname = hostname;
    }
    let os_version = detect_os_version();
    if os_version != identity.os_version {
        events.push(
            Event::info("machine.os_updated", format!("Operating system is now {os_version}"))
                .with_field("previous", identity.os_version.clone())
                .with_field("current", os_version.clone()),
        );
        identity.os_version = os_version;
    }
    identity.os = std::env::consts::OS.to_string();
    identity.arch = std::env::consts::ARCH.to_string();
    identity.username = detect_username();

    // Self-heal an identity written by an older build, or one whose slug was
    // hand-edited. The slug must always be derivable from the id.
    let expected_suffix = &identity.id.simple().to_string()[..SLUG_ID_CHARS];
    if !identity.slug.ends_with(expected_suffix) {
        let repaired = slug_for(&identity.label, &identity.id);
        events.push(
            Event::warn(
                "machine.slug_repaired",
                format!(
                    "Destination folder name {} did not match this machine's id; using {repaired}",
                    identity.slug
                ),
            )
            .with_field("previous", identity.slug.clone()),
        );
        identity.slug = repaired;
    }
    events
}

/// Rename a machine **without** moving its destination folder.
///
/// This is the whole reason `slug` is stored rather than recomputed: a user who
/// renames "DESKTOP-8H2K1L" to "Andreas' Studio PC" must not end up with two
/// half-populated folders and a repository that kopia can no longer find. The
/// new label is recorded in the destination manifest so a human browsing the
/// drive still sees the current name.
///
/// Returns `None` when the label is unchanged after trimming.
pub fn rename(identity: &mut MachineIdentity, new_label: &str) -> Option<Event> {
    let new_label = new_label.trim();
    if new_label.is_empty() || new_label == identity.label {
        return None;
    }
    let previous = std::mem::replace(&mut identity.label, new_label.to_string());
    Some(
        Event::info(
            "machine.renamed",
            format!("This machine is now called \"{new_label}\" (folder name unchanged)"),
        )
        .with_field("previous_label", previous)
        .with_field("slug", identity.slug.clone()),
    )
}

fn detect_hostname() -> String {
    hostname::get()
        .ok()
        .map(|h| h.to_string_lossy().into_owned())
        .filter(|h| !h.trim().is_empty())
        .or_else(|| whoami::devicename().ok())
        .unwrap_or_else(|| "unknown-host".to_string())
}

fn detect_username() -> String {
    whoami::username().unwrap_or_else(|_| "unknown".to_string())
}

/// A specific, human-meaningful OS version string.
///
/// "windows" is useless in a support ticket; "Windows 11 Pro 24H2 (build
/// 26200.7019)" is not.
pub fn detect_os_version() -> String {
    #[cfg(windows)]
    {
        super::win32::os_version()
    }
    #[cfg(target_os = "linux")]
    {
        linux_os_version()
    }
    #[cfg(target_os = "macos")]
    {
        macos_os_version()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        // Every other Unix (BSD, illumos). `distro()` reads /etc/os-release
        // where it exists and degrades to the platform name where it does not.
        whoami::distro().unwrap_or_else(|_| std::env::consts::OS.to_string())
    }
}

#[cfg(target_os = "linux")]
fn linux_os_version() -> String {
    // `/etc/os-release` is the only cross-distribution contract there is.
    let pretty = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| parse_os_release_pretty_name(&text))
        .or_else(|| whoami::distro().ok())
        .unwrap_or_else(|| "Linux".to_string());
    match std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        Ok(kernel) if !kernel.trim().is_empty() => format!("{pretty} (kernel {})", kernel.trim()),
        _ => pretty,
    }
}

/// Pull `PRETTY_NAME` out of an `os-release` file, handling quoting.
/// Kept separate and `pub(crate)` so it can be tested on any platform.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_os_release_pretty_name(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
            let cleaned = rest.trim().trim_matches('"').trim_matches('\'');
            if !cleaned.is_empty() {
                return Some(cleaned.to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn macos_os_version() -> String {
    // Reading the plist avoids spawning a process, which matters inside a
    // LaunchDaemon where the environment is minimal.
    const PLIST: &str = "/System/Library/CoreServices/SystemVersion.plist";
    if let Ok(text) = std::fs::read_to_string(PLIST) {
        if let Some(v) = parse_plist_string(&text, "ProductVersion") {
            let build = parse_plist_string(&text, "ProductBuildVersion");
            return match build {
                Some(b) => format!("macOS {v} ({b})"),
                None => format!("macOS {v}"),
            };
        }
    }
    match std::process::Command::new("sw_vers").arg("-productVersion").output() {
        Ok(out) if out.status.success() => {
            format!("macOS {}", String::from_utf8_lossy(&out.stdout).trim())
        }
        _ => "macOS".to_string(),
    }
}

/// Minimal `<key>K</key><string>V</string>` scraper. Good enough for
/// SystemVersion.plist, which is a fixed, Apple-controlled file; deliberately
/// not a general plist parser.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_plist_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("<key>{key}</key>");
    let after = text.split_once(&needle)?.1;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")?;
    Some(after[start..start + end].trim().to_string())
}

// ---------------------------------------------------------------------------
// The destination manifest
// ---------------------------------------------------------------------------

/// One machine's record inside a destination's `_superbackup/machines/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineRecord {
    #[serde(default = "default_manifest_schema")]
    pub schema_version: u32,
    pub id: Uuid,
    pub label: String,
    pub hostname: String,
    pub os: String,
    #[serde(default)]
    pub os_version: String,
    pub arch: String,
    /// The folder under the destination root that belongs to this machine.
    pub slug: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    /// Version of superbackup that last wrote this record.
    pub superbackup_version: String,
    /// Every label this machine has had, oldest first. Renaming keeps the
    /// folder, so without this a human browsing the drive would have no way to
    /// connect "andreas-desktop-4f2a…" to the PC now called "Studio".
    #[serde(default)]
    pub previous_labels: Vec<LabelChange>,
    /// Forward-compatibility bucket: fields written by a newer build survive a
    /// round trip through an older one.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn default_manifest_schema() -> u32 {
    MANIFEST_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelChange {
    pub label: String,
    pub changed_at: DateTime<Utc>,
}

impl MachineRecord {
    /// True when this record was written by a different machine than `me`.
    /// The GUI uses it for the "this destination also holds backups from 2
    /// other PCs" line, and to refuse a destructive operation on a folder that
    /// is not ours.
    pub fn is_foreign(&self, me: &Uuid) -> bool {
        &self.id != me
    }

    /// Days since this machine last wrote here, for a "last seen 94 days ago"
    /// badge next to a stale record.
    pub fn days_since_seen(&self, now: DateTime<Utc>) -> i64 {
        (now - self.last_seen).num_days().max(0)
    }

    fn from_identity(identity: &MachineIdentity, now: DateTime<Utc>) -> MachineRecord {
        MachineRecord {
            schema_version: MANIFEST_SCHEMA_VERSION,
            id: identity.id,
            label: identity.label.clone(),
            hostname: identity.hostname.clone(),
            os: identity.os.clone(),
            os_version: identity.os_version.clone(),
            arch: identity.arch.clone(),
            slug: identity.slug.clone(),
            first_seen: now,
            last_seen: now,
            superbackup_version: crate::VERSION.to_string(),
            previous_labels: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

/// `<root>/_superbackup`
pub fn manifest_dir(root: &Path) -> PathBuf {
    root.join(MANIFEST_DIR)
}

/// `<root>/_superbackup/machines`
pub fn machines_dir(root: &Path) -> PathBuf {
    manifest_dir(root).join("machines")
}

/// `<root>/_superbackup/README.txt`
pub fn readme_path(root: &Path) -> PathBuf {
    manifest_dir(root).join("README.txt")
}

/// `<root>/_superbackup/machines/<uuid>.json`
pub fn record_path(root: &Path, id: &Uuid) -> PathBuf {
    machines_dir(root).join(format!("{id}.json"))
}

/// The folder this machine owns inside a destination root.
pub fn machine_root(root: &Path, identity: &MachineIdentity) -> PathBuf {
    root.join(&identity.slug)
}

/// Create or update this machine's record, and rewrite the human README.
///
/// Idempotent, and safe to call on every run. `first_seen` is preserved from
/// any existing record; a changed label is appended to the history rather than
/// overwriting it.
pub fn write_manifest(root: &Path, identity: &MachineIdentity) -> Result<MachineRecord> {
    write_manifest_at(root, identity, Utc::now())
}

/// Testable core of [`write_manifest`] with an injected clock.
pub fn write_manifest_at(
    root: &Path,
    identity: &MachineIdentity,
    now: DateTime<Utc>,
) -> Result<MachineRecord> {
    let dir = machines_dir(root);
    std::fs::create_dir_all(&dir).ctx(format!("creating {}", dir.display()))?;

    let path = record_path(root, &identity.id);
    let mut record = match read_record(&path) {
        Some(mut existing) => {
            if existing.label != identity.label {
                existing
                    .previous_labels
                    .push(LabelChange { label: existing.label.clone(), changed_at: now });
                // Bound the history: a script renaming the machine in a loop
                // must not grow the record without limit.
                let len = existing.previous_labels.len();
                if len > 16 {
                    existing.previous_labels.drain(..len - 16);
                }
            }
            existing.label = identity.label.clone();
            existing.hostname = identity.hostname.clone();
            existing.os = identity.os.clone();
            existing.os_version = identity.os_version.clone();
            existing.arch = identity.arch.clone();
            // The slug is never rewritten from the identity if it would move
            // an existing folder; instead we keep what is on the destination
            // and let `refresh` repair the local side.
            if existing.slug.is_empty() {
                existing.slug = identity.slug.clone();
            }
            existing.superbackup_version = crate::VERSION.to_string();
            existing
        }
        None => MachineRecord::from_identity(identity, now),
    };
    record.last_seen = now;
    record.schema_version = MANIFEST_SCHEMA_VERSION;

    let json = serde_json::to_vec_pretty(&record)
        .map_err(|e| Error::Internal(format!("serialising machine record: {e}")))?;
    write_atomic(&path, &json)?;

    // Rewrite the README with the full, current list. Best effort: a read-only
    // destination must still accept the record write above.
    if let Err(e) = write_readme(root) {
        tracing::warn!(error = %e, root = %root.display(), "could not refresh destination README");
    }
    Ok(record)
}

fn read_record(path: &Path) -> Option<MachineRecord> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<MachineRecord>(&bytes) {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "ignoring unreadable machine record");
            None
        }
    }
}

/// Every machine that has ever written to this destination, newest first.
///
/// Unreadable or foreign files are skipped with a warning rather than failing
/// the whole listing — a destination shared with a future version of
/// superbackup must still be usable.
pub fn list_machines(root: &Path) -> Result<Vec<MachineRecord>> {
    let dir = machines_dir(root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        // A destination nobody has written to yet holds no machines. That is
        // an answer, not an error.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(format!("listing {}", dir.display()), e)),
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip the atomic-write temporaries `paths::write_atomic` leaves
        // mid-flight, and anything that is not one of our records.
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        if let Some(record) = read_record(&path) {
            out.push(record);
        }
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.last_seen));
    Ok(out)
}

/// Remove a machine's record. Used by "forget this machine" in the GUI after
/// its folder has been deleted; it never touches backup data itself.
pub fn forget_machine(root: &Path, id: &Uuid) -> Result<bool> {
    let path = record_path(root, id);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            let _ = write_readme(root);
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::io(format!("removing {}", path.display()), e)),
    }
}

/// Write the human-readable explanation of the folder.
///
/// This file is the feature. Someone opens a shared drive, sees `_superbackup`
/// and three cryptic folders, and this tells them what they are looking at,
/// whose data is whose, and — importantly — that deleting a folder destroys
/// that PC's backup history and that the contents are encrypted and cannot be
/// browsed directly.
pub fn write_readme(root: &Path) -> Result<()> {
    let machines = list_machines(root)?;
    let text = render_readme(&machines, Utc::now());
    let path = readme_path(root);
    std::fs::create_dir_all(manifest_dir(root))
        .ctx(format!("creating {}", manifest_dir(root).display()))?;
    write_atomic(&path, text.as_bytes())
}

/// Pure renderer, so the wording can be tested without touching a disk.
///
/// CRLF line endings: this file is most often opened by double-clicking it on
/// Windows, and Notepad on Windows 10 and earlier renders lone LFs as one long
/// line.
pub fn render_readme(machines: &[MachineRecord], generated: DateTime<Utc>) -> String {
    let mut lines: Vec<String> = vec![
        "What is this folder?".into(),
        "=====================".into(),
        String::new(),
        "This drive, share or bucket is used by superbackup, a backup program.".into(),
        "Each computer that backs up here owns one folder next to this one. The".into(),
        "folder name is the computer's name plus a short random code, so that".into(),
        "two computers with the same name never collide.".into(),
        String::new(),
    ];

    if machines.is_empty() {
        lines.push("No computer has finished writing a backup here yet.".into());
    } else {
        lines.push(format!("Computers backing up here ({}):", machines.len()));
        lines.push(String::new());
        for m in machines {
            lines.push(format!("  Folder:      {}", m.slug));
            lines.push(format!("  Computer:    {}", m.label));
            if m.hostname != m.label {
                lines.push(format!("  Host name:   {}", m.hostname));
            }
            let os = if m.os_version.is_empty() { m.os.clone() } else { m.os_version.clone() };
            lines.push(format!("  System:      {os} ({})", m.arch));
            lines.push(format!("  First seen:  {}", m.first_seen.format("%Y-%m-%d")));
            lines.push(format!(
                "  Last backup: {} ({} days ago)",
                m.last_seen.format("%Y-%m-%d %H:%M UTC"),
                m.days_since_seen(generated)
            ));
            if let Some(previous) = m.previous_labels.last() {
                lines.push(format!("  Renamed:     previously \"{}\"", previous.label));
            }
            lines.push(format!("  Written by:  superbackup {}", m.superbackup_version));
            lines.push(String::new());
        }
    }

    lines.push("Can I read the backups directly?".into());
    lines.push("--------------------------------".into());
    lines.push(String::new());
    lines.push("No. The backup data is stored in an encrypted Kopia repository. The".into());
    lines.push("files inside each computer's folder are encrypted chunks with opaque".into());
    lines.push("names; there is no way to browse them in Explorer or Finder, and".into());
    lines.push("nobody without that computer's repository password can read them —".into());
    lines.push("including whoever owns this drive or bucket.".into());
    lines.push(String::new());
    lines.push("To restore, install superbackup or Kopia, point it at this location,".into());
    lines.push("and supply the repository password for the computer you want back.".into());
    lines.push(String::new());
    lines.push("Can I delete a folder?".into());
    lines.push("----------------------".into());
    lines.push(String::new());
    lines.push("Deleting a computer's folder permanently destroys every backup that".into());
    lines.push("computer has ever made here. There is no recycle bin and no other".into());
    lines.push("copy. If a computer is gone for good and you want the space back,".into());
    lines.push("delete its folder and the matching file in _superbackup/machines/.".into());
    lines.push(String::new());
    lines.push("Do not rename or edit anything inside _superbackup/ by hand.".into());
    lines.push(String::new());
    lines.push(format!(
        "This file is regenerated automatically. Last updated {}.",
        generated.format("%Y-%m-%d %H:%M UTC")
    ));

    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out
}

/// Convenience: write the manifest for every destination that has a local
/// root. S3 destinations are handled by the kopia workstream, which uploads
/// the same two objects under the bucket prefix.
pub fn write_manifest_for_destinations(
    destinations: &[Destination],
    identity: &MachineIdentity,
) -> Vec<(Uuid, Result<MachineRecord>)> {
    destinations
        .iter()
        .filter(|d| d.enabled)
        .filter_map(|d| match &d.kind {
            DestinationKind::LocalRepository { path }
            | DestinationKind::OneDrive { path, .. }
            | DestinationKind::LocalMirror { path } => Some((d.id, write_manifest(path, identity))),
            DestinationKind::S3 { .. } => None,
        })
        .collect()
}

/// A one-line summary for the GUI's destination card.
pub fn describe_occupancy(machines: &[MachineRecord], me: &Uuid) -> String {
    let foreign = machines.iter().filter(|m| m.is_foreign(me)).count();
    let mine = machines.len() - foreign;
    match (mine, foreign) {
        (0, 0) => "No backups here yet".to_string(),
        (_, 0) => "Holds backups from this PC only".to_string(),
        (0, 1) => "Holds backups from 1 other PC".to_string(),
        (0, n) => format!("Holds backups from {n} other PCs"),
        (_, 1) => "Holds backups from this PC and 1 other".to_string(),
        (_, n) => format!("Holds backups from this PC and {n} others"),
    }
}

/// Severity for the "foreign machines present" notice, so the GUI can pick a
/// colour without re-implementing the rule.
pub fn occupancy_severity(machines: &[MachineRecord], me: &Uuid) -> Severity {
    if machines.iter().any(|m| m.is_foreign(me)) {
        Severity::Info
    } else {
        Severity::Debug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(label: &str, id: Uuid) -> MachineIdentity {
        MachineIdentity {
            id,
            label: label.to_string(),
            hostname: label.to_string(),
            os: "windows".into(),
            os_version: "Windows 11 Pro 24H2 (build 26200.1)".into(),
            arch: "x86_64".into(),
            username: "andreas".into(),
            slug: slug_for(label, &id),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn same_label_on_two_machines_yields_distinct_slugs() {
        let a = identity("Laptop", Uuid::new_v4());
        let b = identity("Laptop", Uuid::new_v4());
        assert_ne!(a.slug, b.slug, "two PCs called Laptop must not share a folder");
        assert!(a.slug.starts_with("laptop-"));
        assert!(b.slug.starts_with("laptop-"));
    }

    #[test]
    fn slug_is_derived_from_id_not_label_so_rename_keeps_the_folder() {
        let id = Uuid::new_v4();
        let mut me = identity("DESKTOP-8H2K1L", id);
        let before = me.slug.clone();
        let event = rename(&mut me, "Andreas' Studio PC").expect("rename should produce an event");
        assert_eq!(me.slug, before, "renaming must never move the folder");
        assert_eq!(me.label, "Andreas' Studio PC");
        assert_eq!(event.kind, "machine.renamed");
        assert!(rename(&mut me, "Andreas' Studio PC").is_none(), "no-op rename is silent");
        assert!(rename(&mut me, "   ").is_none(), "blank label is rejected");
    }

    #[test]
    fn refresh_repairs_a_slug_that_lost_its_id_suffix() {
        let id = Uuid::new_v4();
        let mut me = identity("PC", id);
        me.slug = "hand-edited".into();
        let events = refresh(&mut me);
        assert!(me.slug.ends_with(&id.simple().to_string()[..8]));
        assert!(events.iter().any(|e| e.kind == "machine.slug_repaired"));
    }

    #[test]
    fn os_release_parsing_handles_quotes() {
        let text = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n";
        assert_eq!(parse_os_release_pretty_name(text).as_deref(), Some("Ubuntu 24.04.1 LTS"));
        assert_eq!(parse_os_release_pretty_name("ID=alpine\n"), None);
    }

    #[test]
    fn plist_scraper_finds_the_product_version() {
        let text = "<dict><key>ProductVersion</key><string>15.3.1</string>\
                    <key>ProductBuildVersion</key><string>24D70</string></dict>";
        assert_eq!(parse_plist_string(text, "ProductVersion").as_deref(), Some("15.3.1"));
        assert_eq!(parse_plist_string(text, "Nope"), None);
    }

    #[test]
    fn readme_names_every_machine_and_says_it_is_encrypted() {
        let now = Utc::now();
        let records = vec![
            MachineRecord::from_identity(&identity("Studio", Uuid::new_v4()), now),
            MachineRecord::from_identity(&identity("Laptop", Uuid::new_v4()), now),
        ];
        let text = render_readme(&records, now);
        assert!(text.contains("Studio"));
        assert!(text.contains("Laptop"));
        assert!(text.to_lowercase().contains("kopia"));
        assert!(text.to_lowercase().contains("encrypted"));
        assert!(text.contains("\r\n"), "Notepad needs CRLF");
    }

    #[test]
    fn empty_readme_still_explains_the_folder() {
        let text = render_readme(&[], Utc::now());
        assert!(text.contains("No computer has finished writing a backup here yet."));
        assert!(text.contains("superbackup"));
    }

    #[test]
    fn occupancy_wording_distinguishes_foreign_machines() {
        let me = Uuid::new_v4();
        let now = Utc::now();
        let mine = MachineRecord::from_identity(&identity("Mine", me), now);
        let theirs = MachineRecord::from_identity(&identity("Theirs", Uuid::new_v4()), now);
        assert_eq!(describe_occupancy(&[], &me), "No backups here yet");
        assert_eq!(
            describe_occupancy(std::slice::from_ref(&mine), &me),
            "Holds backups from this PC only"
        );
        assert_eq!(
            describe_occupancy(&[mine, theirs.clone()], &me),
            "Holds backups from this PC and 1 other"
        );
        assert_eq!(describe_occupancy(&[theirs], &me), "Holds backups from 1 other PC");
    }
}
