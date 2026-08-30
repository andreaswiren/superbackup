//! Building and running one kopia invocation, securely and cancellably.
//!
//! # The argv rule
//!
//! **No secret ever appears in an argument.** On Linux any local user can read
//! `/proc/<pid>/cmdline`; on Windows any process can read `CommandLine` out of
//! `Win32_Process` over WMI without elevation. Kopia offers `--password`, and
//! using it would leak the repository key to every process on the machine for
//! the lifetime of the backup. Instead:
//!
//! | Secret | Delivered as |
//! |---|---|
//! | Repository passphrase | `KOPIA_PASSWORD` |
//! | New passphrase (rotation) | `KOPIA_NEW_PASSWORD` |
//! | S3 access key | `AWS_ACCESS_KEY_ID` |
//! | S3 secret key | `AWS_SECRET_ACCESS_KEY` |
//! | S3 session token | `AWS_SESSION_TOKEN` |
//!
//! All five are bound to those exact variables by kopia itself (`cli/app.go`,
//! `cli/storage_s3.go`, `cli/command_repository_change_password.go`), and
//! kingpin treats an environment-supplied value as satisfying a `Required()`
//! flag — which is why `--access-key` can be omitted entirely.
//!
//! [`KopiaCommand::audit_argv`] enforces the rule mechanically: before every
//! spawn, every argument is scanned for every secret this command carries, and
//! a hit refuses the launch.
//!
//! It refuses rather than panicking, in debug builds too, and that is a
//! deliberate choice. A leak here would be a programming error, and panicking
//! is the usual way to make one impossible to ignore — but this code runs
//! inside an unattended backup daemon, where a panic is an aborted backup and a
//! refusal is a loud, logged, classified error the scheduler can report and
//! retry around. The failure is surfaced as
//! [`super::error::KopiaFailure::Unusable`] with an explanation naming the
//! variable, and the test suite asserts the refusal directly, so nothing is
//! lost by not aborting the process.
//!
//! # The environment rule
//!
//! The child environment is **built from empty**, not inherited. An ambient
//! `AWS_ACCESS_KEY_ID` from the user's shell, or a stray `KOPIA_PASSWORD`,
//! would otherwise silently take priority over the destination's own
//! credentials and back data up to the wrong bucket. Only an explicit
//! allowlist of process-hygiene variables is passed through.
//!
//! # The isolation rule
//!
//! Every invocation is pinned to `--config-file` under superbackup's data
//! directory and to its own cache directory. A user running `kopia` by hand
//! uses `~/.config/kopia/repository.config`; we never read, write, or race
//! with it.
//!
//! # The pipe rule
//!
//! Progress is streamed out through a bounded channel with `try_send`, never
//! `send().await`. A slow GUI must not be able to stall the task draining
//! kopia's stderr, because a full pipe buffer blocks kopia's own writes and
//! deadlocks the backup. Dropping a progress frame is free — the next one is a
//! superset.

use super::error::{KopiaError, KopiaFailure};
use super::progress::ProgressTracker;
use crate::redact;
use crate::secret::Secret;
use crate::state::Progress;
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

/// Cap on captured stdout. `snapshot list --json` over a decade of hourly
/// snapshots is large but bounded; anything past this is pathological and
/// truncating beats an out-of-memory kill of the backup daemon.
const MAX_STDOUT_BYTES: usize = 64 * 1024 * 1024;

/// How much stderr to keep for the error detail. Go error chains put the root
/// cause last, so the *tail* is what matters.
const MAX_STDERR_TAIL_LINES: usize = 200;
const MAX_STDERR_TAIL_BYTES: usize = 32 * 1024;

/// Cap on collected warnings, so a source tree with a million unreadable files
/// cannot turn the run history into a memory leak.
const MAX_WARNINGS: usize = 100;

/// Grace period for reaping a killed child before we give up waiting on it.
const REAP_TIMEOUT: Duration = Duration::from_secs(10);

/// Grace period for the stdout/stderr pumps to notice the pipes closed.
const PUMP_JOIN_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

/// The write end of a cancellation signal. Dropping it does **not** cancel.
#[derive(Debug)]
pub struct CancelHandle {
    tx: watch::Sender<bool>,
}

impl CancelHandle {
    /// Ask every command holding the matching token to stop. Idempotent.
    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }
    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }
    pub fn token(&self) -> CancelToken {
        CancelToken { rx: self.tx.subscribe() }
    }
}

