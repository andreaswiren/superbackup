//! `R-1` … `R-5`. A backup tool is judged here.
//!
//! Source picker, snapshot list, a flat virtualised browser with breadcrumb
//! navigation (L4), the options modal, and the result — all of it replaced by
//! a single unlock panel while the vault is locked, because listing snapshots
//! needs the repository passphrase and a stale cached tree would be a lie.

use std::collections::HashMap;

use chrono::Utc;
use egui::{Align, Layout, Sense, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::ipc::protocol::{
    ConflictPolicy, EntryKind, ErrorPayload, ListingReply, SnapshotEntry, SnapshotInfo,
};
use superbackup_core::model::Destination;

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::data::{Action, Gate};
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::modals::{self, Modal, RestoreOptionsState};
use crate::gui::theme::{self, radius, size, space, Type};
use crate::gui::widgets::{self, Button};

#[derive(Default)]
pub struct State {
    pub selected_destination: Option<Uuid>,
    pub selected_snapshot: Option<String>,
    pub snapshots: HashMap<Uuid, Vec<SnapshotInfo>>,
    pub loading_snapshots: Option<Uuid>,
    pub listing: Option<ListingReply>,
    pub loading_listing: Option<String>,
    pub path: String,
    pub history: Vec<String>,
    pub filter: String,
    pub show_hidden: bool,
    pub selection: Vec<String>,
    pub error: Option<String>,
    pub result: Option<RestoreResult>,
}

#[derive(Debug, Clone)]
pub enum RestoreResult {
    Started,
}

impl State {
    pub fn select(&mut self, destination: Uuid) {
        self.selected_destination = Some(destination);
        self.selected_snapshot = None;
        self.listing = None;
        self.path.clear();
        self.selection.clear();
    }
    pub fn snapshots_requested(&mut self, destination: Uuid) {
        self.loading_snapshots = Some(destination);
        self.error = None;
    }
    pub fn snapshots_arrived(&mut self, destination: Uuid, snapshots: Vec<SnapshotInfo>) {
        self.loading_snapshots = None;
        self.snapshots.insert(destination, snapshots);
    }
    pub fn listing_requested(&mut self, path: String) {
        self.loading_listing = Some(path);
        self.error = None;
    }
    pub fn listing_arrived(&mut self, _destination: Uuid, path: String, listing: ListingReply) {
        self.loading_listing = None;
        self.path = path;
        self.listing = Some(listing);
    }
    pub fn restore_started(&mut self) {
        self.result = Some(RestoreResult::Started);
    }
    pub fn failed(&mut self, payload: ErrorPayload) {
        self.loading_snapshots = None;
        self.loading_listing = None;
        self.error = Some(payload.message);
    }
    pub fn busy(&self) -> bool {
        self.loading_snapshots.is_some() || self.loading_listing.is_some()
    }
    /// Selection survives navigation, so this is not cleared when a folder
    /// changes.
    pub fn toggle(&mut self, path: String) {
        if self.selection.contains(&path) {
            self.selection.retain(|p| p != &path);
        } else {
            self.selection.push(path);
        }
    }
    pub fn breadcrumb(&self) -> Vec<String> {
        self.path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect()
    }
}

impl App {
    pub(crate) fn restore_actions(&mut self, ui: &mut Ui) {
        if self.screens.restore.selected_snapshot.is_none() {
            return;
        }
        let count = self.screens.restore.selection.len();
        let mut restore = false;
        let label = copy::restore_browse_restore_n(count.max(1));
        let mut button = Button::primary(&label).icon(Icon::History).enabled(count > 0);
        if let Some(reason) = self.data.gate(Action::Restore).reason() {
            button = button.disabled_because(reason);
        }
        if button.show(ui).clicked() {
            restore = true;
        }
        let mut hidden = self.screens.restore.show_hidden;
        if widgets::toggle(ui, &mut hidden, copy::restore::BROWSE_HIDDEN, None, true).clicked() {
            self.screens.restore.show_hidden = hidden;
        }
        widgets::Field::new()
            .width(240.0)
            .placeholder(copy::restore::BROWSE_FILTER)
            .show(ui, &mut self.screens.restore.filter);

        if restore {
            self.open_restore_options();
        }
    }

    fn open_restore_options(&mut self) {
        let (Some(destination), Some(snapshot)) = (
            self.screens.restore.selected_destination,
            self.screens.restore.selected_snapshot.clone(),
        ) else {
            return;
        };
        let items = self.screens.restore.selection.clone();
        let bytes = self
            .screens
            .restore
            .listing
            .as_ref()
            .map(|listing| {
                listing
                    .entries
                    .iter()
                    .filter(|e| items.iter().any(|p| p.ends_with(&e.name)))
                    .map(|e| e.size_bytes)
                    .sum::<u64>()
            })
            .unwrap_or(0);
        let mut state = RestoreOptionsState::new(destination, snapshot, items, bytes);
        state.free_bytes =
            superbackup_core::platform::disk_space(std::path::Path::new(&state.target))
                .map(|(free, _)| free);
        self.open_modal(Modal::RestoreOptions(Box::new(state)));
    }

    pub(crate) fn show_restore(&mut self, ui: &mut Ui) {
        // Locked: the whole screen is one unlock panel. No snapshot metadata.
        if self.data.gate(Action::Restore) == Gate::NeedsUnlock {
            let t = theme::tokens(ui.ctx());
            let mut unlock = false;
            ui.allocate_ui_with_layout(
                ui.available_size(),
                Layout::top_down(Align::Center),
                |ui| {
                    ui.add_space((ui.available_height() * 0.35).max(0.0));
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                    Icon::Lock.paint(ui.painter(), rect, t.warning.mark);
                    ui.add_space(space::XL);
                    widgets::text(ui, copy::locked::RESTORE_TITLE, Type::H2, t.text_primary);
                    ui.add_space(space::M);
                    ui.allocate_ui_with_layout(
                        Vec2::new(420.0, 0.0),
                        Layout::top_down(Align::Center),
                        |ui| {
                            widgets::paragraph_at(
                                ui,
                                copy::locked::RESTORE_BODY,
                                Type::Body,
                                t.text_secondary,
                                420.0,
                            );
                        },
                    );
                    ui.add_space(space::XXL);
                    if Button::primary(copy::action::UNLOCK).onboarding().show(ui).clicked() {
                        unlock = true;
                    }
                },
            );
            if unlock {
                self.pending = Some(crate::gui::app::Pending::Restore);
                self.open_modal(Modal::Unlock(modals::UnlockState::blocking()));
            }
            return;
        }

        let repositories: Vec<Destination> =
            self.data.destinations.iter().filter(|d| d.kind.is_repository()).cloned().collect();
        let mirrors: Vec<Destination> =
            self.data.destinations.iter().filter(|d| !d.kind.is_repository()).cloned().collect();

        if repositories.is_empty() {
            let empty = if mirrors.is_empty() {
                &copy::empty::RESTORE_NO_DESTINATIONS
            } else {
                &copy::empty::RESTORE_MIRRORS_ONLY
            };
            let (primary, _) = widgets::empty_state(ui, Icon::History, empty, None);
            if primary {
                self.go(crate::gui::nav::Route::NewDestination);
            }
            return;
        }

        let narrow = ui.available_width() < 820.0;
        let pane = if narrow { 220.0 } else { 280.0 };
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(pane, ui.available_height()),
                Layout::top_down(Align::Min),
                |ui| {
                    self.restore_sources(ui, &repositories, &mirrors);
                },
            );
            ui.add_space(space::XL);
            widgets::vertical_rule(ui, ui.available_height());
            ui.add_space(space::XL);
            ui.vertical(|ui| {
                if self.screens.restore.selected_snapshot.is_some() {
                    self.restore_browser(ui);
                } else {
                    self.restore_snapshots(ui);
                }
            });
        });
    }

    fn restore_sources(
        &mut self,
        ui: &mut Ui,
        repositories: &[Destination],
        mirrors: &[Destination],
    ) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();
        widgets::text(ui, copy::restore::SOURCES, Type::H3, t.text_primary);
        ui.add_space(space::M);

        let mut select: Option<Uuid> = None;
        widgets::scroll_area(ui, "restore-sources", |ui| {
            for destination in repositories {
                let selected = self.screens.restore.selected_destination == Some(destination.id);
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.0), Sense::click());
                if selected {
                    ui.painter().rect_filled(rect, radius::CONTROL, t.bg_selected);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, radius::CONTROL, t.bg_surface_hover);
                }
                if response.has_focus() {
                    widgets::focus_ring(ui, rect.shrink(2.0), radius::CONTROL);
                }
                let icon_rect = egui::Rect::from_min_size(
                    egui::Pos2::new(rect.left() + 10.0, rect.center().y - 8.0),
                    Vec2::splat(16.0),
                );
                Icon::for_destination_kind(&destination.kind).paint(
                    ui.painter(),
                    icon_rect,
                    t.text_secondary,
                );
                let snapshots = self
                    .screens
                    .restore
                    .snapshots
                    .get(&destination.id)
                    .map(|s| s.len())
                    .unwrap_or(0);
                let name = widgets::galley(ui, &destination.name, Type::BodyStrong, t.text_primary);
                // Three states, not one. A destination nobody has opened has
                // not been asked about, and saying "Loading…" about it is a
                // lie that never resolves — which is exactly what every
                // unselected destination did, for ever. A destination that
                // genuinely holds nothing gets its own answer too, rather than
                // looking like one that is still working.
                let asked = self.screens.restore.snapshots.contains_key(&destination.id);
                let in_flight = self.screens.restore.loading_snapshots == Some(destination.id);
                let sub_text = if snapshots > 0 {
                    let newest = self
                        .screens
                        .restore
                        .snapshots
                        .get(&destination.id)
                        .and_then(|s| s.first())
                        .map(|s| copy::restore_newest(&format::relative_past(s.created_at, now)))
                        .unwrap_or_default();
                    format!("{} · {}", copy::restore_snapshot_count(snapshots), newest)
                } else if in_flight {
                    copy::state::LOADING.to_string()
                } else if asked {
                    copy::restore::NO_SNAPSHOTS.to_string()
                } else {
                    copy::restore::NOT_LOOKED.to_string()
                };
                let sub = widgets::galley(ui, sub_text, Type::Small, t.text_muted);
                let name_h = name.size().y;
                ui.painter().galley(
                    egui::Pos2::new(rect.left() + 36.0, rect.top() + 8.0),
                    name,
                    t.text_primary,
                );
                ui.painter().galley(
                    egui::Pos2::new(rect.left() + 36.0, rect.top() + 8.0 + name_h),
                    sub,
                    t.text_muted,
                );
                let announce = format!("{}, {}", destination.name, destination.kind.label());
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        true,
                        selected,
                        &announce,
                    )
                });
                if response.clicked() {
                    select = Some(destination.id);
                }
                ui.add_space(space::XS);
            }

            if !mirrors.is_empty() {
                ui.add_space(space::XL);
                widgets::text(ui, copy::restore::MIRRORS_GROUP, Type::H3, t.text_primary);
                ui.add_space(space::XS);
                widgets::paragraph_at(
                    ui,
                    copy::restore::MIRRORS_NOTE,
                    Type::Small,
                    t.text_muted,
                    260.0,
                );
                ui.add_space(space::M);
                for mirror in mirrors {
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        Icon::FolderSync.paint(ui.painter(), rect, t.text_muted);
                        ui.add_space(space::M);
                        widgets::elided(
                            ui,
                            &mirror.name,
                            Type::Small,
                            t.text_secondary,
                            120.0,
                            false,
                        );
                        if Button::ghost(copy::action::OPEN_FOLDER).compact().show(ui).clicked() {
                            if let Some(path) = mirror.kind.local_path() {
                                let _ = open::that_detached(path);
                            }
                        }
                    });
                }
            }
        });

        if let Some(id) = select {
            self.screens.restore.select(id);
            self.request_snapshots(id);
        }
    }

    fn restore_snapshots(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();
        let Some(destination) = self.screens.restore.selected_destination else {
            widgets::paragraph(ui, "Choose where to restore from.", Type::Body, t.text_secondary);
            return;
        };

        if let Some(message) = self.screens.restore.error.clone() {
            let mut verify = false;
            widgets::banner(ui, widgets::BannerKind::Danger, &message, None, |ui| {
                if Button::secondary(copy::action::VERIFY).compact().show(ui).clicked() {
                    verify = true;
                }
            });
            if verify {
                self.request_verify(destination);
            }
            return;
        }

        let snapshots =
            self.screens.restore.snapshots.get(&destination).cloned().unwrap_or_default();
        if self.screens.restore.loading_snapshots == Some(destination) {
            for _ in 0..6 {
                skeleton_row(ui);
            }
            return;
        }
        if snapshots.is_empty() {
            let (primary, _) =
                widgets::empty_state(ui, Icon::History, &copy::empty::RESTORE_NO_SNAPSHOTS, None);
            if primary {
                self.request_run_all();
            }
            return;
        }

        let retention =
            self.data.destination(&destination).map(|d| d.retention.clone()).unwrap_or_default();
        widgets::text(
            ui,
            copy::restore_retention_note(
                retention.keep_latest,
                retention.keep_hourly,
                retention.keep_daily,
                retention.keep_weekly,
                retention.keep_monthly,
                retention.keep_annual,
            ),
            Type::Small,
            t.text_muted,
        );
        ui.add_space(space::L);

        let mut open: Option<String> = None;
        let repository_path = self
            .data
            .destination(&destination)
            .map(crate::gui::viewmodel::destination_location)
            .unwrap_or_default();
        widgets::table_frame(ui, |ui| {
            let gap = ui.spacing().item_spacing.x;
            // The repository path shares what is left with the source, because
            // both are paths and either can be long. The browse button is
            // fixed: an affordance that changes width is one people stop
            // aiming at.
            const BROWSE_W: f32 = 44.0;
            let flexible =
                (ui.available_width() - 150.0 - 90.0 - 90.0 - 120.0 - BROWSE_W - gap * 7.0)
                    .max(220.0);
            let source_width = (flexible * 0.45).max(120.0);
            let repo_width = (flexible - source_width).max(100.0);
            egui_extras::TableBuilder::new(ui)
                .id_salt("restore-snapshots")
                // Rows are clickable, and a table senses `hover` unless it is
                // told otherwise — so `row.response().clicked()` was false on
                // every row of every table in the application, and every list
                // that opens something by being clicked did nothing at all.
                .sense(egui::Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(egui_extras::Column::exact(150.0))
                .column(egui_extras::Column::exact(source_width))
                .column(egui_extras::Column::exact(repo_width))
                .column(egui_extras::Column::exact(90.0))
                .column(egui_extras::Column::exact(90.0))
                .column(egui_extras::Column::exact(120.0))
                .column(egui_extras::Column::exact(BROWSE_W))
                .header(size::TABLE_HEADER_H, |mut header| {
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::WHEN, Some(true));
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, "Source", None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::REPOSITORY, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::FILES, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::SIZE, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::ID, None);
                    });
                    header.col(|_ui| {});
                })
                .body(|body| {
                    // Virtualised: thousands of snapshots cost the same as ten.
                    body.rows(size::TABLE_ROW_H, snapshots.len(), |mut row| {
                        let index = row.index();
                        let Some(snapshot) = snapshots.get(index) else {
                            return;
                        };
                        row.col(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                widgets::text(
                                    ui,
                                    format::absolute(snapshot.created_at),
                                    Type::MonoSmall,
                                    t.text_primary,
                                );
                                widgets::text(
                                    ui,
                                    format::relative_past(snapshot.created_at, now),
                                    Type::Small,
                                    t.text_muted,
                                );
                            });
                        });
                        row.col(|ui| {
                            let width = ui.available_width();
                            widgets::elided(
                                ui,
                                &snapshot.source_path,
                                Type::MonoSmall,
                                t.text_secondary,
                                width,
                                false,
                            );
                        });
                        row.col(|ui| {
                            // Where this snapshot physically lives. Constant
                            // for one destination, but it is the answer to
                            // "where is my backup actually stored" and the
                            // rows are where people look for it.
                            let width = ui.available_width();
                            widgets::elided(
                                ui,
                                &repository_path,
                                Type::MonoSmall,
                                t.text_muted,
                                width,
                                // Elide from the left: the tail of a path is
                                // what distinguishes it.
                                true,
                            )
                            .on_hover_text(repository_path.clone());
                        });
                        row.col(|ui| {
                            widgets::numeric_cell(
                                ui,
                                &snapshot
                                    .file_count
                                    .map(format::count)
                                    .unwrap_or_else(|| "—".into()),
                            );
                        });
                        row.col(|ui| {
                            widgets::numeric_cell(
                                ui,
                                &snapshot
                                    .total_bytes
                                    .map(format::bytes)
                                    .unwrap_or_else(|| "—".into()),
                            );
                        });
                        row.col(|ui| {
                            widgets::text(
                                ui,
                                format::short_snapshot(&snapshot.id),
                                Type::MonoSmall,
                                t.text_muted,
                            )
                            .on_hover_text(snapshot.id.clone());
                        });
                        row.col(|ui| {
                            // A visible way in. The whole row is clickable
                            // too, but a row that opens something with no
                            // affordance on it is a row nobody clicks.
                            if widgets::icon_button_compact(
                                ui,
                                Icon::FolderOpen,
                                copy::restore::BROWSE_HINT,
                                true,
                            )
                            .clicked()
                            {
                                open = Some(snapshot.id.clone());
                            }
                        });
                        if row.response().clicked() {
                            open = Some(snapshot.id.clone());
                        }
                    });
                });
        });

        if let Some(id) = open {
            self.screens.restore.selected_snapshot = Some(id.clone());
            self.screens.restore.path = String::new();
            self.request_browse(destination, id, String::new());
        }
    }

    fn restore_browser(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let (Some(destination), Some(snapshot)) = (
            self.screens.restore.selected_destination,
            self.screens.restore.selected_snapshot.clone(),
        ) else {
            return;
        };

        // The path breadcrumb.
        let segments = self.screens.restore.breadcrumb();
        let mut navigate: Option<String> = None;
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            if widgets::icon_button_compact(ui, Icon::ArrowLeft, copy::action::BACK, true).clicked()
            {
                if let Some(previous) = self.screens.restore.history.pop() {
                    navigate = Some(previous);
                }
            }
            if widgets::link(ui, "Home").clicked() {
                navigate = Some(String::new());
            }
            let mut accumulated = String::new();
            for segment in &segments {
                widgets::text(ui, "/", Type::Small, t.text_muted);
                accumulated.push('/');
                accumulated.push_str(segment);
                if widgets::link(ui, segment).clicked() {
                    navigate = Some(accumulated.trim_start_matches('/').to_string());
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let snapshots =
                    self.screens.restore.snapshots.get(&destination).cloned().unwrap_or_default();
                let labels: Vec<String> =
                    snapshots.iter().map(|s| format::absolute(s.created_at)).collect();
                let mut index = snapshots.iter().position(|s| s.id == snapshot).unwrap_or(0);
                if !labels.is_empty()
                    && widgets::combo(ui, "restore-snapshot", &mut index, &labels, 180.0, true)
                {
                    if let Some(chosen) = snapshots.get(index) {
                        self.screens.restore.selected_snapshot = Some(chosen.id.clone());
                        navigate = Some(self.screens.restore.path.clone());
                    }
                }
                widgets::text(ui, copy::restore::BROWSE_SNAPSHOT, Type::Small, t.text_muted);
            });
        });
        ui.add_space(space::L);

        // The selection strip.
        if !self.screens.restore.selection.is_empty() {
            let count = self.screens.restore.selection.len();
            let mut clear = false;
            egui::Frame::new()
                .fill(t.bg_raised)
                .corner_radius(radius::CONTROL)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        widgets::text(
                            ui,
                            copy::restore_browse_selected(count, 0),
                            Type::Small,
                            t.text_secondary,
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::link(ui, copy::restore::BROWSE_CLEAR).clicked() {
                                clear = true;
                            }
                        });
                    });
                });
            ui.add_space(space::L);
            if clear {
                self.screens.restore.selection.clear();
            }
        }

        if self.screens.restore.loading_listing.is_some() {
            for _ in 0..10 {
                skeleton_row(ui);
            }
            return;
        }

        let listing = self.screens.restore.listing.clone();
        let Some(listing) = listing else {
            widgets::text(ui, copy::restore::BROWSE_READING, Type::Small, t.text_muted);
            return;
        };

        let needle = self.screens.restore.filter.trim().to_lowercase();
        let entries: Vec<SnapshotEntry> = listing
            .entries
            .iter()
            .filter(|e| self.screens.restore.show_hidden || !e.name.starts_with('.'))
            .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
            .cloned()
            .collect();

        if entries.is_empty() {
            widgets::table_frame(ui, |ui| {
                ui.set_min_height(120.0);
                widgets::empty_state(ui, Icon::FolderOpen, &copy::empty::SNAPSHOT_DIR, None);
            });
            return;
        }

        // Folders first, then files, both alphabetical.
        let mut sorted = entries;
        sorted.sort_by(|a, b| {
            let rank = |k: EntryKind| match k {
                EntryKind::Directory => 0,
                _ => 1,
            };
            rank(a.kind)
                .cmp(&rank(b.kind))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        let all_selected = sorted
            .iter()
            .all(|e| self.screens.restore.selection.contains(&entry_path(&listing.path, &e.name)));
        let any_selected = sorted
            .iter()
            .any(|e| self.screens.restore.selection.contains(&entry_path(&listing.path, &e.name)));

        let mut toggle: Option<String> = None;
        let mut enter: Option<String> = None;
        let mut select_all = false;

        widgets::table_frame(ui, |ui| {
            let gap = ui.spacing().item_spacing.x;
            let name_width = (ui.available_width() - 32.0 - 90.0 - 130.0 - gap * 4.0).max(180.0);
            egui_extras::TableBuilder::new(ui)
                .id_salt("restore-listing")
                // Rows are clickable, and a table senses `hover` unless it is
                // told otherwise — so `row.response().clicked()` was false on
                // every row of every table in the application, and every list
                // that opens something by being clicked did nothing at all.
                .sense(egui::Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(egui_extras::Column::exact(32.0))
                .column(egui_extras::Column::exact(name_width))
                .column(egui_extras::Column::exact(90.0))
                .column(egui_extras::Column::exact(130.0))
                .header(size::TABLE_HEADER_H, |mut header| {
                    header.col(|ui| {
                        let state = if all_selected {
                            Some(true)
                        } else if any_selected {
                            None
                        } else {
                            Some(false)
                        };
                        if widgets::tri_checkbox(ui, state, "Select everything here", true)
                            .clicked()
                        {
                            select_all = true;
                        }
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::NAME, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::SIZE, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::MODIFIED, None);
                    });
                })
                .body(|body| {
                    // 28px rows, virtualised: a 400,000-entry directory costs
                    // the same as a ten-entry one (L4).
                    body.rows(size::TABLE_ROW_H_COMPACT, sorted.len(), |mut row| {
                        let index = row.index();
                        let Some(entry) = sorted.get(index) else {
                            return;
                        };
                        let path = entry_path(&listing.path, &entry.name);
                        let selected = self.screens.restore.selection.contains(&path);
                        row.set_selected(selected);
                        row.col(|ui| {
                            let mut on = selected;
                            if widgets::checkbox(ui, &mut on, "", None, true).clicked() {
                                toggle = Some(path.clone());
                            }
                        });
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                let (rect, _) =
                                    ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                                let icon = match entry.kind {
                                    EntryKind::Directory => Icon::Folder,
                                    EntryKind::Symlink => Icon::ExternalLink,
                                    EntryKind::File => Icon::FileText,
                                };
                                icon.paint(ui.painter(), rect, t.text_muted);
                                ui.add_space(space::M);
                                let width = ui.available_width();
                                let response = widgets::elided(
                                    ui,
                                    &entry.name,
                                    Type::Small,
                                    t.text_primary,
                                    width,
                                    false,
                                );
                                if entry.kind == EntryKind::Directory
                                    && response.interact(Sense::click()).clicked()
                                {
                                    enter = Some(path.clone());
                                }
                            });
                        });
                        row.col(|ui| {
                            let text = match entry.kind {
                                EntryKind::Directory => "—".to_string(),
                                _ => format::bytes(entry.size_bytes),
                            };
                            widgets::numeric_cell(ui, &text);
                        });
                        row.col(|ui| {
                            let text = entry
                                .modified_at
                                .map(format::absolute)
                                .unwrap_or_else(|| "—".to_string());
                            widgets::text(ui, text, Type::MonoSmall, t.text_muted);
                        });

                        let response = row.response();
                        let announce = format!(
                            "{}, {}, {}",
                            entry.name,
                            match entry.kind {
                                EntryKind::Directory => "folder",
                                EntryKind::Symlink => "link",
                                EntryKind::File => "file",
                            },
                            format::bytes(entry.size_bytes)
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &announce)
                        });
                        if response.double_clicked() && entry.kind == EntryKind::Directory {
                            enter = Some(path.clone());
                        }
                    });
                });
        });

        if listing.truncated {
            ui.add_space(space::M);
            widgets::text(
                ui,
                "This directory was too large to list in full.",
                Type::Small,
                t.warning.tint_text,
            );
        }

        if select_all {
            if all_selected {
                for entry in &sorted {
                    let path = entry_path(&listing.path, &entry.name);
                    self.screens.restore.selection.retain(|p| p != &path);
                }
            } else {
                for entry in &sorted {
                    let path = entry_path(&listing.path, &entry.name);
                    if !self.screens.restore.selection.contains(&path) {
                        self.screens.restore.selection.push(path);
                    }
                }
            }
        }
        if let Some(path) = toggle {
            self.screens.restore.toggle(path);
        }
        if let Some(path) = enter {
            let current = self.screens.restore.path.clone();
            self.screens.restore.history.push(current);
            self.request_browse(destination, snapshot.clone(), path);
        } else if let Some(path) = navigate {
            self.request_browse(destination, snapshot, path);
        }
    }
}

