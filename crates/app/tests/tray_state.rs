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
//! It also checks the mark itself. Every tray icon is the **application mark**
//! with a status badge in a well knocked out of its bottom-right corner, so
//! there are two things to hold true at once and both of them break at 16 px
//! before they break anywhere else:
//!
//! * the badge still reads as its state — `each_badge_is_a_distinct_silhouette`,
//!   `the_marks_are_distinct_in_greyscale_at_every_tray_size`,
//!   `attention_and_failed_are_told_apart_by_shape_at_16px`;
//! * the mark still reads as the mark — `the_base_mark_is_common_to_all_five_states`,
//!   `the_mark_is_two_pieces_at_every_size`, `the_badge_never_touches_the_mark`.
//!
//! All of it in greyscale and in the macOS template as well as in colour,
//! because "one man in twelve" cannot tell red from green and macOS discards
//! colour outright: colour is confirmation, never the message.
//!
//! The `contact_sheet`, `pixel_check` and `running_frames` generators at the
//! bottom are `#[ignore]`d and write PNGs. They are not decoration — every
//! number in `tray/icons.rs` was chosen by looking at their output, and an icon
//! nobody has viewed at 16 px is not finished.

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

// `daemon::handler` reports which build it is, so the module that
// answers that has to exist in this synthetic crate too.
#[path = "../src/build.rs"]
mod build;
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
        machine_hostname: "pc".into(),
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
            notes: Vec::new(),
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

// ---------------------------------------------------------------------------
// Pixel helpers for the mark tests
// ---------------------------------------------------------------------------

const STATES: [Health; 5] =
    [Health::Idle, Health::Running, Health::Attention, Health::Paused, Health::Failed];
const VARIANTS: [Variant; 3] = [Variant::LightTaskbar, Variant::DarkTaskbar, Variant::Template];
/// The sizes a notification area actually asks for: `SM_CXSMICON` at 100 %,
/// 125 %, 150 % and 200 % DPI.
const TRAY_SIZES: [u32; 4] = [16, 20, 24, 32];

/// Rec. 601 luma plus alpha — exactly what a greyscale printout or a
/// monochrome taskbar would show.
fn greyscale(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|p| {
            let y = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).round() as u8;
            [y, p[3]]
        })
        .collect()
}

/// The pixels inside the badge's clear-space disc, as indices into an RGBA
/// buffer of `size` × `size`.
fn badge_region(size: u32) -> Vec<usize> {
    let scale = size as f32 / tray::icons::CANVAS;
    let reach = tray::icons::BADGE_RADIUS + tray::icons::BADGE_CLEARANCE;
    let mut out = Vec::new();
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 + 0.5) / scale - tray::icons::BADGE_X;
            let dy = (y as f32 + 0.5) / scale - tray::icons::BADGE_Y;
            if dx.hypot(dy) <= reach {
                out.push((y * size + x) as usize);
            }
        }
    }
    out
}

/// How many four-connected opaque regions the bitmap contains.
///
/// The single most useful measurement in this file: it is what proved that a
/// 25.5-unit mark is severed into three fragments at 16 px, and that a
/// 2.4-unit clear space lets the badge fuse into the mark.
fn opaque_regions(rgba: &[u8], size: u32, threshold: u8) -> usize {
    let size = size as usize;
    let solid = |i: usize| rgba[i * 4 + 3] > threshold;
    let mut seen = vec![false; size * size];
    let mut regions = 0;
    for start in 0..size * size {
        if !solid(start) || seen[start] {
            continue;
        }
        regions += 1;
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % size, i / size);
            let mut visit = |nx: usize, ny: usize| {
                let j = ny * size + nx;
                if solid(j) && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(x - 1, y);
            }
            if x + 1 < size {
                visit(x + 1, y);
            }
            if y > 0 {
                visit(x, y - 1);
            }
            if y + 1 < size {
                visit(x, y + 1);
            }
        }
    }
    regions
}

