//! OneDrive discovery, sync-state awareness, and destination validation.
//!
//! # Why not `%USERPROFILE%\OneDrive`
//!
//! That guess is wrong on every interesting machine. A work PC puts the
//! business folder at `%USERPROFILE%\OneDrive - Contoso`; a PC with two
//! tenants has two of them; a user who moved the folder to `D:\` has none of
//! them under the profile at all; and `%OneDrive%` points at whichever account
//! happened to sign in last. The authoritative source on Windows is
//! `HKCU\Software\Microsoft\OneDrive\Accounts\<Personal|BusinessN>`, which the
//! sync client itself writes and keeps current. We read that, then cross-check
//! the `%OneDrive%`, `%OneDriveConsumer%` and `%OneDriveCommercial%`
//! environment variables so a folder the registry has not caught up with is
//! still offered.
//!
//! # Files On-Demand is the real hazard
//!
//! A Kopia repository inside a OneDrive folder is a directory of thousands of
//! opaque blobs. If Files On-Demand dehydrates them, kopia's next read stalls
//! for minutes or fails outright, and a "free up space" sweep can quietly turn
//! a working repository into a directory of placeholders. So we check the
//! cloud-file attributes before accepting a path, and we ask Windows to pin the
//! repository folder as "Always keep on this device".
//!
//! # Platform reality
//!
//! * **Windows** — full support: registry discovery, multiple accounts, quota-
//!   aware free space, placeholder detection, programmatic pinning.
//! * **macOS** — discovery only, via `~/Library/CloudStorage/OneDrive-*`. macOS
//!   has no public API for per-file pinning; the user must use Finder's
//!   "Always Keep on This Device".
//! * **Linux** — Microsoft ships no client. We detect the common third-party
//!   ones (abraunegg's `onedrive`, `onedriver`, an rclone FUSE mount) and
//!   otherwise return nothing, cleanly. None of them support pinning, and all
//!   of them are FUSE mounts that a system service will not be able to see.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::model::DestinationKind;
use crate::state::Severity;

/// Below this much free space we warn even if the repository would fit today.
/// Kopia grows; a full OneDrive stops syncing everything, not just us.
pub const LOW_SPACE_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// …or below this fraction of the volume, for small SSDs where 5 GB is a lot.
pub const LOW_SPACE_FRACTION: f64 = 0.05;

/// Windows path length at which we start warning. `MAX_PATH` is 260; kopia
/// appends roughly 60 characters of shard directories and blob names beneath
/// the repository root, and long-path support is opt-in per process and not
/// guaranteed for every tool the user may point at the folder later.
pub const PATH_WARN_LEN: usize = 160;
pub const PATH_ERROR_LEN: usize = 200;

/// The folder we suggest inside an account. A single, obvious, human name.
pub const SUGGESTED_FOLDER: &str = "Backup";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OneDriveKind {
    /// A consumer Microsoft account.
    Personal,
    /// A work or school account. `tenant` is the organisation name as OneDrive
    /// spells it in the folder name, when we could determine it.
    Business { tenant: Option<String> },
    /// Not Microsoft's client: `onedriver`, abraunegg's `onedrive`, or an
    /// rclone mount. Behaves like a plain folder, with caveats.
    ThirdParty { client: String },
}

impl OneDriveKind {
    pub fn title(&self) -> String {
        match self {
            OneDriveKind::Personal => "OneDrive Personal".to_string(),
            OneDriveKind::Business { tenant: Some(t) } => format!("OneDrive for Business — {t}"),
            OneDriveKind::Business { tenant: None } => "OneDrive for Business".to_string(),
            OneDriveKind::ThirdParty { client } => format!("OneDrive via {client}"),
        }
    }
}

/// How the files under a path are materialised locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    /// Pinned: the sync engine guarantees a local copy. What we want.
    AlwaysAvailable,
    /// Present locally but not pinned — a "free up space" sweep may evict it.
    AvailableNotPinned,
    /// Dehydrated placeholders. Reading them triggers a network download.
    OnlineOnly,
    /// Not a cloud-backed folder at all (plain disk, or a non-Windows host).
    NotCloudBacked,
    /// We could not tell, and refuse to guess.
    Unknown,
}

