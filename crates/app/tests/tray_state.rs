//! The tray, derived from health and nothing else.
//!
//! `DESIGN_SYSTEM.md` §7.3 is explicit: the icon shown is
//! `StatusSnapshot::derive_health(...)`, and **the GUI must never compute this
//! independently**. This file checks that the tray's whole visible state —
//! icon, tooltip and menu — is a function of the daemon's snapshot, and that
//! the precedence order is the one the design specifies:
//!
//! ```text
//!   Failed > Running > Paused > Attention (locked vault or stale job) > Idle
//! ```
//!
//! It also checks the mark itself: five states, two Windows variants, twelve
//! running frames, and greyscale distinctness — because "one man in twelve"
//! cannot tell red from green, and colour is confirmation, never the message.

#![allow(dead_code)]

mod cli {
    pub mod exit {
        pub const OK: i32 = 0;
        pub const FAILED: i32 = 1;
        pub const USAGE: i32 = 2;
        pub const DAEMON_UNREACHABLE: i32 = 3;
        pub const LOCKED: i32 = 4;
        pub const CANCELLED: i32 = 5;
    }

    #[derive(Debug, Clone, Default)]
    pub struct GlobalArgs {
        pub json: bool,
        pub quiet: bool,
        pub verbose: u8,
        pub no_input: bool,
        pub home: Option<std::path::PathBuf>,
        pub service: bool,
        pub timeout: u64,
    }
}

#[path = "../src/daemon/mod.rs"]
mod daemon;
#[path = "../src/tray/mod.rs"]
mod tray;

use chrono::{Duration, Utc};
use superbackup_core::engine::testing::test_job;
use superbackup_core::model::Config;
use superbackup_core::state::{
    DestinationRun, Health, JobRun, JobSummary, Progress, RunStatus, StatusSnapshot, Trigger,
};
use tray::icons::{IconKey, Variant, RUNNING_FRAMES};
use tray::menu::{self, Action, Item};
use uuid::Uuid;

fn snapshot() -> StatusSnapshot {
    StatusSnapshot {
        health: Health::Idle,
        version: "0.1.0".into(),
        machine_label: "pc".into(),
        machine_slug: "pc".into(),
        unlocked: true,
        paused: false,
        paused_until: None,
        service_installed: false,
        service_running: false,
        kopia_version: Some("0.21.5".into()),
        active_runs: vec![],
        jobs: Default::default(),
        next_scheduled: None,
        recent_events: vec![],
        uptime_seconds: 120,
        generated_at: Utc::now(),
    }
}

fn running(job_id: Uuid, name: &str) -> JobRun {
    JobRun {
        run_id: Uuid::new_v4(),
        job_id,
        job_name: name.into(),
        trigger: Trigger::Schedule,
        status: RunStatus::Running,
        started_at: Utc::now(),
        finished_at: None,
        destinations: vec![DestinationRun {
            destination_id: Uuid::new_v4(),
            destination_name: "disk".into(),
            status: RunStatus::Running,
            started_at: Some(Utc::now()),
            finished_at: None,
            progress: Progress {
                bytes_processed: 420,
                bytes_total: Some(1000),
                bytes_per_second: 5_000_000.0,
                estimated_seconds_remaining: Some(90),
                ..Default::default()
            },
            snapshot_id: None,
            error: None,
            warnings: vec![],
        }],
    }
}

/// The precedence order in §7.3, checked against the core's own rule.
#[test]
fn health_precedence_is_the_daemons_and_never_the_trays() {
    // Failed beats everything, including a run in progress.
    assert_eq!(
        StatusSnapshot::derive_health(true, true, true, true, true),
        Health::Failed
    );
    // Running beats paused: a backup that is happening is what the user wants
    // to see, even if schedules are off.
    assert_eq!(
        StatusSnapshot::derive_health(true, true, true, false, true),
        Health::Running
    );
    // Paused beats attention.
    assert_eq!(
        StatusSnapshot::derive_health(false, true, false, false, true),
        Health::Paused
    );
    // A locked vault is attention.
    assert_eq!(
        StatusSnapshot::derive_health(false, false, false, false, false),
        Health::Attention
    );
    // A stale job is attention too.
    assert_eq!(
        StatusSnapshot::derive_health(true, false, false, false, true),
        Health::Attention
    );
    // Everything fine.
    assert_eq!(
        StatusSnapshot::derive_health(true, false, false, false, false),
        Health::Idle
    );
}

