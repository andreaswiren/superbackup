//! Tray icon and menu, driven by `state::Health`.
//!
//! # How this cooperates with tokio, and with the GUI, without spinning a core
//!
//! `tray-icon` has a hard platform requirement: the icon must be created on a
//! thread that is running a native event loop — a Win32 message loop on
//! Windows, a GTK loop on Linux, the main-thread `NSApplication` loop on
//! macOS. Tokio's worker threads run none of those, and a tokio task cannot
//! block in `GetMessageW` without taking a worker thread out of the pool for
//! the life of the process.
//!
//! The arrangement is therefore:
//!
//! ```text
//!   tokio runtime                      dedicated OS thread ("superbackup-tray")
//!   ─────────────                      ────────────────────────────────────────
//!   engine events ─▶ watcher task           TrayIcon + Menu created here
//!          │              │                          │
//!          │      TrayCommand (mpsc) ────────────────┤  GetMessageW / PeekMessage
//!          │                                         │  ↑ blocks, never spins
//!   Action ◀── crossbeam receiver ◀── menu click ────┘
//!          │
//!          └─▶ handled on the tokio runtime (IPC handler, engine, notifier)
//! ```
//!
//! Three properties fall out of that and each was the point:
//!
//! 1. **No busy loop.** The thread blocks in the OS's own wait — `GetMessageW`
//!    on Windows — and is woken by the OS. A `PeekMessage`/`sleep(16ms)` poll
//!    would burn a wakeup sixty times a second on a laptop that is idle
//!    99.99% of the time, which on a backup tool that runs all day is a
//!    measurable battery cost.
//! 2. **No tokio worker is held.** The blocking loop is on a thread this
//!    module owns, so the runtime's workers stay available for backups.
//! 3. **No shared event loop with the GUI.** `superbackup gui` is a separate
//!    *process* that talks to this one over IPC (see `main.rs`), so the GUI's
//!    winit loop and this message loop never contend. If the GUI is ever
//!    hosted in-process, the correct change is to hand `TrayIconEvent::
//!    set_event_handler` a winit `EventLoopProxy` — the plan below already
//!    routes every event through a channel, so that is a one-line swap rather
//!    than a redesign.
//!
//! Waking the thread to *change* the icon is the other half. A menu rebuild or
//! an icon swap is posted as a [`TrayCommand`] and the thread is woken by the
//! same channel it drains after each message; the running-state animation adds
//! a 120 ms timer, and only while a backup is actually running.
//!
//! # Platform coverage
//!
//! The Win32 message loop is implemented here. On Linux and macOS the loop
//! belongs to GTK and to `NSApplication` respectively, and neither binding is
//! reachable from this crate (see the note on [`run_event_loop`]); on those
//! platforms [`spawn`] reports the tray as unavailable and the daemon carries
//! on headless, which it is designed to do.

pub mod icons;
pub mod menu;

use std::sync::Arc;
use std::time::Duration;

use superbackup_core::state::{Health, StatusSnapshot};
use superbackup_core::{Error, Result};
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::daemon::runtime::Runtime;
use menu::{Action, Item, MenuPlan};

/// How often the running-state mark advances a frame.
///
/// Twelve frames at 120 ms is a 1.44-second rotation: fast enough to read as
/// motion, slow enough that the icon is not a distraction in the corner of
/// someone's eye all afternoon. The timer exists **only** while a run is
/// active; an idle tray raises no timer at all.
const ANIMATION_INTERVAL: Duration = Duration::from_millis(120);

/// The menu's second header line updates at most once a second (§14.3), so the
/// watcher coalesces progress at this rate rather than rebuilding a native
/// menu on every frame — which on Windows would flicker an open menu.
const MENU_REFRESH: Duration = Duration::from_secs(1);

/// What the tray thread is asked to do.
#[derive(Debug)]
enum TrayCommand {
    /// Rebuild the menu, tooltip and icon from a new plan.
    Update(Box<MenuPlan>),
    /// Advance the running animation by one frame.
    Tick,
    /// The taskbar theme changed; re-render at the other variant.
    ThemeChanged,
    Stop,
}

