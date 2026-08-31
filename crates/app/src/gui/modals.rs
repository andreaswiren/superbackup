//! Every modal in the product, and the one rule they all obey: never nested
//! (L13). A flow that needs two decisions is a multi-step modal with its own
//! internal step state, not a modal on top of a modal.

// The interface is a library-shaped tree inside a binary crate. Its components,
// view models and fixtures are also compiled by `crates/app/tests/gui_app.rs`
// as a separate crate, so items that are used and tested there look unused from
// the binary's side. The allow is scoped to this module rather than the crate.
#![allow(dead_code)]
use std::path::PathBuf;

use uuid::Uuid;

use superbackup_core::ipc::protocol::{ConflictPolicy, ProbeReply, Request};
use superbackup_core::model::{Destination, Job, PassphraseSource};

use super::app::App;
use super::copy;
use super::daemon::Intent;
use super::data::Data;
use super::format;
use super::icons::Icon;
pub use super::screens::wizard::WizardState;
use super::theme::{self, space, Type};
use super::validation;
use super::widgets::{self, Button, ModalSize};

/// What a confirmation actually does when it is confirmed. Kept as data rather
/// than a closure so a modal can be constructed, inspected and tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    DeleteJob(Uuid),
    DisableAllJobs,
    RemoveDestination { id: Uuid, delete_files: bool },
    DeleteProvider(Uuid),
    StopRun(Uuid),
    StopAll,
    ResetSettings,
    ResetVault,
    RemoveAllConfiguration,
    Nothing,
}

#[derive(Debug, Clone)]
pub struct Confirm {
    pub title: String,
    pub body: String,
    /// The specifics: what will happen, and what will not.
    pub bullets: Vec<String>,
    /// Bullets that describe something irreversible.
    pub danger_bullets: Vec<String>,
    /// The primary button's label, which always carries the verb and the
    /// object — never `OK`.
    pub verb: String,
    pub destructive: bool,
    /// When set, the primary button stays disabled until this exact string is
    /// typed. Case sensitive, by design.
    pub type_to_confirm: Option<String>,
    pub typed: String,
    /// An optional second decision, off by default, that escalates the modal.
    pub extra_toggle: Option<(String, String, bool)>,
    pub action: ConfirmAction,
}

impl Confirm {
    pub fn new(
        title: impl Into<String>,
        body: impl Into<String>,
        verb: impl Into<String>,
    ) -> Confirm {
        Confirm {
            title: title.into(),
            body: body.into(),
            bullets: Vec::new(),
            danger_bullets: Vec::new(),
            verb: verb.into(),
            destructive: true,
            type_to_confirm: None,
            typed: String::new(),
            extra_toggle: None,
            action: ConfirmAction::Nothing,
        }
    }
    pub fn bullet(mut self, text: impl Into<String>) -> Confirm {
        self.bullets.push(text.into());
        self
    }
    pub fn danger_bullet(mut self, text: impl Into<String>) -> Confirm {
        self.danger_bullets.push(text.into());
        self
    }
    pub fn typed_confirmation(mut self, needle: impl Into<String>) -> Confirm {
        self.type_to_confirm = Some(needle.into());
        self
    }
    pub fn toggle(mut self, label: impl Into<String>, helper: impl Into<String>) -> Confirm {
        self.extra_toggle = Some((label.into(), helper.into(), false));
        self
    }
    pub fn action(mut self, action: ConfirmAction) -> Confirm {
        self.action = action;
        self
    }
    pub fn safe(mut self) -> Confirm {
        self.destructive = false;
        self
    }
    pub fn can_confirm(&self) -> bool {
        match &self.type_to_confirm {
            Some(needle) => &self.typed == needle,
            None => true,
        }
    }
}

/// `V-1`. Blocking when the user reached it by attempting a locked action;
/// dismissible when they opened it themselves from the rail.
#[derive(Debug, Clone, Default)]
pub struct UnlockState {
    pub passphrase: String,
    pub revealed: bool,
    pub remember: bool,
    pub blocking: bool,
    pub busy: bool,
    pub attempts: u32,
    pub error: Option<String>,
}

impl UnlockState {
    pub fn blocking() -> UnlockState {
        UnlockState { blocking: true, ..Default::default() }
    }
    pub fn voluntary() -> UnlockState {
        UnlockState::default()
    }
    /// A wrong passphrase keeps the text so a single typo can be corrected.
    pub fn fail(&mut self) {
        self.busy = false;
        self.attempts += 1;
        self.error = Some(copy::vault::UNLOCK_WRONG.to_string());
    }
}

/// `T-5`. Blocking: no `x`, and Escape does nothing.
#[derive(Debug, Clone)]
pub struct WriteDownState {
    pub destination: Uuid,
    pub location: String,
    pub passphrase: String,
    pub acknowledged: bool,
    pub copied: bool,
}

impl WriteDownState {
    /// Grouped into eight-character chunks, four to a line, so it can be
    /// transcribed by hand without ambiguity.
    pub fn grouped(&self) -> String {
        let chars: Vec<char> = self.passphrase.chars().collect();
        let mut out = String::new();
        for (index, chunk) in chars.chunks(8).enumerate() {
            if index > 0 {
                out.push(if index % 4 == 0 { '\n' } else { ' ' });
                if index % 4 != 0 {
                    out.push(' ');
                }
            }
            out.extend(chunk);
        }
        out
    }
}

/// The connect flow for a repository that already exists at a location.
#[derive(Debug, Clone)]
pub struct ConnectState {
    pub destination: Uuid,
    pub location: String,
    pub derive_from_master: bool,
    pub offer_derive: bool,
    pub passphrase: String,
    pub revealed: bool,
    pub busy: bool,
    pub error: Option<String>,
}

/// `P-4`, three internal steps and no nesting.
#[derive(Debug, Clone)]
pub struct RotateState {
    pub provider: Uuid,
    pub step: u8,
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
    pub revealed: bool,
    pub verifying: bool,
    /// Per-destination verification outcome.
    pub results: Vec<(Uuid, Option<Result<(), String>>)>,
    pub old_key_id: String,
}

impl RotateState {
    pub fn new(provider: Uuid) -> RotateState {
        RotateState {
            provider,
            step: 1,
            access_key: String::new(),
            secret_key: String::new(),
            session_token: None,
            revealed: false,
            verifying: false,
            results: Vec::new(),
            old_key_id: String::new(),
        }
    }
    /// Any failure blocks step 3 unless at least one destination passed and the
    /// user accepts what will break.
    pub fn can_continue(&self) -> bool {
        !self.results.is_empty() && self.results.iter().all(|(_, r)| matches!(r, Some(Ok(()))))
    }
    pub fn any_passed(&self) -> bool {
        self.results.iter().any(|(_, r)| matches!(r, Some(Ok(()))))
    }
}

