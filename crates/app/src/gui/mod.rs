//! The egui window: dashboard, jobs, destinations, providers, settings.
//!
//! Owned by the GUI workstream. Talks to the running instance over IPC like
//! any other client, so it can be developed and tested against a mock daemon.

use std::process::ExitCode;

use superbackup_core::paths::Paths;

pub mod copy;
pub mod daemon;
pub mod data;
pub mod format;
pub mod icons;
pub mod theme;
pub mod validation;
pub mod viewmodel;
pub mod widgets;

/// Open the window, or focus an already-open one.
pub fn open_or_focus(_paths: Paths, _global: &crate::cli::GlobalArgs) -> ExitCode {
    eprintln!("superbackup: the interface is not wired up yet");
    ExitCode::from(crate::cli::exit::FAILED as u8)
}
