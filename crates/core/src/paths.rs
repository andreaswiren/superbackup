//! Where superbackup keeps its files, on every supported platform.
//!
//! | Purpose | Windows | Linux | macOS |
//! |---|---|---|---|
//! | Config | `%APPDATA%\superbackup` | `~/.config/superbackup` | `~/Library/Application Support/superbackup` |
//! | Data / state | `%LOCALAPPDATA%\superbackup` | `~/.local/share/superbackup` | `~/Library/Application Support/superbackup` |
//! | Logs | `<data>\logs` | `<data>/logs` | `~/Library/Logs/superbackup` |
//! | Cache | `%LOCALAPPDATA%\superbackup\cache` | `~/.cache/superbackup` | `~/Library/Caches/superbackup` |
//!
//! When running as a system service there is no user profile to speak of, so
//! [`Paths::for_service`] switches to a machine-wide root
//! (`%PROGRAMDATA%\superbackup`, `/var/lib/superbackup`, `/Library/Application Support/superbackup`).
//!
//! Every location can be overridden with `SUPERBACKUP_HOME`, which is what the
//! test suite and portable installs use.

use crate::error::{Error, IoContext, Result};
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "superbackup";
pub const ENV_HOME: &str = "SUPERBACKUP_HOME";

/// Resolved filesystem layout for one running instance.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// True when these paths came from the machine-wide service root.
    pub service_scope: bool,
}

impl Paths {
    /// Paths for the interactive (per-user) instance.
    pub fn discover() -> Result<Paths> {
        if let Some(root) = std::env::var_os(ENV_HOME) {
            return Ok(Paths::rooted_at(PathBuf::from(root), false));
        }
        let dirs = directories::ProjectDirs::from("io", "superbackup", APP_NAME)
            .ok_or_else(|| Error::Config("no home directory for this user".into()))?;

        let data_dir = dirs.data_dir().to_path_buf();
        let log_dir = if cfg!(target_os = "macos") {
            home_relative("Library/Logs/superbackup").unwrap_or_else(|| data_dir.join("logs"))
        } else {
            data_dir.join("logs")
        };

        Ok(Paths {
            config_dir: dirs.config_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
            log_dir,
            data_dir,
            service_scope: false,
        })
    }

    /// The layout an interactive launch should open.
    ///
    /// [`Paths::discover`] answers "where does this user's installation live
    /// by default", which is the right answer exactly once — until somebody
    /// runs with `--home` and then double-clicks the executable. The two
    /// installations look identical from the outside: same icon, same window,
    /// same "Locked" screen, same elided path with the distinguishing part cut
    /// off the front. The master passphrase of one is simply wrong for the
    /// other, so the application asks for a passphrase that cannot work and
    /// says "That passphrase did not work. Passphrases are case sensitive."
    ///
    /// So the root of an explicitly chosen installation is remembered, and a
    /// launch with no `--home` reopens it. The pointer is followed only when
    /// it still names a vault: an installation that was deleted or moved must
    /// not leave the application unable to start at all.
    pub fn discover_last_used() -> Result<Paths> {
        // An explicit environment override outranks a remembered choice: it is
        // this launch speaking, not the last one.
        if std::env::var_os(ENV_HOME).is_some() {
            return Paths::discover();
        }
        match remembered_root() {
            Some(root) => Ok(Paths::rooted_at(root, false)),
            None => Paths::discover(),
        }
    }

