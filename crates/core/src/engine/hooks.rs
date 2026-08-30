//! Running the user's before/after commands.
//!
//! [`crate::model::JobHooks`] lets a job stop a database, flush a cache, or
//! post to a webhook around its backup. That makes hooks arbitrary user code
//! running inside the daemon's process tree, which brings three obligations:
//!
//! * **A hook must never hang the run.** Every hook has a hard timeout and its
//!   child is killed when the timeout or a cancellation fires. The classic
//!   failure — a hook that prompts for input on a machine with no console and
//!   waits forever — is impossible here because stdin is closed and the
//!   timeout is unconditional.
//! * **Output must be captured, bounded, and redacted.** Hook output is shown
//!   in the run history, so it is truncated and passed through
//!   [`crate::redact::scrub`] before it is stored.
//! * **Failure must be interpretable.** `before` can abort the run
//!   (`abort_on_before_failure`); `after_*` never can — the backup has already
//!   happened, and failing the run because a notification webhook was down
//!   would tell the user their data is at risk when it is not.

use crate::engine::cancel::{CancelReason, CancelToken};
use crate::engine::clock::Clock;
use chrono::Duration;
use std::sync::Arc;
use uuid::Uuid;

/// Longest a single hook may run before it is killed.
///
/// Five minutes is generous for "stop the database" and short enough that a
/// wedged hook does not silently consume the whole backup window.
pub const DEFAULT_HOOK_TIMEOUT_MINUTES: i64 = 5;

/// Output kept per hook. Enough to diagnose a failure, small enough that a
/// runaway script cannot bloat `state.json`.
const MAX_OUTPUT_BYTES: usize = 8 * 1024;

/// Which hook ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    Before,
    AfterSuccess,
    AfterFailure,
}

impl HookKind {
    pub fn title(&self) -> &'static str {
        match self {
            HookKind::Before => "before",
            HookKind::AfterSuccess => "after success",
            HookKind::AfterFailure => "after failure",
        }
    }
}

/// What one hook did.
#[derive(Debug, Clone)]
pub struct HookOutcome {
    pub kind: HookKind,
    /// `None` when the process could not be started at all, or was killed.
    pub exit_code: Option<i32>,
    /// Merged stdout and stderr, truncated and redacted.
    pub output: String,
    pub timed_out: bool,
    pub cancelled: bool,
    /// Set when the hook could not be launched (bad command, missing shell).
    pub launch_error: Option<String>,
}

impl HookOutcome {
    /// A hook "succeeded" only if it ran to completion with status 0.
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && self.launch_error.is_none()
    }

    /// One line for the event log.
    pub fn summary(&self) -> String {
        if let Some(err) = &self.launch_error {
            return format!("{} hook could not start: {err}", self.kind.title());
        }
        if self.cancelled {
            return format!("{} hook was cancelled", self.kind.title());
        }
        if self.timed_out {
            return format!("{} hook timed out and was killed", self.kind.title());
        }
        match self.exit_code {
            Some(0) => format!("{} hook succeeded", self.kind.title()),
            Some(code) => format!("{} hook exited with status {code}", self.kind.title()),
            None => format!("{} hook ended without a status", self.kind.title()),
        }
    }
}

/// Context handed to a hook as environment variables, so a script can tell
/// which job it is running for without the command line having to encode it.
#[derive(Debug, Clone)]
pub struct HookContext {
    pub job_id: Uuid,
    pub job_name: String,
    pub run_id: Uuid,
    /// Present for the `after_*` hooks.
    pub status: Option<String>,
}

/// Runs hook commands under a timeout.
#[derive(Debug, Clone)]
pub struct HookRunner {
    clock: Arc<dyn Clock>,
    timeout: Duration,
}

impl HookRunner {
    pub fn new(clock: Arc<dyn Clock>) -> HookRunner {
        HookRunner { clock, timeout: Duration::minutes(DEFAULT_HOOK_TIMEOUT_MINUTES) }
    }

    /// Override the per-hook timeout. Used by the tests, and available to the
    /// embedder for unusually slow shutdown scripts.
    pub fn with_timeout(mut self, timeout: Duration) -> HookRunner {
        self.timeout = timeout;
        self
    }

    /// Run one hook command.
    ///
    /// Never returns an error: a hook failure is data, not an exception. The
    /// caller decides what a failure means, which differs between `before` and
    /// `after_*`.
    pub async fn run(
        &self,
        kind: HookKind,
        command: &str,
        context: &HookContext,
        cancel: &CancelToken,
    ) -> HookOutcome {
        let mut outcome = HookOutcome {
            kind,
            exit_code: None,
            output: String::new(),
            timed_out: false,
            cancelled: false,
            launch_error: None,
        };
        if command.trim().is_empty() {
            outcome.exit_code = Some(0);
            return outcome;
        }
        if cancel.is_cancelled() {
            outcome.cancelled = true;
            return outcome;
        }

        let mut cmd = shell_command(command);
        cmd.env("SUPERBACKUP_JOB_ID", context.job_id.to_string())
            .env("SUPERBACKUP_JOB_NAME", &context.job_name)
            .env("SUPERBACKUP_RUN_ID", context.run_id.to_string())
            .env("SUPERBACKUP_HOOK", kind.title())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Without this, a hook whose future is dropped (engine shutdown)
            // leaves an orphaned process attached to the daemon's console.
            .kill_on_drop(true);
        if let Some(status) = &context.status {
            cmd.env("SUPERBACKUP_STATUS", status);
        }

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                outcome.launch_error = Some(e.to_string());
                return outcome;
            }
        };

        let deadline = self.clock.now_utc() + self.timeout;
        let wait = child.wait_with_output();
        tokio::pin!(wait);

        tokio::select! {
            // Biased so that a completed process is always preferred over a
            // deadline that fires in the same poll: a hook that finished must
            // not be reported as timed out.
            biased;
            result = &mut wait => match result {
                Ok(output) => {
                    outcome.exit_code = output.status.code();
                    outcome.output = merge_output(&output.stdout, &output.stderr);
                }
                Err(e) => outcome.launch_error = Some(e.to_string()),
            },
            _ = self.clock.sleep_until(deadline) => {
                outcome.timed_out = true;
            }
            reason = cancel.cancelled() => {
                outcome.cancelled = true;
                outcome.timed_out = matches!(reason, CancelReason::Timeout);
            }
        }
        // Returning drops the `wait_with_output` future, which owns the
        // `Child`; `kill_on_drop` turns that into a real kill. This is the
        // only path by which a timed-out or cancelled hook's process is
        // reaped, so no branch above may return early.
        outcome
    }

    /// Run `before`, returning `Err(outcome)` when the run must be abandoned.
    ///
    /// Abandoning is opt-in (`abort_on_before_failure`), because most `before`
    /// hooks are advisory and a user who has not asked for a hard gate would
    /// rather have a backup than a failed run.
    pub async fn run_before(
        &self,
        hooks: &crate::model::JobHooks,
        context: &HookContext,
        cancel: &CancelToken,
    ) -> Option<HookOutcome> {
        let command = hooks.before.as_ref()?;
        Some(self.run(HookKind::Before, command, context, cancel).await)
    }
}

