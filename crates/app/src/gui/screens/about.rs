//! `AB-1`. One centred 560px column. The Kopia attribution is mandatory and
//! appears here, in the onboarding welcome, and in the diagnostic bundle.

use egui::{Align, Layout, Sense, Ui, Vec2};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::icons::{self, Icon};
use crate::gui::theme::{self, space, Type};
use crate::gui::widgets::{self, Button};

const WEBSITE: &str = "https://github.com/andreaswiren/superbackup";
const DOCS: &str = "https://github.com/andreaswiren/superbackup/tree/main/docs";
const ISSUES: &str = "https://github.com/andreaswiren/superbackup/issues/new";
const KOPIA_DOCS: &str = "https://kopia.io/docs/";
const RELEASES: &str = "https://github.com/andreaswiren/superbackup/releases";

impl App {
    pub(crate) fn show_about(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let info = superbackup_core::build_info();
        let mut open: Option<String> = None;

        widgets::scroll_area(ui, "about", |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 0.0),
                Layout::top_down(Align::Center),
                |ui| {
                    ui.add_space(space::H2);
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
                    icons::health_mark(
                        ui.painter(),
                        rect,
                        superbackup_core::state::Health::Idle,
                        t.accent,
                        None,
                        0.0,
                    );
                    ui.add_space(space::XL);
                    widgets::text(ui, copy::APP_NAME, Type::Display, t.text_primary);
                    ui.add_space(space::XS);
                    widgets::text(
                        ui,
                        copy::about_version(&info.version),
                        Type::Body,
                        t.text_muted,
                    );
                    ui.add_space(space::XXS);
                    widgets::text(
                        ui,
                        copy::about_build(&info.target_os, &info.target_arch, "from source"),
                        Type::MonoSmall,
                        t.text_muted,
                    );
                    ui.add_space(space::XL);
                    ui.allocate_ui_with_layout(
                        Vec2::new(560.0, 0.0),
                        Layout::top_down(Align::Center),
                        |ui| {
                            widgets::paragraph_at(
                                ui,
                                copy::about::TAGLINE,
                                Type::Body,
                                t.text_secondary,
                                560.0,
                            );
                        },
                    );
                },
            );

            ui.add_space(space::H2);
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width().min(560.0), 0.0),
                Layout::top_down(Align::Min),
                |ui| {
                    widgets::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let kopia = self
                            .data
                            .snapshot
                            .as_ref()
                            .and_then(|s| s.kopia_version.clone())
                            .unwrap_or_else(|| copy::about::KOPIA_MISSING.to_string());
                        widgets::kv(ui, copy::about::KOPIA, &kopia, false);
                        widgets::kv(
                            ui,
                            copy::about::MACHINE,
                            &format!("{} ({})", self.data.machine_label(), self.data.machine_slug()),
                            false,
                        );
                        widgets::kv(
                            ui,
                            copy::about::SCHEMA,
                            &superbackup_core::model::CONFIG_SCHEMA_VERSION.to_string(),
                            false,
                        );
                    });

                    ui.add_space(space::XL);
                    widgets::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        widgets::text(ui, copy::about::LICENCES, Type::H2, t.text_primary);
                        ui.add_space(space::L);
                        widgets::paragraph_at(
                            ui,
                            copy::about::LICENCE_SELF,
                            Type::Small,
                            t.text_secondary,
                            520.0,
                        );
                        ui.add_space(space::S);
                        widgets::paragraph_at(
                            ui,
                            copy::about::LICENCE_KOPIA,
                            Type::Small,
                            t.text_secondary,
                            520.0,
                        );
                        ui.add_space(space::M);
                        ui.horizontal(|ui| {
                            if widgets::link(ui, copy::about::LICENCE_KOPIA_VIEW).clicked() {
                                open = Some("https://www.apache.org/licenses/LICENSE-2.0".into());
                            }
                            ui.add_space(space::L);
                            if widgets::link(ui, copy::about::LICENCE_KOPIA_SITE).clicked() {
                                open = Some("https://kopia.io".into());
                            }
                        });
                        ui.add_space(space::M);
                        widgets::paragraph_at(
                            ui,
                            copy::about::LICENCE_FONTS,
                            Type::Small,
                            t.text_muted,
                            520.0,
                        );
                        ui.add_space(space::L);
                        if Button::secondary(copy::about::LICENCE_COPY_ALL)
                            .icon(Icon::Copy)
                            .show(ui)
                            .clicked()
                        {
                            let block = format!(
                                "{}\n\n{}\n\n{}",
                                copy::about::LICENCE_SELF,
                                copy::about::LICENCE_KOPIA,
                                copy::about::LICENCE_FONTS
                            );
                            ui.ctx().copy_text(block);
                            self.toasts.success(copy::toast::COPIED_CLIPBOARD);
                        }
                    });

                    ui.add_space(space::XL);
                    ui.horizontal_wrapped(|ui| {
                        for (label, url) in [
                            (copy::about::LINK_WEBSITE, WEBSITE),
                            (copy::about::LINK_DOCS, DOCS),
                            (copy::about::LINK_ISSUE, ISSUES),
                            (copy::about::LINK_KOPIA_DOCS, KOPIA_DOCS),
                            (copy::about::LINK_RELEASES, RELEASES),
                        ] {
                            if Button::ghost(label).icon(Icon::ExternalLink).show(ui).clicked() {
                                let target = if label == copy::about::LINK_ISSUE {
                                    // Pre-fill the version and OS, so a report
                                    // arrives with the two facts it needs.
                                    format!(
                                        "{url}?body=superbackup%20{}%20on%20{}-{}",
                                        info.version, info.target_os, info.target_arch
                                    )
                                } else {
                                    url.to_string()
                                };
                                open = Some(target);
                            }
                        }
                    });

                    ui.add_space(space::XL);
                    widgets::text(ui, copy::about::COPYRIGHT, Type::Small, t.text_muted);
                    ui.add_space(space::H2);
                },
            );
        });

        if let Some(url) = open {
            let _ = open::that_detached(url);
        }
    }
}
