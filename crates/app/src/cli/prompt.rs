//! Asking the user things, and refusing to when nobody is there.
//!
//! Two rules run through this module.
//!
//! **A passphrase never reaches `argv`.** There is no `--passphrase` flag and
//! there is no route that reintroduces one: a passphrase on a command line is
//! readable by every other process on the machine through the process list,
//! and is written verbatim into shell history. It arrives by prompt or by
//! `--passphrase-file`, and nowhere else. Every buffer it passes through is a
//! [`Secret`], which zeroes itself on drop.
//!
//! **`--no-input` fails rather than waits.** A destructive command blocked on
//! an invisible prompt is indistinguishable from a crash, and the script that
//! called it will sit there until somebody notices. So the prompt does not
//! happen: the command exits 2 and says which flag would have answered it.

use std::io::{BufRead, Read, Write};
use std::path::Path;

use superbackup_core::error::ErrorCode;
use superbackup_core::secret::Secret;

use super::context::Ctx;
use super::output::{CliError, CliResult};

/// Ask a yes/no question about something destructive.
///
/// Returns `Ok(())` only on an explicit yes. Declining is
/// [`ErrorCode::JobCancelled`] — exit code 5 — because the user *did* answer,
/// and a script wants to tell "you said no" apart from "that failed".
pub fn confirm(ctx: &mut Ctx, what: &str, skip: bool) -> CliResult<()> {
    if skip {
        return Ok(());
    }
    if ctx.global.no_input {
        return Err(CliError::usage(format!(
            "{what} needs confirmation, and --no-input forbids prompting"
        ))
        .with_hint("Pass -y to confirm without being asked."));
    }

    let question = format!("{what}. Continue? [y/N] ");
    let answer = ask_line(ctx, &question)?;
    let answer = answer.trim().to_lowercase();
    if answer == "y" || answer == "yes" {
        Ok(())
    } else {
        Err(CliError::new(ErrorCode::JobCancelled, "cancelled; nothing was changed"))
    }
}

/// Ask for a line of ordinary, non-secret text.
///
/// The prompt goes to stderr so that `--json` output and any pipeline reading
/// stdout stay clean.
pub fn ask_line(ctx: &mut Ctx, question: &str) -> CliResult<String> {
    if ctx.global.no_input {
        return Err(CliError::usage(format!(
            "this command needs an answer to \"{}\", and --no-input forbids prompting",
            question.trim()
        )));
    }
    eprint!("{question}");
    let _ = std::io::stderr().flush();

    let mut line = String::new();
    let read = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| CliError::new(ErrorCode::Io, format!("could not read your answer: {e}")))?;
    if read == 0 {
        // Closed stdin. Treat as a decline rather than looping or blocking.
        return Ok(String::new());
    }
    Ok(line)
}

/// Ask a question with a default, for wizard steps.
pub fn ask_with_default(ctx: &mut Ctx, question: &str, default: &str) -> CliResult<String> {
    let answer = ask_line(ctx, &format!("{question} [{default}] "))?;
    let trimmed = answer.trim();
    Ok(if trimmed.is_empty() { default.to_string() } else { trimmed.to_string() })
}

pub fn ask_yes_no(ctx: &mut Ctx, question: &str, default_yes: bool) -> CliResult<bool> {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    let answer = ask_line(ctx, &format!("{question} {suffix} "))?;
    match answer.trim().to_lowercase().as_str() {
        "" => Ok(default_yes),
        "y" | "yes" => Ok(true),
        _ => Ok(false),
    }
}

// ---------------------------------------------------------------------------
// Passphrases
// ---------------------------------------------------------------------------

/// Where a passphrase comes from: the file the user named, or their keyboard.
///
/// `file` is `--passphrase-file`; `-` means stdin, so a script can pipe one in
/// without it ever existing on disk.
pub fn passphrase(ctx: &mut Ctx, file: Option<&Path>, question: &str) -> CliResult<Secret> {
    match file {
        Some(path) => from_file(path),
        None => from_terminal(ctx, question),
    }
}

