//! `O-1` … `O-7`. Seven steps at 880 × 640, no rail and no status strip.
//!
//! Steps 1–3 are mandatory: without a master passphrase the application cannot
//! store a single credential. The vault is created when `Continue` is pressed
//! on O-3, not on O-2, so a user who backs out has not left a half-initialised
//! vault behind.

use egui::{Align, Layout, Sense, Ui, Vec2};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::daemon::Intent;
use crate::gui::icons::{self, Icon};
use crate::gui::screens::wizard::Template;
use crate::gui::theme::{self, radius, space, Type};
use crate::gui::validation::{self, OnboardingStep};
use crate::gui::widgets::{self, Button};
use superbackup_core::ipc::protocol::Request;

#[derive(Debug, Clone)]
pub struct Onboarding {
    pub step: OnboardingStep,
    pub passphrase: String,
    pub confirm: String,
    pub revealed: bool,
    pub acknowledged: bool,
    pub weak_acknowledged: bool,
    pub template: Option<Template>,
    pub create_onedrive: bool,
    pub autostart: bool,
    /// Add superbackup to the Start menu / applications launcher.
    pub create_shortcut: bool,
    pub start_minimised: bool,
    pub install_service: bool,
    pub use_keychain: bool,
    pub scan_done: bool,
    /// Why the vault could not be created, when that is what happened.
    pub vault_error: Option<String>,
    /// Set once the vault exists, so leaving and re-entering the step cannot
    /// try to create a second one over the top of the first.
    pub vault_created: bool,
}

impl Default for Onboarding {
    fn default() -> Onboarding {
        Onboarding {
            // Being findable in the applications menu is the one choice with
            // no downside: it costs a file under the user's own profile and
            // makes the program possible to start a second time. Running at
            // every login is a real imposition and is *not* defaulted on.
            create_shortcut: true,
            autostart: false,
            start_minimised: true,
            step: OnboardingStep::default(),
            passphrase: String::new(),
            confirm: String::new(),
            revealed: false,
            acknowledged: false,
            weak_acknowledged: false,
            template: None,
            create_onedrive: false,
            install_service: false,
            use_keychain: false,
            scan_done: false,
            vault_error: None,
            vault_created: false,
        }
    }
}

impl Onboarding {
    pub fn score(&self) -> u8 {
        validation::passphrase_score(&self.passphrase)
    }

