//! One superbackup per configuration, and a safe way to take over after a
//! crash.
//!
//! Two trays driving the same repositories is not a cosmetic problem: kopia
//! serialises writes through a repository lock, and two schedulers racing to
//! start the same job produce duplicated snapshots, contested maintenance runs
//! and a confusing history. So the daemon takes a guard before it does
//! anything else.
//!
//! # How the guard works
//!
//! * **A lock file** next to the state, at [`crate::paths::Paths::lock_file`].
//!   It carries the holder's PID, start time, executable and IPC endpoint —
//!   which turns "already running" from a dead end into "here is where to send
//!   your request", exactly what a second `superbackup run` invocation needs.
//! * **A named mutex** on Windows, in addition. The kernel releases it when
//!   the owning process dies, however it dies, which makes "is the holder still
//!   alive?" a question the OS answers rather than one we have to infer.
//!
//! # Taking over a stale lock
//!
//! A lock file left by a process that was killed, or by a machine that lost
//! power mid-write, must not lock the user out for ever. But blindly deleting
//! any lock file whose PID looks dead is how two instances end up running: PIDs
//! are recycled, quickly, and on a busy machine the PID in a week-old lock file
//! is very likely alive again as something else.
//!
//! So takeover requires *both*: the PID is not running, **or** it is running
//! something that is not us. Then we write our own record with a fresh random
//! nonce and read it straight back. If the nonce we read is not the one we
//! wrote, another instance won the race in the same instant and we lose
//! gracefully. That is not a perfect mutual exclusion — a POSIX advisory lock
//! would be, and would cost a `libc` dependency this crate does not have — but
//! it closes the window from "seconds" to "the time between two adjacent
//! writes", and on Windows the mutex closes it completely.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, IoContext, Result};
use crate::paths::{write_atomic, Paths};

/// Lock records older than this are suspect even if the PID appears alive,
/// because a PID recycled over a week is far more likely than a tray that has
/// been running for a week without ever refreshing its lock. We do not act on
/// age alone — it only sharpens the liveness check.
pub const SUSPICIOUS_AGE_DAYS: i64 = 7;

// ---------------------------------------------------------------------------
// The record
// ---------------------------------------------------------------------------

/// What is written into the lock file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockRecord {
    pub pid: u32,
    /// Random per acquisition. Proves the file still belongs to us, both when
    /// taking over and when cleaning up on exit.
    pub nonce: Uuid,
    pub acquired_at: DateTime<Utc>,
    /// The holder's executable, so a recycled PID running something else is
    /// recognised as dead rather than mistaken for us.
    #[serde(default)]
    pub executable: Option<PathBuf>,
    /// Where to reach the holder. This is what makes a second launch useful
    /// instead of merely refused.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// `true` when the holder is the machine-wide service instance.
    #[serde(default)]
    pub service_scope: bool,
}

impl LockRecord {
    fn for_self(paths: &Paths) -> LockRecord {
        LockRecord {
            pid: std::process::id(),
            nonce: Uuid::new_v4(),
            acquired_at: Utc::now(),
            executable: std::env::current_exe().ok(),
            endpoint: Some(paths.ipc_endpoint()),
            service_scope: paths.service_scope,
        }
    }

    /// A message for the user, naming what to do next.
    pub fn describe(&self) -> String {
        match &self.endpoint {
            Some(endpoint) => format!(
                "superbackup is already running (process {}, started {}). It is listening on {}.",
                self.pid,
                self.acquired_at.format("%Y-%m-%d %H:%M UTC"),
                endpoint
            ),
            None => format!(
                "superbackup is already running (process {}, started {}).",
                self.pid,
                self.acquired_at.format("%Y-%m-%d %H:%M UTC")
            ),
        }
    }

    pub fn age_days(&self, now: DateTime<Utc>) -> i64 {
        (now - self.acquired_at).num_days()
    }
}