/// The marks survive being desaturated — shape, not colour, carries the state.
///
/// Checked at every size the tray asks for and in all three variants, not only
/// at 32 px on a dark taskbar. 16 px is where a colour-only distinction shows
/// up, and the macOS template is where colour is gone entirely.
#[test]
fn the_marks_are_distinct_in_greyscale_at_every_tray_size() {
    for variant in VARIANTS {
        for size in TRAY_SIZES {
            let rendered: Vec<(Health, Vec<u8>)> = STATES
                .iter()
                .map(|health| {
                    let rgba = tray::icons::rasterise(IconKey::new(*health, variant, 6), size)
                        .expect("render");
                    (*health, greyscale(&rgba))
                })
                .collect();

            for (i, (a_health, a)) in rendered.iter().enumerate() {
                for (b_health, b) in rendered.iter().skip(i + 1) {
                    // "Differs" means differs enough for an eye to see: a
                    // one-level difference in an antialiased edge is not a
                    // distinction anybody can act on.
                    let differing =
                        a.iter().zip(b.iter()).filter(|(x, y)| x.abs_diff(**y) > 24).count();
                    assert!(
                        differing >= 16,
                        "{a_health:?} and {b_health:?} differ in only {differing} greyscale \
                         samples at {size}px in {variant:?}; the distinction is being carried \
                         by colour alone"
                    );
                }
            }
        }
    }
}

/// Every state's badge is a different *silhouette*, not a different colour.
///
/// Measured on alpha alone, inside the badge's own clear-space disc, so the
/// mark — which is identical in all five — cannot flatter the result. The
/// measured floor is 12.8 % of the badge region (`paused` against `failed`, at
/// 24 px and 32 px); the bar is set at 8 % so a small tuning change does not
/// fail the build, and a real regression does.
#[test]
fn each_badge_is_a_distinct_silhouette() {
    for variant in VARIANTS {
        for size in TRAY_SIZES {
            let region = badge_region(size);
            let rendered: Vec<(Health, Vec<u8>)> = STATES
                .iter()
                .map(|health| {
                    (
                        *health,
                        tray::icons::rasterise(IconKey::new(*health, variant, 6), size)
                            .expect("render"),
                    )
                })
                .collect();

            for (i, (a_health, a)) in rendered.iter().enumerate() {
                for (b_health, b) in rendered.iter().skip(i + 1) {
                    let differing = region
                        .iter()
                        .filter(|&&p| a[p * 4 + 3].abs_diff(b[p * 4 + 3]) > 96)
                        .count();
                    let share = differing as f32 / region.len() as f32;
                    assert!(
                        share >= 0.08,
                        "at {size}px in {variant:?}, {a_health:?} and {b_health:?} differ over \
                         only {:.1}% of the badge; their silhouettes have converged",
                        share * 100.0
                    );
                }
            }
        }
    }
}

/// `attention` and `failed` specifically, at 16 px specifically.
///
/// This is the pair the previous design lost: a 2.4-unit glyph inside a
/// 6-unit pip is 1.2 px at 16 px, so an exclamation and a cross both rendered
/// as an indistinct smear told apart only by the smear's lightness — and in
/// the template, where both were the same hole, not told apart at all. They
/// are now a solid triangle and a cross, which differ in silhouette before
/// they differ in anything else.
#[test]
fn attention_and_failed_are_told_apart_by_shape_at_16px() {
    let region = badge_region(16);
    for variant in VARIANTS {
        let attention = tray::icons::rasterise(IconKey::new(Health::Attention, variant, 0), 16)
            .expect("render");
        let failed =
            tray::icons::rasterise(IconKey::new(Health::Failed, variant, 0), 16).expect("render");

        // Alpha only: this must hold with every scrap of colour discarded.
        let differing = region
            .iter()
            .filter(|&&p| attention[p * 4 + 3].abs_diff(failed[p * 4 + 3]) > 96)
            .count();
        assert!(
            differing >= 8,
            "attention and failed differ in only {differing} of {} badge pixels at 16px in \
             {variant:?}, on alpha alone",
            region.len()
        );
    }
}