/// `R-4`, which converts in place into `R-5` rather than opening a second
/// modal.
#[derive(Debug, Clone)]
pub struct RestoreOptionsState {
    pub destination: Uuid,
    pub snapshot: String,
    pub items: Vec<String>,
    pub estimated_bytes: u64,
    pub to_original: bool,
    pub target: String,
    pub recreate_structure: bool,
    /// No default when restoring to the original location: the user must
    /// choose what happens to files that are already there.
    pub conflict: Option<ConflictPolicy>,
    pub timestamps: bool,
    pub permissions: bool,
    pub typed_overwrite: String,
    pub free_bytes: Option<u64>,
    pub running: bool,
}

impl RestoreOptionsState {
    pub fn new(destination: Uuid, snapshot: String, items: Vec<String>, bytes: u64) -> Self {
        RestoreOptionsState {
            destination,
            snapshot,
            items,
            estimated_bytes: bytes,
            to_original: false,
            target: default_restore_target(),
            recreate_structure: true,
            conflict: Some(ConflictPolicy::Skip),
            timestamps: true,
            // Not meaningfully portable on Windows, so it is off and explained
            // rather than offered and ignored.
            permissions: !cfg!(windows),
            typed_overwrite: String::new(),
            free_bytes: None,
            running: false,
        }
    }

    /// The one destructive path in the product that writes over live user
    /// data, and the only one that asks for a typed confirmation.
    pub fn needs_typed_confirmation(&self) -> bool {
        self.to_original && self.conflict == Some(ConflictPolicy::Overwrite)
    }

    pub fn can_restore(&self) -> bool {
        if self.items.is_empty() || self.conflict.is_none() {
            return false;
        }
        if !self.to_original && self.target.trim().is_empty() {
            return false;
        }
        if self.needs_typed_confirmation() && self.typed_overwrite != "overwrite" {
            return false;
        }
        if let Some(free) = self.free_bytes {
            if self.estimated_bytes > free {
                return false;
            }
        }
        true
    }
}

fn default_restore_target() -> String {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M");
    let base = dirs_download();
    base.join(format!("superbackup-restore-{stamp}")).to_string_lossy().into_owned()
}

fn dirs_download() -> PathBuf {
    if let Some(dirs) = directories_home() {
        let downloads = dirs.join("Downloads");
        if downloads.exists() {
            return downloads;
        }
        return dirs;
    }
    std::env::temp_dir()
}

fn directories_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

#[derive(Debug, Clone, Default)]
pub struct ChangePassphraseState {
    pub current: String,
    pub replacement: String,
    pub confirm: String,
    pub revealed: bool,
    pub busy: bool,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UnsavedState {
    pub what: String,
    pub tabs: String,
    pub then: Box<super::nav::Route>,
}

#[derive(Debug, Clone, Default)]
pub struct NewProjectState {
    pub name: String,
    pub description: String,
    pub colour: usize,
}

/// The ten fixed project hues, so a project's colour is never an arbitrary
/// picker value that fails contrast somewhere.
pub const PROJECT_COLOURS: [&str; 10] = [
    "#5B9BFF", "#4FBF6B", "#E0A83A", "#FF6B72", "#B07FFF", "#3ECFCF", "#F58A5B", "#8B93A5",
    "#7EE29A", "#FF9AC1",
];

#[derive(Debug, Clone)]
pub enum Modal {
    Unlock(UnlockState),
    Confirm(Confirm),
    WriteDown(WriteDownState),
    Connect(ConnectState),
    Wizard(Box<WizardState>),
    Rotate(RotateState),
    RestoreOptions(Box<RestoreOptionsState>),
    ChangePassphrase(ChangePassphraseState),
    Unsaved(UnsavedState),
    NewProject(NewProjectState),
    CronHelp,
    Export,
}

impl Modal {
    /// True while the modal is waiting on the daemon, so the app keeps asking
    /// for frames while a spinner is turning and stops when it is not.
    pub fn busy(&self) -> bool {
        match self {
            Modal::Unlock(s) => s.busy,
            Modal::Connect(s) => s.busy,
            Modal::Rotate(s) => s.verifying,
            Modal::ChangePassphrase(s) => s.busy,
            Modal::RestoreOptions(s) => s.running,
            _ => false,
        }
    }

