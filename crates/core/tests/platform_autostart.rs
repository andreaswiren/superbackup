//! Start-at-login.
//!
//! The pure part — command-line quoting, `.desktop` escaping, and the
//! stale-path detection that is the only failure anybody actually hits — is
//! tested here without touching the registry or the user's home directory.
//!
//! The tests that mutate real system state (writing to
//! `HKCU\...\CurrentVersion\Run`, or dropping a file in `~/.config/autostart`)
//! are marked `#[ignore]`. Run them deliberately with
//! `cargo test -p superbackup-core --test platform_autostart -- --ignored`.
//! They restore the previous state on the way out.

use superbackup_core::platform::autostart::{self, AutostartSpec, AutostartState};

fn windows_exe() -> &'static str {
    r"C:\Program Files\superbackup\superbackup.exe"
}

fn unix_exe() -> &'static str {
    "/opt/super backup/bin/superbackup"
}

fn spec() -> AutostartSpec {
    AutostartSpec::for_executable(if cfg!(windows) { windows_exe() } else { unix_exe() })
}

#[test]
fn the_registered_command_always_starts_minimised() {
    let spec = spec();
    assert!(spec.args.iter().any(|a| a == autostart::MINIMISED_FLAG));
    let line = if cfg!(windows) { spec.windows_command_line() } else { spec.desktop_exec() };
    assert!(line.contains(autostart::MINIMISED_FLAG), "{line}");
}

#[test]
fn a_windows_path_with_spaces_is_quoted() {
    let spec = AutostartSpec::for_executable(windows_exe());
    let line = spec.windows_command_line();
    assert!(
        line.starts_with('"'),
        "an unquoted Program Files path lets Windows run C:\\Program.exe instead: {line}"
    );
    let argv = autostart::parse_windows_command_line(&line);
    assert_eq!(argv, vec![windows_exe().to_string(), autostart::MINIMISED_FLAG.to_string()]);
}

#[test]
fn windows_quoting_round_trips_for_hostile_arguments() {
    for arg in [
        r"C:\Program Files\superbackup\superbackup.exe",
        r"C:\ends with backslash\",
        r#"a "quoted" thing"#,
        r"C:\a\\b",
        "no-spaces",
        "",
        "tab\there",
    ] {
        let quoted = autostart::quote_windows_arg(arg);
        assert_eq!(
            autostart::parse_windows_command_line(&quoted),
            vec![arg.to_string()],
            "{arg:?} -> {quoted}"
        );
    }
}

#[test]
fn desktop_exec_escaping_round_trips_for_hostile_arguments() {
    for arg in [
        "/opt/super backup/bin/superbackup",
        "/opt/back\\slash/sb",
        "/opt/$HOME/sb",
        "/opt/`whoami`/sb",
        "/opt/quote\"here/sb",
        "--minimised",
        "100%",
    ] {
        let escaped = autostart::escape_desktop_arg(arg);
        assert_eq!(
            autostart::parse_desktop_exec(&escaped),
            vec![arg.to_string()],
            "{arg:?} -> {escaped}"
        );
    }
}

#[test]
fn a_desktop_entry_is_well_formed_and_readable_back() {
    let spec = AutostartSpec::for_executable(unix_exe());
    let text = autostart::render_desktop_entry(&spec);
    assert!(text.starts_with("[Desktop Entry]"));
    assert!(text.contains("Type=Application"));
    assert!(text.contains("Terminal=false"));

    let exec = autostart::parse_desktop_entry_exec(&text).expect("an Exec= line");
    let argv = autostart::parse_desktop_exec(&exec);
    assert_eq!(argv[0], unix_exe());
    assert_eq!(argv[1], autostart::MINIMISED_FLAG);
}

#[test]
fn a_launch_agent_is_well_formed_and_readable_back() {
    let spec = AutostartSpec::for_executable("/Applications/superbackup.app/Contents/MacOS/sb");
    let plist = autostart::render_launch_agent(&spec);
    assert!(plist.contains("<?xml"));
    assert!(plist.contains(autostart::LAUNCH_AGENT_LABEL));
    assert!(plist.contains("<key>RunAtLoad</key>"));

    let command = autostart::parse_launch_agent_command(&plist).expect("ProgramArguments");
    let argv = autostart::parse_desktop_exec(&command);
    assert_eq!(argv[0], "/Applications/superbackup.app/Contents/MacOS/sb");
    assert_eq!(argv[1], autostart::MINIMISED_FLAG);
}