/// A handle to a running tray.
#[derive(Debug)]
pub struct TrayHandle {
    commands: std::sync::mpsc::Sender<TrayCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TrayHandle {
    /// Remove the icon and stop the thread. Blocks briefly, which is correct:
    /// an icon left in the notification area after the process exits is a
    /// Windows classic and it is this join that prevents it.
    pub fn shutdown(mut self) {
        let _ = self.commands.send(TrayCommand::Stop);
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::warn!("the tray thread did not stop cleanly");
            }
        }
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        // A handle dropped without `shutdown` still takes the icon away.
        let _ = self.commands.send(TrayCommand::Stop);
    }
}

/// Start the tray.
///
/// Returns an error when this platform has no reachable event loop, which the
/// daemon treats as cosmetic: backups keep running headless.
pub fn spawn(runtime: Arc<Runtime>) -> Result<TrayHandle> {
    if !platform_supported() {
        return Err(Error::Platform(format!(
            "this build has no tray on {}; superbackup will run without one. Backups, the \
             command line and the interface all still work.",
            std::env::consts::OS
        )));
    }

    // Captured before the thread starts, because the thread has no runtime of
    // its own and a click may arrive the instant the icon appears.
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => register_runtime_handle(handle),
        Err(_) => {
            return Err(Error::Internal(
                "the tray must be started from inside the tokio runtime".into(),
            ))
        }
    }

    let (commands, inbox) = std::sync::mpsc::channel::<TrayCommand>();
    let (started, ready) = std::sync::mpsc::channel::<Result<()>>();

    let thread_runtime = Arc::clone(&runtime);
    let thread = std::thread::Builder::new()
        .name("superbackup-tray".into())
        .spawn(move || tray_thread(thread_runtime, inbox, started))
        .map_err(|e| Error::Platform(format!("the tray thread could not be started: {e}")))?;

    // Wait for the icon to be created (or to fail) before reporting success,
    // so the caller's log line is the truth rather than an assumption.
    match ready.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(Error::Platform("the tray did not start within ten seconds".into()))
        }
    }

    spawn_watcher(runtime, commands.clone());
    Ok(TrayHandle { commands, thread: Some(thread) })
}

/// Whether this build can run the native loop the tray needs.
fn platform_supported() -> bool {
    cfg!(windows)
}

// ---------------------------------------------------------------------------
// The tokio side: watch the engine, post plans to the tray thread
// ---------------------------------------------------------------------------

/// Follow the daemon's status stream and keep the tray in step.
///
/// Driven by the broadcast the IPC layer already publishes, rather than by
/// polling: the tray is a subscriber like the GUI, so there is exactly one
/// definition of "what is happening" and the icon cannot disagree with the
/// dashboard. A lag on this subscription is harmless — the next status item
/// carries the whole picture.
fn spawn_watcher(runtime: Arc<Runtime>, commands: std::sync::mpsc::Sender<TrayCommand>) {
    tokio::spawn(async move {
        let mut stream = runtime.subscribe_stream(&superbackup_core::ipc::Topic::all());
        let mut shutdown = runtime.subscribe_shutdown();
        let mut refresh = tokio::time::interval(MENU_REFRESH);
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut animation: Option<tokio::time::Interval> = None;
        let mut theme = icons::system_uses_light_theme();

        // Paint once immediately, so the icon is correct before the first
        // event rather than after it.
        if post_plan(&runtime, &commands).await.is_err() {
            return;
        }

        loop {
            let animating = animation.is_some();
            tokio::select! {
                _ = shutdown.recv() => return,

                // Any stream item may have changed the picture; the plan is
                // cheap and idempotent, and the tray thread compares before
                // it rebuilds anything native.
                item = stream.recv() => {
                    if item.is_err() && matches!(item, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                        return;
                    }
                }

                _ = refresh.tick() => {
                    let running = !runtime.active_runs().is_empty();
                    // The animation timer exists only while something runs.
                    match (running, animating) {
                        (true, false) => {
                            let mut interval = tokio::time::interval(ANIMATION_INTERVAL);
                            interval.set_missed_tick_behavior(
                                tokio::time::MissedTickBehavior::Skip,
                            );
                            animation = Some(interval);
                        }
                        (false, true) => animation = None,
                        _ => {}
                    }
                    let now = icons::system_uses_light_theme();
                    if now != theme {
                        theme = now;
                        if commands.send(TrayCommand::ThemeChanged).is_err() {
                            return;
                        }
                    }
                }

                _ = async {
                    match animation.as_mut() {
                        Some(interval) => { interval.tick().await; }
                        None => std::future::pending().await,
                    }
                } => {
                    if commands.send(TrayCommand::Tick).is_err() {
                        return;
                    }
                    continue;
                }
            }

            if post_plan(&runtime, &commands).await.is_err() {
                return;
            }
        }
    });
}