impl SyncState {
    /// True when writing a repository here risks dehydration.
    pub fn is_risky(&self) -> bool {
        matches!(self, SyncState::OnlineOnly | SyncState::AvailableNotPinned)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneDriveAccount {
    /// The root of the synced folder.
    pub path: PathBuf,
    /// What to show in a picker, e.g. "OneDrive — Contoso (a.wiren@contoso.com)".
    pub display_name: String,
    pub kind: OneDriveKind,
    /// Registry subkey on Windows (`Personal`, `Business1`, …), so a later
    /// re-detection can match an account across renames.
    #[serde(default)]
    pub account_key: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    /// Free bytes available to this user on the volume holding `path`.
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub sync_state: SyncState,
    /// Human-readable problems the GUI must show next to this account.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl OneDriveAccount {
    /// Where we propose to put the repository: one obvious folder at the root
    /// of the account, not buried, and not the account root itself — putting a
    /// repository at the root would mix opaque blobs in with the user's
    /// documents and make "delete the backup" a dangerous operation.
    pub fn suggested_repository_root(&self) -> PathBuf {
        self.path.join(SUGGESTED_FOLDER)
    }

    /// The destination kind to store in the config for this account.
    pub fn to_destination_kind(&self, path: PathBuf) -> DestinationKind {
        DestinationKind::OneDrive { path, account: Some(self.display_name.clone()) }
    }

    pub fn is_nearly_full(&self) -> bool {
        if self.total_bytes == 0 {
            return false;
        }
        self.available_bytes < LOW_SPACE_BYTES
            || (self.available_bytes as f64) < (self.total_bytes as f64) * LOW_SPACE_FRACTION
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Every OneDrive account this user has, newest information first.
///
/// Never fails: a machine with no OneDrive returns an empty vector, which is a
/// perfectly good answer and must not block the setup wizard.
pub fn detect() -> Vec<OneDriveAccount> {
    let mut accounts = detect_raw();
    accounts.retain(|a| a.path.is_dir());
    dedupe_by_path(&mut accounts);
    for account in &mut accounts {
        enrich(account);
    }
    accounts
}

fn detect_raw() -> Vec<OneDriveAccount> {
    #[cfg(windows)]
    {
        let mut found = detect_windows_registry();
        found.extend(detect_windows_env());
        found
    }
    #[cfg(target_os = "macos")]
    {
        detect_macos()
    }
    #[cfg(target_os = "linux")]
    {
        detect_linux()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Vec::new()
    }
}

/// Fill in free space, sync state and warnings for a discovered account.
fn enrich(account: &mut OneDriveAccount) {
    if let Some((available, total)) = super::disk_space(&account.path) {
        account.available_bytes = available;
        account.total_bytes = total;
    }
    account.sync_state = sync_state(&account.path);

    if account.is_nearly_full() {
        account.warnings.push(format!(
            "Only {} free on this drive. OneDrive stops syncing everything when it fills up, \
             not just your backups.",
            bytesize::ByteSize(account.available_bytes)
        ));
    }
    match account.sync_state {
        SyncState::OnlineOnly => account.warnings.push(
            "Files here are online-only (Files On-Demand). superbackup will pin the backup \
             folder so Windows keeps a local copy."
                .to_string(),
        ),
        SyncState::AvailableNotPinned => account.warnings.push(
            "This folder is not pinned. Windows may free up space by removing local copies; \
             superbackup will pin the backup folder it creates."
                .to_string(),
        ),
        _ => {}
    }
    if let OneDriveKind::ThirdParty { client } = &account.kind {
        account.warnings.push(format!(
            "{client} is a third-party OneDrive client. It is a FUSE mount owned by your login \
             session, so a system-wide superbackup service will not be able to see it — run \
             superbackup as a user service instead."
        ));
    }
}

fn dedupe_by_path(accounts: &mut Vec<OneDriveAccount>) {
    let mut seen: Vec<String> = Vec::new();
    accounts.retain(|a| {
        let key = canonical_key(&a.path);
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
}

/// A comparison key for "is this the same folder?". Windows paths are
/// case-insensitive; Linux and macOS are (usually) not, and we do not try to
/// out-guess a case-insensitive APFS volume.
fn canonical_key(path: &Path) -> String {
    let text = std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

// --- Windows ---------------------------------------------------------------

#[cfg(windows)]
fn detect_windows_registry() -> Vec<OneDriveAccount> {
    use super::win32::{Hive, RegKey};

    let Some(root) = RegKey::open(Hive::CurrentUser, r"Software\Microsoft\OneDrive\Accounts") else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for key_name in root.subkeys() {
        let path = format!(r"Software\Microsoft\OneDrive\Accounts\{key_name}");
        let Some(key) = RegKey::open(Hive::CurrentUser, &path) else {
            continue;
        };
        let Some(folder) = key.string("UserFolder").filter(|s| !s.trim().is_empty()) else {
            // An account key with no UserFolder is a half-configured or
            // signed-out account. Skipping it is correct.
            continue;
        };
        let folder = PathBuf::from(folder);
        let email = key.string("UserEmail").filter(|s| !s.is_empty());
        let display = key.string("DisplayName").filter(|s| !s.is_empty());

        // `Business` is a DWORD written by the client; the key name is the
        // fallback for older clients that omitted it.
        let is_business =
            key.dword("Business").unwrap_or(0) == 1 || key_name.starts_with("Business");
        let tenant = key
            .string("DisplayName")
            .filter(|_| is_business)
            .or_else(|| tenant_from_folder_name(&folder));

        let kind = if is_business {
            OneDriveKind::Business { tenant }
        } else {
            OneDriveKind::Personal
        };

        out.push(OneDriveAccount {
            display_name: compose_display_name(&kind, display.as_deref(), email.as_deref()),
            path: folder,
            kind,
            account_key: Some(key_name),
            email,
            available_bytes: 0,
            total_bytes: 0,
            sync_state: SyncState::Unknown,
            warnings: Vec::new(),
        });
    }
    out
}

/// The environment variables OneDrive sets for the *current* session. These
/// lag behind the registry and only ever describe the default account, but on
/// a machine where the registry read was denied by policy they are all we have.
#[cfg(windows)]
fn detect_windows_env() -> Vec<OneDriveAccount> {
    let mut out = Vec::new();
    for (var, kind) in [
        ("OneDriveConsumer", OneDriveKind::Personal),
        ("OneDriveCommercial", OneDriveKind::Business { tenant: None }),
        ("OneDrive", OneDriveKind::Personal),
    ] {
        let Some(value) = std::env::var_os(var) else {
            continue;
        };
        let path = PathBuf::from(value);
        if path.as_os_str().is_empty() {
            continue;
        }
        let kind = match kind {
            OneDriveKind::Business { .. } => {
                OneDriveKind::Business { tenant: tenant_from_folder_name(&path) }
            }
            other => other,
        };
        out.push(OneDriveAccount {
            display_name: compose_display_name(&kind, None, None),
            path,
            kind,
            account_key: None,
            email: None,
            available_bytes: 0,
            total_bytes: 0,
            sync_state: SyncState::Unknown,
            warnings: Vec::new(),
        });
    }
    out
}

/// `C:\Users\me\OneDrive - Contoso Ltd` -> `Contoso Ltd`.
pub(crate) fn tenant_from_folder_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let (_, tenant) = name.split_once(" - ")?;
    let tenant = tenant.trim();
    if tenant.is_empty() {
        None
    } else {
        Some(tenant.to_string())
    }
}

pub(crate) fn compose_display_name(
    kind: &OneDriveKind,
    display: Option<&str>,
    email: Option<&str>,
) -> String {
    let base = match (kind, display) {
        (OneDriveKind::Business { .. }, Some(d)) => format!("OneDrive for Business — {d}"),
        _ => kind.title(),
    };
    match email {
        Some(e) => format!("{base} ({e})"),
        None => base,
    }
}

// --- macOS -----------------------------------------------------------------

#[cfg(target_os = "macos")]
fn detect_macos() -> Vec<OneDriveAccount> {
    let Some(home) = directories::BaseDirs::new() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    // Modern (File Provider) location.
    let cloud = home.home_dir().join("Library/CloudStorage");
    if let Ok(entries) = std::fs::read_dir(&cloud) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("OneDrive") {
                continue;
            }
            // `OneDrive-Personal` or `OneDrive-Contoso Ltd`.
            let suffix = name.strip_prefix("OneDrive-").unwrap_or("").trim().to_string();
            let kind = if suffix.eq_ignore_ascii_case("Personal") || suffix.is_empty() {
                OneDriveKind::Personal
            } else {
                OneDriveKind::Business { tenant: Some(suffix) }
            };
            out.push(OneDriveAccount {
                display_name: compose_display_name(&kind, None, None),
                path: entry.path(),
                kind,
                account_key: Some(name),
                email: None,
                available_bytes: 0,
                total_bytes: 0,
                sync_state: SyncState::Unknown,
                warnings: Vec::new(),
            });
        }
    }

    // The legacy pre-CloudStorage location, still present on older macOS and
    // on machines upgraded from one.
    let legacy = home.home_dir().join("OneDrive");
    if legacy.is_dir() {
        out.push(OneDriveAccount {
            display_name: OneDriveKind::Personal.title(),
            path: legacy,
            kind: OneDriveKind::Personal,
            account_key: None,
            email: None,
            available_bytes: 0,
            total_bytes: 0,
            sync_state: SyncState::Unknown,
            warnings: Vec::new(),
        });
    }
    out
}

// --- Linux -----------------------------------------------------------------

#[cfg(target_os = "linux")]
fn detect_linux() -> Vec<OneDriveAccount> {
    let mut out = Vec::new();

    // 1. abraunegg/onedrive keeps its target directory in a plain config file.
    if let Some(base) = directories::BaseDirs::new() {
        let config_root = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| base.home_dir().join(".config"));
        if let Ok(entries) = std::fs::read_dir(&config_root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name != "onedrive" && !name.starts_with("onedrive-") {
                    continue;
                }
                let cfg = entry.path().join("config");
                let sync_dir = std::fs::read_to_string(&cfg)
                    .ok()
                    .and_then(|t| parse_onedrive_config_sync_dir(&t))
                    .unwrap_or_else(|| "~/OneDrive".to_string());
                let path = expand_tilde(&sync_dir, base.home_dir());
                out.push(third_party(path, "the onedrive client (abraunegg)"));
            }
        }
    }

    // 2. FUSE mounts: onedriver, and rclone mounts of a OneDrive remote.
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        for (device, mount_point, fstype) in onedrive_mounts(&mounts) {
            let client = if fstype.contains("onedriver") {
                "onedriver".to_string()
            } else {
                format!("rclone ({device})")
            };
            out.push(third_party(PathBuf::from(mount_point), &client));
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn third_party(path: PathBuf, client: &str) -> OneDriveAccount {
    let kind = OneDriveKind::ThirdParty { client: client.to_string() };
    OneDriveAccount {
        display_name: kind.title(),
        path,
        kind,
        account_key: None,
        email: None,
        available_bytes: 0,
        total_bytes: 0,
        sync_state: SyncState::Unknown,
        warnings: Vec::new(),
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn expand_tilde(value: &str, home: &Path) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        home.join(rest)
    } else if value == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(value)
    }
}

/// `sync_dir = "~/OneDrive"` out of an abraunegg `onedrive` config file.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_onedrive_config_sync_dir(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "sync_dir" {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// Mount points from `/proc/mounts` that look like a OneDrive client.
///
/// `/proc/mounts` octal-escapes spaces and a few other characters in the
/// device and mount-point fields; a mount at `/home/me/My Drive` arrives as
/// `/home/me/My\040Drive`, and un-escaping it is the difference between
/// finding the folder and silently ignoring it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn onedrive_mounts(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mount_point), Some(fstype)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let device = unescape_mount_field(device);
        let mount_point = unescape_mount_field(mount_point);
        let looks_like_onedrive = fstype.contains("onedriver")
            || (fstype.starts_with("fuse") && device.to_lowercase().contains("onedrive"));
        if looks_like_onedrive {
            out.push((device, mount_point, fstype.to_string()));
        }
    }
    out
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn unescape_mount_field(field: &str) -> String {
    let bytes: Vec<char> = field.chars().collect();
    let mut out = String::with_capacity(field.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '\\' && i + 3 < bytes.len() {
            let digits: String = bytes[i + 1..i + 4].iter().collect();
            if let Ok(code) = u8::from_str_radix(&digits, 8) {
                out.push(code as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Sync state and pinning
// ---------------------------------------------------------------------------

/// What the cloud-file attributes say about a path.
///
/// On Windows this reads `FILE_ATTRIBUTE_PINNED` / `_UNPINNED` /
/// `_RECALL_ON_DATA_ACCESS` / `_OFFLINE`, which is what File Explorer's
/// green-tick and cloud icons are drawn from. Everywhere else there is no
/// equivalent and we say so rather than guessing.
pub fn sync_state(path: &Path) -> SyncState {
    #[cfg(windows)]
    {
        use windows::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_PINNED, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_UNPINNED,
        };
        let Some(attrs) = super::win32::file_attributes(path) else {
            return SyncState::Unknown;
        };
        let has = |flag: windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES| {
            attrs & flag.0 != 0
        };
        if has(FILE_ATTRIBUTE_PINNED) {
            return SyncState::AlwaysAvailable;
        }
        if has(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
            || has(FILE_ATTRIBUTE_RECALL_ON_OPEN)
            || has(FILE_ATTRIBUTE_OFFLINE)
        {
            return SyncState::OnlineOnly;
        }
        if has(FILE_ATTRIBUTE_UNPINNED) {
            return SyncState::AvailableNotPinned;
        }
        // A cloud-backed directory that is neither pinned nor dehydrated is
        // still under the sync engine's control; a reparse point is the tell.
        if has(FILE_ATTRIBUTE_REPARSE_POINT) {
            return SyncState::AvailableNotPinned;
        }
        SyncState::NotCloudBacked
    }
    #[cfg(not(windows))]
    {
        // macOS File Provider and every Linux FUSE client can dehydrate files
        // too, but neither exposes the state through a stable public API.
        // Claiming "NotCloudBacked" would be a lie, so we do not.
        let _ = path;
        SyncState::Unknown
    }
}

/// The result of preparing a repository folder inside a cloud-synced account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedFolder {
    pub path: PathBuf,
    /// True when we successfully told the OS to keep this folder local.
    pub pinned: bool,
    pub sync_state: SyncState,
    /// Exact clicks the user must perform when we could not do it ourselves.
    /// The GUI shows these verbatim; they are written to be followed, not to
    /// be read.
    #[serde(default)]
    pub manual_steps: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Create the repository folder and make the platform keep it materialised.
///
/// On Windows this sets `FILE_ATTRIBUTE_PINNED` and clears
/// `FILE_ATTRIBUTE_UNPINNED`, which is exactly what File Explorer's "Always
/// keep on this device" does (and what `attrib +P -U` does from a shell). Two
/// honest caveats, both surfaced to the user:
///
/// 1. The attribute applies to the directory and to content created inside it
///    afterwards. It does not retroactively hydrate files that are already
///    dehydrated; OneDrive does that asynchronously once it notices.
/// 2. Group Policy can force "Files On-Demand" and storage-sense eviction on a
///    managed device, which overrides the pin.
pub fn prepare_repository_folder(path: &Path) -> Result<PreparedFolder> {
    std::fs::create_dir_all(path).ctx(format!("creating {}", path.display()))?;

    let mut prepared = PreparedFolder {
        path: path.to_path_buf(),
        pinned: false,
        sync_state: SyncState::Unknown,
        manual_steps: Vec::new(),
        warnings: Vec::new(),
    };

    #[cfg(windows)]
    {
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_PINNED, FILE_ATTRIBUTE_UNPINNED};
        match super::win32::file_attributes(path) {
            Some(current) => {
                let desired = (current & !FILE_ATTRIBUTE_UNPINNED.0) | FILE_ATTRIBUTE_PINNED.0;
                match super::win32::set_file_attributes(path, desired) {
                    Ok(()) => prepared.pinned = true,
                    Err(e) => {
                        tracing::warn!(error = %e, path = %path.display(), "could not pin folder");
                        prepared.manual_steps.extend(manual_pin_steps_windows(path));
                    }
                }
            }
            None => prepared.manual_steps.extend(manual_pin_steps_windows(path)),
        }
    }
    #[cfg(target_os = "macos")]
    {
        prepared.manual_steps.push(format!(
            "In Finder, open {}, right-click the folder and choose \
             \"Always Keep on This Device\". macOS offers no way for an application to do this \
             for you.",
            path.display()
        ));
    }
    #[cfg(target_os = "linux")]
    {
        prepared.warnings.push(
            "Linux OneDrive clients are third-party and have no concept of pinning. Make sure \
             the client is configured to download files rather than mount them on demand, or \
             kopia will stall on every read."
                .to_string(),
        );
    }

    prepared.sync_state = sync_state(path);
    if prepared.sync_state == SyncState::OnlineOnly {
        prepared.warnings.push(
            "This folder still reports as online-only. Wait for OneDrive to finish downloading \
             it before creating the repository."
                .to_string(),
        );
    }
    Ok(prepared)
}

#[cfg(windows)]
fn manual_pin_steps_windows(path: &Path) -> Vec<String> {
    vec![
        format!("Open File Explorer and go to {}.", path.display()),
        "Right-click the folder and choose \"Always keep on this device\".".to_string(),
        "If that entry is missing, OneDrive's Files On-Demand is switched off, and the folder \
         is already kept locally — nothing more to do."
            .to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// A single reason a chosen path is or may be unusable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub severity: Severity,
    /// Stable machine-readable code, for tests and for the GUI to key off.
    /// A `String` rather than a `&'static str` so the whole thing survives a
    /// round trip through IPC to the GUI process.
    pub code: String,
    pub message: String,
    /// What the user should do about it, when there is a concrete answer.
    #[serde(default)]
    pub remedy: Option<String>,
}

impl ValidationIssue {
    fn error(code: &'static str, message: impl Into<String>, remedy: Option<&str>) -> Self {
        ValidationIssue {
            severity: Severity::Error,
            code: code.to_string(),
            message: message.into(),
            remedy: remedy.map(str::to_string),
        }
    }
    fn warn(code: &'static str, message: impl Into<String>, remedy: Option<&str>) -> Self {
        ValidationIssue {
            severity: Severity::Warning,
            code: code.to_string(),
            message: message.into(),
            remedy: remedy.map(str::to_string),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Validation {
    pub issues: Vec<ValidationIssue>,
}

impl Validation {
    /// True when nothing blocks using this path. Warnings do not block.
    pub fn is_usable(&self) -> bool {
        !self.issues.iter().any(|i| i.severity == Severity::Error)
    }
    pub fn errors(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.issues.iter().filter(|i| i.severity == Severity::Error)
    }
    pub fn warnings(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.issues.iter().filter(|i| i.severity == Severity::Warning)
    }
    /// Collapse to a single `Error` for callers that just want a `Result`.
    pub fn into_result(self) -> Result<Vec<ValidationIssue>> {
        if self.is_usable() {
            Ok(self.issues)
        } else {
            let joined = self
                .errors()
                .map(|i| i.message.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            Err(Error::Validation(joined))
        }
    }
}

/// Validate a candidate repository location.
///
/// `sources` are the folders the user intends to back up; `required_bytes` is
/// the caller's estimate of the first snapshot, or 0 when unknown.
///
/// This performs I/O (a write probe, a free-space query). The pure rules are
/// factored out into [`layout_issues`] and [`path_length_issue`] so they can be
/// tested without a filesystem.
pub fn validate(path: &Path, sources: &[PathBuf], required_bytes: u64) -> Validation {
    let mut issues = layout_issues(path, sources);
    issues.extend(path_length_issue(path));

    // Write permission. We must actually try: on Windows an ACL can permit
    // listing a directory and deny creating files in it, and `metadata()`
    // cheerfully reports it as writable.
    match write_probe(path) {
        Ok(()) => {}
        Err(e) => issues.push(ValidationIssue::error(
            "not_writable",
            format!("Cannot write to {}: {}", path.display(), e),
            Some("Choose a different folder, or grant your account write permission on this one."),
        )),
    }

    if let Some((available, total)) = super::disk_space(path) {
        if required_bytes > 0 && available < required_bytes {
            issues.push(ValidationIssue::error(
                "insufficient_space",
                format!(
                    "Needs about {} but only {} is free.",
                    bytesize::ByteSize(required_bytes),
                    bytesize::ByteSize(available)
                ),
                Some("Free up space, exclude large folders, or pick another destination."),
            ));
        } else if available < LOW_SPACE_BYTES
            || (total > 0 && (available as f64) < total as f64 * LOW_SPACE_FRACTION)
        {
            issues.push(ValidationIssue::warn(
                "low_space",
                format!("Only {} free on this drive.", bytesize::ByteSize(available)),
                Some("Backups grow over time; keep an eye on this."),
            ));
        }
    }

    match sync_state(path) {
        SyncState::OnlineOnly => issues.push(ValidationIssue::error(
            "online_only",
            "This folder is stored online-only. Kopia cannot keep a repository in files that \
             Windows may replace with placeholders."
                .to_string(),
            Some(
                "Right-click the folder in File Explorer and choose \"Always keep on this \
                 device\", then try again.",
            ),
        )),
        SyncState::AvailableNotPinned => issues.push(ValidationIssue::warn(
            "not_pinned",
            "This folder is not pinned to the device. Windows may free up space by removing \
             the local copies."
                .to_string(),
            Some("superbackup will pin the folder it creates; leave that setting alone."),
        )),
        _ => {}
    }

    Validation { issues }
}

/// Rules that depend only on the shape of the paths. Pure — no I/O.
pub fn layout_issues(path: &Path, sources: &[PathBuf]) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    for source in sources {
        if path.starts_with(source) {
            issues.push(ValidationIssue::error(
                "inside_source",
                format!(
                    "{} is inside {}, which this job backs up. The repository would try to \
                     back itself up, growing without limit.",
                    path.display(),
                    source.display()
                ),
                Some("Pick a folder outside every source, or exclude this path from the job."),
            ));
        } else if source.starts_with(path) {
            issues.push(ValidationIssue::warn(
                "contains_source",
                format!(
                    "{} contains {}, which this job backs up. That works, but the folder will \
                     hold both your files and the encrypted repository.",
                    path.display(),
                    source.display()
                ),
                Some("A dedicated, empty folder is easier to reason about."),
            ));
        }
    }
    issues
}

/// Windows path-length rule. Pure — no I/O.
///
/// Returned as an `Option` rather than a bool so the message can carry the
/// actual length, which is what makes the error actionable.
pub fn path_length_issue(path: &Path) -> Option<ValidationIssue> {
    if !cfg!(windows) {
        return None;
    }
    let len = path.as_os_str().len();
    if len >= PATH_ERROR_LEN {
        Some(ValidationIssue::error(
            "path_too_long",
            format!(
                "This path is {len} characters. Kopia adds about 60 more beneath it, which \
                 exceeds Windows' 260-character limit."
            ),
            Some("Choose a shorter path, closer to the root of the drive."),
        ))
    } else if len >= PATH_WARN_LEN {
        Some(ValidationIssue::warn(
            "path_long",
            format!("This path is {len} characters, which leaves little room for Windows' \
                     260-character limit."),
            Some("A shorter path closer to the drive root is safer."),
        ))
    } else {
        None
    }
}

/// Actually try to create and delete a file. The only honest permission test.
fn write_probe(path: &Path) -> std::result::Result<(), std::io::Error> {
    std::fs::create_dir_all(path)?;
    let probe = path.join(format!(".superbackup-write-test-{}", std::process::id()));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&probe)?;
        f.write_all(b"superbackup write test")?;
        f.flush()?;
    }
    std::fs::remove_file(&probe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_inside_a_source_is_rejected() {
        let source = PathBuf::from("/home/me/Documents");
        let issues = layout_issues(Path::new("/home/me/Documents/Backup"), &[source]);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "inside_source");
        assert_eq!(issues[0].severity, Severity::Error);
    }

    #[test]
    fn a_repository_containing_a_source_is_only_a_warning() {
        let source = PathBuf::from("/data/projects");
        let issues = layout_issues(Path::new("/data"), &[source]);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "contains_source");
        assert_eq!(issues[0].severity, Severity::Warning);
    }

    #[test]
    fn unrelated_paths_produce_no_issues() {
        let source = PathBuf::from("/home/me/Documents");
        assert!(layout_issues(Path::new("/mnt/backup"), &[source]).is_empty());
    }

    #[test]
    fn path_length_rule_matches_the_platform() {
        let long = PathBuf::from(format!("C:\\{}", "x".repeat(PATH_ERROR_LEN)));
        let issue = path_length_issue(&long);
        if cfg!(windows) {
            let issue = issue.expect("a 200+ character path must be rejected on Windows");
            assert_eq!(issue.code, "path_too_long");
        } else {
            assert!(issue.is_none(), "only Windows has a 260-character problem");
        }
    }

    #[test]
    fn validation_is_usable_only_without_errors() {
        let v = Validation {
            issues: vec![ValidationIssue::warn("low_space", "nearly full", None)],
        };
        assert!(v.is_usable());
        let v = Validation {
            issues: vec![ValidationIssue::error("not_writable", "denied", None)],
        };
        assert!(!v.is_usable());
        assert!(v.into_result().is_err());
    }

    #[test]
    fn tenant_is_read_from_the_folder_name() {
        assert_eq!(
            tenant_from_folder_name(Path::new(r"C:\Users\me\OneDrive - Contoso Ltd")).as_deref(),
            Some("Contoso Ltd")
        );
        assert_eq!(tenant_from_folder_name(Path::new(r"C:\Users\me\OneDrive")), None);
    }

    #[test]
    fn display_names_include_the_account() {
        let kind = OneDriveKind::Business { tenant: Some("Contoso".into()) };
        assert_eq!(
            compose_display_name(&kind, Some("Contoso"), Some("me@contoso.com")),
            "OneDrive for Business — Contoso (me@contoso.com)"
        );
        assert_eq!(compose_display_name(&OneDriveKind::Personal, None, None), "OneDrive Personal");
    }

    #[test]
    fn onedrive_config_sync_dir_is_parsed() {
        let text = "# comment\nsync_dir = \"~/Cloud/OneDrive\"\nskip_file = \"~*\"\n";
        assert_eq!(
            parse_onedrive_config_sync_dir(text).as_deref(),
            Some("~/Cloud/OneDrive")
        );
        assert_eq!(parse_onedrive_config_sync_dir("# sync_dir = \"x\"\n"), None);
    }

    #[test]
    fn proc_mounts_are_unescaped_and_filtered() {
        let text = "\
/dev/sda1 / ext4 rw 0 0
onedrive: /home/me/My\\040Drive fuse.rclone rw,nosuid 0 0
onedriver /home/me/OneDrive fuse.onedriver rw 0 0
dropbox: /home/me/Dropbox fuse.rclone rw 0 0
";
        let mounts = onedrive_mounts(text);
        assert_eq!(mounts.len(), 2, "dropbox and ext4 must not match: {mounts:?}");
        assert_eq!(mounts[0].1, "/home/me/My Drive");
        assert_eq!(mounts[1].2, "fuse.onedriver");
    }

    #[test]
    fn tilde_expansion_only_touches_a_leading_tilde() {
        let home = Path::new("/home/me");
        assert_eq!(expand_tilde("~/OneDrive", home), PathBuf::from("/home/me/OneDrive"));
        assert_eq!(expand_tilde("/mnt/od", home), PathBuf::from("/mnt/od"));
        assert_eq!(expand_tilde("~", home), PathBuf::from("/home/me"));
    }

    #[test]
    fn nearly_full_uses_both_absolute_and_relative_thresholds() {
        let mut account = OneDriveAccount {
            path: PathBuf::from("/tmp"),
            display_name: "x".into(),
            kind: OneDriveKind::Personal,
            account_key: None,
            email: None,
            available_bytes: 100 * 1024 * 1024 * 1024,
            total_bytes: 4000 * 1024 * 1024 * 1024,
            sync_state: SyncState::Unknown,
            warnings: vec![],
        };
        assert!(account.is_nearly_full(), "100 GB of 4 TB is under 5%");
        account.total_bytes = 200 * 1024 * 1024 * 1024;
        assert!(!account.is_nearly_full(), "100 GB of 200 GB is fine");
        account.available_bytes = 1024 * 1024 * 1024;
        assert!(account.is_nearly_full(), "1 GB is under the absolute floor");
    }

    #[test]
    fn detect_never_panics_and_never_returns_a_missing_path() {
        for account in detect() {
            assert!(account.path.is_dir(), "{} was returned but does not exist", account.path.display());
        }
    }
}
