//! # superbackup-core
//!
//! The engine behind superbackup: configuration, the encrypted secret vault,
//! the Kopia command-line driver, the scheduler, platform integration, and the
//! IPC protocol shared by the tray, the GUI and the CLI.
//!
//! ## Layering
//!
//! ```text
//!  model  ── the user's intent (jobs, destinations, providers, schedules)
//!  state  ── what actually happened (runs, progress, events, health)
//!    │
//!  config ── load/save/migrate `model`, atomically
//!  crypto ── seal every secret `model` refers to
//!    │
//!  kopia  ── drive the kopia CLI, parse its JSON progress
//!  engine ── decide what to run, run it, throttle it, record it
//!    │
//!  ipc    ── expose all of the above to the tray, GUI and CLI
//! ```
//!
//! ## Security invariants
//!
//! 1. Secret material exists only inside [`secret::Secret`], which zeroes on
//!    drop and refuses to `Display` or `Serialize` itself.
//! 2. `config.json` never contains a secret — only [`model::SecretRef`]
//!    handles resolved against the unlocked vault.
//! 3. Secrets reach kopia through environment variables and stdin, never
//!    through argv, which is world-readable on every supported platform.
//! 4. Anything written to a log, an event, an IPC response, or a notification
//!    passes through redaction first.

#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_debug_implementations, rust_2018_idioms)]

pub mod config;
pub mod crypto;
pub mod engine;
pub mod error;
pub mod ipc;
pub mod kopia;
pub mod model;
pub mod paths;
pub mod platform;
pub mod redact;
pub mod remote;
pub mod secret;
pub mod state;

pub use error::{Error, ErrorCode, Result};

/// Semantic version of the running build, from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Human-readable build identifier shown in the About screen and in
/// `superbackup version --json`.
pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: VERSION,
        target_os: std::env::consts::OS,
        target_arch: std::env::consts::ARCH,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BuildInfo {
    pub version: &'static str,
    pub target_os: &'static str,
    pub target_arch: &'static str,
}