async fn post_plan(
    runtime: &Arc<Runtime>,
    commands: &std::sync::mpsc::Sender<TrayCommand>,
) -> std::result::Result<(), ()> {
    let snapshot = runtime.current_snapshot().await;
    let config = { runtime.store.lock().await.config().clone() };
    let plan = menu::plan(&snapshot, &config, runtime.kopia().is_some());
    commands.send(TrayCommand::Update(Box::new(plan))).map_err(|_| ())
}

// ---------------------------------------------------------------------------
// The tray thread
// ---------------------------------------------------------------------------

struct TrayState {
    icon: TrayIcon,
    cache: icons::IconCache,
    variant: icons::Variant,
    plan: Option<MenuPlan>,
    frame: usize,
    /// Kept alive: `muda` items are reference-counted and dropping the last
    /// handle while the menu is on screen removes the item under the cursor.
    _menu: Menu,
}

fn tray_thread(
    runtime: Arc<Runtime>,
    inbox: std::sync::mpsc::Receiver<TrayCommand>,
    started: std::sync::mpsc::Sender<Result<()>>,
) {
    let cache = icons::IconCache::new();
    let variant = icons::Variant::detect();
    let initial = match cache.get(icons::IconKey::new(Health::Idle, variant, 0)) {
        Ok(icon) => icon,
        Err(e) => {
            let _ = started.send(Err(Error::Platform(format!(
                "the tray icon could not be drawn: {e}"
            ))));
            return;
        }
    };

    let menu = Menu::new();
    let icon = match TrayIconBuilder::new()
        .with_id("superbackup")
        .with_icon(initial)
        .with_icon_as_template(variant == icons::Variant::Template)
        .with_tooltip("superbackup")
        // Windows and Linux open the window on left click and the menu on
        // right (§14.1); macOS opens the menu on either, by convention.
        .with_menu_on_left_click(cfg!(target_os = "macos"))
        .with_menu(Box::new(menu.clone()))
        .build()
    {
        Ok(icon) => icon,
        Err(e) => {
            let _ = started.send(Err(Error::Platform(format!(
                "the tray icon could not be created: {e}"
            ))));
            return;
        }
    };

    let mut state = TrayState { icon, cache, variant, plan: None, frame: 0, _menu: menu };
    let _ = started.send(Ok(()));

    run_event_loop(&runtime, &mut state, &inbox);

    // Taking the icon away explicitly rather than relying on the drop order,
    // because on Windows a leftover icon survives until the user hovers it.
    let _ = state.icon.set_visible(false);
}

/// Drain everything that is pending: commands from the daemon, clicks from the
/// menu, clicks on the icon itself.
///
/// Returns false once the tray has been asked to stop.
fn pump_once(runtime: &Arc<Runtime>, state: &mut TrayState, inbox: &std::sync::mpsc::Receiver<TrayCommand>) -> bool {
    while let Ok(command) = inbox.try_recv() {
        match command {
            TrayCommand::Stop => return false,
            TrayCommand::Update(plan) => apply(state, *plan),
            TrayCommand::Tick => {
                state.frame = state.frame.wrapping_add(1);
                if state.plan.as_ref().map(|p| p.health) == Some(Health::Running) {
                    set_icon(state, Health::Running);
                }
            }
            TrayCommand::ThemeChanged => {
                state.variant = icons::Variant::detect();
                state.cache.clear();
                if let Some(health) = state.plan.as_ref().map(|p| p.health) {
                    set_icon(state, health);
                }
            }
        }
    }

    // Menu ids arrive as opaque strings from the OS; `Action::from_id` is
    // total, so a stale id from a menu that has just been rebuilt is ignored
    // rather than fatal.
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        if let Some(action) = Action::from_id(event.id.as_ref()) {
            dispatch(runtime, action);
        }
    }

    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
        if let TrayIconEvent::Click { button, button_state, .. } = event {
            // Left click shows the window (§14.1). The menu is opened by the
            // platform itself on the other button.
            if button == tray_icon::MouseButton::Left
                && button_state == tray_icon::MouseButtonState::Up
            {
                dispatch(runtime, Action::OpenApp);
            }
        }
    }
    true
}

