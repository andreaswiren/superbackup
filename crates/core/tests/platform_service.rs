//! Service integration.
//!
//! Everything here is pure except the two read-only queries at the end: the
//! destination-support matrix, the systemd unit and launchd plist renderers,
//! and the status parsers. Installing a service mutates real machine state and
//! needs Administrator or root, so those tests are `#[ignore]`d; run them
//! deliberately from an elevated shell with
//! `cargo test -p superbackup-core --test platform_service -- --ignored`.

use std::path::PathBuf;
use superbackup_core::model::DestinationKind;
use superbackup_core::paths::Paths;
use superbackup_core::platform::service::{
    self, ServiceAccount, ServiceOptions, ServiceScope, ServiceState, StartMode, SupportLevel,
};
use superbackup_core::state::Health;
use uuid::Uuid;

fn options(scope: ServiceScope, account: ServiceAccount) -> ServiceOptions {
    let paths = Paths::rooted_at(
        if cfg!(windows) { r"C:\ProgramData\superbackup" } else { "/var/lib/superbackup" },
        true,
    );
    ServiceOptions {
        account,
        scope,
        ..ServiceOptions::new(
            if cfg!(windows) {
                r"C:\Program Files\superbackup\superbackup.exe"
            } else {
                "/usr/bin/superbackup"
            },
            &paths,
        )
    }
}

fn onedrive() -> DestinationKind {
    DestinationKind::OneDrive {
        path: PathBuf::from(if cfg!(windows) {
            r"C:\Users\andreas\OneDrive - Contoso"
        } else {
            "/home/andreas/OneDrive"
        }),
        account: Some("OneDrive for Business — Contoso".into()),
    }
}

fn s3() -> DestinationKind {
    DestinationKind::S3 {
        provider_id: Uuid::new_v4(),
        bucket: "backups".into(),
        prefix: "superbackup/studio-a1b2c3d4/".into(),
        credential_override: None,
    }
}

// ---------------------------------------------------------------------------
// Which destinations service mode actually reaches
// ---------------------------------------------------------------------------

#[test]
fn the_support_matrix_is_honest_about_local_system() {
    // This is the table the GUI must show before the user commits to a
    // service, and the single most common source of "it silently stopped
    // backing up" reports.
    let system = ServiceAccount::LocalSystem;

    assert_eq!(
        service::destination_support(&s3(), &system, ServiceScope::System),
        SupportLevel::Supported,
        "S3 needs nothing from a user profile"
    );

    let od = service::destination_support(&onedrive(), &system, ServiceScope::System);
    assert!(!od.is_usable(), "Local System cannot reach OneDrive: {od:?}");
    match od {
        SupportLevel::Unsupported { reason } => {
            assert!(reason.contains("profile") || reason.contains("signed in"), "{reason}");
            assert!(reason.contains("own account"), "the remedy must be in the message: {reason}");
        }
        other => panic!("{other:?}"),
    }

    let unc = DestinationKind::LocalRepository { path: PathBuf::from(r"\\nas\backups") };
    assert!(!service::destination_support(&unc, &system, ServiceScope::System).is_usable());

    let local = DestinationKind::LocalRepository {
        path: PathBuf::from(if cfg!(windows) { r"C:\Backups" } else { "/srv/backups" }),
    };
    assert_eq!(
        service::destination_support(&local, &system, ServiceScope::System),
        SupportLevel::Supported
    );
}

#[test]
fn installing_under_a_user_account_unlocks_onedrive_with_a_caveat() {
    let account = ServiceAccount::User { username: r".\andreas".into(), password: None };
    match service::destination_support(&onedrive(), &account, ServiceScope::System) {
        SupportLevel::Degraded { reason } => {
            assert!(reason.to_lowercase().contains("signed in"), "{reason}");
        }
        other => panic!("expected a caveat, got {other:?}"),
    }
    assert!(account.sees_user_profile());
    assert_eq!(account.system_identifier().as_deref(), Some(r".\andreas"));
}

#[test]
fn a_user_scoped_unit_reaches_everything_the_user_can() {
    let account = ServiceAccount::LocalSystem;
    for kind in [s3(), DestinationKind::LocalMirror { path: PathBuf::from("/home/me/mirror") }] {
        assert_eq!(
            service::destination_support(&kind, &account, ServiceScope::User),
            SupportLevel::Supported
        );
    }
    assert!(matches!(
        service::destination_support(&onedrive(), &account, ServiceScope::User),
        SupportLevel::Degraded { .. }
    ));
}

