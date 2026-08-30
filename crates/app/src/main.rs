//! superbackup — one executable, several personalities.
//!
//! Invoked with no arguments it becomes the tray icon, the scheduler and the
//! IPC server. Invoked with a subcommand it becomes a thin client that asks
//! the already-running instance and prints the answer. See
//! `docs/ARCHITECTURE.md` for why it is arranged that way.

mod cli;

use clap::Parser;

fn main() -> std::process::ExitCode {
    let parsed = cli::Cli::parse();
    match parsed.command {
        None => {
            // TODO(integration): start tray + scheduler + IPC server.
            println!("superbackup {} — tray mode not yet wired", superbackup_core::VERSION);
            std::process::ExitCode::SUCCESS
        }
        Some(cli::Command::Version) => {
            let info = superbackup_core::build_info();
            if parsed.global.json {
                println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
            } else {
                println!("superbackup {} ({} {})", info.version, info.target_os, info.target_arch);
            }
            std::process::ExitCode::SUCCESS
        }
        Some(_) => {
            // TODO(integration): dispatch over IPC to the running instance.
            eprintln!("superbackup: this command is not wired up yet");
            std::process::ExitCode::from(cli::exit::FAILED as u8)
        }
    }
}
