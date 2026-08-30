//! Tracing: a rolling file in `paths.log_dir()`, plus a console layer when
//! there is a console to write to.
//!
//! ## Why the log file is opened before the configuration is read
//!
//! The most valuable log lines a backup daemon ever writes are the ones about
//! why it could not start: a corrupt vault, an unreadable `config.json`, a
//! second instance already holding the lock. Those all happen *before* the
//! configuration that would say what log level to use. So logging is
//! initialised first, at a default level, and the configured level is applied
//! afterwards through a reload handle.
//!
//! ## Rotation
//!
//! Daily files named `superbackup.log.YYYY-MM-DD`, pruned to
//! `Settings::log_retention_days`. Rolling by date rather than by size is what
//! makes retention expressible in days at all, and "keep the last fourteen
//! days" is the promise a privacy policy can actually make.
//!
//! A retention of zero means "keep everything", which is a defensible choice
//! for someone diagnosing an intermittent failure, so it disables pruning
//! rather than deleting today's file.

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{Duration, NaiveDate, Utc};
use superbackup_core::model::LogLevel;
use superbackup_core::paths::Paths;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Stem of the rolling log file.
pub const LOG_STEM: &str = "superbackup.log";

/// A handle for changing the level once the configuration has been read.
pub struct LogControl {
    reload: tracing_subscriber::reload::Handle<EnvFilter, tracing_subscriber::Registry>,
    log_dir: PathBuf,
}

impl std::fmt::Debug for LogControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogControl").field("log_dir", &self.log_dir).finish()
    }
}

impl LogControl {
    /// Apply the configured verbosity.
    ///
    /// `-v` on the command line raises it further, and never lowers it: a user
    /// who asked for more output on this run must get it even if the stored
    /// setting says `warn`.
    pub fn set_level(&self, level: LogLevel, extra_verbosity: u8) {
        let base = match (level, extra_verbosity) {
            (_, v) if v >= 3 => "trace",
            (_, 2) => "debug",
            (LogLevel::Error, 1) | (LogLevel::Warn, 1) => "info",
            (LogLevel::Info, 1) | (LogLevel::Debug, 1) | (LogLevel::Trace, 1) => "debug",
            (level, _) => level.as_filter(),
        };
        match EnvFilter::try_new(directives(base)) {
            Ok(filter) => {
                if let Err(e) = self.reload.reload(filter) {
                    tracing::warn!(error = %e, "could not change the log level");
                }
            }
            Err(e) => tracing::warn!(error = %e, "ignoring an unusable log filter"),
        }
    }

    /// Delete log files older than the retention window.
    pub fn prune(&self, retention_days: u32) {
        prune_logs(&self.log_dir, retention_days, Utc::now().date_naive());
    }
}

/// Filter directives: the app and the core at the requested level, and every
/// dependency at `warn`.
///
/// Without the second half, `debug` turns into a wall of `hyper` and `rustls`
/// frames that hides the one line about the backup.
fn directives(level: &str) -> String {
    format!("{level},superbackup={level},superbackup_core={level}")
}

/// Initialise tracing. Call once, as early as possible.
///
/// `RUST_LOG` wins when it is set: someone debugging in the field has already
/// decided what they want to see.
pub fn init(paths: &Paths, quiet: bool) -> LogControl {
    // A log directory we cannot create is not a reason to refuse to start —
    // the console layer still works, and a daemon that runs without a log file
    // is far better than one that will not run.
    if let Err(e) = std::fs::create_dir_all(&paths.log_dir) {
        eprintln!("superbackup: could not create {}: {e}", paths.log_dir.display());
    }

    let initial = std::env::var("RUST_LOG")
        .ok()
        .and_then(|value| EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| {
            EnvFilter::try_new(directives("info"))
                .unwrap_or_else(|_| EnvFilter::new("info"))
        });
    let (filter, reload) = tracing_subscriber::reload::Layer::new(initial);

    let file = RollingFile::new(paths.log_dir.clone());
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(file);

    // No console layer when quiet, and none at all on a Windows service, where
    // stderr goes nowhere and the formatter would just burn cycles.
    let console = (!quiet).then(|| {
        tracing_subscriber::fmt::layer()
            .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
            .with_target(false)
            .with_writer(std::io::stderr)
            .boxed()
    });

    let registry = tracing_subscriber::registry().with(filter).with(file_layer).with(console);
    // A second `init` in one process is a programming error, not a runtime
    // one — and in tests it is expected, so it is ignored rather than fatal.
    if registry.try_init().is_err() {
        tracing::debug!("tracing was already initialised");
    }

    LogControl { reload, log_dir: paths.log_dir.clone() }
}

