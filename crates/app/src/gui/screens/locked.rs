//! The whole window, when the vault is locked.
//!
//! # Why this replaces the application rather than warning inside it
//!
//! A locked vault used to be reported in four places at once: a red banner on
//! the dashboard, a pill in the status strip, a per-screen empty state, and a
//! modal that appeared the moment anything needed a key. The user could still
//! walk through Jobs, Destinations and Storage providers, reading a
//! configuration that describes exactly where their backups live and what they
//! are called — and every action they tried raised the same prompt again.
//!
//! That is worse than it looks. Saying the same thing four times teaches people
//! to dismiss it, and a locked vault that still shows the configuration is
//! protecting the keys while publishing the map to them.
//!
//! So locked is a state of the *window*, not a warning inside it. One place to
//! unlock, one thing to read.
//!
//! # What survives the lock, and why
//!
//! Recent runs, and only those: the five most recent, as status and time. It is
//! the one question worth answering without a passphrase — *did last night's
//! backup work?* — and answering it needs nothing secret. `job.history` is
//! deliberately one of the few commands that carry no `needs_unlock` flag, and
//! run history holds no paths, no credentials and no destination names beyond
//! the job's own label.
//!
//! Everything else waits. A configuration is not shown, because a configuration
//! is a description of where someone's data lives.

use chrono::Utc;
use egui::{Align, Layout, Sense, Ui, Vec2};

use superbackup_core::state::RunStatus;

use crate::gui::app::App;
use crate::gui::icons::Icon;
use crate::gui::theme::{self, radius, space, Type};
use crate::gui::widgets::{self, Button};
use crate::gui::{copy, format};

/// How many recent runs the lock screen shows.
///
/// Five is enough to see a pattern — three greens and two reds is a different
/// story from five greens — without turning the lock screen into a report.
const RECENT_RUNS: usize = 5;