    /// `Continue` is enabled at twelve characters with a matching
    /// confirmation; a weak score does not block, it adds friction on O-3.
    pub fn can_continue(&self) -> bool {
        match self.step {
            OnboardingStep::Passphrase => {
                validation::master_passphrase(&self.passphrase, &self.confirm).ok()
            }
            OnboardingStep::NoRecovery => {
                self.acknowledged && (self.score() >= 2 || self.weak_acknowledged)
            }
            _ => true,
        }
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let t = theme::tokens(ui.ctx());
    let Some(mut state) = app.onboarding.take() else {
        return;
    };

    let full = ui.max_rect();
    let mut content = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                egui::Pos2::new(full.left() + 40.0, full.top() + 32.0),
                egui::Pos2::new(full.right() - 40.0, full.bottom() - 72.0),
            ))
            .layout(Layout::top_down(Align::Min)),
    );

    // The step indicator: seven dots, the current one 20px wide.
    content.allocate_ui_with_layout(
        Vec2::new(content.available_width(), 24.0),
        Layout::top_down(Align::Center),
        |ui| {
            ui.horizontal(|ui| {
                let total = OnboardingStep::ALL.len();
                let width = total as f32 * 6.0 + (total - 1) as f32 * 8.0 + 14.0;
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                for step in OnboardingStep::ALL {
                    let current = step == state.step;
                    let done = step.index() < state.step.index();
                    let size = if current { Vec2::new(20.0, 6.0) } else { Vec2::splat(6.0) };
                    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                    let colour = if current {
                        t.accent
                    } else if done {
                        t.text_muted
                    } else {
                        t.border_strong
                    };
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(3), colour);
                    ui.add_space(2.0);
                }
            });
            ui.add_space(space::M);
            widgets::text(
                ui,
                format!("Step {} of {}", state.step.index() + 1, OnboardingStep::ALL.len()),
                Type::Small,
                t.text_muted,
            );
        },
    );
    content.add_space(space::H2);

    widgets::scroll_area(&mut content, ("onboarding", state.step), |ui| match state.step {
        OnboardingStep::Welcome => welcome(ui),
        OnboardingStep::Passphrase => passphrase(ui, &mut state),
        OnboardingStep::NoRecovery => no_recovery(ui, &mut state, app),
        OnboardingStep::Scan => scan(ui, &mut state, app),
        OnboardingStep::FirstJob => first_job(ui, &mut state, app),
        OnboardingStep::KeepRunning => keep_running(ui, &mut state),
        OnboardingStep::Done => done(ui, app),
    });

    // The fixed 72px footer.
    let footer = egui::Rect::from_min_max(
        egui::Pos2::new(full.left(), full.bottom() - 72.0),
        full.right_bottom(),
    );
    ui.painter().rect_filled(
        egui::Rect::from_min_size(footer.left_top(), Vec2::new(footer.width(), 1.0)),
        0,
        t.border_subtle,
    );
    let mut footer_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(footer.shrink2(Vec2::new(40.0, 20.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let mut advance = false;
    let mut back = false;
    let mut skip = false;
    if state.step.previous().is_some()
        && Button::ghost(copy::action::BACK).onboarding().show(&mut footer_ui).clicked()
    {
        back = true;
    }
    if state.step.skippable() {
        footer_ui.add_space(space::L);
        if Button::ghost("Skip setup").onboarding().show(&mut footer_ui).clicked() {
            skip = true;
        }
    }
    footer_ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let label = match state.step {
            OnboardingStep::Done => copy::onboarding::DONE_PRIMARY,
            _ => copy::action::CONTINUE,
        };
        if Button::primary(label).onboarding().enabled(state.can_continue()).show(ui).clicked() {
            advance = true;
        }
        if state.step == OnboardingStep::Done
            && Button::ghost(copy::onboarding::DONE_SECONDARY).onboarding().show(ui).clicked()
        {
            skip = true;
        }
    });

    if advance {
        // Leaving the acknowledgement step is the moment the passphrase has
        // been typed, confirmed, and understood to be unrecoverable — so it is
        // the moment the vault gets written.
        //
        // The window does this itself rather than asking the daemon, because
        // on a first run there is no daemon: it refuses to start until the
        // vault exists. That is the same reason `superbackup init` writes the
        // file directly, and it does not weaken the thin-client rule, which
        // exists to stop two processes driving one kopia repository. No
        // repository is touched here.
        if state.step == OnboardingStep::NoRecovery && !state.vault_created {
            match create_vault(app, &state.passphrase) {
                Ok(()) => {
                    state.vault_created = true;
                    state.vault_error = None;
                }
                Err(message) => {
                    // Stay on the step. Advancing past a failed vault would
                    // walk the user through the rest of setup and then strand
                    // them with nothing stored.
                    state.vault_error = Some(message);
                    app.onboarding = Some(state);
                    return;
                }
            }
        }
        match state.step.next() {
            Some(next) => state.step = next,
            None => {
                // Apply what was chosen on the "keep it running" step.
                //
                // Those switches were rendered, stored and then discarded:
                // nothing read `autostart` or `install_service`, so a user who
                // asked for both got neither and had no way to tell. They are
                // applied here, each independently, so one failing does not
                // silently drop the others.
                apply_setup_choices(app, &state);
                app.onboarding = None;
                app.request_run_all();
                return;
            }
        }
    }
    if back {
        if let Some(previous) = state.step.previous() {
            state.step = previous;
        }
    }
    if skip {
        app.onboarding = None;
        return;
    }
    app.onboarding = Some(state);
}

/// Write the vault, and unlock this session against it.
///
/// Returns the message to show on the step when it could not be done. Every
/// failure here is worth stopping for: a full disk, a read-only profile, or a
/// vault that appeared underneath us all mean the passphrase the user just
/// chose was not stored.
fn create_vault(app: &mut App, passphrase: &str) -> Result<(), String> {
    let Some(paths) = app.paths.clone() else {
        // A window with no paths is a test or the screenshot harness. It has
        // no business writing a vault, and saying so is better than pretending
        // the step succeeded.
        return Err(copy::onboarding::VAULT_NO_PATHS.to_string());
    };
    paths.ensure().map_err(|e| copy::onboarding_vault_failed(&e.to_string()))?;

    if superbackup_core::config::is_initialised(&paths) {
        // Somebody ran `superbackup init` in another window while this one was
        // open. Refusing is the only safe answer: initialising over the top
        // would replace a vault that may already hold repository keys.
        return Err(copy::onboarding::VAULT_ALREADY.to_string());
    }

    let secret = superbackup_core::secret::Secret::from_string(passphrase.to_string());
    superbackup_core::config::Store::initialise(paths, &secret)
        .map_err(|e| copy::onboarding_vault_failed(&e.to_string()))?;

    // Unlock the session with the passphrase just chosen, so the user is not
    // asked for it again on the very next screen. If no daemon is listening
    // yet the request simply fails and the normal unlock prompt does the job
    // later, which is why nothing here depends on its answer.
    app.unlock(passphrase.to_string());
    Ok(())
}