#[test]
fn an_entry_left_behind_by_an_upgrade_is_reported_as_stale_not_enabled() {
    let want = spec();
    let old = if cfg!(windows) {
        AutostartSpec::for_executable(r"C:\Users\me\Downloads\superbackup.exe")
            .windows_command_line()
    } else {
        AutostartSpec::for_executable("/home/me/Downloads/superbackup").desktop_exec()
    };
    match autostart::classify(&old, &want) {
        AutostartState::Stale { registered, expected } => {
            assert!(registered.contains("Downloads"), "{registered}");
            assert_eq!(expected, want.executable.to_string_lossy());
        }
        other => panic!("a moved executable must be Stale, got {other:?}"),
    }
    assert!(AutostartState::Stale {
        registered: "a".into(),
        expected: "b".into()
    }
    .needs_repair());
}

#[test]
fn an_entry_belonging_to_another_program_is_not_touched() {
    let want = spec();
    let foreign = if cfg!(windows) {
        r#""C:\Windows\System32\notepad.exe""#
    } else {
        "/usr/bin/gedit"
    };
    let state = autostart::classify(foreign, &want);
    assert!(
        matches!(state, AutostartState::Unrecognised { .. }),
        "we must not claim somebody else's entry: {state:?}"
    );
    assert!(state.needs_repair(), "but the GUI should still be able to mention it");
}

#[test]
fn a_matching_entry_is_enabled_and_needs_nothing() {
    let want = spec();
    let current =
        if cfg!(windows) { want.windows_command_line() } else { want.desktop_exec() };
    assert_eq!(autostart::classify(&current, &want), AutostartState::Enabled);
    assert!(!AutostartState::Enabled.needs_repair());
    assert!(AutostartState::Enabled.is_enabled());
}

#[test]
fn status_reads_the_real_platform_without_changing_it() {
    // Read-only. On a machine with no entry this reports Disabled; on a
    // developer machine that has one it reports whatever is really there. The
    // point is that it never errors and never writes.
    let spec = AutostartSpec::current().expect("the test binary has a path");
    let status = autostart::status(&spec).expect("status must not fail");
    assert!(!status.location.is_empty());
    assert!(!status.state.summary().is_empty());
    match (&status.state, &status.registered_command) {
        (AutostartState::Disabled, cmd) => assert!(cmd.is_none()),
        (_, cmd) => assert!(cmd.is_some(), "a non-disabled state must name its command"),
    }
}

#[test]
#[ignore = "writes to HKCU\\...\\Run (Windows) or ~/.config/autostart (Unix); restores afterwards"]
fn enable_disable_round_trip_against_the_real_platform() {
    let spec = AutostartSpec::current().expect("current exe");
    let before = autostart::status(&spec).expect("status");

    autostart::enable(&spec).expect("enable");
    assert!(autostart::is_enabled().expect("is_enabled"));
    let after = autostart::status(&spec).expect("status");
    assert_eq!(after.state, AutostartState::Enabled, "{after:?}");

    // Simulate the post-upgrade failure and prove it self-heals.
    let moved = AutostartSpec::for_executable(if cfg!(windows) {
        r"C:\definitely\not\here\superbackup.exe"
    } else {
        "/definitely/not/here/superbackup"
    });
    autostart::enable(&moved).expect("enable at the wrong path");
    let stale = autostart::status(&spec).expect("status");
    assert!(stale.state.needs_repair(), "{stale:?}");
    let healed = autostart::heal(&spec).expect("heal").expect("an event describing the repair");
    assert_eq!(healed.kind, "autostart.repaired");
    assert_eq!(autostart::status(&spec).expect("status").state, AutostartState::Enabled);

    autostart::disable().expect("disable");
    assert!(!autostart::is_enabled().expect("is_enabled"));
    autostart::disable().expect("disabling twice must be a no-op");

    // Put the machine back the way we found it.
    if before.state.is_enabled() {
        autostart::enable(&spec).expect("restore");
    }
}
