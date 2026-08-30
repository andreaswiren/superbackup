//! `A-1`. Runs and events, both virtualised, both honest about the 200-run
//! bound rather than pretending to be infinite.

use chrono::Utc;
use egui::{Align, Layout, Sense, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::state::{JobRun, RunStatus, Severity};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::modals::Modal;
use crate::gui::nav::Route;
use crate::gui::theme::{self, size, space, Type};
use crate::gui::viewmodel::{self, RunFilters, TimeRange};
use crate::gui::widgets::{self, Button};

pub struct State {
    pub events_view: bool,
    pub range: TimeRange,
    pub filters: RunFilters,
    pub severity: Severity,
    pub expanded_event: Option<Uuid>,
}

impl Default for State {
    fn default() -> Self {
        State {
            events_view: false,
            range: TimeRange::Week,
            filters: RunFilters::default(),
            // Debug rows are only reachable by choosing `All`.
            severity: Severity::Info,
            expanded_event: None,
        }
    }
}

impl State {
    pub fn filter_job(&mut self, job: Uuid) {
        self.filters.job = Some(job);
        self.range = TimeRange::All;
    }
    pub fn clear_filters(&mut self) {
        self.filters = RunFilters::default();
    }
}

impl App {
    pub(crate) fn activity_actions(&mut self, ui: &mut Ui) {
        let mut export = false;
        if Button::ghost(copy::activity::EXPORT).icon(Icon::Download).show(ui).clicked() {
            export = true;
        }
        let ranges: Vec<String> = TimeRange::ALL.iter().map(|r| r.title().to_string()).collect();
        let mut index =
            TimeRange::ALL.iter().position(|r| *r == self.screens.activity.range).unwrap_or(1);
        if widgets::combo(ui, "activity-range", &mut index, &ranges, 160.0, true) {
            self.screens.activity.range = TimeRange::ALL[index];
        }
        widgets::Field::new()
            .width(240.0)
            .placeholder(copy::activity::SEARCH)
            .show(ui, &mut self.screens.activity.filters.search);
        let mut tab = usize::from(self.screens.activity.events_view);
        widgets::segmented(
            ui,
            &mut tab,
            &[copy::activity::TAB_RUNS, copy::activity::TAB_EVENTS],
        );
        self.screens.activity.events_view = tab == 1;
        if export {
            self.open_modal(Modal::Export);
        }
    }

    pub(crate) fn show_activity(&mut self, ui: &mut Ui) {
        if self.screens.activity.events_view {
            self.activity_events(ui);
        } else {
            self.activity_runs(ui);
        }
    }

    fn activity_runs(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();

        if self.data.history.is_empty() && !self.data.loading {
            let (primary, _) = widgets::empty_state(ui, Icon::List, &copy::empty::ACTIVITY, None);
            if primary {
                self.request_run_all();
            }
            return;
        }

        // Active filters, as removable chips.
        if self.screens.activity.filters.any() {
            let mut clear = false;
            let mut clear_job = false;
            let mut clear_status = false;
            ui.horizontal(|ui| {
                if let Some(job) = self.screens.activity.filters.job {
                    let name = self.data.job_name(&job);
                    if Button::secondary(&copy::activity_filter_job(&name))
                        .compact()
                        .icon(Icon::X)
                        .show(ui)
                        .clicked()
                    {
                        clear_job = true;
                    }
                }
                if let Some(status) = self.screens.activity.filters.status {
                    if Button::secondary(&copy::activity_filter_status(status.title()))
                        .compact()
                        .icon(Icon::X)
                        .show(ui)
                        .clicked()
                    {
                        clear_status = true;
                    }
                }
                if Button::ghost(copy::action::CLEAR_FILTERS).compact().show(ui).clicked() {
                    clear = true;
                }
            });
            ui.add_space(space::L);
            if clear {
                self.screens.activity.clear_filters();
            }
            if clear_job {
                self.screens.activity.filters.job = None;
            }
            if clear_status {
                self.screens.activity.filters.status = None;
            }
        }

        let rows: Vec<JobRun> = viewmodel::visible_runs(
            &self.data.history,
            &self.screens.activity.filters,
            self.screens.activity.range,
            now,
        )
        .into_iter()
        .cloned()
        .collect();

        if rows.is_empty() {
            let (clear, _) =
                widgets::empty_state(ui, Icon::SearchX, &copy::empty::ACTIVITY_FILTERED, None);
            if clear {
                self.screens.activity.clear_filters();
            }
            return;
        }

        let narrow = ui.available_width() < 840.0;
        let mut open: Option<Uuid> = None;

        widgets::table_frame(ui, |ui| {
            let mut builder = egui_extras::TableBuilder::new(ui)
                .id_salt("activity-runs")
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(egui_extras::Column::exact(32.0))
                .column(egui_extras::Column::exact(130.0))
                .column(egui_extras::Column::exact(180.0));
            if !narrow {
                builder = builder.column(egui_extras::Column::exact(90.0));
            }
            builder = builder.column(egui_extras::Column::remainder().at_least(150.0));
            if !narrow {
                builder = builder.column(egui_extras::Column::exact(80.0));
                builder = builder.column(egui_extras::Column::exact(90.0));
            }
            builder = builder.column(egui_extras::Column::exact(32.0));

            builder
                .header(size::TABLE_HEADER_H, |mut header| {
                    header.col(|ui| {
                        widgets::table_header(ui, "", None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::STARTED, Some(true));
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::JOB, None);
                    });
                    if !narrow {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::col::TRIGGER, None);
                        });
                    }
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::DESTINATIONS, None);
                    });
                    if !narrow {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::col::DURATION, None);
                        });
                        header.col(|ui| {
                            widgets::table_header(ui, copy::col::UPLOADED, None);
                        });
                    }
                    header.col(|ui| {
                        widgets::table_header(ui, "", None);
                    });
                })
                .body(|body| {
                    body.rows(size::TABLE_ROW_H, rows.len(), |mut row| {
                        let index = row.index();
                        let Some(run) = rows.get(index) else {
                            return;
                        };
                        // The stored status, which `derive_status` already
                        // refuses to flatten.
                        let status = run.status;
                        row.col(|ui| {
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            Icon::for_status(status).paint(
                                ui.painter(),
                                rect,
                                t.status_for(status).mark,
                            );
                            response.on_hover_text(status.title());
                            // A failed run carries a 3px danger spine.
                            if status == RunStatus::Failed {
                                let full = ui.max_rect();
                                ui.painter().rect_filled(
                                    egui::Rect::from_min_size(
                                        full.left_top(),
                                        Vec2::new(3.0, full.height()),
                                    ),
                                    0,
                                    t.danger.mark,
                                );
                            }
                        });
                        row.col(|ui| {
                            widgets::text(
                                ui,
                                format::absolute(run.started_at),
                                Type::MonoSmall,
                                t.text_secondary,
                            );
                        });
                        row.col(|ui| {
                            widgets::elided(
                                ui,
                                &run.job_name,
                                Type::BodyStrong,
                                t.text_primary,
                                166.0,
                                false,
                            );
                        });
                        if !narrow {
                            row.col(|ui| {
                                widgets::text(
                                    ui,
                                    copy::trigger(run.trigger),
                                    Type::Small,
                                    t.text_muted,
                                );
                            });
                        }
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = space::XS;
                                for destination in &run.destinations {
                                    let palette = t.status_for(destination.status);
                                    let label = format!(
                                        "{}: {}",
                                        destination.destination_name,
                                        destination.status.title()
                                    );
                                    widgets::status_dot(ui, palette.mark, &label, 8.0);
                                }
                                ui.add_space(space::XS);
                                widgets::text(
                                    ui,
                                    viewmodel::destination_summary(run),
                                    Type::MonoSmall,
                                    t.text_muted,
                                );
                            });
                        });
                        if !narrow {
                            row.col(|ui| {
                                let text = run
                                    .duration_seconds()
                                    .map(format::duration)
                                    .unwrap_or_else(|| "—".to_string());
                                widgets::numeric_cell(ui, &text);
                            });
                            row.col(|ui| {
                                let uploaded: u64 = run
                                    .destinations
                                    .iter()
                                    .map(|d| d.progress.bytes_uploaded)
                                    .sum();
                                widgets::numeric_cell(ui, &format::bytes(uploaded));
                            });
                        }
                        row.col(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            Icon::ChevronRight.paint(ui.painter(), rect, t.text_muted);
                        });

                        let response = row.response();
                        let announce = format!(
                            "{}, {}, {}, {}",
                            run.job_name,
                            status.title(),
                            format::absolute(run.started_at),
                            viewmodel::destination_summary(run)
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &announce)
                        });
                        if response.clicked() {
                            open = Some(run.run_id);
                        }
                    });
                });
        });

        ui.add_space(space::M);
        widgets::text(ui, copy::activity::HISTORY_NOTE, Type::Small, t.text_muted);

        if let Some(id) = open {
            self.go(Route::RunDetail(id));
        }
    }

    fn activity_events(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();

        let severities = [
            (Severity::Debug, copy::activity::SEVERITY_ALL),
            (Severity::Info, copy::activity::SEVERITY_INFO),
            (Severity::Warning, copy::activity::SEVERITY_WARN),
            (Severity::Error, copy::activity::SEVERITY_ERROR),
        ];
        ui.horizontal(|ui| {
            widgets::text(ui, copy::activity::SEVERITY, Type::Small, t.text_secondary);
            let labels: Vec<String> =
                severities.iter().map(|(_, label)| (*label).to_string()).collect();
            let mut index = severities
                .iter()
                .position(|(s, _)| *s == self.screens.activity.severity)
                .unwrap_or(1);
            if widgets::combo(ui, "activity-severity", &mut index, &labels, 200.0, true) {
                self.screens.activity.severity = severities[index].0;
            }
        });
        if self.screens.activity.severity == Severity::Debug {
            ui.add_space(space::S);
            widgets::text(ui, copy::activity::DEBUG_NOTE, Type::Small, t.text_muted);
        }
        ui.add_space(space::L);

        let rows: Vec<superbackup_core::state::Event> = viewmodel::visible_events(
            &self.data.events,
            &self.screens.activity.filters.search,
            self.screens.activity.severity,
            self.screens.activity.range,
            now,
        )
        .into_iter()
        .cloned()
        .collect();

        if rows.is_empty() {
            widgets::empty_state(ui, Icon::List, &copy::empty::EVENTS, None);
            return;
        }

        let mut expand: Option<Uuid> = None;
        widgets::table_frame(ui, |ui| {
            egui_extras::TableBuilder::new(ui)
                .id_salt("activity-events")
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(egui_extras::Column::exact(28.0))
                .column(egui_extras::Column::exact(130.0))
                .column(egui_extras::Column::exact(160.0))
                .column(egui_extras::Column::remainder().at_least(200.0))
                .column(egui_extras::Column::exact(130.0))
                .header(size::TABLE_HEADER_H, |mut header| {
                    header.col(|ui| {
                        widgets::table_header(ui, "", None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::TIME, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::EVENT, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::MESSAGE, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::JOB, None);
                    });
                })
                .body(|body| {
                    body.rows(size::TABLE_ROW_H_COMPACT, rows.len(), |mut row| {
                        let index = row.index();
                        let Some(event) = rows.get(index) else {
                            return;
                        };
                        let palette = t.severity(event.severity);
                        row.col(|ui| {
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                            Icon::for_severity(event.severity).paint(
                                ui.painter(),
                                rect,
                                palette.mark,
                            );
                            response.on_hover_text(format!("{:?}", event.severity));
                        });
                        row.col(|ui| {
                            widgets::text(
                                ui,
                                format::absolute_seconds(event.at),
                                Type::MonoSmall,
                                t.text_muted,
                            )
                            .on_hover_text(format::offset_note(event.at));
                        });
                        row.col(|ui| {
                            widgets::elided(
                                ui,
                                &event.kind,
                                Type::MonoSmall,
                                t.text_secondary,
                                148.0,
                                false,
                            );
                        });
                        row.col(|ui| {
                            let width = ui.available_width();
                            widgets::elided(
                                ui,
                                &event.message,
                                Type::Small,
                                t.text_primary,
                                width,
                                false,
                            );
                        });
                        row.col(|ui| {
                            let name = event
                                .job_id
                                .map(|id| self.data.job_name(&id))
                                .unwrap_or_else(|| "—".to_string());
                            widgets::elided(
                                ui,
                                &name,
                                Type::Small,
                                t.text_muted,
                                118.0,
                                false,
                            );
                        });
                        if row.response().clicked() {
                            expand = Some(event.id);
                        }
                    });
                });
        });

        if let Some(id) = expand {
            self.screens.activity.expanded_event =
                if self.screens.activity.expanded_event == Some(id) { None } else { Some(id) };
        }
        if let Some(id) = self.screens.activity.expanded_event {
            if let Some(event) = rows.iter().find(|e| e.id == id) {
                ui.add_space(space::L);
                widgets::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    widgets::kv(ui, copy::col::EVENT, &event.kind, true);
                    widgets::kv(ui, copy::col::MESSAGE, &event.message, false);
                    if let Some(run_id) = event.run_id {
                        widgets::kv(ui, copy::run::DETAIL_RUN_ID, &run_id.to_string(), true);
                    }
                    for (key, value) in &event.fields {
                        widgets::kv(ui, key, &value.to_string(), true);
                    }
                });
            }
        }
    }
}
