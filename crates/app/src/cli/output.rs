//! Two audiences, one implementation.
//!
//! Everything a command wants to say goes through [`Ui`]. That is not
//! ceremony: it is the mechanism that makes `--json` trustworthy. In JSON mode
//! every human-facing method here is a no-op, so a command *cannot* leak a
//! sentence of English into a document a program is parsing, even by accident.
//! The single JSON document is written once, by [`Ui::finish`], after the
//! command has returned.
//!
//! Diagnostics go to stderr. `--quiet` silences everything except errors;
//! errors are never silenced, because a script that asked for quiet asked for
//! less noise, not for failures to disappear.

use std::io::{IsTerminal, Write};

use serde::Serialize;
use superbackup_core::error::{Error, ErrorCode};

use super::args::{exit, ColorChoice, GlobalArgs};
use super::format::{Colour, Style, Table};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A failure on its way to the user, carrying everything the envelope needs.
///
/// `code` is deliberately a [`ErrorCode`] from the core rather than a
/// CLI-private enum: the schema publishes that closed set and tells callers to
/// branch on it, so inventing a second vocabulary at this layer would make the
/// published contract a half-truth.
#[derive(Debug, Clone)]
pub struct CliError {
    pub code: ErrorCode,
    pub message: String,
    pub hint: Option<String>,
}

impl CliError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> CliError {
        CliError { code, message: message.into(), hint: None }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> CliError {
        self.hint = Some(hint.into());
        self
    }

    /// Bad usage: an unknown job, a malformed argument, contradictory flags.
    pub fn usage(message: impl Into<String>) -> CliError {
        CliError::new(ErrorCode::Validation, message)
    }

    /// Something the running daemon's protocol cannot express.
    ///
    /// Reported honestly rather than approximated. A backup tool that quietly
    /// ignores `--destination` and writes everywhere is worse than one that
    /// refuses.
    pub fn unsupported(what: &str, why: &str) -> CliError {
        CliError::new(ErrorCode::Validation, format!("{what} is not available: {why}"))
    }

    /// A reply that does not match the command that was sent. A protocol
    /// violation by the daemon, reported as an error rather than a panic.
    pub fn protocol(message: impl Into<String>) -> CliError {
        CliError::new(ErrorCode::Ipc, message)
            .with_hint("The daemon may be a different version. Run `superbackup version`.")
    }

    /// The stable exit code for this failure.
    ///
    /// Anything above 2 says something specific about *why*, so a script can
    /// tell "your backup failed" from "I could not reach the daemon".
    pub fn exit_code(&self) -> i32 {
        match self.code {
            ErrorCode::DaemonUnreachable => exit::DAEMON_UNREACHABLE,
            ErrorCode::Locked => exit::LOCKED,
            ErrorCode::JobCancelled => exit::CANCELLED,
            // Usage: the command could not be understood or named something
            // that does not exist. Fixing it means changing the invocation.
            ErrorCode::Validation
            | ErrorCode::JobNotFound
            | ErrorCode::Schedule
            | ErrorCode::Config => exit::USAGE,
            // Everything else ran and failed.
            _ => exit::FAILED,
        }
    }
}

impl From<Error> for CliError {
    fn from(e: Error) -> CliError {
        let code = e.code();
        CliError {
            code,
            message: e.to_string(),
            hint: e.hint().map(|h| h.to_string()).or_else(|| default_hint(code)),
        }
    }
}