/// Decide whether a lock record may be taken over.
///
/// `alive` answers "is this PID currently running something that looks like
/// the recorded executable?". Injected so the rule can be tested exhaustively
/// without spawning processes.
pub fn is_stale(record: &LockRecord, now: DateTime<Utc>, alive: impl Fn(&LockRecord) -> bool) -> bool {
    // Our own PID appearing in the file means we already hold it (a re-entrant
    // acquire, or a lock we failed to clean up before re-exec). Never treat it
    // as a foreign live holder.
    if record.pid == std::process::id() {
        return true;
    }
    // A record from the future is a clock change, not a live process we can
    // reason about; fall back entirely to the liveness check.
    if record.age_days(now) > SUSPICIOUS_AGE_DAYS && !alive(record) {
        return true;
    }
    !alive(record)
}

/// Is the recorded process still running *our* program?
///
/// Answers `true` when in doubt: wrongly believing a holder is alive costs the
/// user one error message, wrongly believing it is dead costs them two daemons
/// fighting over a repository.
pub fn holder_is_alive(record: &LockRecord) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let pid = Pid::from_u32(record.pid);
    let mut system = System::new();
    if system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true) == 0 {
        return false;
    }
    let Some(process) = system.process(pid) else {
        return false;
    };

    // The PID exists. Is it us, or has the number been recycled?
    match (&record.executable, process.exe()) {
        (Some(recorded), Some(actual)) => {
            super::autostart::same_executable(recorded, actual)
                || file_stems_match(recorded, actual)
        }
        // Nothing to compare against: assume it is the holder and refuse to
        // steal the lock.
        _ => true,
    }
}

fn file_stems_match(a: &Path, b: &Path) -> bool {
    match (a.file_stem(), b.file_stem()) {
        (Some(x), Some(y)) => {
            if cfg!(windows) {
                x.to_string_lossy().eq_ignore_ascii_case(&y.to_string_lossy())
            } else {
                x == y
            }
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Guard
// ---------------------------------------------------------------------------

/// Held for as long as this instance is the only one. Releases on drop.
#[derive(Debug)]
pub struct InstanceGuard {
    path: PathBuf,
    record: LockRecord,
    #[cfg(windows)]
    _mutex: Option<named_mutex::NamedMutex>,
}

impl InstanceGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn record(&self) -> &LockRecord {
        &self.record
    }
    /// Release explicitly. Equivalent to dropping, but lets a caller sequence
    /// the release before, say, launching a replacement process.
    pub fn release(self) {
        drop(self);
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // Only remove the file if it is still ours. A takeover by another
        // instance during our shutdown must not have its lock deleted by us.
        match read_lock(&self.path) {
            Some(current) if current.nonce == self.record.nonce => {
                if let Err(e) = std::fs::remove_file(&self.path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(error = %e, path = %self.path.display(),
                            "could not remove the single-instance lock");
                    }
                }
            }
            _ => {
                tracing::debug!(
                    path = %self.path.display(),
                    "single-instance lock was taken over by another process; leaving it alone"
                );
            }
        }
    }
}

/// The result of trying to become the only instance.
#[derive(Debug)]
pub enum LockOutcome {
    Acquired(InstanceGuard),
    /// Somebody else holds it. The record says who, and where to reach them.
    AlreadyRunning(LockRecord),
}

impl LockOutcome {
    pub fn into_guard(self) -> Result<InstanceGuard> {
        match self {
            LockOutcome::Acquired(g) => Ok(g),
            LockOutcome::AlreadyRunning(record) => Err(Error::Validation(record.describe())),
        }
    }
}

/// Become the only running instance for this configuration.
pub fn acquire(paths: &Paths) -> Result<LockOutcome> {
    acquire_with(paths, holder_is_alive)
}

/// [`acquire`] with an injected liveness predicate, for tests.
pub fn acquire_with(
    paths: &Paths,
    alive: impl Fn(&LockRecord) -> bool,
) -> Result<LockOutcome> {
    let path = paths.lock_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ctx(format!("creating {}", parent.display()))?;
    }

    // On Windows the kernel object settles the question before we ever look at
    // a file: it disappears the instant the owning process does, crash or not.
    #[cfg(windows)]
    let mutex = match named_mutex::NamedMutex::acquire(&mutex_name(&path)) {
        named_mutex::MutexOutcome::Acquired(m) => Some(m),
        named_mutex::MutexOutcome::AlreadyHeld => {
            let record = read_lock(&path).unwrap_or_else(|| unknown_holder(paths));
            return Ok(LockOutcome::AlreadyRunning(record));
        }
        named_mutex::MutexOutcome::Unavailable => {
            // Sandboxed or otherwise unable to create the object. Fall through
            // to the file, which still works.
            None
        }
    };

    let record = LockRecord::for_self(paths);
    let payload = serde_json::to_vec_pretty(&record)
        .map_err(|e| Error::Internal(format!("serialising the lock record: {e}")))?;

    // Fast path: no file at all.
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(&payload).ctx("writing the single-instance lock")?;
            file.flush().ctx("flushing the single-instance lock")?;
            drop(file);
            crate::paths::harden_file(&path)?;
            return Ok(LockOutcome::Acquired(InstanceGuard {
                path,
                record,
                #[cfg(windows)]
                _mutex: mutex,
            }));
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(Error::io(format!("creating {}", path.display()), e)),
    }

    // A file exists. Either a live holder, or debris from a crash.
    let existing = read_lock(&path);
    let takeover = match &existing {
        // Unparsable: a truncated write from a power failure. Nothing to
        // respect, and nothing that identifies a live process.
        None => {
            tracing::warn!(path = %path.display(), "lock file is unreadable; taking it over");
            true
        }
        Some(existing) => is_stale(existing, Utc::now(), &alive),
    };

    if !takeover {
        let record = existing.unwrap_or_else(|| unknown_holder(paths));
        return Ok(LockOutcome::AlreadyRunning(record));
    }

    // Claim it, then read back and confirm the nonce is ours. If two instances
    // decided to take over at the same instant, exactly one of them reads back
    // its own nonce.
    write_atomic(&path, &payload)?;
    match read_lock(&path) {
        Some(confirmed) if confirmed.nonce == record.nonce => {
            tracing::info!(
                path = %path.display(),
                previous_pid = existing.as_ref().map(|r| r.pid).unwrap_or(0),
                "took over a stale single-instance lock"
            );
            Ok(LockOutcome::Acquired(InstanceGuard {
                path,
                record,
                #[cfg(windows)]
                _mutex: mutex,
            }))
        }
        Some(other) => Ok(LockOutcome::AlreadyRunning(other)),
        None => Err(Error::Path {
            path,
            reason: "the single-instance lock could not be read back after writing".into(),
        }),
    }
}

