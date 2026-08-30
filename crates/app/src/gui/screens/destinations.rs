//! `T-1`. The destination table, and the verify flow's state.

use std::collections::HashMap;

use chrono::Utc;
use egui::{Align, Layout, Sense, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::ipc::protocol::{ErrorPayload, ProbeReply};
use superbackup_core::model::{Destination, DestinationKind};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::data::Action;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::modals::{self, Modal};
use crate::gui::nav::Route;
use crate::gui::screens::job_editor::destination_location;
use crate::gui::theme::{self, size, space, Type};
use crate::gui::viewmodel::{self, DestinationStatus};
use crate::gui::widgets::{self, Button};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeState {
    Running,
    Ok,
    Failed(String),
}

#[derive(Default)]
pub struct State {
    pub search: String,
    /// `None` = every kind.
    pub kind_filter: Option<usize>,
    probes: HashMap<Uuid, ProbeState>,
}

impl State {
    pub fn probe_started(&mut self, id: Uuid) {
        self.probes.insert(id, ProbeState::Running);
    }
    pub fn probe(&mut self, id: Uuid, probe: ProbeReply) {
        let state = if probe.reachable && probe.writable {
            ProbeState::Ok
        } else {
            ProbeState::Failed(modals::probe_message(&probe))
        };
        self.probes.insert(id, state);
    }
    pub fn probe_failed(&mut self, id: Uuid, payload: ErrorPayload) {
        self.probes.insert(id, ProbeState::Failed(payload.message));
    }
    pub fn failed_probe(&self, id: Uuid) -> bool {
        matches!(self.probes.get(&id), Some(ProbeState::Failed(_)))
    }
    pub fn probing(&self, id: Uuid) -> bool {
        matches!(self.probes.get(&id), Some(ProbeState::Running))
    }
    pub fn probe_message(&self, id: Uuid) -> Option<&str> {
        match self.probes.get(&id) {
            Some(ProbeState::Failed(message)) => Some(message),
            _ => None,
        }
    }
    pub fn busy(&self) -> bool {
        self.probes.values().any(|p| *p == ProbeState::Running)
    }
}

const KIND_FILTERS: [&str; 5] = [
    "All",
    copy::dest::STATUS_READY,
    copy::dest::STATUS_NOT_CONNECTED,
    copy::dest::STATUS_UNREACHABLE,
    copy::badge::DISABLED,
];

impl App {
    pub(crate) fn destinations_actions(&mut self, ui: &mut Ui) {
        let mut new_destination = false;
        if Button::primary(copy::dest::NEW).icon(Icon::Plus).show(ui).clicked() {
            new_destination = true;
        }
        let options: Vec<String> = KIND_FILTERS.iter().map(|s| s.to_string()).collect();
        let mut index = self.screens.destinations.kind_filter.unwrap_or(0);
        if widgets::combo(ui, "dest-filter", &mut index, &options, 160.0, true) {
            self.screens.destinations.kind_filter = (index > 0).then_some(index);
        }
        widgets::Field::new()
            .width(240.0)
            .placeholder(copy::dest::SEARCH)
            .show(ui, &mut self.screens.destinations.search);
        if new_destination {
            self.go(Route::NewDestination);
        }
    }

    pub(crate) fn show_destinations(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();

        if self.data.destinations.is_empty() && !self.data.loading {
            let (primary, _) =
                widgets::empty_state(ui, Icon::HardDrive, &copy::empty::DESTINATIONS, None);
            if primary {
                self.go(Route::NewDestination);
            }
            return;
        }

        let needle = self.screens.destinations.search.trim().to_lowercase();
        let filter = self.screens.destinations.kind_filter;
        let rows: Vec<Destination> = self
            .data
            .destinations
            .iter()
            .filter(|d| {
                needle.is_empty()
                    || d.name.to_lowercase().contains(&needle)
                    || destination_location(&self.data, d).to_lowercase().contains(&needle)
            })
            .filter(|d| {
                let status =
                    viewmodel::destination_status(d, self.screens.destinations.failed_probe(d.id));
                match filter {
                    None => true,
                    Some(1) => status == DestinationStatus::Ready,
                    Some(2) => status == DestinationStatus::NotConnected,
                    Some(3) => status == DestinationStatus::Unreachable,
                    Some(4) => status == DestinationStatus::Disabled,
                    _ => true,
                }
            })
            .cloned()
            .collect();

        let narrow = ui.available_width() < 840.0;
        let mut open: Option<Uuid> = None;
        let mut verify: Option<Uuid> = None;
        let mut menu: Option<(&'static str, Uuid)> = None;

        widgets::table_frame(ui, |ui| {
            let mut builder = egui_extras::TableBuilder::new(ui)
                .id_salt("destinations")
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(egui_extras::Column::exact(36.0))
                .column(egui_extras::Column::exact(200.0))
                .column(egui_extras::Column::remainder().at_least(200.0));
            if !narrow {
                builder = builder.column(egui_extras::Column::exact(90.0));
                builder = builder.column(egui_extras::Column::exact(120.0));
            }
            builder = builder
                .column(egui_extras::Column::exact(100.0))
                .column(egui_extras::Column::exact(120.0));

            builder
                .header(size::TABLE_HEADER_H, |mut header| {
                    header.col(|ui| {
                        widgets::table_header(ui, "", None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::NAME, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::LOCATION, None);
                    });
                    if !narrow {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::col::USED_BY, None);
                        });
                        header.col(|ui| {
                            widgets::table_header(ui, copy::col::LAST_VERIFIED, None);
                        });
                    }
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::STATUS, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, "", None);
                    });
                })
                .body(|body| {
                    body.rows(44.0, rows.len(), |mut row| {
                        let index = row.index();
                        let Some(destination) = rows.get(index) else {
                            return;
                        };
                        let failed = self.screens.destinations.failed_probe(destination.id);
                        let status = viewmodel::destination_status(destination, failed);
                        let dim = if destination.enabled { 1.0 } else { 0.6 };

                        row.col(|ui| {
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                            Icon::for_destination_kind(&destination.kind).paint(
                                ui.painter(),
                                rect,
                                theme::alpha(t.text_secondary, dim),
                            );
                            response.on_hover_text(destination.kind.label());
                        });
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                widgets::elided(
                                    ui,
                                    &destination.name,
                                    Type::BodyStrong,
                                    theme::alpha(t.text_primary, dim),
                                    if destination.auto_discovered { 160.0 } else { 186.0 },
                                    false,
                                );
                                if destination.auto_discovered {
                                    let (rect, response) =
                                        ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                                    Icon::Sparkles.paint(ui.painter(), rect, t.text_muted);
                                    response.on_hover_text(copy::dest::AUTO_FOUND);
                                }
                            });
                        });
                        row.col(|ui| {
                            let location = destination_location(&self.data, destination);
                            let width = ui.available_width();
                            widgets::elided(
                                ui,
                                &location,
                                Type::MonoSmall,
                                t.text_muted,
                                width,
                                false,
                            );
                        });
                        if !narrow {
                            row.col(|ui| {
                                let jobs = self.data.jobs_using(&destination.id);
                                let response = widgets::count_pill(
                                    ui,
                                    &copy::dest_used_by(jobs.len()),
                                );
                                if !jobs.is_empty() {
                                    let names: Vec<&str> =
                                        jobs.iter().map(|j| j.name.as_str()).collect();
                                    response.on_hover_text(names.join("\n"));
                                }
                            });
                            row.col(|ui| match destination.last_verified_at {
                                Some(at) => {
                                    widgets::text(
                                        ui,
                                        format::relative_past(at, now),
                                        Type::Small,
                                        t.text_secondary,
                                    );
                                }
                                None => {
                                    widgets::text(
                                        ui,
                                        copy::state::NEVER,
                                        Type::Small,
                                        t.warning.tint_text,
                                    );
                                }
                            });
                        }
                        row.col(|ui| {
                            let (palette, icon) = match status {
                                DestinationStatus::Ready => (t.success, Icon::CheckCircle),
                                DestinationStatus::NotConnected => {
                                    (t.neutral, Icon::MinusCircle)
                                }
                                DestinationStatus::Unreachable => (t.danger, Icon::XOctagon),
                                DestinationStatus::Disabled => (t.neutral, Icon::Pause),
                            };
                            widgets::badge(ui, palette, Some(icon), status.title());
                        });
                        row.col(|ui| {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                widgets::overflow_menu(
                                    ui,
                                    ("dest-row", destination.id),
                                    "More actions for this destination",
                                    |ui| {
                                        if widgets::menu_item(ui, copy::action::VERIFY_NOW, true) {
                                            menu = Some(("verify", destination.id));
                                        }
                                        if widgets::menu_item(ui, "Browse snapshots…", true) {
                                            menu = Some(("browse", destination.id));
                                        }
                                        if widgets::menu_item(ui, copy::action::EDIT, true) {
                                            menu = Some(("edit", destination.id));
                                        }
                                        if widgets::menu_item(
                                            ui,
                                            if destination.enabled {
                                                copy::action::DISABLE
                                            } else {
                                                copy::action::ENABLE
                                            },
                                            true,
                                        ) {
                                            menu = Some(("toggle", destination.id));
                                        }
                                        widgets::divider(ui);
                                        if widgets::menu_item_danger(
                                            ui,
                                            copy::action::REMOVE,
                                            true,
                                        ) {
                                            menu = Some(("remove", destination.id));
                                        }
                                    },
                                );
                                let gate = self.data.gate(Action::VerifyDestination);
                                let mut button = Button::ghost(copy::action::VERIFY)
                                    .compact()
                                    .busy(self.screens.destinations.probing(destination.id))
                                    .a11y(format!("Verify destination \"{}\"", destination.name));
                                if let Some(reason) = gate.reason() {
                                    button = button.disabled_because(reason);
                                }
                                if button.show(ui).clicked() {
                                    verify = Some(destination.id);
                                }
                            });
                        });

                        let response = row.response();
                        let announce = format!(
                            "{}, {}, {}, {}",
                            destination.name,
                            destination.kind.label(),
                            status.title(),
                            destination_location(&self.data, destination)
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &announce)
                        });
                        if response.clicked() {
                            open = Some(destination.id);
                        }
                    });
                });
        });

        // A failed probe explains itself in place rather than only in a toast.
        for destination in &rows {
            if let Some(message) = self.screens.destinations.probe_message(destination.id) {
                ui.add_space(space::L);
                let title = format!("{}: {}", destination.name, message);
                widgets::banner(ui, widgets::BannerKind::Danger, &title, None, |_| {});
            }
        }

        if let Some(id) = verify {
            self.request_verify(id);
        }
        if let Some(id) = open {
            self.go(Route::DestinationEditor(id));
        }
        if let Some((action, id)) = menu {
            match action {
                "verify" => self.request_verify(id),
                "edit" => self.go(Route::DestinationEditor(id)),
                "browse" => {
                    self.screens.restore.select(id);
                    self.go(Route::Restore);
                }
                "toggle" => {
                    if let Some(mut destination) = self.data.destination(&id).cloned() {
                        destination.enabled = !destination.enabled;
                        self.ask(
                            crate::gui::daemon::Intent::SaveDestination(destination.name.clone()),
                            superbackup_core::ipc::protocol::Request::DestinationUpdate {
                                destination: Box::new(destination),
                            },
                        );
                    }
                }
                "remove" => {
                    let confirm = modals::remove_destination_confirm(&self.data, id);
                    self.open_modal(Modal::Confirm(confirm));
                }
                _ => {}
            }
        }
    }
}

