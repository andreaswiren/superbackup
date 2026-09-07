//! Installing the GitHub CLI, when it is the thing standing in the way.
//!
//! # Why superbackup offers to do this at all
//!
//! `gh` is how a repository gets created on GitHub without superbackup ever
//! holding a token of the user's — the user signs in once with `gh auth
//! login`, and we borrow that. It is the best arrangement on offer, and it is
//! also the one that fails at the last step with "the GitHub CLI is not
//! installed", in a dialog, at the moment somebody was trying to do something
//! else. Telling them to go and find a download page is how a feature stops
//! being used.
//!
//! # Why a package manager rather than a downloader
//!
//! superbackup already downloads and verifies kopia
//! ([`crate::kopia::install`]), so the machinery exists. It is deliberately
//! *not* reused here.
//!
//! kopia is a private dependency: superbackup runs a specific version, keeps
//! it in its own directory, and nothing else on the machine cares. `gh` is the
//! user's own tool. It goes on their PATH, they will run it themselves, it
//! stores credentials under their profile, and it needs updating on somebody's
//! schedule. A second copy in a folder of ours, invisible to `gh --version` in
//! their terminal and never updated, would be worse than not having it.
//!
//! So this asks the platform's package manager, which is what GitHub's own
//! installation instructions say, and which leaves the user with a `gh` that
//! behaves like every other tool they installed.
//!
//! # What is on the command line
//!
//! A fixed package identifier and nothing else, ever. No part of this takes a
//! string from a caller, and there is deliberately no parameter through which
//! one could be passed: a function that installs software must not be one that
//! can be pointed at arbitrary software.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A package manager that can install `gh` on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Installer {
    /// Ships with Windows 11 and recent Windows 10.
    Winget,
    /// The usual answer on macOS.
    Homebrew,
    /// Debian and Ubuntu. Needs GitHub's apt repository, which is why this is
    /// reported but not run for the user — see [`plan`].
    Apt,
    /// Fedora and RHEL.
    Dnf,
    /// Arch.
    Pacman,
}

impl Installer {
    /// What the user would see it called.
    pub fn label(self) -> &'static str {
        match self {
            Installer::Winget => "winget",
            Installer::Homebrew => "Homebrew",
            Installer::Apt => "apt",
            Installer::Dnf => "dnf",
            Installer::Pacman => "pacman",
        }
    }

    /// Can superbackup run this itself, unattended?
    ///
    /// Only where the command needs no elevation and no repository has to be
    /// added first. Everywhere else the command is *shown* and the user runs
    /// it: a backup tool that starts asking for a root password is a backup
    /// tool people stop trusting, and one that edits apt sources on their
    /// behalf is worse.
    pub fn runnable(self) -> bool {
        matches!(self, Installer::Winget | Installer::Homebrew | Installer::Pacman)
    }

    /// The exact command, for running or for showing.
    fn argv(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Installer::Winget => (
                "winget",
                &[
                    "install",
                    "--id",
                    "GitHub.cli",
                    "--source",
                    "winget",
                    "--exact",
                    // Without these it waits for a keypress nobody can give
                    // it: this runs with no console attached.
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                    "--disable-interactivity",
                ],
            ),
            Installer::Homebrew => ("brew", &["install", "gh"]),
            Installer::Pacman => ("pacman", &["-S", "--noconfirm", "github-cli"]),
            Installer::Apt => ("sudo", &["apt", "install", "gh"]),
            Installer::Dnf => ("sudo", &["dnf", "install", "gh"]),
        }
    }

    /// The command as a person would type it.
    pub fn command_line(self) -> String {
        let (program, args) = self.argv();
        std::iter::once(program)
            .chain(args.iter().copied())
            // `--disable-interactivity` is for us, not for them; a person
            // running this in their own terminal wants the prompts.
            .filter(|a| *a != "--disable-interactivity")
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// What can be done about a missing `gh` on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// Already installed; there is nothing to do.
    pub installed: bool,
    /// The package manager found, if any.
    pub installer: Option<Installer>,
    /// True when superbackup can run it without a password prompt.
    pub can_run: bool,
    /// The command, so the interface can show it whether or not it can run it.
    pub command: Option<String>,
}

/// Is `gh` on PATH?
pub fn installed() -> bool {
    locate().is_some()
}

fn locate() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "gh.exe" } else { "gh" };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(exe)).find(|c| c.is_file())
}

fn which(tool: &str) -> bool {
    let exe = if cfg!(windows) { format!("{tool}.exe") } else { tool.to_string() };
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(&exe).is_file()))
        .unwrap_or(false)
}

