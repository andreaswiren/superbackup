//! The command-line interface: argument definitions, the machine-readable
//! schema, output formatting, and the thin client that forwards everything to
//! the running instance.

pub mod args;
pub mod schema;

pub use args::{exit, Cli, Command};
pub use schema::Schema;
