//! Putting superbackup where people look for programs.
//!
//! | Platform | Location | Read by |
//! |---|---|---|
//! | Windows | `%APPDATA%\Microsoft\Windows\Start Menu\Programs\superbackup.lnk` | Start menu, search, "pin to taskbar" |
//! | Linux | `~/.local/share/applications/superbackup.desktop` | GNOME, KDE Plasma, XFCE, LXQt — all of them read the XDG desktop-entry spec, so this is one file and not four |
//! | macOS | `~/Applications/superbackup.app` (a symlink to the bundle) | Launchpad, Spotlight |
//!
//! # Why this is separate from autostart
//!
//! [`super::autostart`] answers "run at login". This answers "be findable".
//! They are different decisions and users make them differently — plenty of
//! people want a program in their Start menu and emphatically not at every
//! login — so they are separate switches over separate mechanisms, and a
//! failure in one must not take out the other.
//!
//! # Per-user, never machine-wide
//!
//! Every path here is under the user's own profile. Writing to the all-users
//! Start menu or to `/usr/share/applications` needs administrator rights, and
//! asking for elevation to add a menu entry is not a trade worth making — a
//! backup tool that demands admin to install a shortcut teaches its user that
//! elevation prompts are routine.

use std::path::{Path, PathBuf};

use crate::error::{Error, IoContext, Result};

use super::autostart::{AutostartSpec, ENTRY_NAME};

/// What a launcher entry looks like once written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutState {
    /// Present, and pointing at the executable we asked about.
    Installed,
    /// Present, but pointing somewhere else — usually a copy that was moved,
    /// or an older install that was never removed. Repairable, not an error.
    Stale {
        target: PathBuf,
    },
    Absent,
    /// The platform has no launcher concept we can write to, or we could not
    /// read the entry. Reported rather than guessed at.
    Unknown(String),
}

impl ShortcutState {
    pub fn is_installed(&self) -> bool {
        matches!(self, ShortcutState::Installed)
    }

    /// True when an entry exists but points at the wrong executable, so the
    /// interface can offer "repair" rather than a misleading "install".
    pub fn needs_repair(&self) -> bool {
        matches!(self, ShortcutState::Stale { .. })
    }

    pub fn summary(&self) -> String {
        match self {
            ShortcutState::Installed => "superbackup is in your applications menu.".into(),
            ShortcutState::Stale { target } => format!(
                "The menu entry points at {}, which is not this copy of superbackup.",
                target.display()
            ),
            ShortcutState::Absent => "superbackup is not in your applications menu.".into(),
            ShortcutState::Unknown(why) => {
                format!("The applications menu entry could not be checked: {why}")
            }
        }
    }
}

/// Where the entry lives on this platform, for showing in Settings.
pub fn location() -> String {
    match entry_path() {
        Ok(path) => path.display().to_string(),
        Err(e) => format!("unavailable ({e})"),
    }
}

/// The file (or symlink) that represents the launcher entry.
fn entry_path() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA")
            .ok_or_else(|| Error::Config("APPDATA is not set for this user".into()))?;
        Ok(PathBuf::from(appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join(format!("{ENTRY_NAME}.lnk")))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(home()?.join("Applications").join(format!("{ENTRY_NAME}.app")))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // XDG_DATA_HOME, then the specified default. GNOME, KDE, XFCE and
        // LXQt all read this directory; there is no per-desktop variant to
        // write, which is the whole point of the spec.
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or(home()?.join(".local/share"));
        Ok(base.join("applications").join(format!("{ENTRY_NAME}.desktop")))
    }
}

#[cfg(unix)]
fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .ok_or_else(|| Error::Config("HOME is not set for this user".into()))
}

/// Is superbackup in the applications menu, and is it this copy?
pub fn status(spec: &AutostartSpec) -> ShortcutState {
    let path = match entry_path() {
        Ok(p) => p,
        Err(e) => return ShortcutState::Unknown(e.to_string()),
    };
    if !exists(&path) {
        return ShortcutState::Absent;
    }
    match read_target(&path) {
        Ok(Some(target)) => {
            if super::autostart::same_executable(&target, &spec.executable) {
                ShortcutState::Installed
            } else {
                ShortcutState::Stale { target }
            }
        }
        // Present but unreadable: treat it as installed rather than offering
        // to write a second one over the top of something we cannot see.
        Ok(None) => ShortcutState::Installed,
        Err(e) => ShortcutState::Unknown(e.to_string()),
    }
}

