//! Running `git`, without hanging and without a console flash.
//!
//! # Why a subprocess at all
//!
//! [`crate::remote`] deliberately does *not* shell out to git: it talks to the
//! GitHub API directly, because it carries a token of ours and a token handed
//! to git ends up in `.git/config` and in git's own error messages.
//!
//! This module is the opposite case and the reasoning inverts. These are the
//! **user's own** repositories, read with the **user's own** credentials —
//! their SSH agent, their credential helper, their `insteadOf` rewrites, their
//! corporate proxy. Reimplementing git well enough to read a working tree, and
//! then reimplementing every authentication path a developer machine actually
//! uses, would be a worse tool that agreed with `git status` most of the time.
//! We carry no secret into these commands, so the argv rule that governs the
//! kopia driver has nothing to protect here.
//!
//! # The two rules that matter
//!
//! **It must never hang.** Git will happily block forever asking for a
//! password on a terminal that does not exist, or waiting on an unreachable
//! host. In a tray application that is an application that has stopped
//! responding, which we have already shipped once. Every invocation therefore
//! gets a deadline, and every invocation gets `GIT_TERMINAL_PROMPT=0` plus the
//! credential managers' own non-interactive switches, so a repository needing
//! credentials fails in milliseconds instead of waiting for someone who is not
//! there.
//!
//! **It must never flash a window.** Same `CREATE_NO_WINDOW` as the kopia
//! driver: a scan touching forty repositories would otherwise pop forty
//! console windows across the user's screen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::process::Command;

use crate::error::{Error, Result};

/// How long any one git invocation may take.
///
/// Local commands finish in milliseconds. The long one is `ls-remote`, which
/// makes a network round trip, and where the honest answer after fifteen
/// seconds is "the remote did not answer" rather than a spinner that never
/// stops.
pub const LOCAL_TIMEOUT: Duration = Duration::from_secs(20);
pub const REMOTE_TIMEOUT: Duration = Duration::from_secs(25);

/// What one invocation produced.
#[derive(Debug, Clone)]
pub struct Output {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }

    /// The most useful line of failure text, for a message shown to a person.
    ///
    /// Git puts the actual reason on stderr and prefixes much of it with
    /// `fatal:` or `error:`; the first non-empty line is almost always the one
    /// worth repeating, and the rest is a hint about `git config` that helps
    /// nobody reading a list of forty repositories.
    pub fn failure(&self) -> String {
        self.stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("git failed without saying why")
            .to_string()
    }
}

/// The `git` executable, located once.
#[derive(Debug, Clone)]
pub struct Git {
    program: PathBuf,
}

static LOCATED: OnceLock<Option<PathBuf>> = OnceLock::new();

impl Git {
    /// Find `git`, or explain that it is not installed.
    ///
    /// Cached process-wide: an inventory of forty repositories would otherwise
    /// search `PATH` a hundred and twenty times to reach the same answer.
    pub fn locate() -> Result<Self> {
        let found = LOCATED.get_or_init(find_git).clone();
        match found {
            Some(program) => Ok(Self { program }),
            None => Err(Error::Config(
                "git is not installed, or is not on this account's PATH. Install it from \
                 https://git-scm.com and reopen superbackup."
                    .into(),
            )),
        }
    }