/// Carry out the choices made on the last step.
///
/// Each is a separate request, and deliberately so: the applications-menu
/// entry, starting at login, and installing a service are three different
/// mechanisms that fail for three different reasons. Bundling them would mean
/// a refused elevation prompt for the service silently costing the user their
/// Start-menu entry as well.
///
/// Nothing here blocks. The replies arrive as ordinary toasts, and the
/// interface is usable while they land.
fn apply_setup_choices(app: &mut App, state: &Onboarding) {
    if state.create_shortcut {
        app.ask(Intent::Fire, Request::AppSetShortcut { enabled: true });
    }
    if state.autostart {
        app.ask(Intent::Fire, Request::ServiceSetAutostart { enabled: true });
    }
    if state.install_service {
        // Elevation is prompted for by the daemon, and refusing it is a
        // normal outcome rather than an error worth blocking setup over.
        app.ask(Intent::Fire, Request::ServiceInstall {});
    }
}

fn welcome(ui: &mut Ui) {
    let t = theme::tokens(ui.ctx());
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 0.0),
        Layout::top_down(Align::Center),
        |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
            icons::health_mark(
                ui.painter(),
                rect,
                superbackup_core::state::Health::Idle,
                t.accent,
                None,
                0.0,
            );
            ui.add_space(space::H3);
            widgets::text(ui, copy::onboarding::WELCOME_TITLE, Type::Display, t.text_primary);
            ui.add_space(space::L);
            ui.allocate_ui_with_layout(
                Vec2::new(520.0, 0.0),
                Layout::top_down(Align::Center),
                |ui| {
                    widgets::paragraph_at(
                        ui,
                        copy::onboarding::WELCOME_BODY,
                        Type::Body,
                        t.text_secondary,
                        520.0,
                    );
                },
            );
        },
    );

    ui.add_space(space::H2);
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 0.0),
        Layout::top_down(Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(Vec2::new(520.0, 0.0), Layout::top_down(Align::Min), |ui| {
                for (icon, title, body) in [
                    (
                        Icon::Repeat,
                        copy::onboarding::WELCOME_F1_TITLE,
                        copy::onboarding::WELCOME_F1_BODY,
                    ),
                    (
                        Icon::FilterX,
                        copy::onboarding::WELCOME_F2_TITLE,
                        copy::onboarding::WELCOME_F2_BODY,
                    ),
                    (
                        Icon::Lock,
                        copy::onboarding::WELCOME_F3_TITLE,
                        copy::onboarding::WELCOME_F3_BODY,
                    ),
                ] {
                    ui.horizontal_top(|ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                        icon.paint(ui.painter(), rect, t.accent);
                        ui.add_space(space::L);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = space::XXS;
                            widgets::text(ui, title, Type::BodyStrong, t.text_primary);
                            widgets::paragraph_at(ui, body, Type::Small, t.text_secondary, 460.0);
                        });
                    });
                    ui.add_space(space::XL);
                }
                widgets::text(ui, copy::onboarding::WELCOME_KOPIA, Type::Small, t.text_muted);
            });
        },
    );
}

fn passphrase(ui: &mut Ui, state: &mut Onboarding) {
    let t = theme::tokens(ui.ctx());
    let report = validation::master_passphrase(&state.passphrase, &state.confirm);
    let score = state.score();

    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(Vec2::new(360.0, 0.0), Layout::top_down(Align::Min), |ui| {
            widgets::text(ui, copy::onboarding::PASS_TITLE, Type::Display, t.text_primary);
            ui.add_space(space::XL);
            widgets::paragraph_at(
                ui,
                copy::onboarding::PASS_LEAD,
                Type::Body,
                t.text_secondary,
                340.0,
            );
            ui.add_space(space::L);
            widgets::paragraph_at(
                ui,
                copy::onboarding::PASS_NOT_REPO,
                Type::Small,
                t.text_muted,
                340.0,
            );
        });
        ui.add_space(space::H1);
        ui.allocate_ui_with_layout(Vec2::new(400.0, 0.0), Layout::top_down(Align::Min), |ui| {
            let mut revealed = state.revealed;
            widgets::passphrase_field(
                ui,
                &mut state.passphrase,
                copy::onboarding::PASS_FIELD,
                &mut revealed,
                report.for_field(validation::Field::Passphrase),
                400.0,
            );
            ui.add_space(space::M);
            widgets::strength_meter(ui, score, 366.0);

            ui.add_space(space::XL);
            for (met, label) in [
                (state.passphrase.chars().count() >= 12, copy::onboarding::PASS_REQ_LENGTH),
                // Unverifiable, so never a green tick.
                (false, copy::onboarding::PASS_REQ_UNIQUE),
                (false, copy::onboarding::PASS_REQ_WORDS),
            ] {
                ui.horizontal(|ui| {
                    ui.set_min_height(20.0);
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    if met {
                        Icon::CheckCircle.paint(ui.painter(), rect, t.success.mark);
                    } else {
                        Icon::Circle.paint(ui.painter(), rect, t.text_muted);
                    }
                    ui.add_space(space::M);
                    widgets::text(
                        ui,
                        label,
                        Type::Small,
                        if met { t.text_primary } else { t.text_muted },
                    );
                });
            }

            ui.add_space(space::XL);
            // Mismatch is shown on blur or submit, never while typing.
            let confirm_error = (!state.confirm.is_empty())
                .then(|| report.for_field(validation::Field::PassphraseConfirm))
                .flatten();
            widgets::passphrase_field(
                ui,
                &mut state.confirm,
                copy::onboarding::PASS_CONFIRM,
                &mut revealed,
                confirm_error,
                400.0,
            );
            state.revealed = revealed;

            ui.add_space(space::XL);
            if Button::ghost(copy::onboarding::PASS_SUGGEST).show(ui).clicked() {
                let suggestion = diceware();
                state.passphrase = suggestion.clone();
                state.confirm = suggestion;
                state.revealed = true;
            }
        });
    });
}

