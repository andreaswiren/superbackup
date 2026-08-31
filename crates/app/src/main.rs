//! superbackup — one executable, several personalities.
//!
//! | Invocation | Role |
//! |---|---|
//! | `superbackup` | Tray icon + scheduler + IPC server, in one process |
//! | `superbackup gui` | Open the window, or focus the running instance |
//! | `superbackup daemon` | Headless scheduler + IPC server, no tray |
//! | `superbackup service run` | Service entry point, invoked by the OS |
//! | anything else | Thin client: ask the running instance, print the answer |
//!
//! There is one executable to install, sign and update, and a user's mental
//! model is "superbackup is running or it isn't" rather than "which of the four
//! superbackup processes is the broken one".
//!
//! The CLI never opens a repository or touches the vault. Only one process
//! ever holds that role, because two processes driving one Kopia repository
//! risks corrupting it. See `docs/ARCHITECTURE.md`.

// This binary is deliberately a **console** application on Windows, even though
// its default mode is a tray icon.
//
// The obvious choice is `windows_subsystem = "windows"`, and it was wrong. A
// GUI-subsystem process has no console, so `superbackup status` typed at a
// PowerShell prompt printed nothing at all — and worse, the shell does not
// *wait* for a GUI-subsystem process, so it returned immediately and never set
// an exit code. A CLI that prints nothing and reports no status is not a CLI,
// and this one is a headline feature that an agent is meant to drive.
//
// So: console subsystem, and the tray and GUI detach from the console at
// startup (see `detach_console`). The cost is one brief console flash when the
// tray is launched from Explorer or at login. The alternative cost was a silent
// CLI in every terminal, which is far worse.

mod cli;
mod daemon;
mod gui;
mod service;
mod tray;

use std::process::ExitCode;

use clap::Parser;
use superbackup_core::paths::Paths;

/// Release the console this process was given, for modes that have a window
/// instead of a terminal.
///
/// The tray and the GUI are long-lived and must not hold a console open behind
/// them. `FreeConsole` closes it immediately; when the process was launched
/// from an existing terminal there is nothing to close and this is a no-op that
/// simply detaches.
#[cfg(windows)]
fn detach_console() {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

    // SAFETY: both calls take no memory we own. Hiding before freeing keeps the
    // window from lingering for a frame on slower machines.
    unsafe {
        let window = GetConsoleWindow();
        if !window.is_null() {
            ShowWindow(window, SW_HIDE);
        }
        FreeConsole();
    }
}

fn main() -> ExitCode {
    let parsed = cli::Cli::parse();

    // Modes that own a window rather than a terminal give the console back.
    // `daemon` deliberately keeps it: it logs to stdout and is usually run in a
    // terminal on purpose.
    #[cfg(windows)]
    if matches!(parsed.command, None | Some(cli::Command::Gui)) {
        detach_console();
    }
    let global = parsed.global.clone();

    let paths = match resolve_paths(&global) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("superbackup: {e}");
            if let Some(hint) = e.hint() {
                eprintln!("  {hint}");
            }
            return ExitCode::from(cli::exit::USAGE as u8);
        }
    };

    match parsed.command {
        // No subcommand: the tray, the scheduler and the IPC server, together.
        None => daemon::run_foreground(paths, &global, daemon::Surface::Tray),

        Some(cli::Command::Daemon(args)) => {
            let surface =
                if args.no_tray { daemon::Surface::Headless } else { daemon::Surface::Tray };
            daemon::run_foreground(paths, &global, surface)
        }

        Some(cli::Command::Gui) => gui::open_or_focus(paths, &global),

        Some(cli::Command::Service(cli::args::ServiceCommand::Run)) => {
            service::run_as_service(paths, &global)
        }

        // Answered in-process: these describe the binary itself and must work
        // with no daemon running, no configuration, and no vault.
        Some(cli::Command::Schema) => match cli::Schema::generate().to_json() {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("superbackup: could not render schema: {e}");
                ExitCode::from(cli::exit::FAILED as u8)
            }
        },

        Some(cli::Command::Version) => {
            let info = superbackup_core::build_info();
            if global.json {
                match serde_json::to_string_pretty(&info) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("superbackup: {e}");
                        return ExitCode::from(cli::exit::FAILED as u8);
                    }
                }
            } else {
                println!("superbackup {} ({} {})", info.version, info.target_os, info.target_arch);
            }
            ExitCode::SUCCESS
        }

        // Everything else is a request to the running instance.
        Some(command) => cli::execute(command, global, paths),
    }
}

/// Honour `--home` / `SUPERBACKUP_HOME`, and pick the per-user or the
/// machine-wide layout.
fn resolve_paths(global: &cli::GlobalArgs) -> superbackup_core::Result<Paths> {
    let paths = match (&global.home, global.service) {
        (Some(root), service) => Paths::rooted_at(root.clone(), service),
        (None, true) => Paths::for_service()?,
        (None, false) => Paths::discover()?,
    };
    Ok(paths)
}