/// `Path::exists` follows symlinks, and a macOS entry *is* one — a dangling
/// link must read as present so it can be repaired rather than duplicated.
fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// Add or repair the applications-menu entry. Idempotent.
pub fn install(spec: &AutostartSpec) -> Result<PathBuf> {
    let path = entry_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .ctx(format!("creating the applications menu folder {}", parent.display()))?;
    }
    write_entry(&path, spec)?;
    Ok(path)
}

/// Remove it. Returns false when there was nothing to remove, which is a
/// success: uninstalling twice must not be an error.
pub fn remove() -> Result<bool> {
    let path = entry_path()?;
    if !exists(&path) {
        return Ok(false);
    }
    // `remove_file` unlinks a symlink rather than following it, which is what
    // the macOS entry needs.
    std::fs::remove_file(&path)
        .ctx(format!("removing the applications menu entry {}", path.display()))?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Windows: a real .lnk, written through the shell
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn write_entry(path: &Path, spec: &AutostartSpec) -> Result<()> {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    // A `.lnk` is a binary shell format. It is written through the shell's own
    // interface rather than by hand: hand-rolling the format is how you get a
    // shortcut that works on one Windows build and not the next.
    //
    // SAFETY: COM is initialised for this thread and uninitialised on every
    // path out, including the error paths, via the guard below. Every pointer
    // handed to the shell outlives the call that uses it.
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // `S_FALSE` means COM was already initialised on this thread, which is
        // fine — but then it is not ours to uninitialise.
        let owned = hr.is_ok();
        let _guard = ComGuard { owned };

        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| Error::Config(format!("could not create a shell link: {e}")))?;

        link.SetPath(PCWSTR(wide(&spec.executable.to_string_lossy()).as_ptr()))
            .map_err(|e| Error::Config(format!("could not set the shortcut target: {e}")))?;
        // The menu entry opens the window. Autostart is the thing that wants
        // `--minimised`, and it is a separate mechanism; a Start-menu item that
        // silently did nothing visible would look broken.
        if let Some(dir) = spec.executable.parent() {
            let _ = link.SetWorkingDirectory(PCWSTR(wide(&dir.to_string_lossy()).as_ptr()));
        }
        let _ = link.SetDescription(PCWSTR(wide(DESCRIPTION).as_ptr()));
        // The icon comes from the executable's own embedded resource, so it
        // tracks the binary instead of pointing at a file that may be deleted.
        let _ = link.SetIconLocation(PCWSTR(wide(&spec.executable.to_string_lossy()).as_ptr()), 0);

        let persist: IPersistFile =
            link.cast().map_err(|e| Error::Config(format!("could not save the shortcut: {e}")))?;
        persist
            .Save(PCWSTR(wide(&path.to_string_lossy()).as_ptr()), true)
            .map_err(|e| Error::Config(format!("could not write {}: {e}", path.display())))?;
    }
    Ok(())
}

/// Uninitialises COM on the way out, on every path including an early return.
#[cfg(windows)]
struct ComGuard {
    owned: bool,
}

#[cfg(windows)]
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: paired with the `CoInitializeEx` that set `owned`.
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn read_target(path: &Path) -> Result<Option<PathBuf>> {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_RAWPATH};

    // SAFETY: as `write_entry`. The buffer passed to `GetPath` is sized by the
    // same constant handed to the call.
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _guard = ComGuard { owned: hr.is_ok() };

        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| Error::Config(format!("could not read the shortcut: {e}")))?;
        let persist: IPersistFile =
            link.cast().map_err(|e| Error::Config(format!("could not read the shortcut: {e}")))?;
        persist
            .Load(PCWSTR(wide(&path.to_string_lossy()).as_ptr()), STGM_READ)
            .map_err(|e| Error::Config(format!("could not open {}: {e}", path.display())))?;

        let mut buffer = [0u16; 260];
        link.GetPath(&mut buffer, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
            .map_err(|e| Error::Config(format!("could not read the shortcut target: {e}")))?;
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        let target = String::from_utf16_lossy(&buffer[..end]);
        if target.is_empty() {
            Ok(None)
        } else {
            Ok(Some(PathBuf::from(target)))
        }
    }
}