    /// Is git available at all? For a screen that wants to explain its absence
    /// rather than show an error over an empty list.
    pub fn available() -> bool {
        LOCATED.get_or_init(find_git).is_some()
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Run git in `dir`, and return what it said whether it succeeded or not.
    ///
    /// A non-zero exit is *not* an error here. Half of what an inventory wants
    /// to know is reported through exit codes — `merge-base --is-ancestor`
    /// answers a yes/no question with 0 and 1 — and callers that do want a
    /// failure to be an error say so with [`Self::run_ok`].
    pub async fn run(&self, dir: &Path, args: &[&str], timeout: Duration) -> Result<Output> {
        let mut cmd = Command::new(&self.program);
        cmd.current_dir(dir);
        // Config that must hold no matter what the user's own config says.
        // `-c` beats every config file, so this cannot be switched off by a
        // repository that sets its own pager or askpass helper.
        cmd.arg("-c").arg("core.pager=cat");
        cmd.arg("-c").arg("credential.interactive=false");
        cmd.args(args);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        for (key, value) in non_interactive_env() {
            cmd.env(key, value);
        }
        crate::kopia::harden_child(&mut cmd);

        let child =
            cmd.spawn().map_err(|e| Error::io(format!("running git in {}", dir.display()), e))?;

        let finished = tokio::time::timeout(timeout, child.wait_with_output()).await;
        let output = match finished {
            Ok(result) => {
                result.map_err(|e| Error::io(format!("reading git in {}", dir.display()), e))?
            }
            // The child is killed by dropping it, which `wait_with_output`
            // already owns — so the timeout leaves nothing behind.
            Err(_) => {
                return Err(Error::Config(format!(
                    "git did not answer within {} seconds in {}",
                    timeout.as_secs(),
                    dir.display()
                )))
            }
        };

        Ok(Output {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// The same, but a non-zero exit becomes an error carrying git's own words.
    pub async fn run_ok(&self, dir: &Path, args: &[&str], timeout: Duration) -> Result<Output> {
        let output = self.run(dir, args, timeout).await?;
        if !output.ok() {
            return Err(Error::Config(output.failure()));
        }
        Ok(output)
    }
}

/// Environment overrides that make git give up rather than wait for a person.
///
/// Set on top of the inherited environment rather than replacing it: unlike
/// the kopia driver, which supplies its own credentials and must not let an
/// ambient `AWS_ACCESS_KEY_ID` win, git *needs* the user's environment to
/// reach their own remotes — `SSH_AUTH_SOCK`, `GIT_SSH_COMMAND`, proxy
/// variables, and whatever their credential helper reads.
pub fn non_interactive_env() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        // Git's own prompt for a username or password on the controlling
        // terminal. Without this, an https remote with no stored credential
        // blocks until the timeout on every single scan.
        ("GIT_TERMINAL_PROMPT", "0"),
        // Git Credential Manager, which ships with Git for Windows and would
        // otherwise raise a *GUI* dialog — the one kind of prompt that
        // `GIT_TERMINAL_PROMPT` cannot suppress.
        ("GCM_INTERACTIVE", "never"),
        ("GCM_PROVIDER", "auto"),
        // OpenSSH's own prompts, for a key with a passphrase.
        ("SSH_ASKPASS_REQUIRE", "never"),
        // Host-key confirmation on a first connection is a prompt too.
        ("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new"),
        // Advice blocks are written for a human at a terminal and only make
        // the one useful line of stderr harder to find.
        ("GIT_ADVICE", "0"),
        ("LC_ALL", "C"),
    ])
}

/// Look for `git` on `PATH`, then in the places Windows installers use.
///
/// `PATH` is nearly always enough. The fallbacks matter for the service, which
/// runs as LocalSystem with a `PATH` that has never seen a per-user install.
fn find_git() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "git.exe" } else { "git" };

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    #[cfg(windows)]
    {
        for root in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            let Some(base) = std::env::var_os(root) else { continue };
            for tail in ["Git\\cmd\\git.exe", "Programs\\Git\\cmd\\git.exe"] {
                let candidate = PathBuf::from(&base).join(tail);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        for candidate in ["/usr/bin/git", "/usr/local/bin/git", "/opt/homebrew/bin/git"] {
            let candidate = PathBuf::from(candidate);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole reason this module exists rather than `Command::new("git")`.
    /// A missing variable here is a tray application that stops repainting
    /// while git waits for a password nobody is going to type.
    #[test]
    fn nothing_git_runs_can_ask_a_question() {
        let env = non_interactive_env();
        assert_eq!(env.get("GIT_TERMINAL_PROMPT"), Some(&"0"), "git's own terminal prompt");
        assert_eq!(env.get("GCM_INTERACTIVE"), Some(&"never"), "the Windows GUI credential dialog");
        assert_eq!(env.get("SSH_ASKPASS_REQUIRE"), Some(&"never"), "ssh key passphrase prompt");
        let ssh = env.get("GIT_SSH_COMMAND").copied().unwrap_or_default();
        assert!(ssh.contains("BatchMode=yes"), "ssh must not prompt: {ssh}");
        assert!(
            ssh.contains("StrictHostKeyChecking=accept-new"),
            "an unknown host key is a prompt too: {ssh}"
        );
    }

    #[test]
    fn a_missing_git_explains_itself_instead_of_failing_obscurely() {
        // Whichever this machine is, the message on the failure path has to
        // name the program and say where to get it.
        if Git::available() {
            let git = Git::locate().expect("located");
            assert!(git.program().is_file());
        } else {
            let err = Git::locate().expect_err("no git");
            let text = err.to_string();
            assert!(text.contains("git is not installed"), "{text}");
            assert!(text.contains("git-scm.com"), "{text}");
        }
    }

    #[tokio::test]
    async fn a_non_zero_exit_is_data_until_a_caller_asks_for_an_error() {
        if !Git::available() {
            return;
        }
        let git = Git::locate().expect("git");
        let dir = std::env::current_dir().expect("cwd");
        // `--is-ancestor` answers a question with its exit code, so `run` must
        // hand that back rather than turning it into an error.
        let out = git
            .run(&dir, &["merge-base", "--is-ancestor", "HEAD", "HEAD"], LOCAL_TIMEOUT)
            .await
            .expect("ran");
        assert_eq!(out.status, Some(0), "a commit is its own ancestor");

        let bad = git.run(&dir, &["rev-parse", "definitely-not-a-ref"], LOCAL_TIMEOUT).await;
        let bad = bad.expect("ran without erroring");
        assert!(!bad.ok(), "the command failed");
        assert!(!bad.failure().is_empty(), "and said why");

        // The same command through `run_ok` is an error carrying that text.
        let err = git
            .run_ok(&dir, &["rev-parse", "definitely-not-a-ref"], LOCAL_TIMEOUT)
            .await
            .expect_err("run_ok surfaces the failure");
        assert!(!err.to_string().is_empty());
    }
}