    /// Paths for the machine-wide service instance, which must not depend on
    /// any interactive user profile.
    pub fn for_service() -> Result<Paths> {
        if let Some(root) = std::env::var_os(ENV_HOME) {
            return Ok(Paths::rooted_at(PathBuf::from(root), true));
        }
        let root = if cfg!(windows) {
            let program_data = std::env::var_os("PROGRAMDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
            program_data.join(APP_NAME)
        } else if cfg!(target_os = "macos") {
            PathBuf::from("/Library/Application Support").join(APP_NAME)
        } else {
            PathBuf::from("/var/lib").join(APP_NAME)
        };
        Ok(Paths::rooted_at(root, true))
    }

    /// A fully self-contained layout under one directory. Used by
    /// `SUPERBACKUP_HOME`, portable installs, and every integration test.
    pub fn rooted_at(root: impl Into<PathBuf>, service_scope: bool) -> Paths {
        // Absolute, always. `--home dist/demo-home` and the same folder named
        // in full are one directory, and a client that spelled it the short
        // way used to hash a different key and report "nothing is listening"
        // about the daemon it was looking straight at. The daemon also runs
        // somewhere else entirely — as a service, quite possibly in
        // `C:\Windows\System32` — where a relative root names a folder that
        // does not exist.
        //
        // `std::path::absolute` rather than `canonicalize`: the root usually
        // does not exist yet on a first run, and canonicalising a missing path
        // fails. It also returns Windows' `\?\` form, which then reads back
        // in error messages as something the user never typed.
        let root = root.into();
        let root = std::path::absolute(&root).unwrap_or(root);
        Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            log_dir: root.join("logs"),
            cache_dir: root.join("cache"),
            service_scope,
        }
    }

    /// The single directory this layout is rooted at, when it has one.
    ///
    /// A rooted layout keeps config, data, logs and cache in one folder — the
    /// shape [`Paths::rooted_at`] produces, and what `--home` names. The
    /// per-user default has no such folder: on Windows the configuration is in
    /// `%APPDATA%` and the data in `%LOCALAPPDATA%`, which is two trees and no
    /// single root to write down.
    pub fn root(&self) -> Option<PathBuf> {
        let parent = self.config_dir.parent()?;
        [&self.data_dir, &self.log_dir, &self.cache_dir]
            .iter()
            .all(|dir| dir.parent() == Some(parent))
            .then(|| parent.to_path_buf())
    }

    /// Create every directory, with restrictive permissions where the platform
    /// supports them. Safe to call repeatedly.
    pub fn ensure(&self) -> Result<()> {
        for dir in [&self.config_dir, &self.data_dir, &self.log_dir, &self.cache_dir] {
            std::fs::create_dir_all(dir).ctx(format!("creating directory {}", dir.display()))?;
        }
        // Logs and cache are hardened too. Logs carry third-party output from
        // kopia and git, which redaction is a safety net for rather than a
        // guarantee, so they are exactly as interesting to another local user
        // as the config is.
        harden_dir(&self.config_dir)?;
        harden_dir(&self.data_dir)?;
        harden_dir(&self.log_dir)?;
        harden_dir(&self.cache_dir)?;
        Ok(())
    }

    /// Non-secret configuration.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    /// The sealed vault holding every secret. This is the only file that is
    /// ever safe to sync to a Git repository.
    pub fn vault_file(&self) -> PathBuf {
        self.config_dir.join("config.sbvault")
    }

    /// Rolling backups of the vault, written before every mutation.
    pub fn vault_backup_dir(&self) -> PathBuf {
        self.config_dir.join("vault-backups")
    }

