//! The egui window: dashboard, jobs, destinations, providers, settings.
//!
//! Owned by the GUI workstream. Talks to the running instance over IPC like
//! any other client, so it can be developed and tested against a mock daemon.
//!
//! # Shape
//!
//! * [`theme`], [`widgets`], [`icons`], [`format`], [`copy`] — the design
//!   system: tokens, the eighteen components, the icon set, the formatting
//!   rules, and every user-facing string.
//! * [`data`], [`viewmodel`], [`validation`] — what each screen *says*,
//!   computed without a rendering context so it can be tested directly.
//! * [`daemon`] — the IPC link, with a mock transport for development.
//! * [`app`], [`nav`], [`screens`], [`modals`], [`toasts`] — the window.
//! * [`render`] — an offscreen rasteriser, for the design review's
//!   screenshots.

use std::process::ExitCode;
use std::time::Duration;

use superbackup_core::paths::Paths;

pub mod app;
pub mod copy;
pub mod daemon;
pub mod data;
pub mod fixtures;
pub mod format;
pub mod icons;
pub mod modals;
pub mod nav;
pub mod render;
pub mod screens;
pub mod theme;
pub mod toasts;
pub mod validation;
pub mod viewmodel;
pub mod widgets;

/// Default 1100 × 720; minimum 900 × 600, enforced rather than suggested.
const DEFAULT_SIZE: [f32; 2] = [1100.0, 720.0];
const MIN_SIZE: [f32; 2] = [900.0, 600.0];

/// Open the window, or focus an already-open one.
pub fn open_or_focus(paths: Paths, global: &crate::cli::GlobalArgs) -> ExitCode {
    let endpoint = paths.ipc_endpoint();
    let timeout = Duration::from_secs(global.timeout.max(5));

    let viewport = egui::ViewportBuilder::default()
        .with_title(copy::window_title(
            superbackup_core::state::Health::Idle.title(),
        ))
        .with_inner_size(DEFAULT_SIZE)
        .with_min_inner_size(MIN_SIZE)
        .with_app_id("superbackup");

    // The window follows the OS theme until the user chooses otherwise; eframe
    // reports the system theme through `Visuals::dark_mode`, which `App` reads.
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    let outcome = eframe::run_native(
        "superbackup",
        options,
        Box::new(move |cc| Ok(Box::new(Window::new(&cc.egui_ctx, endpoint, timeout)))),
    );

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("superbackup: the window could not be opened: {error}");
            ExitCode::from(crate::cli::exit::FAILED as u8)
        }
    }
}

/// The `eframe` shell around [`app::App`].
struct Window {
    app: app::App,
}

impl Window {
    fn new(ctx: &egui::Context, endpoint: String, timeout: Duration) -> Window {
        Window { app: app::App::new(ctx, endpoint, timeout) }
    }
}

impl eframe::App for Window {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Folders dropped onto the window are an additive affordance (L15).
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect()
        });
        if !dropped.is_empty() {
            self.app.accept_dropped_folders(dropped);
        }

        self.app.frame(ctx);

        // The title carries the health, and the running job's percentage.
        let title = match self.app.data.active_runs().first() {
            Some(run) => copy::window_title_running(
                &run.job_name,
                run.overall_fraction().map(|f| (f * 100.0) as i64).unwrap_or(0),
            ),
            None => copy::window_title(self.app.data.health().title()),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}
