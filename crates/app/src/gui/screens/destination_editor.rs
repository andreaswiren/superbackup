//! `T-2` and `T-4`. One editor over four kinds, plus repository creation and
//! the full encryption panel.

use chrono::Utc;
use egui::{Align, Layout, Ui};
use uuid::Uuid;

use superbackup_core::ipc::protocol::{ErrorPayload, KeyCheckReply, RepositoryReply, Request};
use superbackup_core::model::{
    default_s3_prefix, normalise_prefix, Destination, DestinationKind, EccAlgorithm,
    EncryptionAlgorithm, EncryptionSettings, HashAlgorithm, PassphraseSource, RetentionPolicy,
    Splitter,
};
use superbackup_core::platform::identity::MachineRecord;

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

/// What the bucket picker can offer right now.
///
/// The picker is an *accelerator over* the text field, never a replacement for
/// it, so every state here has to leave typing available. `Unavailable` is the
/// ordinary case, not the exceptional one: an offline laptop, a locked vault,
/// a key scoped to one bucket, or a provider the user has not saved yet all
/// land there, and none of them may stop a destination being created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BucketPicker {
    /// Nothing asked for yet.
    Idle,
    Loading,
    /// Names to choose from. May legitimately be empty — an account with no
    /// buckets is a real answer, not a failure.
    Ready(Vec<String>),
    /// Could not be produced, and why. Shown next to the field, never in place
    /// of it.
    Unavailable(String),
}

/// What is already stored under the chosen bucket and prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixCheck {
    Loading,
    /// The prefix already holds a kopia repository.
    Repository,
    /// Objects are there, but no repository.
    Occupied,
    Empty,
    Unavailable(String),
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
    /// The picker's state, keyed by provider so switching provider does not
    /// show one account's buckets under another's name.
    pub buckets: Option<(Uuid, BucketPicker)>,
    /// The prefix check, keyed by provider for the same reason.
    pub prefix_check: Option<(Uuid, PrefixCheck)>,
    pub access_key: String,
    pub secret_key: String,
    pub revealed: bool,
    pub own_credentials: bool,
    /// Validation is inline and on blur: a form nobody has filled in yet is not
    /// a form full of mistakes. Set once the user tries to save.
    pub show_errors: bool,
    pub pin_offline: bool,
    pub mirror_prune: bool,
    /// The destination whose encryption key is being checked right now.
    pub key_check_running: Option<Uuid>,
    /// The last check's outcome, keyed by destination so a stale reply for a
    /// destination the user has navigated away from cannot be shown against a
    /// different one.
    pub key_check: Option<(Uuid, KeyCheckOutcome)>,
    /// The machines that have left a record at this destination, read from the
    /// destination's own folder. `None` until it has been looked for.
    pub machines: Option<(Uuid, Result<Vec<MachineRecord>, String>)>,
    /// Google Drive for Desktop mounts found on this machine, detected once
    /// per editor session. `None` until it has been looked for.
    pub gdrive: Option<Vec<superbackup_core::platform::GoogleDriveAccount>>,
    /// Set when the user chose "New storage provider…" from this editor, so
    /// the provider they create is selected here when they come back rather
    /// than leaving them to find their way to it.
    pub awaiting_new_provider: bool,
}

/// What a `dest.check_key` said, reduced to the three answers that differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCheckOutcome {
    /// The repository opened.
    Opened,
    /// The location is reachable but holds no repository, so the key could not
    /// be checked against anything. Deliberately not "wrong key": telling a
    /// user their key is bad when nothing tried it would send them hunting.
    NoRepository,
    /// The repository refused the key, or the check could not be made.
    Refused(String),
}

impl State {
    pub fn key_check_started(&mut self, destination: Uuid) {
        self.key_check_running = Some(destination);
        self.key_check = None;
    }
    pub fn key_check_arrived(&mut self, destination: Uuid, reply: KeyCheckReply) {
        self.key_check_running = None;
        let outcome = if reply.valid {
            KeyCheckOutcome::Opened
        } else if reply.no_repository {
            KeyCheckOutcome::NoRepository
        } else {
            KeyCheckOutcome::Refused(
                reply.detail.unwrap_or_else(|| copy::keys::CHECK_BAD.to_string()),
            )
        };
        self.key_check = Some((destination, outcome));
    }
    pub fn key_check_failed(&mut self, destination: Uuid, payload: ErrorPayload) {
        self.key_check_running = None;
        self.key_check = Some((destination, KeyCheckOutcome::Refused(payload.message)));
    }