    /// Run history and job state, kept out of the config so that a config pull
    /// from Git never clobbers local history.
    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }

    /// Append-only newline-delimited JSON event log.
    pub fn event_log(&self) -> PathBuf {
        self.data_dir.join("events.ndjson")
    }

    /// Kopia's own config directory, kept separate from any kopia the user
    /// may run by hand so the two never fight over `repository.config`.
    pub fn kopia_config_dir(&self) -> PathBuf {
        self.data_dir.join("kopia")
    }

    /// The per-destination kopia config file.
    pub fn kopia_config_for(&self, destination_id: &uuid::Uuid) -> PathBuf {
        self.kopia_config_dir().join(format!("{destination_id}.config"))
    }

    pub fn kopia_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("kopia")
    }

    /// A bundled or downloaded kopia binary, when the user has no system one.
    pub fn bundled_kopia(&self) -> PathBuf {
        let exe = if cfg!(windows) { "kopia.exe" } else { "kopia" };
        self.data_dir.join("bin").join(exe)
    }

    /// Local clone of the remote configuration repository.
    pub fn remote_clone_dir(&self) -> PathBuf {
        self.cache_dir.join("remote-config")
    }

    /// A short stable tag identifying *this* configuration root.
    ///
    /// Derived from the config directory, so two instances rooted at different
    /// `SUPERBACKUP_HOME` values never share an endpoint or a lock. SHA-256
    /// rather than `DefaultHasher` because the value has to be identical across
    /// processes and across builds — the CLI computes it independently of the
    /// daemon and the two must agree.
    pub(crate) fn instance_tag(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(normalised_key(&self.config_dir).as_bytes());
        hex::encode(&h.finalize()[..4])
    }

    /// IPC endpoint: a named pipe on Windows, a unix socket elsewhere.
    ///
    /// The name incorporates [`Paths::instance_tag`], and that is not cosmetic.
    /// The Windows pipe namespace is machine-global: a fixed
    /// `\\.\pipe\superbackup` meant that a portable install, a second user's
    /// tray, and every integration test on the box all addressed **one**
    /// daemon, whatever `SUPERBACKUP_HOME` said. That surfaced as tests passing
    /// against a stray daemon left running from an unrelated run — with a
    /// different vault, already unlocked. The same collision existed on Unix
    /// whenever `XDG_RUNTIME_DIR` was set, since the socket lived there rather
    /// than under the root.
    ///
    /// The service instance additionally carries a `-service` suffix so a
    /// user-mode tray and a machine-wide service can coexist on one box.
    pub fn ipc_endpoint(&self) -> String {
        let suffix = if self.service_scope { "-service" } else { "" };
        let tag = self.instance_tag();
        if cfg!(windows) {
            format!(r"\\.\pipe\superbackup{suffix}-{tag}")
        } else if self.service_scope {
            format!("/run/superbackup/superbackup{suffix}-{tag}.sock")
        } else {
            self.runtime_dir().join(format!("superbackup{suffix}-{tag}.sock")).display().to_string()
        }
    }

    /// `$XDG_RUNTIME_DIR` when available, else a short directory under the
    /// system temporary folder.
    ///
    /// # Why not the data directory
    ///
    /// Because it does not fit. A Unix socket path lives in `sun_path`, which
    /// is **104 bytes on macOS** and 108 on Linux — a limit in the kernel
    /// struct, not the filesystem — and `bind` on a longer path fails with an
    /// error that says nothing whatever about length.
    ///
    /// macOS never sets `XDG_RUNTIME_DIR`, and its data directory is
    /// `~/Library/Application Support/io.superbackup.superbackup`. With
    /// `/run/superbackup-<tag>.sock` on the end that is 92 bytes before the
    /// username, leaving eleven characters for it: `alice` fitted and
    /// `andreas.wiren` did not. On those machines the daemon never bound, the
    /// window said nothing was listening, and no backup ever ran.
    ///
    /// So the fallback is short by construction. The directory still belongs
    /// to this instance and is still private — `Server::bind` restricts the
    /// socket's *parent* to 0700, which is what keeps another local user out
    /// of the endpoint, so putting the socket straight into the temporary
    /// folder would make `bind` try to chmod a directory that is not ours.
    /// Short and private, not short instead of private.
    pub fn runtime_dir(&self) -> PathBuf {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            return PathBuf::from(dir).join(APP_NAME);
        }
        std::env::temp_dir().join(format!("sb-{}", self.instance_tag()))
    }

    /// Single-instance lock, so two trays never drive the same repositories.
    pub fn lock_file(&self) -> PathBuf {
        self.data_dir.join("superbackup.lock")
    }
}

fn home_relative(sub: &str) -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().join(sub))
}

