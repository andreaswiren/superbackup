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

use nav::{Route, SettingsSection};

pub mod app;
pub mod copy;
pub mod daemon;
pub mod data;
pub mod fixtures;
pub mod format;
pub mod icons;
pub mod kopia;
pub mod markdown;
pub mod modals;
pub mod nav;
pub mod passkey;
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

/// The application mark, embedded so the window carries it with no runtime
/// file lookup.
///
/// The Windows executable gets its icon from the resource compiled in by
/// `build.rs`, but that is the icon Explorer and the taskbar *button* use — the
/// window itself, Alt-Tab, and every Linux and macOS surface take theirs from
/// here. Without it the window shows eframe's default and the application looks
/// unfinished in exactly the place a user looks most.
///
/// A decode failure is not worth failing to start over: the window simply keeps
/// the default icon.
fn window_icon() -> egui::IconData {
    const PNG: &[u8] = include_bytes!("../../../../assets/icons/png/superbackup-256.png");
    match image::load_from_memory(PNG) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData { rgba: rgba.into_raw(), width, height }
        }
        Err(_) => egui::IconData::default(),
    }
}

/// Where a `--screen` name from the tray lands.
///
/// Deliberately forgiving. This is one half of superbackup talking to the
/// other, and the cost of being strict was five tray entries that opened
/// nothing at all. An unknown name gets the dashboard, which is wrong but
/// visible; refusing gets a window that never appears, which is wrong and
/// invisible.
fn route_for(screen: Option<&str>, job: Option<&str>) -> Option<Route> {
    let job = || job.and_then(|id| uuid::Uuid::parse_str(id).ok());
    Some(match screen?.trim().to_ascii_lowercase().as_str() {
        "dashboard" | "home" => Route::Dashboard,
        // A job named with the screen goes to that job rather than the list,
        // which is what the tray's per-job entries mean by it.
        "jobs" => match job() {
            Some(id) => Route::JobDetail(id),
            None => Route::Jobs,
        },
        "activity" => match job() {
            Some(id) => Route::JobDetail(id),
            None => Route::Activity,
        },
        "destinations" => Route::Destinations,
        "providers" | "storage" => Route::Providers,
        "git" => Route::Git,
        "credentials" | "keys" => Route::Credentials,
        "restore" => Route::Restore,
        "settings" => Route::Settings(SettingsSection::General),
        // The vault is opened from a modal over whatever is behind it, and
        // `App` raises that modal itself whenever the vault is locked. So this
        // needs no special case beyond landing somewhere sensible.
        "unlock" => Route::Dashboard,
        _ => return None,
    })
}

/// Open the window, or focus an already-open one.
pub fn open_or_focus(
    paths: Paths,
    global: &crate::cli::GlobalArgs,
    args: &crate::cli::GuiArgs,
) -> ExitCode {
    let endpoint = paths.ipc_endpoint();
    let timeout = Duration::from_secs(global.timeout.max(5));
    let route = route_for(args.screen.as_deref(), args.job.as_deref());

    // What this window would run to start a daemon of its own. Carried through
    // so the started instance reads the same files this one does — a window on
    // `--home dist/demo-home` that started a daemon on the default root would
    // set up an installation nobody asked for and then fail to talk to it.
    let mut launch = vec!["daemon".to_string()];
    if let Some(home) = &global.home {
        launch.push("--home".to_string());
        launch.push(home.display().to_string());
    }

    let viewport = egui::ViewportBuilder::default()
        .with_title(copy::window_title(superbackup_core::state::Health::Idle.title()))
        .with_inner_size(DEFAULT_SIZE)
        .with_min_inner_size(MIN_SIZE)
        .with_app_id("superbackup")
        .with_icon(window_icon());

    // The window follows the OS theme until the user chooses otherwise; eframe
    // reports the system theme through `Visuals::dark_mode`, which `App` reads.
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    let outcome = eframe::run_native(
        "superbackup",
        options,
        Box::new(move |cc| {
            Ok(Box::new(Window::new(&cc.egui_ctx, endpoint, timeout, paths, route, launch)))
        }),
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
    fn new(
        ctx: &egui::Context,
        endpoint: String,
        timeout: Duration,
        paths: Paths,
        route: Option<Route>,
        launch: Vec<String>,
    ) -> Window {
        let mut app = app::App::new(ctx, endpoint, timeout).with_paths(paths);
        app.launch = launch;
        // After `with_paths`, so a first run still opens onboarding rather
        // than the screen the tray asked for: a window that cannot do
        // anything yet must not pretend otherwise.
        if let Some(route) = route {
            app.go(route);
        }
        Window { app }
    }
}

impl eframe::App for Window {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Folders dropped onto the window are an additive affordance (L15).
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
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