/// Read a passphrase from a file, or from stdin when the path is `-`.
///
/// Only the first line is taken, with its line ending removed: an editor that
/// adds a trailing newline must not change the passphrase. Everything read is
/// wrapped in a [`Secret`] so the buffer is zeroed on the way out, including
/// the part that was discarded.
pub fn from_file(path: &Path) -> CliResult<Secret> {
    let raw: Vec<u8> = if path == Path::new("-") {
        let mut buffer = Vec::new();
        std::io::stdin()
            .lock()
            .read_to_end(&mut buffer)
            .map_err(|e| CliError::new(ErrorCode::Io, format!("reading the passphrase from stdin: {e}")))?;
        buffer
    } else {
        std::fs::read(path).map_err(|e| {
            CliError::new(
                ErrorCode::Io,
                format!("reading the passphrase from {}: {e}", path.display()),
            )
            .with_hint("The file must contain the passphrase on its first line.")
        })?
    };

    let end = raw.iter().position(|b| *b == b'\n').unwrap_or(raw.len());
    let mut line = raw[..end].to_vec();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    // Dropping a `Secret` zeroes it, so this scrubs the full buffer including
    // whatever followed the first line.
    let _scrub = Secret::new(raw);

    if line.is_empty() {
        let _scrub_line = Secret::new(line);
        let source =
            if path == Path::new("-") { "stdin".to_string() } else { path.display().to_string() };
        return Err(CliError::new(
            ErrorCode::Validation,
            format!("no passphrase was found in {source}"),
        ));
    }
    Ok(Secret::new(line))
}

/// Prompt on a terminal, with the echo turned off.
///
/// Refuses in three situations, all of which would otherwise end badly:
/// under `--no-input`, when stdin is not a terminal, and when this platform
/// will not let the echo be turned off. The last one matters: printing a
/// passphrase into the scrollback of a shared terminal is worse than failing.
pub fn from_terminal(ctx: &mut Ctx, question: &str) -> CliResult<Secret> {
    if ctx.global.no_input {
        return Err(CliError::usage("a passphrase is needed and --no-input forbids prompting")
            .with_hint("Pass --passphrase-file FILE, or `--passphrase-file -` to read stdin."));
    }
    if !ctx.ui.stdin_is_tty {
        return Err(CliError::usage("a passphrase is needed but there is no terminal to ask on")
            .with_hint("Pass --passphrase-file FILE, or `--passphrase-file -` to read stdin."));
    }

    let Some(guard) = echo::disable() else {
        return Err(CliError::new(
            ErrorCode::Platform,
            "this terminal will not let superbackup turn off the echo, so it will not ask for \
             a passphrase here",
        )
        .with_hint("Pass --passphrase-file FILE instead."));
    };

    eprint!("{question}");
    let _ = std::io::stderr().flush();

    let mut typed = String::new();
    let read = std::io::stdin().lock().read_line(&mut typed);
    drop(guard);
    // The user's Return was not echoed, so the cursor is still on the prompt.
    eprintln!();

    let read = read
        .map_err(|e| CliError::new(ErrorCode::Io, format!("could not read the passphrase: {e}")))?;
    if read == 0 {
        return Err(CliError::new(ErrorCode::JobCancelled, "no passphrase was entered"));
    }
    while typed.ends_with('\n') || typed.ends_with('\r') {
        typed.pop();
    }
    if typed.is_empty() {
        let _scrub = Secret::from_string(typed);
        return Err(CliError::new(ErrorCode::Validation, "an empty passphrase was not accepted"));
    }
    Ok(Secret::from_string(typed))
}

/// Ask twice and compare, for a passphrase that is being *set* rather than
/// checked. A typo in a passphrase there is no recovery from is not a
/// recoverable mistake.
pub fn new_passphrase(ctx: &mut Ctx, first: &str, second: &str) -> CliResult<Secret> {
    let one = from_terminal(ctx, first)?;
    let two = from_terminal(ctx, second)?;
    if !one.ct_eq(&two) {
        return Err(CliError::new(ErrorCode::Validation, "those two passphrases are not the same"));
    }
    drop(two);
    Ok(one)
}

// ---------------------------------------------------------------------------
// Terminal echo
// ---------------------------------------------------------------------------

/// Turning the terminal echo off, per platform.
///
/// This crate has neither `libc` nor `windows-sys` among its dependencies, so
/// each platform is handled with what it already links: three kernel32 calls
/// on Windows, and `stty` on unix. Neither ever sees the passphrase — they
/// only change the mode of the terminal that the passphrase is typed into.
mod echo {
    #[cfg(windows)]
    mod imp {
        use std::ffi::c_void;

        const STD_INPUT_HANDLE: u32 = -10i32 as u32;
        const ENABLE_ECHO_INPUT: u32 = 0x0004;

        #[link(name = "kernel32")]
        extern "system" {
            fn GetStdHandle(which: u32) -> *mut c_void;
            fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
            fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
        }

