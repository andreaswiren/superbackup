//! `J-1`. The job table: search, group, filter, sort, and the row actions.

use chrono::Utc;
use egui::{Align, Layout, Sense, Ui, Vec2};
use egui_extras::{Column, TableBuilder};
use uuid::Uuid;

use superbackup_core::ipc::protocol::Request;
use superbackup_core::model::Job;

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::daemon::Intent;
use crate::gui::data::Action;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::modals::{self, Modal};
use crate::gui::nav::{Route, SettingsSection};
use crate::gui::theme::{self, size, space, Type};
use crate::gui::viewmodel::{self, CardState, ColumnSpec, GroupBy, JobFilter, SortKey};
use crate::gui::widgets::{self, Button};

pub struct State {
    pub search: String,
    pub filter: JobFilter,
    pub group: GroupBy,
    pub sort: SortKey,
    pub descending: bool,
    pub selected: Vec<Uuid>,
}

impl Default for State {
    fn default() -> Self {
        State {
            search: String::new(),
            filter: JobFilter::All,
            group: GroupBy::None,
            // Problems first, then name.
            sort: SortKey::Status,
            descending: false,
            selected: Vec::new(),
        }
    }
}

/// `UX_SPEC.md` §6.1. The widths are trimmed from the specification's own
/// numbers, which sum wider than the content column they sit in; the drop
/// order is the specification's: uploaded, then next run, then folders.
const JOB_COLUMNS: [ColumnSpec; 8] = [
    ColumnSpec::keep("status", 32.0),
    ColumnSpec::keep("name", 184.0),
    ColumnSpec::droppable("sources", 52.0, 4),
    ColumnSpec::keep("destinations", 124.0),
    ColumnSpec::keep("schedule", 132.0),
    ColumnSpec::keep("last", 104.0),
    ColumnSpec::droppable("next", 104.0, 3),
    ColumnSpec::droppable("uploaded", 84.0, 1),
];

impl App {
    pub(crate) fn jobs_actions(&mut self, ui: &mut Ui) {
        let mut new_job = false;
        if Button::primary(copy::jobs::NEW).icon(Icon::Plus).show(ui).clicked() {
            new_job = true;
        }

        let filters: Vec<String> = JobFilter::ALL.iter().map(|f| f.title().to_string()).collect();
        let mut filter_index =
            JobFilter::ALL.iter().position(|f| *f == self.screens.jobs.filter).unwrap_or(0);
        if widgets::combo_labelled(
            ui,
            "jobs-filter",
            Some(copy::jobs::FILTER),
            &mut filter_index,
            &filters,
            170.0,
            true,
        ) {
            self.screens.jobs.filter = JobFilter::ALL[filter_index];
        }

        let groups: Vec<String> = GroupBy::ALL.iter().map(|g| g.title().to_string()).collect();
        let mut group_index =
            GroupBy::ALL.iter().position(|g| *g == self.screens.jobs.group).unwrap_or(0);
        if widgets::combo_labelled(
            ui,
            "jobs-group",
            Some(copy::jobs::GROUP_BY),
            &mut group_index,
            &groups,
            170.0,
            true,
        ) {
            self.screens.jobs.group = GroupBy::ALL[group_index];
        }

        widgets::Field::new()
            .width(240.0)
            .placeholder(copy::jobs::SEARCH)
            .show(ui, &mut self.screens.jobs.search);

        if new_job {
            let wizard = crate::gui::screens::wizard::WizardState::new(&self.data);
            self.open_modal(Modal::Wizard(Box::new(wizard)));
        }
    }

