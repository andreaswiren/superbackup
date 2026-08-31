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
            replicated_from: None,
            skipped_reason: None,
        }],
    }
}

/// The precedence order in §7.3, checked against the core's own rule.
#[test]
fn health_precedence_is_the_daemons_and_never_the_trays() {
    // Failed beats everything, including a run in progress.
    assert_eq!(StatusSnapshot::derive_health(true, true, true, true, true), Health::Failed);
    // Running beats paused: a backup that is happening is what the user wants
    // to see, even if schedules are off.
    assert_eq!(StatusSnapshot::derive_health(true, true, true, false, true), Health::Running);
    // Paused beats attention.
    assert_eq!(StatusSnapshot::derive_health(false, true, false, false, true), Health::Paused);
    // A locked vault is attention.
    assert_eq!(StatusSnapshot::derive_health(false, false, false, false, false), Health::Attention);
    // A stale job is attention too.
    assert_eq!(StatusSnapshot::derive_health(true, false, false, false, true), Health::Attention);
    // Everything fine.
    assert_eq!(StatusSnapshot::derive_health(true, false, false, false, false), Health::Idle);
}

/// Every health value produces a distinct, non-blank mark in both Windows
/// variants and in the macOS template.
#[test]
fn every_health_has_its_own_mark_in_every_variant() {
    let states = [Health::Idle, Health::Running, Health::Attention, Health::Paused, Health::Failed];
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
    let states = [Health::Idle, Health::Running, Health::Attention, Health::Paused, Health::Failed];
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
    assert!(
        plan.tooltip_detail.contains("Last backup")
            || plan.tooltip_detail.contains("in about 3 hours"),
        "{}",
        plan.tooltip_detail
    );

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
    snap.jobs
        .insert(jobs[0], JobSummary { last_status: Some(RunStatus::Failed), ..Default::default() });
    let plan = menu::plan(&snap, &config, true);
    assert_eq!(plan.tooltip_title, "superbackup — Backup failed");
    assert!(plan.tooltip_detail.contains("dev code"), "{}", plan.tooltip_detail);

    // Locked, which takes precedence over a stale job.
    let mut snap = snapshot();
    snap.health = Health::Attention;
    snap.unlocked = false;
    snap.jobs.insert(
        jobs[1],
        JobSummary { last_success: Some(Utc::now() - Duration::days(90)), ..Default::default() },
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

    let always_present =
        [Action::RunAll, Action::OpenApp, Action::OpenActivity, Action::OpenSettings, Action::Quit];

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
    for health in [Health::Idle, Health::Running, Health::Attention, Health::Paused, Health::Failed]
    {
        let stem = IconKey::new(health, Variant::DarkTaskbar, 0).stem();
        assert!(
            stem.starts_with(health.icon_stem()),
            "{stem} does not start with {}",
            health.icon_stem()
        );
    }
}

/// The knockout no longer eats the arc's round terminals.
///
/// §7.1 asked for a 280° sweep with round caps *and* a 1.5-unit knockout, and
/// those contradict: the knockout disc cut the ring over 2°–88° while the gap
/// was only 5°–85°, so both round ends were sliced into flat crescents. The
/// gap is now derived from the pip rather than fixed, and this is the
/// relationship that derivation has to keep.
#[test]
fn the_round_caps_survive_the_knockout() {
    for profile in [tray::icons::LARGE, tray::icons::SMALL] {
        let (start, sweep) = profile.arc_span();

        // Where the knockout disc actually crosses the ring.
        let pip_distance = ((23.0f32 - 16.0).powi(2) + (23.0f32 - 16.0).powi(2)).sqrt();
        let knockout = profile.pip_radius + 1.5;
        let ring = 10.5f32;
        let cosine =
            (pip_distance.powi(2) + ring.powi(2) - knockout.powi(2)) / (2.0 * pip_distance * ring);
        let knockout_half = cosine.clamp(-1.0, 1.0).acos().to_degrees();

        // The arc's own terminals, plus the half-stroke its round cap adds.
        let cap_half = ((3.0f32 / 2.0) / ring).asin().to_degrees();
        let gap_half = (360.0 - sweep) / 2.0;

        assert!(
            gap_half > knockout_half + cap_half,
            "the knockout ({knockout_half:.1}°) plus a round cap ({cap_half:.1}°) is wider \
             than half the gap ({gap_half:.1}°): the arc's ends are being truncated"
        );
        // And the gap is still centred down-right, as §7.1 requires.
        let gap_centre = (start + sweep + gap_half) % 360.0;
        assert!(
            (gap_centre - 45.0).abs() < 0.5,
            "the gap drifted off the down-right diagonal: centred at {gap_centre:.1}°"
        );
    }
}

/// The travelling arc never seals the ring.
///
/// A closed ring is `idle` and `paused`'s distinguishing feature. The moving
/// arc used to be drawn on the full circle, so on the frames where it crossed
/// the gap `running` briefly wore another state's silhouette.
#[test]
fn the_travelling_arc_never_closes_the_ring() {
    for size in [16u32, 32] {
        for frame in 0..RUNNING_FRAMES {
            let key = IconKey::new(Health::Running, Variant::DarkTaskbar, frame);
            let rgba = tray::icons::rasterise(key, size).expect("render");
            // Sample the gap's centre — the down-right diagonal, on the ring's
            // own radius. Nothing may ever be painted there.
            let scale = size as f32 / 32.0;
            let angle = 45.0f32.to_radians();
            let x = ((16.0 + 10.5 * angle.cos()) * scale) as usize;
            let y = ((16.0 + 10.5 * angle.sin()) * scale) as usize;
            let alpha = rgba[(y * size as usize + x) * 4 + 3];
            assert!(
                alpha < 64,
                "at {size}px, frame {frame} paints the gap (alpha {alpha}) — the silhouette \
                 is a closed ring, which is what `idle` and `paused` mean"
            );
        }
    }
}

/// The running mark keeps its ring on both taskbars.
///
/// §7.2 fixed the base arc at `#3A4250`, which is 1.61:1 on a dark taskbar —
/// so the ring vanished and only the moving arc was left, and `running`
/// stopped sharing the silhouette that makes the five marks one family.
#[test]
fn the_running_base_arc_is_visible_on_both_taskbars() {
    for (variant, taskbar) in [
        (Variant::LightTaskbar, [0xF3u8, 0xF3, 0xF3]),
        (Variant::DarkTaskbar, [0x20u8, 0x20, 0x20]),
    ] {
        let rgba =
            tray::icons::rasterise(IconKey::new(Health::Running, variant, 6), 32).expect("render");
        let (start, sweep) = tray::icons::LARGE.arc_span();

        let mut painted = 0;
        let mut contrasting = 0;
        for step in 0..48 {
            let angle = (start + sweep * (step as f32 / 47.0)).to_radians();
            let x = (16.0 + 10.5 * angle.cos()).round() as usize;
            let y = (16.0 + 10.5 * angle.sin()).round() as usize;
            let px = &rgba[(y * 32 + x) * 4..][..4];
            if px[3] < 128 {
                continue;
            }
            painted += 1;
            if contrast(&px[..3], &taskbar) >= 3.0 {
                contrasting += 1;
            }
        }
        assert!(painted >= 40, "the ring is barely drawn at all: {painted}/48 samples");
        assert_eq!(
            contrasting, painted,
            "on a {variant:?} taskbar only {contrasting}/{painted} painted samples of the \
             running ring reach 3:1 — part of the base arc is invisible and the state has \
             lost its silhouette"
        );
    }
}

/// WCAG 2.1 relative luminance.
fn luminance(rgb: &[u8]) -> f32 {
    let channel = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2])
}

