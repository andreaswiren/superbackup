//! `D-1`. Answers, in order: is everything fine, what is happening right now,
//! when does it next run, and what do I do about it.

use chrono::Utc;
use egui::{Align, Layout, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::model::Job;
use superbackup_core::state::{JobRun, RunStatus};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::data::Action;
use crate::gui::format;
use crate::gui::icons::{self, Icon};
use crate::gui::modals::{self, Modal};
use crate::gui::nav::Route;
use crate::gui::theme::{self, radius, size, space, Type};
use crate::gui::viewmodel::{self, CardState};
use crate::gui::widgets::{self, Button};

#[derive(Default)]
pub struct State {
    /// The day column the pointer is over, so the tooltip can name it.
    pub hovered_day: Option<usize>,
}

impl App {
    pub(crate) fn dashboard_actions(&mut self, ui: &mut Ui) {
        let gate = self.data.gate(Action::RunJob);
        let has_jobs = self.data.jobs.iter().any(|j| j.enabled);
        let mut run_all = false;
        let mut menu: Option<&'static str> = None;

        widgets::overflow_menu(ui, "dash-menu", "More dashboard actions", |ui| {
            if widgets::menu_item(ui, copy::dash::JOBS_RUN_ALL, gate.allowed() && has_jobs) {
                menu = Some("run-all");
            }
            if widgets::menu_item(ui, copy::dash::JOBS_DISABLE_ALL, has_jobs) {
                menu = Some("disable-all");
            }
            if widgets::menu_item(ui, copy::dash::JOBS_NEW, true) {
                menu = Some("new-job");
            }
        });

        let mut button = Button::primary(copy::action::BACK_UP_NOW).icon(Icon::Play);
        if let Some(reason) = gate.reason() {
            button = button.disabled_because(reason);
        } else if !has_jobs {
            button = button.enabled(false);
        }
        if button.show(ui).clicked() {
            run_all = true;
        }

        if run_all {
            self.request_run_all();
        }
        match menu {
            Some("run-all") => self.request_run_all(),
            Some("disable-all") => {
                self.open_modal(Modal::Confirm(
                    modals::Confirm::new(
                        copy::dash::JOBS_DISABLE_ALL,
                        "Every job stops running on its schedule until you switch it back on.",
                        copy::dash::JOBS_DISABLE_ALL,
                    )
                    .safe()
                    .action(modals::ConfirmAction::DisableAllJobs),
                ));
            }
            Some("new-job") => {
                let wizard = crate::gui::screens::wizard::WizardState::new(&self.data);
                self.open_modal(Modal::Wizard(Box::new(wizard)));
            }
            _ => {}
        }
    }

    pub(crate) fn show_dashboard(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();

        if self.data.jobs.is_empty() && !self.data.loading {
            let (primary, secondary) =
                widgets::empty_state(ui, Icon::Repeat, &copy::empty::JOBS, None);
            if primary {
                let wizard = crate::gui::screens::wizard::WizardState::new(&self.data);
                self.open_modal(Modal::Wizard(Box::new(wizard)));
            }
            if secondary {
                self.go(Route::Settings(crate::gui::nav::SettingsSection::Remote));
            }
            return;
        }

        widgets::scroll_area(ui, "dashboard", |ui| {
                self.health_strip(ui, now);
                ui.add_space(space::XL);

                if !self.data.active_runs().is_empty() {
                    self.active_runs_section(ui, now);
                    ui.add_space(space::XL);
                }

                let count = self.data.jobs.len();
                let mut new_job = false;
                widgets::section_header(ui, copy::dash::JOBS_TITLE, Some(count), |ui| {
                    if Button::ghost(copy::dash::JOBS_NEW).icon(Icon::Plus).compact().show(ui).clicked()
                    {
                        new_job = true;
                    }
                });
                if new_job {
                    let wizard = crate::gui::screens::wizard::WizardState::new(&self.data);
                    self.open_modal(Modal::Wizard(Box::new(wizard)));
                }
                ui.add_space(space::L);
            self.job_grid(ui, now);
            ui.add_space(space::XL);
            let _ = t;
        });
    }

    // -- health strip -------------------------------------------------------

    fn health_strip(&mut self, ui: &mut Ui, now: chrono::DateTime<Utc>) {
        let t = theme::tokens(ui.ctx());
        let total = ui.available_width();
        let tile = ((total - 2.0 * space::XL) / 3.0).floor();
        let health = self.data.health();
        let mut action: Option<&'static str> = None;

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = space::XL;

            // Tile 1 — overall health.
            let failed = health == superbackup_core::state::Health::Failed;
            ui.allocate_ui_with_layout(
                Vec2::new(tile, 88.0),
                Layout::top_down(Align::Min),
                |ui| {
                    widgets::card_tinted(
                        ui,
                        failed.then_some(t.danger.tint_bg),
                        failed.then(|| theme::alpha(t.danger.mark, 0.4)),
                        |ui| {
                            ui.set_height(56.0);
                            ui.horizontal(|ui| {
                                let (rect, response) =
                                    ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                                let spin = ui.input(|i| i.time as f32);
                                let status = t.status_for_health(health);
                                icons::health_mark(
                                    ui.painter(),
                                    rect,
                                    health,
                                    t.neutral.mark,
                                    Some((
                                        status.mark,
                                        if t.dark { t.bg_canvas } else { egui::Color32::WHITE },
                                    )),
                                    spin,
                                );
                                let reason = self.data.health_reason(now);
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Label,
                                        true,
                                        copy::a11y_health(health.title(), &reason),
                                    )
                                });
                                ui.add_space(space::L);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = space::XS;
                                    widgets::text(ui, health.title(), Type::H2, t.text_primary);
                                    let width = (ui.available_width() - 90.0).max(80.0);
                                    widgets::elided(
                                        ui,
                                        &reason,
                                        Type::Small,
                                        t.text_secondary,
                                        width,
                                        false,
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    match health {
                                        superbackup_core::state::Health::Paused => {
                                            if Button::primary(copy::set::PAUSE_RESUME)
                                                .compact()
                                                .show(ui)
                                                .clicked()
                                            {
                                                action = Some("resume");
                                            }
                                        }
                                        superbackup_core::state::Health::Failed => {
                                            if Button::secondary(copy::dash::VIEW_ERROR)
                                                .compact()
                                                .show(ui)
                                                .clicked()
                                            {
                                                action = Some("view-error");
                                            }
                                        }
                                        superbackup_core::state::Health::Attention
                                            if !self.data.unlocked() =>
                                        {
                                            if Button::primary(copy::action::UNLOCK)
                                                .compact()
                                                .show(ui)
                                                .clicked()
                                            {
                                                action = Some("unlock");
                                            }
                                        }
                                        _ => {}
                                    }
                                });
                            });
                        },
                    );
                },
            );

            // Tile 2 — next scheduled run.
            ui.allocate_ui_with_layout(
                Vec2::new(tile, 88.0),
                Layout::top_down(Align::Min),
                |ui| {
                    widgets::card(ui, |ui| {
                        ui.set_height(56.0);
                        ui.spacing_mut().item_spacing.y = space::XS;
                        widgets::text(ui, copy::dash::NEXT_LABEL, Type::Micro, t.text_muted);
                        match self.data.next_scheduled() {
                            Some((job_id, at)) => {
                                let blocked = if !self.data.unlocked() {
                                    Some(copy::locked::NEXT_RUN)
                                } else if self.data.paused() {
                                    Some(copy::locked::PAUSED_NEXT_RUN)
                                } else {
                                    None
                                };
                                let value = format::relative_future(at, now);
                                let colour =
                                    if blocked.is_some() { t.text_muted } else { t.text_primary };
                                let response =
                                    widgets::text(ui, &value, Type::H2, colour);
                                if blocked.is_some() {
                                    // Struck through, so a blocked schedule is
                                    // not merely a quieter shade of normal.
                                    let r = response.rect;
                                    ui.painter().line_segment(
                                        [
                                            egui::Pos2::new(r.left(), r.center().y),
                                            egui::Pos2::new(r.right(), r.center().y),
                                        ],
                                        Stroke::new(1.0_f32, t.text_muted),
                                    );
                                }
                                let name = self.data.job_name(&job_id);
                                let sub = match blocked {
                                    Some(reason) => reason.to_string(),
                                    None => copy::dash_next_value(&name, &format::absolute(at)),
                                };
                                let width = ui.available_width();
                                widgets::elided(
                                    ui,
                                    &sub,
                                    Type::Small,
                                    if blocked.is_some() { t.warning.tint_text } else { t.text_muted },
                                    width,
                                    false,
                                );
                            }
                            None => {
                                widgets::text(ui, copy::dash::NEXT_NONE, Type::H2, t.text_primary);
                                if widgets::link(ui, copy::dash::NEXT_NONE_ACTION).clicked() {
                                    action = Some("schedule");
                                }
                            }
                        }
                    });
                },
            );

            // Tile 3 — the last seven days.
            ui.allocate_ui_with_layout(
                Vec2::new(tile, 88.0),
                Layout::top_down(Align::Min),
                |ui| {
                    widgets::card(ui, |ui| {
                        ui.set_height(56.0);
                        ui.spacing_mut().item_spacing.y = space::XS;
                        widgets::text(ui, copy::dash::WEEK_LABEL, Type::Micro, t.text_muted);
                        let days = self.data.last_seven_days(now);
                        let uploaded: u64 = days.iter().map(|d| d.uploaded).sum();
                        let runs: usize = days.iter().map(|d| d.total()).sum();
                        let failed: usize = days.iter().map(|d| d.failed).sum();
                        // The tile is the narrowest thing on the dashboard, so
                        // the strip is sized to what is left rather than to a
                        // fixed column width that would push the totals out of
                        // the card. Below 210px the totals drop and the strip
                        // keeps the tile, exactly as the reflow rules ask.
                        let inner = ui.available_width();
                        let totals = if inner >= 210.0 { 92.0 } else { 0.0 };
                        let strip = (inner - totals - if totals > 0.0 { space::L } else { 0.0 })
                            .clamp(84.0, 200.0);
                        ui.horizontal(|ui| {
                            self.week_strip(ui, &days, strip);
                            if totals > 0.0 {
                                ui.add_space(space::L);
                                ui.allocate_ui_with_layout(
                                    Vec2::new(totals, 44.0),
                                    Layout::top_down(Align::Min),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.y = space::XXS;
                                        if runs == 0 {
                                            widgets::elided(
                                                ui,
                                                copy::dash::WEEK_NONE,
                                                Type::Small,
                                                t.text_muted,
                                                totals,
                                                false,
                                            );
                                        } else {
                                            widgets::elided(
                                                ui,
                                                &format::bytes(uploaded),
                                                Type::H2,
                                                t.text_primary,
                                                totals,
                                                false,
                                            );
                                            widgets::elided(
                                                ui,
                                                &copy::dash_week_summary(runs, failed),
                                                Type::Small,
                                                t.text_muted,
                                                totals,
                                                false,
                                            );
                                        }
                                    },
                                );
                            }
                        });
                    });
                },
            );
        });

        match action {
            Some("resume") => self.resume(),
            Some("unlock") => {
                self.open_modal(Modal::Unlock(crate::gui::modals::UnlockState::voluntary()))
            }
            Some("view-error") => {
                let failed = self
                    .data
                    .history
                    .iter()
                    .find(|r| r.status == RunStatus::Failed)
                    .map(|r| r.run_id);
                match failed {
                    Some(id) => self.go(Route::RunDetail(id)),
                    None => self.go(Route::Activity),
                }
            }
            Some("schedule") => {
                if let Some(job) = self.data.jobs.first() {
                    let id = job.id;
                    self.screens.job_editor.open_tab(2);
                    self.go(Route::JobEditor(id));
                }
            }
            _ => {}
        }
    }

    fn week_strip(&mut self, ui: &mut Ui, days: &[crate::gui::data::DayOutcome], width: f32) {
        let t = theme::tokens(ui.ctx());
        let count = days.len().max(1) as f32;
        let gap = 4.0;
        let column = ((width - gap * (count - 1.0)) / count).clamp(8.0, 24.0);
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(column * count + gap * (count - 1.0), 44.0),
            Sense::hover(),
        );
        let max = days.iter().map(|d| d.total()).max().unwrap_or(0).max(1) as f32;
        for (index, day) in days.iter().enumerate() {
            let x = rect.left() + index as f32 * (column + gap);
            let bar = Rect::from_min_size(egui::Pos2::new(x, rect.top()), Vec2::new(column, 28.0));
            let response = ui.interact(
                bar,
                egui::Id::new("sb-week").with(index),
                Sense::hover(),
            );
            // A day with nothing in it is a track, not an empty gap.
            ui.painter().rect_filled(bar, egui::CornerRadius::same(2), t.progress_track);
            let mut bottom = bar.bottom();
            for (count, colour) in [
                (day.failed, t.danger.mark),
                (day.warned, t.warning.mark),
                (day.succeeded, t.success.mark),
            ] {
                if count == 0 {
                    continue;
                }
                let height = (count as f32 / max * 28.0).max(4.0);
                let segment = Rect::from_min_max(
                    egui::Pos2::new(bar.left(), bottom - height),
                    egui::Pos2::new(bar.right(), bottom),
                );
                ui.painter().rect_filled(segment, egui::CornerRadius::same(2), colour);
                bottom -= height + 1.0;
            }
            let initial = format::WEEKDAY_INITIAL
                [day.date.format("%u").to_string().parse::<usize>().unwrap_or(1) - 1];
            let g = widgets::galley(ui, initial, Type::Micro, t.text_muted);
            let w = g.size().x;
            ui.painter().galley(
                egui::Pos2::new(bar.center().x - w / 2.0, bar.bottom() + 4.0),
                g,
                t.text_muted,
            );
            let tooltip = copy::dash_week_day_tooltip(
                &format::absolute(day.date),
                day.succeeded,
                day.warned,
                day.failed,
            );
            response.on_hover_text(tooltip);
        }
    }

    // -- active runs --------------------------------------------------------

    fn active_runs_section(&mut self, ui: &mut Ui, now: chrono::DateTime<Utc>) {
        let runs: Vec<JobRun> = self.data.active_runs().to_vec();
        let mut stop_all = false;
        widgets::section_header(ui, copy::dash::RUNNING_TITLE, Some(runs.len()), |ui| {
            if Button::danger_ghost(copy::dash::RUNNING_STOP_ALL)
                .compact()
                .show(ui)
                .clicked()
            {
                stop_all = true;
            }
        });
        ui.add_space(space::L);

        let mut stop: Option<(Uuid, String)> = None;
        for run in &runs {
            self.run_panel(ui, run, now, &mut stop);
            ui.add_space(space::XL);
        }

        if stop_all {
            let names: Vec<String> = runs.iter().map(|r| r.job_name.clone()).collect();
            self.open_modal(Modal::Confirm(modals::stop_all_confirm(&names)));
        }
        if let Some((run_id, name)) = stop {
            self.open_modal(Modal::Confirm(modals::stop_run_confirm(run_id, &name)));
        }
    }

    fn run_panel(
        &mut self,
        ui: &mut Ui,
        run: &JobRun,
        now: chrono::DateTime<Utc>,
        stop: &mut Option<(Uuid, String)>,
    ) {
        let t = theme::tokens(ui.ctx());
        widgets::card_tinted(ui, None, Some(t.border_control), |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = space::S;

            // Row 1: name, badge, stop.
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                let turns = ui.input(|i| i.time as f32) * 0.75;
                Icon::RefreshCw.paint_rotated(ui.painter(), rect, t.info.mark, turns);
                ui.add_space(space::M);
                let name = if run.job_name.is_empty() {
                    copy::state::UNKNOWN.to_string()
                } else {
                    run.job_name.clone()
                };
                widgets::text(ui, &name, Type::H2, t.text_primary);
                ui.add_space(space::L);
                widgets::status_badge(ui, run.status);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::secondary(copy::action::STOP)
                        .icon(Icon::Square)
                        .compact()
                        .show(ui)
                        .clicked()
                    {
                        *stop = Some((run.run_id, name.clone()));
                    }
                });
            });

            // Row 2: when and why.
            widgets::text(
                ui,
                copy::dash_running_started(
                    &format::relative_past(run.started_at, now),
                    copy::trigger(run.trigger),
                ),
                Type::Small,
                t.text_muted,
            );

            // Row 3: the aggregate bar.
            ui.add_space(space::XS);
            let fraction = run.overall_fraction();
            let files_done: u64 = run.destinations.iter().map(|d| d.progress.files_processed).sum();
            // Summed, not maxed: the same source is written to every
            // destination, so `250,800 of 120,000 files` is the shape of a lie.
            let files_total: u64 =
                run.destinations.iter().filter_map(|d| d.progress.files_total).sum();
            let bytes_done: u64 = run.destinations.iter().map(|d| d.progress.bytes_processed).sum();
            let bytes_total: u64 =
                run.destinations.iter().filter_map(|d| d.progress.bytes_total).sum();
            let rate: f64 = run.destinations.iter().map(|d| d.progress.bytes_per_second).sum();
            let skipped: u64 = run.destinations.iter().map(|d| d.progress.errors_ignored).sum();
            let fill = if skipped > 0 { t.progress_fill_warn } else { t.progress_fill };
            let a11y = match fraction {
                Some(f) => copy::a11y_progress(
                    &run.job_name,
                    &copy::dash::RUNNING_TITLE.to_lowercase(),
                    (f * 100.0) as i64,
                    files_done,
                    files_total,
                ),
                None => copy::a11y_progress_estimating(&run.job_name, ""),
            };
            widgets::progress_bar(ui, ui.available_width(), 8.0, fraction, fill, &a11y);

            // Row 4: the numbers.
            ui.horizontal(|ui| {
                let line = if fraction.is_none() {
                    copy::state::ESTIMATING.to_string()
                } else if bytes_total > 0 && files_total > 0 {
                    copy::dash_running_counts(files_done, files_total, bytes_done, bytes_total, rate)
                } else {
                    copy::dash_running_counts_partial(files_done, bytes_done, rate)
                };
                let response = widgets::text(ui, &line, Type::MonoSmall, t.text_secondary);
                let cached: u64 = run.destinations.iter().map(|d| d.progress.files_cached).sum();
                if cached > 0 {
                    response.on_hover_text(copy::dash_running_cached_tooltip(cached));
                }
                if let Some(eta) = run
                    .destinations
                    .iter()
                    .filter_map(|d| d.progress.estimated_seconds_remaining)
                    .max()
                {
                    widgets::text(
                        ui,
                        format!("· {}", copy::dash_running_eta(&format::eta(eta))),
                        Type::MonoSmall,
                        t.text_muted,
                    );
                }
                if skipped > 0 {
                    let label = format!("· {}", copy::dash_running_skipped(skipped));
                    if widgets::text(ui, &label, Type::MonoSmall, t.warning.tint_text)
                        .interact(Sense::click())
                        .clicked()
                    {
                        // The warnings themselves live in the run detail.
                    }
                }
            });

            // Row 5: what is being read right now. The scan happens once for
            // the whole fan-out, so this line belongs to the run.
            if let Some(path) = run
                .destinations
                .iter()
                .find_map(|d| d.progress.current_path.clone())
            {
                let width = ui.available_width();
                widgets::elided(
                    ui,
                    &copy::dash_running_scanning(&path),
                    Type::MonoSmall,
                    t.text_muted,
                    width,
                    true,
                );
            }

            ui.add_space(space::S);
            widgets::divider(ui);
            ui.add_space(space::L);

            // Per-destination rows. Never flattened into one number.
            for destination in &run.destinations {
                self.destination_progress_row(ui, destination);
                ui.add_space(space::XS);
            }
        });
    }

    fn destination_progress_row(
        &self,
        ui: &mut Ui,
        destination: &superbackup_core::state::DestinationRun,
    ) {
        let t = theme::tokens(ui.ctx());
        let narrow = ui.available_width() < 700.0;
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            ui.spacing_mut().item_spacing.x = space::L;

            let kind_icon = self
                .data
                .destination(&destination.destination_id)
                .map(|d| Icon::for_destination_kind(&d.kind))
                .unwrap_or(Icon::HardDrive);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            kind_icon.paint(ui.painter(), rect, t.text_muted);

            ui.allocate_ui_with_layout(
                Vec2::new(90.0, 20.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    widgets::elided(
                        ui,
                        &destination.destination_name,
                        Type::Small,
                        t.text_primary,
                        90.0,
                        false,
                    );
                },
            );

            let badge_w = 120.0;
            let bytes_w = if narrow { 0.0 } else { 120.0 };
            let bar_w = (ui.available_width() - badge_w - bytes_w - 40.0 - space::L * 3.0).max(60.0);
            let fraction = destination.progress.fraction();
            let fill = match destination.status {
                RunStatus::Failed => t.progress_fill_error,
                RunStatus::SucceededWithWarnings => t.progress_fill_warn,
                RunStatus::Succeeded => t.success.mark,
                _ if destination.progress.errors_ignored > 0 => t.progress_fill_warn,
                _ => t.progress_fill,
            };
            let a11y = copy::a11y_progress(
                "",
                &destination.destination_name,
                fraction.map(|f| (f * 100.0) as i64).unwrap_or(0),
                destination.progress.files_processed,
                destination.progress.files_total.unwrap_or(0),
            );
            widgets::progress_bar(ui, bar_w, 6.0, fraction, fill, &a11y);

            ui.allocate_ui_with_layout(
                Vec2::new(40.0, 20.0),
                Layout::right_to_left(Align::Center),
                |ui| match fraction {
                    Some(f) => {
                        widgets::text(ui, format::percent(f), Type::MonoSmall, t.text_secondary);
                    }
                    None => {
                        widgets::text(ui, "—", Type::MonoSmall, t.text_muted);
                    }
                },
            );

            if !narrow {
                ui.allocate_ui_with_layout(
                    Vec2::new(120.0, 20.0),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        widgets::text(
                            ui,
                            format!("{} up", format::bytes(destination.progress.bytes_uploaded)),
                            Type::MonoSmall,
                            t.text_muted,
                        );
                    },
                );
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                widgets::status_badge(ui, destination.status);
                if destination.error.is_some() {
                    let _ = widgets::link(ui, copy::dash::VIEW_ERROR);
                }
            });
        });
    }

    // -- job grid -----------------------------------------------------------

    fn job_grid(&mut self, ui: &mut Ui, now: chrono::DateTime<Utc>) {
        let two_columns = ui.available_width() >= 800.0;
        let gutter = space::XL;
        let card_width = if two_columns {
            ((ui.available_width() - gutter) / 2.0).floor()
        } else {
            ui.available_width()
        };

        let jobs: Vec<Job> = self.data.jobs.clone();
        let mut index = 0;
        while index < jobs.len() {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gutter;
                for _ in 0..if two_columns { 2 } else { 1 } {
                    if let Some(job) = jobs.get(index) {
                        ui.allocate_ui_with_layout(
                            Vec2::new(card_width, size::JOB_CARD_H),
                            Layout::top_down(Align::Min),
                            |ui| {
                                self.job_card(ui, job, now);
                            },
                        );
                        index += 1;
                    }
                }
            });
            ui.add_space(space::XL);
        }
    }

    fn job_card(&mut self, ui: &mut Ui, job: &Job, now: chrono::DateTime<Utc>) {
        let t = theme::tokens(ui.ctx());
        let view = viewmodel::job_view(&self.data, job, now);
        let status = t.status_for(view.status);
        let disabled = matches!(view.state, CardState::Disabled { .. });
        let alpha = if disabled { 0.6 } else { 1.0 };

        let mut open = false;
        let mut run = false;
        let mut enable = false;
        let mut view_error = false;
        let mut menu_action: Option<&'static str> = None;

        let frame = widgets::card(ui, |ui| {
            ui.set_height(size::JOB_CARD_H - 32.0);
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = space::S;

            ui.horizontal(|ui| {
                let width = (ui.available_width() - 190.0).max(80.0);
                widgets::elided(
                    ui,
                    &job.name,
                    Type::H2,
                    theme::alpha(t.text_primary, alpha),
                    width,
                    false,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    widgets::overflow_menu(ui, ("job-card", job.id), "More actions for this job", |ui| {
                        if widgets::menu_item(ui, copy::action::RUN_NOW, job.enabled) {
                            menu_action = Some("run");
                        }
                        if widgets::menu_item(ui, copy::action::EDIT, true) {
                            menu_action = Some("edit");
                        }
                        if widgets::menu_item(ui, "Browse snapshots…", true) {
                            menu_action = Some("browse");
                        }
                        if widgets::menu_item(ui, "View history", true) {
                            menu_action = Some("history");
                        }
                        if widgets::menu_item(
                            ui,
                            if job.enabled { copy::action::DISABLE } else { copy::action::ENABLE },
                            true,
                        ) {
                            menu_action = Some("toggle");
                        }
                        widgets::divider(ui);
                        if widgets::menu_item_danger(ui, copy::action::DELETE, true) {
                            menu_action = Some("delete");
                        }
                    });
                    match view.state {
                        CardState::Disabled { .. } => {
                            widgets::neutral_badge(ui, copy::badge::DISABLED, Some(Icon::Pause));
                        }
                        CardState::NeverRun => {
                            widgets::neutral_badge(ui, copy::badge::NEVER_RUN, None);
                        }
                        _ => {
                            widgets::status_badge(ui, view.status);
                        }
                    }
                });
            });

            let meta_colour = match view.state {
                CardState::Failed { .. } => t.danger.tint_text,
                CardState::Warnings { .. } | CardState::Stale => t.warning.tint_text,
                _ => t.text_muted,
            };
            let width = ui.available_width();
            widgets::elided(ui, &view.meta, Type::Small, theme::alpha(meta_colour, alpha), width, false);

            ui.add_space(space::XXS);
            match &view.state {
                CardState::Running { fraction, rate } => {
                    ui.horizontal(|ui| {
                        let bar_w = (ui.available_width() - 150.0).max(80.0);
                        widgets::progress_bar(
                            ui,
                            bar_w,
                            6.0,
                            *fraction,
                            t.progress_fill,
                            &copy::a11y_job_card_running(
                                &job.name,
                                fraction.map(|f| (f * 100.0) as i64).unwrap_or(0),
                            ),
                        );
                        ui.add_space(space::M);
                        let label = match fraction {
                            Some(f) => format!("{} · {}", format::percent(*f), format::rate(*rate)),
                            None => copy::state::ESTIMATING.to_string(),
                        };
                        widgets::text(ui, label, Type::MonoSmall, t.text_secondary);
                    });
                }
                _ => {
                    ui.horizontal(|ui| {
                        let reserved = 110.0;
                        let available = (ui.available_width() - reserved).max(60.0);
                        let mut used = 0.0;
                        for id in job.destination_ids.iter().take(4) {
                            let (icon, name) = match self.data.destination(id) {
                                Some(d) => (Icon::for_destination_kind(&d.kind), d.name.clone()),
                                None => (Icon::HardDrive, copy::state::UNKNOWN.to_string()),
                            };
                            let problem = self
                                .data
                                .history
                                .iter()
                                .find(|r| r.job_id == job.id)
                                .and_then(|r| {
                                    r.destinations.iter().find(|d| &d.destination_id == id)
                                })
                                .and_then(|d| match d.status {
                                    RunStatus::Failed => Some(t.danger),
                                    RunStatus::SucceededWithWarnings => Some(t.warning),
                                    _ => None,
                                });
                            let chip_max = 150.0_f32.min(available - used);
                            if chip_max < 60.0 {
                                widgets::count_pill(
                                    ui,
                                    &format!("+{}", job.destination_ids.len() as i64 - used as i64),
                                );
                                break;
                            }
                            let response =
                                widgets::destination_chip(ui, icon, &name, problem, chip_max);
                            used += response.rect.width() + space::M;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if disabled {
                                if Button::secondary(copy::action::ENABLE)
                                    .compact()
                                    .show(ui)
                                    .clicked()
                                {
                                    enable = true;
                                }
                            } else {
                                let gate = self.data.gate(Action::RunJob);
                                let mut button =
                                    Button::ghost(copy::action::RUN_NOW).compact().a11y(format!(
                                        "Run job \"{}\" now",
                                        job.name
                                    ));
                                if let Some(reason) = gate.reason() {
                                    button = button.disabled_because(reason);
                                }
                                if button.show(ui).clicked() {
                                    run = true;
                                }
                                if matches!(view.state, CardState::Failed { .. })
                                    && widgets::link(ui, copy::dash::VIEW_ERROR).clicked()
                                {
                                    view_error = true;
                                }
                            }
                        });
                    });
                }
            }
        });

        // The 3px status spine — the only place a status colour touches a card.
        let rect = frame.response.rect;
        ui.painter().rect_filled(
            Rect::from_min_size(rect.left_top(), Vec2::new(3.0, rect.height())),
            egui::CornerRadius { nw: 10, sw: 10, ne: 0, se: 0 },
            theme::alpha(status.mark, alpha),
        );
        // A project's colour sits beside it, with a 4px gap, and never alone:
        // the group header names the project in words.
        if job.project_id.is_some() {
            ui.painter().rect_filled(
                Rect::from_min_size(
                    rect.left_top() + Vec2::new(7.0, 0.0),
                    Vec2::new(2.0, rect.height()),
                ),
                0,
                theme::alpha(t.accent, alpha),
            );
        }

        let card = ui.interact(rect, egui::Id::new("job-card").with(job.id), Sense::click());
        if card.clicked() {
            open = true;
        }
        if card.hovered() {
            ui.painter().rect_stroke(
                rect,
                radius::CARD,
                Stroke::new(1.0_f32, t.border_control),
                StrokeKind::Inside,
            );
        }
        if card.has_focus() {
            widgets::focus_ring(ui, rect, radius::CARD);
        }
        let summary = self.data.summary_for(&job.id).unwrap_or_default();
        let announce = if disabled {
            copy::a11y_job_card_disabled(&job.name, &view.badge, &view.meta)
        } else {
            copy::a11y_job_card(
                &job.name,
                &view.badge,
                &summary
                    .last_run
                    .map(|t| format::relative_past(t, now))
                    .unwrap_or_else(|| copy::state::NEVER.to_lowercase()),
                &summary
                    .next_run
                    .map(|t| format::relative_future(t, now))
                    .unwrap_or_else(|| copy::state::NEVER.to_lowercase()),
                job.destination_ids.len(),
            )
        };
        card.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &announce)
        });

        if run {
            self.request_run(job);
        }
        if enable {
            self.ask(
                crate::gui::daemon::Intent::SaveJob(job.name.clone()),
                superbackup_core::ipc::protocol::Request::JobSetEnabled {
                    job: job.id.to_string(),
                    enabled: true,
                },
            );
        }
        if view_error {
            let failed = self
                .data
                .history
                .iter()
                .find(|r| r.job_id == job.id && r.status == RunStatus::Failed)
                .map(|r| r.run_id);
            match failed {
                Some(id) => self.go(Route::RunDetail(id)),
                None => self.go(Route::Activity),
            }
        }
        if open {
            self.go(Route::JobEditor(job.id));
        }
        match menu_action {
            Some("run") => self.request_run(job),
            Some("edit") => self.go(Route::JobEditor(job.id)),
            Some("browse") => self.go(Route::Restore),
            Some("history") => {
                self.screens.activity.filter_job(job.id);
                self.go(Route::Activity);
            }
            Some("toggle") => {
                self.ask(
                    crate::gui::daemon::Intent::SaveJob(job.name.clone()),
                    superbackup_core::ipc::protocol::Request::JobSetEnabled {
                        job: job.id.to_string(),
                        enabled: !job.enabled,
                    },
                );
            }
            Some("delete") => {
                self.open_modal(Modal::Confirm(modals::delete_job_confirm(Some(job))));
            }
            _ => {}
        }
    }
}