/// A way forward for the codes the core does not hint about, where the user
/// would otherwise be left with a sentence and no next move.
fn default_hint(code: ErrorCode) -> Option<String> {
    let hint = match code {
        ErrorCode::Kopia | ErrorCode::Internal => {
            "Run `superbackup doctor`; it checks the things that cause this."
        }
        ErrorCode::RepoNotConnected => "Connect to it with `superbackup destination connect NAME`.",
        ErrorCode::RepoExists => {
            "Add the destination with --connect-existing to use the repository that is there."
        }
        ErrorCode::JobRunning => {
            "Watch it with `superbackup status`, or stop it with `superbackup stop`."
        }
        _ => return None,
    };
    Some(hint.to_string())
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type CliResult<T> = std::result::Result<T, CliError>;

// ---------------------------------------------------------------------------
// Outcome
// ---------------------------------------------------------------------------

/// What a command produced.
///
/// The `value` is the `data` member of the JSON envelope and is built from
/// `core` types serialised directly, so the CLI's contract cannot drift away
/// from the daemon's.
#[derive(Debug)]
pub struct Outcome {
    pub value: Option<serde_json::Value>,
    /// Usually `exit::OK`. A command that *ran* and found the answer to be
    /// negative — a failed backup, a doctor check that did not pass — reports
    /// `exit::FAILED` while still emitting `"ok":true`, because the command
    /// itself did what was asked.
    pub exit: i32,
    /// The command already wrote its own stream to stdout, so no envelope
    /// follows. `watch` is a stream of NDJSON objects, and appending a
    /// document to it would give `| jq` one line it could not parse.
    pub streamed: bool,
}

impl Outcome {
    pub fn ok() -> Outcome {
        Outcome { value: None, exit: exit::OK, streamed: false }
    }

    pub fn data(value: impl Serialize) -> CliResult<Outcome> {
        Ok(Outcome { value: Some(to_value(value)?), exit: exit::OK, streamed: false })
    }

    pub fn negative(value: impl Serialize) -> CliResult<Outcome> {
        Ok(Outcome { value: Some(to_value(value)?), exit: exit::FAILED, streamed: false })
    }

    /// The command already wrote its own stream to stdout (`watch`). No
    /// envelope follows.
    pub fn streamed() -> Outcome {
        Outcome { value: None, exit: exit::OK, streamed: true }
    }
}

fn to_value(value: impl Serialize) -> CliResult<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|e| CliError::new(ErrorCode::Internal, format!("could not render JSON: {e}")))
}

// ---------------------------------------------------------------------------
// The UI
// ---------------------------------------------------------------------------

/// Where output goes. Boxed so tests can capture both streams.
pub struct Ui {
    out: Box<dyn Write + Send>,
    err: Box<dyn Write + Send>,
    pub json: bool,
    pub quiet: bool,
    /// True when stdout is a terminal: the gate on animation, on colour, and
    /// on truncating to the terminal width.
    pub out_is_tty: bool,
    /// True when stdin is a terminal, i.e. a prompt would reach a human.
    pub stdin_is_tty: bool,
    pub out_style: Style,
    pub err_style: Style,
    /// Usable columns. Very large on a pipe, so nothing is truncated there.
    pub width: usize,
    /// Set once the envelope has been written, so it cannot be written twice.
    finished: bool,
    /// Columns the last transient line occupied, so the next one can blank it.
    transient_width: usize,
}

/// What a pipe gets: effectively unlimited, so redirecting to a file never
/// silently shortens a path.
const PIPE_WIDTH: usize = 10_000;
const DEFAULT_TTY_WIDTH: usize = 100;

impl Ui {
    /// The real thing: stdout, stderr, and the environment's opinion about
    /// colour.
    pub fn from_env(global: &GlobalArgs) -> Ui {
        let out_tty = std::io::stdout().is_terminal();
        let err_tty = std::io::stderr().is_terminal();
        let stdin_tty = std::io::stdin().is_terminal();
        Ui {
            out: Box::new(std::io::stdout()),
            err: Box::new(std::io::stderr()),
            json: global.json,
            quiet: global.quiet,
            out_is_tty: out_tty,
            stdin_is_tty: stdin_tty,
            out_style: Style { colour: !global.json && colour_allowed(global.color, out_tty) },
            err_style: Style { colour: colour_allowed(global.color, err_tty) },
            width: if out_tty { terminal_width() } else { PIPE_WIDTH },
            finished: false,
            transient_width: 0,
        }
    }

