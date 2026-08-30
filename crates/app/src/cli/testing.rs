//! Scaffolding for the CLI's own tests.
//!
//! Two things live here: a pair of capturing streams so a test can read what a
//! command printed, and a harness that binds a real IPC endpoint in front of
//! [`MockHandler`] so every command can be exercised end to end without a
//! daemon, a repository, or a vault.
//!
//! Real socket, real framing, real protocol. Mocking below the handler would
//! hide exactly the things worth testing — reply-shape mismatches, error codes
//! surviving the round trip, and what happens when nothing is listening.

#![cfg(test)]

use std::io::Write;
use std::sync::{Arc, Mutex};

use superbackup_core::ipc::testing::MockHandler;
use superbackup_core::ipc::{Limits, Server, ServerHandle, ServerOptions};

use super::args::{ColorChoice, GlobalArgs};
use super::client::Daemon;
use super::context::Ctx;
use super::output::Ui;

// ---------------------------------------------------------------------------
// Capturing output
// ---------------------------------------------------------------------------

type Shared = Arc<Mutex<Vec<u8>>>;

/// A `Write` that appends into a shared buffer.
pub struct Sink(Shared);

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.lock() {
            Ok(mut v) => {
                v.extend_from_slice(buf);
                Ok(buf.len())
            }
            Err(_) => Err(std::io::Error::other("captured output lock was poisoned")),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Both captured streams of one [`Ui`].
#[derive(Clone, Default)]
pub struct Captured {
    out: Shared,
    err: Shared,
}

impl Captured {
    pub fn new() -> Captured {
        Captured::default()
    }
    pub fn out(&self) -> Sink {
        Sink(Arc::clone(&self.out))
    }
    pub fn err(&self) -> Sink {
        Sink(Arc::clone(&self.err))
    }
    pub fn stdout(&self) -> String {
        read(&self.out)
    }
    pub fn stderr(&self) -> String {
        read(&self.err)
    }
    /// Parse stdout as the JSON envelope. Fails loudly if it is not exactly
    /// one document, which is itself the assertion most of these tests want.
    pub fn json(&self) -> serde_json::Value {
        let text = self.stdout();
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("stdout was not one JSON document ({e}):\n{text}"))
    }
}

fn read(shared: &Shared) -> String {
    shared.lock().map(|v| String::from_utf8_lossy(&v).into_owned()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// A daemon that is not a daemon
// ---------------------------------------------------------------------------

/// A unique endpoint per test, so the suite can run in parallel.
///
/// Note that this does *not* come from `Paths::ipc_endpoint`: on Windows that
/// is the fixed pipe name `\\.\pipe\superbackup` regardless of `--home`, so
/// tests that used it would collide with each other and with any real daemon
/// on the developer's machine.
pub fn unique_endpoint(tag: &str) -> String {
    let unique = format!("{}-{}-{}", std::process::id(), tag, uuid::Uuid::new_v4().simple());
    if cfg!(windows) {
        format!(r"\\.\pipe\superbackup-cli-{unique}")
    } else {
        let dir = std::env::temp_dir().join(format!("sb-cli-{unique}"));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("sb.sock").display().to_string()
    }
}

/// A live IPC server backed by [`MockHandler`], plus the runtime driving it.
pub struct Harness {
    pub endpoint: String,
    pub handler: Arc<MockHandler>,
    handle: ServerHandle,
    runtime: Option<tokio::runtime::Runtime>,
    task: Option<tokio::task::JoinHandle<superbackup_core::error::Result<()>>>,
}

impl Harness {
    pub fn start(tag: &str) -> Harness {
        Harness::with_handler(tag, Arc::new(MockHandler::new()))
    }

    pub fn with_handler(tag: &str, handler: Arc<MockHandler>) -> Harness {
        let endpoint = unique_endpoint(tag);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap_or_else(|e| panic!("test runtime: {e}"));
        let options = ServerOptions { limits: Limits::default(), replace_existing: true };
        let server = Server::bind(&endpoint, Arc::clone(&handler), options)
            .unwrap_or_else(|e| panic!("binding {endpoint}: {e}"));
        let handle = server.handle();
        let task = runtime.spawn(server.serve());
        Harness { endpoint, handler, handle, runtime: Some(runtime), task: Some(task) }
    }

    /// A context wired to this server, with captured output.
    pub fn ctx(&self, json: bool) -> (Ctx, Captured) {
        self.ctx_with(json, |_| {})
    }

    pub fn ctx_with(&self, json: bool, tweak: impl FnOnce(&mut GlobalArgs)) -> (Ctx, Captured) {
        let mut global = GlobalArgs {
            json,
            quiet: false,
            verbose: 0,
            no_input: true,
            home: None,
            service: false,
            timeout: 10,
            color: ColorChoice::Never,
        };
        tweak(&mut global);
        let (ui, captured) = Ui::capturing(json);
        let paths = superbackup_core::paths::Paths::rooted_at(
            std::env::temp_dir().join(format!("sb-cli-home-{}", uuid::Uuid::new_v4().simple())),
            false,
        );
        let mut ctx = Ctx::new(global, paths, ui);
        ctx.endpoint_override = Some(self.endpoint.clone());
        (ctx, captured)
    }

    /// Connect the way a command would.
    pub fn daemon(&self, ctx: &mut Ctx) -> Daemon {
        Daemon::connect(ctx, super::client::Start::Never)
            .unwrap_or_else(|e| panic!("connecting to the harness: {}", e.message))
    }
}

/// What one CLI invocation produced.
pub struct RunResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl RunResult {
    /// Parse stdout as the single JSON envelope. Panics if it is not exactly
    /// one document — which is itself the property most of these tests check.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!("stdout was not one JSON document ({e}):\n{}", self.stdout)
        })
    }

    pub fn data(&self) -> serde_json::Value {
        let value = self.json();
        assert_eq!(value["ok"], true, "expected success, got: {}", self.stdout);
        value["data"].clone()
    }

    pub fn error(&self) -> serde_json::Value {
        let value = self.json();
        assert_eq!(value["ok"], false, "expected a failure, got: {}", self.stdout);
        value["error"].clone()
    }
}