fn no_recovery(ui: &mut Ui, state: &mut Onboarding, app: &mut App) {
    let t = theme::tokens(ui.ctx());
    // The vault is written when this step is left, so this is where its
    // failure has to appear — at the top, before the acknowledgement the user
    // is about to give again.
    if let Some(error) = state.vault_error.clone() {
        widgets::banner(ui, widgets::BannerKind::Danger, &error, None, |_| {});
        ui.add_space(space::L);
    }
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
        Icon::AlertTriangle.paint(ui.painter(), rect, t.warning.mark);
        ui.add_space(space::L);
        widgets::text(ui, copy::onboarding::NORECOVERY_TITLE, Type::Display, t.text_primary);
    });
    ui.add_space(space::XL);
    widgets::paragraph_at(
        ui,
        copy::onboarding::NORECOVERY_BODY,
        Type::Body,
        t.text_secondary,
        560.0,
    );

    ui.add_space(space::H3);
    egui::Frame::new()
        .fill(t.bg_raised)
        .corner_radius(radius::CARD)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(560.0));
            ui.horizontal(|ui| {
                if Button::secondary(copy::onboarding::NORECOVERY_COPY)
                    .icon(Icon::Copy)
                    .show(ui)
                    .clicked()
                {
                    ui.ctx().copy_text(state.passphrase.clone());
                    app.toasts.info(copy::onboarding::NORECOVERY_COPIED);
                }
                if Button::secondary(copy::onboarding::NORECOVERY_SAVE)
                    .icon(Icon::FileText)
                    .show(ui)
                    .clicked()
                {
                    app.toasts.info(copy::onboarding::NORECOVERY_SAVE_NOTE);
                }
            });
            ui.add_space(space::M);
            widgets::paragraph_at(
                ui,
                copy::onboarding::NORECOVERY_SAVE_NOTE,
                Type::Small,
                t.text_muted,
                500.0,
            );
        });

    ui.add_space(space::H3);
    if state.score() <= 1 {
        let mut weak = state.weak_acknowledged;
        if widgets::checkbox(ui, &mut weak, copy::onboarding::WEAK_ACK, None, true).clicked() {
            state.weak_acknowledged = weak;
        }
        ui.add_space(space::L);
    }
    let mut acknowledged = state.acknowledged;
    if widgets::checkbox(ui, &mut acknowledged, copy::onboarding::NORECOVERY_ACK, None, true)
        .clicked()
    {
        state.acknowledged = acknowledged;
    }
}