    /// A UI writing into buffers, for tests.
    #[cfg(test)]
    pub fn capturing(json: bool) -> (Ui, super::testing::Captured) {
        let captured = super::testing::Captured::new();
        let ui = Ui {
            out: Box::new(captured.out()),
            err: Box::new(captured.err()),
            json,
            quiet: false,
            out_is_tty: false,
            stdin_is_tty: false,
            out_style: Style { colour: false },
            err_style: Style { colour: false },
            width: 100,
            finished: false,
            transient_width: 0,
        };
        (ui, captured)
    }

    // -- human output -----------------------------------------------------
    //
    // Every one of these is a no-op under `--json`. That is the whole
    // guarantee: prose cannot reach a machine-readable stream by accident.

    /// One line on stdout.
    pub fn line(&mut self, text: impl AsRef<str>) {
        if self.json || self.quiet {
            return;
        }
        let _ = writeln!(self.out, "{}", text.as_ref());
    }

    /// A blank line, collapsed away in quiet and JSON modes.
    pub fn blank(&mut self) {
        self.line("");
    }

    /// A heading, dim rather than loud.
    pub fn heading(&mut self, text: &str) {
        if self.json || self.quiet {
            return;
        }
        let painted = self.out_style.paint(Colour::Bold, text);
        let _ = writeln!(self.out, "{painted}");
    }

    pub fn coloured(&mut self, colour: Colour, text: &str) {
        if self.json || self.quiet {
            return;
        }
        let painted = self.out_style.paint(colour, text);
        let _ = writeln!(self.out, "{painted}");
    }

    /// A key/value pair, aligned to `pad`.
    pub fn field(&mut self, key: &str, value: impl AsRef<str>, pad: usize) {
        if self.json || self.quiet {
            return;
        }
        let _ = writeln!(self.out, "  {key:<pad$}  {}", value.as_ref());
    }

    pub fn table(&mut self, table: &Table) {
        if self.json || self.quiet {
            return;
        }
        for line in table.render(self.width, self.out_style) {
            let _ = writeln!(self.out, "{line}");
        }
    }

    /// A note on stderr. Informational, so `--quiet` suppresses it, and it
    /// never touches stdout — a note on stdout is how `| jq` breaks.
    pub fn note(&mut self, text: impl AsRef<str>) {
        if self.quiet {
            return;
        }
        let _ = writeln!(self.err, "{}", text.as_ref());
    }

    /// Something the CLI is about to do on the user's behalf that they did not
    /// literally ask for — starting a background instance, for one.
    ///
    /// Survives `--quiet` on purpose. "Quiet" asks for less chatter, not for
    /// a process to be launched in silence.
    pub fn announce(&mut self, text: impl AsRef<str>) {
        let _ = writeln!(self.err, "{}", text.as_ref());
    }

    /// A warning on stderr. Survives `--quiet`, because a warning the user
    /// asked not to see is still a warning they need.
    pub fn warn(&mut self, text: impl AsRef<str>) {
        let painted = self.err_style.paint(Colour::Yellow, "warning:");
        let _ = writeln!(self.err, "{painted} {}", text.as_ref());
    }

    /// Raw bytes on stdout, for the NDJSON stream. Flushed per line so that
    /// `| jq` shows events as they happen rather than when the pipe buffer
    /// fills.
    pub fn stream_line(&mut self, text: &str) {
        let _ = writeln!(self.out, "{text}");
        let _ = self.out.flush();
    }

    /// An in-place status line, only on a terminal. On a pipe or in a CI log
    /// this is silent: a carriage return in a log file produces one unreadable
    /// smear of overwritten text.
    pub fn transient(&mut self, text: &str) {
        if self.json || self.quiet || !self.out_is_tty {
            return;
        }
        let width = self.width.saturating_sub(1);
        let trimmed = super::format::truncate(text, width);
        // A carriage return and trailing blanks, not `ESC [ K`: this has to
        // work on a terminal that understands no escape sequences at all, and
        // padding out to the last drawn width is what erases the old line.
        let drawn = super::format::width_of(&trimmed);
        let pad = self.transient_width.saturating_sub(drawn);
        self.transient_width = drawn;
        let _ = write!(
            self.out,
            "
{trimmed}{}",
            " ".repeat(pad)
        );
        let _ = self.out.flush();
    }

