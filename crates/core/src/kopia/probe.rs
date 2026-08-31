//! Running kopia and handing back the *raw* result, so a user can check it.
//!
//! Every other method in this module tree parses kopia's output and returns a
//! typed value; the parse is what makes the rest of the program reliable. This
//! file exists for the one case where parsing is exactly the wrong thing:
//! when a person is trying to satisfy themselves that superbackup and kopia
//! are working, and a summarised verdict is precisely what they do not trust.
//!
//! So a [`RawInvocation`] carries what a human would see if they ran the
//! command in their own terminal — the command line, the exit code, stdout and
//! stderr — and nothing this program concluded from it.
//!
//! # Why showing the command line is safe
//!
//! Because of the invariant `command.rs` enforces: **secrets never enter
//! `argv`**. Repository passphrases and object-store keys travel in the
//! child's environment, [`KopiaCommand::arg`](super::KopiaCommand::arg) does
//! not accept a [`Secret`](crate::secret::Secret), and
//! [`KopiaCommand::audit_argv`](super::KopiaCommand::audit_argv) refuses the
//! spawn if one ever did. The names of the secret-carrying variables are
//! reported alongside — never their values — because "which credential did it
//! use?" is a fair question and "what is it?" is not.
//!
//! Output is still scrubbed by [`crate::redact::scrub`] on its way out. That
//! is not redundancy for its own sake: kopia's stderr is third-party text, and
//! the whole point of this file is to put it in front of a person.

use std::path::Path;
use std::time::Instant;

use super::binary::KopiaSource;
use super::command::{KopiaCommand, RunContext};
use super::driver::KopiaDriver;
use crate::model::Settings;
use crate::paths::Paths;

/// How much of one stream to keep. Enough for any `repository status` or a
/// stack of kopia warnings, small enough that an unexpectedly chatty build
/// cannot turn a diagnostic into a multi-megabyte IPC frame.
const MAX_CAPTURE_BYTES: usize = 16 * 1024;

/// One kopia invocation, reported exactly as it happened.
///
/// Deliberately *not* a `Result`: a non-zero exit is an answer to the question
/// "does this work?", not a failure to ask it. The caller shows the whole
/// record either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawInvocation {
    /// A short label for the command, e.g. `repository status`.
    pub label: String,
    /// The full command line, quoted so it can be pasted into a shell. Carries
    /// no secret; see the module documentation.
    pub command_line: String,
    /// Names — never values — of the environment variables that carried
    /// secrets to the child.
    pub secret_env: Vec<String>,
    /// The process exit code. `None` when kopia never ran, or was killed.
    pub exit_code: Option<i32>,
    /// Scrubbed stdout, truncated at [`MAX_CAPTURE_BYTES`].
    pub stdout: String,
    /// Scrubbed stderr, truncated at [`MAX_CAPTURE_BYTES`].
    pub stderr: String,
    pub duration_ms: u64,
    /// True when kopia ran and exited zero.
    pub ok: bool,
}

impl RawInvocation {
    /// A record for a command that could not be attempted at all — no binary,
    /// a locked vault, a destination with no repository.
    pub fn not_attempted(label: impl Into<String>, reason: impl Into<String>) -> RawInvocation {
        RawInvocation {
            label: label.into(),
            command_line: String::new(),
            secret_env: Vec::new(),
            exit_code: None,
            stdout: String::new(),
            stderr: reason.into(),
            duration_ms: 0,
            ok: false,
        }
    }
}

/// Truncate on a character boundary, marking the cut so nobody reads a partial
/// document as a complete one.
fn cap(text: &str) -> String {
    let scrubbed = crate::redact::scrub(text.trim_end());
    if scrubbed.len() <= MAX_CAPTURE_BYTES {
        return scrubbed.into_owned();
    }
    let mut end = MAX_CAPTURE_BYTES;
    while end > 0 && !scrubbed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… truncated at {MAX_CAPTURE_BYTES} bytes", &scrubbed[..end])
}

/// Run one prepared command and record everything about it.
///
/// Private to this module: building the command is what pins it to a
/// destination's config file and credentials, and a caller that could hand in
/// an arbitrary command would be a way to make the daemon — possibly running
/// as SYSTEM — execute one.
async fn record(label: &str, cmd: KopiaCommand, ctx: &RunContext) -> RawInvocation {
    let command_line = cmd.display_command_line();
    let secret_env: Vec<String> = cmd.secret_env_names().into_iter().map(String::from).collect();
    let started = Instant::now();
    let outcome = cmd.run(ctx).await;
    let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

    match outcome {
        Ok(out) => RawInvocation {
            label: label.to_string(),
            command_line,
            secret_env,
            exit_code: Some(out.status),
            stdout: cap(&out.stdout),
            stderr: cap(&out.stderr_tail),
            duration_ms,
            ok: out.succeeded(),
        },
        Err(e) => RawInvocation {
            label: label.to_string(),
            command_line,
            secret_env,
            exit_code: e.status,
            stdout: String::new(),
            // `KopiaError::detail` is the redacted stderr tail; `message` is
            // this program's classification of it. Both are shown, because the
            // classification is a claim the user is entitled to check against
            // the evidence.
            stderr: cap(&match &e.detail {
                Some(d) => format!("{}\n{d}", e.message),
                None => e.message.clone(),
            }),
            duration_ms,
            ok: false,
        },
    }
}