    /// One of each, for the render smoke test.
    pub fn every(data: &Data) -> Vec<Modal> {
        let destination = data.destinations.first().map(|d| d.id).unwrap_or_else(Uuid::nil);
        let provider = data.providers.first().map(|p| p.id).unwrap_or_else(Uuid::nil);
        let job = data.jobs.first();
        vec![
            Modal::Unlock(UnlockState::blocking()),
            Modal::Unlock(UnlockState::voluntary()),
            Modal::Confirm(delete_job_confirm(job)),
            Modal::Confirm(remove_destination_confirm(data, destination)),
            Modal::Confirm(reset_vault_confirm(data)),
            Modal::WriteDown(WriteDownState {
                destination,
                location: "D:\\superbackup\\repository".into(),
                passphrase: "kX7fQ2mNbR4tYw8ZaP1sDv6HgJ3eLc0U".into(),
                acknowledged: false,
                copied: false,
            }),
            Modal::Connect(ConnectState {
                destination,
                location: "D:\\superbackup\\repository".into(),
                derive_from_master: true,
                offer_derive: true,
                passphrase: String::new(),
                revealed: false,
                busy: false,
                error: None,
            }),
            Modal::Wizard(Box::new(WizardState::new(data))),
            Modal::Rotate(RotateState::new(provider)),
            Modal::RestoreOptions(Box::new(RestoreOptionsState::new(
                destination,
                "k9f2ab7c31de".into(),
                vec!["/home/andreas/dev/web".into(), "/home/andreas/dev/api".into()],
                1_288_490_188,
            ))),
            Modal::ChangePassphrase(ChangePassphraseState::default()),
            Modal::Unsaved(UnsavedState {
                what: job.map(|j| j.name.clone()).unwrap_or_else(|| "this job".into()),
                tabs: copy::job::TAB_SOURCES.to_string(),
                then: Box::new(super::nav::Route::Jobs),
            }),
            Modal::NewProject(NewProjectState::default()),
            Modal::CronHelp,
            Modal::Export,
        ]
    }
}

// ---------------------------------------------------------------------------
// Ready-made confirmations
// ---------------------------------------------------------------------------

pub fn delete_job_confirm(job: Option<&Job>) -> Confirm {
    let name = job.map(|j| j.name.clone()).unwrap_or_else(|| "this job".into());
    Confirm::new(format!("Delete {name}?"), copy::job::DANGER_BODY, format!("Delete {name}"))
        .bullet("The job definition is removed from this machine.")
        .bullet("Snapshots already written to any destination are left exactly as they are.")
        .action(ConfirmAction::DeleteJob(job.map(|j| j.id).unwrap_or_else(Uuid::nil)))
}

pub fn remove_destination_confirm(data: &Data, destination: Uuid) -> Confirm {
    let name = data.destination_name(&destination);
    let using = data.jobs_using(&destination);
    let orphans = data.jobs_orphaned_by(&destination);
    let mut confirm = Confirm::new(
        copy::dest_delete_title(&name),
        copy::dest::DELETE_BODY,
        copy::dest::DELETE_BUTTON,
    )
    .action(ConfirmAction::RemoveDestination { id: destination, delete_files: false });

    if !using.is_empty() {
        let names = using.iter().map(|j| j.name.as_str()).collect::<Vec<_>>().join(", ");
        confirm = confirm.bullet(copy::dest_delete_jobs(using.len(), &names));
    }
    if !orphans.is_empty() {
        let names = orphans.iter().map(|j| j.name.as_str()).collect::<Vec<_>>().join(", ");
        confirm = confirm.danger_bullet(copy::dest_delete_orphans(&names));
    }
    match data.destination(&destination).map(|d| &d.kind) {
        Some(superbackup_core::model::DestinationKind::S3 { prefix, .. }) => {
            confirm = confirm.bullet(copy::dest::DELETE_S3_NOTE);
            let _ = prefix;
        }
        Some(kind) => {
            if let Some(path) = kind.local_path() {
                confirm = confirm.toggle(
                    copy::dest_delete_also_files(&path.to_string_lossy()),
                    copy::dest::DELETE_ALSO_FILES_WARN,
                );
            }
        }
        None => {}
    }
    confirm
}

pub fn delete_provider_confirm(data: &Data, provider: Uuid) -> Confirm {
    let name = data
        .provider(&provider)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| copy::state::UNKNOWN.into());
    let (inheriting, overriding) = data.destinations_using(&provider);
    let in_use = inheriting.len() + overriding.len();
    let mut confirm = Confirm::new(
        copy::prov_delete_title(&name),
        copy::prov::DELETE_BODY,
        format!("Delete {name}"),
    )
    .action(ConfirmAction::DeleteProvider(provider));
    if in_use > 0 {
        confirm = confirm.danger_bullet(copy::prov_delete_in_use(in_use));
        // Blocked rather than merely warned: the primary stays unreachable.
        confirm = confirm.typed_confirmation("\u{0}");
    }
    confirm
}

pub fn stop_run_confirm(run_id: Uuid, job: &str) -> Confirm {
    Confirm::new(copy::run_stop_title(job), copy::run::STOP_BODY, copy::run::STOP_BUTTON)
        .action(ConfirmAction::StopRun(run_id))
}

pub fn stop_all_confirm(names: &[String]) -> Confirm {
    Confirm::new(
        copy::run_stop_all_title(names.len()),
        copy::run_stop_all_body(&names.join(", ")),
        copy::run::STOP_ALL_BUTTON,
    )
    .action(ConfirmAction::StopAll)
}

