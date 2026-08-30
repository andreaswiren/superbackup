//! Notification policy, deduplication and redaction.
//!
//! These run against a log-only [`Notifier`], so nothing appears on the
//! desktop and nothing depends on a notification daemon being present. The one
//! test that raises a real toast is `#[ignore]`d; run it with
//! `cargo test -p superbackup-core --test platform_notify -- --ignored --nocapture`
//! and look at the screen.

use chrono::{Duration, Utc};
use superbackup_core::model::NotificationSettings;
use superbackup_core::platform::notify::{
    self, ActionTarget, Notification, NotificationAction, NotificationKind, Notifier, NotifyOutcome,
};
use superbackup_core::state::Event;
use superbackup_core::ErrorCode;
use uuid::Uuid;

fn settings() -> NotificationSettings {
    NotificationSettings {
        enabled: true,
        on_failure: true,
        on_success: false,
        stale_after_days: 3,
        on_service_error: true,
        dedupe_minutes: 60,
    }
}

fn failure(job: Uuid, code: ErrorCode, body: &str) -> Notification {
    Notification::new(NotificationKind::Failure, "Backup failed", body)
        .with_job(job)
        .with_error_code(code)
}

#[test]
fn a_job_that_fails_every_quarter_hour_produces_one_notification_an_hour() {
    let notifier = Notifier::log_only(settings());
    let job = Uuid::new_v4();
    let t0 = Utc::now();

    let mut shown = 0;
    for minute in (0..180).step_by(15) {
        let at = t0 + Duration::minutes(minute);
        if notifier.notify_at(&failure(job, ErrorCode::Io, "network unreachable"), at).was_shown() {
            shown += 1;
        }
    }
    // Three hours at a 60-minute window: t=0, t=60, t=120 (and t=180 is
    // outside the loop).
    assert_eq!(shown, 3, "twelve failures must not become twelve toasts");
}

#[test]
fn a_job_that_starts_failing_differently_gets_through_immediately() {
    let notifier = Notifier::log_only(settings());
    let job = Uuid::new_v4();
    let t0 = Utc::now();

    assert!(notifier.notify_at(&failure(job, ErrorCode::Io, "disk full"), t0).was_shown());
    assert!(
        !notifier
            .notify_at(&failure(job, ErrorCode::Io, "disk full again"), t0 + Duration::minutes(1))
            .was_shown(),
        "the same problem is suppressed"
    );
    assert!(
        notifier
            .notify_at(
                &failure(job, ErrorCode::BadPassphrase, "wrong passphrase"),
                t0 + Duration::minutes(1)
            )
            .was_shown(),
        "a new kind of failure is news and must not be swallowed"
    );
}

#[test]
fn two_different_jobs_do_not_suppress_each_other() {
    let notifier = Notifier::log_only(settings());
    let t0 = Utc::now();
    assert!(notifier.notify_at(&failure(Uuid::new_v4(), ErrorCode::Io, "x"), t0).was_shown());
    assert!(notifier.notify_at(&failure(Uuid::new_v4(), ErrorCode::Io, "x"), t0).was_shown());
}

#[test]
fn a_success_makes_the_next_failure_news_again() {
    let notifier = Notifier::log_only(settings());
    let job = Uuid::new_v4();
    let t0 = Utc::now();

    assert!(notifier.notify_at(&failure(job, ErrorCode::Io, "x"), t0).was_shown());
    assert!(!notifier
        .notify_at(&failure(job, ErrorCode::Io, "x"), t0 + Duration::minutes(1))
        .was_shown());

    notifier.subject_recovered(&job);
    assert!(
        notifier
            .notify_at(&failure(job, ErrorCode::Io, "x"), t0 + Duration::minutes(2))
            .was_shown(),
        "after a successful run the next failure is a new event"
    );
}

#[test]
fn the_master_switch_and_the_per_kind_switches_are_both_honoured() {
    let notifier = Notifier::log_only(settings());
    assert_eq!(
        notifier.notify(&Notification::new(NotificationKind::Success, "Done", "ok")),
        NotifyOutcome::SuppressedByKind
    );

    let mut all_off = settings();
    all_off.enabled = false;
    notifier.update_settings(all_off);
    assert_eq!(
        notifier.notify(&failure(Uuid::new_v4(), ErrorCode::Io, "x")),
        NotifyOutcome::SuppressedDisabled
    );

    let mut success_on = settings();
    success_on.on_success = true;
    notifier.update_settings(success_on);
    assert!(notifier
        .notify(&Notification::new(NotificationKind::Success, "Done", "ok"))
        .was_shown());
}

