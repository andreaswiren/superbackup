//! The thin client's transport.
//!
//! One connection, one runtime, owned for the life of the command. This is
//! [`BlockingClient`](superbackup_core::ipc::BlockingClient) with the runtime
//! left reachable, because two of this program's commands — `watch` and
//! `run --wait` — need to `select!` a stream against Ctrl-C, and a wrapper
//! that only offers "send a request, get a reply" cannot express that.
//!
//! The one error message that matters most here is the one for a daemon that
//! is not running. `The system cannot find the file specified. (os error 2)`
//! tells a user nothing; exit code 3 and "start the tray app" tells them
//! everything.

use std::future::Future;
use std::time::Duration;

use superbackup_core::error::{Error, ErrorCode};
use superbackup_core::ipc::client::Hello;
use superbackup_core::ipc::protocol::{Reply, Request};
use superbackup_core::ipc::{AutoStart, Client, Subscription, Topic};

use super::context::Ctx;
use super::output::{CliError, CliResult};

/// Whether this command may start a daemon that is not running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// Never. Every read-only query uses this: asking a question is not
    /// permission to launch a background process.
    Never,
    /// Start one if nothing is listening, and say so on stderr first.
    /// Reserved for commands whose whole purpose needs a running instance —
    /// `init`, `run`, `restore`, `unlock`.
    IfNeeded,
}

/// A connection to the running instance, plus the runtime driving it.
pub struct Daemon {
    runtime: tokio::runtime::Runtime,
    client: Client,
}

impl Daemon {
    pub fn connect(ctx: &mut Ctx, start: Start) -> CliResult<Daemon> {
        let endpoint = ctx.endpoint();
        // A zero timeout would mean "give up before asking"; treat it as the
        // smallest sane wait rather than as a hang.
        let timeout = Duration::from_secs(ctx.global.timeout.max(1));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| {
                CliError::new(ErrorCode::Internal, format!("could not start a runtime: {e}"))
            })?;

        // Starting a *user* daemon when the caller asked for the machine-wide
        // service instance would answer the wrong question with the wrong
        // process, so `--service` never autostarts.
        let may_start = start == Start::IfNeeded && !ctx.global.service;

        let first = runtime.block_on(Client::connect_with(&endpoint, timeout));
        let client = match first {
            Ok(c) => c,
            Err(Error::DaemonUnreachable) if may_start => {
                let autostart = build_autostart(ctx)?;
                ctx.ui.announce(
                    "No superbackup instance was running. Starting one in the background.",
                );
                runtime
                    .block_on(Client::connect_or_start(&endpoint, &autostart))
                    .map_err(|e| unreachable_error(e, &endpoint))?
            }
            Err(e) => return Err(unreachable_error(e, &endpoint)),
        };

        Ok(Daemon { runtime, client })
    }

    /// Send a request and wait for its typed reply.
    pub fn request(&self, request: Request) -> CliResult<Reply> {
        self.runtime.block_on(self.client.request(request)).map_err(CliError::from)
    }

    /// Open a live stream.
    pub fn subscribe(&self, topics: Vec<Topic>) -> CliResult<Subscription> {
        self.runtime.block_on(self.client.subscribe(topics)).map_err(CliError::from)
    }

    /// Run a future on this command's runtime. Used by the two streaming
    /// commands, which need the stream and Ctrl-C in the same `select!`.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn hello(&self) -> &Hello {
        self.client.hello()
    }
}

/// `superbackup daemon`, carrying forward the configuration root so the
/// started instance reads the same files this invocation does.
///
/// Uses the current executable rather than a `PATH` lookup: a portable install
/// must start itself, not whichever copy happens to be earlier in `PATH`.
fn build_autostart(ctx: &Ctx) -> CliResult<AutoStart> {
    let mut args: Vec<&str> = vec!["daemon"];
    let home = ctx.global.home.as_ref().map(|h| h.display().to_string());
    if let Some(home) = home.as_deref() {
        args.push("--home");
        args.push(home);
    }
    AutoStart::current_exe(&args).map_err(CliError::from)
}

/// Turn a connection failure into something with an exit code and an action.
fn unreachable_error(e: Error, endpoint: &str) -> CliError {
    match e {
        Error::DaemonUnreachable => CliError::new(
            ErrorCode::DaemonUnreachable,
            format!("nothing is listening on {endpoint}, so there is no superbackup instance to ask"),
        )
        .with_hint(
            "Start it with `superbackup daemon`, open the tray application, or run \
             `superbackup service status` if you installed the service.",
        ),
        other => CliError::from(other),
    }
}

/// Send a request and unwrap its reply into the payload the command asked
/// for, as a `CliResult`.
///
/// A daemon that answers `job` to a `dest.list` is broken; the CLI says so and
/// exits, rather than panicking on an unreachable match arm. The result is a
/// value rather than a `?`-expression so that callers which want to *inspect*
/// the failure — `doctor`, which treats an unreachable daemon as a finding —
/// can do so without the macro forcing an early return.
macro_rules! reply {
    ($daemon:expr, $request:expr, $variant:ident) => {
        $daemon.request($request).and_then(|answer| match answer {
            ::superbackup_core::ipc::protocol::Reply::$variant(payload) => Ok(payload),
            other => Err($crate::cli::output::CliError::protocol(format!(
                "the daemon answered `{}` to a request that can only produce `{}`",
                other.tag(),
                stringify!($variant),
            ))),
        })
    };
}

pub(crate) use reply;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::testing;

    #[test]
    fn nothing_listening_is_exit_three_with_an_action() {
        let (mut ctx, _captured) = testing::unreachable_ctx(false);
        let error = Daemon::connect(&mut ctx, Start::Never).err().expect("must fail");
        assert_eq!(error.code, ErrorCode::DaemonUnreachable);
        assert_eq!(error.exit_code(), crate::cli::exit::DAEMON_UNREACHABLE);
        assert!(error.hint.is_some(), "the user must be told what to do");
        assert!(
            !error.message.contains("os error"),
            "a raw OS error is not an answer: {}",
            error.message
        );
    }

    #[test]
    fn a_read_only_command_never_launches_a_daemon() {
        // Start::Never is the only setting a query may use; if this ever
        // becomes possible to bypass, `superbackup status` on a fresh machine
        // starts a background service nobody asked for.
        let (mut ctx, captured) = testing::unreachable_ctx(false);
        let _ = Daemon::connect(&mut ctx, Start::Never);
        assert!(
            !captured.stderr().contains("Starting"),
            "a query must not announce, let alone perform, an autostart"
        );
    }

    #[test]
    fn a_reply_of_the_wrong_shape_is_an_error_not_a_panic() {
        let harness = testing::Harness::start("wrong-shape");
        let (mut ctx, _c) = harness.ctx(false);
        let daemon = Daemon::connect(&mut ctx, Start::Never).expect("connect");
        // `ping` answers `ack`; ask for it as `jobs`.
        let outcome = reply!(daemon, Request::Ping {}, Jobs);
        let error = outcome.err().expect("a shape mismatch must be an error");
        assert_eq!(error.code, ErrorCode::Ipc);
        assert!(error.message.contains("ack"), "{}", error.message);
    }
}