/// `kopia --version`, run for real against a specific file.
///
/// [`KopiaBinary`](super::KopiaBinary) already probed the version during
/// discovery and cached the banner, but a cached banner is not evidence: it
/// says what happened at startup, possibly days ago, possibly before an
/// antivirus quarantined the file. This runs it now.
pub async fn version_invocation(binary_path: &Path, ctx: &RunContext) -> RawInvocation {
    let mut cmd = KopiaCommand::new(binary_path);
    cmd.global_switch("version");
    record("--version", cmd, ctx).await
}

impl KopiaDriver {
    /// `repository status --json` against this destination, unparsed.
    ///
    /// The companion to
    /// [`KopiaDriver::repository_status`](super::KopiaDriver::repository_status):
    /// same command, same configuration, same credentials — but the caller
    /// gets the bytes rather than this program's reading of them.
    pub async fn status_invocation(&self, ctx: &RunContext) -> RawInvocation {
        let mut cmd = self.base();
        cmd.command("repository").command("status").switch("json");
        record("repository status", cmd, ctx).await
    }
}

/// One route [`KopiaBinary::discover`](super::KopiaBinary::discover) considers,
/// described for a person rather than acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDescription {
    /// `None` for the "nothing was found" row.
    pub source: Option<KopiaSource>,
    pub path: Option<String>,
    /// What this route offered: a file, nothing, or a reason it was passed
    /// over.
    pub outcome: String,
}

/// Describe every discovery route, in the order `discover` tries them.
///
/// Deliberately *re-derived* from the settings rather than reported by
/// `discover` itself, which returns only its answer. The small duplication
/// buys a guarantee worth more than it costs: this function is a description
/// for a human, and a bug in it cannot change which binary actually runs.
///
/// It performs no process spawn — only a `PATH` lookup and two `is_file`
/// checks — so a Settings screen can call it on every refresh.
pub fn describe_routes(settings: &Settings, paths: &Paths) -> Vec<RouteDescription> {
    let mut routes = Vec::new();

    routes.push(match &settings.kopia_path {
        Some(p) => RouteDescription {
            source: Some(KopiaSource::Configured),
            path: Some(p.display().to_string()),
            outcome: if p.is_file() {
                "Set in Settings. Used verbatim, and never managed or replaced.".into()
            } else {
                "Set in Settings, but there is no file at that path.".into()
            },
        },
        None => RouteDescription {
            source: Some(KopiaSource::Configured),
            path: None,
            outcome: "No path is pinned in Settings, so discovery continues.".into(),
        },
    });

    let system = which::which("kopia").ok();
    routes.push(RouteDescription {
        source: Some(KopiaSource::SystemPath),
        path: system.as_ref().map(|p| p.display().to_string()),
        outcome: match (&system, settings.kopia.prefer_system_binary) {
            (None, _) => "No kopia on PATH.".into(),
            (Some(_), true) => "Found on PATH, and preferred over the managed build.".into(),
            (Some(_), false) => {
                "Found on PATH, but the managed build is preferred in Settings.".into()
            }
        },
    });

    let managed = paths.bundled_kopia();
    routes.push(RouteDescription {
        source: Some(KopiaSource::Bundled),
        path: Some(managed.display().to_string()),
        outcome: if managed.is_file() {
            "Downloaded and kept current by superbackup.".into()
        } else {
            "Not installed yet.".into()
        },
    });

    routes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_is_described_whether_or_not_it_can_win() {
        let dir = std::env::temp_dir().join("sb-probe-routes");
        let paths = Paths::rooted_at(&dir, false);
        let routes = describe_routes(&Settings::default(), &paths);
        assert_eq!(routes.len(), 3, "all three routes are always described");
        assert_eq!(routes[0].source, Some(KopiaSource::Configured));
        assert_eq!(routes[1].source, Some(KopiaSource::SystemPath));
        assert_eq!(routes[2].source, Some(KopiaSource::Bundled));
        // The default settings pin nothing, and the row still explains itself
        // rather than being blank.
        assert!(routes[0].path.is_none());
        assert!(!routes[0].outcome.is_empty());
    }

    #[test]
    fn a_pinned_path_that_does_not_exist_says_so_rather_than_looking_fine() {
        let settings = Settings {
            kopia_path: Some(std::path::PathBuf::from("/nowhere/kopia")),
            ..Settings::default()
        };
        let paths = Paths::rooted_at(std::env::temp_dir().join("sb-probe-routes"), false);
        let routes = describe_routes(&settings, &paths);
        assert!(routes[0].outcome.contains("no file at that path"), "{:?}", routes[0]);
    }

    #[test]
    fn a_long_capture_is_cut_on_a_character_boundary_and_says_so() {
        let long = "é".repeat(MAX_CAPTURE_BYTES);
        let capped = cap(&long);
        assert!(capped.len() < long.len());
        assert!(capped.ends_with("bytes"), "{capped}");
        // Still valid UTF-8 by construction; the assertion is that we did not
        // slice through the two-byte character.
        assert!(capped.contains('é'));
    }

    #[test]
    fn a_short_capture_is_passed_through_untouched() {
        assert_eq!(cap("  0.21.1 build: abc\n"), "  0.21.1 build: abc");
    }

    #[test]
    fn a_command_that_never_ran_is_still_a_record() {
        let record = RawInvocation::not_attempted("repository status", "no destination was chosen");
        assert!(!record.ok);
        assert_eq!(record.exit_code, None);
        assert!(record.command_line.is_empty());
        assert!(record.stderr.contains("no destination"));
    }

    #[test]
    fn the_version_command_line_is_a_bare_flag() {
        let mut cmd = KopiaCommand::new("/opt/kopia");
        cmd.global_switch("version");
        assert_eq!(cmd.display_command_line(), "/opt/kopia --version");
        assert!(cmd.secret_env_names().is_empty());
    }
}