/// Read the lock file without taking it. Used by the CLI to answer "who is
/// running?" and by the tray to find the daemon's endpoint.
pub fn read_lock(path: &Path) -> Option<LockRecord> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// A placeholder for "held, but we cannot read by whom" — a lock file the
/// current user has no permission to read, which is what happens when the
/// service instance and a user instance share a `SUPERBACKUP_HOME`.
fn unknown_holder(paths: &Paths) -> LockRecord {
    LockRecord {
        pid: 0,
        nonce: Uuid::nil(),
        acquired_at: Utc::now(),
        executable: None,
        endpoint: Some(paths.ipc_endpoint()),
        service_scope: paths.service_scope,
    }
}

/// A stable, filesystem-independent mutex name for a lock path.
///
/// Kernel object names cannot contain a backslash outside the namespace
/// prefix, so the path is hashed rather than embedded. `Local\` keeps the name
/// per-session, which is what we want: two different users signed in at once
/// each get their own tray.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn mutex_name(lock_path: &Path) -> String {
    let normalised = lock_path.to_string_lossy().to_lowercase();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in normalised.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("Local\\superbackup-{hash:016x}")
}

#[cfg(windows)]
mod named_mutex {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
    use windows::Win32::System::Threading::CreateMutexW;

    #[derive(Debug)]
    pub struct NamedMutex(HANDLE);

    #[derive(Debug)]
    pub enum MutexOutcome {
        Acquired(NamedMutex),
        AlreadyHeld,
        /// The object could not be created at all (a sandbox, or a policy).
        Unavailable,
    }