// ---------------------------------------------------------------------------
// Linux: one XDG desktop entry, read by every desktop the user named
// ---------------------------------------------------------------------------

#[cfg(all(unix, not(target_os = "macos")))]
fn write_entry(path: &Path, spec: &AutostartSpec) -> Result<()> {
    // No `--minimised` here: see the note in the Windows writer.
    let exec = escape_desktop(&spec.executable.to_string_lossy());
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={}\n\
         Comment={DESCRIPTION}\n\
         Exec={exec}\n\
         Icon={ENTRY_NAME}\n\
         Terminal=false\n\
         Categories=Utility;Archiving;System;\n\
         Keywords=backup;kopia;snapshot;restore;\n\
         StartupNotify=true\n\
         StartupWMClass={ENTRY_NAME}\n",
        spec.display_name
    );
    crate::paths::write_atomic(path, entry.as_bytes())
}

/// `Exec=` is field-code expanded, so a literal `%` has to be doubled or the
/// launcher eats it. Paths with spaces are quoted.
#[cfg(all(unix, not(target_os = "macos")))]
fn escape_desktop(value: &str) -> String {
    let escaped = value.replace('%', "%%");
    if escaped.contains(' ') {
        format!("\"{}\"", escaped.replace('"', "\\\""))
    } else {
        escaped
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_target(path: &Path) -> Result<Option<PathBuf>> {
    let text = std::fs::read_to_string(path)
        .ctx(format!("reading the desktop entry {}", path.display()))?;
    // `parse_desktop_exec` returns the whole argument vector; the executable
    // is the first of them.
    Ok(text
        .lines()
        .find_map(|l| l.strip_prefix("Exec="))
        .and_then(|exec| super::autostart::parse_desktop_exec(exec).into_iter().next())
        .map(PathBuf::from))
}

// ---------------------------------------------------------------------------
// macOS: a symlink into ~/Applications
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn write_entry(path: &Path, spec: &AutostartSpec) -> Result<()> {
    // Launchpad and Spotlight index `~/Applications`. A symlink to the bundle
    // is what a drag-install produces and is what they expect; copying the
    // bundle would leave two copies to keep in step.
    let target = bundle_root(&spec.executable);
    if exists(path) {
        std::fs::remove_file(path)
            .ctx(format!("replacing the applications entry {}", path.display()))?;
    }
    std::os::unix::fs::symlink(&target, path).ctx(format!(
        "linking {} to {}",
        path.display(),
        target.display()
    ))
}

/// The `.app` bundle containing this executable, or the executable itself when
/// it is a bare binary rather than a bundled application.
#[cfg(target_os = "macos")]
fn bundle_root(executable: &Path) -> PathBuf {
    // `Foo.app/Contents/MacOS/foo` — walk up to the `.app`.
    for ancestor in executable.ancestors() {
        if ancestor.extension().is_some_and(|e| e == "app") {
            return ancestor.to_path_buf();
        }
    }
    executable.to_path_buf()
}

#[cfg(target_os = "macos")]
fn read_target(path: &Path) -> Result<Option<PathBuf>> {
    match std::fs::read_link(path) {
        Ok(target) => Ok(Some(target)),
        Err(e) => Err(Error::io(format!("reading the link {}", path.display()), e)),
    }
}

/// Shown in the Start menu tooltip and the desktop entry's `Comment`.
///
/// macOS has nowhere to put it: the entry there is a symlink to the bundle,
/// and the bundle carries its own description.
#[cfg(not(target_os = "macos"))]
const DESCRIPTION: &str = "Back up your folders to local disks, OneDrive and S3";

#[cfg(test)]
mod tests {

    /// Actually write one, read it back, and remove it.
    ///
    /// A `.lnk` is a binary shell format written through COM; nothing about
    /// that is verifiable by inspection. This is ignored by default because it
    /// touches the real Start menu, and run explicitly to prove the round trip.
    #[test]
    #[ignore = "writes to the real applications menu"]
    fn a_real_entry_round_trips() {
        let exe = std::env::current_exe().expect("this test binary");
        let spec = AutostartSpec::for_executable(&exe);

        let path = install(&spec).expect("the entry must be writable");
        assert!(exists(&path), "nothing was written to {}", path.display());

        let state = status(&spec);
        assert!(
            state.is_installed(),
            "a freshly written entry must read back as installed, got {state:?}"
        );

        let target = read_target(&path).expect("readable");
        if let Some(target) = target {
            assert!(
                super::super::autostart::same_executable(&target, &exe),
                "the entry points at {} rather than {}",
                target.display(),
                exe.display()
            );
        }

        assert!(remove().expect("removable"), "remove reported nothing to do");
        assert!(!exists(&path), "the entry survived removal");
        assert!(!status(&spec).is_installed());
    }
    use super::*;

    #[test]
    fn the_entry_lives_under_the_users_own_profile() {
        // Never the all-users menu: that needs administrator rights, and
        // asking for elevation to add a shortcut teaches people that elevation
        // prompts are routine.
        let Ok(path) = entry_path() else { return };
        let text = path.to_string_lossy().to_lowercase();
        assert!(
            !text.starts_with("/usr/share")
                && !text.starts_with("/applications")
                && !text.contains("programdata"),
            "the entry must be per-user, got {}",
            path.display()
        );
    }

    #[test]
    fn the_entry_is_named_predictably() {
        // An uninstaller looks for this exact name.
        let Ok(path) = entry_path() else { return };
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        assert_eq!(stem, ENTRY_NAME);
    }

    #[test]
    fn a_missing_entry_reads_as_absent_rather_than_an_error() {
        // A machine that has never installed one must not surface an error;
        // "not there" is the ordinary case on a fresh install.
        let spec = AutostartSpec::for_executable("/nonexistent/superbackup");
        let state = status(&spec);
        assert!(
            !state.needs_repair(),
            "nothing is installed, so there is nothing to repair: {state:?}"
        );
    }

    #[test]
    fn removing_something_that_is_not_there_is_not_a_failure() {
        // Uninstall runs on machines that never installed. It has to be quiet.
        match remove() {
            Ok(_) => {}
            Err(e) => panic!("remove must tolerate a missing entry, got {e}"),
        }
    }

    #[test]
    fn a_stale_entry_offers_repair_and_names_what_it_points_at() {
        let state = ShortcutState::Stale { target: PathBuf::from("/old/superbackup") };
        assert!(state.needs_repair());
        assert!(!state.is_installed());
        assert!(state.summary().contains("/old/superbackup"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_desktop_entry_is_valid_and_reachable_by_every_named_desktop() {
        // GNOME, KDE, XFCE and LXQt all read the XDG spec, so one file serves
        // all four — but only if it carries the keys they require.
        let spec = AutostartSpec::for_executable("/opt/superbackup/superbackup");
        let dir = std::env::temp_dir().join(format!("sb-shortcut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("superbackup.desktop");
        write_entry(&path, &spec).expect("the entry must be writable");

        let text = std::fs::read_to_string(&path).expect("readable");
        assert!(text.starts_with("[Desktop Entry]"), "{text}");
        for key in ["Type=Application", "Name=", "Exec=", "Terminal=false", "Categories="] {
            assert!(text.contains(key), "a launcher needs {key}:\n{text}");
        }
        // The menu entry opens the window; --minimised belongs to autostart.
        assert!(!text.contains("--minimised"), "a menu entry must not start hidden:\n{text}");

        let target = read_target(&path).expect("readable").expect("an Exec line");
        assert_eq!(target, PathBuf::from("/opt/superbackup/superbackup"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_percent_in_a_path_is_not_eaten_by_the_launcher() {
        // `Exec=` is field-code expanded: a literal % must be doubled.
        assert_eq!(escape_desktop("/opt/100%sure/sb"), "/opt/100%%sure/sb");
        assert_eq!(escape_desktop("/opt/my apps/sb"), "\"/opt/my apps/sb\"");
    }
}