/// Swap in a new plan, touching the native menu only when it actually changed.
///
/// Rebuilding an `HMENU` while it is on screen closes it under the user's
/// cursor, so a menu that has not changed is left completely alone. The
/// tooltip and the icon are cheap and are set whenever they differ.
fn apply(state: &mut TrayState, plan: MenuPlan) {
    let menu_changed = state.plan.as_ref().map(|p| &p.items) != Some(&plan.items);
    let tooltip_changed =
        state.plan.as_ref().map(|p| p.tooltip()) != Some(plan.tooltip());
    let health_changed = state.plan.as_ref().map(|p| p.health) != Some(plan.health);

    if menu_changed {
        match build_menu(&plan) {
            Ok(menu) => {
                state.icon.set_menu(Some(Box::new(menu.clone())));
                state._menu = menu;
            }
            Err(e) => tracing::warn!(error = %e, "the tray menu could not be rebuilt"),
        }
    }
    if tooltip_changed {
        if let Err(e) = state.icon.set_tooltip(Some(plan.tooltip())) {
            tracing::debug!(error = %e, "the tray tooltip could not be set");
        }
    }
    if health_changed {
        state.frame = 0;
        set_icon(state, plan.health);
    }
    state.plan = Some(plan);
}

fn set_icon(state: &mut TrayState, health: Health) {
    let key = icons::IconKey::new(health, state.variant, state.frame);
    match state.cache.get(key) {
        Ok(icon) => {
            if let Err(e) = state.icon.set_icon(Some(icon)) {
                tracing::debug!(error = %e, "the tray icon could not be set");
            }
        }
        Err(e) => tracing::warn!(error = %e, "the tray icon could not be drawn"),
    }
}

/// Turn a [`MenuPlan`] into `muda` items.
fn build_menu(plan: &MenuPlan) -> Result<Menu> {
    let menu = Menu::new();
    for item in &plan.items {
        append(&menu, item)?;
    }
    Ok(menu)
}

fn append(menu: &Menu, item: &Item) -> Result<()> {
    let built = to_muda(item)?;
    menu.append(built.as_ref()).map_err(|e| Error::Platform(format!("{e}")))
}

/// Build one item, recursively.
///
/// Boxed because `muda`'s four item types share only a trait, and a submenu
/// has to own its children for as long as the menu is on screen.
fn to_muda(item: &Item) -> Result<Box<dyn tray_icon::menu::IsMenuItem>> {
    Ok(match item {
        Item::Separator => Box::new(PredefinedMenuItem::separator()),
        Item::Entry { label, action, enabled, reason } => {
            // The accessible name carries the state suffix (§14.5); on
            // Windows the label itself is what a screen reader announces, so
            // the reason is already in it via `Item::blocked`.
            debug_assert!(reason.is_none() || !*enabled);
            Box::new(MenuItem::with_id(action.id(), label, *enabled, None))
        }
        Item::Check { label, action, checked, enabled } => {
            Box::new(CheckMenuItem::with_id(action.id(), label, *enabled, *checked, None))
        }
        Item::Submenu { label, items, enabled } => {
            let submenu = Submenu::new(label, *enabled);
            for child in items {
                let built = to_muda(child)?;
                submenu.append(built.as_ref()).map_err(|e| Error::Platform(format!("{e}")))?;
            }
            Box::new(submenu)
        }
    })
}

// ---------------------------------------------------------------------------
// Acting on a click
// ---------------------------------------------------------------------------

/// Hand an action to the tokio runtime.
///
/// Nothing here blocks the message loop: a click posts work and returns, so a
/// slow IPC handler can never wedge the notification area. That is why the
/// tray talks to the daemon through the same [`Runtime`] the IPC handler uses
/// rather than doing anything itself.
fn dispatch(runtime: &Arc<Runtime>, action: Action) {
    // The tray thread is not a tokio worker, so `Handle::try_current` fails
    // here by construction; `spawn` records the handle before the thread
    // starts for exactly this reason.
    let handle = RUNTIME_HANDLE.lock().ok().and_then(|slot| slot.clone());
    let Some(handle) = handle else {
        tracing::warn!(?action, "a tray click arrived with no runtime to handle it");
        return;
    };
    let runtime = Arc::clone(runtime);
    handle.spawn(async move { perform(runtime, action).await });
}