    /// The outcome to render for `destination`, if the last one was about it.
    pub fn key_check_for(&self, destination: Uuid) -> Option<&KeyCheckOutcome> {
        self.key_check.as_ref().filter(|(id, _)| *id == destination).map(|(_, o)| o)
    }

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
        matches!(self.creation, Some(CreationPhase::Running(_))) || self.key_check_running.is_some()
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
                    replicate_from: None,
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
        self.buckets = None;
        self.prefix_check = None;
    }

    // -- the bucket picker ---------------------------------------------------
    //
    // Kept as plain view-model methods, separate from any rendering, so the
    // states can be driven and asserted in a test without laying out a frame.

    pub fn buckets_requested(&mut self, provider: Uuid) {
        self.buckets = Some((provider, BucketPicker::Loading));
    }

    pub fn buckets_arrived(
        &mut self,
        provider: Uuid,
        reply: &superbackup_core::ipc::protocol::BucketsReply,
    ) {
        let state = if reply.listed {
            BucketPicker::Ready(reply.buckets.iter().map(|b| b.name.clone()).collect())
        } else {
            // Includes the qualified case — credentials proven, listing
            // refused — which is exactly when the manual field matters most.
            BucketPicker::Unavailable(
                reply.detail.clone().unwrap_or_else(|| copy::prov::ERR_NO_LIST.to_string()),
            )
        };
        self.buckets = Some((provider, state));
    }

    pub fn buckets_unavailable(&mut self, provider: Uuid, detail: String) {
        self.buckets = Some((provider, BucketPicker::Unavailable(detail)));
    }

    /// The picker's state for `provider`, or `Idle` when it belongs to another.
    pub fn picker(&self, provider: Uuid) -> BucketPicker {
        match &self.buckets {
            Some((id, state)) if *id == provider => state.clone(),
            _ => BucketPicker::Idle,
        }
    }

    pub fn objects_requested(&mut self, provider: Uuid) {
        self.prefix_check = Some((provider, PrefixCheck::Loading));
    }

    pub fn objects_arrived(
        &mut self,
        provider: Uuid,
        reply: &superbackup_core::ipc::protocol::ObjectsReply,
    ) {
        let state = match (reply.listed, reply.holds_repository, reply.keys.is_empty()) {
            (false, _, _) => PrefixCheck::Unavailable(
                reply.detail.clone().unwrap_or_else(|| copy::dest::S3_PREFIX_UNKNOWN.to_string()),
            ),
            (true, true, _) => PrefixCheck::Repository,
            (true, false, true) => PrefixCheck::Empty,
            (true, false, false) => PrefixCheck::Occupied,
        };
        self.prefix_check = Some((provider, state));
    }

    pub fn objects_unavailable(&mut self, provider: Uuid, detail: String) {
        self.prefix_check = Some((provider, PrefixCheck::Unavailable(detail)));
    }

    pub fn prefix_state(&self, provider: Uuid) -> Option<PrefixCheck> {
        match &self.prefix_check {
            Some((id, state)) if *id == provider => Some(state.clone()),
            _ => None,
        }
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
                self.replication_panel(ui, &report);
            }

            let is_replica = self
                .screens
                .destination_editor
                .draft
                .as_ref()
                .is_some_and(|d| d.replicate_from.is_some());

            if is_repository && !is_replica {
                self.encryption_panel(ui, existing.as_ref());
                widgets::form_group(ui, copy::job::RETENTION_TITLE, None);
                if let Some(draft) = &mut self.screens.destination_editor.draft {
                    crate::gui::screens::retention_editor(ui, &mut draft.retention);
                }
            } else if is_replica {
                // Not a disabled encryption panel: a greyed-out algorithm
                // picker still implies there is a separate key behind it.
                // Retention is the source's too — the replica holds the
                // source's manifests, so expiring a snapshot here would be
                // undone by the next copy.
                widgets::form_group(ui, copy::chain::ENCRYPTION_INHERITED, None);
                widgets::paragraph(
                    ui,
                    copy::chain::ENCRYPTION_INHERITED_BODY,
                    Type::Small,
                    theme::tokens(ui.ctx()).text_muted,
                );
            }

            widgets::form_group(ui, copy::job::BANDWIDTH_TITLE, None);
            if let Some(draft) = &mut self.screens.destination_editor.draft {
                let mut overriding = draft.bandwidth.is_some();
                if widgets::toggle(ui, &mut overriding, copy::job::BANDWIDTH_CUSTOM, None, true)
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

            // Only for a destination that exists: a draft has no folder to
            // read, and an empty card on the create form would suggest the
            // question had been asked and answered.
            if let Some(destination) = &existing {
                self.machines_panel(ui, destination);
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
                let others: Vec<Destination> =
                    self.data.destinations.iter().filter(|d| d.id != draft.id).cloned().collect();
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

        // Google Drive is offered here rather than as a kind of its own,
        // because that is what it is: a folder on this machine that Drive for
        // Desktop keeps in sync. kopia's own `gdrive` backend is marked
        // "[Not maintained]" upstream *and* authenticates as a service
        // account, whose files are owned by that account and count against a
        // quota the user does not have — so it would not use the storage they
        // pay for. See `platform::gdrive`.
        if !onedrive {
            self.gdrive_picker(ui);
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
                self.screens.destination_editor.path_input = path.to_string_lossy().into_owned();
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

    /// Offer any Google Drive for Desktop folder found on this machine.
    ///
    /// Detection is cheap and cached for the life of the editor: it walks a
    /// handful of documented paths and reads a volume label. When nothing is
    /// found the section is absent entirely — an empty "Google Drive" heading
    /// on a machine without it is noise.
    fn gdrive_picker(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let accounts = self
            .screens
            .destination_editor
            .gdrive
            .get_or_insert_with(superbackup_core::platform::gdrive::detect);
        if accounts.is_empty() {
            return;
        }
        let accounts = accounts.clone();

        let mut chosen: Option<String> = None;
        widgets::text(ui, copy::dest::GDRIVE_TITLE, Type::BodyStrong, t.text_primary);
        ui.add_space(space::XS);
        widgets::paragraph_at(ui, copy::dest::GDRIVE_BODY, Type::Small, t.text_muted, 560.0);
        ui.add_space(space::M);

        for account in &accounts {
            widgets::card(ui, |ui| {
                ui.set_width(ui.available_width().min(560.0));
                ui.horizontal(|ui| {
                    widgets::text(ui, &account.display_name, Type::BodyStrong, t.text_primary);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Button::secondary(copy::dest::GDRIVE_USE).compact().show(ui).clicked() {
                            chosen = Some(
                                account.suggested_repository_root().to_string_lossy().into_owned(),
                            );
                        }
                    });
                });
                ui.add_space(space::XS);
                widgets::text(
                    ui,
                    account.suggested_repository_root().to_string_lossy(),
                    Type::MonoSmall,
                    t.text_muted,
                );
                // Streaming is the one thing that makes a Drive folder a bad
                // place for a repository, and it is the default. Saying so
                // here, next to the button, is the whole point of detecting
                // the mode at all.
                for warning in &account.warnings {
                    ui.add_space(space::S);
                    widgets::banner(ui, widgets::BannerKind::Warning, warning, None, |_| {});
                }
            });
            ui.add_space(space::M);
        }

        if let Some(path) = chosen {
            self.screens.destination_editor.path_input = path;
        }
        ui.add_space(space::L);
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
        let mut index =
            current.and_then(|id| self.data.providers.iter().position(|p| p.id == id)).unwrap_or(0);
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
        let mut open_admin: Option<String> = None;
        match current.and_then(|id| self.data.provider(&id)) {
            Some(provider) => {
                let superbackup_core::model::ProviderKind::S3 {
                    endpoint,
                    region,
                    flavour,
                    admin_url,
                    ..
                } = &provider.kind;
                let line = format!("{endpoint} · {region} · {}", flavour.title());
                // The console link belongs to the account, so it is stored on
                // the provider — but the question "where do I log in to fix
                // this?" is asked from here, looking at a destination. A
                // destination knows its provider, so it can answer.
                let admin = admin_url
                    .as_deref()
                    .filter(|u| !u.trim().is_empty())
                    .filter(|u| superbackup_core::model::validate_admin_url(u).is_ok())
                    .map(str::to_string);
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
                                300.0,
                                false,
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::link(ui, copy::dest::S3_PROVIDER_EDIT).clicked() {
                                    new_provider = false;
                                }
                                if let Some(url) = &admin {
                                    ui.add_space(space::L);
                                    if widgets::link(ui, copy::dest::S3_ADMIN_OPEN).clicked() {
                                        open_admin = Some(url.clone());
                                    }
                                }
                                // Keeps the links off the endpoint text when
                                // the strip is only just wide enough for both.
                                ui.add_space(space::L);
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

        if new_provider {
            // Choosing "New storage provider…" used to set a flag that nothing
            // read, so the option simply did nothing. The draft survives the
            // trip — `State::load` only resets when the destination being
            // edited changes — so the editor is exactly as it was on return.
            self.screens.destination_editor.awaiting_new_provider = true;
            self.go(Route::NewProvider);
            return;
        }

        if let Some(url) = open_admin {
            // Documentation, opened in the user's own browser. Nothing in this
            // application ever connects to it itself.
            if let Err(e) = open::that_detached(&url) {
                self.toasts.warning(format!("That address could not be opened ({e})."));
            }
        }

        ui.add_space(space::XL);
        // The manual field comes first and is never disabled. The picker below
        // is an accelerator over it, not a gate in front of it — the user
        // asked for a list *and* said typing must remain an option, and the
        // only way to honour that is for the list to be the optional half.
        widgets::Field::new()
            .label(copy::dest::S3_BUCKET)
            .helper(copy::dest::S3_BUCKET_HELPER)
            .width(280.0)
            .error(report.for_field(Field::Bucket))
            .show(ui, &mut self.screens.destination_editor.bucket_input);
        ui.add_space(space::M);
        self.s3_bucket_picker(ui, current);

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

        self.s3_prefix_check(ui, current);

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

    /// The bucket picker.
    ///
    /// Everything here is additive to the text field above it. There is no
    /// state in which the picker takes the field's place, none in which a
    /// failed listing disables anything, and none in which the user has to
    /// wait for a network round trip before typing a name they already know.
    /// That is the design constraint: the user asked to choose from a list and
    /// said "entering them should also be an option though", so the *list* is
    /// the optional half.
    fn s3_bucket_picker(&mut self, ui: &mut Ui, provider: Option<Uuid>) {
        let t = theme::tokens(ui.ctx());
        let Some(provider_id) = provider else {
            return;
        };
        // A provider that is not in the configuration yet cannot be asked
        // anything, and saying so is more use than a button that fails.
        if self.data.provider(&provider_id).is_none() {
            widgets::text(ui, copy::dest::S3_BUCKET_UNSAVED, Type::Small, t.text_muted);
            return;
        }

        let mut chosen: Option<String> = None;
        let mut fetch = false;
        match self.screens.destination_editor.picker(provider_id) {
            BucketPicker::Idle => {
                if Button::ghost(copy::dest::S3_BUCKET_CHOOSE)
                    .icon(Icon::List)
                    .compact()
                    .show(ui)
                    .clicked()
                {
                    fetch = true;
                }
            }
            BucketPicker::Loading => {
                ui.horizontal(|ui| {
                    widgets::spinner(ui, 14.0, t.text_muted);
                    ui.add_space(space::S);
                    widgets::text(ui, copy::dest::S3_BUCKET_LISTING, Type::Small, t.text_muted);
                });
            }
            BucketPicker::Ready(names) if names.is_empty() => {
                ui.horizontal(|ui| {
                    widgets::text(ui, copy::prov::TEST_OK_NONE, Type::Small, t.text_muted);
                    ui.add_space(space::M);
                    if widgets::link(ui, copy::dest::S3_BUCKET_RETRY).clicked() {
                        fetch = true;
                    }
                });
            }
            BucketPicker::Ready(names) => {
                // "Type a name instead" is the first entry rather than a mode
                // switch, so leaving the list is one click and never a dead
                // end.
                let mut options: Vec<String> = vec![copy::dest::S3_BUCKET_TYPE.to_string()];
                options.extend(names.iter().cloned());
                let typed = self.screens.destination_editor.bucket_input.clone();
                let mut index = names.iter().position(|n| *n == typed).map_or(0, |i| i + 1);
                ui.horizontal(|ui| {
                    if widgets::combo(ui, "dest-bucket-pick", &mut index, &options, 280.0, true)
                        && index > 0
                    {
                        chosen = names.get(index - 1).cloned();
                    }
                    ui.add_space(space::M);
                    if widgets::link(ui, copy::dest::S3_BUCKET_RETRY).clicked() {
                        fetch = true;
                    }
                });
            }
            // The important state. Offline, a locked vault, or a key that may
            // not enumerate buckets all arrive here, and none of them is an
            // error the user has to clear: the field above still works.
            BucketPicker::Unavailable(detail) => {
                widgets::paragraph_at(ui, &detail, Type::Small, t.text_muted, 520.0);
                ui.add_space(space::S);
                if widgets::link(ui, copy::dest::S3_BUCKET_RETRY).clicked() {
                    fetch = true;
                }
            }
        }
        if let Some(name) = chosen {
            self.screens.destination_editor.bucket_input = name;
        }
        if fetch {
            self.request_bucket_list(provider_id);
        }
    }

    /// What is already stored where this destination would write.
    ///
    /// Answers a question the user would otherwise discover only by pressing
    /// "Create repository" and being told the prefix is taken.
    fn s3_prefix_check(&mut self, ui: &mut Ui, provider: Option<Uuid>) {
        let t = theme::tokens(ui.ctx());
        let Some(provider_id) = provider else {
            return;
        };
        if self.data.provider(&provider_id).is_none() {
            return;
        }
        let bucket = self.screens.destination_editor.bucket_input.trim().to_string();
        if bucket.is_empty() {
            return;
        }
        let prefix = self.screens.destination_editor.prefix_input.clone();

        ui.add_space(space::M);
        let mut check = false;
        match self.screens.destination_editor.prefix_state(provider_id) {
            None => {
                if widgets::link(ui, copy::dest::S3_PREFIX_CHECK).clicked() {
                    check = true;
                }
            }
            Some(PrefixCheck::Loading) => {
                ui.horizontal(|ui| {
                    widgets::spinner(ui, 14.0, t.text_muted);
                    ui.add_space(space::S);
                    widgets::text(ui, copy::dest::S3_BUCKET_LISTING, Type::Small, t.text_muted);
                });
            }
            Some(state) => {
                let (kind, message) = match state {
                    PrefixCheck::Repository => {
                        (widgets::BannerKind::Info, copy::dest::S3_PREFIX_HAS_REPO.to_string())
                    }
                    PrefixCheck::Occupied => {
                        (widgets::BannerKind::Warning, copy::dest::S3_PREFIX_OCCUPIED.to_string())
                    }
                    PrefixCheck::Empty => {
                        (widgets::BannerKind::Success, copy::dest::S3_PREFIX_EMPTY.to_string())
                    }
                    PrefixCheck::Unavailable(detail) => (widgets::BannerKind::Warning, detail),
                    // Handled above; a `match` arm rather than an unreachable
                    // so a new state cannot silently fall through.
                    PrefixCheck::Loading => (widgets::BannerKind::Info, String::new()),
                };
                widgets::banner(ui, kind, &message, None, |ui| {
                    if Button::ghost(copy::dest::S3_PREFIX_CHECK).compact().show(ui).clicked() {
                        check = true;
                    }
                });
            }
        }
        if check {
            self.request_prefix_check(provider_id, bucket, prefix);
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

    /// "Check the stored key", and what it said.
    ///
    /// The interface cannot show a repository encryption key — the daemon has
    /// no request that returns one, by design — so the useful question it
    /// *can* answer is whether the key it holds still opens the repository.
    /// That is not a format check: the daemon opens the repository with it.
    /// Where this destination's contents come from: the job's folders, or an
    /// existing repository at another destination.
    ///
    /// This is the interface to `Destination::replicate_from`, and the reason
    /// it is a panel of its own rather than a checkbox is the consequence it
    /// carries. `kopia repository sync-to` copies the source's *format blob*,
    /// which is where the repository's identity and key parameters live. The
    /// result is not a second repository that happens to hold the same
    /// snapshots; it is the same repository, in a second place, opened with the
    /// same passphrase. There is no configuration in which the offsite copy is
    /// independently keyed, and a user who believes there is will keep one
    /// passphrase, lose the other, and find out at restore time.
    ///
    /// So the panel states that where it cannot be missed, and the encryption
    /// panel is removed rather than disabled — a greyed-out algorithm picker
    /// would still suggest there is a separate key behind it.
    fn replication_panel(&mut self, ui: &mut Ui, report: &validation::Report) {
        let t = theme::tokens(ui.ctx());
        let Some(draft) = &self.screens.destination_editor.draft else { return };
        let self_id = draft.id;
        let current = draft.replicate_from;

        // Anything that is a repository, is not this destination, and does not
        // already sit downstream of it. The last exclusion is what keeps the
        // picker from offering a choice the validator would immediately
        // reject — an unselectable option is a better explanation than an
        // error message about a loop.
        let candidates: Vec<(Uuid, String)> = self
            .data
            .destinations
            .iter()
            .filter(|d| {
                d.id != self_id && d.kind.is_repository() && !self.feeds_from(d.id, self_id)
            })
            .map(|d| (d.id, d.name.clone()))
            .collect();

        widgets::form_group(ui, copy::chain::TITLE, None);

        // One opt-in, not a pair of alternatives. "From the job's folders" is
        // what a destination does; it does not need to be chosen.
        let mut choose: Option<Option<Uuid>> = None;
        let mut is_copy = current.is_some();
        if widgets::toggle(
            ui,
            &mut is_copy,
            copy::chain::FROM_DESTINATION,
            Some(copy::chain::FROM_DESTINATION_HELP),
            !candidates.is_empty(),
        )
        .clicked()
        {
            choose = Some(if is_copy { candidates.first().map(|(id, _)| *id) } else { None });
        }

        if current.is_some() {
            ui.add_space(space::M);
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                ui.vertical(|ui| {
                    let names: Vec<String> =
                        candidates.iter().map(|(_, name)| name.clone()).collect();
                    let mut index = current
                        .and_then(|id| candidates.iter().position(|(c, _)| *c == id))
                        .unwrap_or(0);
                    if widgets::combo_labelled(
                        ui,
                        "dest-replicate-from",
                        Some(copy::chain::PICK_LABEL),
                        &mut index,
                        &names,
                        400.0,
                        true,
                    ) {
                        if let Some((id, _)) = candidates.get(index) {
                            choose = Some(Some(*id));
                        }
                    }
                    if let Some(message) = report.for_field(Field::ReplicateFrom) {
                        ui.add_space(space::XS);
                        widgets::text(ui, message, Type::Small, t.danger.tint_text);
                    }
                });
            });

            ui.add_space(space::L);
            widgets::banner(
                ui,
                widgets::BannerKind::Warning,
                copy::chain::SHARED_KEY_TITLE,
                Some(copy::chain::SHARED_KEY_BODY),
                |_| {},
            );
        }

        if let Some(value) = choose {
            if let Some(draft) = &mut self.screens.destination_editor.draft {
                draft.replicate_from = value;
                if value.is_some() {
                    // The settings are the source's, and keeping a stale local
                    // copy of them would show the user numbers that are not
                    // what the repository actually uses. The passphrase handle
                    // goes for the same reason: there is no separate key here
                    // to point at.
                    draft.encryption = None;
                    draft.passphrase_ref = None;
                } else if draft.encryption.is_none() {
                    draft.encryption = Some(EncryptionSettings::default());
                }
            }
        }
    }

    /// Does `candidate` already draw, directly or through a chain, from
    /// `root`? Used to keep the picker from offering a loop.
    ///
    /// Bounded by the same depth limit the core validator uses, so a config
    /// that already contains a cycle — one edited by hand, or pulled from a
    /// Git repository — makes this return rather than spin.
    fn feeds_from(&self, candidate: Uuid, root: Uuid) -> bool {
        let mut current = candidate;
        for _ in 0..=superbackup_core::model::MAX_REPLICATION_DEPTH {
            let Some(destination) = self.data.destination(&current) else { return false };
            match destination.replicate_from {
                Some(parent) if parent == root => return true,
                Some(parent) => current = parent,
                None => return false,
            }
        }
        // Deeper than the limit: treat it as unavailable rather than walking
        // further, which is the safe answer for a picker.
        true
    }

    fn key_check_controls(&mut self, ui: &mut Ui, destination: &Destination) {
        let t = theme::tokens(ui.ctx());
        let id = destination.id;
        let running = self.screens.destination_editor.key_check_running == Some(id);
        let mut check = false;

        ui.horizontal(|ui| {
            let mut button = Button::secondary(copy::keys::CHECK_STORED).icon(Icon::Shield);
            if running {
                button = button.busy(true);
            }
            if self.data.gate(Action::VerifyDestination) == Gate::NeedsUnlock {
                button = button.disabled_because(copy::locked::ACTION_BLOCKED);
            }
            if button.show(ui).clicked() {
                check = true;
            }
            if running {
                ui.add_space(space::M);
                widgets::text(ui, copy::keys::CHECKING, Type::Small, t.text_secondary);
            }
        });
        ui.add_space(space::XS);
        widgets::paragraph_at(ui, copy::keys::CHECK_NOTE, Type::Small, t.text_muted, 560.0);

        match self.screens.destination_editor.key_check_for(id) {
            Some(KeyCheckOutcome::Opened) => {
                ui.add_space(space::M);
                widgets::banner(
                    ui,
                    widgets::BannerKind::Success,
                    copy::keys::CHECK_OK,
                    None,
                    |_| {},
                );
            }
            Some(KeyCheckOutcome::NoRepository) => {
                ui.add_space(space::M);
                widgets::banner(
                    ui,
                    widgets::BannerKind::Info,
                    copy::keys::CHECK_NONE,
                    None,
                    |_| {},
                );
            }
            Some(KeyCheckOutcome::Refused(detail)) => {
                ui.add_space(space::M);
                let detail = detail.clone();
                widgets::banner(
                    ui,
                    widgets::BannerKind::Danger,
                    copy::keys::CHECK_BAD,
                    Some(&detail),
                    |_| {},
                );
            }
            None => {}
        }

        if check {
            // `None`: check the key the vault already holds, which is the
            // question "is my backup still openable?".
            self.request_check_key(id, None);
        }
    }

    /// Which computers have left a record at this destination.
    ///
    /// Read here rather than over IPC, and that is a deliberate limit worth
    /// knowing: the window reads the destination's own folder with the *user's*
    /// rights. A destination the daemon can reach but this session cannot —
    /// a mapped drive under a service account — reports that it could not be
    /// read, rather than claiming no computer has ever backed up there.
    fn machines_panel(&mut self, ui: &mut Ui, destination: &Destination) {
        let t = theme::tokens(ui.ctx());
        let id = destination.id;
        widgets::form_group(ui, copy::machines::TITLE, None);

        if !matches!(
            destination.kind,
            DestinationKind::LocalRepository { .. }
                | DestinationKind::OneDrive { .. }
                | DestinationKind::LocalMirror { .. }
        ) {
            widgets::paragraph_at(
                ui,
                copy::machines::UNSUPPORTED,
                Type::Small,
                t.text_muted,
                560.0,
            );
            ui.add_space(space::XS);
            widgets::paragraph_at(ui, copy::machines::SETTING_S3, Type::Small, t.text_muted, 560.0);
            return;
        }

        // Read once per destination and cached: this is a small directory
        // listing, but it is still blocking I/O and an immediate-mode
        // interface would otherwise do it sixty times a second.
        let stale = self
            .screens
            .destination_editor
            .machines
            .as_ref()
            .map(|(cached, _)| *cached != id)
            .unwrap_or(true);
        if stale {
            let result = superbackup_core::platform::identity::list_machines_for(destination)
                .map_err(|_| copy::machines::UNREADABLE.to_string());
            self.screens.destination_editor.machines = Some((id, result));
        }

        let mut refresh = false;
        match self.screens.destination_editor.machines.as_ref().map(|(_, r)| r) {
            Some(Err(message)) => {
                let message = message.clone();
                widgets::paragraph_at(ui, &message, Type::Small, t.text_muted, 560.0);
            }
            Some(Ok(machines)) if machines.is_empty() => {
                widgets::empty_state(ui, Icon::HardDrive, &copy::empty::MACHINES, None);
            }
            Some(Ok(machines)) => {
                let me = self.data.snapshot.as_ref().map(|s| s.machine_slug.clone());
                for machine in machines {
                    widgets::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            widgets::text(ui, &machine.label, Type::BodyStrong, t.text_primary);
                            if me.as_deref() == Some(machine.slug.as_str()) {
                                ui.add_space(space::M);
                                widgets::neutral_badge(ui, copy::machines::THIS_PC, None);
                            }
                        });
                        ui.add_space(space::XS);
                        widgets::text(ui, &machine.slug, Type::MonoSmall, t.text_secondary);
                        ui.add_space(space::XS);
                        widgets::text(
                            ui,
                            copy::machines_last_seen(&crate::gui::format::relative_past(
                                machine.last_seen,
                                Utc::now(),
                            )),
                            Type::Small,
                            t.text_muted,
                        );
                    });
                    ui.add_space(space::S);
                }
            }
            None => {}
        }
        ui.add_space(space::M);
        if Button::secondary(copy::machines::REFRESH).compact().show(ui).clicked() {
            refresh = true;
        }
        if refresh {
            self.screens.destination_editor.machines = None;
        }
    }

    // -- T-4: the encryption panel -----------------------------------------

    fn encryption_panel(&mut self, ui: &mut Ui, existing: Option<&Destination>) {
        let t = theme::tokens(ui.ctx());
        // A repository that exists already has fixed settings; the panel is
        // replaced by a read-only summary that says why.
        // Whether the repository actually exists — NOT whether a passphrase
        // handle has been assigned.
        //
        // `passphrase_ref` is set when the destination is *added*; the key
        // behind it and the repository itself are created later. Treating the
        // handle as proof of a repository meant a destination that had never
        // been set up rendered the "already configured" view and hid the
        // "Create repository" button entirely — leaving no way anywhere in the
        // application to create one.
        //
        // `last_verified_at` is the closest signal the client has until the
        // probe reports repository presence directly. Being wrong in this
        // direction is safe: offering the button when a repository already
        // exists costs one clear "a repository already exists here" from kopia,
        // whereas hiding it strands the user.
        let connected = existing
            .map(|d| d.passphrase_ref.is_some() && d.last_verified_at.is_some())
            .unwrap_or(false);
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
                ui.add_space(space::XL);
                self.key_check_controls(ui, destination);
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
                            widgets::text(ui, copy::enc::RECOMMENDED, Type::SmallStrong, t.accent);
                        });
                    }
                    ui.add_space(space::S);
                }

                ui.add_space(space::XL);
                widgets::text(ui, copy::enc::HASH, Type::H3, t.text_primary);
                ui.add_space(space::S);
                let hashes: Vec<String> =
                    HashAlgorithm::all().iter().map(|h| h.kopia_id().to_string()).collect();
                let mut hash_index =
                    HashAlgorithm::all().iter().position(|h| *h == settings.hash).unwrap_or(0);
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
                widgets::paragraph_at(
                    ui,
                    copy::enc::SPLITTER_BODY,
                    Type::Small,
                    t.text_muted,
                    520.0,
                );
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
            create =
                create.disabled_because("Save this destination before creating its repository.");
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
            let mut button = Button::primary(copy::action::SAVE_CHANGES).enabled(report.ok());
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
                    && Button::danger_ghost(copy::action::REMOVE)
                        .icon(Icon::Trash)
                        .show(ui)
                        .clicked()
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
                // A replica's repository is made by the first backup that
                // copies into it; creating one here would make a different
                // repository that could never be synchronised. So the offer is
                // only for a destination that owns its repository.
                let offer_repository = existing.is_none()
                    && draft.kind.is_repository()
                    && draft.replicate_from.is_none();
                let request = if existing.is_some() {
                    Request::DestinationUpdate { destination: Box::new(draft) }
                } else {
                    Request::DestinationCreate { destination: Box::new(draft) }
                };
                let intent = if offer_repository {
                    Intent::CreateDestination(name)
                } else {
                    Intent::SaveDestination(name)
                };
                self.ask(intent, request);
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
