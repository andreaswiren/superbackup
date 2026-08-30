//! The running instance: scheduler, engine, IPC server, and (optionally) tray.
//!
//! Owned by the daemon workstream. Entry points called from `main.rs` are the
//! contract; everything else here is private.

use std::process::ExitCode;

use superbackup_core::paths::Paths;

/// Whether this instance shows a tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// Tray icon present; the GUI can be opened from it.
    Tray,
    /// No tray. Used by `--no-tray` and by the service host.
    Headless,
}

/// Start the scheduler, the engine and the IPC server, and block until exit.
pub fn run_foreground(
    _paths: Paths,
    _global: &crate::cli::GlobalArgs,
    _surface: Surface,
) -> ExitCode {
    eprintln!("superbackup: the daemon is not wired up yet");
    ExitCode::from(crate::cli::exit::FAILED as u8)
}