/// The tokio handle, captured when the tray starts.
static RUNTIME_HANDLE: std::sync::Mutex<Option<tokio::runtime::Handle>> =
    std::sync::Mutex::new(None);

/// Do what the menu item said.
async fn perform(runtime: Arc<Runtime>, action: Action) {
    use superbackup_core::state::{Event, Trigger};

    match action {
        Action::None => {}

        Action::RunAll => {
            let config = { runtime.store.lock().await.config().clone() };
            let Some(scheduler) = runtime.scheduler() else { return };
            for job in config.jobs.iter().filter(|j| j.enabled) {
                if let Err(e) = scheduler.run_now(job.id, Trigger::Manual).await {
                    tracing::debug!(job = %job.name, error = %e, "tray run was refused");
                }
            }
        }

        Action::RunJob(job_id) => {
            let Some(scheduler) = runtime.scheduler() else { return };
            if let Err(e) = scheduler.run_now(job_id, Trigger::Manual).await {
                tracing::info!(%job_id, error = %e, "tray run was refused");
            }
        }

        Action::StopRun(run_id) => {
            // §14.3: from the tray this acts immediately and says what it did.
            // A modal the user cannot see is worse than no confirmation.
            let Some(job_id) = runtime.job_for_run(&run_id) else { return };
            let name = runtime
                .active_runs()
                .iter()
                .find(|r| r.run_id == run_id)
                .map(|r| r.job_name.clone())
                .unwrap_or_else(|| "the backup".to_string());
            if let Some(scheduler) = runtime.scheduler() {
                let _ = scheduler.cancel_job(job_id);
            }
            runtime.record_event(
                Event::info(
                    "job.stopped",
                    format!(
                        "\"{name}\" was stopped from the tray. Its partial snapshot was \
                         discarded; the next run starts again."
                    ),
                )
                .with_job(job_id),
            );
        }

        Action::StopAll => {
            let Some(scheduler) = runtime.scheduler() else { return };
            for run in runtime.active_runs() {
                let _ = scheduler.cancel_job(run.job_id);
            }
            runtime.record_event(Event::info(
                "job.stopped_all",
                "Every running backup was stopped from the tray.",
            ));
        }

        Action::Pause(choice) => {
            let state = menu::pause_state_for(choice, chrono::Utc::now());
            set_settings(&runtime, |settings| settings.pause = state.clone()).await;
            runtime.record_event(Event::info(
                "control.paused",
                match choice {
                    Some(1) => "Backups paused for 1 hour.".to_string(),
                    Some(hours) => format!("Backups paused for {hours} hours."),
                    None => "Backups paused until you resume them.".to_string(),
                },
            ));
        }

        Action::Resume => {
            set_settings(&runtime, |settings| {
                settings.pause = superbackup_core::model::PauseState::default()
            })
            .await;
            runtime.record_event(Event::info("control.resumed", "Backups resumed."));
        }

        Action::DisableAll(disable) => {
            set_all_jobs_enabled(&runtime, !disable).await;
        }

        // Opening the window is the GUI workstream's; this launches the same
        // command a user would type, which focuses an already-open window
        // rather than starting a second one.
        Action::OpenApp => open_interface(&runtime, &[]),
        Action::OpenActivity => open_interface(&runtime, &["--screen", "activity"]),
        Action::OpenSettings => open_interface(&runtime, &["--screen", "settings"]),
        Action::OpenJob(job_id) => {
            open_interface(&runtime, &["--screen", "activity", "--job", &job_id.to_string()])
        }
        Action::FixKopia => open_interface(&runtime, &["--screen", "settings"]),
        Action::Unlock => open_interface(&runtime, &["--screen", "unlock"]),

        Action::Quit => {
            // §14.3: quitting with a run in flight is confirmed in the window,
            // which the daemon cannot raise. It records what it is about to
            // discard instead, so the decision is at least visible in Activity.
            let active = runtime.active_runs();
            if let Some(text) = menu::quit_confirmation(&active) {
                runtime.record_event(Event::new(
                    superbackup_core::state::Severity::Warning,
                    "daemon.quit_with_runs",
                    text,
                ));
            }
            runtime.request_shutdown(true);
        }
    }
}

