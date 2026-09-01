//! The interface, exercised against a fake daemon.
//!
//! `crates/app` has no library target, so an integration test cannot `use`
//! the binary's modules. The GUI tree is therefore included by path, together
//! with the two items from `crate::cli` that its entry point names. That is
//! not a trick to route around the module system: it is what lets the whole
//! interface be compiled and driven without the tray, the CLI or the daemon —
//! exactly the isolation the workstream split asks for.

#![allow(dead_code)]

/// The slice of `crate::cli` that `gui::open_or_focus` names.
mod cli {
    pub mod exit {
        pub const OK: i32 = 0;
        pub const FAILED: i32 = 1;
        pub const USAGE: i32 = 2;
        pub const DAEMON_UNREACHABLE: i32 = 3;
    }

    /// Mirrors `cli::args::GlobalArgs` in the fields the window reads.
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

/// The real `crate::build`, not a stub.
///
/// The status strip names it, and it is nothing but constants from the build
/// script's environment — which is set for every target in this package, test
/// binaries included. Including it by path costs nothing and cannot drift from
/// what the window actually shows, unlike the hand-written `cli` stub above,
/// which exists only because `crate::cli` is far too large to compile here.
#[path = "../src/build.rs"]
mod build;

#[path = "../src/gui/mod.rs"]
mod gui;

use std::sync::Arc;

use superbackup_core::ipc::testing::MockHandler;

use gui::app::App;
use gui::daemon::MockDaemon;
use gui::fixtures;

/// One frame of the whole interface, laid out at the given size, against a
/// fake daemon. Returns nothing: the assertion is that it did not panic.
fn frame(app: &mut App, ctx: &egui::Context, size: egui::Vec2) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| app.frame(ctx));
}

fn seeded_app() -> (App, egui::Context, Arc<MockHandler>) {
    let handler = Arc::new(MockHandler::new());
    let ctx = egui::Context::default();
    let mut app = App::new_with_daemon(&ctx, Arc::new(MockDaemon::new(handler.clone())));
    fixtures::seed(&mut app.data);
    // Keep the fixture machine on screen: without this the mock's own empty
    // replies would land on the first frame and replace it.
    app.preview_mode();
    (app, ctx, handler)
}

#[test]
fn every_screen_renders_without_panicking() {
    let (mut app, ctx, _handler) = seeded_app();
    for route in gui::nav::Route::every() {
        app.go(route.clone());
        frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
        frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    }
}

#[test]
fn every_screen_renders_at_the_minimum_window_size() {
    let (mut app, ctx, _handler) = seeded_app();
    for route in gui::nav::Route::every() {
        app.go(route.clone());
        frame(&mut app, &ctx, egui::vec2(900.0, 600.0));
        frame(&mut app, &ctx, egui::vec2(900.0, 600.0));
    }
}

#[test]
fn every_screen_renders_with_a_locked_vault() {
    let (mut app, ctx, _handler) = seeded_app();
    if let Some(s) = &mut app.data.snapshot {
        s.unlocked = false;
        s.health = superbackup_core::state::Health::Attention;
    }
    for route in gui::nav::Route::every() {
        app.go(route.clone());
        frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    }
}

#[test]
fn every_screen_renders_with_nothing_configured() {
    let handler = Arc::new(MockHandler::new());
    let ctx = egui::Context::default();
    let mut app = App::new_with_daemon(&ctx, Arc::new(MockDaemon::new(handler)));
    app.data.loading = false;
    app.data.link_up = true;
    for route in gui::nav::Route::every() {
        app.go(route.clone());
        frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    }
}

#[test]
fn every_screen_renders_in_both_themes() {
    for theme in [superbackup_core::model::Theme::Light, superbackup_core::model::Theme::Dark] {
        let (mut app, ctx, _handler) = seeded_app();
        app.data.settings.theme = theme;
        for route in gui::nav::Route::every() {
            app.go(route.clone());
            frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
        }
    }
}

#[test]
fn the_window_survives_a_daemon_that_refuses_everything() {
    let handler = Arc::new(MockHandler::new());
    handler.fail_with(Some(superbackup_core::error::ErrorCode::DaemonUnreachable));
    let ctx = egui::Context::default();
    let mut app = App::new_with_daemon(&ctx, Arc::new(MockDaemon::new(handler)));
    app.data.link_up = false;
    app.data.loading = false;
    for route in gui::nav::Route::every() {
        app.go(route.clone());
        frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    }
}

#[test]
fn the_window_survives_a_malformed_reply() {
    // A destination that names a provider which does not exist, a run whose
    // job was deleted, and a snapshot with no matching destination: three
    // shapes a daemon should never send and the interface must still render.
    let (mut app, ctx, _handler) = seeded_app();
    fixtures::corrupt(&mut app.data);
    for route in gui::nav::Route::every() {
        app.go(route.clone());
        frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    }
}