pub fn reset_vault_confirm(data: &Data) -> Confirm {
    let names = data
        .destinations
        .iter()
        .filter(|d| d.kind.is_repository())
        .map(|d| d.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Confirm::new(copy::set::SEC_RESET_TITLE, copy::set::SEC_RESET_BODY, copy::set::SEC_RESET_BUTTON)
        .danger_bullet(copy::set_sec_reset_affected(&names))
        .bullet("Nothing at any destination is deleted.")
        .typed_confirmation("superbackup")
        .action(ConfirmAction::ResetVault)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Draw whichever modal is open. Returns `Some` to keep it open.
pub fn show(app: &mut App, ctx: &egui::Context, modal: Modal) -> Option<Modal> {
    match modal {
        Modal::Unlock(state) => show_unlock(app, ctx, state),
        Modal::Confirm(state) => show_confirm(app, ctx, state),
        Modal::WriteDown(state) => show_write_down(app, ctx, state),
        Modal::Connect(state) => show_connect(app, ctx, state),
        Modal::Wizard(state) => {
            super::screens::wizard::show(app, ctx, *state).map(Box::new).map(Modal::Wizard)
        }
        Modal::Rotate(state) => show_rotate(app, ctx, state),
        Modal::RestoreOptions(state) => super::screens::restore::show_options(app, ctx, *state)
            .map(Box::new)
            .map(Modal::RestoreOptions),
        Modal::ChangePassphrase(state) => show_change_passphrase(app, ctx, state),
        Modal::Unsaved(state) => show_unsaved(app, ctx, state),
        Modal::NewProject(state) => show_new_project(app, ctx, state),
        Modal::CronHelp => show_cron_help(ctx),
        Modal::Export => show_export(app, ctx),
    }
}

fn show_unlock(app: &mut App, ctx: &egui::Context, mut state: UnlockState) -> Option<Modal> {
    let t = theme::tokens(ctx);
    let mut submit = false;
    let mut cancel = false;
    let blocking = state.blocking;

    let (close, _) = widgets::modal(
        ctx,
        "sb-unlock",
        ModalSize::Small,
        copy::vault::UNLOCK_TITLE,
        Some((Icon::Lock, t.accent)),
        blocking,
        |m| {
            m.body(|ui| {
                widgets::paragraph(ui, copy::vault::UNLOCK_BODY, Type::Small, t.text_secondary);
                ui.add_space(space::XL);
                let response = widgets::passphrase_field(
                    ui,
                    &mut state.passphrase,
                    copy::vault::UNLOCK_FIELD,
                    &mut state.revealed,
                    state.error.as_deref(),
                    280.0,
                );
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if state.attempts == 0 {
                    response.request_focus();
                }
                // Only where the OS keychain is switched on; absent, not disabled.
                if app.data.settings.use_os_keychain {
                    ui.add_space(space::L);
                    widgets::checkbox(
                        ui,
                        &mut state.remember,
                        copy::vault::UNLOCK_REMEMBER,
                        None,
                        true,
                    );
                }
                // Reserved error area, so the modal does not jump.
                ui.add_space(space::M);
                let (rect, _) = ui.allocate_exact_size(
                    egui::Vec2::new(ui.available_width(), 20.0),
                    egui::Sense::hover(),
                );
                if state.attempts >= 3 {
                    let g = widgets::galley_wrapped(
                        ui,
                        copy::vault::UNLOCK_NO_RECOVERY,
                        Type::Small,
                        t.text_muted,
                        rect.width(),
                    );
                    ui.painter().galley(rect.min, g, t.text_muted);
                }
            });
            m.footer(|ui| {
                let label =
                    if state.busy { copy::vault::UNLOCK_BUSY } else { copy::vault::UNLOCK_BUTTON };
                if Button::primary(label)
                    .busy(state.busy)
                    .enabled(!state.busy && !state.passphrase.is_empty())
                    .show(ui)
                    .clicked()
                {
                    submit = true;
                }
                if !blocking && Button::ghost(copy::action::CANCEL).show(ui).clicked() {
                    cancel = true;
                }
            });
        },
    );

    if submit && !state.passphrase.is_empty() {
        state.busy = true;
        state.error = None;
        app.unlock(state.passphrase.clone());
    }
    if cancel || (close && !blocking) {
        // A voluntary unlock that is dismissed also drops the pending intent:
        // the user changed their mind.
        app.pending = None;
        return None;
    }
    Some(Modal::Unlock(state))
}

fn show_confirm(app: &mut App, ctx: &egui::Context, mut state: Confirm) -> Option<Modal> {
    let t = theme::tokens(ctx);
    let mut confirmed = false;
    let mut cancelled = false;

    let (close, _) = widgets::modal(
        ctx,
        "sb-confirm",
        ModalSize::Small,
        &state.title.clone(),
        state.destructive.then_some((Icon::AlertTriangle, t.danger.mark)),
        false,
        |m| {
            m.body(|ui| {
                widgets::paragraph(ui, state.body.clone(), Type::Body, t.text_secondary);
                if !state.bullets.is_empty() || !state.danger_bullets.is_empty() {
                    ui.add_space(space::L);
                }
                for bullet in &state.danger_bullets {
                    bullet_row(ui, bullet, t.danger.tint_text);
                }
                for bullet in &state.bullets {
                    bullet_row(ui, bullet, t.text_secondary);
                }
                if let Some((label, helper, on)) = &mut state.extra_toggle {
                    ui.add_space(space::XL);
                    let label = label.clone();
                    let helper = helper.clone();
                    widgets::checkbox(ui, on, &label, Some(&helper), true);
                    if *on && state.type_to_confirm.is_none() {
                        // Ticking the escalation promotes the modal to a typed
                        // confirmation, and changes the verb with it.
                        state.type_to_confirm = Some(state.title.clone());
                    }
                }
                if let Some(needle) = &state.type_to_confirm {
                    if needle != "\u{0}" {
                        ui.add_space(space::XL);
                        let label = copy::confirm_type_name(needle);
                        widgets::Field::new().label(&label).width(240.0).show(ui, &mut state.typed);
                    }
                }
            });
            m.footer(|ui| {
                let blocked = state.type_to_confirm.as_deref() == Some("\u{0}");
                let verb = if state.extra_toggle.as_ref().map(|(_, _, on)| *on).unwrap_or(false) {
                    copy::dest::DELETE_BUTTON_FILES.to_string()
                } else {
                    state.verb.clone()
                };
                let button =
                    if state.destructive { Button::danger(&verb) } else { Button::primary(&verb) };
                let enabled = state.can_confirm() && !blocked;
                let button = if blocked {
                    button.disabled_because(copy::prov::DELETE_GOTO)
                } else {
                    button.enabled(enabled)
                };
                if button.show(ui).clicked() {
                    confirmed = true;
                }
                if Button::ghost(copy::action::CANCEL).show(ui).clicked() {
                    cancelled = true;
                }
            });
        },
    );

    if confirmed {
        perform(app, &state);
        return None;
    }
    if cancelled || close {
        return None;
    }
    Some(Modal::Confirm(state))
}

fn bullet_row(ui: &mut egui::Ui, text: &str, colour: egui::Color32) {
    ui.horizontal_top(|ui| {
        ui.add_space(space::XS);
        let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(10.0, 20.0), egui::Sense::hover());
        ui.painter().circle_filled(
            egui::Pos2::new(rect.left() + 3.0, rect.top() + 9.0),
            2.5,
            colour,
        );
        widgets::paragraph_at(
            ui,
            text,
            Type::Small,
            colour,
            (ui.available_width() - 4.0).max(120.0),
        );
    });
    ui.add_space(space::XS);
}

fn perform(app: &mut App, confirm: &Confirm) {
    match &confirm.action {
        ConfirmAction::DeleteJob(id) => {
            let name = app.data.job_name(id);
            app.ask(Intent::DeleteJob(name), Request::JobDelete { job: id.to_string() });
            app.go(super::nav::Route::Jobs);
        }
        ConfirmAction::DisableAllJobs => {
            let jobs: Vec<Job> = app.data.jobs.iter().filter(|j| j.enabled).cloned().collect();
            for job in jobs {
                app.ask(
                    Intent::SaveJob(job.name.clone()),
                    Request::JobSetEnabled { job: job.id.to_string(), enabled: false },
                );
            }
            app.toasts.info(copy::toast::JOBS_DISABLED_ALL);
        }
        ConfirmAction::RemoveDestination { id, .. } => {
            let name = app.data.destination_name(id);
            app.ask(
                Intent::DeleteDestination(name),
                Request::DestinationDelete { destination: id.to_string(), force: true },
            );
            app.go(super::nav::Route::Destinations);
        }
        ConfirmAction::DeleteProvider(id) => {
            let name =
                app.data.provider(id).map(|p| p.name.clone()).unwrap_or_else(|| id.to_string());
            app.ask(
                Intent::DeleteProvider(name),
                Request::ProviderDelete { provider: id.to_string(), force: false },
            );
            app.go(super::nav::Route::Providers);
        }
        ConfirmAction::StopRun(run_id) => app.request_stop(*run_id),
        ConfirmAction::StopAll => app.request_stop_all(),
        ConfirmAction::ResetSettings => {
            app.data.settings = superbackup_core::model::Settings::default();
            app.save_settings();
            app.toasts.info(copy::toast::SETTINGS_SAVED);
        }
        // The daemon owns the vault; the interface asks and reports.
        ConfirmAction::ResetVault | ConfirmAction::RemoveAllConfiguration => {
            app.toasts.warning(
                "This build cannot reset the vault from the window yet. Use the command line.",
            );
        }
        ConfirmAction::Nothing => {}
    }
}

fn show_write_down(app: &mut App, ctx: &egui::Context, mut state: WriteDownState) -> Option<Modal> {
    let t = theme::tokens(ctx);
    let mut done = false;
    widgets::modal(
        ctx,
        "sb-writedown",
        ModalSize::Large,
        copy::writedown::TITLE,
        Some((Icon::KeyRound, t.warning.mark)),
        true,
        |m| {
            m.body(|ui| {
                widgets::paragraph(
                    ui,
                    copy::writedown_body(&state.location),
                    Type::Body,
                    t.text_secondary,
                );
                ui.add_space(space::XXL);

                // The passphrase itself: focusable, so a screen reader can be
                // pointed at it deliberately, and never announced automatically.
                let grouped = state.grouped();
                let response = egui::Frame::new()
                    .fill(t.bg_code)
                    .stroke(egui::Stroke::new(1.0_f32, t.border_subtle))
                    .corner_radius(super::theme::radius::CONTROL)
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for line in grouped.lines() {
                            widgets::text(ui, line, Type::MonoStrong, t.text_primary);
                        }
                    })
                    .response;
                let interact = ui.interact(
                    response.rect,
                    egui::Id::new("sb-writedown-passphrase"),
                    egui::Sense::focusable_noninteractive(),
                );
                interact.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Label,
                        true,
                        copy::a11y::PASSPHRASE_BLOCK,
                    )
                });
                ui.add_space(space::M);
                widgets::text(ui, copy::writedown::GROUPING, Type::Small, t.text_muted);

                ui.add_space(space::XXL);
                ui.horizontal(|ui| {
                    if Button::secondary(copy::writedown::COPY).icon(Icon::Copy).show(ui).clicked()
                    {
                        ui.ctx().copy_text(state.passphrase.clone());
                        state.copied = true;
                    }
                    if Button::secondary(copy::writedown::SAVE)
                        .icon(Icon::FileText)
                        .show(ui)
                        .clicked()
                    {
                        app.toasts.info(copy::writedown::SAVE_NOTE);
                    }
                    if cfg!(any(windows, target_os = "macos"))
                        && Button::ghost(copy::writedown::PRINT)
                            .icon(Icon::Printer)
                            .show(ui)
                            .clicked()
                    {
                        app.toasts.info(copy::writedown::SAVE_NOTE);
                    }
                });
                if state.copied {
                    ui.add_space(space::M);
                    widgets::text(ui, copy::writedown::COPIED, Type::Small, t.text_muted);
                }
                ui.add_space(space::M);
                widgets::text(ui, copy::writedown::SAVE_NOTE, Type::Small, t.text_muted);

                ui.add_space(space::XXL);
                widgets::checkbox(ui, &mut state.acknowledged, copy::writedown::ACK, None, true);
                ui.add_space(space::L);
                widgets::paragraph(ui, copy::writedown::ESCAPE, Type::Small, t.text_muted);
            });
            m.footer(|ui| {
                if Button::primary(copy::action::DONE)
                    .enabled(state.acknowledged)
                    .show(ui)
                    .clicked()
                {
                    done = true;
                }
            });
        },
    );
    if done {
        return None;
    }
    Some(Modal::WriteDown(state))
}