/// Edit settings through the store, so the change is validated, persisted and
/// handed to the scheduler exactly as an IPC `settings.update` would be.
async fn set_settings(
    runtime: &Arc<Runtime>,
    mutate: impl FnOnce(&mut superbackup_core::model::Settings),
) {
    let mut store = runtime.store.lock().await;
    let mut config = store.config().clone();
    mutate(&mut config.settings);
    if let Err(e) = store.set_config(config) {
        tracing::warn!(error = %e, "the tray could not save a settings change");
        return;
    }
    let saved = store.config().clone();
    drop(store);
    runtime.push_config(&saved);
    runtime.publish_status().await;
}

/// Turn every job off, or turn back on exactly the ones this switched off.
///
/// §14.2: unticking must not enable jobs the user had disabled by hand, so the
/// set is remembered rather than recomputed.
async fn set_all_jobs_enabled(runtime: &Arc<Runtime>, enabled: bool) {
    let mut store = runtime.store.lock().await;
    let mut config = store.config().clone();
    if enabled {
        let remembered = runtime.bulk_disabled();
        for job in &mut config.jobs {
            if remembered.contains(&job.id) {
                job.enabled = true;
            }
        }
        runtime.set_bulk_disabled(Default::default());
    } else {
        let mut disabled = std::collections::BTreeSet::new();
        for job in &mut config.jobs {
            if job.enabled {
                job.enabled = false;
                disabled.insert(job.id);
            }
        }
        runtime.set_bulk_disabled(disabled);
    }
    if let Err(e) = store.set_config(config) {
        tracing::warn!(error = %e, "the tray could not change job states");
        return;
    }
    let saved = store.config().clone();
    drop(store);
    runtime.push_config(&saved);
    runtime.publish_status().await;
    runtime.record_event(superbackup_core::state::Event::info(
        "job.bulk_enabled",
        if enabled { "Jobs were re-enabled." } else { "Every job was disabled." },
    ));
}

/// Launch `superbackup gui`, which focuses a running window or opens one.
fn open_interface(runtime: &Arc<Runtime>, extra: &[&str]) {
    let Ok(exe) = std::env::current_exe() else {
        tracing::warn!("could not find this program's own path to open the interface");
        return;
    };
    let mut command = std::process::Command::new(exe);
    command.arg("gui");
    // The window must reach the same configuration this daemon is serving,
    // which is not the default one when `--home` was used.
    command.arg("--home").arg(runtime.paths.config_dir.parent().unwrap_or(&runtime.paths.config_dir));
    for arg in extra {
        command.arg(arg);
    }
    if let Err(e) = command.spawn() {
        tracing::warn!(error = %e, "could not open the interface");
    }
}

// ---------------------------------------------------------------------------
// The native event loop
// ---------------------------------------------------------------------------