/// The application mark is identical in all five states.
///
/// This is what makes the set read as *one application* rather than five
/// unrelated icons, and it is the whole point of rebuilding the tray on the
/// brand mark. Outside the badge's clear-space disc every state must be the
/// same picture, pixel for pixel — which is also why the well is knocked out
/// of `idle` too, even though `idle` could have kept its corner.
#[test]
fn the_base_mark_is_common_to_all_five_states() {
    for variant in VARIANTS {
        for size in TRAY_SIZES {
            let badge: std::collections::HashSet<usize> = badge_region(size).into_iter().collect();
            let reference =
                tray::icons::rasterise(IconKey::new(Health::Idle, variant, 0), size).expect("ref");
            for health in STATES {
                let rgba =
                    tray::icons::rasterise(IconKey::new(health, variant, 6), size).expect("render");
                for pixel in 0..(size * size) as usize {
                    if badge.contains(&pixel) {
                        continue;
                    }
                    assert_eq!(
                        &rgba[pixel * 4..pixel * 4 + 4],
                        &reference[pixel * 4..pixel * 4 + 4],
                        "{health:?} differs from idle outside the badge at pixel {pixel} \
                         ({size}px, {variant:?}): the five marks are no longer one application"
                    );
                }
            }
        }
    }
}

/// The mark is two congruent pieces, and stays two pieces once rasterised.
///
/// The badge's clear space is a disc bitten out of the lower half's inner
/// corner. Push it too far and that half is cut into two fragments — the mark
/// then reads as three unrelated blocks, and "one square, cut in two, the
/// second piece slid clear" is gone. A 25.5-unit mark is connected in the
/// vector and **three fragments at 16 px and 20 px**; 24.5 is the span that
/// holds, and this is the test that found it.
#[test]
fn the_mark_is_two_pieces_at_every_size() {
    for size in TRAY_SIZES {
        // The template is the strict case: the mark is a single flat ink, so
        // nothing but geometry can be holding the pieces apart.
        let rgba = tray::icons::rasterise(IconKey::new(Health::Idle, Variant::Template, 0), size)
            .expect("render");
        // Blank out the badge so only the mark is counted.
        let mut mark = rgba.clone();
        for pixel in badge_region(size) {
            mark[pixel * 4 + 3] = 0;
        }
        for threshold in [96u8, 128] {
            let regions = opaque_regions(&mark, size, threshold);
            assert_eq!(
                regions, 2,
                "the mark is {regions} piece(s) at {size}px (alpha > {threshold}), not the two \
                 congruent halves it is built from"
            );
        }
    }
}

/// The badge never touches the mark.
///
/// The clear space is a real cutout in the alpha, so this has to hold in the
/// macOS template — where the mark and the badge are the same black and only
/// the gap tells them apart. A 2.4-unit clear space fails here at 16 px;
/// 2.8 passes at every size, state, variant and threshold.
#[test]
fn the_badge_never_touches_the_mark() {
    // Two halves of the mark, plus the badge — and `paused`'s badge is itself
    // two bars.
    let expected = |health: Health| if health == Health::Paused { 4 } else { 3 };
    for size in TRAY_SIZES {
        for health in STATES {
            let rgba = tray::icons::rasterise(IconKey::new(health, Variant::Template, 6), size)
                .expect("render");
            for threshold in [96u8, 110, 128] {
                let regions = opaque_regions(&rgba, size, threshold);
                assert_eq!(
                    regions,
                    expected(health),
                    "{health:?} at {size}px (alpha > {threshold}) has {regions} separate \
                     shapes, not {}: the badge has fused into the mark",
                    expected(health)
                );
            }
        }
    }
}

/// The reference SVGs in `assets/tray/` are the same drawing the program makes.
///
/// `assets/tray/README.md` has always *claimed* that the checked-in files and
/// `tray/icons.rs` cannot quietly diverge, because `tools/icons/geometry.py`
/// mirrors the module. Claiming it is not the same as knowing it: the two did
/// diverge once already, over whether the macOS template's glyph was painted or
/// punched, and the divergence was found by reading rather than by failing.
/// This rasterises both and compares the pixels, at the size the difference
/// would matter at.
#[test]
fn the_checked_in_reference_svgs_match_what_the_program_draws() {
    let tray = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("tray");
    if !tray.is_dir() {
        // A source checkout always has these; a vendored build may not.
        return;
    }

    let render = |svg: &str, size: u32| -> Vec<u8> {
        let tree =
            resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).expect("parse");
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("pixmap");
        let scale = size as f32 / tray::icons::CANVAS;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        pixmap.take()
    };

    let mut checked = 0;
    for variant in VARIANTS {
        for health in STATES {
            let frames = if health == Health::Running { RUNNING_FRAMES } else { 1 };
            for frame in 0..frames {
                let key = IconKey::new(health, variant, frame);
                let file = tray.join(format!("{}.svg", key.stem()));
                let reference = std::fs::read_to_string(&file)
                    .unwrap_or_else(|e| panic!("{} is missing: {e}", file.display()));

                for size in [16u32, 32] {
                    assert_eq!(
                        render(&reference, size),
                        render(&tray::icons::svg(key), size),
                        "{} does not match what tray/icons.rs draws at {size}px — run \
                         `python tools/icons/build.py`",
                        key.stem()
                    );
                }
                checked += 1;
            }
        }
    }
    // Five states × three variants, with `running` counted twelve times each.
    assert_eq!(checked, 3 * (4 + RUNNING_FRAMES));
}