fn show_connect(app: &mut App, ctx: &egui::Context, mut state: ConnectState) -> Option<Modal> {
    let t = theme::tokens(ctx);
    let mut connect = false;
    let (close, _) = widgets::modal(
        ctx,
        "sb-connect",
        ModalSize::Medium,
        copy::dest::CONNECT_TITLE,
        Some((Icon::HardDrive, t.accent)),
        false,
        |m| {
            m.body(|ui| {
                widgets::paragraph(ui, copy::dest::CONNECT_BODY, Type::Body, t.text_secondary);
                ui.add_space(space::XL);
                widgets::kv(ui, copy::dest::FOLDER, &state.location, true);
                ui.add_space(space::XL);
                if state.offer_derive {
                    if widgets::radio(
                        ui,
                        state.derive_from_master,
                        copy::dest::CONNECT_DERIVE,
                        None,
                        true,
                    )
                    .clicked()
                    {
                        state.derive_from_master = true;
                    }
                    if widgets::radio(
                        ui,
                        !state.derive_from_master,
                        copy::dest::CONNECT_TYPE,
                        None,
                        true,
                    )
                    .clicked()
                    {
                        state.derive_from_master = false;
                    }
                    ui.add_space(space::L);
                }
                if !state.derive_from_master {
                    widgets::passphrase_field(
                        ui,
                        &mut state.passphrase,
                        copy::dest::CONNECT_FIELD,
                        &mut state.revealed,
                        state.error.as_deref(),
                        400.0,
                    );
                }
                ui.add_space(space::L);
                widgets::text(ui, copy::dest::CONNECT_SETTINGS_NOTE, Type::Small, t.text_muted);
            });
            m.footer(|ui| {
                if Button::primary(copy::dest::FOLDER_FOUND_REPO_ACTION)
                    .busy(state.busy)
                    .enabled(
                        !state.busy && (state.derive_from_master || !state.passphrase.is_empty()),
                    )
                    .show(ui)
                    .clicked()
                {
                    connect = true;
                }
                if Button::ghost(copy::action::CANCEL).show(ui).clicked() {
                    connect = false;
                }
            });
        },
    );
    if connect {
        state.busy = true;
        app.ask(
            Intent::CreateRepository(state.destination),
            Request::DestinationRepoConnect { destination: state.destination.to_string() },
        );
        return None;
    }
    if close {
        return None;
    }
    Some(Modal::Connect(state))
}