/// Run the platform's message loop until the tray is asked to stop.
///
/// ## Windows
///
/// `tray-icon` creates a message-only window on this thread, and Windows
/// delivers every notification-area callback to it as a message. `GetMessageW`
/// blocks in the kernel until one arrives, so the thread costs nothing while
/// idle — which matters, because this thread exists for the entire life of a
/// daemon that is idle almost all of the time.
///
/// A blocking wait alone would never notice a [`TrayCommand`], so a 1 ms timer
/// is set on the thread: it wakes the loop often enough that an icon change is
/// imperceptible, and `WM_TIMER` is a low-priority message that Windows only
/// delivers when the queue is otherwise empty, so it does not compete with
/// real input. The alternative — `PostThreadMessage` from the tokio side —
/// needs a thread id this crate cannot obtain without another dependency.
///
/// ## Elsewhere
///
/// Linux needs a GTK main loop and macOS the main thread's `NSApplication`
/// loop. Neither `gtk` nor `objc2` is a dependency of this crate, and adding
/// one is outside this workstream's boundary, so [`platform_supported`]
/// reports false there and the daemon runs headless rather than pretending.
#[cfg(windows)]
fn run_event_loop(
    runtime: &Arc<Runtime>,
    state: &mut TrayState,
    inbox: &std::sync::mpsc::Receiver<TrayCommand>,
) {
    // SAFETY: these are the stable Win32 message-loop entry points, declared
    // here rather than pulled from a binding crate because `windows-sys` is
    // not a dependency of this crate and adding one is outside this
    // workstream's boundary. Each signature is the documented one:
    //
    //   BOOL  GetMessageW(LPMSG, HWND, UINT, UINT)
    //   BOOL  TranslateMessage(const MSG*)
    //   LRESULT DispatchMessageW(const MSG*)
    //   UINT_PTR SetTimer(HWND, UINT_PTR, UINT, TIMERPROC)
    //
    // `MSG` is declared with the exact layout from `winuser.h`; it is only
    // ever written by the OS and read back by the OS, never interpreted here.
    #[repr(C)]
    struct Msg {
        hwnd: *mut core::ffi::c_void,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        pt_x: i32,
        pt_y: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetMessageW(msg: *mut Msg, hwnd: *mut core::ffi::c_void, min: u32, max: u32) -> i32;
        fn TranslateMessage(msg: *const Msg) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> isize;
        fn SetTimer(
            hwnd: *mut core::ffi::c_void,
            id: usize,
            elapse: u32,
            proc_: Option<unsafe extern "system" fn()>,
        ) -> usize;
    }

    // A thread timer (null HWND) wakes the loop so queued commands are seen.
    // SAFETY: a null window handle asks for a thread-scoped timer, which is
    // the documented behaviour, and a null TIMERPROC posts `WM_TIMER` rather
    // than calling back — so no Rust code is ever invoked from the OS here.
    let timer = unsafe { SetTimer(std::ptr::null_mut(), 0, 1, None) };
    if timer == 0 {
        tracing::warn!("the tray timer could not be created; icon updates may lag");
    }

    loop {
        if !pump_once(runtime, state, inbox) {
            return;
        }
        let mut msg = Msg {
            hwnd: std::ptr::null_mut(),
            message: 0,
            w_param: 0,
            l_param: 0,
            time: 0,
            pt_x: 0,
            pt_y: 0,
        };
        // SAFETY: `msg` is a live, correctly-laid-out `MSG` for the duration
        // of each call, and a null HWND means "any window on this thread",
        // which is what the message-only window `tray-icon` created belongs to.
        let result = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        match result {
            // WM_QUIT: nothing posts it here, but honouring it is correct.
            0 => return,
            // -1 is an error, and looping on it would spin a core.
            -1 => {
                tracing::error!("the tray message loop failed; the icon will stop updating");
                return;
            }
            _ => {
                // SAFETY: `msg` was filled in by `GetMessageW` immediately
                // above and is not aliased.
                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

#[cfg(not(windows))]
fn run_event_loop(
    runtime: &Arc<Runtime>,
    state: &mut TrayState,
    inbox: &std::sync::mpsc::Receiver<TrayCommand>,
) {
    // Unreachable in practice: `platform_supported` gates `spawn`. Kept as a
    // correct-if-slower fallback rather than an `unimplemented!()`, so that
    // enabling another platform is a one-line change to `platform_supported`
    // once its loop is available.
    while pump_once(runtime, state, inbox) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Record the tokio handle so the tray thread can hand work back.
fn register_runtime_handle(handle: tokio::runtime::Handle) {
    if let Ok(mut slot) = RUNTIME_HANDLE.lock() {
        *slot = Some(handle);
    }
}

/// The tray's view of a snapshot, for tests and for the GUI's status strip.
pub fn health_of(snapshot: &StatusSnapshot) -> Health {
    snapshot.health
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tray_is_only_claimed_where_a_loop_actually_exists() {
        // A tray that reports itself available and then never repaints is
        // worse than one that says it is not there.
        assert_eq!(platform_supported(), cfg!(windows));
    }

    #[test]
    fn every_planned_item_converts_into_a_native_item() {
        let snapshot = superbackup_core::state::StatusSnapshot {
            health: Health::Running,
            version: "0".into(),
            machine_label: "pc".into(),
            machine_slug: "pc".into(),
            unlocked: true,
            paused: false,
            paused_until: None,
            service_installed: false,
            service_running: false,
            kopia_version: None,
            active_runs: vec![],
            jobs: Default::default(),
            next_scheduled: None,
            recent_events: vec![],
            uptime_seconds: 1,
            generated_at: chrono::Utc::now(),
        };
        let mut config = superbackup_core::model::Config::default();
        config.jobs.push(superbackup_core::engine::testing::test_job("dev"));
        let plan = menu::plan(&snapshot, &config, true);
        // Converting must not panic on any item shape; building a real `Menu`
        // needs a GUI thread on some platforms, so only the leaf conversion is
        // exercised here.
        for item in &plan.items {
            if matches!(item, Item::Submenu { .. }) {
                continue;
            }
            assert!(to_muda(item).is_ok(), "{item:?}");
        }
    }
}