/// The read end of a cancellation signal, cheap to clone and share.
#[derive(Debug, Clone)]
pub struct CancelToken {
    rx: watch::Receiver<bool>,
}

/// Create a linked cancel handle and token.
pub fn cancellation() -> (CancelHandle, CancelToken) {
    let (tx, rx) = watch::channel(false);
    (CancelHandle { tx }, CancelToken { rx })
}

impl CancelToken {
    /// A token that is never triggered, for unattended internal commands.
    pub fn never() -> CancelToken {
        let (tx, rx) = watch::channel(false);
        // Leak the sender so `changed()` never reports "closed" and the future
        // below stays pending instead of resolving as if cancelled.
        std::mem::forget(tx);
        CancelToken { rx }
    }

    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    /// Resolves once cancellation is requested, and otherwise never.
    ///
    /// A dropped [`CancelHandle`] deliberately leaves this pending forever: a
    /// sender going out of scope means "nobody will cancel", and treating it
    /// as a cancellation would abort backups whenever a caller tidied up.
    pub async fn cancelled(&self) {
        let mut rx = self.rx.clone();
        if *rx.borrow_and_update() {
            return;
        }
        while rx.changed().await.is_ok() {
            if *rx.borrow_and_update() {
                return;
            }
        }
        std::future::pending::<()>().await
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Something worth telling the engine about while kopia is still running.
#[derive(Debug, Clone)]
pub enum KopiaEvent {
    /// A fresh progress sample. Always a complete picture, never a delta, so
    /// dropping one is harmless.
    Progress(Progress),
    /// A file kopia could not read, or another non-fatal problem. Also
    /// accumulated in [`CommandOutput::warnings`].
    Warning(String),
    /// A redacted stderr line that was not progress. For the log view.
    Log(String),
}

/// Bounded, non-blocking sender for [`KopiaEvent`]s.
#[derive(Debug, Clone)]
pub struct EventSink {
    tx: mpsc::Sender<KopiaEvent>,
}

impl EventSink {
    /// Create a sink and its receiver. `capacity` bounds the memory a stalled
    /// consumer can cost; 64 is roughly 20 seconds of kopia's 300 ms progress
    /// cadence, which is far more slack than a GUI needs.
    pub fn channel(capacity: usize) -> (EventSink, mpsc::Receiver<KopiaEvent>) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (EventSink { tx }, rx)
    }

    /// Emit without ever blocking. Returns `false` when the event was dropped
    /// because the consumer is behind or gone.
    pub fn emit(&self, event: KopiaEvent) -> bool {
        self.tx.try_send(event).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Run context
// ---------------------------------------------------------------------------

/// Everything about *how* to run a command, as opposed to *what* to run.
#[derive(Debug, Clone, Default)]
pub struct RunContext {
    pub cancel: Option<CancelToken>,
    pub events: Option<EventSink>,
    /// Wall-clock budget. `None` means "as long as it takes", which is correct
    /// for a first full backup of a terabyte over a domestic uplink.
    pub timeout: Option<Duration>,
    /// Seeds [`Progress::current_path`]; kopia's progress line has no such
    /// field.
    pub current_path: Option<String>,
    /// Seeds the totals from a prior `snapshot estimate`, so the bar is
    /// meaningful before kopia's own estimation pass finishes.
    pub seed_files: Option<u64>,
    pub seed_bytes: Option<u64>,
}

impl RunContext {
    pub fn new() -> Self {
        RunContext::default()
    }
    pub fn with_cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }
    pub fn with_events(mut self, sink: EventSink) -> Self {
        self.events = Some(sink);
        self
    }
    pub fn with_timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }
    pub fn with_current_path(mut self, path: impl Into<String>) -> Self {
        self.current_path = Some(path.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// The result of one successful kopia invocation.
#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    pub status: i32,
    /// Raw stdout, **not** redacted, because it is machine-readable JSON that
    /// redaction would corrupt. It never contains credentials: kopia's JSON
    /// outputs are manifests, policies and status blocks. Use
    /// [`CommandOutput::redacted_stdout`] before it reaches a log or a user.
    pub stdout: String,
    /// Whether stdout hit the 64 MiB capture cap and was cut short.
    pub stdout_truncated: bool,
    /// Redacted tail of stderr.
    pub stderr_tail: String,
    /// Non-fatal problems kopia reported while running.
    pub warnings: Vec<String>,
    /// The last progress sample seen.
    pub progress: Progress,
    /// Progress frames dropped because the consumer was behind. Diagnostic
    /// only; a non-zero value is normal under load and is not an error.
    pub dropped_events: u64,
}

impl CommandOutput {
    pub fn succeeded(&self) -> bool {
        self.status == 0
    }
    /// stdout with credentials scrubbed, for logs and error text.
    pub fn redacted_stdout(&self) -> String {
        redact::scrub(&self.stdout).into_owned()
    }
}

// ---------------------------------------------------------------------------
// The command builder
// ---------------------------------------------------------------------------

/// One kopia invocation, under construction.
///
/// Arguments and secrets are kept apart by type: [`KopiaCommand::arg`] accepts
/// anything `OsStr`-ish, and [`Secret`] deliberately implements none of that,
/// so putting a passphrase in argv requires going out of one's way. The audit
/// in [`KopiaCommand::audit_argv`] catches the case where somebody did.
#[derive(Debug)]
pub struct KopiaCommand {
    program: PathBuf,
    /// Flags that must precede the subcommand (kingpin application flags).
    global_args: Vec<OsString>,
    /// The subcommand path, e.g. `["repository", "create", "s3"]`.
    command: Vec<String>,
    /// Everything after the subcommand.
    args: Vec<OsString>,
    env: Vec<(String, OsString)>,
    secret_env: Vec<(String, Secret)>,
}

impl KopiaCommand {
    /// Start a command for a kopia binary that has already been verified.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        KopiaCommand {
            program: program.into(),
            global_args: Vec::new(),
            command: Vec::new(),
            args: Vec::new(),
            env: Vec::new(),
            secret_env: Vec::new(),
        }
    }

    /// An application-level flag, which kingpin requires before the
    /// subcommand: `--config-file`, `--progress`, `--log-level`, `--password`.
    pub fn global(&mut self, flag: &str, value: impl AsRef<OsStr>) -> &mut Self {
        let mut arg = OsString::from(format!("--{flag}="));
        arg.push(value);
        self.global_args.push(arg);
        self
    }

    /// An application-level boolean, rendered as kingpin's `--flag=true|false`
    /// so an explicit `false` overrides kopia's own default.
    pub fn global_bool(&mut self, flag: &str, value: bool) -> &mut Self {
        self.global_args.push(OsString::from(format!("--{flag}={value}")));
        self
    }

    /// Append one word of the subcommand path.
    pub fn command(&mut self, word: impl Into<String>) -> &mut Self {
        self.command.push(word.into());
        self
    }

    /// A positional argument.
    pub fn arg(&mut self, value: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(value.as_ref().to_os_string());
        self
    }

    /// `--flag=value`. The `=` form is deliberate: a value that begins with a
    /// dash — an S3 prefix, a Windows path, a glob — would otherwise be parsed
    /// as the next flag.
    pub fn flag(&mut self, flag: &str, value: impl AsRef<OsStr>) -> &mut Self {
        let mut arg = OsString::from(format!("--{flag}="));
        arg.push(value);
        self.args.push(arg);
        self
    }

    /// `--flag=true` / `--flag=false`.
    pub fn flag_bool(&mut self, flag: &str, value: bool) -> &mut Self {
        self.args.push(OsString::from(format!("--{flag}={value}")));
        self
    }

    /// A bare `--flag` with no value.
    pub fn switch(&mut self, flag: &str) -> &mut Self {
        self.args.push(OsString::from(format!("--{flag}")));
        self
    }

    /// A repeated flag, once per value. Used for `--add-ignore` and `--tags`.
    pub fn repeated<I, S>(&mut self, flag: &str, values: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for v in values {
            self.flag(flag, v);
        }
        self
    }

    /// A non-secret environment variable.
    pub fn env(&mut self, name: impl Into<String>, value: impl AsRef<OsStr>) -> &mut Self {
        self.env.push((name.into(), value.as_ref().to_os_string()));
        self
    }

    /// A secret environment variable. This is the *only* way a secret may
    /// reach kopia.
    pub fn secret_env(&mut self, name: impl Into<String>, value: &Secret) -> &mut Self {
        self.secret_env.push((name.into(), value.clone()));
        self
    }

    /// A stable, credential-free label for logs and errors: `repository create s3`.
    pub fn label(&self) -> String {
        if self.command.is_empty() {
            "kopia".to_string()
        } else {
            self.command.join(" ")
        }
    }

    /// The full argument vector, in the order it will be passed.
    pub fn argv(&self) -> Vec<OsString> {
        let mut v =
            Vec::with_capacity(self.global_args.len() + self.command.len() + self.args.len());
        v.extend(self.global_args.iter().cloned());
        v.extend(self.command.iter().map(OsString::from));
        v.extend(self.args.iter().cloned());
        v
    }

    /// The names of the environment variables that will carry secrets. Exposed
    /// so tests and `superbackup doctor` can assert the delivery mechanism
    /// without ever touching the values.
    pub fn secret_env_names(&self) -> Vec<&str> {
        self.secret_env.iter().map(|(k, _)| k.as_str()).collect()
    }

    /// Prove that no secret this command carries appears anywhere in argv.
    ///
    /// Called automatically before every spawn. Public because it is also the
    /// unit under test: a test can assert the refusal in isolation, without
    /// having to reach it through [`KopiaCommand::run`].
    pub fn audit_argv(&self) -> std::result::Result<(), KopiaError> {
        for arg in self.argv() {
            let bytes = arg.as_encoded_bytes();
            for (name, secret) in &self.secret_env {
                // A one-byte "secret" would match almost any argument; such a
                // value is not protectable anyway and flagging it would be
                // noise. Anything of real length is checked.
                if secret.len() < 4 {
                    continue;
                }
                if contains_subslice(bytes, secret.expose()) {
                    return Err(KopiaError::local(
                        self.label(),
                        KopiaFailure::Unusable,
                        Some(format!(
                            "refusing to launch kopia: the value of {name} would have been \
                             visible in the process command line, which every user on this \
                             machine can read"
                        )),
                    )
                    .with_message(
                        "superbackup blocked a kopia command that would have exposed a \
                         credential in its command line.",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Turn this into a configured [`tokio::process::Command`].
    ///
    /// The environment is built from empty; see the module documentation for
    /// why inheriting it is not safe.
    fn to_process(&self) -> std::result::Result<Command, KopiaError> {
        self.audit_argv()?;

        let mut cmd = Command::new(&self.program);
        cmd.args(self.argv());
        cmd.env_clear();
        for name in PASSTHROUGH_ENV {
            if let Some(v) = std::env::var_os(name) {
                cmd.env(name, v);
            }
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        for (k, secret) in &self.secret_env {
            // Kopia reads every one of these as UTF-8; a non-UTF-8 passphrase
            // could not have been typed into it in the first place.
            match secret.expose_str() {
                Some(s) => {
                    cmd.env(k, s);
                }
                None => {
                    return Err(KopiaError::local(
                        self.label(),
                        KopiaFailure::Unusable,
                        Some(format!("{k} is not valid UTF-8 and cannot be passed to kopia")),
                    ));
                }
            }
        }

        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        // Backstop: if this future is dropped on a panic or an early return,
        // the child must not survive holding a repository open.
        cmd.kill_on_drop(true);
        harden_child(&mut cmd);
        Ok(cmd)
    }

    /// Spawn, stream, and wait.
    ///
    /// Returns `Ok` for a zero exit status and a classified [`KopiaError`] for
    /// anything else, including cancellation and timeout.
    pub async fn run(self, ctx: &RunContext) -> std::result::Result<CommandOutput, KopiaError> {
        let label = self.label();
        let mut process = self.to_process()?;

        // Cancelled before we even started: do not launch at all.
        if ctx.cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
            return Err(KopiaError::local(label, KopiaFailure::Cancelled, None));
        }

        let mut child = process.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                KopiaError::local(
                    label.clone(),
                    KopiaFailure::Unusable,
                    Some("the kopia executable disappeared between discovery and launch".into()),
                )
            } else {
                KopiaError::local(label.clone(), KopiaFailure::Unusable, Some(e.to_string()))
            }
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let out_task = tokio::spawn(async move {
            match stdout {
                Some(r) => read_capped(r, MAX_STDOUT_BYTES).await,
                None => (Vec::new(), false),
            }
        });

        let mut capture = StderrCapture::new(ctx);
        let err_task = tokio::spawn(async move {
            if let Some(r) = stderr {
                capture.pump(r).await;
            }
            capture
        });

        let cancel = ctx.cancel.clone().unwrap_or_else(CancelToken::never);
        let timeout = ctx.timeout;

        // `Child::wait` is cancel-safe, so losing the race here does not lose
        // the child; the borrow is released and we can then kill it.
        let stop = tokio::select! {
            res = child.wait() => Stop::Exited(res),
            _ = cancel.cancelled() => Stop::Cancelled,
            _ = sleep_opt(timeout) => Stop::TimedOut,
        };

        let mut interrupted: Option<KopiaFailure> = None;
        let status = match stop {
            Stop::Exited(res) => res,
            Stop::Cancelled => {
                interrupted = Some(KopiaFailure::Cancelled);
                terminate(&mut child, &label).await
            }
            Stop::TimedOut => {
                interrupted = Some(KopiaFailure::Timeout);
                terminate(&mut child, &label).await
            }
        };

        // Both pumps end when their pipes close, which the kill above
        // guarantees. The timeout is a last resort against a grandchild that
        // inherited a pipe; leaking a reader task is better than wedging the
        // daemon.
        let (stdout_bytes, stdout_truncated) = join_or_default(out_task, (Vec::new(), false)).await;
        let capture = join_or_default(err_task, StderrCapture::new(ctx)).await;

        let output = CommandOutput {
            status: status.as_ref().ok().and_then(|s| s.code()).unwrap_or(-1),
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stdout_truncated,
            stderr_tail: capture.tail_text(),
            warnings: capture.warnings,
            progress: capture.tracker.snapshot(),
            dropped_events: capture.dropped,
        };

        match interrupted {
            Some(KopiaFailure::Cancelled) => {
                return Err(KopiaError::local(label, KopiaFailure::Cancelled, None));
            }
            Some(KopiaFailure::Timeout) => {
                let secs = timeout.map(|d| d.as_secs()).unwrap_or(0);
                return Err(KopiaError::local(
                    label,
                    KopiaFailure::Timeout,
                    Some(format!("stopped after {secs} seconds")),
                ));
            }
            _ => {}
        }

        match status {
            Ok(s) if s.success() => Ok(output),
            Ok(s) => {
                // Some failures land on stdout (kingpin usage errors), so fall
                // back to it rather than reporting an empty reason.
                let detail = if output.stderr_tail.trim().is_empty() {
                    output.redacted_stdout()
                } else {
                    output.stderr_tail.clone()
                };
                Err(KopiaError::from_output(label, s.code(), &detail))
            }
            Err(e) => Err(KopiaError::local(
                label,
                KopiaFailure::Unknown,
                Some(format!("could not wait for kopia: {e}")),
            )),
        }
    }
}

/// Why the wait stopped.
enum Stop {
    Exited(std::io::Result<std::process::ExitStatus>),
    Cancelled,
    TimedOut,
}

/// A future that resolves after `d`, or never when `d` is `None`.
async fn sleep_opt(d: Option<Duration>) {
    match d {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}

/// Kill a child and make sure it is reaped, leaving no zombie and no kopia
/// still holding the repository open.
///
/// On Unix this is `SIGKILL` and on Windows `TerminateProcess`; neither gives
/// kopia a chance to unwind. That is safe by design: a kopia repository is
/// append-only content-addressed storage, so an interrupted upload leaves
/// unreferenced blobs — reclaimed by the next `maintenance run` — and never a
/// corrupt or half-written snapshot. Trading a graceful shutdown for a
/// guaranteed one is the right way round when the alternative is a wedged
/// process holding a lock the user cannot see.
async fn terminate(
    child: &mut tokio::process::Child,
    label: &str,
) -> std::io::Result<std::process::ExitStatus> {
    if let Err(e) = child.start_kill() {
        tracing::warn!(command = label, error = %e, "could not signal kopia to stop");
    }
    match tokio::time::timeout(REAP_TIMEOUT, child.wait()).await {
        Ok(result) => {
            if let Err(e) = &result {
                tracing::warn!(command = label, error = %e, "could not reap kopia");
            }
            result
        }
        Err(_) => {
            tracing::error!(
                command = label,
                "kopia did not exit within {}s of being killed",
                REAP_TIMEOUT.as_secs()
            );
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "kopia did not exit after being killed",
            ))
        }
    }
}

async fn join_or_default<T: Send + 'static>(handle: tokio::task::JoinHandle<T>, fallback: T) -> T {
    match tokio::time::timeout(PUMP_JOIN_TIMEOUT, handle).await {
        Ok(Ok(v)) => v,
        _ => fallback,
    }
}

/// Read a pipe to EOF, stopping at `limit` bytes.
async fn read_capped<R: tokio::io::AsyncRead + Unpin>(mut r: R, limit: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    let mut truncated = false;
    loop {
        match r.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if out.len() < limit {
                    let room = limit - out.len();
                    out.extend_from_slice(&buf[..n.min(room)]);
                    if n > room {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
                // Keep draining even past the limit: stopping would fill the
                // pipe and block kopia forever.
            }
            Err(_) => break,
        }
    }
    (out, truncated)
}

// ---------------------------------------------------------------------------
// stderr processing
// ---------------------------------------------------------------------------

/// Consumes kopia's stderr: separates progress from prose, redacts everything,
/// keeps a bounded tail for error reporting, and forwards events.
#[derive(Debug)]
struct StderrCapture {
    tracker: ProgressTracker,
    sink: Option<EventSink>,
    warnings: Vec<String>,
    tail: VecDeque<String>,
    tail_bytes: usize,
    dropped: u64,
}

impl StderrCapture {
    fn new(ctx: &RunContext) -> Self {
        let mut tracker = ProgressTracker::new();
        if let Some(p) = &ctx.current_path {
            tracker.set_current_path(p.clone());
        }
        tracker.seed_totals(ctx.seed_files, ctx.seed_bytes);
        StderrCapture {
            tracker,
            sink: ctx.events.clone(),
            warnings: Vec::new(),
            tail: VecDeque::new(),
            tail_bytes: 0,
            dropped: 0,
        }
    }

    /// Read stderr, splitting on **both** `\n` and `\r`.
    ///
    /// Kopia rewrites its progress line in place with a leading `\r` and no
    /// newline at all, so a line-oriented reader sees nothing until the
    /// process exits — and the GUI's progress bar never moves.
    async fn pump<R: tokio::io::AsyncRead + Unpin>(&mut self, mut r: R) {
        // A single unterminated "line" is bounded so that a kopia writing
        // megabytes without a separator cannot exhaust memory.
        const MAX_PENDING: usize = 64 * 1024;
        let mut pending: Vec<u8> = Vec::with_capacity(4096);
        let mut buf = [0u8; 8 * 1024];
        loop {
            let n = match r.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            pending.extend_from_slice(&buf[..n]);
            let mut start = 0;
            for i in 0..pending.len() {
                if pending[i] == b'\n' || pending[i] == b'\r' {
                    self.line(&pending[start..i]);
                    start = i + 1;
                }
            }
            pending.drain(..start);
            if pending.len() > MAX_PENDING {
                self.line(&pending);
                pending.clear();
            }
        }
        if !pending.is_empty() {
            self.line(&pending);
        }
    }

    fn line(&mut self, raw: &[u8]) {
        let text = String::from_utf8_lossy(raw);
        let text = text.trim_end();
        if text.trim().is_empty() {
            return;
        }
        if self.tracker.feed(text) {
            let snapshot = self.tracker.snapshot();
            self.emit(KopiaEvent::Progress(snapshot));
            return;
        }

        // Not progress: redact before it can reach a log, an event, or an error.
        let scrubbed = redact::scrub(text).trim().to_string();
        if scrubbed.is_empty() {
            return;
        }

        if is_warning(&scrubbed) {
            if self.warnings.len() < MAX_WARNINGS {
                self.warnings.push(scrubbed.clone());
            } else if self.warnings.len() == MAX_WARNINGS {
                self.warnings.push(
                    "Further warnings were suppressed; see the kopia log for the full list."
                        .to_string(),
                );
            }
            self.emit(KopiaEvent::Warning(scrubbed.clone()));
        } else {
            self.emit(KopiaEvent::Log(scrubbed.clone()));
        }

        self.tail_bytes += scrubbed.len() + 1;
        self.tail.push_back(scrubbed);
        while self.tail.len() > MAX_STDERR_TAIL_LINES || self.tail_bytes > MAX_STDERR_TAIL_BYTES {
            match self.tail.pop_front() {
                Some(dropped) => {
                    self.tail_bytes = self.tail_bytes.saturating_sub(dropped.len() + 1)
                }
                None => break,
            }
        }
    }

    fn emit(&mut self, event: KopiaEvent) {
        if let Some(sink) = &self.sink {
            if !sink.emit(event) {
                self.dropped += 1;
            }
        }
    }

    fn tail_text(&self) -> String {
        self.tail.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

/// Recognise kopia's non-fatal problem reports.
///
/// `cli/cli_progress.go` prints ignored errors as
/// `Ignored error when processing "<path>": <err>` and `cli/command_ls.go`
/// prints `- Error in "<path>": <err>`. Both mean "the backup continued but
/// something was skipped", which is exactly what
/// [`crate::state::DestinationRun::warnings`] is for.
fn is_warning(line: &str) -> bool {
    let l = line.trim_start_matches(['!', ' ']);
    l.starts_with("Ignored error when processing")
        || l.starts_with("- Error in ")
        || l.starts_with("Ignored ")
        || l.contains("error(s) while snapshotting")
        || l.starts_with("WARN ")
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

// ---------------------------------------------------------------------------
// Process hygiene
// ---------------------------------------------------------------------------

/// Environment variables passed through to the child.
///
/// Everything else is dropped, so nothing the user happens to have exported
/// can override a destination's own credentials or config path. Notably absent:
/// every `KOPIA_*` and `AWS_*` variable, which the driver sets explicitly.
const PASSTHROUGH_ENV: &[&str] = &[
    // Process basics.
    "PATH",
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "PATHEXT",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "TEMP",
    "TMP",
    "TMPDIR",
    // Identity and home, which kopia uses for the snapshot's user@host and for
    // resolving `~` in a user-supplied path.
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "USERNAME",
    "USER",
    "LOGNAME",
    "USERDOMAIN",
    "COMPUTERNAME",
    "HOSTNAME",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    // Corporate networks are unreachable without these.
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    // Distributions that do not put the CA bundle where Go looks by default.
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// Platform-specific spawn hardening.
///
/// On Windows a GUI or service process spawning a console application flashes a
/// console window on every backup, once per destination, forever. `CREATE_NO_WINDOW`
/// suppresses it without detaching the pipes we depend on.
pub(crate) fn harden_child(cmd: &mut Command) {
    #[cfg(windows)]
    {
        // `tokio::process::Command` re-exports this itself, so no trait import
        // is needed (and importing `std::os::windows::process::CommandExt`
        // here would be dead code).
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd_with_secret() -> KopiaCommand {
        let mut c = KopiaCommand::new("kopia");
        c.global("config-file", "/tmp/x.config")
            .command("repository")
            .command("create")
            .command("s3")
            .flag("bucket", "backups")
            .secret_env("KOPIA_PASSWORD", &Secret::from_str("correct horse battery staple"));
        c
    }

    #[test]
    fn secrets_never_reach_argv() {
        let c = cmd_with_secret();
        let joined =
            c.argv().iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        assert!(!joined.contains("correct horse"), "passphrase leaked: {joined}");
        assert!(!joined.contains("--password"), "kopia's --password must never be used");
        assert_eq!(c.secret_env_names(), vec!["KOPIA_PASSWORD"]);
        c.audit_argv().expect("clean command must pass the audit");
    }

    #[test]
    fn the_audit_catches_a_secret_placed_in_argv() {
        let mut c = KopiaCommand::new("kopia");
        c.command("repository")
            .command("connect")
            .secret_env("KOPIA_PASSWORD", &Secret::from_str("hunter2hunter2"))
            // Exactly the mistake the audit exists to stop.
            .flag("password", "hunter2hunter2");
        let err = c.audit_argv().expect_err("audit must refuse");
        assert_eq!(err.failure, KopiaFailure::Unusable);
        assert!(err.detail.as_deref().unwrap_or("").contains("KOPIA_PASSWORD"));
        // The refusal itself must not repeat the secret.
        assert!(!format!("{err:?}").contains("hunter2"));
    }

    #[test]
    fn the_audit_catches_a_secret_embedded_in_a_larger_argument() {
        let mut c = KopiaCommand::new("kopia");
        c.command("repository")
            .secret_env("AWS_SECRET_ACCESS_KEY", &Secret::from_str("s3cr3tkey0123"))
            .flag("endpoint", "https://user:s3cr3tkey0123@gateway.storjshare.io");
        assert!(c.audit_argv().is_err());
    }

    #[test]
    fn flags_use_the_equals_form_so_dashes_survive() {
        let mut c = KopiaCommand::new("kopia");
        c.command("policy").command("set").flag("add-ignore", "-weird-name");
        let argv: Vec<String> = c.argv().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(argv, vec!["policy", "set", "--add-ignore=-weird-name"]);
    }

    #[test]
    fn global_flags_precede_the_subcommand() {
        let c = cmd_with_secret();
        let argv: Vec<String> = c.argv().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(argv[0], "--config-file=/tmp/x.config");
        assert_eq!(&argv[1..4], &["repository", "create", "s3"]);
        assert_eq!(c.label(), "repository create s3");
    }

    #[test]
    fn warning_detection_matches_kopias_real_wording() {
        assert!(is_warning(" ! Ignored error when processing \"C:\\\\x\\\\y\": access is denied"));
        assert!(is_warning("- Error in \"/a/b\": permission denied"));
        assert!(!is_warning("Created snapshot with root k1 and ID k2 in 3s"));
    }

    #[tokio::test]
    async fn stderr_pump_splits_on_carriage_returns() {
        let data = concat!(
            " | 1 hashing, 10 hashed (1 MB), 0 cached (0 B), uploaded 1 MB, estimating...\r",
            " | 0 hashing, 20 hashed (2 MB), 0 cached (0 B), uploaded 2 MB, estimating...\r",
            " ! Ignored error when processing \"/x\": permission denied\n",
        );
        let (sink, mut rx) = EventSink::channel(16);
        let ctx = RunContext::new().with_events(sink);
        let mut cap = StderrCapture::new(&ctx);
        cap.pump(std::io::Cursor::new(data.as_bytes().to_vec())).await;

        assert_eq!(cap.tracker.progress().bytes_processed, 2_000_000);
        assert_eq!(cap.warnings.len(), 1);

        let mut progress_events = 0;
        let mut warnings = 0;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                KopiaEvent::Progress(_) => progress_events += 1,
                KopiaEvent::Warning(_) => warnings += 1,
                KopiaEvent::Log(_) => {}
            }
        }
        assert_eq!(progress_events, 2, "each \\r-terminated frame must be delivered");
        assert_eq!(warnings, 1);
    }

    #[tokio::test]
    async fn a_full_channel_drops_events_instead_of_blocking() {
        let (sink, _rx) = EventSink::channel(1);
        let ctx = RunContext::new().with_events(sink);
        let mut cap = StderrCapture::new(&ctx);
        let mut data = String::new();
        for i in 1..=50 {
            data.push_str(&format!(
                " | 0 hashing, {i} hashed ({i} MB), 0 cached (0 B), uploaded {i} MB, estimating...\r"
            ));
        }
        // Completes only because `emit` never awaits; a blocking send here
        // would hang the test, which is precisely the deadlock we are guarding.
        tokio::time::timeout(
            Duration::from_secs(5),
            cap.pump(std::io::Cursor::new(data.into_bytes())),
        )
        .await
        .expect("the stderr pump must never block on a full event channel");
        assert!(cap.dropped > 0, "events should have been dropped, not queued");
        assert_eq!(cap.tracker.progress().bytes_processed, 50_000_000);
    }

    #[tokio::test]
    async fn stderr_is_redacted_before_it_is_kept() {
        let ctx = RunContext::new();
        let mut cap = StderrCapture::new(&ctx);
        cap.pump(std::io::Cursor::new(
            b"kopia: error: AWS_SECRET_ACCESS_KEY=abc/def+123 was rejected\n".to_vec(),
        ))
        .await;
        let tail = cap.tail_text();
        assert!(!tail.contains("abc/def+123"), "credential survived redaction: {tail}");
        assert!(tail.contains("AWS_SECRET_ACCESS_KEY"));
    }

    #[tokio::test]
    async fn stderr_tail_is_bounded() {
        let ctx = RunContext::new();
        let mut cap = StderrCapture::new(&ctx);
        let mut data = String::new();
        for i in 0..5000 {
            data.push_str(&format!("noise line {i}\n"));
        }
        data.push_str("kopia: error: invalid repository password\n");
        cap.pump(std::io::Cursor::new(data.into_bytes())).await;
        let tail = cap.tail_text();
        assert!(tail.len() <= MAX_STDERR_TAIL_BYTES + 128, "tail grew unbounded");
        assert!(tail.contains("invalid repository password"), "root cause must survive");
    }

    #[tokio::test]
    async fn cancel_token_resolves_once_and_never_otherwise() {
        let (handle, token) = cancellation();
        assert!(!token.is_cancelled());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), token.cancelled()).await.is_err(),
            "an uncancelled token must stay pending"
        );
        handle.cancel();
        assert!(token.is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), token.cancelled())
            .await
            .expect("a cancelled token must resolve");
    }

    #[tokio::test]
    async fn a_dropped_handle_does_not_cancel() {
        let (handle, token) = cancellation();
        drop(handle);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), token.cancelled()).await.is_err(),
            "dropping the handle must not look like a cancellation"
        );
        assert!(tokio::time::timeout(Duration::from_millis(50), CancelToken::never().cancelled())
            .await
            .is_err());
    }
}