/// A `MakeWriter` that appends to one file per day.
#[derive(Debug, Clone)]
struct RollingFile {
    dir: PathBuf,
}

impl RollingFile {
    fn new(dir: PathBuf) -> RollingFile {
        RollingFile { dir }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RollingFile {
    type Writer = DayFile;
    fn make_writer(&'a self) -> DayFile {
        DayFile {
            path: self.dir.join(format!("{LOG_STEM}.{}", Utc::now().format("%Y-%m-%d"))),
        }
    }
}

/// Opens on every write rather than holding a handle.
///
/// A backup daemon writes a handful of lines a minute, so the syscall cost is
/// irrelevant, and not holding the handle means log rotation, a user deleting
/// the folder, and a roaming profile all behave sensibly instead of leaving
/// the daemon writing to an unlinked inode for the rest of the week.
#[derive(Debug)]
struct DayFile {
    path: PathBuf,
}

impl Write for DayFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        file.write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Delete rotated logs older than `retention_days`.
///
/// Pure enough to test: `today` is injected, and it only ever removes files
/// whose name matches the stem *and* parses as a date. A file the user dropped
/// in the log folder is never touched.
pub fn prune_logs(dir: &Path, retention_days: u32, today: NaiveDate) -> usize {
    if retention_days == 0 {
        return 0;
    }
    let cutoff = today - Duration::days(retention_days as i64);
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(suffix) = name.strip_prefix(&format!("{LOG_STEM}.")) else { continue };
        let Ok(date) = NaiveDate::parse_from_str(suffix, "%Y-%m-%d") else { continue };
        if date < cutoff {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(e) => tracing::debug!(error = %e, file = %name, "could not prune a log file"),
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"x").expect("write");
    }

    #[test]
    fn pruning_removes_old_logs_and_leaves_everything_else_alone() {
        let dir = std::env::temp_dir().join(format!("sb-logs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        touch(&dir, "superbackup.log.2020-01-01");
        touch(&dir, "superbackup.log.2030-01-01");
        touch(&dir, "superbackup.log.not-a-date");
        touch(&dir, "important-notes.txt");

        let today = NaiveDate::from_ymd_opt(2030, 1, 2).expect("a real date");
        let removed = prune_logs(&dir, 7, today);

        assert_eq!(removed, 1);
        assert!(!dir.join("superbackup.log.2020-01-01").exists());
        assert!(dir.join("superbackup.log.2030-01-01").exists());
        assert!(dir.join("superbackup.log.not-a-date").exists(), "unparseable names are left");
        assert!(dir.join("important-notes.txt").exists(), "only our own files are pruned");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_retention_of_zero_keeps_everything() {
        let dir = std::env::temp_dir().join(format!("sb-logs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        touch(&dir, "superbackup.log.2000-01-01");
        let today = NaiveDate::from_ymd_opt(2030, 1, 2).expect("a real date");
        assert_eq!(prune_logs(&dir, 0, today), 0);
        assert!(dir.join("superbackup.log.2000-01-01").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn directives_pin_dependencies_below_the_app() {
        let d = directives("debug");
        assert!(d.contains("superbackup=debug"));
        assert!(d.contains("superbackup_core=debug"));
    }
}