/// Every badge ink clears 3:1 against the taskbar it is drawn on.
///
/// SC 1.4.11 governs graphics that carry meaning, and the badge is the only
/// thing carrying the state. The previous design fixed `#E0A83A` and `#C2313A`
/// for both taskbars: 1.92:1 on light and 2.94:1 on dark respectively — the
/// second being the *failure* state. Each ink is now chosen per variant.
#[test]
fn every_badge_ink_clears_three_to_one_on_its_own_taskbar() {
    let cases = [
        (Variant::LightTaskbar, [0xF3u8, 0xF3, 0xF3]),
        (Variant::DarkTaskbar, [0x20u8, 0x20, 0x20]),
    ];
    for (variant, taskbar) in cases {
        for health in STATES {
            let ink = variant.badge_ink(health);
            let rgb = [
                u8::from_str_radix(&ink[1..3], 16).unwrap(),
                u8::from_str_radix(&ink[3..5], 16).unwrap(),
                u8::from_str_radix(&ink[5..7], 16).unwrap(),
            ];
            let ratio = contrast(&rgb, &taskbar);
            assert!(
                ratio >= 3.0,
                "{health:?}'s badge ink {ink} is {ratio:.2}:1 on a {variant:?} taskbar, under \
                 SC 1.4.11's 3:1 — the shape carrying the state is not visible"
            );
        }
        // And the mark itself.
        let ink = variant.mark_ink();
        let rgb = [
            u8::from_str_radix(&ink[1..3], 16).unwrap(),
            u8::from_str_radix(&ink[3..5], 16).unwrap(),
            u8::from_str_radix(&ink[5..7], 16).unwrap(),
        ];
        assert!(contrast(&rgb, &taskbar) >= 3.0, "the mark ink {ink} fails on {variant:?}");
    }
}