#[test]
fn a_zero_dedupe_window_shows_everything() {
    let mut s = settings();
    s.dedupe_minutes = 0;
    let notifier = Notifier::log_only(s);
    let job = Uuid::new_v4();
    let t0 = Utc::now();
    for _ in 0..5 {
        assert!(notifier.notify_at(&failure(job, ErrorCode::Io, "x"), t0).was_shown());
    }
}

#[test]
fn every_notification_carries_something_the_tray_can_act_on() {
    let job = Uuid::new_v4();
    let n = failure(job, ErrorCode::Kopia, "kopia exited 1")
        .with_action(NotificationAction::retry_job(job));

    let targets: Vec<&ActionTarget> = n.actions.iter().map(|a| &a.target).collect();
    assert!(targets.contains(&&ActionTarget::OpenJob { job_id: job }));
    assert!(targets.contains(&&ActionTarget::RetryJob { job_id: job }));
    for action in &n.actions {
        assert!(!action.id.is_empty());
        assert!(action.label.len() <= 20, "Windows truncates long buttons: {}", action.label);
    }

    // The tray runs in another process, so the payload must survive JSON.
    let json = serde_json::to_string(&n).expect("serialise");
    let back: Notification = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(back.actions, n.actions);
    assert_eq!(back.dedupe_key(), n.dedupe_key());
}

#[test]
fn the_activity_log_and_the_notifier_agree_about_what_deserves_a_toast() {
    let job = Uuid::new_v4();
    assert_eq!(
        Notification::from_event(&Event::error("job.failed", "boom").with_job(job)).map(|n| n.kind),
        Some(NotificationKind::Failure)
    );
    assert_eq!(
        Notification::from_event(&Event::error("service.exited", "code 1")).map(|n| n.kind),
        Some(NotificationKind::ServiceError)
    );
    assert!(
        Notification::from_event(&Event::info("job.started", "starting")).is_none(),
        "routine activity is log material, not a toast"
    );
    assert!(Notification::from_event(&Event::info("vault.unlocked", "ok")).is_none());
}

#[test]
fn the_installer_contract_for_windows_toasts_is_documented_in_code() {
    let reqs = notify::installer_requirements();
    assert!(reqs.iter().any(|r| r.contains(notify::APP_USER_MODEL_ID)));
    assert!(reqs.iter().any(|r| r.contains("Start-menu shortcut")));
    assert!(
        reqs.iter().any(|r| r.contains("silently discards")),
        "the consequence of getting it wrong must be spelled out"
    );

    // Reading the registration is read-only and must never fail.
    let reg = notify::toast_registration();
    if reg.registered {
        assert!(reg.warning.is_none());
    } else {
        assert!(reg.warning.is_some(), "an unregistered app must explain itself");
    }
}

#[test]
fn a_notifier_never_returns_an_error_however_hostile_the_input() {
    let notifier = Notifier::log_only(settings());
    let nasty = Notification::new(
        NotificationKind::Failure,
        "Backup failed \u{202e}\u{0}",
        "\u{1}\u{2}\u{3} KOPIA_PASSWORD=hunter2 \u{feff}".repeat(200),
    );
    // The contract is a `NotifyOutcome`, never a panic and never a `Result`.
    let outcome = notifier.notify(&nasty);
    assert!(matches!(
        outcome,
        NotifyOutcome::Shown
            | NotifyOutcome::Failed { .. }
            | NotifyOutcome::Unavailable { .. }
            | NotifyOutcome::Deduped { .. }
    ));
}

#[test]
#[ignore = "raises a real desktop notification; run it and look at the screen"]
fn a_real_notification_appears_on_the_desktop() {
    let notifier = Notifier::new(settings());
    if let Some(warning) = notifier.platform_warning() {
        println!("platform warning: {warning}");
    }
    let job = Uuid::new_v4();
    let outcome = notifier.notify(
        &failure(job, ErrorCode::Kopia, "This is a superbackup self-test notification.")
            .with_action(NotificationAction::retry_job(job)),
    );
    println!("outcome: {outcome:?}");
    assert!(
        matches!(outcome, NotifyOutcome::Shown | NotifyOutcome::Unavailable { .. }),
        "{outcome:?}"
    );
}