impl App {
    pub(crate) fn show_locked(&mut self, ctx: &egui::Context) {
        // The authenticator answers up to a minute after the click, so the
        // frame is what notices, not the button.
        self.poll_passkey();
        if self.screens.locked.passkey.is_some() {
            // A prompt is up and this window must keep painting behind it.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let t = theme::tokens(ctx);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg_canvas)).show(ctx, |ui| {
            let available = ui.available_height();
            widgets::scroll_area(ui, "sb-locked", |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), 0.0),
                    Layout::top_down(Align::Center),
                    |ui| {
                        // Centred vertically when there is room, pinned to
                        // the top when there is not, so a short window
                        // scrolls instead of clipping the unlock control.
                        ui.add_space((available * 0.12).clamp(24.0, 120.0));
                        self.locked_identity(ui);
                        ui.add_space(space::H2);
                        self.locked_unlock(ui);
                        ui.add_space(space::H2);
                        self.locked_recent(ui);
                        ui.add_space(space::H2);
                        self.locked_installation(ui);
                        ui.add_space(space::H2);
                    },
                );
            });
        });
    }

    /// What this window is looking at, and the ways out of it.
    ///
    /// A locked window used to be a passphrase field and nothing else, which
    /// is a problem the moment somebody has more than one installation or the
    /// background process is not running: the screen said neither, so the only
    /// way to find out was to get in, and getting in was the thing that was
    /// not working.
    ///
    /// So it says which vault it is asking about, whether anything is running
    /// behind it, and what version this is — the same three facts the status
    /// strip carries once you are inside — and it offers the two things a
    /// person locked out actually wants: a different vault, or a new one.
    fn locked_installation(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let mut open_other = false;
        let mut create_new = false;
        let mut about = false;

        ui.allocate_ui_with_layout(Vec2::new(420.0, 0.0), Layout::top_down(Align::Center), |ui| {
            // Which installation. The path, because a person with two of them
            // needs to know which one is refusing.
            if let Some(paths) = &self.paths {
                widgets::text(ui, copy::vault::LOCKED_VAULT_PATH, Type::Small, t.text_secondary);
                ui.add_space(space::XXS);
                widgets::elided(
                    ui,
                    &paths.config_dir.display().to_string(),
                    Type::MonoSmall,
                    t.text_muted,
                    400.0,
                    // From the left: two installations differ at the end of
                    // the path, not the beginning.
                    true,
                );
                ui.add_space(space::L);
            }

            // The status strip's own three facts, on the screen that has no
            // status strip.
            let running = self.data.link_up;
            let installed = self.data.snapshot.as_ref().is_some_and(|s| s.service_installed);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(
                    rect.center(),
                    4.0,
                    if running { t.success.mark } else { t.danger.mark },
                );
                ui.add_space(space::S);
                widgets::text(ui, copy::locked_daemon(running), Type::Small, t.text_muted);
                ui.add_space(space::L);
                widgets::text(ui, copy::locked_service(installed), Type::Small, t.text_muted);
                ui.add_space(space::L);
                widgets::text(
                    ui,
                    format!("superbackup {}", superbackup_core::VERSION),
                    Type::Small,
                    t.text_muted,
                );
            });

            ui.add_space(space::L);
            ui.horizontal(|ui| {
                if Button::ghost(copy::vault::LOCKED_OPEN_OTHER)
                    .compact()
                    .show(ui)
                    .on_hover_text(copy::vault::LOCKED_OPEN_OTHER_HINT)
                    .clicked()
                {
                    open_other = true;
                }
                if Button::ghost(copy::vault::LOCKED_NEW_VAULT)
                    .compact()
                    .show(ui)
                    .on_hover_text(copy::vault::LOCKED_NEW_VAULT_HINT)
                    .clicked()
                {
                    create_new = true;
                }
                if Button::ghost(copy::vault::LOCKED_ABOUT).compact().show(ui).clicked() {
                    about = true;
                }
            });
        });

        if open_other {
            self.switch_vault(ui.ctx(), false);
        }
        if create_new {
            self.switch_vault(ui.ctx(), true);
        }
        if about {
            // A dialog rather than the About *page*: the page lives behind the
            // lock screen, which this window is not drawing. Everything on it
            // describes the program rather than the installation, so none of
            // it is behind the lock in the first place.
            self.open_modal(crate::gui::modals::Modal::Confirm(self.locked_about()));
        }
    }

    /// The About page's facts, in a dialog a locked window can show.
    fn locked_about(&self) -> crate::gui::modals::Confirm {
        let mut confirm = crate::gui::modals::Confirm::new(
            copy::vault::LOCKED_ABOUT,
            copy::about::TAGLINE,
            copy::action::CLOSE,
        )
        .bullet(format!("Version {}", superbackup_core::VERSION))
        .bullet(copy::about::LICENCE_SELF)
        .bullet(copy::about::LICENCE_KOPIA);
        if let Some(paths) = &self.paths {
            confirm = confirm.bullet(format!("Vault: {}", paths.config_dir.display()));
        }
        confirm.destructive = false;
        confirm
    }

    /// Point superbackup at a different configuration folder.
    ///
    /// Restarts, and has to: the configuration root is fixed when the process
    /// starts — it decides the IPC endpoint, which daemon this window talks
    /// to, and where every path is rooted — so changing it in place would
    /// leave a window addressing one installation and a daemon serving
    /// another.
    ///
    /// `create` says which mistake to refuse. Opening a folder with no vault
    /// in it, and creating one in a folder that already has a vault, are both
    /// the user having picked the wrong button, and neither is what they meant.
    fn switch_vault(&mut self, ctx: &egui::Context, create: bool) {
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let paths = superbackup_core::paths::Paths::rooted_at(&folder, false);
        let exists = superbackup_core::config::is_initialised(&paths);
        if create && exists {
            self.toasts.warning(copy::vault::LOCKED_ALREADY_A_VAULT);
            return;
        }
        if !create && !exists {
            self.toasts.warning(copy::vault::LOCKED_NOT_A_VAULT);
            return;
        }

        let started = superbackup_core::ipc::client::AutoStart::current_exe(&[
            "--home",
            &folder.display().to_string(),
        ])
        .and_then(|autostart| autostart.spawn());
        match started {
            // This window is now looking at the wrong installation, and the
            // new one owns the screen.
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => self.toasts.danger(copy::vault::LOCKED_OPEN_OTHER, e.to_string()),
        }
    }

    /// Which machine this is. On a locked screen it is the only way to tell one
    /// superbackup window from another.
    fn locked_identity(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
        crate::gui::icons::health_mark(
            ui.painter(),
            rect,
            superbackup_core::state::Health::Paused,
            t.text_muted,
            None,
            0.0,
        );
        ui.add_space(space::L);
        widgets::text(ui, self.data.machine_label(), Type::H2, t.text_primary);
        ui.add_space(space::XXS);
        widgets::text(ui, copy::vault::LOCKED_TITLE, Type::Body, t.text_secondary);
    }

    /// The single unlock control in the application.
    fn locked_unlock(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let busy = self.screens.locked.busy;
        let mut submit = false;
        let mut use_passkey = None;

        ui.allocate_ui_with_layout(Vec2::new(360.0, 0.0), Layout::top_down(Align::Center), |ui| {
            widgets::paragraph_at(ui, copy::vault::LOCKED_BODY, Type::Small, t.text_muted, 360.0);
            ui.add_space(space::XL);

            let error = self.screens.locked.error.clone();
            let response = widgets::passphrase_field(
                ui,
                &mut self.screens.locked.passphrase,
                copy::vault::UNLOCK_FIELD,
                &mut self.screens.locked.revealed,
                error.as_deref(),
                360.0,
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            // Focus once, not every frame: re-requesting it each pass eats
            // clicks aimed at anything else on the screen.
            if !self.screens.locked.focused {
                response.request_focus();
                self.screens.locked.focused = true;
            }

            if self.data.settings.use_os_keychain {
                ui.add_space(space::L);
                widgets::checkbox(
                    ui,
                    &mut self.screens.locked.remember,
                    copy::vault::UNLOCK_REMEMBER,
                    None,
                    true,
                );
            }

            ui.add_space(space::XL);
            let label = if busy { copy::vault::UNLOCK_BUSY } else { copy::vault::UNLOCK_BUTTON };
            if Button::primary(label)
                .busy(busy)
                .enabled(!busy && !self.screens.locked.passphrase.trim().is_empty())
                .show(ui)
                .clicked()
            {
                submit = true;
            }

            // The other way in, offered only where there is one.
            //
            // Below the passphrase and not above it: the passphrase is the way
            // that always works, and a passkey is an addition. Putting it
            // first would suggest the machine prefers it, on a screen where
            // the honest hierarchy matters — a lost authenticator must not
            // read like a lost vault.
            if let Some(entry) = self.first_passkey() {
                ui.add_space(space::L);
                let waiting = self.screens.locked.passkey.is_some();
                let label = match &self.screens.locked.passkey {
                    Some(pending) => pending.line,
                    None => copy::vault::UNLOCK_PASSKEY,
                };
                if Button::secondary(label)
                    .busy(waiting)
                    .enabled(!waiting && !busy)
                    .show(ui)
                    .on_hover_text(copy::vault::UNLOCK_PASSKEY_HINT)
                    .clicked()
                {
                    use_passkey = Some(entry);
                }
            }

            // A passphrase that cannot be recovered is worth saying once
            // the user has clearly stopped remembering it, and not before.
            if self.screens.locked.attempts >= 3 {
                ui.add_space(space::L);
                widgets::paragraph_at(
                    ui,
                    copy::vault::UNLOCK_NO_RECOVERY,
                    Type::Small,
                    t.text_muted,
                    360.0,
                );
            }
        });

        if submit && !self.screens.locked.passphrase.trim().is_empty() && !busy {
            self.screens.locked.busy = true;
            self.screens.locked.error = None;
            let passphrase = self.screens.locked.passphrase.clone();
            self.unlock(passphrase);
        }

        if let Some(entry) = use_passkey {
            if let Some(paths) = self.paths.clone() {
                self.screens.locked.error = None;
                self.screens.locked.passkey =
                    Some(crate::gui::passkey::Pending::unlock(paths, entry));
            }
        }
    }

    /// The passkey to offer, when this machine has one and can use it.
    ///
    /// The first enrolled, not a chooser. Most people have one, and an unlock
    /// screen is the wrong place to make somebody pick from a list before they
    /// can get in — an authenticator that is not the one asked for simply
    /// refuses, and they can try the next.
    fn first_passkey(&self) -> Option<superbackup_core::credentials::passkey::Enrolled> {
        let paths = self.paths.as_ref()?;
        if !superbackup_core::platform::webauthn::support().usable() {
            return None;
        }
        superbackup_core::credentials::passkey::list(paths).into_iter().next()
    }

    /// Take whatever the authenticator said, once per frame.
    ///
    /// Called from the frame rather than from the button, because the answer
    /// arrives up to a minute after the click.
    pub(crate) fn poll_passkey(&mut self) {
        let Some(pending) = &mut self.screens.locked.passkey else {
            return;
        };
        let Some(outcome) = pending.poll() else {
            return;
        };
        self.screens.locked.passkey = None;
        match outcome {
            crate::gui::passkey::Outcome::Unlocked(passphrase) => {
                // Straight to the ordinary unlock. The daemon is not told a
                // passkey was involved, and does not need to be: what it is
                // handed is a passphrase somebody proved they were entitled
                // to.
                self.screens.locked.busy = true;
                self.unlock(passphrase.expose_str().unwrap_or_default().to_string());
            }
            crate::gui::passkey::Outcome::Failed(why) => self.screens.locked.fail(why),
            crate::gui::passkey::Outcome::Enrolled(_) => {}
        }
    }

    /// The one question worth answering without a passphrase: did the backups
    /// run, and how did they end?
    fn locked_recent(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();
        let runs: Vec<_> = self.data.history.iter().take(RECENT_RUNS).cloned().collect();

        ui.allocate_ui_with_layout(Vec2::new(420.0, 0.0), Layout::top_down(Align::Center), |ui| {
            widgets::text(ui, copy::vault::LOCKED_RECENT, Type::BodyStrong, t.text_secondary);
            ui.add_space(space::M);

            if runs.is_empty() {
                widgets::text(ui, copy::vault::LOCKED_RECENT_NONE, Type::Small, t.text_muted);
                return;
            }

            for run in &runs {
                egui::Frame::new()
                    .fill(t.bg_surface)
                    .corner_radius(radius::CARD)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (mark, colour) = status_mark(run.status, &t);
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                            mark.paint(ui.painter(), rect, colour);
                            ui.add_space(space::M);

                            ui.vertical(|ui| {
                                widgets::elided(
                                    ui,
                                    &run.job_name,
                                    Type::Body,
                                    t.text_primary,
                                    200.0,
                                    false,
                                );
                                ui.add_space(space::XXS);
                                widgets::text(
                                    ui,
                                    locked_status_word(run.status),
                                    Type::Small,
                                    colour,
                                );
                            });

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                widgets::text(
                                    ui,
                                    format::relative(run.started_at, now),
                                    Type::MonoSmall,
                                    t.text_muted,
                                );
                            });
                        });
                    });
                ui.add_space(space::S);
            }
        });
    }
}

