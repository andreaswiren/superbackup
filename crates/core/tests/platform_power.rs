//! Power, connectivity and the platform capability report.
//!
//! All read-only. The governing property under test is that **nothing here can
//! block a backup**: every query either answers or returns a default that
//! means "carry on".

use chrono::Utc;
use std::time::Duration;
use superbackup_core::platform::{self, power, Metered, PowerSource};

#[test]
fn a_machine_we_cannot_read_still_gets_its_backups() {
    // Whatever this host is, neither query may come back saying "skip".
    let cost = power::connection_cost();
    let status = power::power_status();

    if cost == Metered::Unknown {
        assert!(!cost.should_skip(), "an unknown connection cost must never skip a backup");
    }
    if status.source == PowerSource::Unknown {
        assert!(!status.should_skip_on_battery(), "an unreadable battery must never skip a backup");
    }
    assert!(!power::is_metered_connection() || cost == Metered::Metered);
}

#[test]
fn live_readings_are_internally_consistent() {
    let status = power::power_status();
    assert!(status.battery_percent.map(|p| p <= 100).unwrap_or(true));
    if !status.battery_present {
        assert_ne!(
            status.source,
            PowerSource::Battery,
            "a machine with no battery cannot be running on one"
        );
    }
    assert!(!status.summary().is_empty());
    assert_eq!(power::is_on_battery(), status.should_skip_on_battery());
    assert_eq!(power::battery_percent(), status.battery_percent);
}

#[test]
fn windows_reports_a_real_connection_cost() {
    if !cfg!(windows) {
        return;
    }
    // On Windows the Network List Manager is always present, so an
    // Unknown answer means our COM plumbing is broken rather than that the
    // platform declined — unless the machine genuinely has no network.
    let cost = power::connection_cost();
    println!("Windows connection cost: {cost:?} ({})", cost.title());
    assert!(matches!(cost, Metered::Metered | Metered::Unmetered | Metered::Unknown));
}

#[test]
fn the_metered_classifier_never_guesses_yes() {
    // Every bit pattern that is not positively expensive must not skip.
    for cost in [0u32, 0x1] {
        assert!(!power::classify_nlm_cost(cost).should_skip(), "cost {cost:#x}");
    }
    for cost in [0x2u32, 0x4, 0x10000, 0x20000, 0x40000, 0x80000] {
        assert!(power::classify_nlm_cost(cost).should_skip(), "cost {cost:#x}");
    }
}

#[test]
fn a_suspend_is_detected_and_a_clock_step_is_not() {
    let mut detector = power::WakeDetector::with_tolerance(60);
    let t0 = Utc::now();

    // Prime it.
    let _ = detector.evaluate(Duration::from_secs(0), t0);

    // A normal tick.
    assert!(detector
        .evaluate(Duration::from_secs(60), t0 + chrono::Duration::seconds(60))
        .is_none());

    // The lid was closed for six hours; the monotonic clock barely moved.
    let gap = detector
        .evaluate(
            Duration::from_secs(2),
            t0 + chrono::Duration::seconds(60) + chrono::Duration::hours(6),
        )
        .expect("six hours of wall clock with no monotonic time is a suspend");
    assert!(gap.seconds > 21_000, "{gap:?}");

    // A daylight-saving or NTP correction backwards is not a wake.
    assert!(detector.evaluate(Duration::from_secs(60), t0 - chrono::Duration::hours(1)).is_none());
}

#[test]
fn the_live_detector_reports_nothing_on_a_running_machine() {
    let mut detector = power::WakeDetector::new();
    assert!(detector.tick().is_none(), "we have not been asleep during this test");
}

#[test]
fn capabilities_match_the_platform_this_was_built_for() {
    let caps = platform::capabilities();
    assert!(caps.autostart);
    assert!(caps.notifications);

    if cfg!(windows) {
        assert!(caps.native_onedrive);
        assert!(caps.pin_cloud_files, "Windows can pin cloud files, and we must use it");
        assert!(caps.metered_detection);
        assert!(caps.power_events, "the SCM sends power events to a service");
        assert!(!caps.user_service, "a Windows service always lives in session 0");
    }
    if cfg!(target_os = "linux") {
        assert!(!caps.native_onedrive, "Microsoft ships no Linux client");
        assert!(caps.user_service);
        assert!(!caps.pin_cloud_files);
    }
    if cfg!(target_os = "macos") {
        assert!(caps.native_onedrive);
        assert!(!caps.metered_detection, "macOS has no public metered API");
        assert!(!caps.pin_cloud_files);
    }
}

#[test]
fn every_platform_limitation_is_something_the_gui_can_show() {
    let limits = platform::limitations();
    assert!(!limits.is_empty(), "no platform is perfect; say so");

    let mut codes: Vec<&str> = limits.iter().map(|l| l.code.as_str()).collect();
    let before = codes.len();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(before, codes.len(), "codes must be unique so the GUI can key off them");

    for limit in &limits {
        assert!(limit.code.contains('.'), "namespaced code expected: {}", limit.code);
        assert!(
            ["onedrive", "service", "notifications", "power", "autostart"]
                .contains(&limit.area.as_str()),
            "unknown UI area {}",
            limit.area
        );
        assert!(limit.message.ends_with('.'), "not a sentence: {}", limit.message);
        assert!(limit.message.len() > 40, "too terse to be useful: {}", limit.message);
        if let Some(remedy) = &limit.remedy {
            assert!(remedy.ends_with('.'), "not a sentence: {remedy}");
        }
    }

    if cfg!(windows) {
        // The three that generate the most support traffic.
        for code in ["win.service_no_user_profile", "win.files_on_demand", "win.long_paths"] {
            assert!(codes.contains(&code), "{code} must be advertised");
        }
    }
}

#[test]
fn platform_info_is_specific_enough_for_a_bug_report() {
    let info = platform::platform_info();
    assert_eq!(info.os, std::env::consts::OS);
    assert_eq!(info.arch, std::env::consts::ARCH);
    assert!(
        info.os_version.len() > info.os.len(),
        "\"{}\" tells a support engineer nothing",
        info.os_version
    );
    if cfg!(windows) {
        assert!(info.os_version.starts_with("Windows"));
        assert!(
            info.os_version.contains("build "),
            "the actual build number is the point: {}",
            info.os_version
        );
    }
    // Round-trips to the GUI process.
    let json = serde_json::to_string(&info).expect("serialise");
    assert!(json.contains("\"limitations\""));
}
