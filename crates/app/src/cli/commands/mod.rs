//! One module per group of commands, and the dispatcher that reaches them.

pub mod data;
pub mod doctor;
pub mod everyday;
pub mod objects;
pub mod setup;
pub mod vault;

use superbackup_core::ipc::protocol::Request;
use superbackup_core::model::{Destination, Job, StorageProvider};

use super::args::Command;
use super::client::{reply, Daemon};
use super::context::Ctx;
use super::output::{CliResult, Outcome};
use super::resolve::{self, Kind};

/// Route a parsed command to the code that performs it.
///
/// `Gui`, `Daemon`, `Schema`, `Version` and `service run` are handled in
/// `main.rs` before this is reached: they describe or start the process itself
/// and must work with no daemon, no configuration and no vault. They are
/// listed here anyway so that adding a command to `args.rs` without wiring it
/// up is a compile error rather than a silent no-op.
pub fn dispatch(ctx: &mut Ctx, command: Command) -> CliResult<Outcome> {
    match command {
        Command::Status(args) => everyday::status(ctx, args),
        Command::Run(args) => everyday::run(ctx, args),
        Command::Stop(args) => everyday::stop(ctx, args),
        Command::Pause(args) => everyday::pause(ctx, args),
        Command::Resume => everyday::resume(ctx),
        Command::Watch(args) => everyday::watch(ctx, args),

        Command::Job(sub) => objects::job(ctx, sub),
        Command::Destination(sub) => objects::destination(ctx, sub),
        Command::Provider(sub) => objects::provider(ctx, sub),
        Command::Project(sub) => objects::project(ctx, sub),

        Command::Snapshots(args) => data::snapshots(ctx, args),
        Command::Restore(args) => data::restore(ctx, args),
        Command::Browse(args) => data::browse(ctx, args),

        Command::Unlock(args) => vault::unlock(ctx, args),
        Command::Lock => vault::lock(ctx),
        Command::ChangePassphrase => vault::change_passphrase(ctx),

        Command::Init(args) => setup::init(ctx, args),
        Command::Service(sub) => setup::service(ctx, sub),
        Command::Autostart(sub) => setup::autostart(ctx, sub),
        Command::Config(sub) => setup::config(ctx, sub),
        Command::Remote(sub) => setup::remote(ctx, sub),

        Command::Doctor(args) => doctor::doctor(ctx, args),

        // Reached only if main.rs stops intercepting these.
        Command::Gui | Command::Daemon(_) | Command::Schema | Command::Version => {
            Err(super::output::CliError::new(
                superbackup_core::error::ErrorCode::Internal,
                "this command is handled before the thin client and should not have reached it",
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Shared lookups
// ---------------------------------------------------------------------------
//
// Every command that takes a name fetches the list and resolves locally rather
// than passing the raw string to the daemon. The daemon resolves prefixes too,
// but it cannot tell the user *which* jobs a prefix collided with, and a
// backup tool that guesses is a backup tool that eventually restores the wrong
// folder.

pub fn jobs(daemon: &Daemon) -> CliResult<Vec<Job>> {
    Ok(reply!(daemon, Request::JobList { include_disabled: true }, Jobs)?.jobs)
}

pub fn destinations(daemon: &Daemon) -> CliResult<Vec<Destination>> {
    Ok(reply!(daemon, Request::DestinationList {}, Destinations)?.destinations)
}

pub fn providers(daemon: &Daemon) -> CliResult<Vec<StorageProvider>> {
    Ok(reply!(daemon, Request::ProviderList {}, Providers)?.providers)
}

pub fn resolve_job(daemon: &Daemon, needle: &str) -> CliResult<Job> {
    let all = jobs(daemon)?;
    resolve::one(needle, &all, Kind::Job).cloned()
}

pub fn resolve_destination(daemon: &Daemon, needle: &str) -> CliResult<Destination> {
    let all = destinations(daemon)?;
    resolve::one(needle, &all, Kind::Destination).cloned()
}

pub fn resolve_provider(daemon: &Daemon, needle: &str) -> CliResult<StorageProvider> {
    let all = providers(daemon)?;
    resolve::one(needle, &all, Kind::Provider).cloned()
}
