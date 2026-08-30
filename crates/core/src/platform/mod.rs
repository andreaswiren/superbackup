//! Platform integration: everything that is different on Windows, Linux and
//! macOS, kept behind one honest API.
//!
//! ```text
//! identity        who this PC is, and the manifest that says so inside a
//!                 shared destination
//! onedrive        finding OneDrive properly, and refusing to put a repository
//!                 somewhere the sync engine will dissolve it
//! autostart       run at login (and notice when the recorded path went stale)
//! service         run with nobody logged in, and say which destinations that
//!                 actually reaches
//! notify          desktop notifications, deduplicated and redacted
//! power           metered connection, battery, and sleep/wake
//! single_instance one daemon per configuration, with safe stale-lock takeover
//! ```
//!
//! # Design rules
//!
//! * **Windows first, Linux second, macOS third — but all three compile and
//!   all three have a real implementation.** There is no `unimplemented!()` in
//!   this module. Where a platform genuinely cannot answer a question (macOS
//!   has no metered-connection API), the answer is an explicit "unknown" that
//!   the caller is documented to treat as permission to proceed, not a panic
//!   and not a silent lie.
//! * **Degrade, never fail.** Nothing in here may stop a backup. A registry
//!   read that is denied by policy, a notification daemon that is not running,
//!   a battery that cannot be read — all produce a default and a log line.
//! * **`unsafe` exists only where a Win32 signature requires it**: the
//!   registry, file-attribute, disk and token wrappers in [`win32`], the
//!   `INetworkCostManager` COM call in [`power`], and the named mutex in
//!   [`single_instance`]. Nowhere else, and every block carries a `// SAFETY:`
//!   note naming the invariant it relies on.
//! * **Every platform limitation is discoverable at runtime**, through
//!   [`capabilities`] and [`limitations`], so the GUI can explain itself rather
//!   than presenting a control that quietly does nothing.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub mod autostart;
pub mod identity;
pub mod notify;
pub mod onedrive;
pub mod power;
pub mod service;
pub mod single_instance;

#[cfg(windows)]
mod win32;

pub use identity::{list_machines, write_manifest, MachineRecord};
pub use notify::{Notification, NotificationKind, Notifier, NotifyOutcome};
pub use onedrive::{OneDriveAccount, OneDriveKind, SyncState, Validation, ValidationIssue};
pub use power::{Metered, PowerSource, PowerStatus, WakeDetector};
pub use service::{ServiceOptions, ServiceScope, ServiceState, ServiceStatus};
pub use single_instance::{InstanceGuard, LockOutcome};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Free bytes available to the current user, and the volume total, for the
/// volume holding `path`.
///
/// Windows uses `GetDiskFreeSpaceExW`, which honours per-user disk quotas — on
/// a quota'd corporate machine the volume's raw free space is not the space
/// this user may actually use, and reporting the wrong one turns a clear "not
/// enough room" into a mid-backup failure. Elsewhere we ask `sysinfo` for the
/// mounted filesystem containing the path.
///
/// `None` when the path does not exist or the platform will not say.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    #[cfg(windows)]
    {
        // The API wants an existing directory; walk up until we find one, so a
        // caller can ask about a folder it has not created yet.
        let mut probe = path;
        loop {
            if probe.is_dir() {
                return win32::disk_space(probe);
            }
            probe = probe.parent()?;
        }
    }
    #[cfg(not(windows))]
    {
        use sysinfo::Disks;
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let disks = Disks::new_with_refreshed_list();
        // The filesystem containing the path is the one with the longest
        // matching mount point: `/home` must win over `/`.
        disks
            .list()
            .iter()
            .filter(|d| target.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
            .map(|d| (d.available_space(), d.total_space()))
    }
}

/// The effective user id on Unix, or `None` on Windows and anywhere we cannot
/// tell.
///
/// This crate has no `libc` dependency, so we read `/proc/self/status` where it
/// exists and fall back to `id -u`. Both are cheap and neither needs a C
/// toolchain.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn effective_uid() -> Option<u32> {
    #[cfg(windows)]
    {
        None
    }
    #[cfg(not(windows))]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            if let Some(uid) = parse_proc_status_uid(&status) {
                return Some(uid);
            }
        }
        let output = std::process::Command::new("id").arg("-u").output().ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    }
}

/// `Uid:\t1000\t1000\t1000\t1000` — real, effective, saved, filesystem.
/// We want the effective id, the second field.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn parse_proc_status_uid(status: &str) -> Option<u32> {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let mut fields = rest.split_whitespace();
            let _real = fields.next()?;
            return fields.next()?.parse().ok();
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Capabilities and limitations
// ---------------------------------------------------------------------------

