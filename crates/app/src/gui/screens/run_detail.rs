//! `A-2`. One run, one card per destination. The fan-out is never flattened:
//! a run that succeeded twice and failed once shows all three.

use chrono::Utc;
use egui::{Align, Layout, Sense, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::state::{DestinationRun, JobRun, RunStatus};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::nav::Route;
use crate::gui::theme::{self, space, Type};
use crate::gui::viewmodel;
use crate::gui::widgets::{self, Button};

#[derive(Default)]
pub struct State {
    pub expanded_warnings: Option<Uuid>,
    pub expanded_details: Option<Uuid>,
}

impl App {
    pub(crate) fn show_run_detail(&mut self, ui: &mut Ui, run_id: Uuid) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();
        let run = self
            .data
            .history
            .iter()
            .chain(self.data.active_runs().iter())
            .find(|r| r.run_id == run_id)
            .cloned();
        let Some(run) = run else {
            widgets::banner(
                ui,
                widgets::BannerKind::Warning,
                "That run is no longer in the local history.",
                Some(copy::activity::HISTORY_NOTE),
                |_| {},
            );
            return;
        };

        let mut retry = false;
        let mut copy_summary = false;
        let mut browse: Option<Uuid> = None;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // Summary card.
            widgets::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    widgets::status_badge(ui, run.status);
                    ui.add_space(space::L);
                    widgets::text(ui, &run.job_name, Type::H1, t.text_primary);
                });
                ui.add_space(space::L);
                let half = (ui.available_width() / 2.0).max(240.0);
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(half - space::XL, 120.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            widgets::kv(ui, copy::run::DETAIL_STATUS, run.status.title(), false);
                            widgets::kv(
                                ui,
                                copy::run::DETAIL_TRIGGER,
                                copy::trigger(run.trigger),
                                false,
                            );
                            widgets::kv(
                                ui,
                                copy::run::DETAIL_DURATION,
                                &run
                                    .duration_seconds()
                                    .map(format::duration)
                                    .unwrap_or_else(|| copy::state::UNKNOWN.to_string()),
                                false,
                            );
                            widgets::kv(
                                ui,
                                copy::run::DETAIL_DESTINATIONS,
                                &viewmodel::destination_summary(&run),
                                false,
                            );
                        },
                    );
                    ui.allocate_ui_with_layout(
                        Vec2::new(half - space::XL, 120.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            widgets::kv(
                                ui,
                                copy::run::DETAIL_STARTED,
                                &format::absolute_seconds(run.started_at),
                                false,
                            );
                            widgets::kv(
                                ui,
                                copy::run::DETAIL_FINISHED,
                                &run
                                    .finished_at
                                    .map(format::absolute_seconds)
                                    .unwrap_or_else(|| "—".to_string()),
                                false,
                            );
                            copyable_id(ui, copy::run::DETAIL_RUN_ID, &run.run_id);
                            copyable_id(ui, copy::run::DETAIL_JOB_ID, &run.job_id);
                        },
                    );
                });
                if run.status == RunStatus::SucceededWithWarnings
                    || run.destinations.iter().any(|d| d.status == RunStatus::Failed)
                {
                    ui.add_space(space::M);
                    widgets::paragraph_at(
                        ui,
                        copy::run::DETAIL_PARTIAL,
                        Type::Small,
                        t.text_muted,
                        560.0,
                    );
                }
            });

            ui.add_space(space::XL);

            for destination in &run.destinations {
                self.destination_card(ui, &run, destination, &mut browse);
                ui.add_space(space::XL);
            }

            ui.add_space(space::L);
            ui.horizontal(|ui| {
                if Button::primary(copy::run::DETAIL_RETRY).icon(Icon::Play).show(ui).clicked() {
                    retry = true;
                }
                if Button::secondary(copy::run::DETAIL_COPY_SUMMARY)
                    .icon(Icon::Copy)
                    .show(ui)
                    .clicked()
                {
                    copy_summary = true;
                }
            });
            ui.add_space(space::H2);
        });

        if copy_summary {
            let text = viewmodel::run_summary_text(&run, now);
            ui.ctx().copy_text(text);
            self.toasts.success(copy::toast::COPIED_CLIPBOARD);
        }
        if retry {
            if let Some(job) = self.data.job(&run.job_id).cloned() {
                self.request_run(&job);
            } else {
                self.toasts.warning(copy::err::JOB_NOT_FOUND);
            }
        }
        if let Some(id) = browse {
            self.screens.restore.select(id);
            self.go(Route::Restore);
        }
    }

    fn destination_card(
        &mut self,
        ui: &mut Ui,
        run: &JobRun,
        destination: &DestinationRun,
        browse: &mut Option<Uuid>,
    ) {
        let t = theme::tokens(ui.ctx());
        let failed = destination.status == RunStatus::Failed;
        widgets::card_tinted(
            ui,
            None,
            failed.then(|| theme::alpha(t.danger.mark, 0.4)),
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let icon = self
                        .data
                        .destination(&destination.destination_id)
                        .map(|d| Icon::for_destination_kind(&d.kind))
                        .unwrap_or(Icon::HardDrive);
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                    icon.paint(ui.painter(), rect, t.text_secondary);
                    ui.add_space(space::L);
                    widgets::text(ui, &destination.destination_name, Type::H2, t.text_primary);
                    ui.add_space(space::L);
                    widgets::status_badge(ui, destination.status);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let (Some(started), Some(finished)) =
                            (destination.started_at, destination.finished_at)
                        {
                            widgets::text(
                                ui,
                                format::duration((finished - started).num_seconds()),
                                Type::MonoSmall,
                                t.text_muted,
                            );
                        }
                    });
                });
                ui.add_space(space::L);

                match &destination.snapshot_id {
                    Some(id) => {
                        let id = id.clone();
                        widgets::kv_with(ui, copy::run::DETAIL_SNAPSHOT, |ui| {
                            widgets::text(
                                ui,
                                format::short_snapshot(&id),
                                Type::MonoSmall,
                                t.text_primary,
                            )
                            .on_hover_text(id.clone());
                            if widgets::icon_button_compact(
                                ui,
                                Icon::Copy,
                                copy::action::COPY,
                                true,
                            )
                            .clicked()
                            {
                                ui.ctx().copy_text(id.clone());
                            }
                            if widgets::link(ui, copy::run::DETAIL_BROWSE).clicked() {
                                *browse = Some(destination.destination_id);
                            }
                        });
                    }
                    None => {
                        widgets::kv(ui, copy::run::DETAIL_SNAPSHOT, copy::run::DETAIL_NO_SNAPSHOT, false);
                    }
                }

                widgets::kv(
                    ui,
                    copy::run::DETAIL_FILES_LABEL,
                    &copy::run_detail_files(
                        destination.progress.files_processed,
                        destination.progress.files_cached,
                        destination.progress.errors_ignored,
                    ),
                    false,
                );
                widgets::kv(
                    ui,
                    copy::run::DETAIL_DATA_LABEL,
                    &copy::run_detail_data(
                        destination.progress.bytes_processed,
                        destination.progress.bytes_uploaded,
                    ),
                    false,
                );
                widgets::kv(
                    ui,
                    copy::run::DETAIL_THROUGHPUT_LABEL,
                    &copy::run_detail_throughput(destination.progress.bytes_per_second),
                    false,
                );

                if !destination.warnings.is_empty() {
                    ui.add_space(space::L);
                    let label = copy::run_detail_warnings(destination.warnings.len());
                    if widgets::link(ui, &label).clicked() {
                        self.screens.run_detail.expanded_warnings =
                            if self.screens.run_detail.expanded_warnings
                                == Some(destination.destination_id)
                            {
                                None
                            } else {
                                Some(destination.destination_id)
                            };
                    }
                    if self.screens.run_detail.expanded_warnings
                        == Some(destination.destination_id)
                    {
                        ui.add_space(space::M);
                        widgets::code_block(
                            ui,
                            &destination.warnings.join("\n"),
                            240.0,
                            Some(t.warning),
                        );
                    }
                }

                if let Some(error) = &destination.error {
                    ui.add_space(space::XL);
                    widgets::card_tinted(
                        ui,
                        Some(t.danger.tint_bg),
                        Some(theme::alpha(t.danger.mark, 0.4)),
                        |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (rect, _) =
                                    ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                                Icon::XOctagon.paint(ui.painter(), rect, t.danger.mark);
                                ui.add_space(space::M);
                                widgets::text(
                                    ui,
                                    RunStatus::Failed.title(),
                                    Type::BodyStrong,
                                    t.danger.tint_text,
                                );
                            });
                            ui.add_space(space::M);
                            widgets::paragraph_at(
                                ui,
                                &error.message,
                                Type::H3,
                                t.text_primary,
                                (ui.available_width() - 8.0).max(240.0),
                            );
                            ui.add_space(space::S);
                            // The error code is shown, not hidden: it is the
                            // stable identifier a user pastes into an issue.
                            widgets::text(
                                ui,
                                copy::run_detail_error_code(
                                    &format!("{:?}", error.code).to_lowercase(),
                                    &format::absolute_seconds(error.occurred_at),
                                ),
                                Type::MonoSmall,
                                t.text_muted,
                            );
                            if let Some(hint) = &error.hint {
                                ui.add_space(space::L);
                                widgets::banner(
                                    ui,
                                    widgets::BannerKind::Info,
                                    hint,
                                    None,
                                    |_| {},
                                );
                            }
                            if let Some(detail) = &error.detail {
                                ui.add_space(space::L);
                                let expanded = self.screens.run_detail.expanded_details
                                    == Some(destination.destination_id);
                                let label = if expanded {
                                    copy::action::HIDE_DETAILS
                                } else {
                                    copy::action::SHOW_DETAILS
                                };
                                if widgets::link(ui, label).clicked() {
                                    self.screens.run_detail.expanded_details = if expanded {
                                        None
                                    } else {
                                        Some(destination.destination_id)
                                    };
                                }
                                if expanded {
                                    ui.add_space(space::M);
                                    widgets::code_block(ui, detail, 240.0, Some(t.danger));
                                    ui.add_space(space::S);
                                    widgets::text(
                                        ui,
                                        copy::run::DETAIL_REDACTED,
                                        Type::Small,
                                        t.text_muted,
                                    );
                                }
                            }
                        },
                    );
                }
            },
        );
        let _ = run;
    }
}

fn copyable_id(ui: &mut Ui, label: &str, id: &Uuid) {
    let t = theme::tokens(ui.ctx());
    let full = id.to_string();
    widgets::kv_with(ui, label, |ui| {
        widgets::text(ui, format::short_uuid(id), Type::MonoSmall, t.text_primary)
            .on_hover_text(full.clone());
        if widgets::icon_button_compact(ui, Icon::Copy, copy::action::COPY, true).clicked() {
            ui.ctx().copy_text(full.clone());
        }
    });
}