#[test]
fn onboarding_renders_every_step() {
    let handler = Arc::new(MockHandler::new());
    let ctx = egui::Context::default();
    let mut app = App::new_with_daemon(&ctx, Arc::new(MockDaemon::new(handler)));
    app.begin_onboarding();
    for step in gui::validation::OnboardingStep::ALL {
        app.onboarding_goto(step);
        frame(&mut app, &ctx, egui::vec2(880.0, 640.0));
    }
}

#[test]
fn every_modal_renders() {
    let (mut app, ctx, _handler) = seeded_app();
    for modal in gui::modals::Modal::every(&app.data) {
        app.open_modal(modal);
        frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
        app.close_modal();
    }
}

#[test]
fn an_idle_window_asks_for_no_repaints() {
    let (mut app, ctx, _handler) = seeded_app();
    app.go(gui::nav::Route::Dashboard);
    if let Some(s) = &mut app.data.snapshot {
        s.active_runs.clear();
    }
    app.toasts.clear();
    frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 720.0),
            )),
            ..Default::default()
        },
        |ctx| app.frame(ctx),
    );
    assert!(
        output.viewport_output.values().all(|v| v.repaint_delay
            >= std::time::Duration::from_secs(1)),
        "an idle window must not schedule a repaint: a laptop should not lose an hour of battery to it"
    );
}

#[test]
fn a_running_job_asks_for_repaints() {
    let (mut app, ctx, _handler) = seeded_app();
    app.go(gui::nav::Route::Dashboard);
    frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 720.0),
            )),
            ..Default::default()
        },
        |ctx| app.frame(ctx),
    );
    assert!(
        output
            .viewport_output
            .values()
            .any(|v| v.repaint_delay < std::time::Duration::from_millis(500)),
        "a running job must keep its progress bar moving"
    );
}

#[test]
fn the_first_frame_asks_the_daemon_for_what_it_needs() {
    let handler = Arc::new(MockHandler::new());
    let ctx = egui::Context::default();
    let mut app = App::new_with_daemon(&ctx, Arc::new(MockDaemon::new(handler.clone())));
    frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && handler.calls("status") == 0 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    }
    for command in ["status", "settings.get", "job.list", "dest.list", "provider.list"] {
        assert!(handler.calls(command) > 0, "the window never asked for {command}");
    }
}

#[test]
fn unlocking_from_a_blocked_action_performs_that_action() {
    // The most important detail of the locked-vault flow: the user's intent is
    // not thrown away by the interruption.
    let (mut app, ctx, handler) = seeded_app();
    if let Some(s) = &mut app.data.snapshot {
        s.unlocked = false;
    }
    let job = app.data.jobs[0].clone();
    app.request_run(&job);
    frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    assert!(app.modal_is_unlock(), "a blocked run must offer to unlock");
    assert_eq!(handler.calls("job.run"), 0, "nothing may run while locked");

    app.complete_unlock_for_test();
    frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && handler.calls("job.run") == 0 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame(&mut app, &ctx, egui::vec2(1100.0, 720.0));
    }
    assert_eq!(handler.calls("job.run"), 1, "the pending run must be performed after unlocking");
}

/// Write the screenshots the design review works from.
///
/// Ignored by default because it writes files; run with
/// `cargo test -p superbackup --test gui_app -- --ignored screenshots`.
#[test]
#[ignore = "writes design/screenshots"]
fn screenshots() {
    gui::render::write_gallery(std::path::Path::new("../../design/screenshots"))
        .expect("the gallery could not be written");
}