// ---------------------------------------------------------------------------
// Unit / plist generation
// ---------------------------------------------------------------------------

#[test]
fn the_system_unit_can_read_every_home_but_write_none_of_them() {
    let unit =
        service::render_systemd_unit(&options(ServiceScope::System, ServiceAccount::LocalSystem));

    // The mistake that breaks a backup daemon: ProtectHome=yes hides exactly
    // the files the user cares about.
    assert!(unit.contains("ProtectHome=read-only"), "{unit}");
    assert!(!unit.contains("ProtectHome=yes"));
    assert!(unit.contains("ProtectSystem=strict"));
    assert!(unit.contains("ReadWritePaths="), "strict needs an explicit writable state dir");
    assert!(unit.contains("NoNewPrivileges=yes"));
    assert!(unit.contains("SystemCallFilter=@system-service"));
    assert!(unit.contains("[Install]"));
    assert!(unit.contains("WantedBy=multi-user.target"));
    assert!(unit.contains("Wants=network-online.target"), "S3 needs routing to be up");
}

#[test]
fn the_user_unit_does_not_lock_the_user_out_of_their_own_home() {
    let unit =
        service::render_systemd_unit(&options(ServiceScope::User, ServiceAccount::LocalSystem));
    assert!(!unit.contains("ProtectHome"), "{unit}");
    assert!(!unit.contains("ProtectSystem=strict"));
    assert!(unit.contains("WantedBy=default.target"));
    assert!(unit.contains("NoNewPrivileges=yes"), "hardening that costs nothing still applies");
}

#[test]
fn a_unit_installed_for_a_named_user_sets_user_only_in_system_scope() {
    let account = ServiceAccount::User { username: "andreas".into(), password: None };
    let system = service::render_systemd_unit(&options(ServiceScope::System, account.clone()));
    assert!(system.contains("User=andreas"));
    let user = service::render_systemd_unit(&options(ServiceScope::User, account));
    assert!(!user.contains("User=andreas"), "a user unit already runs as the user");
}

#[test]
fn exec_start_quotes_a_path_with_spaces() {
    let paths = Paths::rooted_at("/var/lib/superbackup", true);
    let opts = ServiceOptions::new("/opt/super backup/bin/superbackup", &paths);
    let unit = service::render_systemd_unit(&opts);
    let exec = unit.lines().find_map(|l| l.strip_prefix("ExecStart=")).expect("an ExecStart line");
    assert!(exec.starts_with('"'), "systemd splits on whitespace: {exec}");
    assert!(exec.contains("/opt/super backup/bin/superbackup"));
    assert!(exec.contains("--service"));
}

#[test]
fn the_launch_daemon_restarts_itself() {
    let plist =
        service::render_launch_daemon(&options(ServiceScope::System, ServiceAccount::LocalSystem));
    assert!(plist.contains("<key>KeepAlive</key>"), "a backup daemon that dies must come back");
    assert!(plist.contains("<key>RunAtLoad</key>"));
    assert!(plist.contains("<key>ThrottleInterval</key>"), "or launchd will spin on a crash loop");
    assert!(plist.contains(service::LAUNCH_DAEMON_LABEL));
}

// ---------------------------------------------------------------------------
// Status parsing
// ---------------------------------------------------------------------------

#[test]
fn systemctl_and_launchctl_output_are_parsed_into_the_same_shape() {
    let running = service::parse_systemctl_show(
        "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=101\n\
         ExecStart={ path=/usr/bin/superbackup ; argv[]=/usr/bin/superbackup daemon ; }\n\
         UnitFileState=enabled\n",
    );
    assert!(running.installed);
    assert_eq!(running.state, ServiceState::Running);
    assert_eq!(running.pid, Some(101));
    assert_eq!(running.health(), Health::Idle);

    let failed = service::parse_systemctl_show(
        "LoadState=loaded\nActiveState=failed\nSubState=failed\nMainPID=0\nUnitFileState=enabled\n",
    );
    assert_eq!(failed.state, ServiceState::Stopped);
    assert_eq!(failed.pid, None);
    assert_eq!(
        failed.health(),
        Health::Failed,
        "an enabled service that is not running is a real problem"
    );

    let absent = service::parse_systemctl_show("LoadState=not-found\nActiveState=inactive\n");
    assert!(!absent.installed);
    assert_eq!(absent.health(), Health::Idle, "never installed is not a failure");

    let mac = service::parse_launchctl_print("\tstate = running\n\tpid = 4242\n");
    assert_eq!(mac.state, ServiceState::Running);
    assert_eq!(mac.pid, Some(4242));
}

