//! The command-line interface: argument definitions, output formatting, and
//! the thin client that forwards everything to the running instance.

pub mod args;

pub use args::{exit, Cli, Command};