fn show_rotate(app: &mut App, ctx: &egui::Context, mut state: RotateState) -> Option<Modal> {
    let t = theme::tokens(ctx);
    let name = app
        .data
        .provider(&state.provider)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| copy::state::UNKNOWN.into());
    let (inheriting, overriding) = app.data.destinations_using(&state.provider);
    let affected: Vec<(Uuid, String)> = inheriting.iter().map(|d| (d.id, d.name.clone())).collect();
    let unaffected: Vec<String> = overriding.iter().map(|d| d.name.clone()).collect();
    let jobs = app.data.jobs_via_provider(&state.provider).len();

    let mut advance = false;
    let mut verify = false;
    let mut finish = false;

    let (close, _) = widgets::modal(
        ctx,
        "sb-rotate",
        ModalSize::Medium,
        &copy::prov_rotate_title(&name),
        Some((Icon::KeyRound, t.warning.mark)),
        false,
        |m| {
            m.body(|ui| match state.step {
                1 => {
                    widgets::banner(
                        ui,
                        widgets::BannerKind::Warning,
                        copy::prov::ROTATE_LEAD,
                        Some(copy::prov::ROTATE_OLD_VALID),
                        |_| {},
                    );
                    ui.add_space(space::XL);
                    widgets::text(
                        ui,
                        copy::prov_impact(affected.len(), jobs),
                        Type::BodyStrong,
                        t.text_primary,
                    );
                    ui.add_space(space::M);
                    for (_, name) in &affected {
                        bullet_row(ui, name, t.text_secondary);
                    }
                    if !unaffected.is_empty() {
                        ui.add_space(space::XL);
                        widgets::text(
                            ui,
                            copy::prov::IMPACT_UNAFFECTED,
                            Type::BodyStrong,
                            t.text_primary,
                        );
                        ui.add_space(space::M);
                        for name in &unaffected {
                            bullet_row(ui, name, t.text_muted);
                        }
                    }
                }
                2 => {
                    widgets::text(ui, copy::prov::ROTATE_NEW_CREDS, Type::H3, t.text_primary);
                    ui.add_space(space::L);
                    widgets::Field::new()
                        .label(copy::prov::ACCESS_KEY)
                        .width(400.0)
                        .show(ui, &mut state.access_key);
                    ui.add_space(space::L);
                    let mut revealed = state.revealed;
                    widgets::passphrase_field(
                        ui,
                        &mut state.secret_key,
                        copy::prov::SECRET_KEY,
                        &mut revealed,
                        None,
                        400.0,
                    );
                    state.revealed = revealed;
                    ui.add_space(space::XL);
                    for (id, result) in &state.results {
                        let name = app.data.destination_name(id);
                        match result {
                            None => widgets::checklist_row(
                                ui,
                                widgets::StepState::Running,
                                &copy::prov_rotate_verifying(&name),
                                None,
                            ),
                            Some(Ok(())) => widgets::checklist_row(
                                ui,
                                widgets::StepState::Done,
                                &name,
                                Some(copy::prov::ROTATE_PASS),
                            ),
                            Some(Err(reason)) => widgets::checklist_row(
                                ui,
                                widgets::StepState::Failed,
                                &name,
                                Some(&copy::prov_rotate_fail(reason)),
                            ),
                        }
                    }
                    if !state.results.is_empty() && !state.can_continue() {
                        ui.add_space(space::L);
                        widgets::paragraph(
                            ui,
                            copy::prov::ROTATE_BLOCKED,
                            Type::Small,
                            t.warning.tint_text,
                        );
                    }
                }
                _ => {
                    widgets::text(ui, copy::prov::ROTATE_DONE_TITLE, Type::H2, t.text_primary);
                    ui.add_space(space::M);
                    widgets::paragraph(
                        ui,
                        copy::prov::ROTATE_DONE_BODY,
                        Type::Body,
                        t.text_secondary,
                    );
                    ui.add_space(space::XL);
                    widgets::paragraph(
                        ui,
                        copy::prov_rotate_done_revoke(&state.old_key_id),
                        Type::Small,
                        t.text_secondary,
                    );
                    ui.add_space(space::L);
                    if Button::secondary("Copy key ID").icon(Icon::Copy).show(ui).clicked() {
                        ui.ctx().copy_text(state.old_key_id.clone());
                    }
                }
            });
            m.footer(|ui| match state.step {
                1 => {
                    if Button::primary(copy::action::CONTINUE).show(ui).clicked() {
                        advance = true;
                    }
                    if Button::ghost(copy::action::CANCEL).show(ui).clicked() {
                        advance = false;
                    }
                }
                2 => {
                    let ready = !state.access_key.trim().is_empty() && !state.secret_key.is_empty();
                    if Button::primary(copy::prov::ROTATE_VERIFY)
                        .busy(state.verifying)
                        .enabled(ready && !state.verifying)
                        .show(ui)
                        .clicked()
                    {
                        verify = true;
                    }
                    if state.any_passed() && !state.can_continue() {
                        if Button::danger(copy::prov::ROTATE_CONTINUE_ANYWAY).show(ui).clicked() {
                            finish = true;
                        }
                    } else if state.can_continue()
                        && Button::primary(copy::action::CONTINUE).show(ui).clicked()
                    {
                        finish = true;
                    }
                }
                _ => {
                    if Button::primary(copy::action::DONE).show(ui).clicked() {
                        advance = true;
                    }
                }
            });
        },
    );

    if advance {
        match state.step {
            1 => state.step = 2,
            _ => return None,
        }
    }
    if verify {
        state.verifying = true;
        state.results = affected.iter().map(|(id, _)| (*id, None)).collect();
        for (id, _) in &affected {
            app.ask(
                Intent::TestDestination(*id),
                Request::DestinationTest { destination: id.to_string() },
            );
        }
    }
    if finish {
        app.ask(
            Intent::SaveProvider(name),
            Request::ProviderRotateCredentials {
                provider: state.provider.to_string(),
                access_key_id: superbackup_core::ipc::protocol::SecretString::from_string(
                    state.access_key.clone(),
                ),
                secret_access_key: superbackup_core::ipc::protocol::SecretString::from_string(
                    state.secret_key.clone(),
                ),
                session_token: None,
            },
        );
        state.step = 3;
        state.verifying = false;
    }
    if close {
        return None;
    }
    Some(Modal::Rotate(state))
}

fn show_change_passphrase(
    app: &mut App,
    ctx: &egui::Context,
    mut state: ChangePassphraseState,
) -> Option<Modal> {
    let t = theme::tokens(ctx);
    let mut submit = false;
    let report = validation::master_passphrase(&state.replacement, &state.confirm);
    let score = validation::passphrase_score(&state.replacement);

    let (close, _) = widgets::modal(
        ctx,
        "sb-change-passphrase",
        ModalSize::Medium,
        copy::set::SEC_CHANGE,
        Some((Icon::KeyRound, t.accent)),
        false,
        |m| {
            m.body(|ui| {
                if state.done {
                    widgets::text(ui, copy::set::SEC_CHANGE_DONE_TITLE, Type::H2, t.text_primary);
                    ui.add_space(space::M);
                    widgets::paragraph(
                        ui,
                        copy::set::SEC_CHANGE_DONE_BODY,
                        Type::Body,
                        t.text_secondary,
                    );
                    return;
                }
                let mut revealed = state.revealed;
                widgets::passphrase_field(
                    ui,
                    &mut state.current,
                    copy::set::SEC_CHANGE_CURRENT,
                    &mut revealed,
                    state.error.as_deref(),
                    400.0,
                );
                ui.add_space(space::XL);
                widgets::passphrase_field(
                    ui,
                    &mut state.replacement,
                    copy::set::SEC_CHANGE_NEW,
                    &mut revealed,
                    report.for_field(validation::Field::Passphrase),
                    400.0,
                );
                ui.add_space(space::M);
                widgets::strength_meter(ui, score, 400.0);
                ui.add_space(space::XL);
                widgets::passphrase_field(
                    ui,
                    &mut state.confirm,
                    copy::set::SEC_CHANGE_CONFIRM,
                    &mut revealed,
                    report.for_field(validation::Field::PassphraseConfirm),
                    400.0,
                );
                state.revealed = revealed;
            });
            m.footer(|ui| {
                if state.done {
                    if Button::primary(copy::action::DONE).show(ui).clicked() {
                        submit = true;
                    }
                    return;
                }
                if Button::primary(copy::action::SAVE)
                    .busy(state.busy)
                    .enabled(report.ok() && !state.current.is_empty() && !state.busy)
                    .show(ui)
                    .clicked()
                {
                    submit = true;
                }
                if Button::ghost(copy::action::CANCEL).show(ui).clicked() {
                    submit = false;
                }
            });
        },
    );

    if submit {
        if state.done {
            return None;
        }
        state.busy = true;
        app.ask(
            Intent::Fire,
            Request::VaultChangePassphrase {
                current: superbackup_core::ipc::protocol::SecretString::from_string(
                    state.current.clone(),
                ),
                replacement: superbackup_core::ipc::protocol::SecretString::from_string(
                    state.replacement.clone(),
                ),
            },
        );
        state.done = true;
        state.busy = false;
    }
    if close {
        return None;
    }
    Some(Modal::ChangePassphrase(state))
}

