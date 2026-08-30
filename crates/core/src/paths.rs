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
            std::fs::create_dir_all(dir)
                .ctx(format!("creating directory {}", dir.display()))?;
        }
        harden_dir(&self.config_dir)?;
        harden_dir(&self.data_dir)?;
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

    /// IPC endpoint: a named pipe on Windows, a unix socket elsewhere.
    ///
    /// The service instance uses a distinct name so that a user-mode tray and
    /// a machine-wide service can coexist on one box.
    pub fn ipc_endpoint(&self) -> String {
        let suffix = if self.service_scope { "-service" } else { "" };
        if cfg!(windows) {
            format!(r"\\.\pipe\superbackup{suffix}")
        } else if self.service_scope {
            format!("/run/superbackup/superbackup{suffix}.sock")
        } else {
            self.runtime_dir().join(format!("superbackup{suffix}.sock")).display().to_string()
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
        let meta = std::fs::metadata(path)
            .ctx(format!("reading permissions of {}", path.display()))?;
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
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;

    let dir = path
        .parent()
        .ok_or_else(|| Error::Path { path: path.into(), reason: "has no parent".into() })?;
    std::fs::create_dir_all(dir).ctx(format!("creating {}", dir.display()))?;

    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));

    {
        let mut f = std::fs::File::create(&tmp)
            .ctx(format!("creating temporary file {}", tmp.display()))?;
        f.write_all(contents).ctx("writing temporary file")?;
        f.flush().ctx("flushing temporary file")?;
        f.sync_all().ctx("syncing temporary file")?;
    }
    harden_file(&tmp)?;

    // Windows `rename` fails if the destination exists, so replace explicitly.
    #[cfg(windows)]
    {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }

    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::io(format!("replacing {}", path.display()), e)
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

#[cfg(test)]
mod tests {
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
