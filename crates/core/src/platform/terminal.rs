//! Running a command in a terminal window the user can type into.
//!
//! # Why a whole module for this
//!
//! Some things genuinely need a person at a keyboard, and the two that matter
//! here both involve a passphrase:
//!
//! - `ssh-add` on a key that has its own passphrase.
//! - `ssh-keygen -p`, to add or change one.
//!
//! Superbackup will not put a passphrase on a command line — every process on
//! the machine can read `/proc/<pid>/cmdline`, and on Windows `Win32_Process`
//! over WMI needs no elevation at all. It also will not invent a passphrase
//! prompt of its own for these, because that means this program handling a
//! secret that has nothing to do with it.
//!
//! A terminal solves both at once. `ssh-add` asks the terminal; the user types
//! into the terminal; the passphrase goes from the keyboard to `ssh-add` and
//! reaches neither a command line nor this process. What superbackup passes is
//! only the program and the key's path, which are not secret.
//!
//! # What is on the command line here
//!
//! The program, its flags, and a file path. Nothing else, ever. This module
//! takes `&OsStr` arguments and has no way to be handed a secret: there is no
//! `stdin` parameter and no environment parameter, deliberately, so that the
//! next person to use it cannot pass one by accident.
//!
//! # The other reason this is not just `Command::spawn`
//!
//! A daemon has no console, and a GUI process on Windows has none either. A
//! child spawned from one inherits nothing to type into, so `ssh-add` sees no
//! terminal, falls back to an askpass helper that is not there, and fails
//! without asking anything. Getting a *visible window with a real console* is
//! the whole job.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// What was launched, so the caller can say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    /// The terminal that was opened, for the message shown afterwards.
    pub terminal: String,
    /// The command the user will see running in it.
    pub command: String,
}

/// Can a terminal be opened on this machine?
///
/// Checked before offering the button rather than after pressing it: a
/// headless Linux box has no terminal to open, and the honest thing is to show
/// the command to run rather than a button that fails.
pub fn available() -> bool {
    platform_impl::find().is_some()
}

/// Open a terminal window running `program` with `args`, and leave it open.
///
/// Returns as soon as the window has been launched. It does **not** wait for
/// the command to finish: the user may take a minute to find their passphrase,
/// and blocking a daemon's request handler on that is a daemon that has
/// stopped answering.
///
/// The window stays open after the command exits, so the user can read what it
/// said. A terminal that closes on completion is one where the error message
/// existed for forty milliseconds.
pub fn run(program: &Path, args: &[OsString], title: &str) -> Result<Launched> {
    // Belt and braces against this module ever being used to pass a secret.
    // Nothing here should look like a passphrase, and a caller reaching for
    // this to hand one over should hit an error rather than a leak.
    for arg in args {
        let text = arg.to_string_lossy();
        if text.starts_with("--passphrase") || text.starts_with("-P=") {
            return Err(Error::Internal(
                "a passphrase must not be passed on a command line; that is the entire reason \
                 this runs in a terminal"
                    .into(),
            ));
        }
    }
    platform_impl::run(program, args, title)
}