fn scan(ui: &mut Ui, state: &mut Onboarding, app: &mut App) {
    let t = theme::tokens(ui.ctx());
    widgets::text(ui, copy::onboarding::SCAN_TITLE, Type::Display, t.text_primary);
    ui.add_space(space::M);
    widgets::paragraph_at(ui, copy::onboarding::SCAN_LEAD, Type::Body, t.text_secondary, 560.0);
    ui.add_space(space::H3);

    // Probe 1: kopia.
    let kopia = app.data.snapshot.as_ref().and_then(|s| s.kopia_version.clone());
    widgets::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
            match &kopia {
                Some(_) => Icon::CheckCircle.paint(ui.painter(), rect, t.success.mark),
                None => Icon::AlertTriangle.paint(ui.painter(), rect, t.warning.mark),
            }
            ui.add_space(space::L);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = space::XXS;
                match &kopia {
                    Some(version) => {
                        widgets::text(
                            ui,
                            copy::onboarding_kopia_found(version),
                            Type::BodyStrong,
                            t.text_primary,
                        );
                    }
                    None => {
                        widgets::text(
                            ui,
                            copy::onboarding::KOPIA_MISSING,
                            Type::BodyStrong,
                            t.text_primary,
                        );
                        widgets::paragraph_at(
                            ui,
                            copy::onboarding::KOPIA_MISSING_BODY,
                            Type::Small,
                            t.text_secondary,
                            480.0,
                        );
                    }
                }
            });
            if kopia.is_none() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let _ = Button::secondary(copy::onboarding::KOPIA_CHOOSE).compact().show(ui);
                    let _ = Button::primary(copy::onboarding::KOPIA_DOWNLOAD).compact().show(ui);
                });
            }
        });
    });
    ui.add_space(space::L);

    // Probe 2: OneDrive.
    let onedrive = superbackup_core::platform::onedrive::detect();
    widgets::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
            if onedrive.is_empty() {
                Icon::MinusCircle.paint(ui.painter(), rect, t.neutral.mark);
            } else {
                Icon::CheckCircle.paint(ui.painter(), rect, t.success.mark);
            }
            ui.add_space(space::L);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = space::XXS;
                if onedrive.is_empty() {
                    widgets::paragraph_at(
                        ui,
                        copy::onboarding::ONEDRIVE_NONE,
                        Type::Body,
                        t.text_secondary,
                        520.0,
                    );
                } else {
                    let account = onedrive[0].display_name.clone();
                    widgets::text(
                        ui,
                        copy::onboarding_onedrive_found(&account),
                        Type::BodyStrong,
                        t.text_primary,
                    );
                    widgets::elided(
                        ui,
                        &onedrive[0].path.to_string_lossy(),
                        Type::MonoSmall,
                        t.text_muted,
                        460.0,
                        false,
                    );
                    let mut create = state.create_onedrive;
                    if widgets::checkbox(
                        ui,
                        &mut create,
                        copy::onboarding::ONEDRIVE_CREATE,
                        Some(copy::onboarding::ONEDRIVE_EXPLAIN),
                        true,
                    )
                    .clicked()
                    {
                        state.create_onedrive = create;
                    }
                }
            });
        });
    });
    ui.add_space(space::L);

    // Probe 3: disk space.
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    widgets::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let space_info = superbackup_core::platform::disk_space(&home);
            let low = space_info.map(|(free, _)| free < 20 * 1024 * 1024 * 1024).unwrap_or(false);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
            if low {
                Icon::AlertTriangle.paint(ui.painter(), rect, t.warning.mark);
            } else {
                Icon::CheckCircle.paint(ui.painter(), rect, t.success.mark);
            }
            ui.add_space(space::L);
            match space_info {
                Some((free, _)) => {
                    let drive = home.to_string_lossy().chars().take(2).collect::<String>();
                    let line = if low {
                        copy::onboarding_disk_low(free, &drive)
                    } else {
                        copy::onboarding_disk_ok(free, &drive)
                    };
                    widgets::paragraph_at(ui, line, Type::Body, t.text_secondary, 520.0);
                }
                None => {
                    widgets::text(ui, copy::state::UNKNOWN, Type::Body, t.text_muted);
                }
            }
        });
    });
    state.scan_done = true;
}

fn first_job(ui: &mut Ui, state: &mut Onboarding, app: &mut App) {
    let t = theme::tokens(ui.ctx());
    widgets::text(ui, copy::onboarding::JOB_TITLE, Type::Display, t.text_primary);
    ui.add_space(space::M);
    widgets::paragraph_at(ui, copy::onboarding::JOB_LEAD, Type::Body, t.text_secondary, 560.0);
    ui.add_space(space::H3);

    let selected = state.template.unwrap_or(Template::Development);
    let width = ((ui.available_width() - space::XL) / 2.0).floor().min(340.0);
    let mut chosen: Option<Template> = None;
    for row in Template::ALL.chunks(2) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = space::XL;
            for template in row {
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 104.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        let is_selected = *template == selected;
                        let frame =
                            widgets::card_tinted(ui, None, is_selected.then_some(t.accent), |ui| {
                                ui.set_width(ui.available_width());
                                ui.set_height(72.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = space::XXS;
                                    widgets::text(ui, template.title(), Type::H2, t.text_primary);
                                    widgets::paragraph_at(
                                        ui,
                                        template.body(),
                                        Type::Small,
                                        t.text_secondary,
                                        (ui.available_width() - 8.0).max(120.0),
                                    );
                                    widgets::paragraph_at(
                                        ui,
                                        template.detail(),
                                        Type::Small,
                                        t.text_muted,
                                        (ui.available_width() - 8.0).max(120.0),
                                    );
                                });
                            });
                        let response = ui.interact(
                            frame.response.rect,
                            egui::Id::new("onboarding-template").with(template.title()),
                            Sense::click(),
                        );
                        if response.clicked() {
                            chosen = Some(*template);
                        }
                    },
                );
            }
        });
        ui.add_space(space::XL);
    }
    if let Some(template) = chosen {
        state.template = Some(template);
    }

    ui.add_space(space::L);
    widgets::paragraph_at(ui, copy::onboarding::JOB_DERIVED, Type::Small, t.text_muted, 560.0);
    ui.add_space(space::S);
    widgets::paragraph_at(ui, copy::onboarding::JOB_LATER, Type::Small, t.text_muted, 560.0);
    let _ = app;
}

