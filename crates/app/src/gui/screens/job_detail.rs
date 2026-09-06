//! One job, everything about it, in one place.
//!
//! Clicking a job used to open its *settings*, which answers "how is this
//! configured" when the question a person clicking a job card has is almost
//! always "what has this been doing". Its runs were in Activity, filtered; its
//! destinations were on the dashboard, only while it happened to be running;
//! its errors were behind a run id nobody had. This is the job-centric view:
//! what it backs up, where to, what happened the last few times, and what it
//! has said for itself.
//!
//! Settings are one button away, rather than the only thing here.

use chrono::{DateTime, Utc};
use egui::{Align, Layout, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::state::{JobRun, RunStatus};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::data::Action;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::nav::Route;
use crate::gui::theme::{self, space, Type};
use crate::gui::widgets::{self, Button};

/// How many past runs the page shows before sending the reader to Activity.
///
/// Enough to see a pattern — a job that has failed the last three times looks
/// different from one that failed once — and short enough that the events
/// below it are still on the same screen.
const RECENT_RUNS: usize = 8;

impl App {
    pub(crate) fn job_detail_actions(&mut self, ui: &mut Ui, id: Uuid) {
        let Some(job) = self.data.job(&id).cloned() else { return };
        let running = self.data.active_runs().iter().any(|r| r.job_id == id);

        let gate = self.data.gate(Action::RunJob);
        let mut run = Button::primary(copy::action::RUN_NOW).icon(Icon::Play);
        if let Some(reason) = gate.reason() {
            run = run.disabled_because(reason);
        } else if running {
            run = run.enabled(false);
        }
        if run.show(ui).clicked() {
            self.request_run(&job);
        }
        if Button::secondary(copy::action::EDIT).icon(Icon::Pencil).show(ui).clicked() {
            self.go(Route::JobEditor(id));
        }
    }

    pub(crate) fn show_job_detail(&mut self, ui: &mut Ui, id: Uuid) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();
        let Some(job) = self.data.job(&id).cloned() else {
            // A job deleted while its page was open. Say so rather than
            // rendering an empty shell that looks like a loading state.
            widgets::empty_state(
                ui,
                Icon::SearchX,
                &crate::gui::copy::Empty {
                    title: copy::job_detail::GONE,
                    body: copy::job_detail::GONE_BODY,
                    primary: None,
                    secondary: None,
                },
                None,
            );
            return;
        };

        widgets::scroll_area(ui, "job-detail", |ui| {
            self.job_detail_summary(ui, &job, now);
            ui.add_space(space::XL);
            self.job_detail_running(ui, id, now);
            self.job_detail_runs(ui, id, now);
            ui.add_space(space::XL);
            self.job_detail_events(ui, id, now);
            ui.add_space(space::XL);
            let _ = t;
        });
    }

    /// What this job is, and what it did last.
    fn job_detail_summary(
        &mut self,
        ui: &mut Ui,
        job: &superbackup_core::model::Job,
        now: DateTime<Utc>,
    ) {
        let t = theme::tokens(ui.ctx());
        let summary = self
            .data
            .snapshot
            .as_ref()
            .and_then(|s| s.jobs.get(&job.id).cloned())
            .unwrap_or_default();

        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::text(ui, &job.name, Type::H1, t.text_primary);
                ui.add_space(space::M);
                match summary.last_status {
                    Some(status) => {
                        widgets::status_badge(ui, status);
                    }
                    None => {
                        widgets::neutral_badge(ui, copy::job_detail::NEVER_RUN, None);
                    }
                }
                if !job.enabled {
                    ui.add_space(space::S);
                    widgets::neutral_badge(ui, copy::job_detail::DISABLED, Some(Icon::Pause));
                }
            });
            if !job.description.is_empty() {
                ui.add_space(space::XS);
                widgets::paragraph(ui, job.description.clone(), Type::Small, t.text_muted);
            }

            ui.add_space(space::L);
            // The sentence, with the expression itself on hover: a paraphrase
            // of when a backup runs is a thing you want to be able to check.
            let schedule = crate::gui::viewmodel::schedule_string(&job.schedule);
            let row = widgets::kv(ui, copy::job_detail::SCHEDULE, &schedule, false);
            if let superbackup_core::model::Schedule::Cron { expression } = &job.schedule {
                row.on_hover_text(expression.clone());
            }
            widgets::kv(
                ui,
                copy::job_detail::NEXT_RUN,
                &match summary.next_run {
                    // A disabled job has a next run in the data and none in
                    // reality; showing the date would be a promise it will not
                    // keep.
                    Some(at) if job.enabled => format::relative(at, now),
                    _ => copy::job_detail::NOT_SCHEDULED.to_string(),
                },
                false,
            );
            widgets::kv(
                ui,
                copy::job_detail::LAST_RUN,
                &match summary.last_run {
                    Some(at) => format::relative(at, now),
                    None => copy::job_detail::NEVER_RUN.to_string(),
                },
                false,
            );

            ui.add_space(space::L);
            widgets::text(ui, copy::job_detail::SOURCES, Type::H3, t.text_primary);
            ui.add_space(space::XS);
            if job.sources.is_empty() {
                widgets::paragraph(ui, copy::job_detail::NO_SOURCES, Type::Small, t.warning.tint_text);
            }
            for source in &job.sources {
                widgets::text(
                    ui,
                    source.path.display().to_string(),
                    Type::MonoSmall,
                    t.text_secondary,
                );
            }

            ui.add_space(space::L);
            widgets::text(ui, copy::job_detail::DESTINATIONS, Type::H3, t.text_primary);
            ui.add_space(space::XS);
            if job.destination_ids.is_empty() {
                widgets::paragraph(
                    ui,
                    copy::job_detail::NO_DESTINATIONS,
                    Type::Small,
                    t.warning.tint_text,
                );
            }
            for id in &job.destination_ids {
                match self.data.destination(id) {
                    Some(destination) => {
                        // The full address, endpoint included: `s3://bucket/…`
                        // does not say whose S3 it is, and StorJ, Wasabi and
                        // AWS look identical in that form.
                        let provider = destination
                            .kind
                            .provider_id()
                            .and_then(|id| self.data.provider(id));
                        let location = crate::gui::viewmodel::destination_location_full(
                            destination,
                            provider,
                        );
                        widgets::kv(ui, &destination.name, &location, true);
                    }
                    // A dangling id is the state that makes a job fail with
                    // nothing to point at, so it is shown rather than skipped.
                    None => {
                        widgets::kv(ui, &id.to_string(), copy::job_detail::MISSING_DEST, false);
                    }
                }
            }
        });
    }

    /// The live panel, while this job is running.
    ///
    /// The same one the dashboard shows — per-destination bars, the file and
    /// byte counts, the throughput graph — rather than a second, lesser
    /// rendering of the same run. A job page that went quiet the moment the
    /// job started would be the one page you would want open at that moment.
    fn job_detail_running(&mut self, ui: &mut Ui, id: Uuid, now: DateTime<Utc>) {
        let running: Vec<superbackup_core::state::JobRun> =
            self.data.active_runs().iter().filter(|r| r.job_id == id).cloned().collect();
        if running.is_empty() {
            return;
        }
        let mut stop: Option<(Uuid, String)> = None;
        for run in &running {
            self.run_panel(ui, run, now, &mut stop);
            ui.add_space(space::XL);
        }
        if let Some((run_id, name)) = stop {
            self.open_modal(crate::gui::modals::Modal::Confirm(
                crate::gui::modals::stop_run_confirm(run_id, &name),
            ));
        }
    }

    /// The last few runs, newest first.
    fn job_detail_runs(&mut self, ui: &mut Ui, id: Uuid, now: DateTime<Utc>) {
        let t = theme::tokens(ui.ctx());
        let mut runs: Vec<JobRun> =
            self.data.history.iter().filter(|r| r.job_id == id).cloned().collect();
        runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        let total = runs.len();
        runs.truncate(RECENT_RUNS);

        widgets::section_header(ui, copy::job_detail::RUNS, Some(total), |_| {});
        ui.add_space(space::M);

        if runs.is_empty() {
            widgets::paragraph(ui, copy::job_detail::NO_RUNS, Type::Small, t.text_muted);
            return;
        }

        let mut open: Option<Uuid> = None;
        widgets::table_frame(ui, |ui| {
            for run in &runs {
                let response = widgets::row_card(ui, None, |ui: &mut Ui| {
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            Vec2::new(150.0, 20.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                widgets::text(
                                    ui,
                                    format::relative(run.started_at, now),
                                    Type::Small,
                                    t.text_primary,
                                );
                            },
                        );
                        ui.allocate_ui_with_layout(
                            Vec2::new(170.0, 20.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                widgets::status_badge(ui, run.status);
                            },
                        );
                        widgets::text(
                            ui,
                            run_line(run),
                            Type::MonoSmall,
                            t.text_muted,
                        );
                    });
                });
                if response.response.clicked() {
                    open = Some(run.run_id);
                }
            }
        });
        if let Some(run_id) = open {
            self.go(Route::RunDetail(run_id));
        }

        if total > runs.len() {
            ui.add_space(space::M);
            if widgets::link(ui, copy::job_detail::ALL_RUNS).clicked() {
                self.screens.activity.filter_job(id);
                self.go(Route::Activity);
            }
        }
    }

    /// What this job has said for itself, from the activity log.
    fn job_detail_events(&mut self, ui: &mut Ui, id: Uuid, now: DateTime<Utc>) {
        let t = theme::tokens(ui.ctx());
        let events: Vec<_> = self
            .data
            .events
            .iter()
            .filter(|e| e.job_id == Some(id))
            .take(12)
            .cloned()
            .collect();

        widgets::section_header(ui, copy::job_detail::ACTIVITY, Some(events.len()), |_| {});
        ui.add_space(space::M);
        if events.is_empty() {
            widgets::paragraph(ui, copy::job_detail::NO_ACTIVITY, Type::Small, t.text_muted);
            return;
        }
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            for event in &events {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(120.0, 18.0),
                        Layout::left_to_right(Align::Min),
                        |ui| {
                            widgets::text(
                                ui,
                                format::relative(event.at, now),
                                Type::MonoSmall,
                                t.text_muted,
                            );
                        },
                    );
                    let colour = match event.severity {
                        superbackup_core::state::Severity::Error => t.danger.tint_text,
                        superbackup_core::state::Severity::Warning => t.warning.tint_text,
                        _ => t.text_secondary,
                    };
                    widgets::paragraph_at(
                        ui,
                        event.message.clone(),
                        Type::Small,
                        colour,
                        ui.available_width().max(120.0),
                    );
                });
                ui.add_space(space::XS);
            }
        });
        if widgets::link(ui, copy::job_detail::ALL_ACTIVITY).clicked() {
            self.screens.activity.filter_job(id);
            self.go(Route::Activity);
        }
    }
}

/// One run in a line: how much moved, and how long it took.
fn run_line(run: &JobRun) -> String {
    let mut parts = Vec::new();
    let uploaded: u64 = run.destinations.iter().map(|d| d.progress.bytes_uploaded).sum();
    parts.push(format!("{} up", format::bytes(uploaded)));
    if let Some(finished) = run.finished_at {
        let seconds = (finished - run.started_at).num_seconds().max(0);
        parts.push(format::duration(seconds));
    }
    let failed = run.destinations.iter().filter(|d| d.status == RunStatus::Failed).count();
    if failed > 0 {
        parts.push(format!("{failed} destination(s) failed"));
    }
    parts.join(" · ")
}
