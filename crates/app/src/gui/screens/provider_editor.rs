//! `P-2`, `P-3` and `P-4`'s impact display. A provider is credentials plus an
//! endpoint, defined once and reused; the editor's job is to make that reuse
//! visible before anything is changed.

use std::collections::HashMap;

use chrono::Utc;
use egui::{Align, Layout, Ui};
use uuid::Uuid;

use superbackup_core::ipc::protocol::{ErrorPayload, ProbeReply, Request, SecretString};
use superbackup_core::model::{
    ProviderKind, S3Credentials, S3Flavour, SecretRef, StorageProvider,
};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::daemon::Intent;
use crate::gui::data::{Action, Gate};
use crate::gui::icons::Icon;
use crate::gui::modals::{self, Modal};
use crate::gui::nav::Route;
use crate::gui::theme::{self, radius, space, Type};
use crate::gui::validation::{self, Field};
use crate::gui::widgets::{self, Button, StepState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeState {
    Running,
    Ok { buckets: Vec<String> },
    Failed(String),
}

#[derive(Default)]
pub struct State {
    pub draft: Option<StorageProvider>,
    pub original: Option<StorageProvider>,
    pub access_key: String,
    pub secret_key: String,
    pub session_token: String,
    pub use_session_token: bool,
    pub revealed: bool,
    pub replacing_secret: bool,
    pub show_buckets: bool,
    pub impact_open: bool,
    /// As in the destination editor: an untouched form shows no errors.
    pub show_errors: bool,
    probes: HashMap<Uuid, ProbeState>,
    known_refs: Vec<SecretRef>,
}

impl State {
    pub fn probe_started(&mut self, id: Uuid) {
        self.probes.insert(id, ProbeState::Running);
    }
    pub fn probe(&mut self, id: Uuid, probe: ProbeReply) {
        let state = if probe.reachable {
            ProbeState::Ok { buckets: Vec::new() }
        } else {
            ProbeState::Failed(modals::probe_message(&probe))
        };
        self.probes.insert(id, state);
    }
    pub fn probe_failed(&mut self, id: Uuid, payload: ErrorPayload) {
        self.probes.insert(id, ProbeState::Failed(payload.message));
    }
    pub fn probing(&self, id: Uuid) -> bool {
        matches!(self.probes.get(&id), Some(ProbeState::Running))
    }
    pub fn busy(&self) -> bool {
        self.probes.values().any(|p| *p == ProbeState::Running)
    }
    pub fn secret_refs(&mut self, refs: Vec<SecretRef>) {
        self.known_refs = refs;
    }
    fn has_stored_secret(&self, provider: &StorageProvider) -> bool {
        provider.secret_refs().iter().any(|r| self.known_refs.contains(r))
    }

    fn load(&mut self, provider: Option<&StorageProvider>) {
        let same = match (&self.original, provider) {
            (Some(o), Some(p)) => o.id == p.id,
            (None, None) => self.draft.is_some(),
            _ => false,
        };
        if same {
            return;
        }
        match provider {
            Some(p) => {
                self.draft = Some(p.clone());
                self.original = Some(p.clone());
                self.replacing_secret = false;
                self.show_errors = true;
            }
            None => {
                let id = Uuid::new_v4();
                self.draft = Some(StorageProvider {
                    id,
                    name: String::new(),
                    kind: ProviderKind::S3 {
                        endpoint: S3Flavour::Storj.default_endpoint().unwrap_or("").to_string(),
                        region: S3Flavour::Storj.default_region().unwrap_or("").to_string(),
                        credentials: S3Credentials::for_provider(&id),
                        tls: true,
                        path_style: S3Flavour::Storj.wants_path_style(),
                        flavour: S3Flavour::Storj,
                    },
                    notes: String::new(),
                    created_at: Utc::now(),
                    last_verified_at: None,
                });
                self.original = None;
                self.replacing_secret = true;
                self.show_errors = false;
            }
        }
        self.access_key.clear();
        self.secret_key.clear();
        self.session_token.clear();
        self.use_session_token = false;
        self.show_buckets = false;
    }
}

impl App {
    pub(crate) fn show_provider_editor(&mut self, ui: &mut Ui, id: Option<Uuid>) {
        let t = theme::tokens(ui.ctx());
        let existing = id.and_then(|id| self.data.provider(&id).cloned());
        if id.is_some() && existing.is_none() {
            widgets::banner(
                ui,
                widgets::BannerKind::Warning,
                "That storage provider no longer exists.",
                None,
                |_| {},
            );
            return;
        }
        self.screens.provider_editor.load(existing.as_ref());

        let mut report = self.provider_report();
        if !self.screens.provider_editor.show_errors {
            report.problems.clear();
        }
        let mut save = false;
        let mut test = false;
        let mut rotate = false;

        widgets::scroll_area(ui, "provider-editor", |ui| {
            // The impact strip: used by N destinations across M jobs.
            if let Some(provider) = &existing {
                let (inheriting, overriding) = self.data.destinations_using(&provider.id);
                let jobs = self.data.jobs_via_provider(&provider.id);
                egui::Frame::new()
                    .fill(t.bg_raised)
                    .corner_radius(radius::CONTROL)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            widgets::text(
                                ui,
                                copy::prov_impact(inheriting.len(), jobs.len()),
                                Type::Small,
                                t.text_secondary,
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::link(ui, copy::prov::IMPACT_SHOW).clicked() {
                                    self.screens.provider_editor.impact_open =
                                        !self.screens.provider_editor.impact_open;
                                }
                            });
                        });
                    });
                if self.screens.provider_editor.impact_open {
                    ui.add_space(space::M);
                    for destination in &inheriting {
                        widgets::kv(
                            ui,
                            &destination.name,
                            &crate::gui::screens::job_editor::destination_location(
                                &self.data,
                                destination,
                            ),
                            true,
                        );
                    }
                    if !overriding.is_empty() {
                        ui.add_space(space::L);
                        widgets::text(
                            ui,
                            copy::prov::IMPACT_UNAFFECTED,
                            Type::BodyStrong,
                            t.text_primary,
                        );
                        for destination in &overriding {
                            widgets::kv(ui, &destination.name, destination.kind.label(), false);
                        }
                    }
                }
                ui.add_space(space::XL);
            }

            widgets::card(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(draft) = &mut self.screens.provider_editor.draft {
                    widgets::Field::new()
                        .label(copy::prov::NAME)
                        .placeholder(copy::prov::NAME_PLACEHOLDER)
                        .char_limit(64)
                        .error(report.for_field(Field::Name))
                        .show(ui, &mut draft.name);
                    ui.add_space(space::XL);
                    widgets::Field::new()
                        .label(copy::prov::NOTES)
                        .helper(copy::prov::NOTES_BODY)
                        .rows(3)
                        .show(ui, &mut draft.notes);
                }
            });

            self.provider_connection(ui, &report);
            self.provider_credentials(ui, &report, existing.as_ref());
            self.provider_test_panel(ui, existing.as_ref());

            ui.add_space(space::H2);
            ui.horizontal(|ui| {
                let mut button = Button::primary(copy::prov::SAVE).enabled(report.ok());
                if let Some(summary) = report.summary() {
                    button = button.disabled_because(Box::leak(summary.into_boxed_str()));
                }
                if button.show(ui).clicked() {
                    save = true;
                }
                let gate = self.data.gate(Action::TestProvider);
                let mut test_button =
                    Button::secondary(copy::action::TEST_CONNECTION).icon(Icon::PlugZap);
                if let Some(reason) = gate.reason() {
                    test_button = test_button.disabled_because(reason);
                } else if existing.is_none() {
                    test_button =
                        test_button.disabled_because("Save the provider before testing it.");
                }
                if test_button.show(ui).clicked() {
                    test = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if existing.is_some()
                        && Button::secondary("Rotate keys…").icon(Icon::KeyRound).show(ui).clicked()
                    {
                        rotate = true;
                    }
                });
            });
            ui.add_space(space::H2);
        });

        if save {
            self.screens.provider_editor.show_errors = true;
            if report.ok() {
                self.save_provider(existing.is_some());
            }
        }
        if test {
            if let Some(id) = existing.map(|p| p.id) {
                self.request_test_provider(id);
            }
        }
        if rotate {
            if let Some(id) = id {
                if self.data.gate(Action::RotateKeys).allowed() {
                    self.open_modal(Modal::Rotate(modals::RotateState::new(id)));
                } else {
                    self.pending = Some(crate::gui::app::Pending::RotateKeys(id));
                    self.open_modal(Modal::Unlock(modals::UnlockState::blocking()));
                }
            }
        }
    }

    fn provider_report(&self) -> validation::Report {
        match &self.screens.provider_editor.draft {
            Some(draft) => {
                let others: Vec<StorageProvider> = self
                    .data
                    .providers
                    .iter()
                    .filter(|p| p.id != draft.id)
                    .cloned()
                    .collect();
                let needs_credentials = self.screens.provider_editor.replacing_secret;
                validation::validate_provider(
                    draft,
                    &others,
                    &self.screens.provider_editor.access_key,
                    &self.screens.provider_editor.secret_key,
                    needs_credentials,
                )
            }
            None => validation::Report::default(),
        }
    }

    fn provider_connection(&mut self, ui: &mut Ui, report: &validation::Report) {
        let t = theme::tokens(ui.ctx());
        widgets::form_group(ui, "Connection", None);
        let mut filled_message: Option<String> = None;

        let Some(draft) = &mut self.screens.provider_editor.draft else {
            return;
        };
        let ProviderKind::S3 { endpoint, region, tls, path_style, flavour, .. } = &mut draft.kind;

        let flavours: Vec<String> =
            S3Flavour::all().iter().map(|f| f.title().to_string()).collect();
        let mut index = S3Flavour::all().iter().position(|f| f == flavour).unwrap_or(0);
        widgets::text(ui, copy::prov::TYPE, Type::H3, t.text_primary);
        ui.add_space(space::S);
        if widgets::combo(ui, "provider-flavour", &mut index, &flavours, 400.0, true) {
            if let Some(chosen) = S3Flavour::all().get(index) {
                let previous = *flavour;
                *flavour = *chosen;
                // Defaults are applied only when the field is empty or still
                // holds the previous flavour's default.
                let endpoint_is_default = endpoint.trim().is_empty()
                    || Some(endpoint.as_str()) == previous.default_endpoint();
                if endpoint_is_default {
                    if let Some(default) = chosen.default_endpoint() {
                        *endpoint = default.to_string();
                    }
                }
                let region_is_default =
                    region.trim().is_empty() || Some(region.as_str()) == previous.default_region();
                if region_is_default {
                    *region = chosen.default_region().unwrap_or("").to_string();
                }
                *path_style = chosen.wants_path_style();
                filled_message = Some(copy::prov_type_filled(chosen.title()));
            }
        }
        if let Some(message) = &filled_message {
            ui.add_space(space::S);
            widgets::text(ui, message, Type::Small, t.text_muted);
        }

        ui.add_space(space::XL);
        widgets::Field::new()
            .label(copy::prov::ENDPOINT)
            .mono()
            .error(report.for_field(Field::Endpoint))
            .show(ui, endpoint);
        ui.add_space(space::S);
        match validation::parse_endpoint(endpoint) {
            Ok(parsed) => {
                widgets::text(
                    ui,
                    copy::prov_endpoint_parsed(
                        &parsed.scheme,
                        &parsed.host,
                        if *tls { "on" } else { "off" },
                        parsed.port,
                    ),
                    Type::Small,
                    t.text_muted,
                );
            }
            Err(message) => {
                widgets::text(ui, message, Type::Small, t.danger.tint_text);
            }
        }

        ui.add_space(space::XL);
        let region_required = *flavour == S3Flavour::AwsS3;
        widgets::Field::new()
            .label(copy::prov::REGION)
            .width(200.0)
            .helper(if region_required {
                copy::prov::REGION_REQUIRED
            } else {
                copy::prov::REGION_OPTIONAL
            })
            .error(report.for_field(Field::Region))
            .show(ui, region);

        ui.add_space(space::XL);
        let mut tls_on = *tls;
        if widgets::toggle(ui, &mut tls_on, copy::prov::TLS, None, true).clicked() {
            *tls = tls_on;
        }
        if !*tls {
            ui.add_space(space::S);
            widgets::paragraph_at(
                ui,
                copy::prov::TLS_OFF_WARNING,
                Type::Small,
                t.warning.tint_text,
                560.0,
            );
        }

        ui.add_space(space::L);
        let mut path_style_on = *path_style;
        let helper = copy::prov::PATH_STYLE_BODY;
        if widgets::toggle(ui, &mut path_style_on, copy::prov::PATH_STYLE, Some(helper), true)
            .clicked()
        {
            *path_style = path_style_on;
        }
        // The model says this field changes nothing today; the interface must
        // not imply an effect it does not have.
        ui.horizontal(|ui| {
            ui.add_space(44.0);
            widgets::paragraph_at(
                ui,
                "kopia's S3 backend chooses the addressing style itself, so this is recorded but not acted on yet.",
                Type::Small,
                t.text_muted,
                520.0,
            );
        });
    }

    fn provider_credentials(
        &mut self,
        ui: &mut Ui,
        report: &validation::Report,
        existing: Option<&StorageProvider>,
    ) {
        let t = theme::tokens(ui.ctx());
        widgets::form_group(ui, copy::prov::CREDS_TITLE, None);

        if self.data.gate(Action::TestProvider) == Gate::NeedsUnlock {
            if widgets::inline_unlock(ui) {
                self.open_modal(Modal::Unlock(modals::UnlockState::voluntary()));
            }
            return;
        }

        let stored = existing
            .map(|p| self.screens.provider_editor.has_stored_secret(p))
            .unwrap_or(false);
        if stored && !self.screens.provider_editor.replacing_secret {
            widgets::kv(ui, copy::prov::SECRET_KEY, "••••••••••••", true);
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                widgets::text(ui, copy::prov::CREDS_STORED, Type::Small, t.text_muted);
                ui.add_space(space::M);
                if Button::ghost(copy::prov::CREDS_REPLACE).compact().show(ui).clicked() {
                    self.screens.provider_editor.replacing_secret = true;
                }
            });
            return;
        }

        // An access key id is an identifier, not a secret: masking it makes
        // verification harder for no benefit.
        widgets::Field::new()
            .label(copy::prov::ACCESS_KEY)
            .error(report.for_field(Field::Credentials))
            .show(ui, &mut self.screens.provider_editor.access_key);
        ui.add_space(space::XL);
        let mut revealed = self.screens.provider_editor.revealed;
        widgets::passphrase_field(
            ui,
            &mut self.screens.provider_editor.secret_key,
            copy::prov::SECRET_KEY,
            &mut revealed,
            None,
            400.0,
        );
        self.screens.provider_editor.revealed = revealed;

        ui.add_space(space::L);
        let mut use_token = self.screens.provider_editor.use_session_token;
        if widgets::checkbox(
            ui,
            &mut use_token,
            copy::prov::USE_SESSION_TOKEN,
            Some(copy::prov::SESSION_BODY),
            true,
        )
        .clicked()
        {
            self.screens.provider_editor.use_session_token = use_token;
        }
        if use_token {
            ui.add_space(space::M);
            widgets::Field::new()
                .label(copy::prov::SESSION_TOKEN)
                .rows(3)
                .mono()
                .show(ui, &mut self.screens.provider_editor.session_token);
        }

        ui.add_space(space::L);
        widgets::paragraph_at(ui, copy::prov::CREDS_FOOTNOTE, Type::Small, t.text_muted, 560.0);
    }

    fn provider_test_panel(&mut self, ui: &mut Ui, existing: Option<&StorageProvider>) {
        let t = theme::tokens(ui.ctx());
        let Some(provider) = existing else {
            return;
        };
        let Some(state) = self.screens.provider_editor.probes.get(&provider.id).cloned() else {
            return;
        };
        ui.add_space(space::XL);
        let steps = [
            copy::prov::TEST_RESOLVING,
            copy::prov::TEST_TLS,
            copy::prov::TEST_SIGNING,
            copy::prov::TEST_LISTING,
        ];
        match state {
            ProbeState::Running => {
                for step in steps {
                    widgets::checklist_row(ui, StepState::Running, step, None);
                }
            }
            ProbeState::Ok { buckets } => {
                widgets::banner(
                    ui,
                    widgets::BannerKind::Success,
                    &copy::prov_test_ok(buckets.len()),
                    None,
                    |_| {},
                );
                if !buckets.is_empty() {
                    ui.add_space(space::M);
                    if widgets::link(ui, copy::prov::TEST_SHOW_BUCKETS).clicked() {
                        self.screens.provider_editor.show_buckets =
                            !self.screens.provider_editor.show_buckets;
                    }
                    if self.screens.provider_editor.show_buckets {
                        widgets::code_block(ui, &buckets.join("\n"), 200.0, None);
                    }
                }
            }
            ProbeState::Failed(message) => {
                let mut copy_details = false;
                widgets::banner(
                    ui,
                    widgets::BannerKind::Danger,
                    &message,
                    Some(copy::prov::ERR_DIAG_NOTE),
                    |ui| {
                        if Button::secondary(copy::prov::ERR_COPY_DIAG).compact().show(ui).clicked()
                        {
                            copy_details = true;
                        }
                    },
                );
                if copy_details {
                    let ProviderKind::S3 { endpoint, region, tls, path_style, flavour, .. } =
                        &provider.kind;
                    let block = format!(
                        "provider: {}\nflavour: {}\nendpoint: {endpoint}\nregion: {region}\ntls: {tls}\npath_style: {path_style}\nresult: {message}",
                        provider.name,
                        flavour.title()
                    );
                    ui.ctx().copy_text(block);
                    self.toasts.info(copy::toast::COPIED_CLIPBOARD);
                }
            }
        }
        let _ = t;
    }

    fn save_provider(&mut self, existing: bool) {
        let Some(draft) = self.screens.provider_editor.draft.clone() else {
            return;
        };
        let name = draft.name.clone();
        let provider_id = draft.id;
        let request = if existing {
            Request::ProviderUpdate { provider: Box::new(draft) }
        } else {
            Request::ProviderCreate { provider: Box::new(draft) }
        };
        self.ask(Intent::SaveProvider(name), request);

        // The credentials go into the vault separately, under the handles the
        // model derives, never onto a command line.
        let access = self.screens.provider_editor.access_key.clone();
        let secret = self.screens.provider_editor.secret_key.clone();
        if !access.trim().is_empty() && !secret.is_empty() {
            let credentials = S3Credentials::for_provider(&provider_id);
            self.ask(
                Intent::Fire,
                Request::VaultSetSecret {
                    secret_ref: credentials.access_key_ref.clone(),
                    value: SecretString::from_string(access),
                },
            );
            self.ask(
                Intent::Fire,
                Request::VaultSetSecret {
                    secret_ref: credentials.secret_key_ref.clone(),
                    value: SecretString::from_string(secret),
                },
            );
        }
        self.go(Route::Providers);
    }
}