fn show_unsaved(app: &mut App, ctx: &egui::Context, state: UnsavedState) -> Option<Modal> {
    let t = theme::tokens(ctx);
    let mut outcome: Option<bool> = None;
    let mut cancel = false;
    let (close, _) = widgets::modal(
        ctx,
        "sb-unsaved",
        ModalSize::Small,
        &copy::job_unsaved_title(&state.what),
        Some((Icon::AlertTriangle, t.warning.mark)),
        false,
        |m| {
            m.body(|ui| {
                widgets::paragraph(
                    ui,
                    copy::job_unsaved_body(&state.tabs),
                    Type::Body,
                    t.text_secondary,
                );
            });
            m.footer(|ui| {
                if Button::primary(copy::action::SAVE).show(ui).clicked() {
                    outcome = Some(true);
                }
                if Button::secondary(copy::job::UNSAVED_DISCARD).show(ui).clicked() {
                    outcome = Some(false);
                }
                if Button::ghost(copy::action::CANCEL).show(ui).clicked() {
                    cancel = true;
                }
            });
        },
    );
    match outcome {
        Some(true) => {
            app.save_open_job();
            app.go((*state.then).clone());
            None
        }
        Some(false) => {
            app.discard_open_job();
            app.go((*state.then).clone());
            None
        }
        None => {
            if cancel || close {
                None
            } else {
                Some(Modal::Unsaved(state))
            }
        }
    }
}

fn show_new_project(
    app: &mut App,
    ctx: &egui::Context,
    mut state: NewProjectState,
) -> Option<Modal> {
    let t = theme::tokens(ctx);
    let mut create = false;
    let (close, _) = widgets::modal(
        ctx,
        "sb-new-project",
        ModalSize::Small,
        copy::job::PROJECT_NEW,
        None,
        false,
        |m| {
            m.body(|ui| {
                widgets::Field::new()
                    .label(copy::job::NAME)
                    .width(360.0)
                    .char_limit(64)
                    .show(ui, &mut state.name);
                ui.add_space(space::L);
                widgets::Field::new()
                    .label(copy::job::DESCRIPTION)
                    .width(360.0)
                    .rows(2)
                    .show(ui, &mut state.description);
                ui.add_space(space::XL);
                widgets::text(ui, "Colour", Type::H3, t.text_primary);
                ui.add_space(space::M);
                ui.horizontal(|ui| {
                    for (index, hex) in PROJECT_COLOURS.iter().enumerate() {
                        let colour = parse_hex(hex).unwrap_or(t.accent);
                        let (rect, response) =
                            ui.allocate_exact_size(egui::Vec2::splat(24.0), egui::Sense::click());
                        ui.painter().rect_filled(rect, super::theme::radius::BADGE, colour);
                        if state.colour == index {
                            ui.painter().rect_stroke(
                                rect.expand(2.0),
                                super::theme::radius::BADGE,
                                egui::Stroke::new(2.0_f32, t.text_primary),
                                egui::StrokeKind::Outside,
                            );
                        }
                        if response.clicked() {
                            state.colour = index;
                        }
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::RadioButton,
                                true,
                                state.colour == index,
                                format!("Colour {}", index + 1),
                            )
                        });
                    }
                });
            });
            m.footer(|ui| {
                if Button::primary(copy::action::ADD)
                    .enabled(!state.name.trim().is_empty())
                    .show(ui)
                    .clicked()
                {
                    create = true;
                }
                if Button::ghost(copy::action::CANCEL).show(ui).clicked() {
                    create = false;
                }
            });
        },
    );
    if create {
        // The IPC surface has no `project.create`; the interface says so
        // rather than pretending the project was stored.
        app.toasts.warning(
            "Projects cannot be created from the window yet: the daemon exposes no command for them.",
        );
        return None;
    }
    if close {
        return None;
    }
    Some(Modal::NewProject(state))
}

pub fn parse_hex(hex: &str) -> Option<egui::Color32> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(egui::Color32::from_rgb(r, g, b))
}

fn show_cron_help(ctx: &egui::Context) -> Option<Modal> {
    let t = theme::tokens(ctx);
    let (close, _) = widgets::modal(
        ctx,
        "sb-cron-help",
        ModalSize::Medium,
        copy::job::SCHEDULE_CRON_HELP,
        Some((Icon::Clock, t.accent)),
        false,
        |m| {
            m.body(|ui| {
            widgets::paragraph(
                ui,
                "Five fields, separated by spaces: minute, hour, day of the month, month, and day of the week. A star means every value.",
                Type::Body,
                t.text_secondary,
            );
            ui.add_space(space::XL);
            for (expression, meaning) in [
                ("0 2 * * *", "Every day at 02:00"),
                ("*/15 * * * *", "Every fifteen minutes"),
                ("0 3 * * 1-5", "At 03:00 from Monday to Friday"),
                ("30 1 1 * *", "At 01:30 on the first of each month"),
            ] {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(140.0, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            widgets::text(ui, expression, Type::Mono, t.text_primary);
                        },
                    );
                    widgets::text(ui, meaning, Type::Small, t.text_secondary);
                });
                ui.add_space(space::S);
            }
        });
            m.footer(|ui| {
                let _ = Button::primary(copy::action::CLOSE).show(ui);
            });
        },
    );
    if close {
        None
    } else {
        Some(Modal::CronHelp)
    }
}

