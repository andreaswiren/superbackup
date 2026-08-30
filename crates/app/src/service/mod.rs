//! OS service entry points: the Windows SCM, systemd, and launchd.
//!
//! ```text
//!   Windows   SCM ──▶ service_dispatcher ──▶ ServiceEvents ──▶ daemon::run
//!   systemd   exec ──▶ (Type=notify-less)  ──▶ SIGTERM      ──▶ daemon::run
//!   launchd   exec ──▶                      ──▶ SIGTERM      ──▶ daemon::run
//! ```
//!
//! ## One body, three front doors
//!
//! Everything below funnels into the same [`daemon::run`] the tray uses, with
//! two differences and only two: [`Surface::Headless`], and a
//! [`Notifier::log_only`] when the process is in Session 0. Keeping the bodies
//! identical is what stops "it works from the tray but not as a service" —
//! the failure mode that makes a backup service untrustworthy — from being
//! possible at all.
//!
//! ## Session 0
//!
//! A Windows service always runs in session 0, which is isolated from every
//! interactive desktop. A toast raised there is invisible, a message box hangs
//! forever, and a passphrase prompt can never be answered. So the service:
//!
//! * shows no tray and no window,
//! * uses a notifier that only writes to the log — the tray app raises the
//!   toast on the service's behalf, over IPC,
//! * never blocks on anything a human would have to answer.
//!
//! [`daemon::run`] already arranges the notifier from
//! `Surface::Headless` plus `paths.service_scope`; this module's job is the
//! dispatcher, the control handler, and telling the truth about what the
//! service can reach.
//!
//! ## What a LocalSystem service cannot reach
//!
//! This is the part users find out about at the worst possible moment, so
//! [`destination_report`] states it at start-up instead: a service running as
//! Local System has no user profile, so it cannot see OneDrive, cannot see a
//! mapped drive letter, and usually cannot reach a UNC share. The report goes
//! into the activity log every time the service starts, and
//! `service.install` returns the same sentence to whoever asked for the
//! install.

use std::process::ExitCode;
use std::sync::Arc;

use superbackup_core::config::ConfigStore;
use superbackup_core::model::Config;
use superbackup_core::paths::Paths;
use superbackup_core::platform::service::{
    destination_support, ServiceAccount, ServiceScope, SupportLevel,
};
use superbackup_core::state::{Event, Severity};

use crate::daemon::{self, Surface};

/// Entry point invoked by the operating system's service manager.
pub fn run_as_service(paths: Paths, global: &crate::cli::GlobalArgs) -> ExitCode {
    #[cfg(windows)]
    {
        windows_service(paths, global)
    }
    #[cfg(not(windows))]
    {
        // systemd and launchd both `exec` the binary and signal it with
        // SIGTERM, which `daemon::run` already handles — so a unit file that
        // runs `superbackup service run` behaves identically to one that runs
        // `superbackup daemon --no-tray`. The entry point exists so that both
        // spellings work and so `ExecStart` can name the documented one.
        unix_service(paths, global)
    }
}

/// The systemd / launchd path: run headless and wait for a signal.
#[cfg(not(windows))]
fn unix_service(paths: Paths, global: &crate::cli::GlobalArgs) -> ExitCode {
    announce(&paths);
    daemon::run_foreground(paths, global, Surface::Headless)
}

/// The Windows path: hand control to the SCM, then run the daemon inside the
/// service's worker.
///
/// [`platform::service::dispatch`] blocks until the service stops and gives
/// the worker a [`ServiceEvents`] channel carrying Stop, Shutdown, Suspend,
/// Resume and session changes. Stop and Shutdown are wired to the daemon's own
/// shutdown; the others are logged, because the scheduler's clock-gap
/// detection already handles a machine that slept and reacting twice would
/// double every catch-up run.
#[cfg(windows)]
fn windows_service(paths: Paths, global: &crate::cli::GlobalArgs) -> ExitCode {
    use superbackup_core::platform::service::{self as svc, ServiceSignal};

    let verbosity = global.verbose;
    let quiet = global.quiet;
    let service_paths = paths.clone();

    let worker: svc::ServiceWorker = Box::new(move |events: svc::ServiceEvents| {
        let control = daemon::logging::init(&service_paths, true);
        let _ = quiet;
        tracing::info!("the superbackup service is starting");
        announce(&service_paths);

        let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!(error = %e, "the service could not start its async runtime");
                return;
            }
        };

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        // The SCM delivers control messages on its own thread, so they are
        // read here and forwarded once. A blocking `recv` on a tokio worker
        // would hold one for the life of the service.
        std::thread::Builder::new()
            .name("superbackup-scm".into())
            .spawn(move || {
                let mut stop = Some(stop_tx);
                while let Some(signal) = events.recv() {
                    match signal {
                        ServiceSignal::Stop | ServiceSignal::Shutdown => {
                            tracing::info!(?signal, "the service was asked to stop");
                            if let Some(tx) = stop.take() {
                                let _ = tx.send(());
                            }
                            return;
                        }
                        // The engine infers sleep from a clock gap on every
                        // platform, so acting on these as well would run
                        // catch-up twice.
                        other => tracing::info!(?other, "service control message"),
                    }
                }
            })
            .ok();

        let hooks = daemon::Hooks { ready: None, external_stop: Some(stop_rx) };
        let result = runtime.block_on(daemon::run(
            service_paths,
            Surface::Headless,
            control,
            verbosity,
            Some(hooks),
        ));
        if let Err(e) = result {
            tracing::error!(error = %e, "the superbackup service stopped with an error");
        }
    });

    match svc::dispatch(superbackup_core::platform::service::DEFAULT_SERVICE_NAME, worker) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Reaching here means the process was started by hand rather than
            // by the SCM, which is a common mistake and deserves the sentence
            // rather than a stack trace.
            eprintln!("superbackup: {e}");
            ExitCode::from(crate::cli::exit::USAGE as u8)
        }
    }
}