fn entry_path(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), name)
    }
}

/// A 28px skeleton bar, so the layout does not jump when data lands.
fn skeleton_row(ui: &mut Ui) {
    let t = theme::tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * 0.6, 12.0)),
        egui::CornerRadius::same(4),
        t.bg_raised,
    );
    ui.add_space(space::M);
}

/// `R-4` and `R-5`: the options modal, which converts in place into the
/// progress view rather than opening a second modal (L13).
pub fn show_options(
    app: &mut App,
    ctx: &egui::Context,
    mut state: RestoreOptionsState,
) -> Option<RestoreOptionsState> {
    let t = theme::tokens(ctx);
    let mut start = false;
    let mut cancel = false;

    let (close, _) = widgets::modal(
        ctx,
        "sb-restore-options",
        widgets::ModalSize::Medium,
        &copy::restore_options_title(state.items.len()),
        Some((Icon::History, t.accent)),
        state.running,
        |m| {
            m.body(|ui| {
                if state.running {
                    widgets::progress_bar(
                        ui,
                        ui.available_width(),
                        8.0,
                        None,
                        t.progress_fill,
                        &copy::a11y_progress_restore(0, 0, 0),
                    );
                    ui.add_space(space::M);
                    widgets::text(ui, copy::state::ESTIMATING, Type::MonoSmall, t.text_secondary);
                    return;
                }

                widgets::text(
                    ui,
                    copy::restore_options_what(
                        state.items.len(),
                        state.estimated_bytes,
                        &state.snapshot,
                    ),
                    Type::Body,
                    t.text_secondary,
                );
                ui.add_space(space::M);
                widgets::code_block(ui, &state.items.join("\n"), 120.0, None);

                ui.add_space(space::XL);
                widgets::text(ui, copy::restore::OPTIONS_WHERE, Type::H3, t.text_primary);
                ui.add_space(space::M);
                if widgets::radio(
                    ui,
                    state.to_original,
                    copy::restore::OPTIONS_ORIGINAL,
                    None,
                    true,
                )
                .clicked()
                {
                    state.to_original = true;
                    // Restoring over the original location makes the conflict
                    // question mandatory: there is no safe default.
                    state.conflict = None;
                }
                if widgets::radio(
                    ui,
                    !state.to_original,
                    copy::restore::OPTIONS_ELSEWHERE,
                    None,
                    true,
                )
                .clicked()
                {
                    state.to_original = false;
                    state.conflict = Some(ConflictPolicy::Skip);
                }
                if !state.to_original {
                    ui.horizontal_top(|ui| {
                        ui.add_space(28.0);
                        widgets::Field::new().width(340.0).mono().show(ui, &mut state.target);
                        if Button::secondary(copy::action::BROWSE).show(ui).clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                state.target = path.to_string_lossy().into_owned();
                            }
                        }
                    });
                    ui.add_space(space::M);
                    ui.horizontal(|ui| {
                        ui.add_space(28.0);
                        let mut structure = state.recreate_structure;
                        let helper = (!structure).then_some(copy::restore::OPTIONS_FLAT_WARN);
                        if widgets::checkbox(
                            ui,
                            &mut structure,
                            copy::restore::OPTIONS_STRUCTURE,
                            helper,
                            true,
                        )
                        .clicked()
                        {
                            state.recreate_structure = structure;
                        }
                    });
                }

                ui.add_space(space::XL);
                widgets::text(ui, copy::restore::OPTIONS_CONFLICT, Type::H3, t.text_primary);
                ui.add_space(space::M);
                for (policy, label, helper, danger) in [
                    (
                        ConflictPolicy::Skip,
                        copy::restore::OPTIONS_SKIP,
                        copy::restore::OPTIONS_SKIP_BODY,
                        false,
                    ),
                    (
                        ConflictPolicy::Overwrite,
                        copy::restore::OPTIONS_OVERWRITE,
                        copy::restore::OPTIONS_OVERWRITE_BODY,
                        true,
                    ),
                    (
                        ConflictPolicy::KeepBoth,
                        copy::restore::OPTIONS_KEEP_BOTH,
                        copy::restore::OPTIONS_KEEP_BOTH_BODY,
                        false,
                    ),
                ] {
                    if widgets::radio(ui, state.conflict == Some(policy), label, Some(helper), true)
                        .clicked()
                    {
                        state.conflict = Some(policy);
                    }
                    let _ = danger;
                    ui.add_space(space::XS);
                }

                ui.add_space(space::XL);
                widgets::text(ui, copy::restore::OPTIONS_ALSO, Type::H3, t.text_primary);
                ui.add_space(space::M);
                let mut timestamps = state.timestamps;
                if widgets::checkbox(
                    ui,
                    &mut timestamps,
                    copy::restore::OPTIONS_TIMESTAMPS,
                    None,
                    true,
                )
                .clicked()
                {
                    state.timestamps = timestamps;
                }
                ui.add_space(space::S);
                let mut permissions = state.permissions;
                // Disabled with an explanation on Windows rather than offered and
                // silently ignored.
                let windows = cfg!(windows);
                widgets::checkbox(
                    ui,
                    &mut permissions,
                    copy::restore::OPTIONS_PERMISSIONS,
                    windows.then_some(copy::restore::OPTIONS_PERMS_WINDOWS),
                    !windows,
                );
                if !windows {
                    state.permissions = permissions;
                }

                if let Some(free) = state.free_bytes {
                    ui.add_space(space::XL);
                    if state.estimated_bytes > free {
                        widgets::text(
                            ui,
                            copy::restore_options_not_enough(state.estimated_bytes, free),
                            Type::Small,
                            t.danger.tint_text,
                        );
                    } else {
                        widgets::text(
                            ui,
                            copy::restore_options_free_space(free),
                            Type::Small,
                            t.text_muted,
                        );
                    }
                }

                if state.needs_typed_confirmation() {
                    ui.add_space(space::XL);
                    widgets::Field::new()
                        .label(copy::restore::OPTIONS_TYPE_CONFIRM)
                        .width(200.0)
                        .show(ui, &mut state.typed_overwrite);
                }
            });
            m.footer(|ui| {
                if state.running {
                    if Button::danger_ghost(copy::restore::PROGRESS_CANCEL).show(ui).clicked() {
                        cancel = true;
                    }
                    return;
                }
                let danger = state.conflict == Some(ConflictPolicy::Overwrite);
                let label = if danger {
                    copy::restore::OPTIONS_BUTTON_DANGER
                } else {
                    copy::restore::OPTIONS_BUTTON
                };
                let button = if danger { Button::danger(label) } else { Button::primary(label) };
                if button.enabled(state.can_restore()).show(ui).clicked() {
                    start = true;
                }
                if Button::ghost(copy::action::CANCEL).show(ui).clicked() {
                    cancel = true;
                }
            });
        },
    );

    if start {
        let target = std::path::PathBuf::from(state.target.clone());
        let conflict = state.conflict.unwrap_or(ConflictPolicy::Skip);
        let path = state.items.first().cloned().unwrap_or_default();
        app.request_restore(state.destination, state.snapshot.clone(), path, target, conflict);
        state.running = true;
        return Some(state);
    }
    if cancel || close {
        return None;
    }
    Some(state)
}