fn contrast(a: &[u8], b: &[u8]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Write a contact sheet of every mark, at every size, on both taskbars, in
/// colour and in greyscale, plus the macOS template and its inversion.
///
/// Not an assertion — it is the tool that found the problems the numeric tests
/// now guard, and the only way to check a *drawing* is to look at it. Run with
/// `cargo test -p superbackup --test tray_state -- --ignored contact_sheet`;
/// the path is printed.
#[test]
#[ignore = "writes a PNG for a human to look at"]
fn contact_sheet() {
    const SIZES: [u32; 4] = [16, 20, 24, 32];
    const CELL: u32 = 44;
    let states = [Health::Idle, Health::Running, Health::Attention, Health::Paused, Health::Failed];

    // (background, greyscale?, variant, invert?)
    let rows: [([u8; 3], bool, Variant, bool); 6] = [
        ([0xF3, 0xF3, 0xF3], false, Variant::LightTaskbar, false),
        ([0xF3, 0xF3, 0xF3], true, Variant::LightTaskbar, false),
        ([0x20, 0x20, 0x20], false, Variant::DarkTaskbar, false),
        ([0x20, 0x20, 0x20], true, Variant::DarkTaskbar, false),
        ([0xF3, 0xF3, 0xF3], false, Variant::Template, false),
        ([0x20, 0x20, 0x20], false, Variant::Template, true),
    ];

    let width = states.len() as u32 * SIZES.len() as u32 * CELL;
    let height = rows.len() as u32 * CELL;
    let mut canvas = vec![0u8; (width * height * 3) as usize];

    for (row, (background, grey, variant, invert)) in rows.iter().enumerate() {
        for (state_index, health) in states.iter().enumerate() {
            for (size_index, size) in SIZES.iter().enumerate() {
                let rgba = tray::icons::rasterise(IconKey::new(*health, *variant, 6), *size)
                    .expect("render");
                let cell_x = (state_index * SIZES.len() + size_index) as u32 * CELL;
                let cell_y = row as u32 * CELL;
                let inset = (CELL - size) / 2;

                for y in 0..CELL {
                    for x in 0..CELL {
                        let mut px = *background;
                        let (ix, iy) = (x as i64 - inset as i64, y as i64 - inset as i64);
                        if ix >= 0 && iy >= 0 && (ix as u32) < *size && (iy as u32) < *size {
                            let s = ((iy as u32 * size + ix as u32) * 4) as usize;
                            let (fg, alpha) = if *invert {
                                ([0xFFu8, 0xFF, 0xFF], rgba[s + 3])
                            } else {
                                ([rgba[s], rgba[s + 1], rgba[s + 2]], rgba[s + 3])
                            };
                            let a = alpha as f32 / 255.0;
                            for c in 0..3 {
                                px[c] = (fg[c] as f32 * a + px[c] as f32 * (1.0 - a)).round() as u8;
                            }
                        }
                        if *grey {
                            let y8 = (0.299 * px[0] as f32
                                + 0.587 * px[1] as f32
                                + 0.114 * px[2] as f32)
                                .round() as u8;
                            px = [y8, y8, y8];
                        }
                        let o = (((cell_y + y) * width + cell_x + x) * 3) as usize;
                        canvas[o..o + 3].copy_from_slice(&px);
                    }
                }
            }
        }
    }

    let out = std::env::temp_dir().join("superbackup-tray-contact-sheet.png");
    image::save_buffer(&out, &canvas, width, height, image::ExtendedColorType::Rgb8)
        .expect("write the contact sheet");
    eprintln!("contact sheet: {}", out.display());
}

/// Every mark at 16 px and 20 px, magnified 8× with no smoothing, so the
/// actual pixels can be counted.
///
/// A contact sheet shows whether a set reads; this shows *why*. It is what the
/// small profile was tuned against — the exclamation's bar and dot had to stay
/// two shapes, and the cross had to stay four arms, at the size where one
/// canvas unit is half a pixel.
#[test]
#[ignore = "writes a PNG for a human to look at"]
fn pixel_check() {
    const ZOOM: u32 = 8;
    const PAD: u32 = 4;
    let states = [Health::Idle, Health::Running, Health::Attention, Health::Paused, Health::Failed];
    let rows: [(u32, Variant, [u8; 3]); 4] = [
        (16, Variant::LightTaskbar, [0xF3, 0xF3, 0xF3]),
        (16, Variant::DarkTaskbar, [0x20, 0x20, 0x20]),
        (16, Variant::Template, [0xF3, 0xF3, 0xF3]),
        (20, Variant::DarkTaskbar, [0x20, 0x20, 0x20]),
    ];

    let cell = 20 * ZOOM + PAD * 2;
    let width = states.len() as u32 * cell;
    let height = rows.len() as u32 * cell;
    let mut canvas = vec![0u8; (width * height * 3) as usize];

    for (row, (size, variant, background)) in rows.iter().enumerate() {
        for (column, health) in states.iter().enumerate() {
            let rgba =
                tray::icons::rasterise(IconKey::new(*health, *variant, 6), *size).expect("render");
            let origin_x = column as u32 * cell;
            let origin_y = row as u32 * cell;
            for y in 0..cell {
                for x in 0..cell {
                    let mut px = *background;
                    let (sx, sy) = (x as i64 - PAD as i64, y as i64 - PAD as i64);
                    if sx >= 0 && sy >= 0 {
                        let (sx, sy) = (sx as u32 / ZOOM, sy as u32 / ZOOM);
                        if sx < *size && sy < *size {
                            let s = ((sy * size + sx) * 4) as usize;
                            let a = rgba[s + 3] as f32 / 255.0;
                            for c in 0..3 {
                                px[c] = (rgba[s + c] as f32 * a + px[c] as f32 * (1.0 - a)).round()
                                    as u8;
                            }
                        }
                    }
                    let o = (((origin_y + y) * width + origin_x + x) * 3) as usize;
                    canvas[o..o + 3].copy_from_slice(&px);
                }
            }
        }
    }

    let out = std::env::temp_dir().join("superbackup-tray-pixelcheck.png");
    image::save_buffer(&out, &canvas, width, height, image::ExtendedColorType::Rgb8)
        .expect("write the pixel check");
    eprintln!("pixel check: {}", out.display());
}
