//! The running instance: scheduler, engine, IPC server, and (optionally) tray.
//!
//! ```text
//!   run_foreground
//!     │
//!     ├─ tracing → paths.log_dir(), rolling daily, pruned by retention
//!     ├─ single-instance lock            ── refuse, do not displace
//!     ├─ Store::open / initialise        ── config loaded, vault still locked
//!     ├─ Runtime (Arc)                   ── the shared view of everything
//!     ├─ kopia: ensure_available         ── in the background, with progress
//!     ├─ engine: EngineBuilder::spawn    ── scheduler + runner + executor
//!     ├─ event pump                      ── engine → IPC, log, notifications
//!     ├─ IPC server bound                ── the moment clients may connect
//!     ├─ tray (Surface::Tray only)
//!     └─ wait for Ctrl-C / SIGTERM / control.shutdown, then unwind in reverse
//! ```
//!
//! ## The order is the design
//!
//! **The lock is taken before anything is opened.** Two daemons driving one
//! kopia repository is a data-loss bug, and the cheapest possible moment to
//! discover a second instance is before either has touched a repository.
//!
//! **The IPC server binds after the engine, before the tray.** A client that
//! can connect must find a daemon that can answer, so nothing is served until
//! the scheduler exists; and the tray is the last thing up because it is the
//! only part whose absence is merely cosmetic.
//!
//! **kopia is fetched in the background.** A first run downloads a binary over
//! whatever connection the user has. Blocking start-up on that produces a
//! frozen window with no explanation, which is why
//! [`KopiaInstaller::ensure_available`] is driven by its own task that streams
//! [`InstallProgress`] into the activity log and the status stream, and why a
//! failure leaves the daemon running: everything except starting a backup
//! still works, and the user needs the Settings screen to fix it.
//!
//! **Shutdown unwinds in the reverse order.** Stop accepting, stop the engine,
//! let in-flight runs unwind (or cancel them), flush state, drop the lock.

pub mod dryrun;
pub mod environment;
pub mod events;
pub mod executor;
pub mod handler;
pub mod keychain;
pub mod lifecycle;
pub mod logging;
pub mod rekey;
pub mod runtime;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use superbackup_core::config::Store;
use superbackup_core::engine::{EngineBuilder, SystemClock};
use superbackup_core::ipc::{Server, ServerOptions};
use superbackup_core::kopia::{InstallProgressSink, KopiaInstaller};
use superbackup_core::paths::Paths;
use superbackup_core::platform::{self, InstanceGuard, LockOutcome, Notifier};
use superbackup_core::state::{Event, PersistedState, Severity};
use superbackup_core::{Error, Result};

use self::runtime::Runtime;

/// How long shutdown waits for in-flight runs to unwind after they have been
/// asked to stop.
///
/// The executor's contract is "return within about a second of cancellation",
/// and the runner adds its own grace on top. Ten seconds is generous enough
/// that a well-behaved kopia always finishes killing its child, and short
/// enough that a Windows service does not get killed by the SCM for taking too
/// long.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Whether this instance shows a tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// Tray icon present; the GUI can be opened from it.
    Tray,
    /// No tray. Used by `--no-tray` and by the service host.
    Headless,
}

impl Surface {
    pub fn has_tray(self) -> bool {
        self == Surface::Tray
    }
}

/// Start the scheduler, the engine and the IPC server, and block until exit.
pub fn run_foreground(paths: Paths, global: &crate::cli::GlobalArgs, surface: Surface) -> ExitCode {
    let control = logging::init(&paths, global.quiet);
    tracing::info!(
        version = superbackup_core::VERSION,
        endpoint = %paths.ipc_endpoint(),
        ?surface,
        "superbackup starting"
    );

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("superbackup: could not start the async runtime: {e}");
            return ExitCode::from(crate::cli::exit::FAILED as u8);
        }
    };

    let verbosity = global.verbose;
    match runtime.block_on(run(paths, surface, control, verbosity, None)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "superbackup stopped");
            eprintln!("superbackup: {e}");
            if let Some(hint) = e.hint() {
                eprintln!("  {hint}");
            }
            ExitCode::from(exit_code_for(&e))
        }
    }
}