fn keep_running(ui: &mut Ui, state: &mut Onboarding) {
    let t = theme::tokens(ui.ctx());
    widgets::text(ui, copy::onboarding::RUN_TITLE, Type::Display, t.text_primary);
    ui.add_space(space::M);
    widgets::paragraph_at(ui, copy::onboarding::RUN_LEAD, Type::Body, t.text_secondary, 560.0);
    ui.add_space(space::H3);

    widgets::card(ui, |ui| {
        ui.set_width(ui.available_width());
        let mut shortcut = state.create_shortcut;
        if widgets::toggle(
            ui,
            &mut shortcut,
            copy::onboarding::SHORTCUT_TITLE,
            Some(copy::onboarding::shortcut_body()),
            true,
        )
        .clicked()
        {
            state.create_shortcut = shortcut;
        }
    });

    ui.add_space(space::XL);
    widgets::card(ui, |ui| {
        ui.set_width(ui.available_width());
        let mut autostart = state.autostart;
        if widgets::toggle(
            ui,
            &mut autostart,
            copy::onboarding::AUTOSTART_TITLE,
            Some(copy::onboarding::AUTOSTART_BODY),
            true,
        )
        .clicked()
        {
            state.autostart = autostart;
        }
        ui.add_space(space::L);
        ui.horizontal(|ui| {
            ui.add_space(28.0);
            let mut minimised = state.start_minimised;
            if widgets::toggle(
                ui,
                &mut minimised,
                copy::onboarding::MINIMISED_TITLE,
                Some(copy::onboarding::MINIMISED_BODY),
                state.autostart,
            )
            .clicked()
            {
                state.start_minimised = minimised;
            }
        });
    });

    ui.add_space(space::XL);
    widgets::card(ui, |ui| {
        ui.set_width(ui.available_width());
        let mut service = state.install_service;
        if widgets::toggle(
            ui,
            &mut service,
            copy::onboarding::SERVICE_TITLE,
            Some(copy::onboarding::SERVICE_BODY),
            superbackup_core::platform::capabilities().system_service,
        )
        .clicked()
        {
            state.install_service = service;
            state.use_keychain = service;
        }
        // Why a service is worth it, next to the switch rather than buried in
        // documentation: this is the one decision on this screen whose
        // consequences are not obvious from its name.
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            ui.add_space(28.0);
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            Icon::Info.paint(ui.painter(), rect, t.text_muted);
            ui.add_space(space::S);
            let label =
                widgets::text(ui, copy::onboarding::SERVICE_WHY, Type::Small, t.text_secondary);
            response.union(label).on_hover_text(copy::onboarding::SERVICE_WHY_TOOLTIP);
        });
        if state.install_service {
            ui.add_space(space::L);
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                ui.vertical(|ui| {
                    let mut keychain = state.use_keychain;
                    if widgets::toggle(
                        ui,
                        &mut keychain,
                        &copy::onboarding_service_keychain(keychain_name()),
                        None,
                        true,
                    )
                    .clicked()
                    {
                        state.use_keychain = keychain;
                    }
                    ui.add_space(space::S);
                    widgets::paragraph_at(
                        ui,
                        copy::onboarding::SERVICE_KEYCHAIN_WARN,
                        Type::Small,
                        t.warning.tint_text,
                        460.0,
                    );
                });
            });
            ui.add_space(space::L);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                Icon::Shield.paint(ui.painter(), rect, t.text_muted);
                ui.add_space(space::M);
                widgets::text(ui, copy::onboarding::SERVICE_ELEVATE, Type::Small, t.text_muted);
            });
        }
    });
}

fn done(ui: &mut Ui, app: &mut App) {
    let t = theme::tokens(ui.ctx());
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 0.0),
        Layout::top_down(Align::Center),
        |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
            Icon::CheckCircle.paint(ui.painter(), rect, t.success.mark);
            ui.add_space(space::XL);
            widgets::text(ui, copy::onboarding::DONE_TITLE, Type::Display, t.text_primary);
            ui.add_space(space::H3);
        },
    );

    let jobs = format::plural(app.data.jobs.len(), "job", "jobs");
    let destinations = format::plural(app.data.destinations.len(), "destination", "destinations");
    let next = app
        .data
        .next_scheduled()
        .map(|(_, at)| crate::gui::format::relative_future(at, chrono::Utc::now()))
        .unwrap_or_else(|| copy::dash::NEXT_NONE.to_lowercase());
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 0.0),
        Layout::top_down(Align::Center),
        |ui| {
            widgets::text(
                ui,
                copy::onboarding_done_summary(&jobs, &destinations, &next),
                Type::Body,
                t.text_secondary,
            );
            ui.add_space(space::H3);
            ui.allocate_ui_with_layout(
                Vec2::new(520.0, 0.0),
                Layout::top_down(Align::Center),
                |ui| {
                    widgets::paragraph_at(
                        ui,
                        copy::onboarding::DONE_TRAY,
                        Type::Small,
                        t.text_muted,
                        520.0,
                    );
                },
            );
        },
    );
}