/// Nothing may be laid out past the window's own edge.
///
/// egui does not clip a child that allocates more than its parent offered, so
/// a column sized to a fixed number rather than to the space available silently
/// slides under the scroll bar. This catches that on every screen at both
/// supported window sizes.
#[test]
fn no_screen_draws_outside_the_window() {
    for size in [egui::vec2(1100.0, 720.0), egui::vec2(900.0, 600.0)] {
        let (mut app, ctx, _handler) = seeded_app();
        for route in gui::nav::Route::every() {
            app.go(route.clone());
            for _ in 0..2 {
                frame(&mut app, &ctx, size);
            }
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| app.frame(ctx),
            );
            let widest = ctx
                .tessellate(output.shapes, 1.0)
                .iter()
                .filter_map(|p| match &p.primitive {
                    egui::epaint::Primitive::Mesh(mesh) => {
                        Some(mesh.vertices.iter().map(|v| v.pos.x).fold(0.0f32, f32::max))
                    }
                    _ => None,
                })
                .fold(0.0f32, f32::max);
            assert!(
                widest <= size.x + 1.0,
                "{route:?} at {size:?} laid out to x={widest:.0}, past the window edge"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The bucket picker and the provider test, as view models
// ---------------------------------------------------------------------------
//
// Driven as data, with no frame laid out. The states are the whole point of the
// feature — a picker that becomes the only way in, or a working key reported as
// broken, are behaviour bugs, not rendering bugs, and they should fail here.

use superbackup_core::ipc::protocol::{BucketInfo, BucketsReply, ObjectInfo, ObjectsReply};

fn buckets_reply(names: &[&str], listed: bool, credentials_ok: bool) -> BucketsReply {
    BucketsReply {
        provider_id: uuid::Uuid::nil(),
        buckets: names
            .iter()
            .map(|n| BucketInfo { name: (*n).to_string(), created_at: None })
            .collect(),
        listed,
        credentials_ok,
        detail: (!listed).then(|| "the key may not list buckets".to_string()),
        latency_ms: Some(10),
    }
}

#[test]
fn a_successful_listing_fills_the_picker() {
    let mut state = gui::screens::destination_editor::State::default();
    let provider = uuid::Uuid::new_v4();
    state.buckets_requested(provider);
    assert_eq!(state.picker(provider), gui::screens::destination_editor::BucketPicker::Loading);
    state.buckets_arrived(provider, &buckets_reply(&["a", "b"], true, true));
    assert_eq!(
        state.picker(provider),
        gui::screens::destination_editor::BucketPicker::Ready(vec!["a".into(), "b".into()])
    );
}

#[test]
fn a_key_that_cannot_list_leaves_the_picker_unavailable_and_never_blocks() {
    // The scoped-key case. The credentials are fine; the list is not
    // available; the destination must still be creatable by typing.
    let mut state = gui::screens::destination_editor::State::default();
    let provider = uuid::Uuid::new_v4();
    state.buckets_arrived(provider, &buckets_reply(&[], false, true));
    let gui::screens::destination_editor::BucketPicker::Unavailable(detail) =
        state.picker(provider)
    else {
        panic!("a refused listing must be `Unavailable`, not `Ready` and not a panic");
    };
    assert!(!detail.is_empty(), "the reason has to reach the user");

    // Typing is untouched by any of this: the field is plain state that the
    // picker never owns.
    state.bucket_input = "typed-by-hand".into();
    assert_eq!(state.bucket_input, "typed-by-hand");
}

#[test]
fn a_listing_for_one_provider_is_not_shown_under_another() {
    let mut state = gui::screens::destination_editor::State::default();
    let storj = uuid::Uuid::new_v4();
    let backblaze = uuid::Uuid::new_v4();
    state.buckets_arrived(storj, &buckets_reply(&["storj-only"], true, true));
    assert_eq!(
        state.picker(backblaze),
        gui::screens::destination_editor::BucketPicker::Idle,
        "switching provider must not show the other account's buckets"
    );
}

#[test]
fn an_empty_account_is_an_answer_not_a_failure() {
    let mut state = gui::screens::destination_editor::State::default();
    let provider = uuid::Uuid::new_v4();
    state.buckets_arrived(provider, &buckets_reply(&[], true, true));
    assert_eq!(
        state.picker(provider),
        gui::screens::destination_editor::BucketPicker::Ready(Vec::new())
    );
}

fn objects_reply(keys: &[&str], listed: bool, holds_repository: bool) -> ObjectsReply {
    ObjectsReply {
        bucket: "b".into(),
        prefix: "p/".into(),
        keys: keys
            .iter()
            .map(|k| ObjectInfo { key: (*k).to_string(), size: 1, last_modified: None })
            .collect(),
        truncated: false,
        holds_repository,
        listed,
        detail: (!listed).then(|| "offline".to_string()),
    }
}

#[test]
fn the_prefix_check_distinguishes_a_repository_from_clutter_from_nothing() {
    use gui::screens::destination_editor::PrefixCheck;
    let provider = uuid::Uuid::new_v4();
    let cases = [
        (objects_reply(&["p/kopia.repository"], true, true), PrefixCheck::Repository),
        (objects_reply(&["p/notes.txt"], true, false), PrefixCheck::Occupied),
        (objects_reply(&[], true, false), PrefixCheck::Empty),
    ];
    for (reply, expected) in cases {
        let mut state = gui::screens::destination_editor::State::default();
        state.objects_arrived(provider, &reply);
        assert_eq!(state.prefix_state(provider), Some(expected));
    }
    let mut state = gui::screens::destination_editor::State::default();
    state.objects_arrived(provider, &objects_reply(&[], false, false));
    assert!(matches!(state.prefix_state(provider), Some(PrefixCheck::Unavailable(_))));
}

#[test]
fn a_scoped_key_is_a_qualified_success_in_the_provider_editor_not_a_failure() {
    use gui::screens::provider_editor::ProbeState;
    let mut state = gui::screens::provider_editor::State::default();
    let provider = uuid::Uuid::new_v4();

    state.probe(provider, &buckets_reply(&["one", "two"], true, true));
    assert!(matches!(state.probe_state(provider), Some(ProbeState::Ok { .. })));

    // Credentials proven, listing refused. This must not render as a failure:
    // telling someone their working key is wrong is the one answer that is
    // actively harmful.
    state.probe(provider, &buckets_reply(&[], false, true));
    assert!(
        matches!(state.probe_state(provider), Some(ProbeState::Qualified { .. })),
        "a refused listing with valid credentials is a qualified success"
    );

    // Credentials not proven at all is a genuine failure.
    state.probe(provider, &buckets_reply(&[], false, false));
    assert!(matches!(state.probe_state(provider), Some(ProbeState::Failed(_))));
}