    /// Erase whatever [`Ui::transient`] last drew.
    pub fn clear_transient(&mut self) {
        if self.json || self.quiet || !self.out_is_tty || self.transient_width == 0 {
            return;
        }
        let _ = write!(
            self.out,
            "
{}
",
            " ".repeat(self.transient_width)
        );
        self.transient_width = 0;
        let _ = self.out.flush();
    }

    /// Progress lines for a pipe or a CI log: whole lines, no animation, and
    /// only when something meaningful changed.
    pub fn progress_line(&mut self, text: &str) {
        if self.json || self.quiet {
            return;
        }
        if self.out_is_tty {
            self.transient(text);
        } else {
            let _ = writeln!(self.out, "{text}");
            let _ = self.out.flush();
        }
    }

    // -- the envelope -----------------------------------------------------

    /// Write the single JSON document, if this is a JSON run.
    pub fn finish(&mut self, outcome: &Outcome) {
        if !self.json || self.finished || outcome.streamed {
            return;
        }
        self.finished = true;
        let body = serde_json::json!({
            "ok": true,
            "data": outcome.value.clone().unwrap_or(serde_json::Value::Null),
        });
        self.write_json(&body);
    }

    /// Report a failure: the envelope in JSON mode, a two-clause message on
    /// stderr otherwise.
    pub fn fail(&mut self, error: &CliError) {
        if self.json {
            if self.finished {
                return;
            }
            self.finished = true;
            let mut err = serde_json::Map::new();
            err.insert("code".into(), serde_json::to_value(error.code).unwrap_or_default());
            err.insert("message".into(), serde_json::Value::String(error.message.clone()));
            err.insert(
                "hint".into(),
                match &error.hint {
                    Some(h) => serde_json::Value::String(h.clone()),
                    None => serde_json::Value::Null,
                },
            );
            let body = serde_json::json!({ "ok": false, "error": err });
            self.write_json(&body);
            return;
        }

        let label = self.err_style.paint(Colour::Red, "superbackup:");
        let _ = writeln!(self.err, "{label} {}", error.message);
        if let Some(hint) = &error.hint {
            let painted = self.err_style.paint(Colour::Dim, hint);
            let _ = writeln!(self.err, "  {painted}");
        }
    }

    fn write_json(&mut self, value: &serde_json::Value) {
        match serde_json::to_string_pretty(value) {
            Ok(text) => {
                let _ = writeln!(self.out, "{text}");
            }
            // Serialising a Value that was already built cannot realistically
            // fail, but printing a panic instead of a document would be the
            // worst possible response to it.
            Err(e) => {
                let _ = writeln!(
                    self.out,
                    "{{\"ok\":false,\"error\":{{\"code\":\"internal\",\"message\":\"the response could not be serialised\",\"hint\":null}}}}"
                );
                let _ = writeln!(self.err, "superbackup: {e}");
            }
        }
        let _ = self.out.flush();
    }

    pub fn flush(&mut self) {
        let _ = self.out.flush();
        let _ = self.err.flush();
    }
}

/// `--color`, `NO_COLOR`, `TERM=dumb` and "is this even a terminal", in the
/// order that respects the user most.
fn colour_allowed(choice: ColorChoice, is_tty: bool) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            if std::env::var_os("NO_COLOR").is_some() {
                return false;
            }
            if std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false) {
                return false;
            }
            is_tty
        }
    }
}