/// Every health value produces a distinct, non-blank mark in both Windows
/// variants and in the macOS template.
#[test]
fn every_health_has_its_own_mark_in_every_variant() {
    let states =
        [Health::Idle, Health::Running, Health::Attention, Health::Paused, Health::Failed];
    for variant in [Variant::LightTaskbar, Variant::DarkTaskbar, Variant::Template] {
        let mut rendered = Vec::new();
        for health in states {
            let rgba = tray::icons::rasterise(IconKey::new(health, variant, 0), 32)
                .unwrap_or_else(|e| panic!("{health:?}/{variant:?} did not render: {e}"));
            let visible = rgba.chunks_exact(4).filter(|p| p[3] > 32).count();
            assert!(visible > 40, "{health:?}/{variant:?} rendered a blank icon");
            rendered.push((health, rgba));
        }
        for (i, (a_health, a)) in rendered.iter().enumerate() {
            for (b_health, b) in rendered.iter().skip(i + 1) {
                assert_ne!(
                    a, b,
                    "{a_health:?} and {b_health:?} render identically in {variant:?}: a \
                     colour-blind user could not tell them apart"
                );
            }
        }
    }
}

/// The marks survive being desaturated — shape, not colour, carries the state.
#[test]
fn the_marks_are_distinct_in_greyscale() {
    let states =
        [Health::Idle, Health::Running, Health::Attention, Health::Paused, Health::Failed];
    let grey: Vec<(Health, Vec<u8>)> = states
        .iter()
        .map(|health| {
            let rgba = tray::icons::rasterise(IconKey::new(*health, Variant::DarkTaskbar, 0), 32)
                .expect("render");
            // Rec. 601 luma, plus alpha: exactly what a greyscale printout or
            // a monochrome taskbar would show.
            let grey: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|p| {
                    let y = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
                        .round() as u8;
                    [y, p[3]]
                })
                .collect();
            (*health, grey)
        })
        .collect();

    for (i, (a_health, a)) in grey.iter().enumerate() {
        for (b_health, b) in grey.iter().skip(i + 1) {
            let differing = a.iter().zip(b.iter()).filter(|(x, y)| x != y).count();
            assert!(
                differing > 100,
                "{a_health:?} and {b_health:?} differ in only {differing} greyscale samples; \
                 the distinction is carried by colour alone"
            );
        }
    }
}

/// The running animation is twelve distinct frames, and only running animates.
#[test]
fn only_the_running_state_animates_and_it_has_twelve_frames() {
    let mut frames = Vec::new();
    for frame in 0..RUNNING_FRAMES {
        frames.push(
            tray::icons::rasterise(IconKey::new(Health::Running, Variant::DarkTaskbar, frame), 32)
                .expect("render a frame"),
        );
    }
    for (i, a) in frames.iter().enumerate() {
        for (j, b) in frames.iter().enumerate().skip(i + 1) {
            assert_ne!(a, b, "running frames {i} and {j} are identical");
        }
    }
    // A frame index is meaningless for every other state, so the cache never
    // grows twelve copies of an icon that does not move.
    for health in [Health::Idle, Health::Attention, Health::Paused, Health::Failed] {
        let still = tray::icons::rasterise(IconKey::new(health, Variant::DarkTaskbar, 0), 32)
            .expect("render");
        let same = tray::icons::rasterise(IconKey::new(health, Variant::DarkTaskbar, 7), 32)
            .expect("render");
        assert_eq!(still, same, "{health:?} must not animate");
    }
}

/// The tooltip is the two lines §7.5 specifies, for every state.
#[test]
fn the_tooltip_says_what_the_state_means() {
    let mut config = Config::default();
    let mut jobs = Vec::new();
    for name in ["dev code", "photos"] {
        let job = test_job(name);
        jobs.push(job.id);
        config.jobs.push(job);
    }

    // Idle with a next run.
    let mut snap = snapshot();
    snap.next_scheduled = Some((jobs[0], Utc::now() + Duration::hours(3)));
    let plan = menu::plan(&snap, &config, true);
    assert_eq!(plan.tooltip_title, "superbackup — Up to date");
    assert!(plan.tooltip_detail.contains("Last backup") || plan.tooltip_detail.contains("in about 3 hours"), "{}", plan.tooltip_detail);

    // Running: job name, percentage, rate, remaining.
    let mut snap = snapshot();
    snap.health = Health::Running;
    snap.active_runs = vec![running(jobs[0], "dev code")];
    let plan = menu::plan(&snap, &config, true);
    assert_eq!(plan.tooltip_title, "superbackup — Backing up");
    assert!(plan.tooltip_detail.starts_with("dev code"));
    assert!(plan.tooltip_detail.contains("42%"), "{}", plan.tooltip_detail);

    // Paused, with a time.
    let mut snap = snapshot();
    snap.health = Health::Paused;
    snap.paused = true;
    snap.paused_until = Some(Utc::now() + Duration::hours(2));
    let plan = menu::plan(&snap, &config, true);
    assert_eq!(plan.tooltip_title, "superbackup — Paused");
    assert!(plan.tooltip_detail.starts_with("Paused until "), "{}", plan.tooltip_detail);

    // Failed, naming the job.
    let mut snap = snapshot();
    snap.health = Health::Failed;
    snap.jobs.insert(
        jobs[0],
        JobSummary { last_status: Some(RunStatus::Failed), ..Default::default() },
    );
    let plan = menu::plan(&snap, &config, true);
    assert_eq!(plan.tooltip_title, "superbackup — Backup failed");
    assert!(plan.tooltip_detail.contains("dev code"), "{}", plan.tooltip_detail);

    // Locked, which takes precedence over a stale job.
    let mut snap = snapshot();
    snap.health = Health::Attention;
    snap.unlocked = false;
    snap.jobs.insert(
        jobs[1],
        JobSummary {
            last_success: Some(Utc::now() - Duration::days(90)),
            ..Default::default()
        },
    );
    let plan = menu::plan(&snap, &config, true);
    assert_eq!(plan.tooltip_detail, "The vault is locked");
}

