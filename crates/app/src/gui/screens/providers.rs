//! `P-1`. The provider table. The whole screen exists to make reuse visible,
//! so that rotating one key is understood as touching many buckets.

use chrono::Utc;
use egui::{Align, Layout, Sense, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::model::{ProviderKind, StorageProvider};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::data::Action;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::modals::{self, Modal};
use crate::gui::nav::Route;
use crate::gui::theme::{self, size, space, Type};
use crate::gui::widgets::{self, Button};

#[derive(Default)]
pub struct State {
    pub search: String,
}

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

        let needle = self.screens.providers.search.trim().to_lowercase();
        let rows: Vec<StorageProvider> = self
            .data
            .providers
            .iter()
            .filter(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle))
            .cloned()
            .collect();

        let narrow = ui.available_width() < 840.0;
        let mut open: Option<Uuid> = None;
        let mut test: Option<Uuid> = None;
        let mut menu: Option<(&'static str, Uuid)> = None;

        widgets::table_frame(ui, |ui| {
            let mut builder = egui_extras::TableBuilder::new(ui)
                .id_salt("providers")
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(egui_extras::Column::exact(36.0))
                .column(egui_extras::Column::exact(220.0))
                .column(egui_extras::Column::remainder().at_least(200.0))
                .column(egui_extras::Column::exact(130.0));
            if !narrow {
                builder = builder.column(egui_extras::Column::exact(120.0));
            }
            builder = builder.column(egui_extras::Column::exact(120.0));

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
                    if !narrow {
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

                        row.col(|ui| {
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                            Icon::Database.paint(ui.painter(), rect, t.text_secondary);
                            response.on_hover_text(flavour.title());
                        });
                        row.col(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                widgets::elided(
                                    ui,
                                    &provider.name,
                                    Type::BodyStrong,
                                    t.text_primary,
                                    206.0,
                                    false,
                                );
                                if !provider.notes.is_empty() {
                                    widgets::elided(
                                        ui,
                                        &provider.notes,
                                        Type::Small,
                                        t.text_muted,
                                        206.0,
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
                        if !narrow {
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
                                        if widgets::menu_item_danger(
                                            ui,
                                            copy::action::DELETE,
                                            true,
                                        ) {
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
                        let announce =
                            format!("{}, {endpoint}, {region}", provider.name);
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &announce)
                        });
                        if response.clicked() {
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