    pub(crate) fn show_jobs(&mut self, ui: &mut Ui) {
        let now = Utc::now();
        let t = theme::tokens(ui.ctx());

        if self.data.jobs.is_empty() && !self.data.loading {
            let (primary, secondary) =
                widgets::empty_state(ui, Icon::Repeat, &copy::empty::JOBS, None);
            if primary {
                let wizard = crate::gui::screens::wizard::WizardState::new(&self.data);
                self.open_modal(Modal::Wizard(Box::new(wizard)));
            }
            if secondary {
                // The secondary result used to be discarded, so "Import from
                // another machine…" was a button that did nothing at all when
                // pressed. Sharing jobs between machines is the shared-config
                // repository, so send the user where that is actually set up
                // rather than leaving the click unanswered.
                self.go(Route::Settings(SettingsSection::Remote));
            }
            return;
        }

        let state = &self.screens.jobs;
        let rows = viewmodel::visible_jobs(
            &self.data,
            &state.search,
            state.filter,
            state.sort,
            state.descending,
            now,
        );

        if rows.is_empty() {
            let total = self.data.jobs.len();
            let body = copy::empty_jobs_filtered_body(total);
            let (clear, _) =
                widgets::empty_state(ui, Icon::SearchX, &copy::empty::JOBS_FILTERED, Some(&body));
            if clear {
                self.screens.jobs.search.clear();
                self.screens.jobs.filter = JobFilter::All;
            }
            return;
        }

        // Selection bar replaces the header actions while rows are selected.
        let mut bulk: Option<&'static str> = None;
        if !self.screens.jobs.selected.is_empty() {
            let count = self.screens.jobs.selected.len();
            let mut act: Option<&'static str> = None;
            widgets::banner(
                ui,
                widgets::BannerKind::Info,
                &copy::jobs_selected(count),
                None,
                |ui| {
                    if Button::danger_ghost(copy::action::DELETE).compact().show(ui).clicked() {
                        act = Some("delete");
                    }
                    if Button::ghost(copy::action::DISABLE).compact().show(ui).clicked() {
                        act = Some("disable");
                    }
                    if Button::ghost(copy::action::ENABLE).compact().show(ui).clicked() {
                        act = Some("enable");
                    }
                    if Button::secondary(copy::action::RUN_NOW).compact().show(ui).clicked() {
                        act = Some("run");
                    }
                },
            );
            ui.add_space(space::XL);
            bulk = act;
        }

        let groups = viewmodel::group_jobs(rows, self.screens.jobs.group);
        let grouped = self.screens.jobs.group != GroupBy::None;

        let mut open: Option<Uuid> = None;
        let mut run: Option<Job> = None;
        let mut menu: Option<(&'static str, Uuid)> = None;
        let mut sort_click: Option<SortKey> = None;
        let shown = viewmodel::fit_columns(
            // The table card insets its content, and this is measured before
            // that frame is entered, so the budget has to lose both sides or
            // the last column is pushed past the card edge.
            ui.available_width() - widgets::TABLE_GUTTER,
            92.0,
            ui.spacing().item_spacing.x,
            &JOB_COLUMNS,
        );
        let has = |key: &str| shown.contains(&key);

        widgets::scroll_area(ui, "jobs", |ui| {
            for (heading, rows) in &groups {
                if grouped {
                    ui.horizontal(|ui| {
                        ui.set_min_height(28.0);
                        widgets::status_dot(ui, t.accent, heading, 8.0);
                        ui.add_space(space::M);
                        widgets::text(ui, heading, Type::H3, t.text_primary);
                        ui.add_space(space::M);
                        widgets::count_pill(ui, &rows.len().to_string());
                    });
                    ui.add_space(space::M);
                }

                widgets::table_frame(ui, |ui| {
                    let state = &self.screens.jobs;
                    let sorted = |key: SortKey| (state.sort == key).then_some(state.descending);
                    let mut builder = TableBuilder::new(ui)
                        .id_salt(("jobs", heading))
                        // Rows are clickable, and a table senses `hover` unless it is
                        // told otherwise — so `row.response().clicked()` was false on
                        // every row of every table in the application, and every list
                        // that opens something by being clicked did nothing at all.
                        .sense(egui::Sense::click())
                        .striped(false)
                        .cell_layout(Layout::left_to_right(Align::Center));
                    for spec in JOB_COLUMNS.iter().filter(|c| has(c.key)) {
                        builder = builder.column(Column::exact(spec.width));
                    }
                    builder = builder.column(Column::remainder().at_least(92.0));

                    builder
                        .header(size::TABLE_HEADER_H, |mut header| {
                            header.col(|ui| {
                                if widgets::table_header(ui, "", sorted(SortKey::Status))
                                    .interact(Sense::click())
                                    .clicked()
                                {
                                    sort_click = Some(SortKey::Status);
                                }
                            });
                            header.col(|ui| {
                                if widgets::table_header(ui, copy::col::NAME, sorted(SortKey::Name))
                                    .interact(Sense::click())
                                    .clicked()
                                {
                                    sort_click = Some(SortKey::Name);
                                }
                            });
                            if has("sources") {
                                header.col(|ui| {
                                    widgets::table_header(ui, copy::col::SOURCES, None);
                                });
                            }
                            if has("destinations") {
                                header.col(|ui| {
                                    widgets::table_header(ui, copy::col::DESTINATIONS, None);
                                });
                            }
                            header.col(|ui| {
                                widgets::table_header(ui, copy::col::SCHEDULE, None);
                            });
                            header.col(|ui| {
                                if widgets::table_header(
                                    ui,
                                    copy::col::LAST_RUN,
                                    sorted(SortKey::LastRun),
                                )
                                .interact(Sense::click())
                                .clicked()
                                {
                                    sort_click = Some(SortKey::LastRun);
                                }
                            });
                            if has("next") {
                                header.col(|ui| {
                                    if widgets::table_header(
                                        ui,
                                        copy::col::NEXT_RUN,
                                        sorted(SortKey::NextRun),
                                    )
                                    .interact(Sense::click())
                                    .clicked()
                                    {
                                        sort_click = Some(SortKey::NextRun);
                                    }
                                });
                            }
                            if has("uploaded") {
                                header.col(|ui| {
                                    widgets::table_header(ui, copy::col::UPLOADED, None);
                                });
                            }
                            header.col(|ui| {
                                widgets::table_header(ui, "", None);
                            });
                        })
                        .body(|body| {
                            // 36px rows, 48 when any row carries a description
                            // — the second line needs room to be legible.
                            let row_height = if rows.iter().any(|(j, _)| !j.description.is_empty())
                            {
                                48.0
                            } else {
                                size::TABLE_ROW_H
                            };
                            body.rows(row_height, rows.len(), |mut row| {
                                let index = row.index();
                                let Some((job, view)) = rows.get(index) else {
                                    return;
                                };
                                let selected = self.screens.jobs.selected.contains(&job.id);
                                row.set_selected(selected);
                                let summary = self.data.summary_for(&job.id).unwrap_or_default();
                                let disabled = matches!(view.state, CardState::Disabled { .. });
                                let dim = if disabled { 0.6 } else { 1.0 };

                                row.col(|ui| {
                                    let (rect, response) =
                                        ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                                    let (icon, mark) = if disabled {
                                        (Icon::Pause, t.neutral.mark)
                                    } else {
                                        (
                                            Icon::for_status(view.status),
                                            t.status_for(view.status).mark,
                                        )
                                    };
                                    icon.paint(ui.painter(), rect, theme::alpha(mark, dim));
                                    response.on_hover_text(view.badge.clone());
                                });
                                row.col(|ui| {
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                        widgets::elided(
                                            ui,
                                            &job.name,
                                            Type::BodyStrong,
                                            theme::alpha(t.text_primary, dim),
                                            170.0,
                                            false,
                                        );
                                        if !job.description.is_empty() {
                                            widgets::elided(
                                                ui,
                                                &job.description,
                                                Type::Small,
                                                t.text_muted,
                                                170.0,
                                                false,
                                            );
                                        }
                                    });
                                });
                                if has("sources") {
                                    row.col(|ui| {
                                        let response = widgets::text(
                                            ui,
                                            format::count(job.sources.len() as u64),
                                            Type::MonoSmall,
                                            t.text_secondary,
                                        );
                                        let paths: Vec<String> = job
                                            .sources
                                            .iter()
                                            .map(|s| s.path.to_string_lossy().into_owned())
                                            .collect();
                                        if !paths.is_empty() {
                                            response.on_hover_text(paths.join("\n"));
                                        }
                                    });
                                }
                                if has("destinations") {
                                    row.col(|ui| {
                                        self.destination_cell(ui, job);
                                    });
                                }
                                row.col(|ui| {
                                    widgets::elided(
                                        ui,
                                        &viewmodel::schedule_string(&job.schedule),
                                        Type::Small,
                                        t.text_secondary,
                                        120.0,
                                        false,
                                    );
                                });
                                row.col(|ui| {
                                    let text = summary
                                        .last_run
                                        .map(|at| format::relative_past(at, now))
                                        .unwrap_or_else(|| copy::state::NEVER.to_string());
                                    widgets::elided(
                                        ui,
                                        &text,
                                        Type::Small,
                                        t.text_secondary,
                                        92.0,
                                        false,
                                    );
                                });
                                if has("next") {
                                    row.col(|ui| {
                                        let text = match summary.next_run {
                                            Some(at) if job.enabled => {
                                                format::relative_future(at, now)
                                            }
                                            _ => copy::state::NONE.to_string(),
                                        };
                                        widgets::elided(
                                            ui,
                                            &text,
                                            Type::Small,
                                            t.text_muted,
                                            92.0,
                                            false,
                                        );
                                    });
                                }
                                if has("uploaded") {
                                    row.col(|ui| {
                                        widgets::numeric_cell(
                                            ui,
                                            &format::bytes(summary.last_uploaded_bytes),
                                        );
                                    });
                                }
                                row.col(|ui| {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        widgets::overflow_menu(
                                            ui,
                                            ("job-row", job.id),
                                            "More actions",
                                            |ui| {
                                                if widgets::menu_item(ui, copy::action::EDIT, true)
                                                {
                                                    menu = Some(("edit", job.id));
                                                }
                                                if widgets::menu_item(ui, "View history", true) {
                                                    menu = Some(("history", job.id));
                                                }
                                                if widgets::menu_item(
                                                    ui,
                                                    copy::preview::ACTION,
                                                    true,
                                                ) {
                                                    menu = Some(("preview", job.id));
                                                }
                                                if widgets::menu_item(
                                                    ui,
                                                    if job.enabled {
                                                        copy::action::DISABLE
                                                    } else {
                                                        copy::action::ENABLE
                                                    },
                                                    true,
                                                ) {
                                                    menu = Some(("toggle", job.id));
                                                }
                                                widgets::divider(ui);
                                                if widgets::menu_item_danger(
                                                    ui,
                                                    copy::action::DELETE,
                                                    true,
                                                ) {
                                                    menu = Some(("delete", job.id));
                                                }
                                            },
                                        );
                                        let gate = self.data.gate(Action::RunJob);
                                        let running =
                                            matches!(view.state, CardState::Running { .. });
                                        let label = if running {
                                            copy::action::STOP
                                        } else {
                                            copy::action::RUN_NOW
                                        };
                                        let mut button = Button::ghost(label)
                                            .compact()
                                            .a11y(format!("{label} job \"{}\"", job.name));
                                        if let Some(reason) = gate.reason() {
                                            button = button.disabled_because(reason);
                                        } else if !job.enabled {
                                            button = button.enabled(false);
                                        }
                                        if button.show(ui).clicked() {
                                            if running {
                                                menu = Some(("stop", job.id));
                                            } else {
                                                run = Some((*job).clone());
                                            }
                                        }
                                    });
                                });

                                let response = row.response();
                                let cells = format!(
                                    "{}, {}, {}, {}",
                                    job.name,
                                    view.badge,
                                    viewmodel::schedule_string(&job.schedule),
                                    view.meta
                                );
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &cells)
                                });
                                if response.clicked() {
                                    open = Some(job.id);
                                }
                            });
                        });
                });
                ui.add_space(space::XL);
            }
        });

        if let Some(key) = sort_click {
            let state = &mut self.screens.jobs;
            if state.sort == key {
                state.descending = !state.descending;
            } else {
                state.sort = key;
                state.descending = false;
            }
        }
        if let Some(job) = run {
            self.request_run(&job);
        }
        if let Some(id) = open {
            self.go(Route::JobEditor(id));
        }
        if let Some((action, id)) = menu {
            self.job_menu_action(action, id);
        }
        if bulk.is_some() {
            self.bulk_action(bulk);
        }
    }

    fn destination_cell(&self, ui: &mut Ui, job: &Job) {
        let t = theme::tokens(ui.ctx());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::XS;
            let shown = job.destination_ids.iter().take(3);
            for id in shown {
                let (icon, name) = match self.data.destination(id) {
                    Some(d) => (Icon::for_destination_kind(&d.kind), d.name.clone()),
                    None => (Icon::HardDrive, copy::state::UNKNOWN.to_string()),
                };
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                icon.paint(ui.painter(), rect, t.text_secondary);
                response.on_hover_text(name);
            }
            if job.destination_ids.len() > 3 {
                widgets::text(
                    ui,
                    format!("+{}", job.destination_ids.len() - 3),
                    Type::MonoSmall,
                    t.text_muted,
                );
            }
            if job.destination_ids.is_empty() {
                widgets::text(ui, copy::state::NONE, Type::Small, t.danger.tint_text);
            }
        });
    }

    fn job_menu_action(&mut self, action: &str, id: Uuid) {
        let Some(job) = self.data.job(&id).cloned() else {
            return;
        };
        match action {
            "edit" => self.go(Route::JobEditor(id)),
            "preview" => self.request_preview(&job),
            "history" => {
                self.screens.activity.filter_job(id);
                self.go(Route::Activity);
            }
            "toggle" => self.ask(
                Intent::SaveJob(job.name.clone()),
                Request::JobSetEnabled { job: id.to_string(), enabled: !job.enabled },
            ),
            "delete" => {
                self.open_modal(Modal::Confirm(modals::delete_job_confirm(Some(&job))));
            }
            "stop" => {
                if let Some(run) = self.data.active_run_for(&id) {
                    let run_id = run.run_id;
                    self.open_modal(Modal::Confirm(modals::stop_run_confirm(run_id, &job.name)));
                }
            }
            _ => {}
        }
    }

    fn bulk_action(&mut self, action: Option<&'static str>) {
        let Some(action) = action else {
            return;
        };
        let selected = self.screens.jobs.selected.clone();
        for id in selected {
            let Some(job) = self.data.job(&id).cloned() else {
                continue;
            };
            match action {
                "run" => self.request_run(&job),
                "enable" | "disable" => self.ask(
                    Intent::SaveJob(job.name.clone()),
                    Request::JobSetEnabled { job: id.to_string(), enabled: action == "enable" },
                ),
                "delete" => self.ask(
                    Intent::DeleteJob(job.name.clone()),
                    Request::JobDelete { job: id.to_string() },
                ),
                _ => {}
            }
        }
        self.screens.jobs.selected.clear();
    }
}