        /// Restores the console mode it found, whatever happens after.
        pub struct Guard {
            handle: *mut c_void,
            previous: u32,
        }

        pub fn disable() -> Option<Guard> {
            // SAFETY: the three calls take a handle the OS returned and a
            // pointer to a local `u32`; none of them retain either.
            unsafe {
                let handle = GetStdHandle(STD_INPUT_HANDLE);
                if handle.is_null() || handle as isize == -1 {
                    return None;
                }
                let mut previous: u32 = 0;
                if GetConsoleMode(handle, &mut previous) == 0 {
                    // Not a console: a redirected stdin has no mode to change.
                    return None;
                }
                // Line input stays on, so a plain `read_line` still works and
                // editing keys still behave; only the echo goes.
                if SetConsoleMode(handle, previous & !ENABLE_ECHO_INPUT) == 0 {
                    return None;
                }
                Some(Guard { handle, previous })
            }
        }

        impl Drop for Guard {
            fn drop(&mut self) {
                // SAFETY: same handle, same contract as above.
                unsafe {
                    SetConsoleMode(self.handle, self.previous);
                }
            }
        }
    }

    #[cfg(unix)]
    mod imp {
        use std::process::{Command, Stdio};

        pub struct Guard;

        pub fn disable() -> Option<Guard> {
            stty("-echo").then_some(Guard)
        }

        impl Drop for Guard {
            fn drop(&mut self) {
                stty("echo");
            }
        }

        /// `stty` needs the controlling terminal on its own stdin, which is
        /// why stdin is inherited rather than piped.
        fn stty(arg: &str) -> bool {
            Command::new("stty")
                .arg(arg)
                .stdin(Stdio::inherit())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    #[cfg(not(any(windows, unix)))]
    mod imp {
        pub struct Guard;
        pub fn disable() -> Option<Guard> {
            None
        }
    }

    pub use imp::disable;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::testing;

    #[test]
    fn no_input_refuses_to_prompt_instead_of_hanging() {
        let (mut ctx, _c) = testing::unreachable_ctx(false);
        assert!(ctx.global.no_input);
        let error = confirm(&mut ctx, "This deletes the job", false)
            .err()
            .expect("a prompt under --no-input must fail");
        assert_eq!(error.exit_code(), crate::cli::exit::USAGE);
        assert!(error.hint.unwrap_or_default().contains("-y"), "it must name the escape hatch");
    }

    #[test]
    fn confirmation_is_skipped_when_the_user_already_said_yes() {
        let (mut ctx, _c) = testing::unreachable_ctx(false);
        assert!(confirm(&mut ctx, "This deletes the job", true).is_ok());
    }

    #[test]
    fn no_input_refuses_to_ask_for_a_passphrase() {
        let (mut ctx, _c) = testing::unreachable_ctx(false);
        let error = from_terminal(&mut ctx, "Passphrase: ").err().expect("must refuse");
        assert_eq!(error.exit_code(), crate::cli::exit::USAGE);
        assert!(error.hint.unwrap_or_default().contains("--passphrase-file"));
    }

    #[test]
    fn a_passphrase_file_gives_up_only_its_first_line() {
        let dir = std::env::temp_dir().join(format!("sb-pp-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("pp.txt");
        std::fs::write(&path, b"correct horse battery staple\nand a trailing comment\n")
            .expect("write");
        let secret = from_file(&path).expect("reads");
        assert_eq!(secret.expose_str(), Some("correct horse battery staple"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_windows_line_ending_is_not_part_of_the_passphrase() {
        let dir = std::env::temp_dir().join(format!("sb-pp-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("pp.txt");
        std::fs::write(&path, b"hunter2\r\n").expect("write");
        assert_eq!(from_file(&path).expect("reads").expose_str(), Some("hunter2"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_passphrase_file_is_refused_rather_than_sent() {
        let dir = std::env::temp_dir().join(format!("sb-pp-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("pp.txt");
        std::fs::write(&path, b"\n").expect("write");
        let error = from_file(&path).err().expect("an empty passphrase is not a passphrase");
        assert_eq!(error.code, ErrorCode::Validation);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_passphrase_file_says_which_file() {
        let error = from_file(Path::new("/definitely/not/here.txt")).err().expect("must fail");
        assert_eq!(error.code, ErrorCode::Io);
        assert!(error.message.contains("here.txt"), "{}", error.message);
    }
}