fn exit_code_for(error: &Error) -> u8 {
    match error {
        Error::Locked | Error::BadPassphrase => crate::cli::exit::LOCKED as u8,
        Error::Validation(_) => crate::cli::exit::USAGE as u8,
        _ => crate::cli::exit::FAILED as u8,
    }
}

/// Everything a caller may inject, so the service host and the tests can drive
/// the same start-up path the tray does.
#[derive(Default)]
pub struct Hooks {
    /// Signalled once the IPC server is listening and the engine is up.
    pub ready: Option<tokio::sync::oneshot::Sender<Arc<Runtime>>>,
    /// An extra reason to stop, alongside Ctrl-C and `control.shutdown`.
    pub external_stop: Option<tokio::sync::oneshot::Receiver<()>>,
    /// Listen here instead of at [`Paths::ipc_endpoint`].
    ///
    /// For the integration tests. `ipc_endpoint()` derives its name from the
    /// configuration root, so private homes already do not collide — but a
    /// test must not depend on a core detail the daemon does not own, and must
    /// never be reachable by the developer's own running instance. This pins
    /// the endpoint to a name unique to one test in one process. Production
    /// never sets it.
    pub endpoint: Option<String>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks")
            .field("ready", &self.ready.is_some())
            .field("external_stop", &self.external_stop.is_some())
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

/// The daemon proper. Returns when it has shut down cleanly.
pub async fn run(
    paths: Paths,
    surface: Surface,
    control: logging::LogControl,
    verbosity: u8,
    hooks: Option<Hooks>,
) -> Result<()> {
    let mut hooks = hooks.unwrap_or_default();
    paths.ensure()?;

    // 1. Single instance. Before anything is opened, and never by displacing
    //    whoever is already there.
    let guard = acquire_instance(&paths)?;

    // 2. Configuration and vault. The vault stays locked.
    let store = open_store(&paths)?;
    let settings = store.config().settings.clone();
    control.set_level(settings.log_level, verbosity);
    control.prune(settings.log_retention_days);

    let persisted = load_state(&paths);

    // 3. Shared state.
    let environment = Arc::new(environment::DaemonEnvironment::new());
    environment.sample();
    let notifier = Arc::new(match surface {
        // Session 0 has no desktop, so a toast raised there is invisible by
        // design. The tray raises it instead, over IPC.
        Surface::Headless if paths.service_scope => {
            Notifier::log_only(settings.notifications.clone())
        }
        _ => Notifier::new(settings.notifications.clone()),
    });
    let runtime = Runtime::new(
        paths.clone(),
        surface,
        store,
        persisted,
        Arc::clone(&environment),
        Arc::clone(&notifier),
    );
    events::spawn_event_log(Arc::clone(&runtime));
    environment.spawn_sampler();

    runtime.record_event(Event::info(
        "daemon.started",
        format!(
            "superbackup {} started ({}).",
            superbackup_core::VERSION,
            if surface.has_tray() { "tray" } else { "headless" }
        ),
    ));
    if let Some(warning) = notifier.platform_warning() {
        runtime.record_event(Event::new(
            Severity::Warning,
            "notify.limited",
            warning.to_string(),
        ));
    }
    if settings.use_os_keychain {
        if !keychain::available() {
            runtime.record_event(Event::new(
                Severity::Warning,
                "vault.keychain_unavailable",
                keychain::explain_unavailable().to_string(),
            ));
        } else if !keychain::has_local(&paths) {
            // Distinguishing "nothing saved yet" from "saved but unusable"
            // matters: the first needs one manual unlock and then resolves
            // itself, the second needs the user to do something.
            runtime.record_event(Event::info(
                "vault.keychain_empty",
                "Nothing is saved in this machine's credential store yet. Unlock superbackup \
                 once and your passphrase will be remembered from then on.",
            ));
        }
    }

    // 4. A service can reach less than a tray can, and the user must be told
    //    at start-up rather than three days later when their OneDrive backup
    //    has never once run.
    if paths.service_scope {
        let config = { runtime.store.lock().await.config().clone() };
        record_destination_report(
            &runtime,
            &config,
            &platform::service::ServiceAccount::LocalSystem,
            platform::ServiceScope::System,
        );
    }

    // 5. A rotation that did not finish leaves destinations suppressed and the
    //    user told, rather than a schedule full of password failures.
    rekey::restore_after_restart(&runtime).await;

    // 6. kopia, in the background. A failure here must not stop the daemon.
    let kopia_task = spawn_kopia_setup(Arc::clone(&runtime));

    // 7. The engine.
    let clock = Arc::new(SystemClock::new());
    let executor = Arc::new(executor::KopiaExecutor::new(Arc::clone(&runtime), clock.clone()));
    let config = { runtime.store.lock().await.config().clone() };
    let scheduler = EngineBuilder::new(runtime.effective_config(&config), executor)
        .clock(clock)
        .environment(Arc::clone(&environment) as Arc<dyn superbackup_core::engine::Environment>)
        .state(Arc::clone(&runtime.persisted))
        .spawn();
    events::pump_engine_events(Arc::clone(&runtime), scheduler.subscribe());
    runtime.set_scheduler(scheduler.clone());
    lifecycle::spawn_auto_lock(Arc::clone(&runtime));

    // 8. An unattended machine unlocks itself here, when the user asked for it.
    lifecycle::try_keychain_unlock(&runtime).await;

    // 9. IPC. Clients may connect from this line onwards.
    let handler = Arc::new(handler::DaemonHandler::new(Arc::clone(&runtime)));
    let endpoint = hooks.endpoint.take().unwrap_or_else(|| paths.ipc_endpoint());
    let server = Server::bind(&endpoint, handler, ServerOptions::default())?;
    let server_handle = server.handle();
    let serving = tokio::spawn(server.serve());

    // 10. The tray, last, because its absence is only cosmetic.
    let tray = if surface.has_tray() {
        match crate::tray::spawn(Arc::clone(&runtime)) {
            Ok(tray) => Some(tray),
            Err(e) => {
                // A missing status area, a headless X server, a Windows
                // session with no shell: none is a reason to stop backing up.
                tracing::warn!(error = %e, "the tray icon could not be shown");
                runtime.record_event(Event::new(
                    Severity::Warning,
                    "tray.unavailable",
                    format!("The tray icon could not be shown: {e} Backups still run."),
                ));
                None
            }
        }
    } else {
        None
    };

    if let Some(ready) = hooks.ready.take() {
        let _ = ready.send(Arc::clone(&runtime));
    }
    runtime.publish_status().await;
    tracing::info!("superbackup is ready");

    // 11. Wait.
    let stop_runs = wait_for_stop(&runtime, hooks.external_stop.take()).await;

    // 12. Unwind, in reverse.
    tracing::info!(stop_runs, "superbackup is shutting down");
    server_handle.shutdown();
    if let Some(tray) = tray {
        tray.shutdown();
    }
    finish_runs(&runtime, &scheduler, stop_runs).await;
    scheduler.shutdown();
    kopia_task.abort();

    // The server's own `serve` returns once its last connection closes.
    match tokio::time::timeout(SHUTDOWN_GRACE, serving).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => tracing::warn!(error = %e, "the IPC server stopped with an error"),
        Ok(Err(e)) => tracing::warn!(error = %e, "the IPC server task failed"),
        Err(_) => tracing::warn!("the IPC server did not stop within the grace period"),
    }