/// The four destination kinds, with the trade-off line each one owes the user
/// at the point of choice. The mirror's line is the one that matters most:
/// nobody may believe a mirror is encrypted.
pub const KIND_CHOICES: [(&str, &str); 4] = [
    (
        copy::kind::LOCAL_REPOSITORY,
        "Fastest. Same building, so it does not survive a fire or a theft.",
    ),
    (
        copy::kind::ONEDRIVE,
        "Offsite through a folder you already sync. Limited by your OneDrive quota.",
    ),
    (copy::kind::S3, "Offsite and independent. Costs money and needs an account."),
    (
        copy::kind::MIRROR,
        "A plain readable copy. No history, no deduplication, no encryption.",
    ),
];

pub fn kind_icon(index: usize) -> Icon {
    match index {
        0 => Icon::HardDrive,
        1 => Icon::Cloud,
        2 => Icon::Database,
        _ => Icon::FolderSync,
    }
}

pub fn kind_for_index(index: usize, path: std::path::PathBuf, provider: Option<Uuid>) -> DestinationKind {
    match index {
        0 => DestinationKind::LocalRepository { path },
        1 => DestinationKind::OneDrive { path, account: None },
        2 => DestinationKind::S3 {
            provider_id: provider.unwrap_or_else(Uuid::nil),
            bucket: String::new(),
            prefix: String::new(),
            credential_override: None,
        },
        _ => DestinationKind::LocalMirror { path },
    }
}

pub fn index_for_kind(kind: &DestinationKind) -> usize {
    match kind {
        DestinationKind::LocalRepository { .. } => 0,
        DestinationKind::OneDrive { .. } => 1,
        DestinationKind::S3 { .. } => 2,
        DestinationKind::LocalMirror { .. } => 3,
    }
}
