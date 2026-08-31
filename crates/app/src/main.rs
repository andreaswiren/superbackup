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

// The tray/GUI build must not flash a console window on Windows. Debug builds
// keep the console so `cargo run` still shows tracing output.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod cli;
mod daemon;
mod gui;
mod service;
mod tray;

use std::process::ExitCode;

use clap::Parser;
use superbackup_core::paths::Paths;

fn main() -> ExitCode {
    let parsed = cli::Cli::parse();
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
