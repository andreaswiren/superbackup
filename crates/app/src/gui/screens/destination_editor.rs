//! `T-2` and `T-4`. One editor over four kinds, plus repository creation and
//! the full encryption panel.

use chrono::Utc;
use egui::{Align, Layout, Ui};
use uuid::Uuid;

use superbackup_core::ipc::protocol::{ErrorPayload, Request, RepositoryReply};
use superbackup_core::model::{
    default_s3_prefix, normalise_prefix, Destination, DestinationKind, EccAlgorithm,
    EncryptionAlgorithm, EncryptionSettings, HashAlgorithm, PassphraseSource, RetentionPolicy,
    Splitter,
};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::daemon::Intent;
use crate::gui::data::{Action, Gate};
use crate::gui::icons::Icon;
use crate::gui::modals::{self, Modal};
use crate::gui::nav::Route;
use crate::gui::screens::destinations::{index_for_kind, kind_for_index, KIND_CHOICES};
use crate::gui::theme::{self, radius, space, Type};
use crate::gui::validation::{self, Field};
use crate::gui::widgets::{self, Button, StepState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreationPhase {
    Running(usize),
    Done,
    Failed,
}

#[derive(Default)]
pub struct State {
    pub draft: Option<Destination>,
    pub original: Option<Destination>,
    pub encryption_open: bool,
    pub creation: Option<CreationPhase>,
    pub creation_error: Option<String>,
    pub path_input: String,
    pub bucket_input: String,
    pub prefix_input: String,
    pub access_key: String,
    pub secret_key: String,
    pub revealed: bool,
    pub own_credentials: bool,
    /// Validation is inline and on blur: a form nobody has filled in yet is not
    /// a form full of mistakes. Set once the user tries to save.
    pub show_errors: bool,
    pub pin_offline: bool,
    pub mirror_prune: bool,
}

impl State {
    pub fn repository_started(&mut self) {
        self.creation = Some(CreationPhase::Running(0));
        self.creation_error = None;
    }
    pub fn repository_done(&mut self, _reply: RepositoryReply) {
        self.creation = Some(CreationPhase::Done);
    }
    pub fn repository_failed(&mut self, payload: ErrorPayload) {
        self.creation = Some(CreationPhase::Failed);
        self.creation_error = Some(payload.message);
    }
    pub fn busy(&self) -> bool {
        matches!(self.creation, Some(CreationPhase::Running(_)))
    }

    fn load(&mut self, destination: Option<&Destination>, machine_slug: &str) {
        let same = match (&self.original, destination) {
            (Some(o), Some(d)) => o.id == d.id,
            (None, None) => self.draft.is_some(),
            _ => false,
        };
        if same {
            return;
        }
        match destination {
            Some(d) => {
                self.draft = Some(d.clone());
                self.original = Some(d.clone());
                // Stored values are real, so a problem with them is worth
                // showing straight away.
                self.show_errors = true;
                self.path_input = d
                    .kind
                    .local_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let DestinationKind::S3 { bucket, prefix, credential_override, .. } = &d.kind {
                    self.bucket_input = bucket.clone();
                    self.prefix_input = prefix.clone();
                    self.own_credentials = credential_override.is_some();
                }
            }
            None => {
                let fresh = Destination {
                    id: Uuid::new_v4(),
                    name: String::new(),
                    kind: DestinationKind::LocalRepository { path: Default::default() },
                    encryption: Some(EncryptionSettings::default()),
                    passphrase_ref: None,
                    retention: RetentionPolicy::default(),
                    enabled: true,
                    auto_discovered: false,
                    bandwidth: None,
                    created_at: Utc::now(),
                    last_verified_at: None,
                };
                self.draft = Some(fresh);
                self.original = None;
                self.show_errors = false;
                self.path_input.clear();
                self.bucket_input.clear();
                self.prefix_input = default_s3_prefix(machine_slug);
                self.own_credentials = false;
            }
        }
        self.creation = None;
        self.creation_error = None;
        self.encryption_open = false;
        self.pin_offline = true;
        self.mirror_prune = false;
    }
}

impl App {
    pub(crate) fn show_destination_editor(&mut self, ui: &mut Ui, id: Option<Uuid>) {
        let t = theme::tokens(ui.ctx());
        let existing = id.and_then(|id| self.data.destination(&id).cloned());
        if id.is_some() && existing.is_none() {
            widgets::banner(
                ui,
                widgets::BannerKind::Warning,
                "That destination no longer exists.",
                Some("It may have been removed in another window."),
                |_| {},
            );
            return;
        }
        let slug = self.data.machine_slug().to_string();
        self.screens.destination_editor.load(existing.as_ref(), &slug);

        let mut report = self.destination_report();
        if !self.screens.destination_editor.show_errors {
            report.problems.clear();
        }
        let creating = existing.is_none();

        widgets::scroll_area(ui, "destination-editor", |ui| {
            self.destination_common(ui, &report, creating);

            let kind_index = self
                .screens
                .destination_editor
                .draft
                .as_ref()
                .map(|d| index_for_kind(&d.kind))
                .unwrap_or(0);

            match kind_index {
                0 | 1 => self.destination_local(ui, &report, kind_index == 1),
                2 => self.destination_s3(ui, &report),
                _ => self.destination_mirror(ui, &report),
            }

            // A mirror has no encryption panel and no retention section: both
            // are omitted entirely rather than shown disabled, because neither
            // applies to a plain copy.
            let is_repository = kind_index != 3;
            if is_repository {
                self.encryption_panel(ui, existing.as_ref());
                widgets::form_group(ui, copy::job::RETENTION_TITLE, None);
                if let Some(draft) = &mut self.screens.destination_editor.draft {
                    crate::gui::screens::retention_editor(ui, &mut draft.retention);
                }
            }

            widgets::form_group(ui, copy::job::BANDWIDTH_TITLE, None);
            if let Some(draft) = &mut self.screens.destination_editor.draft {
                let mut overriding = draft.bandwidth.is_some();
                if widgets::toggle(
                    ui,
                    &mut overriding,
                    copy::job::BANDWIDTH_CUSTOM,
                    None,
                    true,
                )
                .clicked()
                {
                    draft.bandwidth = overriding.then(Default::default);
                }
                if let Some(bandwidth) = &mut draft.bandwidth {
                    ui.horizontal(|ui| {
                        ui.add_space(28.0);
                        ui.vertical(|ui| {
                            let mut upload = bandwidth.upload_kbps.unwrap_or(2000);
                            widgets::number(
                                ui,
                                &mut upload,
                                1..=10_000_000,
                                copy::job::BANDWIDTH_UNIT,
                                true,
                                copy::job::BANDWIDTH_UPLOAD,
                            );
                            bandwidth.upload_kbps = Some(upload);
                        });
                    });
                }
            }

            ui.add_space(space::H2);
            self.destination_footer(ui, &report, existing.as_ref());
            ui.add_space(space::H2);
            let _ = t;
        });
    }

    fn destination_report(&self) -> validation::Report {
        match &self.screens.destination_editor.draft {
            Some(draft) => {
                let others: Vec<Destination> = self
                    .data
                    .destinations
                    .iter()
                    .filter(|d| d.id != draft.id)
                    .cloned()
                    .collect();
                validation::validate_destination(draft, &others, &self.data.jobs)
            }
            None => validation::Report::default(),
        }
    }

    fn destination_common(&mut self, ui: &mut Ui, report: &validation::Report, creating: bool) {
        let t = theme::tokens(ui.ctx());
        let mut kind_change: Option<usize> = None;
        let Some(draft) = &mut self.screens.destination_editor.draft else {
            return;
        };
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::Field::new()
                .label(copy::dest::NAME)
                .placeholder(copy::dest::NAME_PLACEHOLDER)
                .char_limit(64)
                .error(report.for_field(Field::Name))
                .show(ui, &mut draft.name);

            ui.add_space(space::XL);
            widgets::text(ui, copy::dest::KIND, Type::H3, t.text_primary);
            ui.add_space(space::S);
            if creating {
                let labels: Vec<&str> = KIND_CHOICES.iter().map(|(label, _)| *label).collect();
                let mut index = index_for_kind(&draft.kind);
                let before = index;
                widgets::segmented(ui, &mut index, &labels);
                if index != before {
                    kind_change = Some(index);
                }
                ui.add_space(space::M);
                // The trade-off at the point of choice, including the one that
                // matters most: a mirror is not encrypted.
                let (_, trade_off) = KIND_CHOICES[index];
                let colour = if index == 3 { t.warning.tint_text } else { t.text_muted };
                widgets::paragraph_at(ui, trade_off, Type::Small, colour, 560.0);
            } else {
                ui.horizontal(|ui| {
                    let response = widgets::count_pill(ui, draft.kind.label());
                    response.on_hover_text(copy::dest::KIND_LOCKED);
                });
            }

            ui.add_space(space::XL);
            let mut enabled = draft.enabled;
            if widgets::toggle(
                ui,
                &mut enabled,
                copy::dest::ENABLED,
                Some(copy::dest::ENABLED_BODY),
                true,
            )
            .clicked()
            {
                draft.enabled = enabled;
            }
        });

        if let Some(index) = kind_change {
            let path = std::path::PathBuf::from(self.screens.destination_editor.path_input.clone());
            let provider = self.data.providers.first().map(|p| p.id);
            if let Some(draft) = &mut self.screens.destination_editor.draft {
                draft.kind = kind_for_index(index, path, provider);
                draft.encryption = (index != 3).then(EncryptionSettings::default);
            }
        }
    }

    fn destination_local(&mut self, ui: &mut Ui, report: &validation::Report, onedrive: bool) {
        let t = theme::tokens(ui.ctx());
        widgets::form_group(ui, copy::dest::FOLDER, None);

        if onedrive {
            widgets::banner(
                ui,
                widgets::BannerKind::Info,
                copy::dest::ONEDRIVE_EXPLAIN,
                None,
                |_| {},
            );
            ui.add_space(space::L);
        }

        let mut browse = false;
        // `horizontal_top`, not `horizontal`: a centred row pushes a field that
        // carries helper or error text down, away from its own label.
        ui.horizontal_top(|ui| {
            widgets::Field::new()
                .width(520.0)
                .mono()
                .error(report.for_field(Field::Path))
                .show(ui, &mut self.screens.destination_editor.path_input);
            if Button::secondary(copy::action::BROWSE).show(ui).clicked() {
                browse = true;
            }
        });
        if browse {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                self.screens.destination_editor.path_input =
                    path.to_string_lossy().into_owned();
            }
        }
        let path = std::path::PathBuf::from(self.screens.destination_editor.path_input.clone());
        if let Some(draft) = &mut self.screens.destination_editor.draft {
            match &mut draft.kind {
                DestinationKind::LocalRepository { path: p }
                | DestinationKind::OneDrive { path: p, .. }
                | DestinationKind::LocalMirror { path: p } => *p = path.clone(),
                DestinationKind::S3 { .. } => {}
            }
        }

        ui.add_space(space::L);
        self.path_checks(ui, &path);

        if onedrive {
            ui.add_space(space::XL);
            let capabilities = superbackup_core::platform::capabilities();
            let mut pin = self.screens.destination_editor.pin_offline;
            let reason = if capabilities.pin_cloud_files {
                None
            } else {
                superbackup_core::platform::limitations()
                    .into_iter()
                    .find(|l| l.area == "onedrive")
                    .map(|l| l.message)
            };
            match reason {
                Some(message) => {
                    ui.add_enabled_ui(false, |ui| {
                        widgets::checkbox(
                            ui,
                            &mut pin,
                            copy::dest::ONEDRIVE_PIN,
                            Some(&message),
                            false,
                        );
                    });
                }
                None => {
                    if widgets::checkbox(
                        ui,
                        &mut pin,
                        copy::dest::ONEDRIVE_PIN,
                        Some(copy::dest::ONEDRIVE_PIN_BODY),
                        true,
                    )
                    .clicked()
                    {
                        self.screens.destination_editor.pin_offline = pin;
                    }
                }
            }
            ui.add_space(space::L);
            if Button::ghost(copy::dest::ONEDRIVE_REDETECT).show(ui).clicked() {
                self.toasts.info(copy::onboarding::ONEDRIVE_NONE);
            }
            ui.add_space(space::L);
            let mut account = match &self.screens.destination_editor.draft {
                Some(d) => match &d.kind {
                    DestinationKind::OneDrive { account, .. } => {
                        account.clone().unwrap_or_default()
                    }
                    _ => String::new(),
                },
                None => String::new(),
            };
            widgets::Field::new()
                .label(copy::dest::ONEDRIVE_ACCOUNT)
                .helper(copy::dest::ONEDRIVE_ACCOUNT_BODY)
                .show(ui, &mut account);
            if let Some(draft) = &mut self.screens.destination_editor.draft {
                if let DestinationKind::OneDrive { account: a, .. } = &mut draft.kind {
                    *a = (!account.trim().is_empty()).then_some(account);
                }
            }
        }
        let _ = t;
    }

    fn path_checks(&self, ui: &mut Ui, path: &std::path::Path) {
        let t = theme::tokens(ui.ctx());
        if path.as_os_str().is_empty() {
            return;
        }
        let exists = path.exists();
        widgets::checklist_row(
            ui,
            if exists { StepState::Done } else { StepState::Pending },
            if exists { "This folder exists." } else { copy::dest::FOLDER_WILL_CREATE },
            None,
        );
        if let Some((free, total)) = superbackup_core::platform::disk_space(path) {
            let low = free < 20 * 1024 * 1024 * 1024;
            widgets::checklist_row(
                ui,
                if low { StepState::Failed } else { StepState::Done },
                &copy::dest_folder_free(free, total),
                low.then_some(""),
            );
            if low {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    widgets::paragraph_at(
                        ui,
                        copy::dest_folder_low(free),
                        Type::Small,
                        t.warning.tint_text,
                        520.0,
                    );
                });
            }
        }
        let text = path.to_string_lossy();
        if text.starts_with("\\\\") {
            widgets::checklist_row(ui, StepState::Pending, copy::dest::FOLDER_NETWORK, None);
        }
    }

    fn destination_s3(&mut self, ui: &mut Ui, report: &validation::Report) {
        let t = theme::tokens(ui.ctx());
        widgets::form_group(ui, copy::dest::S3_PROVIDER, None);

        let providers: Vec<String> = self
            .data
            .providers
            .iter()
            .map(|p| p.name.clone())
            .chain(std::iter::once(copy::dest::S3_PROVIDER_NEW.to_string()))
            .collect();
        let current = self
            .screens
            .destination_editor
            .draft
            .as_ref()
            .and_then(|d| d.kind.provider_id().copied());
        let mut index = current
            .and_then(|id| self.data.providers.iter().position(|p| p.id == id))
            .unwrap_or(0);
        let new_index = providers.len() - 1;
        let mut new_provider = false;
        if widgets::combo(ui, "dest-provider", &mut index, &providers, 400.0, true) {
            if index == new_index {
                new_provider = true;
            } else if let Some(provider) = self.data.providers.get(index) {
                let provider_id = provider.id;
                if let Some(draft) = &mut self.screens.destination_editor.draft {
                    if let DestinationKind::S3 { provider_id: p, .. } = &mut draft.kind {
                        *p = provider_id;
                    }
                }
            }
        }

        // The strip that stops the user re-entering credentials per bucket.
        ui.add_space(space::M);
        match current.and_then(|id| self.data.provider(&id)) {
            Some(provider) => {
                let superbackup_core::model::ProviderKind::S3 {
                    endpoint, region, flavour, ..
                } = &provider.kind;
                let line = format!("{endpoint} · {region} · {}", flavour.title());
                egui::Frame::new()
                    .fill(t.bg_raised)
                    .corner_radius(radius::CONTROL)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width().min(560.0));
                        ui.horizontal(|ui| {
                            widgets::elided(
                                ui,
                                &line,
                                Type::MonoSmall,
                                t.text_secondary,
                                380.0,
                                false,
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::link(ui, copy::dest::S3_PROVIDER_EDIT).clicked() {
                                    new_provider = false;
                                }
                            });
                        });
                    });
            }
            None => {
                widgets::banner(
                    ui,
                    widgets::BannerKind::Warning,
                    "This destination names a storage provider that no longer exists.",
                    Some("Choose one above, or create a new provider."),
                    |_| {},
                );
            }
        }

        ui.add_space(space::XL);
        widgets::Field::new()
            .label(copy::dest::S3_BUCKET)
            .width(280.0)
            .error(report.for_field(Field::Bucket))
            .show(ui, &mut self.screens.destination_editor.bucket_input);

        ui.add_space(space::XL);
        let prefix_response = widgets::Field::new()
            .label(copy::dest::S3_PREFIX)
            .width(400.0)
            .mono()
            .helper(copy::dest::S3_PREFIX_BODY)
            .show(ui, &mut self.screens.destination_editor.prefix_input);
        // Normalised on blur, and shown back immediately, so surprises happen
        // at edit time rather than at run time.
        if prefix_response.lost_focus() {
            let normalised = normalise_prefix(&self.screens.destination_editor.prefix_input);
            if normalised != self.screens.destination_editor.prefix_input {
                self.screens.destination_editor.prefix_input = normalised.clone();
                self.toasts.info(copy::dest_s3_prefix_normalised(&normalised));
            }
        }
        ui.add_space(space::S);
        widgets::text(
            ui,
            copy::dest_s3_full_path(
                &self.screens.destination_editor.bucket_input,
                &self.screens.destination_editor.prefix_input,
            ),
            Type::MonoSmall,
            t.text_muted,
        );

        let bucket = self.screens.destination_editor.bucket_input.clone();
        let prefix = self.screens.destination_editor.prefix_input.clone();
        if let Some(draft) = &mut self.screens.destination_editor.draft {
            if let DestinationKind::S3 { bucket: b, prefix: p, .. } = &mut draft.kind {
                *b = bucket;
                *p = prefix;
            }
        }

        widgets::form_group(ui, copy::dest::S3_CREDS, None);
        let provider_name = current
            .and_then(|id| self.data.provider(&id))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| copy::state::UNKNOWN.to_string());
        let mut own = self.screens.destination_editor.own_credentials;
        if widgets::radio(
            ui,
            !own,
            copy::dest::S3_CREDS_INHERIT,
            Some(&copy::dest_s3_creds_inherit_body(&provider_name)),
            true,
        )
        .clicked()
        {
            own = false;
        }
        if widgets::radio(
            ui,
            own,
            copy::dest::S3_CREDS_OWN,
            Some(copy::dest::S3_CREDS_OWN_BODY),
            true,
        )
        .clicked()
        {
            own = true;
        }
        self.screens.destination_editor.own_credentials = own;

        if own {
            ui.add_space(space::L);
            // While the vault is locked this whole group is the inline prompt,
            // in the same position and the same size.
            if self.data.gate(Action::CreateRepository) == Gate::NeedsUnlock {
                if widgets::inline_unlock(ui) {
                    self.open_modal(Modal::Unlock(modals::UnlockState::voluntary()));
                }
            } else {
                widgets::Field::new()
                    .label(copy::prov::ACCESS_KEY)
                    .show(ui, &mut self.screens.destination_editor.access_key);
                ui.add_space(space::L);
                let mut revealed = self.screens.destination_editor.revealed;
                widgets::passphrase_field(
                    ui,
                    &mut self.screens.destination_editor.secret_key,
                    copy::prov::SECRET_KEY,
                    &mut revealed,
                    None,
                    400.0,
                );
                self.screens.destination_editor.revealed = revealed;
            }
        }
    }

    fn destination_mirror(&mut self, ui: &mut Ui, report: &validation::Report) {
        let t = theme::tokens(ui.ctx());
        widgets::form_group(ui, copy::dest::FOLDER, None);
        widgets::banner(ui, widgets::BannerKind::Warning, copy::dest::MIRROR_EXPLAIN, None, |_| {});
        ui.add_space(space::L);

        let mut browse = false;
        ui.horizontal_top(|ui| {
            widgets::Field::new()
                .width(520.0)
                .mono()
                .error(report.for_field(Field::Path))
                .show(ui, &mut self.screens.destination_editor.path_input);
            if Button::secondary(copy::action::BROWSE).show(ui).clicked() {
                browse = true;
            }
        });
        if browse {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                self.screens.destination_editor.path_input = path.to_string_lossy().into_owned();
            }
        }
        let path = std::path::PathBuf::from(self.screens.destination_editor.path_input.clone());
        if let Some(draft) = &mut self.screens.destination_editor.draft {
            if let DestinationKind::LocalMirror { path: p } = &mut draft.kind {
                *p = path.clone();
            }
            // A mirror has no encryption, and the model says so too.
            draft.encryption = None;
            draft.passphrase_ref = None;
        }
        ui.add_space(space::L);
        self.path_checks(ui, &path);

        ui.add_space(space::XL);
        let mut prune = self.screens.destination_editor.mirror_prune;
        if widgets::checkbox(ui, &mut prune, copy::dest::MIRROR_PRUNE, None, true).clicked() {
            self.screens.destination_editor.mirror_prune = prune;
        }
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            widgets::paragraph_at(
                ui,
                copy::dest::MIRROR_PRUNE_BODY,
                Type::Small,
                t.danger.tint_text,
                520.0,
            );
        });
    }

    // -- T-4: the encryption panel -----------------------------------------

    fn encryption_panel(&mut self, ui: &mut Ui, existing: Option<&Destination>) {
        let t = theme::tokens(ui.ctx());
        // A repository that exists already has fixed settings; the panel is
        // replaced by a read-only summary that says why.
        let connected = existing.map(|d| d.passphrase_ref.is_some()).unwrap_or(false);
        widgets::form_group(ui, copy::enc::TITLE, Some(copy::enc::LEAD));

        if connected {
            if let Some(destination) = existing {
                widgets::kv(
                    ui,
                    copy::enc::PASS_TITLE,
                    modals::passphrase_source_line(destination),
                    false,
                );
                ui.add_space(space::XS);
                ui.horizontal(|ui| {
                    ui.add_space(crate::gui::theme::size::KV_LABEL_W + space::XL);
                    widgets::text(ui, copy::writedown::CANNOT_SHOW, Type::Small, t.text_muted);
                });
                if let Some(settings) = &destination.encryption {
                    ui.add_space(space::L);
                    widgets::kv(ui, copy::enc::ALGORITHM, settings.algorithm.kopia_id(), true);
                    widgets::kv(ui, copy::enc::HASH, settings.hash.kopia_id(), true);
                    widgets::kv(ui, copy::enc::SPLITTER, settings.splitter.kopia_id(), true);
                }
                ui.add_space(space::L);
                widgets::paragraph_at(
                    ui,
                    copy::writedown::ESCAPE,
                    Type::Small,
                    t.text_muted,
                    560.0,
                );
            }
            return;
        }

        match self.screens.destination_editor.creation {
            Some(CreationPhase::Running(step)) => {
                self.creation_checklist(ui, Some(step), None);
                return;
            }
            Some(CreationPhase::Failed) => {
                let message = self.screens.destination_editor.creation_error.clone();
                self.creation_checklist(ui, None, message.as_deref());
                ui.add_space(space::L);
                ui.horizontal(|ui| {
                    if Button::primary(copy::action::RETRY).show(ui).clicked() {
                        if let Some(id) = existing.map(|d| d.id) {
                            self.request_create_repository(id);
                        }
                    }
                    if Button::secondary(copy::enc::CREATE_CHANGE).show(ui).clicked() {
                        self.screens.destination_editor.creation = None;
                    }
                });
                return;
            }
            Some(CreationPhase::Done) => {
                widgets::banner(
                    ui,
                    widgets::BannerKind::Success,
                    copy::dest::VERIFY_OK,
                    None,
                    |_| {},
                );
                return;
            }
            _ => {}
        }

        let open = self.screens.destination_editor.encryption_open;
        if !open {
            egui::Frame::new()
                .fill(t.bg_raised)
                .corner_radius(radius::CONTROL)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width().min(700.0));
                    ui.horizontal(|ui| {
                        widgets::paragraph_at(
                            ui,
                            copy::enc::SUMMARY,
                            Type::Small,
                            t.text_secondary,
                            (ui.available_width() - 90.0).max(200.0),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::link(ui, copy::enc::CHANGE).clicked() {
                                self.screens.destination_editor.encryption_open = true;
                            }
                        });
                    });
                });
        } else {
            let mut suggest_splitter = false;
            if let Some(draft) = &mut self.screens.destination_editor.draft {
                let settings = draft.encryption.get_or_insert_with(EncryptionSettings::default);

                widgets::text(ui, copy::enc::ALGORITHM, Type::H3, t.text_primary);
                ui.add_space(space::M);
                for algorithm in EncryptionAlgorithm::all() {
                    let selected = settings.algorithm == *algorithm;
                    let label = algorithm.kopia_id();
                    if widgets::radio(ui, selected, label, Some(algorithm.describe()), true)
                        .clicked()
                    {
                        settings.algorithm = *algorithm;
                    }
                    if *algorithm == EncryptionAlgorithm::Aes256GcmHmacSha256 {
                        ui.horizontal(|ui| {
                            ui.add_space(24.0);
                            widgets::text(
                                ui,
                                copy::enc::RECOMMENDED,
                                Type::SmallStrong,
                                t.accent,
                            );
                        });
                    }
                    ui.add_space(space::S);
                }

                ui.add_space(space::XL);
                widgets::text(ui, copy::enc::HASH, Type::H3, t.text_primary);
                ui.add_space(space::S);
                let hashes: Vec<String> =
                    HashAlgorithm::all().iter().map(|h| h.kopia_id().to_string()).collect();
                let mut hash_index = HashAlgorithm::all()
                    .iter()
                    .position(|h| *h == settings.hash)
                    .unwrap_or(0);
                if widgets::combo(ui, "enc-hash", &mut hash_index, &hashes, 320.0, true) {
                    if let Some(hash) = HashAlgorithm::all().get(hash_index) {
                        settings.hash = *hash;
                    }
                }
                ui.add_space(space::S);
                widgets::paragraph_at(
                    ui,
                    hash_helper(settings.hash),
                    Type::Small,
                    t.text_muted,
                    520.0,
                );

                ui.add_space(space::XL);
                widgets::text(ui, copy::enc::SPLITTER, Type::H3, t.text_primary);
                ui.add_space(space::S);
                let splitters: Vec<String> =
                    Splitter::all().iter().map(|s| s.kopia_id().to_string()).collect();
                let mut splitter_index =
                    Splitter::all().iter().position(|s| *s == settings.splitter).unwrap_or(0);
                if widgets::combo(ui, "enc-splitter", &mut splitter_index, &splitters, 320.0, true)
                {
                    if let Some(splitter) = Splitter::all().get(splitter_index) {
                        settings.splitter = *splitter;
                    }
                }
                ui.add_space(space::S);
                widgets::paragraph_at(ui, copy::enc::SPLITTER_BODY, Type::Small, t.text_muted, 520.0);
                if settings.splitter != Splitter::recommended_for_many_small_files() {
                    ui.add_space(space::M);
                    egui::Frame::new()
                        .fill(t.bg_raised)
                        .corner_radius(radius::CONTROL)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width().min(560.0));
                            ui.horizontal(|ui| {
                                widgets::paragraph_at(
                                    ui,
                                    copy::enc::SPLITTER_SUGGEST,
                                    Type::Small,
                                    t.text_secondary,
                                    (ui.available_width() - 80.0).max(200.0),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if Button::secondary(copy::enc::SPLITTER_SUGGEST_ACTION)
                                        .compact()
                                        .show(ui)
                                        .clicked()
                                    {
                                        suggest_splitter = true;
                                    }
                                });
                            });
                        });
                }

                ui.add_space(space::XL);
                let mut ecc_on = settings.ecc.is_some();
                if widgets::toggle(ui, &mut ecc_on, copy::enc::ECC, Some(copy::enc::ECC_BODY), true)
                    .clicked()
                {
                    settings.ecc = ecc_on.then_some(EccAlgorithm::ReedSolomonCrc32);
                    if ecc_on && settings.ecc_overhead_percent == 0 {
                        settings.ecc_overhead_percent = 5;
                    }
                }
                if settings.ecc.is_some() {
                    ui.horizontal(|ui| {
                        ui.add_space(28.0);
                        let mut overhead = settings.ecc_overhead_percent as u32;
                        widgets::number(
                            ui,
                            &mut overhead,
                            1..=20,
                            "%",
                            true,
                            copy::enc::ECC_OVERHEAD,
                        );
                        settings.ecc_overhead_percent = overhead as u8;
                        ui.add_space(space::M);
                        widgets::text(ui, copy::enc::ECC_ALGORITHM, Type::Small, t.text_muted);
                    });
                }

                ui.add_space(space::XL);
                widgets::text(ui, copy::enc::PASS_TITLE, Type::H3, t.text_primary);
                ui.add_space(space::M);
                for (source, label, helper) in [
                    (
                        PassphraseSource::Generated,
                        copy::enc::PASS_GENERATED,
                        copy::enc::PASS_GENERATED_BODY,
                    ),
                    (
                        PassphraseSource::UserSupplied,
                        copy::enc::PASS_SUPPLIED,
                        copy::enc::PASS_SUPPLIED_BODY,
                    ),
                    (
                        PassphraseSource::DerivedFromMaster,
                        copy::enc::PASS_DERIVED,
                        copy::enc::PASS_DERIVED_BODY,
                    ),
                ] {
                    if widgets::radio(
                        ui,
                        settings.passphrase_source == source,
                        label,
                        Some(helper),
                        true,
                    )
                    .clicked()
                    {
                        settings.passphrase_source = source;
                    }
                    ui.add_space(space::S);
                }
                ui.add_space(space::M);
                widgets::paragraph_at(ui, copy::enc::LEAD, Type::Small, t.text_muted, 560.0);
            }
            if suggest_splitter {
                if let Some(draft) = &mut self.screens.destination_editor.draft {
                    if let Some(settings) = &mut draft.encryption {
                        settings.splitter = Splitter::recommended_for_many_small_files();
                    }
                }
            }
        }

        ui.add_space(space::XL);
        let gate = self.data.gate(Action::CreateRepository);
        let mut create = Button::primary(copy::enc::CREATE).icon(Icon::Plus);
        if let Some(reason) = gate.reason() {
            create = create.disabled_because(reason);
        } else if existing.is_none() {
            create = create.disabled_because("Save this destination before creating its repository.");
        }
        if create.show(ui).clicked() {
            if let Some(id) = existing.map(|d| d.id) {
                self.request_create_repository(id);
            }
        }
    }

    fn creation_checklist(&self, ui: &mut Ui, running: Option<usize>, error: Option<&str>) {
        let t = theme::tokens(ui.ctx());
        let steps = [
            copy::enc::STEP_CHECK,
            copy::enc::STEP_CREATE,
            copy::enc::STEP_STORE,
            copy::enc::STEP_POLICY,
            copy::enc::STEP_MANIFEST,
        ];
        for (index, step) in steps.iter().enumerate() {
            let state = match (running, error) {
                (Some(current), _) if index < current => StepState::Done,
                (Some(current), _) if index == current => StepState::Running,
                (Some(_), _) => StepState::Pending,
                (None, Some(_)) if index == 0 => StepState::Failed,
                _ => StepState::Pending,
            };
            widgets::checklist_row(ui, state, step, None);
            if index == steps.len() - 1 {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    widgets::paragraph_at(
                        ui,
                        copy::enc::MANIFEST_BODY,
                        Type::Small,
                        t.text_muted,
                        520.0,
                    );
                });
            }
        }
        if let Some(message) = error {
            ui.add_space(space::L);
            widgets::banner(
                ui,
                widgets::BannerKind::Danger,
                copy::enc::CREATE_FAILED,
                Some(message),
                |_| {},
            );
        }
    }

    fn destination_footer(
        &mut self,
        ui: &mut Ui,
        report: &validation::Report,
        existing: Option<&Destination>,
    ) {
        let mut save = false;
        let mut verify = false;
        let mut remove = false;
        ui.horizontal(|ui| {
            let mut button =
                Button::primary(copy::action::SAVE_CHANGES).enabled(report.ok());
            if let Some(summary) = report.summary() {
                button = button.disabled_because(Box::leak(summary.into_boxed_str()));
            }
            if button.show(ui).clicked() {
                save = true;
            }
            let gate = self.data.gate(Action::VerifyDestination);
            let mut verify_button = Button::secondary(copy::action::VERIFY).icon(Icon::PlugZap);
            if let Some(reason) = gate.reason() {
                verify_button = verify_button.disabled_because(reason);
            } else if existing.is_none() {
                verify_button = verify_button.enabled(false);
            }
            if verify_button.show(ui).clicked() {
                verify = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if existing.is_some()
                    && Button::danger_ghost(copy::action::REMOVE).icon(Icon::Trash).show(ui).clicked()
                {
                    remove = true;
                }
            });
        });

        if save {
            self.screens.destination_editor.show_errors = true;
            if !report.ok() {
                return;
            }
            if let Some(draft) = self.screens.destination_editor.draft.clone() {
                let name = draft.name.clone();
                let request = if existing.is_some() {
                    Request::DestinationUpdate { destination: Box::new(draft) }
                } else {
                    Request::DestinationCreate { destination: Box::new(draft) }
                };
                self.ask(Intent::SaveDestination(name), request);
                self.go(Route::Destinations);
            }
        }
        if verify {
            if let Some(id) = existing.map(|d| d.id) {
                self.request_verify(id);
            }
        }
        if remove {
            if let Some(id) = existing.map(|d| d.id) {
                let confirm = modals::remove_destination_confirm(&self.data, id);
                self.open_modal(Modal::Confirm(confirm));
            }
        }
    }
}

fn hash_helper(hash: HashAlgorithm) -> &'static str {
    match hash {
        HashAlgorithm::Blake2b256 => copy::enc::HASH_BLAKE2B256,
        HashAlgorithm::Blake2b256128 => copy::enc::HASH_BLAKE2B256128,
        HashAlgorithm::Blake3256 => copy::enc::HASH_BLAKE3256,
        HashAlgorithm::Blake2s256 => copy::enc::HASH_BLAKE2S256,
        HashAlgorithm::HmacSha256 => copy::enc::HASH_HMACSHA256,
        HashAlgorithm::HmacSha256128 => copy::enc::HASH_HMACSHA256128,
    }
}