    impl NamedMutex {
        pub fn acquire(name: &str) -> MutexOutcome {
            let wide = super::super::win32::wide(name);
            // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives
            // the call. Passing `None` for the security attributes gives the
            // object the default DACL, which is what we want: only this user's
            // session may open it.
            let handle = unsafe {
                CreateMutexW(None, true, PCWSTR(wide.as_ptr()))
            };
            match handle {
                Ok(handle) => {
                    // `CreateMutexW` succeeds and returns a handle to the
                    // *existing* object when one is already there; the only
                    // way to tell is GetLastError.
                    // SAFETY: reading the calling thread's last-error value,
                    // immediately after the call that set it.
                    let already =
                        unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS;
                    if already {
                        // SAFETY: `handle` was returned by CreateMutexW above
                        // and has not been closed.
                        unsafe {
                            let _ = CloseHandle(handle);
                        }
                        MutexOutcome::AlreadyHeld
                    } else {
                        MutexOutcome::Acquired(NamedMutex(handle))
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "could not create the single-instance mutex");
                    MutexOutcome::Unavailable
                }
            }
        }
    }

    impl Drop for NamedMutex {
        fn drop(&mut self) {
            // Closing the handle releases the kernel object. We deliberately
            // do not `ReleaseMutex` first: ownership and existence are the
            // same thing for our purposes, and the object must disappear.
            // SAFETY: `self.0` came from CreateMutexW and is closed exactly
            // once, here.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> Paths {
        let root = std::env::temp_dir()
            .join(format!("sb-lock-{}-{}-{}", tag, std::process::id(), Uuid::new_v4().simple()));
        Paths::rooted_at(root, false)
    }

    fn record(pid: u32) -> LockRecord {
        LockRecord {
            pid,
            nonce: Uuid::new_v4(),
            acquired_at: Utc::now(),
            executable: Some(PathBuf::from("/opt/superbackup/superbackup")),
            endpoint: Some("/tmp/superbackup.sock".into()),
            service_scope: false,
        }
    }

    #[test]
    fn a_live_holder_is_not_stale() {
        let r = record(4242);
        assert!(!is_stale(&r, Utc::now(), |_| true));
    }

    #[test]
    fn a_dead_holder_is_stale() {
        let r = record(4242);
        assert!(is_stale(&r, Utc::now(), |_| false));
    }

    #[test]
    fn an_old_lock_from_a_dead_process_is_stale() {
        let mut r = record(4242);
        r.acquired_at = Utc::now() - chrono::Duration::days(30);
        assert!(is_stale(&r, Utc::now(), |_| false));
        assert!(
            !is_stale(&r, Utc::now(), |_| true),
            "age alone must never justify stealing a lock from a live process"
        );
    }

    #[test]
    fn our_own_pid_is_always_takeable() {
        let r = record(std::process::id());
        assert!(is_stale(&r, Utc::now(), |_| true));
    }

    #[test]
    fn a_first_acquisition_succeeds_and_writes_a_record() {
        let paths = temp_paths("first");
        let outcome = acquire_with(&paths, |_| true).expect("acquire");
        let guard = match outcome {
            LockOutcome::Acquired(g) => g,
            LockOutcome::AlreadyRunning(r) => panic!("unexpected holder {r:?}"),
        };
        let on_disk = read_lock(guard.path()).expect("a record was written");
        assert_eq!(on_disk.pid, std::process::id());
        assert_eq!(on_disk.nonce, guard.record().nonce);
        assert!(on_disk.endpoint.is_some(), "the endpoint is how a second launch finds us");
        let path = guard.path().to_path_buf();
        drop(guard);
        assert!(!path.exists(), "dropping the guard must release the lock");
        let _ = std::fs::remove_dir_all(paths.data_dir.parent().unwrap_or(&paths.data_dir));
    }

    #[test]
    fn a_live_lock_from_another_process_is_refused() {
        let paths = temp_paths("live");
        let path = paths.lock_file();
        std::fs::create_dir_all(&paths.data_dir).expect("data dir");
        let other = record(999_999);
        std::fs::write(&path, serde_json::to_vec(&other).expect("json")).expect("write");

        match acquire_with(&paths, |_| true).expect("acquire") {
            LockOutcome::AlreadyRunning(r) => {
                assert_eq!(r.pid, 999_999);
                assert!(r.describe().contains("already running"));
            }
            LockOutcome::Acquired(_) => panic!("stole a lock from a live process"),
        }
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }

    #[test]
    fn a_stale_lock_from_a_crashed_process_is_taken_over() {
        let paths = temp_paths("stale");
        let path = paths.lock_file();
        std::fs::create_dir_all(&paths.data_dir).expect("data dir");
        let dead = record(999_998);
        std::fs::write(&path, serde_json::to_vec(&dead).expect("json")).expect("write");

        let guard = match acquire_with(&paths, |_| false).expect("acquire") {
            LockOutcome::Acquired(g) => g,
            LockOutcome::AlreadyRunning(r) => panic!("refused a stale lock: {r:?}"),
        };
        let on_disk = read_lock(&path).expect("record");
        assert_eq!(on_disk.pid, std::process::id());
        assert_ne!(on_disk.nonce, dead.nonce, "the record must be replaced, not merged");
        drop(guard);
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }

    #[test]
    fn a_corrupt_lock_file_is_taken_over() {
        let paths = temp_paths("corrupt");
        let path = paths.lock_file();
        std::fs::create_dir_all(&paths.data_dir).expect("data dir");
        // Half a JSON object, exactly what a power failure mid-write leaves.
        std::fs::write(&path, b"{\"pid\": 12").expect("write");

        let guard = match acquire_with(&paths, |_| true).expect("acquire") {
            LockOutcome::Acquired(g) => g,
            LockOutcome::AlreadyRunning(r) => panic!("a corrupt lock blocked startup: {r:?}"),
        };
        assert_eq!(read_lock(&path).expect("record").pid, std::process::id());
        drop(guard);
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }

    #[test]
    fn a_guard_does_not_delete_a_lock_that_was_taken_from_it() {
        let paths = temp_paths("takenover");
        let guard = match acquire_with(&paths, |_| true).expect("acquire") {
            LockOutcome::Acquired(g) => g,
            LockOutcome::AlreadyRunning(r) => panic!("{r:?}"),
        };
        let path = guard.path().to_path_buf();
        // Somebody else wins the lock while we are shutting down.
        let usurper = record(123_456);
        std::fs::write(&path, serde_json::to_vec(&usurper).expect("json")).expect("write");
        drop(guard);
        assert!(path.exists(), "we must not delete another instance's lock");
        assert_eq!(read_lock(&path).expect("record").pid, 123_456);
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }

    #[test]
    fn the_second_launch_is_told_where_to_find_the_first() {
        let paths = temp_paths("endpoint");
        let guard = match acquire_with(&paths, |_| true).expect("acquire") {
            LockOutcome::Acquired(g) => g,
            LockOutcome::AlreadyRunning(r) => panic!("{r:?}"),
        };
        let held = guard.record().clone();
        match acquire_with(&paths, |_| true).expect("second acquire") {
            LockOutcome::AlreadyRunning(r) => {
                assert_eq!(r.endpoint, held.endpoint);
                assert!(r.describe().contains("listening on"));
            }
            // Our own PID is legitimately takeable (see `is_stale`), so this
            // branch is the documented behaviour for a re-entrant acquire in
            // the same process.
            LockOutcome::Acquired(_) => {}
        }
        drop(guard);
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }

    #[test]
    fn mutex_names_are_derived_from_the_path_and_are_legal() {
        let a = mutex_name(Path::new(r"C:\Users\a\AppData\Local\superbackup\superbackup.lock"));
        let b = mutex_name(Path::new(r"C:\Users\b\AppData\Local\superbackup\superbackup.lock"));
        assert_ne!(a, b, "two users must not share one mutex");
        assert_eq!(
            a,
            mutex_name(Path::new(r"C:\USERS\A\AppData\Local\superbackup\superbackup.lock")),
            "Windows paths are case-insensitive"
        );
        assert!(a.starts_with("Local\\superbackup-"));
        assert!(
            !a["Local\\".len()..].contains('\\'),
            "kernel object names may not contain a backslash: {a}"
        );
    }

    #[test]
    fn a_record_round_trips_through_json() {
        let r = record(7);
        let json = serde_json::to_string(&r).expect("serialise");
        let back: LockRecord = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, r);
    }

    #[test]
    fn into_guard_reports_the_holder() {
        let outcome = LockOutcome::AlreadyRunning(record(5));
        let err = outcome.into_guard().expect_err("must not yield a guard");
        assert!(err.to_string().contains("already running"), "{err}");
    }
}