/// The running animation is twelve distinct frames, and only running animates.
///
/// Checked at 16 px as well as 32, and in the template as well as on a
/// taskbar: the gap that travels round the ring is 3.4 units of arc, and an
/// animation whose frames collapse into each other once rasterised is a
/// still picture that costs a redraw eight times a second. The template
/// matters because the animation there is carried by alpha alone — macOS has
/// discarded the colour.
#[test]
fn only_the_running_state_animates_and_it_has_twelve_frames() {
    for variant in VARIANTS {
        for size in [16u32, 32] {
            let mut frames = Vec::new();
            for frame in 0..RUNNING_FRAMES {
                frames.push(
                    tray::icons::rasterise(IconKey::new(Health::Running, variant, frame), size)
                        .expect("render a frame"),
                );
            }
            for (i, a) in frames.iter().enumerate() {
                for (j, b) in frames.iter().enumerate().skip(i + 1) {
                    assert_ne!(
                        a, b,
                        "running frames {i} and {j} are identical at {size}px in {variant:?}"
                    );
                }
            }
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

// The three tests that used to live here — `the_round_caps_survive_the_knockout`,
// `the_travelling_arc_never_closes_the_ring` and
// `the_running_base_arc_is_visible_on_both_taskbars` — guarded an abstract ring
// with a status pip that has been replaced by the application mark with a status
// badge. There is no arc, no pip and no knockout of an arc any more, so they were
// removed rather than rewritten to assert nothing. What they were really
// protecting has successors above: that a state never wears another state's
// silhouette is now `each_badge_is_a_distinct_silhouette`, and that the shared
// part of the mark is genuinely shared is `the_base_mark_is_common_to_all_five_states`.

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

/// Every mark at 16 px, magnified 9× with no smoothing, so the actual pixels
/// can be counted — on both taskbars, in colour and in greyscale, plus the
/// macOS template and its inversion, and a 20 px row for the 125 % DPI case.
///
/// A contact sheet shows whether a set reads; this shows *why*. The greyscale
/// and template rows are where a colour-only distinction shows up, and they
/// are what every proportion in `tray/icons.rs` was tuned against: the badge
/// silhouettes had to stay five different shapes at the size where one canvas
/// unit is half a pixel.
///
/// Run with
/// `cargo test -p superbackup --test tray_state -- --ignored pixel_check`.
#[test]
#[ignore = "writes a PNG for a human to look at"]
fn pixel_check() {
    const ZOOM: u32 = 9;
    const PAD: u32 = 3;
    const LIGHT: [u8; 3] = [0xF3, 0xF3, 0xF3];
    const DARK: [u8; 3] = [0x20, 0x20, 0x20];

    // (size, variant, background, greyscale?, invert?)
    let rows: [(u32, Variant, [u8; 3], bool, bool); 7] = [
        (16, Variant::LightTaskbar, LIGHT, false, false),
        (16, Variant::LightTaskbar, LIGHT, true, false),
        (16, Variant::DarkTaskbar, DARK, false, false),
        (16, Variant::DarkTaskbar, DARK, true, false),
        (16, Variant::Template, LIGHT, false, false),
        (16, Variant::Template, DARK, false, true),
        (20, Variant::DarkTaskbar, DARK, false, false),
    ];

    let cell = 20 * ZOOM + PAD * 2;
    let width = STATES.len() as u32 * cell;
    let height = rows.len() as u32 * cell;
    let mut canvas = vec![0u8; (width * height * 3) as usize];

    for (row, (size, variant, background, grey, invert)) in rows.iter().enumerate() {
        for (column, health) in STATES.iter().enumerate() {
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
                            let fg = if *invert {
                                [0xFFu8, 0xFF, 0xFF]
                            } else {
                                [rgba[s], rgba[s + 1], rgba[s + 2]]
                            };
                            let a = rgba[s + 3] as f32 / 255.0;
                            for c in 0..3 {
                                px[c] = (fg[c] as f32 * a + px[c] as f32 * (1.0 - a)).round() as u8;
                            }
                        }
                    }
                    if *grey {
                        let y8 =
                            (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32)
                                .round() as u8;
                        px = [y8, y8, y8];
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

/// The twelve running frames at 16 px, magnified, on a dark taskbar and as a
/// template.
///
/// The animation is carried by *alpha* — the ring's gap travels round — so it
/// has to be checked in the template too, where colour is gone. This is also
/// the sheet that shows the frames are genuinely twelve different pictures at
/// 16 px rather than at 32 px only.
#[test]
#[ignore = "writes a PNG for a human to look at"]
fn running_frames() {
    const ZOOM: u32 = 7;
    const PAD: u32 = 3;
    const SIZE: u32 = 16;
    let rows: [(Variant, [u8; 3]); 2] =
        [(Variant::DarkTaskbar, [0x20, 0x20, 0x20]), (Variant::Template, [0xF3, 0xF3, 0xF3])];

    let cell = SIZE * ZOOM + PAD * 2;
    let width = RUNNING_FRAMES as u32 * cell;
    let height = rows.len() as u32 * cell;
    let mut canvas = vec![0u8; (width * height * 3) as usize];

    for (row, (variant, background)) in rows.iter().enumerate() {
        for frame in 0..RUNNING_FRAMES {
            let rgba = tray::icons::rasterise(IconKey::new(Health::Running, *variant, frame), SIZE)
                .expect("render");
            let origin_x = frame as u32 * cell;
            let origin_y = row as u32 * cell;
            for y in 0..cell {
                for x in 0..cell {
                    let mut px = *background;
                    let (sx, sy) = (x as i64 - PAD as i64, y as i64 - PAD as i64);
                    if sx >= 0 && sy >= 0 {
                        let (sx, sy) = (sx as u32 / ZOOM, sy as u32 / ZOOM);
                        if sx < SIZE && sy < SIZE {
                            let s = ((sy * SIZE + sx) * 4) as usize;
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

    let out = std::env::temp_dir().join("superbackup-tray-running.png");
    image::save_buffer(&out, &canvas, width, height, image::ExtendedColorType::Rgb8)
        .expect("write the running strip");
    eprintln!("running frames: {}", out.display());
}