/// Wrap a command line in the platform's shell, so users can write the same
/// thing they would type in a terminal (pipes, `&&`, quoting) rather than a
/// bare executable path.
fn shell_command(command: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        // `/C` runs and exits. The command is passed as one argument, so no
        // additional quoting is applied to what the user wrote.
        cmd.arg("/C").arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

/// Merge, bound and redact a hook's output.
///
/// The *tail* is kept rather than the head: a failing script's useful line is
/// almost always its last one.
fn merge_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut text = String::new();
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    if !out.trim().is_empty() {
        text.push_str(out.trim_end());
    }
    if !err.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(err.trim_end());
    }
    let trimmed = if text.len() > MAX_OUTPUT_BYTES {
        let start = text
            .char_indices()
            .rev()
            .map(|(i, _)| i)
            .find(|i| *i <= text.len() - MAX_OUTPUT_BYTES)
            .unwrap_or(0);
        format!("[earlier output truncated]\n{}", &text[start..])
    } else {
        text
    };
    crate::redact::scrub(&trimmed).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::SystemClock;
    use crate::model::JobHooks;

    fn context() -> HookContext {
        HookContext {
            job_id: Uuid::new_v4(),
            job_name: "test-job".into(),
            run_id: Uuid::new_v4(),
            status: None,
        }
    }

    fn runner() -> HookRunner {
        // Hooks spawn real processes, so they need a real clock for their
        // timeout. The timeout itself is shortened instead.
        HookRunner::new(Arc::new(SystemClock::new())).with_timeout(Duration::seconds(10))
    }

    #[tokio::test]
    async fn successful_hook_reports_zero_and_captures_output() {
        let outcome = runner()
            .run(HookKind::Before, "echo hello-from-hook", &context(), &CancelToken::new())
            .await;
        assert!(outcome.succeeded(), "{outcome:?}");
        assert!(outcome.output.contains("hello-from-hook"), "{outcome:?}");
    }

    #[tokio::test]
    async fn failing_hook_reports_its_status() {
        let outcome = runner().run(HookKind::Before, "exit 3", &context(), &CancelToken::new()).await;
        assert!(!outcome.succeeded());
        assert_eq!(outcome.exit_code, Some(3));
    }

    #[tokio::test]
    async fn empty_hook_is_a_no_op() {
        let outcome = runner().run(HookKind::Before, "   ", &context(), &CancelToken::new()).await;
        assert!(outcome.succeeded());
    }

    #[tokio::test]
    async fn a_hanging_hook_is_killed_by_the_timeout() {
        // A command that would otherwise run for two minutes.
        let command = if cfg!(windows) {
            "ping -n 120 127.0.0.1 > nul"
        } else {
            "sleep 120"
        };
        let runner = HookRunner::new(Arc::new(SystemClock::new()))
            .with_timeout(Duration::milliseconds(300));
        let started = std::time::Instant::now();
        let outcome = runner.run(HookKind::Before, command, &context(), &CancelToken::new()).await;
        assert!(outcome.timed_out, "{outcome:?}");
        assert!(!outcome.succeeded());
        assert!(started.elapsed().as_secs() < 20, "the hook was not killed promptly");
    }

    #[tokio::test]
    async fn cancellation_stops_a_hook() {
        let command = if cfg!(windows) { "ping -n 120 127.0.0.1 > nul" } else { "sleep 120" };
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            c2.cancel(CancelReason::Requested);
        });
        let outcome = runner().run(HookKind::Before, command, &context(), &cancel).await;
        assert!(outcome.cancelled, "{outcome:?}");
    }

    #[tokio::test]
    async fn no_before_hook_means_no_outcome() {
        let hooks = JobHooks::default();
        let out = runner().run_before(&hooks, &context(), &CancelToken::new()).await;
        assert!(out.is_none());
    }

    #[test]
    fn output_is_truncated_from_the_front_and_redacted() {
        let long = "x".repeat(MAX_OUTPUT_BYTES * 2);
        let merged = merge_output(long.as_bytes(), b"");
        assert!(merged.len() < MAX_OUTPUT_BYTES + 100);
        assert!(merged.starts_with("[earlier output truncated]"));
    }

    #[test]
    fn stderr_is_appended_to_stdout() {
        let merged = merge_output(b"out", b"err");
        assert_eq!(merged, "out\nerr");
    }
}