/// The tooltip's first line is the icon's accessible name and the second its
/// description (§14.5).
#[test]
fn the_tooltip_is_two_lines_and_doubles_as_the_accessible_name() {
    let config = Config::default();
    let plan = menu::plan(&snapshot(), &config, true);
    let tooltip = plan.tooltip();
    let lines: Vec<&str> = tooltip.lines().collect();
    assert_eq!(lines.len(), 2, "a tray tooltip is exactly two lines: {tooltip:?}");
    assert_eq!(lines[0], plan.tooltip_title);
    assert_eq!(lines[1], plan.tooltip_detail);
    assert!(!lines[1].is_empty(), "the second line must always say something");
}

/// The menu's shape is stable across every state: items are disabled, never
/// removed (§14.2).
#[test]
fn the_menus_shape_is_stable_so_muscle_memory_works() {
    let mut config = Config::default();
    config.jobs.push(test_job("dev code"));
    let job_id = config.jobs[0].id;

    let always_present = [
        Action::RunAll,
        Action::OpenApp,
        Action::OpenActivity,
        Action::OpenSettings,
        Action::Quit,
    ];

    let mut states = Vec::new();
    states.push(("idle", snapshot()));

    let mut locked = snapshot();
    locked.unlocked = false;
    locked.health = Health::Attention;
    states.push(("locked", locked));

    let mut paused = snapshot();
    paused.paused = true;
    paused.health = Health::Paused;
    states.push(("paused", paused));

    let mut busy = snapshot();
    busy.health = Health::Running;
    busy.active_runs = vec![running(job_id, "dev code")];
    states.push(("running", busy));

    let mut failed = snapshot();
    failed.health = Health::Failed;
    failed
        .jobs
        .insert(job_id, JobSummary { last_status: Some(RunStatus::Failed), ..Default::default() });
    states.push(("failed", failed));

    for (name, snap) in &states {
        for kopia in [true, false] {
            let plan = menu::plan(snap, &config, kopia);
            for action in &always_present {
                assert!(
                    Item::find(&plan.items, action).is_some(),
                    "{action:?} disappeared in the {name} state (kopia present: {kopia})"
                );
            }
            // Quit is never disabled: a user must always be able to leave.
            assert!(
                Item::find(&plan.items, &Action::Quit).is_some_and(|i| i.is_enabled()),
                "Quit was disabled in the {name} state"
            );
        }
    }
}

/// Every disabled run item says why, in words a screen reader can read.
#[test]
fn a_disabled_item_always_gives_its_reason() {
    let mut config = Config::default();
    config.jobs.push(test_job("dev code"));

    let cases: Vec<(&str, StatusSnapshot, bool)> = vec![
        (
            "vault locked",
            StatusSnapshot { unlocked: false, health: Health::Attention, ..snapshot() },
            true,
        ),
        ("kopia not found", snapshot(), false),
    ];

    for (expected, snap, kopia) in cases {
        let plan = menu::plan(&snap, &config, kopia);
        match Item::find(&plan.items, &Action::RunAll).expect("Back up now must stay visible") {
            Item::Entry { label, enabled, reason, .. } => {
                assert!(!enabled, "expected it to be disabled for `{expected}`");
                assert_eq!(reason.as_deref(), Some(expected));
                assert!(label.contains(expected), "the label must say why: {label}");
            }
            other => panic!("unexpected item {other:?}"),
        }
    }
}

/// An icon stem matches the name the core publishes, so the two cannot drift.
#[test]
fn icon_stems_come_from_the_cores_own_names() {
    for health in
        [Health::Idle, Health::Running, Health::Attention, Health::Paused, Health::Failed]
    {
        let stem = IconKey::new(health, Variant::DarkTaskbar, 0).stem();
        assert!(
            stem.starts_with(health.icon_stem()),
            "{stem} does not start with {}",
            health.icon_stem()
        );
    }
}