/// The four outcomes a person actually cares about, in their words.
///
/// Deliberately not `RunStatus::title()`: that vocabulary is the engine's and
/// covers states this screen never shows — a run cannot be `Running` here,
/// because the scheduler will not start one against a locked vault.
fn locked_status_word(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Succeeded => copy::vault::RUN_COMPLETED,
        RunStatus::SucceededWithWarnings => copy::vault::RUN_WARNINGS,
        RunStatus::Failed => copy::vault::RUN_ERROR,
        RunStatus::Skipped => copy::vault::RUN_MISSED,
        RunStatus::Cancelled => copy::vault::RUN_STOPPED,
        // A run left mid-flight by a daemon that stopped. Saying "unfinished"
        // is honest; claiming it succeeded or failed would not be.
        RunStatus::Queued | RunStatus::Preparing | RunStatus::Running | RunStatus::Finalising => {
            copy::vault::RUN_UNFINISHED
        }
    }
}

fn status_mark(status: RunStatus, t: &theme::Tokens) -> (Icon, egui::Color32) {
    match status {
        RunStatus::Succeeded => (Icon::CheckCircle, t.success.mark),
        RunStatus::SucceededWithWarnings => (Icon::AlertTriangle, t.warning.mark),
        RunStatus::Failed => (Icon::XOctagon, t.danger.mark),
        RunStatus::Skipped => (Icon::Clock, t.text_muted),
        _ => (Icon::Clock, t.text_muted),
    }
}

