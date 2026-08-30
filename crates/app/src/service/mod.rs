//! OS service entry points.
//!
//! Owned by the daemon workstream.

use std::process::ExitCode;

use superbackup_core::paths::Paths;

/// Entry point invoked by the operating system's service manager.
pub fn run_as_service(_paths: Paths, _global: &crate::cli::GlobalArgs) -> ExitCode {
    eprintln!("superbackup: the service host is not wired up yet");
    ExitCode::from(crate::cli::exit::FAILED as u8)
}