/// What this machine can do about it.
pub fn plan() -> Plan {
    if installed() {
        return Plan { installed: true, installer: None, can_run: false, command: None };
    }
    // In the order each platform actually uses.
    let candidates: &[(Installer, &str)] = if cfg!(windows) {
        &[(Installer::Winget, "winget")]
    } else if cfg!(target_os = "macos") {
        &[(Installer::Homebrew, "brew")]
    } else {
        &[(Installer::Apt, "apt"), (Installer::Dnf, "dnf"), (Installer::Pacman, "pacman")]
    };
    let installer = candidates.iter().find(|(_, tool)| which(tool)).map(|(i, _)| *i);
    Plan {
        installed: false,
        can_run: installer.is_some_and(Installer::runnable),
        command: installer.map(Installer::command_line),
        installer,
    }
}

/// Install it, with the platform's package manager.
///
/// Long-running: a download and an install. The caller gives it a generous
/// timeout and tells the user it is happening, because a button that appears
/// to do nothing for ninety seconds is a button people press again.
pub async fn install() -> Result<String> {
    let plan = plan();
    if plan.installed {
        return Ok("The GitHub CLI is already installed.".to_string());
    }
    let Some(installer) = plan.installer else {
        return Err(Error::Config(
            "No package manager superbackup knows how to use was found. Install the GitHub CLI \
             from https://cli.github.com and sign in with `gh auth login`."
                .into(),
        ));
    };
    if !installer.runnable() {
        return Err(Error::Config(format!(
            "Installing the GitHub CLI here needs a password, and superbackup will not ask for \
             one. Run this yourself: {}",
            installer.command_line()
        )));
    }

    let (program, args) = installer.argv();
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    crate::kopia::harden_child(&mut command);

    let output = tokio::time::timeout(std::time::Duration::from_secs(300), command.output())
        .await
        .map_err(|_| {
            Error::Config(format!(
                "{} did not finish within five minutes. Run `{}` yourself to see what it is \
                 waiting for.",
                installer.label(),
                installer.command_line()
            ))
        })?
        .map_err(|e| Error::io(format!("running {}", installer.label()), e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let reason = stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("it did not say why");
        return Err(Error::Config(format!(
            "{} could not install the GitHub CLI: {reason}",
            installer.label()
        )));
    }

    // Installed is not the same as *usable*: winget puts it on the PATH of
    // processes started afterwards, and this one already has its environment.
    // Saying so is the difference between "nothing happened" and "restart it".
    if installed() {
        Ok("The GitHub CLI is installed. Sign in with `gh auth login`.".to_string())
    } else {
        Ok(format!(
            "{} installed the GitHub CLI. Restart superbackup so it picks up the new PATH, then \
             sign in with `gh auth login`.",
            installer.label()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The command line carries a fixed package id and nothing else.
    ///
    /// This function installs software. If a caller could ever influence what
    /// it installs, that would be the most serious defect in the codebase, so
    /// the shape is asserted rather than assumed.
    #[test]
    fn the_command_names_one_fixed_package_and_takes_no_input() {
        for installer in [
            Installer::Winget,
            Installer::Homebrew,
            Installer::Apt,
            Installer::Dnf,
            Installer::Pacman,
        ] {
            let (program, args) = installer.argv();
            assert!(!program.is_empty());
            for arg in args {
                assert!(!arg.is_empty(), "{installer:?}");
            }
            let line = installer.command_line();
            assert!(
                line.contains("gh") || line.contains("GitHub.cli") || line.contains("github-cli"),
                "{installer:?}: {line}"
            );
            // What a person would type, not what we pass: the flag that
            // suppresses prompts is ours alone.
            assert!(!line.contains("--disable-interactivity"), "{line}");
        }
    }

    /// Only the ones that need no password are run for the user.
    #[test]
    fn nothing_that_needs_a_password_is_run_on_the_users_behalf() {
        assert!(Installer::Winget.runnable());
        assert!(Installer::Homebrew.runnable());
        // These shell out through sudo, and a backup tool that starts asking
        // for a root password is one people stop trusting.
        assert!(!Installer::Apt.runnable());
        assert!(!Installer::Dnf.runnable());
        for installer in [Installer::Apt, Installer::Dnf] {
            assert!(
                installer.command_line().starts_with("sudo "),
                "{installer:?} is shown as needing elevation"
            );
        }
    }

    /// Whatever this machine has, the answer is usable: either it is already
    /// there, or there is a command to show.
    #[test]
    fn the_plan_always_leaves_the_user_with_something_to_do() {
        let plan = plan();
        if plan.installed {
            assert!(plan.installer.is_none());
            return;
        }
        // A machine with no recognised package manager still gets told where
        // to go, from `install`'s error rather than from a command.
        if let Some(command) = &plan.command {
            assert!(!command.is_empty());
            assert_eq!(plan.can_run, plan.installer.is_some_and(Installer::runnable));
        }
    }
}
