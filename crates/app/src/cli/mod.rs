//! The command-line interface: argument definitions, the machine-readable
//! schema, output formatting, and the thin client that forwards everything to
//! the running instance.

pub mod args;
pub mod schema;

pub use args::{exit, Cli, Command, GlobalArgs};
pub use schema::Schema;

/// Dispatch a command to the running instance and print the result.
///
/// Owned by the CLI workstream.
pub fn execute(
    _command: Command,
    _global: GlobalArgs,
    _paths: superbackup_core::paths::Paths,
) -> std::process::ExitCode {
    eprintln!("superbackup: this command is not wired up yet");
    std::process::ExitCode::from(exit::FAILED as u8)
}
