//! The command-line interface: argument definitions, the machine-readable
//! schema, output formatting, and the thin client that forwards everything to
//! the running instance.

pub mod args;
pub mod schema;

pub mod client;
pub mod commands;
pub mod context;
pub mod format;
pub mod output;
pub mod prompt;
pub mod resolve;
pub mod schedule;
#[cfg(test)]
pub mod testing;
pub mod timespec;

pub use args::{exit, Cli, Command, GlobalArgs};
pub use schema::Schema;

use std::process::ExitCode;

use context::Ctx;
use output::Ui;

/// Dispatch a command to the running instance and print the result.
///
/// The shape of this function is the guarantee that `--json` is trustworthy:
/// commands render human output as they go and *return* the machine-readable
/// value, and the single JSON document is written here, once, after the
/// command has finished. A command therefore cannot interleave prose with a
/// document, and cannot emit two documents.
pub fn execute(
    command: Command,
    global: GlobalArgs,
    paths: superbackup_core::paths::Paths,
) -> ExitCode {
    init_tracing(&global);

    let ui = Ui::from_env(&global);
    let mut ctx = Ctx::new(global, paths, ui);

    let code = match commands::dispatch(&mut ctx, command) {
        Ok(outcome) => {
            ctx.ui.finish(&outcome);
            outcome.exit
        }
        Err(error) => {
            ctx.ui.fail(&error);
            error.exit_code()
        }
    };
    ctx.ui.flush();
    ExitCode::from(code as u8)
}

/// Send tracing to **stderr**, never to stdout.
///
/// `-v` is a debugging aid, and a debugging aid that appears in the middle of
/// a JSON document has broken the thing it was meant to help diagnose.
fn init_tracing(global: &GlobalArgs) {
    use tracing_subscriber::EnvFilter;

    // Verbosity applies to superbackup's own spans. Turning tokio and hyper up
    // to trace as well would bury the lines the user asked for.
    let directives = match global.verbose {
        0 if global.quiet => "error",
        0 => "warn",
        1 => "warn,superbackup=info,superbackup_core=info",
        2 => "warn,superbackup=debug,superbackup_core=debug",
        _ => "info,superbackup=trace,superbackup_core=trace",
    };

    // An explicit RUST_LOG is the user being specific; do not override it.
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(directives))
        .unwrap_or_else(|_| EnvFilter::new("warn"));

    // `try_init` rather than `init`: a second call must not abort the program
    // over logging.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(global.verbose >= 2)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_env_filter(filter)
        .try_init();
}