/// What the lock screen is holding between frames.
#[derive(Debug, Default)]
pub struct State {
    pub passphrase: String,
    pub revealed: bool,
    pub remember: bool,
    pub busy: bool,
    pub attempts: u32,
    pub error: Option<String>,
    /// Focus is claimed once. See `locked_unlock`.
    pub focused: bool,
    /// The authenticator prompt that is up, if one is.
    pub passkey: Option<crate::gui::passkey::Pending>,
}

impl State {
    /// A rejected passphrase keeps the text, so a single typo can be corrected
    /// rather than retyped.
    pub fn fail(&mut self, message: String) {
        self.busy = false;
        self.attempts += 1;
        self.error = Some(message);
    }

    /// Everything is cleared the moment the vault opens: the passphrase must
    /// not sit in the window's memory for the rest of the session, and the
    /// next lock has to start from a blank field rather than a stale error.
    pub fn clear(&mut self) {
        self.passphrase.clear();
        self.revealed = false;
        self.busy = false;
        self.attempts = 0;
        self.error = None;
        self.focused = false;
        // Dropping the handle does not cancel the operating system's prompt —
        // nothing here can — but the vault is open, so whatever it returns has
        // nothing left to do.
        self.passkey = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_terminal_status_has_a_word_of_its_own() {
        // The four the user asked to be able to tell apart.
        let words = [
            locked_status_word(RunStatus::Succeeded),
            locked_status_word(RunStatus::SucceededWithWarnings),
            locked_status_word(RunStatus::Failed),
            locked_status_word(RunStatus::Skipped),
        ];
        let mut unique = words.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), words.len(), "two outcomes must never read the same: {words:?}");
        assert!(words.iter().all(|w| !w.is_empty()));
    }

    #[test]
    fn an_unfinished_run_is_not_reported_as_success_or_failure() {
        // A daemon killed mid-run leaves these behind. Claiming either outcome
        // would be a lie about whether the data is safe.
        for status in
            [RunStatus::Queued, RunStatus::Preparing, RunStatus::Running, RunStatus::Finalising]
        {
            let word = locked_status_word(status);
            assert_eq!(word, copy::vault::RUN_UNFINISHED, "{status:?} claimed an outcome");
        }
    }

    #[test]
    fn a_failed_unlock_keeps_the_passphrase_and_counts_the_attempt() {
        let mut state = State { passphrase: "corect horse".into(), busy: true, ..State::default() };
        state.fail("That passphrase did not open the vault.".into());
        assert_eq!(state.passphrase, "corect horse", "a typo must be correctable, not retyped");
        assert_eq!(state.attempts, 1);
        assert!(!state.busy);
        assert!(state.error.is_some());
    }

    #[test]
    fn unlocking_leaves_no_passphrase_behind() {
        let mut state = State {
            passphrase: "correct horse battery staple".into(),
            revealed: true,
            attempts: 2,
            error: Some("nope".into()),
            focused: true,
            ..State::default()
        };
        state.clear();
        assert!(state.passphrase.is_empty(), "the passphrase must not outlive the unlock");
        assert!(!state.revealed);
        assert_eq!(state.attempts, 0);
        assert!(state.error.is_none());
        assert!(!state.focused, "the next lock must claim focus again");
    }
}