fn show_export(app: &mut App, ctx: &egui::Context) -> Option<Modal> {
    let t = theme::tokens(ctx);
    let mut chosen: Option<&'static str> = None;
    let (close, _) = widgets::modal(
        ctx,
        "sb-export",
        ModalSize::Small,
        copy::activity::EXPORT,
        Some((Icon::Download, t.accent)),
        false,
        |m| {
            m.body(|ui| {
                for (label, kind) in [
                    (copy::activity::EXPORT_RUNS, "runs"),
                    (copy::activity::EXPORT_EVENTS, "events"),
                    (copy::activity::EXPORT_BUNDLE, "bundle"),
                ] {
                    if Button::secondary(label).min_width(320.0).show(ui).clicked() {
                        chosen = Some(kind);
                    }
                    ui.add_space(space::M);
                }
                ui.add_space(space::M);
                widgets::paragraph(ui, copy::activity::EXPORT_NOTE, Type::Small, t.text_muted);
            });
            m.footer(|ui| {
                let _ = Button::ghost(copy::action::CANCEL).show(ui);
            });
        },
    );
    if let Some(kind) = chosen {
        if kind == "bundle" {
            app.go(super::nav::Route::Settings(super::nav::SettingsSection::Advanced));
        } else {
            app.toasts.info(copy::activity::EXPORT_NOTE);
        }
        return None;
    }
    if close {
        None
    } else {
        Some(Modal::Export)
    }
}

/// The probe result a destination editor and the rotation modal both consume.
pub fn probe_message(probe: &ProbeReply) -> String {
    if probe.reachable && probe.writable {
        copy::dest::VERIFY_OK.to_string()
    } else {
        probe.detail.clone().unwrap_or_else(|| copy::dest::STATUS_UNREACHABLE.to_string())
    }
}

/// The passphrase-source line shown on a destination that already has a
/// repository. It cannot be shown again, and the interface says so.
pub fn passphrase_source_line(destination: &Destination) -> &'static str {
    match destination.encryption.as_ref().map(|e| e.passphrase_source) {
        Some(PassphraseSource::Generated) => copy::writedown::PASS_STORED,
        Some(PassphraseSource::DerivedFromMaster) => copy::writedown::PASS_DERIVED,
        Some(PassphraseSource::UserSupplied) => copy::writedown::PASS_SUPPLIED,
        None => copy::state::NONE,
    }
}

/// A short, human list of names for a confirmation body.
pub fn name_list(names: &[String]) -> String {
    match names.len() {
        0 => copy::state::NONE.to_string(),
        1 => names[0].clone(),
        _ => {
            let head = names[..names.len() - 1].join(", ");
            format!("{head} and {}", names[names.len() - 1])
        }
    }
}

/// `12 Mar 02:00` for a modal title that needs a timestamp.
pub fn stamp(at: chrono::DateTime<chrono::Utc>) -> String {
    format::absolute(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::fixtures;

    fn data() -> Data {
        let mut data = Data::new();
        fixtures::seed(&mut data);
        data
    }

    #[test]
    fn a_typed_confirmation_blocks_the_primary_until_it_matches() {
        let mut confirm = reset_vault_confirm(&data());
        assert!(!confirm.can_confirm());
        confirm.typed = "SUPERBACKUP".into();
        assert!(!confirm.can_confirm(), "the match is case sensitive by design");
        confirm.typed = "superbackup".into();
        assert!(confirm.can_confirm());
    }

    #[test]
    fn removing_a_destination_names_the_jobs_it_would_orphan() {
        let data = data();
        let confirm = remove_destination_confirm(&data, fixtures::DEST_S3);
        assert!(confirm.bullets.iter().any(|b| b.contains("Dev code")));
        // An S3 destination never offers bulk object deletion from this window.
        assert!(confirm.extra_toggle.is_none());
        assert!(confirm.bullets.iter().any(|b| b.contains("Objects in a bucket")));
    }

    #[test]
    fn removing_a_local_destination_offers_the_files_escalation() {
        let data = data();
        let confirm = remove_destination_confirm(&data, fixtures::DEST_LOCAL);
        assert!(confirm.extra_toggle.is_some());
    }

    #[test]
    fn a_provider_in_use_cannot_be_deleted() {
        let data = data();
        let confirm = delete_provider_confirm(&data, fixtures::PROVIDER_STORJ);
        assert!(!confirm.can_confirm(), "deletion is blocked while destinations use it");
        let confirm = delete_provider_confirm(&data, fixtures::PROVIDER_B2);
        assert!(confirm.can_confirm(), "an unused provider deletes normally");
    }

    #[test]
    fn restoring_over_the_original_location_demands_a_typed_word() {
        let mut state = RestoreOptionsState::new(Uuid::nil(), "snap".into(), vec!["a".into()], 10);
        state.to_original = true;
        state.conflict = Some(ConflictPolicy::Overwrite);
        assert!(state.needs_typed_confirmation());
        assert!(!state.can_restore());
        state.typed_overwrite = "overwrite".into();
        assert!(state.can_restore());
    }

    #[test]
    fn restoring_elsewhere_needs_no_typed_word_but_needs_a_folder() {
        let mut state = RestoreOptionsState::new(Uuid::nil(), "snap".into(), vec!["a".into()], 10);
        assert!(!state.needs_typed_confirmation());
        assert!(state.can_restore());
        state.target.clear();
        assert!(!state.can_restore());
    }

    #[test]
    fn a_restore_that_does_not_fit_is_refused() {
        let mut state =
            RestoreOptionsState::new(Uuid::nil(), "snap".into(), vec!["a".into()], 2_000);
        state.free_bytes = Some(1_000);
        assert!(!state.can_restore());
        state.free_bytes = Some(4_000);
        assert!(state.can_restore());
    }

    #[test]
    fn the_generated_passphrase_is_grouped_for_transcription() {
        let state = WriteDownState {
            destination: Uuid::nil(),
            location: "somewhere".into(),
            passphrase: "abcdefghijklmnopqrstuvwxyz012345".into(),
            acknowledged: false,
            copied: false,
        };
        let grouped = state.grouped();
        assert!(grouped.contains("abcdefgh  ijklmnop"));
        // Grouping must not change the value itself.
        assert_eq!(
            grouped.chars().filter(|c| !c.is_whitespace()).collect::<String>(),
            state.passphrase
        );
    }

    #[test]
    fn rotation_blocks_step_three_until_every_destination_passes() {
        let mut state = RotateState::new(Uuid::nil());
        assert!(!state.can_continue());
        state.results =
            vec![(Uuid::nil(), Some(Ok(()))), (Uuid::new_v4(), Some(Err("403".into())))];
        assert!(!state.can_continue());
        assert!(state.any_passed());
        state.results = vec![(Uuid::nil(), Some(Ok(())))];
        assert!(state.can_continue());
    }

    #[test]
    fn every_modal_can_be_constructed_from_the_fixture_machine() {
        let data = data();
        assert!(Modal::every(&data).len() >= 14);
    }
}