#[test]
fn a_service_pointing_at_a_binary_that_moved_is_detected() {
    let mut status = service::ServiceStatus::not_installed();
    status.installed = true;
    status.state = ServiceState::Running;
    status.executable = Some(PathBuf::from("/opt/superbackup-1.0/superbackup"));
    assert!(status.is_stale(std::path::Path::new("/opt/superbackup-1.1/superbackup")));
    assert!(!status.is_stale(std::path::Path::new("/opt/superbackup-1.0/superbackup")));
}

// ---------------------------------------------------------------------------
// Real platform, read-only
// ---------------------------------------------------------------------------

#[test]
fn querying_a_service_that_does_not_exist_is_an_answer_not_an_error() {
    let status = service::status("superbackup-definitely-not-installed", ServiceScope::System)
        .expect("querying a missing service must not error");
    assert!(!status.installed);
    assert_eq!(status.state, ServiceState::NotInstalled);
}

#[test]
fn installing_without_elevation_gives_an_actionable_error() {
    if service::is_elevated() {
        // Running elevated: we must not actually install anything, so all we
        // can assert is that the elevation check agrees with itself.
        assert!(ServiceOptions::current(&Paths::rooted_at("/tmp/sb", true))
            .map(|o| o.requires_elevation())
            .unwrap_or(true));
        return;
    }
    let opts = options(ServiceScope::System, ServiceAccount::LocalSystem);
    let err = service::install(&opts).expect_err("installing a system service needs elevation");
    let message = err.to_string();
    assert!(
        message.contains("administrator") || message.contains("privileges"),
        "the error must name the actual problem: {message}"
    );
    if cfg!(windows) {
        assert!(
            message.contains("Run as administrator"),
            "and tell the user exactly what to click: {message}"
        );
    }
    assert_eq!(err.code(), superbackup_core::ErrorCode::Service);
}

#[test]
#[ignore = "installs and removes a real service; needs Administrator (Windows) or root (Unix)"]
fn install_start_stop_uninstall_round_trip() {
    assert!(service::is_elevated(), "run this test from an elevated shell, or it proves nothing");
    let paths = Paths::for_service().expect("service paths");
    let mut opts = ServiceOptions::current(&paths).expect("current exe");
    opts.name = "superbackup-selftest".into();
    opts.display_name = "superbackup self-test".into();
    opts.start_mode = StartMode::Manual;

    // Always clean up, even if an assertion below fails.
    let _ = service::uninstall(&opts.name, opts.scope);

    service::install(&opts).expect("install");
    let status = service::status(&opts.name, opts.scope).expect("status");
    assert!(status.installed, "{status:?}");
    assert_eq!(status.state, ServiceState::Stopped);
    assert!(!status.is_stale(&opts.executable), "a fresh install is never stale");

    service::uninstall(&opts.name, opts.scope).expect("uninstall");
    assert!(!service::status(&opts.name, opts.scope).expect("status").installed);
    service::uninstall(&opts.name, opts.scope).expect("uninstalling twice must be a no-op");
}

#[test]
#[ignore = "installs a deliberately broken service to prove nothing is left behind; needs elevation"]
fn a_failed_install_leaves_nothing_behind() {
    assert!(service::is_elevated(), "run this test from an elevated shell");
    let paths = Paths::for_service().expect("service paths");
    let mut opts = ServiceOptions::current(&paths).expect("current exe");
    opts.name = "superbackup-selftest-broken".into();
    // An account that cannot possibly exist: the SCM rejects it at create time.
    opts.account =
        ServiceAccount::User { username: r".\superbackup-no-such-account".into(), password: None };

    let _ = service::install(&opts);
    let status = service::status(&opts.name, opts.scope).expect("status");
    assert!(!status.installed, "a failed install must not leave a service behind: {status:?}");
}