    save_state(&runtime).await;
    runtime.forget_master();
    runtime.record_event(Event::info("daemon.stopped", "superbackup stopped."));
    // Give the event-log writer a moment to drain the last line before the
    // process exits; it is an unbounded channel, not a synchronous write.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // The lock is released last, so nothing can start a second daemon while
    // this one is still flushing.
    drop(guard);
    tracing::info!("superbackup stopped");
    Ok(())
}

// ---------------------------------------------------------------------------
// Start-up steps
// ---------------------------------------------------------------------------

/// Become the only instance, or explain who is already there.
///
/// Deliberately refuses rather than taking over: the lock exists because two
/// daemons driving one repository can corrupt it, and "probably stale" is not
/// a good enough reason to risk that. `platform::single_instance` already
/// distinguishes a genuinely dead holder (whose lock it does take over) from a
/// live one, so reaching the error arm means a real second instance.
fn acquire_instance(paths: &Paths) -> Result<InstanceGuard> {
    match platform::single_instance::acquire(paths)? {
        LockOutcome::Acquired(guard) => Ok(guard),
        LockOutcome::AlreadyRunning(record) => Err(Error::Validation(format!(
            "superbackup is already running. {}",
            record.describe()
        ))),
    }
}

/// Open the store, creating nothing.
///
/// A missing vault is a first run, and a first run needs a passphrase that
/// only a human can supply — so it is reported as the actionable error it is
/// rather than papered over with an empty vault, which would strand every
/// existing repository.
fn open_store(paths: &Paths) -> Result<Store> {
    match Store::open_for_repair(paths.clone()) {
        Ok((store, report)) => {
            for issue in &report.warnings {
                tracing::warn!("configuration warning: {issue}");
            }
            for issue in &report.errors {
                // Opened for repair on purpose: a configuration with a
                // dangling id cannot run, but the user must still be able to
                // reach the editor and fix it. The scheduler sees the same
                // config and simply finds no usable destination.
                tracing::error!("configuration error: {issue}");
            }
            Ok(store)
        }
        Err(e) => Err(e),
    }
}

