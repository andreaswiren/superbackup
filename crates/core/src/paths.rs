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
        let root = root.into();
        Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            log_dir: root.join("logs"),
            cache_dir: root.join("cache"),
            service_scope,
        }
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
    fn instance_tag(&self) -> String {
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

    /// `$XDG_RUNTIME_DIR` when available, else the data directory.
    pub fn runtime_dir(&self) -> PathBuf {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|d| PathBuf::from(d).join(APP_NAME))
            .unwrap_or_else(|| self.data_dir.join("run"))
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

    /// The endpoint decides which daemon a client talks to, so two spellings
    /// of one directory must not produce two endpoints.
    ///
    /// This is not hypothetical: `SUPERBACKUP_HOME=C:/x` from a bash shell and
    /// the same path with backslashes from PowerShell addressed different
    /// pipes, so the CLI reported "nothing is listening" about a daemon it had
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

    #[test]
    fn rooted_layout_is_self_contained() {
        let p = Paths::rooted_at("/tmp/sb-test", false);
        assert!(p.config_file().starts_with("/tmp/sb-test"));
        assert!(p.vault_file().ends_with("config.sbvault"));
        assert!(p.state_file().starts_with("/tmp/sb-test"));
    }

    #[test]
    fn service_endpoint_differs_from_user_endpoint() {
        let user = Paths::rooted_at("/tmp/sb-a", false);
        let svc = Paths::rooted_at("/tmp/sb-b", true);
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