/// What this platform can actually do, so the GUI can hide or explain controls
/// instead of offering something that silently does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Start-at-login is implemented.
    pub autostart: bool,
    /// A machine-wide service that runs with nobody logged in.
    pub system_service: bool,
    /// A per-user service (systemd user unit, LaunchAgent). Not a thing on
    /// Windows, where autostart fills the role.
    pub user_service: bool,
    /// Desktop notifications can be raised from this process.
    pub notifications: bool,
    /// Microsoft ships a first-party OneDrive client here.
    pub native_onedrive: bool,
    /// Cloud files can be pinned ("Always keep on this device") in code.
    pub pin_cloud_files: bool,
    /// The OS will tell us whether the connection is metered.
    pub metered_detection: bool,
    /// The OS will tell us about the battery.
    pub battery_detection: bool,
    /// The OS sends us suspend/resume events (as opposed to us inferring them
    /// from a clock gap, which always works).
    pub power_events: bool,
}

/// The capability set for the platform this build targets.
pub const fn capabilities() -> Capabilities {
    Capabilities {
        autostart: true,
        system_service: cfg!(any(windows, unix)),
        // Windows has no per-user service; a Windows service always lives in
        // session 0 whatever account it runs as.
        user_service: cfg!(all(unix, not(target_os = "macos"))) || cfg!(target_os = "macos"),
        notifications: true,
        native_onedrive: cfg!(any(windows, target_os = "macos")),
        pin_cloud_files: cfg!(windows),
        metered_detection: cfg!(any(windows, all(unix, not(target_os = "macos")))),
        battery_detection: true,
        // Only the Windows service receives real power events, via the SCM
        // control handler. Everything else uses the clock-gap detector.
        power_events: cfg!(windows),
    }
}

/// A limitation of this platform that the GUI should be prepared to explain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limitation {
    /// Stable code, for tests and for linking to documentation.
    pub code: String,
    /// Which part of the UI it belongs to: `onedrive`, `service`,
    /// `notifications`, `power`, `autostart`.
    pub area: String,
    /// One sentence, in the user's language, describing what does not work.
    pub message: String,
    /// What the user can do instead, when there is something.
    #[serde(default)]
    pub remedy: Option<String>,
}

/// Owned `String`s rather than `&'static str` so a `Limitation` survives the
/// round trip through IPC into the GUI process; this keeps the list below
/// readable.
fn limitation(code: &str, area: &str, message: &str, remedy: Option<&str>) -> Limitation {
    Limitation {
        code: code.to_string(),
        area: area.to_string(),
        message: message.to_string(),
        remedy: remedy.map(str::to_string),
    }
}

/// Every limitation that applies to the platform this build targets.
///
/// This list exists because the alternative is a user discovering these one at
/// a time, each as a bug report. It is deliberately blunt.
pub fn limitations() -> Vec<Limitation> {
    let mut out = Vec::new();

    if cfg!(windows) {
        out.push(limitation(
            "win.service_no_user_profile",
            "service",
            "A service running as Local System cannot see your OneDrive folder or any mapped              network drive, and cannot read passwords you saved in Windows Credential Manager.",
            Some(
                "Install the service under your own account, or leave those destinations to the                  tray app.",
            ),
        ));
        out.push(limitation(
            "win.service_no_ui",
            "notifications",
            "Windows isolates services from the desktop, so the service itself cannot show              notifications or ask for your passphrase.",
            Some("Keep the tray app running; it shows notifications on the service's behalf."),
        ));
        out.push(limitation(
            "win.toast_requires_shortcut",
            "notifications",
            "Windows only shows notifications from a desktop application that has a Start-menu              shortcut carrying an AppUserModelID.",
            Some("Install superbackup with its installer rather than copying the .exe."),
        ));
        out.push(limitation(
            "win.files_on_demand",
            "onedrive",
            "OneDrive's Files On-Demand can replace a backup repository with online-only              placeholders, which stalls or breaks backups and restores.",
            Some(
                "superbackup pins the folder it creates; do not switch that off, and do not run                  Storage Sense over it.",
            ),
        ));
        out.push(limitation(
            "win.long_paths",
            "onedrive",
            "Windows still limits many tools to 260-character paths, and a repository adds              around 60 characters below the folder you choose.",
            Some("Choose a short path close to the root of the drive."),
        ));
        out.push(limitation(
            "win.install_needs_admin",
            "service",
            "Installing, removing or reconfiguring a Windows service requires Administrator              rights.",
            Some("Start superbackup with \"Run as administrator\" for those actions only."),
        ));
    }

    if cfg!(target_os = "linux") {
        out.push(limitation(
            "linux.no_native_onedrive",
            "onedrive",
            "Microsoft ships no OneDrive client for Linux. superbackup can use a third-party              client's folder, but cannot pin files or read sync state.",
            Some(
                "Configure your client to keep files downloaded, or back up to S3 or a local                  disk instead.",
            ),
        ));
        out.push(limitation(
            "linux.fuse_invisible_to_system_service",
            "service",
            "FUSE mounts (rclone, onedriver) belong to your login session, so a system-wide              service cannot see them.",
            Some("Install a user service instead - it needs no administrator rights."),
        ));
        out.push(limitation(
            "linux.user_service_needs_linger",
            "service",
            "A systemd user service stops when you log out unless lingering is enabled.",
            Some("Run `loginctl enable-linger $USER` to keep it running after logout."),
        ));
        out.push(limitation(
            "linux.metered_needs_networkmanager",
            "power",
            "Only NetworkManager reports whether a connection is metered. Without it,              superbackup assumes the connection is unmetered and backs up normally.",
            None,
        ));
        out.push(limitation(
            "linux.notifications_need_a_session",
            "notifications",
            "Desktop notifications need a D-Bus session, which a headless server or a system              service does not have.",
            Some("Notifications are written to the log instead."),
        ));
    }

    if cfg!(target_os = "macos") {
        out.push(limitation(
            "macos.no_metered_api",
            "power",
            "macOS offers no supported way to ask whether a connection is metered, so \"skip on              metered connections\" has no effect.",
            Some("Use a schedule, or pause backups manually while tethered."),
        ));
        out.push(limitation(
            "macos.no_programmatic_pinning",
            "onedrive",
            "macOS has no API for \"Always Keep on This Device\", so superbackup cannot pin the              backup folder for you.",
            Some(
                "Right-click the backup folder in Finder and choose \"Always Keep on This                  Device\".",
            ),
        ));
        out.push(limitation(
            "macos.full_disk_access",
            "service",
            "macOS blocks access to Desktop, Documents, Downloads and other protected folders              until the app is granted Full Disk Access.",
            Some("Grant superbackup Full Disk Access in System Settings > Privacy & Security."),
        ));
    }

    out.push(limitation(
        "all.sleep_detection_is_inferred",
        "power",
        "Backups missed while the machine was asleep or switched off are detected by comparing          clocks, so a large manual clock change can look like a sleep.",
        None,
    ));

    out
}