/// Load `state.json`, tolerating a missing or unreadable one.
///
/// Run history is a convenience, not the backups themselves. Refusing to start
/// because it could not be parsed would turn a cosmetic problem into an outage.
fn load_state(paths: &Paths) -> PersistedState {
    let path = paths.state_file();
    let Ok(bytes) = std::fs::read(&path) else {
        return PersistedState::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(state) => state,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "run history could not be read");
            // Keep the unreadable file rather than overwriting it: it may be
            // the only record of what happened, and someone may want it.
            let _ = std::fs::rename(&path, path.with_extension("json.unreadable"));
            PersistedState::default()
        }
    }
}

/// Persist run history atomically.
async fn save_state(runtime: &Arc<Runtime>) {
    let state = { runtime.persisted.lock().await.clone() };
    match serde_json::to_vec_pretty(&state) {
        Ok(bytes) => {
            if let Err(e) =
                superbackup_core::paths::write_atomic(&runtime.paths.state_file(), &bytes)
            {
                tracing::error!(error = %e, "run history could not be saved");
            }
        }
        Err(e) => tracing::error!(error = %e, "run history could not be serialised"),
    }
}

/// Find kopia, fetching it if this is a first run.
///
/// On its own task, with progress, because the first run downloads a binary
/// over whatever connection the user has. A frozen window with no explanation
/// is the failure this avoids; "superbackup is fetching kopia — 12 MB of 31 MB"
/// is what replaces it.
fn spawn_kopia_setup(runtime: Arc<Runtime>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (settings, paths) = {
            let store = runtime.store.lock().await;
            (store.config().settings.clone(), runtime.paths.clone())
        };
        let installer = match KopiaInstaller::new(&paths) {
            Ok(i) => i,
            Err(e) => {
                report_kopia_failure(&runtime, &e.message());
                return;
            }
        };

        let (sink, mut progress) = InstallProgressSink::channel(16);
        let reporter = Arc::clone(&runtime);
        let pump = tokio::spawn(async move {
            let mut announced = false;
            while let Some(update) = progress.recv().await {
                if !announced {
                    announced = true;
                    reporter.record_event(Event::info(
                        "kopia.installing",
                        "superbackup is fetching kopia, the engine it uses to store backups.",
                    ));
                }
                // On the status stream rather than the activity log: this
                // fires several times a second, and the log is for things a
                // human reads later.
                reporter.publish(superbackup_core::ipc::StreamItem::Event {
                    event: Box::new(
                        Event::new(
                            Severity::Debug,
                            "kopia.install_progress",
                            match (update.fraction(), update.total_bytes) {
                                (Some(f), Some(_)) => format!(
                                    "{}: {:.0}%",
                                    update.phase.title(),
                                    f * 100.0
                                ),
                                _ => update.phase.title().to_string(),
                            },
                        )
                        .with_field("phase", format!("{:?}", update.phase))
                        .with_field("downloaded_bytes", update.downloaded_bytes),
                    ),
                });
            }
        });

        let outcome = installer.ensure_available(&settings, &paths, Some(&sink)).await;
        drop(sink);
        let _ = pump.await;

        match outcome {
            Ok(binary) => {
                let version = binary.version().to_string();
                runtime.set_kopia(Some(binary));
                runtime.record_event(Event::info(
                    "kopia.ready",
                    format!("kopia {version} is ready."),
                ));
                runtime.publish_status().await;
            }
            Err(e) => report_kopia_failure(&runtime, &e.message()),
        }
    })
}