/// Write the "what this service can and cannot reach" report to the log.
///
/// Deliberately at start-up rather than only at install time: a destination
/// added later, or a service reinstalled under a different account, changes
/// the answer, and the log is where someone diagnosing "my OneDrive backup
/// never runs" will look.
fn announce(paths: &Paths) {
    let (config, _) = match ConfigStore::new(paths.clone()).load_lenient() {
        Ok((config, outcome, _)) => (config, outcome),
        Err(e) => {
            tracing::warn!(error = %e, "the service could not read the configuration to report on it");
            return;
        }
    };
    for line in destination_report(&config, &ServiceAccount::LocalSystem, ServiceScope::System) {
        tracing::warn!("{line}");
    }
}

/// One line per destination the service cannot fully reach.
///
/// Pure, so the honesty is testable: the answer comes from
/// [`platform::service::destination_support`], which is itself pure, rather
/// than from a guess made here.
pub fn destination_report(
    config: &Config,
    account: &ServiceAccount,
    scope: ServiceScope,
) -> Vec<String> {
    let mut out = Vec::new();
    for destination in &config.destinations {
        match destination_support(&destination.kind, account, scope) {
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

/// The same report, as activity-log events, for a running daemon.
pub fn record_destination_report(
    runtime: &Arc<crate::daemon::runtime::Runtime>,
    config: &Config,
    account: &ServiceAccount,
    scope: ServiceScope,
) {
    for line in destination_report(config, account, scope) {
        let severity =
            if line.contains("CANNOT") { Severity::Error } else { Severity::Warning };
        runtime.record_event(Event::new(severity, "service.destination_reach", line));
    }
}

/// Whether a destination is worth offering in a service-mode configuration.
pub fn usable_under_service(
    destination: &superbackup_core::model::Destination,
    account: &ServiceAccount,
    scope: ServiceScope,
) -> bool {
    destination_support(&destination.kind, account, scope).is_usable()
}

#[cfg(test)]
mod tests {
    use super::*;
    use superbackup_core::engine::testing::{test_mirror, test_repository};

    fn config_with(destinations: Vec<superbackup_core::model::Destination>) -> Config {
        Config { destinations, ..Config::default() }
    }

    #[test]
    fn a_local_system_service_is_told_it_cannot_reach_onedrive() {
        let mut onedrive = test_repository("cloud", r"C:\Users\me\OneDrive\backups");
        onedrive.kind = superbackup_core::model::DestinationKind::OneDrive {
            path: r"C:\Users\me\OneDrive\backups".into(),
            account: None,
        };
        let report = destination_report(
            &config_with(vec![onedrive]),
            &ServiceAccount::LocalSystem,
            ServiceScope::System,
        );
        assert_eq!(report.len(), 1);
        assert!(report[0].contains("CANNOT"), "{}", report[0]);
        assert!(report[0].contains("cloud"));
    }

    #[test]
    fn a_service_running_as_the_user_reaches_onedrive_with_a_caveat() {
        let mut onedrive = test_repository("cloud", r"C:\Users\me\OneDrive\backups");
        onedrive.kind = superbackup_core::model::DestinationKind::OneDrive {
            path: r"C:\Users\me\OneDrive\backups".into(),
            account: None,
        };
        let account =
            ServiceAccount::User { username: r".\me".into(), password: None };
        let report =
            destination_report(&config_with(vec![onedrive]), &account, ServiceScope::System);
        assert_eq!(report.len(), 1);
        assert!(!report[0].contains("CANNOT"), "{}", report[0]);
        assert!(report[0].contains("caveat"));
    }

    #[test]
    fn a_mapped_drive_is_reported_as_unreachable_on_windows() {
        let mirror = test_mirror("H drive", r"H:\backups");
        let report = destination_report(
            &config_with(vec![mirror]),
            &ServiceAccount::LocalSystem,
            ServiceScope::System,
        );
        if cfg!(windows) {
            assert_eq!(report.len(), 1);
            assert!(report[0].contains("CANNOT"));
        }
    }

    #[test]
    fn an_ordinary_local_folder_produces_no_noise() {
        let repo = test_repository("disk", r"D:\backups");
        assert!(destination_report(
            &config_with(vec![repo]),
            &ServiceAccount::LocalSystem,
            ServiceScope::System
        )
        .is_empty());
    }

    #[test]
    fn a_user_scoped_unit_reaches_everything_the_user_can() {
        let repo = test_repository("disk", "/home/me/backups");
        assert!(usable_under_service(&repo, &ServiceAccount::LocalSystem, ServiceScope::User));
    }
}