/// A snapshot of the platform, for the About screen and for bug reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub elevated: bool,
    pub capabilities: Capabilities,
    pub limitations: Vec<Limitation>,
}

pub fn platform_info() -> PlatformInfo {
    PlatformInfo {
        os: std::env::consts::OS.to_string(),
        os_version: identity::detect_os_version(),
        arch: std::env::consts::ARCH.to_string(),
        elevated: service::is_elevated(),
        capabilities: capabilities(),
        limitations: limitations(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_are_consistent_with_the_target() {
        let caps = capabilities();
        assert!(caps.autostart, "every supported platform can start at login");
        assert!(caps.battery_detection);
        if cfg!(windows) {
            assert!(caps.pin_cloud_files);
            assert!(caps.native_onedrive);
            assert!(caps.metered_detection);
            assert!(!caps.user_service, "Windows services always live in session 0");
        }
        if cfg!(target_os = "macos") {
            assert!(!caps.metered_detection, "macOS exposes no metered API");
            assert!(!caps.pin_cloud_files);
        }
        if cfg!(target_os = "linux") {
            assert!(!caps.native_onedrive);
            assert!(caps.user_service);
        }
    }

    #[test]
    fn every_limitation_is_a_complete_sentence_with_a_stable_code() {
        let limits = limitations();
        assert!(!limits.is_empty());
        let mut codes: Vec<&str> = limits.iter().map(|l| l.code.as_str()).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(before, codes.len(), "limitation codes must be unique");
        for limit in &limits {
            assert!(limit.message.ends_with('.'), "not a sentence: {}", limit.message);
            assert!(!limit.area.is_empty());
            assert!(limit.code.contains('.'), "codes are namespaced: {}", limit.code);
        }
    }

    #[test]
    fn windows_limitations_name_the_local_system_problem() {
        if !cfg!(windows) {
            return;
        }
        let limits = limitations();
        let service = limits
            .iter()
            .find(|l| l.code == "win.service_no_user_profile")
            .expect("the OneDrive-under-LocalSystem limitation must be advertised");
        assert!(service.message.contains("OneDrive"));
        assert!(service.remedy.is_some());
    }

    #[test]
    fn proc_status_uid_uses_the_effective_id() {
        let status = "Name:\tsuperbackup\nUid:\t1000\t1001\t1000\t1000\nGid:\t1000\t1000\n";
        assert_eq!(parse_proc_status_uid(status), Some(1001));
        assert_eq!(parse_proc_status_uid("Name:\tx\n"), None);
    }

    #[test]
    fn disk_space_answers_for_the_temp_directory() {
        let (available, total) =
            disk_space(&std::env::temp_dir()).expect("the temp directory is on a real filesystem");
        assert!(total > 0, "a mounted volume has a size");
        assert!(available <= total);
    }

    #[test]
    fn disk_space_for_a_path_that_does_not_exist_yet_uses_its_parent() {
        let candidate = std::env::temp_dir().join("sb-not-created-yet").join("deeper");
        // Must not panic, and on Windows must walk up to an existing directory.
        let answer = disk_space(&candidate);
        if cfg!(windows) {
            assert!(answer.is_some(), "Windows must answer from the nearest existing parent");
        }
    }

    #[test]
    fn platform_info_is_specific_about_the_os() {
        let info = platform_info();
        assert_eq!(info.os, std::env::consts::OS);
        assert!(!info.os_version.is_empty(), "a generic OS name is not good enough");
        if cfg!(windows) {
            assert!(info.os_version.starts_with("Windows"), "{}", info.os_version);
        }
    }
}