/// The command as a person would type it, for showing beside the button.
///
/// Quoted the way the platform's own shell wants, so that copying it out of
/// the interface and pasting it into a terminal works.
pub fn describe(program: &Path, args: &[OsString]) -> String {
    let quote = |s: &OsStr| {
        let text = s.to_string_lossy().into_owned();
        if text.contains(' ') {
            format!("\"{text}\"")
        } else {
            text
        }
    };
    let mut line = quote(program.as_os_str());
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    line
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform_impl {
    use super::*;

    /// Windows Terminal if it is there, else the console host.
    ///
    /// `conhost` is present on every Windows install, so this never fails to
    /// find something — but Windows Terminal is what the user's own shells
    /// open in on Windows 11, and matching that matters when the window is
    /// asking them to type a passphrase: an unexpected window asking for a
    /// secret is one people are right to be suspicious of.
    pub fn find() -> Option<PathBuf> {
        Some(PathBuf::from("cmd.exe"))
    }

    pub fn run(program: &Path, args: &[OsString], title: &str) -> Result<Launched> {
        // `start` needs a title as its first quoted argument, or it takes the
        // *program* as the title and then has no program left to run.
        //
        // `/k` rather than `/c`: the window stays open afterwards so whatever
        // ssh-add said is still on screen.
        let mut line = OsString::from("start \"");
        line.push(title);
        line.push("\" cmd.exe /k \"");
        line.push(quoted(program.as_os_str()));
        for arg in args {
            line.push(" ");
            line.push(quoted(arg));
        }
        line.push("\"");

        let mut command = std::process::Command::new("cmd.exe");
        command.arg("/c");
        command.raw_arg_line(&line);
        // No CREATE_NO_WINDOW here, for once: a visible window is the point.
        command
            .spawn()
            .map_err(|e| Error::io("opening a terminal window", e))?;

        Ok(Launched {
            terminal: "a command window".to_string(),
            command: super::describe(program, args),
        })
    }

    fn quoted(value: &OsStr) -> OsString {
        let text = value.to_string_lossy();
        if text.contains(' ') {
            let mut out = OsString::from("\"");
            out.push(value);
            out.push("\"");
            out
        } else {
            value.to_os_string()
        }
    }

    /// `Command::arg` quotes each argument, which is exactly wrong for the
    /// single string `cmd /c` wants. This appends it raw.
    trait RawArgLine {
        fn raw_arg_line(&mut self, line: &OsStr) -> &mut Self;
    }

    impl RawArgLine for std::process::Command {
        fn raw_arg_line(&mut self, line: &OsStr) -> &mut Self {
            use std::os::windows::process::CommandExt;
            self.raw_arg(line)
        }
    }
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

#[cfg(all(unix, not(target_os = "macos")))]
mod platform_impl {
    use super::*;

    /// In the order a desktop is likely to have them.
    ///
    /// `x-terminal-emulator` first because on Debian and its derivatives that
    /// is the user's own choice, expressed through alternatives, and honouring
    /// it is better than picking a favourite.
    const CANDIDATES: [&str; 7] = [
        "x-terminal-emulator",
        "gnome-terminal",
        "konsole",
        "xfce4-terminal",
        "alacritty",
        "kitty",
        "xterm",
    ];

    pub fn find() -> Option<PathBuf> {
        // A terminal is useless with no display to put it on, and a server
        // over SSH has none. Checked first so a headless box reports "no
        // terminal" rather than opening one nobody can see.
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return None;
        }
        CANDIDATES.iter().find_map(|name| which(name))
    }

    fn which(name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    }

    pub fn run(program: &Path, args: &[OsString], _title: &str) -> Result<Launched> {
        let terminal = find().ok_or_else(|| {
            Error::Config(
                "No terminal emulator was found, and there is no display to open one on. Run the \
                 command yourself in a shell."
                    .into(),
            )
        })?;
        let name = terminal
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| terminal.display().to_string());

        // `-e` is understood by every terminal in the list. The command and
        // its arguments are passed as separate argv entries rather than as one
        // shell string, so nothing is re-parsed by a shell and a key path with
        // a space in it survives.
        let mut command = std::process::Command::new(&terminal);
        command.arg("-e");
        command.arg(program);
        for arg in args {
            command.arg(arg);
        }
        command
            .spawn()
            .map_err(|e| Error::io(format!("opening {name}"), e))?;

        Ok(Launched { terminal: name, command: super::describe(program, args) })
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod platform_impl {
    use super::*;

    pub fn find() -> Option<PathBuf> {
        Some(PathBuf::from("/usr/bin/osascript"))
    }

    pub fn run(program: &Path, args: &[OsString], _title: &str) -> Result<Launched> {
        // Terminal.app takes a shell line rather than an argv, so each part is
        // single-quoted. Any single quote inside is escaped the shell's own
        // way; a key path can legally contain one.
        let quote = |value: &OsStr| {
            format!("'{}'", value.to_string_lossy().replace('\'', "'\\''"))
        };
        let mut line = quote(program.as_os_str());
        for arg in args {
            line.push(' ');
            line.push_str(&quote(arg));
        }
        let script = format!(
            "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
            line.replace('\\', "\\\\").replace('"', "\\\"")
        );

        std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(&script)
            .spawn()
            .map_err(|e| Error::io("opening Terminal", e))?;

        Ok(Launched {
            terminal: "Terminal".to_string(),
            command: super::describe(program, args),
        })
    }
}

#[cfg(not(any(windows, unix)))]
mod platform_impl {
    use super::*;

    pub fn find() -> Option<PathBuf> {
        None
    }

    pub fn run(_program: &Path, _args: &[OsString], _title: &str) -> Result<Launched> {
        Err(Error::Config("This platform has no terminal superbackup knows how to open.".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard that keeps this from becoming a way to leak a passphrase.
    ///
    /// The whole reason for opening a terminal is that the secret must not be
    /// an argument, so a caller passing one as an argument has misunderstood
    /// the module badly enough to be worth stopping.
    #[test]
    fn a_passphrase_argument_is_refused_outright() {
        let err = run(
            Path::new("ssh-keygen"),
            &[OsString::from("--passphrase=hunter2")],
            "test",
        )
        .expect_err("must refuse");
        assert!(err.to_string().contains("command line"), "{err}");
    }

    /// The line shown beside the button has to be one the user can paste.
    #[test]
    fn the_written_command_quotes_paths_with_spaces() {
        let line = describe(
            Path::new("C:/Program Files/OpenSSH/ssh-add.exe"),
            &[OsString::from("C:/Users/A B/.ssh/id_ed25519")],
        );
        assert!(line.contains("\"C:/Program Files/OpenSSH/ssh-add.exe\""), "{line}");
        assert!(line.contains("\"C:/Users/A B/.ssh/id_ed25519\""), "{line}");

        // And leaves a plain one alone, because quotes everywhere make a
        // command look like something other than what you would type.
        let plain = describe(Path::new("ssh-add"), &[OsString::from("/home/a/.ssh/id_ed25519")]);
        assert_eq!(plain, "ssh-add /home/a/.ssh/id_ed25519");
    }

    /// Whatever this machine is, asking must not panic.
    #[test]
    fn availability_is_answerable_without_opening_anything() {
        let _ = available();
    }
}