use crate::gui::format;

fn keychain_name() -> &'static str {
    if cfg!(windows) {
        "the Windows Credential Manager"
    } else if cfg!(target_os = "macos") {
        "the macOS Keychain"
    } else {
        "the Secret Service"
    }
}

/// A six-word suggestion from a small embedded list. Not a full diceware
/// wordlist — enough words that six of them are worth having, and the entropy
/// is stated rather than implied.
fn diceware() -> String {
    const WORDS: [&str; 64] = [
        "amber", "anchor", "atlas", "basin", "beacon", "birch", "bramble", "canyon", "cedar",
        "cinder", "clover", "cobalt", "copper", "coral", "cypress", "delta", "ember", "fathom",
        "fennel", "flint", "gable", "garnet", "granite", "harbour", "hazel", "heron", "indigo",
        "ivory", "juniper", "kestrel", "lantern", "larch", "linen", "lupin", "marble", "meadow",
        "mica", "nimbus", "nutmeg", "onyx", "orchard", "otter", "pebble", "pewter", "plover",
        "quartz", "quill", "ranger", "rowan", "saffron", "sable", "sierra", "slate", "sorrel",
        "spruce", "thistle", "timber", "topaz", "umber", "verdant", "walnut", "willow", "yarrow",
        "zephyr",
    ];
    let mut out: Vec<&str> = Vec::new();
    for _ in 0..6 {
        let index = (uuid::Uuid::new_v4().as_u128() % WORDS.len() as u128) as usize;
        out.push(WORDS[index]);
    }
    out.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_passphrase_step_needs_twelve_characters_and_a_match() {
        let mut state = Onboarding { step: OnboardingStep::Passphrase, ..Default::default() };
        assert!(!state.can_continue());
        state.passphrase = "short".into();
        state.confirm = "short".into();
        assert!(!state.can_continue());
        state.passphrase = "a-long-enough-passphrase".into();
        state.confirm = "a-long-enough-passphrase".into();
        assert!(state.can_continue());
    }

    #[test]
    fn a_weak_passphrase_needs_a_second_acknowledgement() {
        let mut state = Onboarding {
            step: OnboardingStep::NoRecovery,
            passphrase: "aaaaaaaaaaaaaa".into(),
            acknowledged: true,
            ..Default::default()
        };
        assert!(state.score() <= 1);
        assert!(!state.can_continue(), "a weak passphrase must be acknowledged separately");
        state.weak_acknowledged = true;
        assert!(state.can_continue());
    }

    #[test]
    fn a_strong_passphrase_needs_only_the_one_acknowledgement() {
        let state = Onboarding {
            step: OnboardingStep::NoRecovery,
            passphrase: "correct-horse-battery-staple-9".into(),
            acknowledged: true,
            ..Default::default()
        };
        assert!(state.can_continue());
    }

    /// A private home under the system temp directory, matching what the
    /// daemon tests do. No `tempfile` dependency exists in this crate, and
    /// adding one to a security-sensitive binary for four tests is not worth
    /// it; these directories are small and named for their owning process.
    fn fresh_home() -> superbackup_core::paths::Paths {
        let root = std::env::temp_dir().join(format!(
            "sb-onboarding-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        superbackup_core::paths::Paths::rooted_at(&root, false)
    }

    /// The switches on the last step must actually do something.
    ///
    /// They were rendered, stored on the state, and then dropped on the floor:
    /// nothing read `autostart` or `install_service`, so a user who asked for
    /// both got neither — and no error either, because nothing had been tried.
    /// This asserts the requests are issued, which is the part that was
    /// missing; whether the daemon can carry them out is its own concern.
    #[tokio::test]
    async fn the_setup_choices_reach_the_daemon() {
        let handler = std::sync::Arc::new(superbackup_core::ipc::testing::MockHandler::new());
        let ctx = egui::Context::default();
        let mut app = crate::gui::app::App::new_with_daemon(
            &ctx,
            std::sync::Arc::new(crate::gui::daemon::MockDaemon::new(handler.clone())),
        );

        let state = Onboarding {
            create_shortcut: true,
            autostart: true,
            install_service: true,
            ..Onboarding::default()
        };
        apply_setup_choices(&mut app, &state);

        // The bridge is asynchronous, so give the requests a moment to land.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if handler.calls("app.set_shortcut") > 0
                && handler.calls("service.set_autostart") > 0
                && handler.calls("service.install") > 0
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        assert!(
            handler.calls("app.set_shortcut") > 0,
            "the applications-menu entry was not asked for"
        );
        assert!(handler.calls("service.set_autostart") > 0, "autostart was not asked for");
        assert!(handler.calls("service.install") > 0, "the service was not asked for");
    }

    /// Nothing chosen means nothing done. A setup that silently installs a
    /// service nobody asked for is worse than one that installs nothing.
    #[tokio::test]
    async fn declining_everything_asks_for_nothing() {
        let handler = std::sync::Arc::new(superbackup_core::ipc::testing::MockHandler::new());
        let ctx = egui::Context::default();
        let mut app = crate::gui::app::App::new_with_daemon(
            &ctx,
            std::sync::Arc::new(crate::gui::daemon::MockDaemon::new(handler.clone())),
        );

        let state = Onboarding {
            create_shortcut: false,
            autostart: false,
            install_service: false,
            ..Onboarding::default()
        };
        apply_setup_choices(&mut app, &state);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        assert_eq!(handler.calls("app.set_shortcut"), 0);
        assert_eq!(handler.calls("service.set_autostart"), 0);
        assert_eq!(handler.calls("service.install"), 0);
    }

    /// Being findable is defaulted on; running at login and installing a
    /// service are not. Those are impositions and have to be asked for.
    #[test]
    fn only_the_harmless_choice_is_on_by_default() {
        let state = Onboarding::default();
        assert!(state.create_shortcut, "a menu entry costs a file and makes the app findable");
        assert!(!state.autostart, "running at every login must be chosen, not assumed");
        assert!(!state.install_service, "a service must be chosen, not assumed");
    }

    /// The regression this whole flow shipped with: `begin_onboarding` was
    /// called only by the screenshot harness, so a fresh install opened a
    /// window with no way to set anything up — and the tray, which refuses to
    /// start without a vault, exited before drawing anything at all.
    #[test]
    fn a_window_with_no_vault_starts_setup() {
        let paths = fresh_home();
        assert!(
            !superbackup_core::config::is_initialised(&paths),
            "a fresh home has no vault, which is the whole premise"
        );

        let ctx = egui::Context::default();
        let app = crate::gui::app::App::new_with_daemon(
            &ctx,
            std::sync::Arc::new(crate::gui::daemon::MockDaemon::new(std::sync::Arc::new(
                superbackup_core::ipc::testing::MockHandler::default(),
            ))),
        )
        .with_paths(paths);
        assert!(app.onboarding.is_some(), "a missing vault must open the setup flow");
    }

    #[test]
    fn a_window_with_a_vault_does_not_start_setup() {
        let paths = fresh_home();
        superbackup_core::config::Store::initialise(
            paths.clone(),
            &superbackup_core::secret::Secret::from_string("correct-horse-battery-staple".into()),
        )
        .expect("the vault must be creatable");

        let ctx = egui::Context::default();
        let app = crate::gui::app::App::new_with_daemon(
            &ctx,
            std::sync::Arc::new(crate::gui::daemon::MockDaemon::new(std::sync::Arc::new(
                superbackup_core::ipc::testing::MockHandler::default(),
            ))),
        )
        .with_paths(paths);
        assert!(
            app.onboarding.is_none(),
            "an installation that is already set up must not be walked through setup again"
        );
    }

    /// Creating a vault over one that already exists would replace keys that
    /// may be the only copy. It has to refuse, not overwrite.
    #[test]
    fn setup_refuses_to_write_over_an_existing_vault() {
        let paths = fresh_home();
        superbackup_core::config::Store::initialise(
            paths.clone(),
            &superbackup_core::secret::Secret::from_string("the-first-passphrase-here".into()),
        )
        .expect("the vault must be creatable");

        let ctx = egui::Context::default();
        let mut app = crate::gui::app::App::new_with_daemon(
            &ctx,
            std::sync::Arc::new(crate::gui::daemon::MockDaemon::new(std::sync::Arc::new(
                superbackup_core::ipc::testing::MockHandler::default(),
            ))),
        );
        app.paths = Some(paths);
        let result = create_vault(&mut app, "a-completely-different-one");
        assert!(result.is_err(), "an existing vault must never be replaced");
    }

    /// A window with no paths is a test or the screenshot harness. It must say
    /// so rather than reporting a success it did not achieve.
    #[test]
    fn setup_without_paths_reports_rather_than_pretending() {
        let ctx = egui::Context::default();
        let mut app = crate::gui::app::App::new_with_daemon(
            &ctx,
            std::sync::Arc::new(crate::gui::daemon::MockDaemon::new(std::sync::Arc::new(
                superbackup_core::ipc::testing::MockHandler::default(),
            ))),
        );
        assert!(create_vault(&mut app, "correct-horse-battery-staple").is_err());
    }

    #[test]
    fn the_suggestion_is_six_words() {
        let suggestion = diceware();
        assert_eq!(suggestion.split('-').count(), 6);
        assert!(suggestion.chars().count() >= 20);
    }
}