/// A missing kopia is loud but not fatal: everything except starting a backup
/// still works, and the user needs the app running to fix it.
fn report_kopia_failure(runtime: &Arc<Runtime>, message: &str) {
    tracing::error!(%message, "kopia is not available");
    runtime.record_event(Event::new(
        Severity::Error,
        "kopia.missing",
        format!(
            "kopia is not available, so backups cannot run: {message} Everything else in \
             superbackup still works — open Settings → Kopia binary to fix it."
        ),
    ));
}

/// One line per destination a service running under `account` cannot fully
/// reach.
///
/// Pure, so the honesty is testable: the answer comes from
/// [`platform::service::destination_support`], which is itself pure, rather
/// than from a guess made here. A LocalSystem service has no user profile, so
/// it cannot see OneDrive, cannot see a mapped drive letter, and usually
/// cannot reach a UNC share — and the user has to be told that at start-up
/// rather than three days later.
pub fn destination_report(
    config: &superbackup_core::model::Config,
    account: &platform::service::ServiceAccount,
    scope: platform::ServiceScope,
) -> Vec<String> {
    use platform::service::SupportLevel;
    let mut out = Vec::new();
    for destination in &config.destinations {
        match platform::service::destination_support(&destination.kind, account, scope) {
            SupportLevel::Supported => {}
            SupportLevel::Degraded { reason } => out.push(format!(
                "\"{}\" works from the service with a caveat: {reason}",
                destination.name
            )),
            SupportLevel::Unsupported { reason } => out.push(format!(
                "\"{}\" CANNOT be backed up by this service: {reason}",
                destination.name
            )),
        }
    }
    out
}

/// [`destination_report`] as activity-log events, for a running daemon.
pub fn record_destination_report(
    runtime: &Arc<Runtime>,
    config: &superbackup_core::model::Config,
    account: &platform::service::ServiceAccount,
    scope: platform::ServiceScope,
) {
    for line in destination_report(config, account, scope) {
        let severity = if line.contains("CANNOT") { Severity::Error } else { Severity::Warning };
        runtime.record_event(Event::new(severity, "service.destination_reach", line));
    }
}

// ---------------------------------------------------------------------------
// Shutdown
// ---------------------------------------------------------------------------