/// Terminal width, from `COLUMNS` where the shell exports it.
///
/// There is no `ioctl`/`GetConsoleScreenBufferInfo` available without a
/// dependency this crate does not have, so an unset `COLUMNS` falls back to a
/// conservative 100. Being narrower than the terminal wastes space; being
/// wider wraps every row and destroys the alignment, so the fallback errs
/// narrow.
fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.trim().parse::<usize>().ok())
        .filter(|w| *w >= 20)
        .unwrap_or(DEFAULT_TTY_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_distinguish_the_failure_classes() {
        assert_eq!(
            CliError::new(ErrorCode::DaemonUnreachable, "x").exit_code(),
            exit::DAEMON_UNREACHABLE
        );
        assert_eq!(CliError::new(ErrorCode::Locked, "x").exit_code(), exit::LOCKED);
        assert_eq!(CliError::new(ErrorCode::JobCancelled, "x").exit_code(), exit::CANCELLED);
        assert_eq!(CliError::usage("x").exit_code(), exit::USAGE);
        assert_eq!(CliError::new(ErrorCode::JobNotFound, "x").exit_code(), exit::USAGE);
        assert_eq!(CliError::new(ErrorCode::Kopia, "x").exit_code(), exit::FAILED);
        assert_eq!(CliError::new(ErrorCode::BadPassphrase, "x").exit_code(), exit::FAILED);
    }

    #[test]
    fn a_core_error_keeps_its_code_and_its_hint() {
        let cli: CliError = Error::Locked.into();
        assert_eq!(cli.code, ErrorCode::Locked);
        assert!(cli.hint.is_some(), "the actionable hint must survive");
        assert_eq!(cli.exit_code(), exit::LOCKED);
    }

    #[test]
    fn human_output_is_silent_in_json_mode() {
        let (mut ui, captured) = Ui::capturing(true);
        ui.line("this is prose");
        ui.heading("So is this");
        ui.field("key", "value", 10);
        ui.finish(&Outcome { value: Some(serde_json::json!({"n": 1})), exit: 0, streamed: false });
        let out = captured.stdout();
        assert!(!out.contains("prose"), "prose leaked into --json: {out}");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("one JSON document");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["data"]["n"], 1);
    }

    #[test]
    fn a_streamed_command_gets_no_trailing_envelope() {
        // `watch` writes NDJSON. One more document after it would hand `| jq`
        // a line it cannot parse, right at the end of the stream.
        let (mut ui, captured) = Ui::capturing(true);
        ui.stream_line("{\"kind\":\"event\"}");
        ui.finish(&Outcome::streamed());
        assert_eq!(
            captured.stdout(),
            "{\"kind\":\"event\"}
"
        );
    }

    #[test]
    fn the_error_envelope_has_exactly_the_published_keys() {
        let (mut ui, captured) = Ui::capturing(true);
        ui.fail(&CliError::new(ErrorCode::Locked, "the vault is locked").with_hint("unlock it"));
        let parsed: serde_json::Value =
            serde_json::from_str(&captured.stdout()).expect("valid JSON");
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["error"]["code"], "locked");
        assert_eq!(parsed["error"]["message"], "the vault is locked");
        assert_eq!(parsed["error"]["hint"], "unlock it");
        let keys: Vec<&String> =
            parsed["error"].as_object().map(|o| o.keys().collect()).unwrap_or_default();
        assert_eq!(keys.len(), 3, "the envelope must not grow keys: {keys:?}");
    }

    #[test]
    fn diagnostics_never_touch_stdout() {
        let (mut ui, captured) = Ui::capturing(true);
        ui.note("connecting");
        ui.warn("something is odd");
        assert_eq!(captured.stdout(), "");
        assert!(captured.stderr().contains("something is odd"));
    }

    #[test]
    fn quiet_silences_notes_but_never_errors() {
        let (mut ui, captured) = Ui::capturing(false);
        ui.quiet = true;
        ui.line("chatter");
        ui.note("chatter");
        ui.fail(&CliError::usage("this must still be visible"));
        assert_eq!(captured.stdout(), "");
        assert!(captured.stderr().contains("this must still be visible"));
        assert!(!captured.stderr().contains("chatter"));
    }

    #[test]
    fn no_color_beats_auto_detection() {
        assert!(!colour_allowed(ColorChoice::Never, true));
        assert!(colour_allowed(ColorChoice::Always, false));
        assert!(!colour_allowed(ColorChoice::Auto, false), "a pipe gets no escapes");
    }
}