/// Restrict a directory to the current user where the platform allows it.
///
/// On Unix this is a straight `chmod 0700`. On Windows the inherited ACL from
/// `%APPDATA%` is already user-scoped, and rewriting the DACL by hand is a
/// reliable way to lock a user out of their own config, so we leave it alone
/// and rely on the vault's encryption rather than on filesystem permissions.
pub fn harden_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta =
            std::fs::metadata(path).ctx(format!("reading permissions of {}", path.display()))?;
        let mut perms = meta.permissions();
        if perms.mode() & 0o077 != 0 {
            perms.set_mode(0o700);
            std::fs::set_permissions(path, perms)
                .ctx(format!("restricting permissions on {}", path.display()))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Same idea, for a single file (0600 on Unix).
/// The file naming the configuration root last opened on purpose.
///
/// One line, one absolute path, no secrets — a path is not one, and this file
/// is read before anything is unlocked.
pub const LAST_ROOT_FILE: &str = "last-home";

/// Overrides [`state_dir`]. For tests, portable installs, and anyone who keeps
/// their profile somewhere the defaults do not expect.
pub const ENV_STATE_DIR: &str = "SUPERBACKUP_STATE_DIR";

/// Where superbackup keeps the few facts that belong to the *user* rather than
/// to any one installation.
///
/// `~/.superbackup` on every platform, and the same folder whichever
/// installation is running — which is the whole point. The pointer to the
/// last-opened root cannot live inside an installation, because it is what
/// gets consulted when no installation has been named yet: putting it in one
/// of them makes it invisible from the other, which is exactly the situation
/// it exists to resolve.
///
/// Not `%PROGRAMDATA%` either, tempting as a machine-wide folder looks. Which
/// installation *this person* last opened is a fact about this person; two
/// accounts on one PC would overwrite each other's answer, and the service
/// never needs it, because a service is given its root explicitly or uses
/// [`Paths::for_service`].
///
/// `SUPERBACKUP_STATE_DIR` overrides it.
pub fn state_dir() -> Option<PathBuf> {
    match std::env::var_os(ENV_STATE_DIR) {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => home_relative(".superbackup"),
    }
}

/// Record which configuration root was opened, so the next bare launch finds
/// it again. Best effort by design — see [`remember_root_in`].
pub fn remember_root(root: &Path) -> Result<()> {
    match state_dir() {
        Some(dir) => remember_root_in(&dir, root),
        None => Ok(()),
    }
}

/// The remembered root, when there is one and it still holds a vault.
pub fn remembered_root() -> Option<PathBuf> {
    remembered_root_in(&state_dir()?)
}

/// As [`remember_root`], against a named directory. Split out for the tests,
/// which must not write into the developer's own installation.
///
/// Writing the pointer is never worth failing a launch over: the worst case is
/// that the next bare launch opens the default installation, which is where it
/// would have gone anyway. So a directory that cannot be created is not an
/// error — but a write that fails once the directory exists is, because that
/// is a disk saying something the caller should hear.
pub fn remember_root_in(dir: &Path, root: &Path) -> Result<()> {
    if std::fs::create_dir_all(dir).is_err() {
        return Ok(());
    }
    let _ = harden_dir(dir);
    let path = dir.join(LAST_ROOT_FILE);
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    write_atomic(&path, root.to_string_lossy().trim().as_bytes())?;
    harden_file(&path)
}

/// As [`remembered_root`], against a named directory.
///
/// Every failure is "no remembered root": a missing file, an unreadable one,
/// a blank line, a path that no longer exists, a path that exists but holds no
/// vault. The pointer is a convenience, and a convenience that can stop the
/// application from starting is a defect.
pub fn remembered_root_in(dir: &Path) -> Option<PathBuf> {
    let recorded = std::fs::read_to_string(dir.join(LAST_ROOT_FILE)).ok()?;
    let recorded = recorded.trim();
    if recorded.is_empty() {
        return None;
    }
    let root = PathBuf::from(recorded);
    // A vault, not merely a folder. An installation that was deleted, renamed,
    // or lives on a drive that is not mounted this morning leaves a pointer
    // behind, and following it would replace "your passphrase does not work"
    // with "there is nothing here at all" — a different way to be stuck.
    crate::crypto::file::VaultFile::exists(&Paths::rooted_at(&root, false)).then_some(root)
}

pub fn harden_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta =
            std::fs::metadata(path).ctx(format!("reading permissions of {}", path.display()))?;
        let mut perms = meta.permissions();
        if perms.mode() & 0o177 != 0 {
            perms.set_mode(0o600);
            std::fs::set_permissions(path, perms)
                .ctx(format!("restricting permissions on {}", path.display()))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Write a file atomically: temp file in the same directory, flush, fsync,
/// then rename over the target. A crash leaves either the old file or the new
/// one, never a truncated mixture — which for the vault is the difference
/// between an inconvenience and a total loss of every repository key.
///
/// # Two mistakes this function used to make
///
/// Both were found by adversarial review, and both could destroy a vault.
///
/// 1. It unlinked the destination first on Windows, believing `rename` could
///    not replace an existing file. That is **false**: `std::fs::rename` maps
///    to `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` and replaces the
///    destination atomically. The unlink was not merely redundant — it opened
///    a window in which the vault existed nowhere, and a sharing violation
///    from an antivirus scanner holding the temp file (an ordinary event on
///    Windows) turned that window into permanent loss.
/// 2. Its error path deleted the temporary file. Combined with (1) that
///    destroyed *both* copies. The temp file is now deliberately left behind
///    on failure, under a recognisable name, so a human can recover from it.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;

    let dir = path
        .parent()
        .ok_or_else(|| Error::Path { path: path.into(), reason: "has no parent".into() })?;
    std::fs::create_dir_all(dir).ctx(format!("creating {}", dir.display()))?;

    // Random suffix, not just the pid: two writers in one process targeting the
    // same file would otherwise collide on one temp path, and the second
    // `create` would truncate the first's buffer mid-write.
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{:08x}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id(),
        rand::random::<u32>()
    ));

    {
        // Create with restrictive permissions from the outset rather than
        // widening then narrowing: the previous ordering wrote the plaintext
        // bytes under the default umask and only chmod'd afterwards, leaving a
        // window in which the file was world-readable.
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).ctx(format!("creating temporary file {}", tmp.display()))?;
        f.write_all(contents).ctx("writing temporary file")?;
        f.flush().ctx("flushing temporary file")?;
        f.sync_all().ctx("syncing temporary file")?;
    }
    harden_file(&tmp)?;

    // `rename` replaces the destination atomically on every supported platform.
    // Nothing is unlinked first, so the target always names either the old
    // contents or the new ones. On failure the temp file survives.
    std::fs::rename(&tmp, path).map_err(|e| {
        Error::io(
            format!(
                "replacing {} (the new contents were written to {} and have been left there)",
                path.display(),
                tmp.display()
            ),
            e,
        )
    })?;

    // Fsync the directory so the rename itself is durable (no-op on Windows).
    #[cfg(unix)]
    {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// The directory, reduced to a form that is stable across the ways one path can
/// be spelled.
///
/// The tag is a hash of the configuration directory, and it decides which pipe
/// or socket a client addresses. Hashing the raw bytes meant that
/// `SUPERBACKUP_HOME=C:/x` and `SUPERBACKUP_HOME=C:\x` — the same directory,
/// and interchangeable everywhere else on Windows — produced two different
/// endpoints, so the CLI could not find the daemon it had just started. Case
/// differed the same way, and Windows paths are case-insensitive.
///
/// Only separators and case are normalised, and case only where the platform
/// is actually case-insensitive. Nothing here canonicalises: the directory may
/// not exist yet, and resolving symlinks would make the endpoint depend on the
/// state of the filesystem rather than on what the user asked for.
fn normalised_key(dir: &Path) -> String {
    let text = dir.to_string_lossy().replace('\\', "/");
    // Trailing separators are noise: `C:/x` and `C:/x/` are one directory.
    let text = text.trim_end_matches('/').to_string();
    if cfg!(windows) || cfg!(target_os = "macos") {
        text.to_lowercase()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {

    use std::path::{Path, PathBuf};

    /// A scratch directory that cleans up after itself.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-paths-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// Give `root` a vault file, so it counts as an installation.
    fn install_at(root: &Path) {
        let paths = super::Paths::rooted_at(root, false);
        std::fs::create_dir_all(&paths.config_dir).expect("config dir");
        std::fs::write(paths.vault_file(), b"not a real vault, but a real file").expect("vault");
    }

    /// The pointer survives a round trip, and names the installation.
    ///
    /// This is what stops the second installation from swallowing the
    /// passphrase. Two of them look identical from the outside — same icon,
    /// same window, same "Locked" screen — and the master passphrase of one is
    /// simply wrong for the other, so a launch that lands on the wrong one
    /// presents as a passphrase that has stopped working.
    #[test]
    fn the_last_opened_installation_is_remembered() {
        let dir = scratch("remember");
        let state = dir.join("state");
        let root = dir.join("somewhere-else");
        install_at(&root);

        assert_eq!(super::remembered_root_in(&state), None, "nothing recorded yet");
        super::remember_root_in(&state, &root).expect("record it");
        assert_eq!(super::remembered_root_in(&state).as_deref(), Some(root.as_path()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pointer to an installation that is no longer there is ignored.
    ///
    /// A deleted folder, a renamed one, an external drive that is not plugged
    /// in this morning. Following the pointer anyway would replace "your
    /// passphrase does not work" with "there is nothing here at all", which is
    /// a different way to be stuck — so it falls back to the default, which is
    /// where a launch with no pointer goes anyway.
    #[test]
    fn a_pointer_to_nothing_is_ignored() {
        let dir = scratch("stale");
        let state = dir.join("state");
        let root = dir.join("was-here");
        install_at(&root);
        super::remember_root_in(&state, &root).expect("record it");

        std::fs::remove_dir_all(&root).expect("remove the installation");
        assert_eq!(
            super::remembered_root_in(&state),
            None,
            "a folder with no vault in it is not an installation"
        );

        // A folder that exists but holds no vault is equally not one.
        std::fs::create_dir_all(&root).expect("empty folder");
        assert_eq!(super::remembered_root_in(&state), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A blank or unreadable pointer is "no pointer", never an error.
    #[test]
    fn a_damaged_pointer_is_not_an_error() {
        let dir = scratch("damaged");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("state dir");
        std::fs::write(
            state.join(super::LAST_ROOT_FILE),
            b"   
  ",
        )
        .expect("blank");
        assert_eq!(super::remembered_root_in(&state), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The recorded path is absolute, whatever was passed in.
    ///
    /// The next launch happens from a different working directory — a Start
    /// menu shortcut, a login, a service — where a relative path names a
    /// folder that does not exist.
    #[test]
    fn the_recorded_path_is_absolute() {
        let dir = scratch("absolute");
        let state = dir.join("state");
        let root = dir.join("rooted");
        install_at(&root);
        super::remember_root_in(&state, &root).expect("record it");
        let recorded = std::fs::read_to_string(state.join(super::LAST_ROOT_FILE)).expect("read");
        assert!(Path::new(recorded.trim()).is_absolute(), "{recorded}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only a layout with one folder has a root to write down.
    ///
    /// The per-user default has not: on Windows the configuration lives in
    /// `%APPDATA%` and the data in `%LOCALAPPDATA%`, which is two trees. There
    /// is nothing to record, and nothing needs recording, because that is
    /// where a launch with no pointer goes anyway.
    #[test]
    fn only_a_single_rooted_layout_has_a_root() {
        let rooted = super::Paths::rooted_at("/tmp/sb-root-check", false);
        assert_eq!(
            rooted.root().as_deref(),
            Some(std::path::absolute("/tmp/sb-root-check").unwrap().as_path()),
        );

        let split = super::Paths {
            config_dir: PathBuf::from("/one/config"),
            data_dir: PathBuf::from("/two/data"),
            log_dir: PathBuf::from("/two/logs"),
            cache_dir: PathBuf::from("/three/cache"),
            service_scope: false,
        };
        assert_eq!(split.root(), None, "four folders in three trees have no single root");
    }

    /// The endpoint decides which daemon a client talks to, so two spellings
    /// of one directory must not produce two endpoints.
    ///
    /// This is not hypothetical: `SUPERBACKUP_HOME=C:/x` from a bash shell and
    /// the same path with backslashes from PowerShell addressed different
    /// pipes, so the CLI reported "nothing is listening" about a daemon it had
    /// One folder, two spellings, one daemon.
    ///
    /// `--home dist/demo-home` from the repository root and the same folder
    /// named in full are the same place, and a user who typed the short form
    /// was told nothing was listening on a pipe the daemon was serving.
    #[test]
    fn a_relative_home_addresses_the_same_instance_as_the_absolute_one() {
        let cwd = std::env::current_dir().expect("cwd");
        let relative = super::Paths::rooted_at("some-home", false);
        let absolute = super::Paths::rooted_at(cwd.join("some-home"), false);
        assert_eq!(
            relative.ipc_endpoint(),
            absolute.ipc_endpoint(),
            "the same folder, spelled two ways, must be one daemon"
        );
        assert_eq!(relative.config_dir, absolute.config_dir);
        // And the stored path is the absolute one, because the daemon may be
        // running from a completely different working directory.
        assert!(relative.config_dir.is_absolute(), "{:?}", relative.config_dir);
    }

    /// started itself moments earlier.
    #[test]
    fn one_directory_spelled_two_ways_is_one_endpoint() {
        let a = Paths::rooted_at(r"C:\Users\andreas\sb", false);
        let b = Paths::rooted_at("C:/Users/andreas/sb", false);
        assert_eq!(a.ipc_endpoint(), b.ipc_endpoint(), "separators must not change the endpoint");

        let trailing = Paths::rooted_at("C:/Users/andreas/sb/", false);
        assert_eq!(a.ipc_endpoint(), trailing.ipc_endpoint(), "a trailing separator is noise");
    }

    #[test]
    fn case_is_ignored_only_where_the_platform_ignores_it() {
        let lower = Paths::rooted_at("C:/users/andreas/sb", false);
        let upper = Paths::rooted_at("C:/Users/Andreas/SB", false);
        if cfg!(windows) || cfg!(target_os = "macos") {
            assert_eq!(lower.ipc_endpoint(), upper.ipc_endpoint());
        } else {
            // Linux paths are case-sensitive, and two directories that really
            // are different must keep their own daemons.
            assert_ne!(lower.ipc_endpoint(), upper.ipc_endpoint());
        }
    }

    #[test]
    fn genuinely_different_homes_still_get_their_own_endpoint() {
        // The whole reason the tag exists: a portable install, a second user's
        // tray and every integration test must not share one daemon.
        let a = Paths::rooted_at("C:/one", false);
        let b = Paths::rooted_at("C:/two", false);
        assert_ne!(a.ipc_endpoint(), b.ipc_endpoint());
    }
    use super::*;

    /// A root already absolute on this platform, so the test asserts the
    /// layout rather than accidentally asserting how `absolute` rewrites a
    /// Unix-shaped path on Windows.
    fn absolute_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn rooted_layout_is_self_contained() {
        let root = absolute_root("sb-test");
        let p = Paths::rooted_at(&root, false);
        assert!(p.config_file().starts_with(&root), "{:?}", p.config_file());
        assert!(p.vault_file().ends_with("config.sbvault"));
        assert!(p.state_file().starts_with(&root), "{:?}", p.state_file());
    }

    #[test]
    fn service_endpoint_differs_from_user_endpoint() {
        let user = Paths::rooted_at(absolute_root("sb-a"), false);
        let svc = Paths::rooted_at(absolute_root("sb-b"), true);
        assert_ne!(user.ipc_endpoint(), svc.ipc_endpoint());
    }

    #[test]
    fn different_homes_never_share_an_endpoint() {
        // The Windows pipe namespace is machine-global, so a fixed name meant
        // every install and every test on the box addressed one daemon
        // regardless of SUPERBACKUP_HOME. Tests then passed against a stray
        // daemon holding a different, already-unlocked vault.
        let a = Paths::rooted_at("/tmp/sb-one", false);
        let b = Paths::rooted_at("/tmp/sb-two", false);
        assert_ne!(
            a.ipc_endpoint(),
            b.ipc_endpoint(),
            "two configuration roots must not address the same daemon"
        );
    }

    #[test]
    fn the_endpoint_is_stable_across_processes() {
        // The CLI derives this independently of the daemon; if it were not
        // reproducible they would never find each other.
        let a = Paths::rooted_at("/tmp/sb-stable", false);
        let b = Paths::rooted_at("/tmp/sb-stable", false);
        assert_eq!(a.ipc_endpoint(), b.ipc_endpoint());
    }

    #[test]
    fn atomic_write_replaces_existing_content() {
        let dir = std::env::temp_dir().join(format!("sb-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("f.json");
        write_atomic(&target, b"first").unwrap();
        write_atomic(&target, b"second").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files were left behind");
        std::fs::remove_dir_all(&dir).ok();
    }
}