/// Run one command exactly as `main.rs` would, against the harness's daemon.
///
/// The argv goes through the real `clap` parser, so a test that passes a flag
/// the parser does not accept fails loudly rather than testing a struct nobody
/// can construct from the command line.
///
/// `--no-input` is forced on: a test that blocked on a prompt would hang the
/// suite, which is the same failure this flag exists to prevent for scripts.
pub fn run(harness: &Harness, argv: &[&str]) -> RunResult {
    use clap::Parser;

    let full: Vec<&str> = std::iter::once("superbackup").chain(argv.iter().copied()).collect();
    let cli = super::args::Cli::try_parse_from(&full)
        .unwrap_or_else(|e| panic!("`{}` must parse: {e}", full.join(" ")));

    let global = cli.global.clone();
    let (mut ctx, captured) = harness.ctx_with(global.json, |g| {
        *g = global.clone();
        g.no_input = true;
        g.color = ColorChoice::Never;
        g.timeout = 10;
    });

    let command = cli.command.unwrap_or_else(|| panic!("`{}` names no command", full.join(" ")));
    let code = match super::commands::dispatch(&mut ctx, command) {
        Ok(outcome) => {
            ctx.ui.finish(&outcome);
            outcome.exit
        }
        Err(error) => {
            ctx.ui.fail(&error);
            error.exit_code()
        }
    };
    RunResult { code, stdout: captured.stdout(), stderr: captured.stderr() }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.handle.shutdown();
        if let (Some(runtime), Some(task)) = (self.runtime.take(), self.task.take()) {
            let _ = runtime.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(5), task).await
            });
            runtime.shutdown_timeout(std::time::Duration::from_secs(2));
        }
    }
}

/// Run one command with nothing listening, for the failure paths that matter
/// most: exit code 3, and the commands that must still work without a daemon.
pub fn run_without_daemon(argv: &[&str]) -> RunResult {
    use clap::Parser;

    let full: Vec<&str> = std::iter::once("superbackup").chain(argv.iter().copied()).collect();
    let cli = super::args::Cli::try_parse_from(&full)
        .unwrap_or_else(|e| panic!("`{}` must parse: {e}", full.join(" ")));

    let (mut ctx, captured) = unreachable_ctx(cli.global.json);
    ctx.global = cli.global.clone();
    ctx.global.no_input = true;
    ctx.global.color = ColorChoice::Never;
    ctx.global.timeout = 2;

    let command = cli.command.unwrap_or_else(|| panic!("`{}` names no command", full.join(" ")));
    let code = match super::commands::dispatch(&mut ctx, command) {
        Ok(outcome) => {
            ctx.ui.finish(&outcome);
            outcome.exit
        }
        Err(error) => {
            ctx.ui.fail(&error);
            error.exit_code()
        }
    };
    RunResult { code, stdout: captured.stdout(), stderr: captured.stderr() }
}

/// A context pointed at an endpoint nothing is listening on.
pub fn unreachable_ctx(json: bool) -> (Ctx, Captured) {
    let global = GlobalArgs {
        json,
        quiet: false,
        verbose: 0,
        no_input: true,
        home: None,
        service: false,
        timeout: 2,
        color: ColorChoice::Never,
    };
    let (ui, captured) = Ui::capturing(json);
    let paths = superbackup_core::paths::Paths::rooted_at(
        std::env::temp_dir().join(format!("sb-cli-dead-{}", uuid::Uuid::new_v4().simple())),
        false,
    );
    let mut ctx = Ctx::new(global, paths, ui);
    ctx.endpoint_override = Some(unique_endpoint("nobody-home"));
    (ctx, captured)
}
