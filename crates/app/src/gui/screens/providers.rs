//! `P-1`. The provider table. The whole screen exists to make reuse visible,
//! so that rotating one key is understood as touching many buckets.

use chrono::Utc;
use egui::{Align, Layout, Sense, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::model::{ProviderKind, StorageProvider};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::daemon::Intent;
use crate::gui::data::Action;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::modals::{self, Modal};
use crate::gui::nav::Route;
use crate::gui::theme::{self, size, space, Type};
use crate::gui::viewmodel::{self, ColumnSpec};
use crate::gui::widgets::{self, Button};
use superbackup_core::ipc::protocol::Request;

#[derive(Default)]
pub struct State {
    pub search: String,
    /// Destinations whose figures have already been asked for, so the page
    /// asks once rather than on every frame.
    pub stats_asked: std::collections::HashSet<uuid::Uuid>,
}

/// `UX_SPEC.md` §9.1. Endpoint is the remainder; last verified drops first.
///
/// The trailing actions column is listed even though it is never dropped:
/// `fit_columns` decides what fits by adding these widths up, and a column the
/// builder draws but the budget does not know about is unaccounted overflow.
/// This table happened to have enough slack to hide it; the activity table did
/// not, and drew its card past the window edge at the minimum size.
const PROVIDER_COLUMNS: [ColumnSpec; 7] = [
    ColumnSpec::keep("flavour", 36.0),
    ColumnSpec::keep("name", 190.0),
    ColumnSpec::keep("used_by", 110.0),
    // What the account is holding and when anything last arrived: the two
    // questions people actually have about object storage, and neither was
    // answerable anywhere in the application before.
    ColumnSpec::droppable("stored", 100.0, 3),
    ColumnSpec::droppable("last_write", 150.0, 2),
    ColumnSpec::droppable("verified", 104.0, 1),
    ColumnSpec::keep("actions", 104.0),
];

impl App {
    pub(crate) fn providers_actions(&mut self, ui: &mut Ui) {
        let mut new_provider = false;
        if Button::primary(copy::prov::NEW).icon(Icon::Plus).show(ui).clicked() {
            new_provider = true;
        }
        widgets::Field::new()
            .width(240.0)
            .placeholder(copy::prov::SEARCH)
            .show(ui, &mut self.screens.providers.search);
        if new_provider {
            self.go(Route::NewProvider);
        }
    }

    pub(crate) fn show_providers(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();

        if self.data.providers.is_empty() && !self.data.loading {
            let (primary, _) =
                widgets::empty_state(ui, Icon::KeyRound, &copy::empty::PROVIDERS, None);
            if primary {
                self.go(Route::NewProvider);
            }
            return;
        }

        // Ask for figures we do not have yet, once, for the destinations on
        // screen. Cached rather than recomputed (`refresh: false`): a repository
        // stat can be a slow call against remote storage, and one per
        // destination on every frame would make this page unusable on exactly
        // the setup it exists to describe.
        let missing: Vec<uuid::Uuid> = self
            .data
            .destinations
            .iter()
            .filter(|d| d.kind.provider_id().is_some())
            .map(|d| d.id)
            .filter(|id| !self.data.destination_stats.contains_key(id))
            .filter(|id| !self.screens.providers.stats_asked.contains(id))
            .collect();
        for id in missing {
            self.screens.providers.stats_asked.insert(id);
            self.ask(
                Intent::DestinationStats(id),
                Request::DestinationStats { destination: id.to_string(), refresh: false },
            );
        }

        let needle = self.screens.providers.search.trim().to_lowercase();
        let rows: Vec<StorageProvider> = self
            .data
            .providers
            .iter()
            .filter(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle))
            .cloned()
            .collect();

        let shown = viewmodel::fit_columns(
            // The table card insets its content, and this is measured before
            // that frame is entered, so the budget has to lose both sides or
            // the last column is pushed past the card edge.
            ui.available_width() - widgets::TABLE_GUTTER,
            200.0,
            ui.spacing().item_spacing.x,
            &PROVIDER_COLUMNS,
        );
        let has = |key: &str| shown.contains(&key);
        let mut open: Option<Uuid> = None;
        let mut test: Option<Uuid> = None;
        let mut menu: Option<(&'static str, Uuid)> = None;

        widgets::table_frame(ui, |ui| {
            let gap = ui.spacing().item_spacing.x;
            // Every fixed column that is actually shown, plus a gap between
            // each pair — computed rather than written out, because the two
            // hand-maintained lists (this sum and the builder below) drifted
            // apart the moment columns were added and the table ran off the
            // right edge of the window.
            let mut fixed = 36.0 + 190.0 + 110.0 + 104.0;
            let mut columns = 5.0; // the four above, plus the endpoint itself
            for (key, width) in [("stored", 100.0_f32), ("last_write", 150.0), ("verified", 104.0)]
            {
                if has(key) {
                    fixed += width;
                    columns += 1.0;
                }
            }
            let endpoint_width = (ui.available_width() - fixed - gap * (columns - 1.0)).max(160.0);
            let mut builder = egui_extras::TableBuilder::new(ui)
                .id_salt("providers")
                // Rows are clickable, and a table senses `hover` unless it is
                // told otherwise — so `row.response().clicked()` was false on
                // every row of every table in the application, and every list
                // that opens something by being clicked did nothing at all.
                .sense(egui::Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(egui_extras::Column::exact(36.0))
                .column(egui_extras::Column::exact(190.0))
                .column(egui_extras::Column::exact(endpoint_width))
                .column(egui_extras::Column::exact(110.0));
            if has("stored") {
                builder = builder.column(egui_extras::Column::exact(100.0));
            }
            if has("last_write") {
                builder = builder.column(egui_extras::Column::exact(150.0));
            }
            if has("verified") {
                builder = builder.column(egui_extras::Column::exact(104.0));
            }
            builder = builder.column(egui_extras::Column::exact(104.0));

            builder
                .header(size::TABLE_HEADER_H, |mut header| {
                    header.col(|ui| {
                        widgets::table_header(ui, "", None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::NAME, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::ENDPOINT, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::USED_BY, None);
                    });
                    if has("stored") {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::prov::COL_STORED, None);
                        });
                    }
                    if has("last_write") {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::prov::COL_LAST_WRITE, None);
                        });
                    }
                    if has("verified") {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::col::LAST_VERIFIED, None);
                        });
                    }
                    header.col(|ui| {
                        widgets::table_header(ui, "", None);
                    });
                })
                .body(|body| {
                    body.rows(44.0, rows.len(), |mut row| {
                        let index = row.index();
                        let Some(provider) = rows.get(index) else {
                            return;
                        };
                        let ProviderKind::S3 { endpoint, region, tls, flavour, .. } =
                            &provider.kind;
                        let usage = self.data.provider_usage(provider.id);

                        row.col(|ui| {
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                            Icon::Database.paint(ui.painter(), rect, t.text_secondary);
                            response.on_hover_text(flavour.title());
                        });
                        row.col(|ui| {
                            // Drawn straight into the cell, with no
                            // `with_layout` around it.
                            //
                            // The table already lays each cell out centred in
                            // the row's height, which is why every other
                            // column here lines up without doing anything.
                            // Wrapping the name in a layout of its own
                            // *replaced* that one with a fresh layout anchored
                            // at the cell's top edge, so the fix for "the name
                            // is too high" was itself what held it too high.
                            // A nested `vertical` is fine — the Last write
                            // column uses one for its two lines and centres
                            // correctly — because it is a block inside the
                            // cell's layout rather than a replacement for it.
                            let lines: &[Type] = if provider.notes.is_empty() {
                                &[Type::BodyStrong]
                            } else {
                                &[Type::BodyStrong, Type::Small]
                            };
                            widgets::stacked_cell(ui, lines, |ui| {
                                widgets::elided(
                                    ui,
                                    &provider.name,
                                    Type::BodyStrong,
                                    t.text_primary,
                                    176.0,
                                    false,
                                );
                                if !provider.notes.is_empty() {
                                    widgets::elided(
                                        ui,
                                        &provider.notes,
                                        Type::Small,
                                        t.text_muted,
                                        176.0,
                                        false,
                                    );
                                }
                            });
                        });
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                if !*tls {
                                    let (rect, response) =
                                        ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                                    Icon::Shield.paint(ui.painter(), rect, t.warning.mark);
                                    response.on_hover_text(copy::prov::NO_TLS);
                                }
                                let line = format!("{endpoint} · {region}");
                                let width = ui.available_width();
                                widgets::elided(
                                    ui,
                                    &line,
                                    Type::MonoSmall,
                                    t.text_muted,
                                    width,
                                    false,
                                );
                            });
                        });
                        row.col(|ui| {
                            let (inheriting, overriding) =
                                self.data.destinations_using(&provider.id);
                            let total = inheriting.len() + overriding.len();
                            if total == 0 {
                                widgets::text(
                                    ui,
                                    copy::prov::USED_BY_NONE,
                                    Type::Small,
                                    t.text_muted,
                                );
                            } else {
                                let names: Vec<&str> = inheriting
                                    .iter()
                                    .chain(overriding.iter())
                                    .map(|d| d.name.as_str())
                                    .collect();
                                widgets::count_pill(ui, &copy::prov_used_by(total))
                                    .on_hover_text(names.join("\n"));
                            }
                        });
                        if has("stored") {
                            row.col(|ui| {
                                // `None` is not zero: a figure that has not
                                // been reported yet must not read as an empty
                                // bucket, which is the one wrong impression
                                // this column could give.
                                match usage.stored_bytes {
                                    Some(bytes) => {
                                        let text = if usage.pending > 0 {
                                            format!("{}+", format::bytes(bytes))
                                        } else {
                                            format::bytes(bytes)
                                        };
                                        widgets::text(ui, text, Type::MonoSmall, t.text_secondary)
                                            .on_hover_text(copy::prov_stored_hint(usage.pending));
                                    }
                                    None => {
                                        widgets::muted_cell(ui, copy::prov::NOT_MEASURED);
                                    }
                                }
                            });
                        }
                        if has("last_write") {
                            row.col(|ui| match usage.last_write {
                                Some(at) => {
                                    widgets::stacked_cell(
                                        ui,
                                        &[Type::Small, Type::MonoSmall],
                                        |ui| {
                                            widgets::text(
                                                ui,
                                                format::relative_past(at, now),
                                                Type::Small,
                                                t.text_secondary,
                                            );
                                            widgets::text(
                                                ui,
                                                copy::prov_last_write(usage.last_write_bytes),
                                                Type::MonoSmall,
                                                t.text_muted,
                                            );
                                        },
                                    );
                                }
                                None => {
                                    widgets::muted_cell(ui, copy::prov::NEVER_WRITTEN);
                                }
                            });
                        }
                        if has("verified") {
                            row.col(|ui| match provider.last_verified_at {
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
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                widgets::overflow_menu(
                                    ui,
                                    ("provider-row", provider.id),
                                    "More actions for this provider",
                                    |ui| {
                                        if widgets::menu_item(
                                            ui,
                                            copy::action::TEST_CONNECTION,
                                            true,
                                        ) {
                                            menu = Some(("test", provider.id));
                                        }
                                        if widgets::menu_item(ui, copy::action::EDIT, true) {
                                            menu = Some(("edit", provider.id));
                                        }
                                        if widgets::menu_item(ui, "Rotate keys…", true) {
                                            menu = Some(("rotate", provider.id));
                                        }
                                        widgets::divider(ui);
                                        if widgets::menu_item_danger(ui, copy::action::DELETE, true)
                                        {
                                            menu = Some(("delete", provider.id));
                                        }
                                    },
                                );
                                let gate = self.data.gate(Action::TestProvider);
                                let mut button = Button::ghost("Test")
                                    .compact()
                                    .busy(self.screens.provider_editor.probing(provider.id))
                                    .a11y(format!("Test connection to \"{}\"", provider.name));
                                if let Some(reason) = gate.reason() {
                                    button = button.disabled_because(reason);
                                }
                                if button.show(ui).clicked() {
                                    test = Some(provider.id);
                                }
                            });
                        });

                        let response = row.response();
                        let announce = format!("{}, {endpoint}, {region}", provider.name);
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &announce)
                        });
                        // A control inside the row wins. The row senses clicks so it
                        // can open the item, but a click on a button inside it lands
                        // here too — so pressing Verify both verified *and* navigated
                        // away, and navigating is what the user saw.
                        if response.clicked() && menu.is_none() {
                            open = Some(provider.id);
                        }
                    });
                });
        });

        ui.add_space(space::L);
        widgets::paragraph_at(ui, copy::empty::PROVIDERS.body, Type::Small, t.text_muted, 560.0);

        if let Some(id) = test {
            self.request_test_provider(id);
        }
        if let Some(id) = open {
            self.go(Route::ProviderEditor(id));
        }
        match menu {
            Some(("test", id)) => self.request_test_provider(id),
            Some(("edit", id)) => self.go(Route::ProviderEditor(id)),
            Some(("rotate", id)) => {
                if self.data.gate(Action::RotateKeys).allowed() {
                    self.open_modal(Modal::Rotate(modals::RotateState::new(id)));
                } else {
                    self.open_modal(Modal::Unlock(modals::UnlockState::blocking()));
                }
            }
            Some(("delete", id)) => {
                let confirm = modals::delete_provider_confirm(&self.data, id);
                self.open_modal(Modal::Confirm(confirm));
            }
            _ => {}
        }
    }
}