/// Block until something asks the daemon to stop. Returns whether in-flight
/// runs should be cancelled rather than allowed to finish.
async fn wait_for_stop(
    runtime: &Arc<Runtime>,
    external: Option<tokio::sync::oneshot::Receiver<()>>,
) -> bool {
    let mut requests = runtime.subscribe_shutdown();
    let external = async move {
        match external {
            Some(rx) => {
                let _ = rx.await;
            }
            None => std::future::pending().await,
        }
    };

    tokio::select! {
        request = requests.recv() => request.map(|r| r.stop_runs).unwrap_or(true),
        _ = ctrl_c() => {
            // Ctrl-C from a terminal is a person waiting for a prompt, so it
            // stops runs rather than waiting for a two-hour backup.
            tracing::info!("interrupt received");
            true
        }
        _ = terminate() => {
            // SIGTERM is a service manager with a timeout, and the same
            // reasoning applies with more urgency.
            tracing::info!("termination signal received");
            true
        }
        _ = external => {
            tracing::info!("the service control handler asked us to stop");
            true
        }
    }
}

async fn ctrl_c() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::warn!(error = %e, "the interrupt handler could not be installed");
        std::future::pending::<()>().await;
    }
}

/// SIGTERM on unix; never fires on Windows, where the SCM control handler and
/// Ctrl-C cover the same ground.
async fn terminate() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "SIGTERM handler could not be installed");
                std::future::pending::<()>().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        std::future::pending::<()>().await;
    }
}

/// Let in-flight runs finish, or stop them cleanly.
///
/// "Cleanly" is the whole point: cancelling through the engine's token means
/// the executor kills its kopia child and waits for the reap *before* the run
/// unwinds, so no repository is left locked by a process that no longer
/// exists. Killing the daemon instead would leave exactly that.
async fn finish_runs(
    runtime: &Arc<Runtime>,
    scheduler: &superbackup_core::engine::SchedulerHandle,
    stop_runs: bool,
) {
    let active = runtime.active_runs();
    if active.is_empty() {
        return;
    }
    if stop_runs {
        tracing::info!(count = active.len(), "stopping in-flight backups");
        for run in &active {
            if let Err(e) = scheduler.cancel_job(run.job_id) {
                tracing::warn!(error = %e, "could not stop a run during shutdown");
            }
        }
    } else {
        tracing::info!(count = active.len(), "waiting for in-flight backups to finish");
    }

    let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
    while !runtime.active_runs().is_empty() {
        if tokio::time::Instant::now() >= deadline {
            let remaining: Vec<String> =
                runtime.active_runs().iter().map(|r| r.job_name.clone()).collect();
            tracing::warn!(
                jobs = ?remaining,
                "backups were still running at the end of the shutdown grace period"
            );
            // Falling through with a run still going is the least bad option:
            // the engine token has fired, so the executor is already killing
            // its child, and holding the process open indefinitely would get
            // the service killed by the SCM instead.
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surface_knows_whether_it_has_a_tray() {
        assert!(Surface::Tray.has_tray());
        assert!(!Surface::Headless.has_tray());
    }

    #[test]
    fn a_locked_vault_maps_onto_the_locked_exit_code() {
        assert_eq!(exit_code_for(&Error::Locked), crate::cli::exit::LOCKED as u8);
        assert_eq!(
            exit_code_for(&Error::Validation("x".into())),
            crate::cli::exit::USAGE as u8
        );
        assert_eq!(
            exit_code_for(&Error::Internal("x".into())),
            crate::cli::exit::FAILED as u8
        );
    }

    #[test]
    fn unreadable_run_history_is_preserved_rather_than_overwritten() {
        let root = std::env::temp_dir().join(format!("sb-state-{}", uuid::Uuid::new_v4()));
        let paths = Paths::rooted_at(&root, false);
        paths.ensure().expect("dirs");
        std::fs::write(paths.state_file(), b"{ not json").expect("seed");

        let state = load_state(&paths);
        assert!(state.history.is_empty());
        assert!(
            paths.state_file().with_extension("json.unreadable").exists(),
            "the unreadable file must be kept, not deleted"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_state_file_is_a_first_run_not_an_error() {
        let root = std::env::temp_dir().join(format!("sb-state-{}", uuid::Uuid::new_v4()));
        let paths = Paths::rooted_at(&root, false);
        paths.ensure().expect("dirs");
        assert!(load_state(&paths).history.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
