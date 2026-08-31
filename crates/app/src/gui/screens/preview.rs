//! The dry run, made visible.
//!
//! The engine has supported rehearsals end to end for some time —
//! `RunRequest::dry_run` reaches the mirror engine and `kopia snapshot
//! estimate` — and none of it was reachable from the window. This screen is
//! that half.
//!
//! Three rules shape it, and each exists because the alternative misleads:
//!
//! 1. **One card per destination, never a merged total.** A job that would
//!    reach two destinations out of three has not backed up, and a single
//!    added-up number is exactly what hides that. Same rule as the run detail
//!    screen, for the same reason.
//! 2. **Say what is not knowable.** A repository rehearsal is `kopia snapshot
//!    estimate`, which reports the whole source and nothing about what kopia
//!    already holds; a mirror rehearsal knows precisely. Neither keeps the
//!    list of files it counted. The screen says so in words rather than
//!    rendering an empty list that reads like "nothing would be copied".
//! 3. **Nothing was written, unmistakably.** A banner at the top, the word in
//!    the title, and the trigger recorded on the run itself so the Activity
//!    history says it too, months later.

use egui::{Align, Layout, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::ipc::protocol::ErrorPayload;

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::nav::Route;
use crate::gui::theme::{self, space, Type};
use crate::gui::viewmodel::{self, ChangeSplit, PathListing, PreviewReport};
use crate::gui::widgets::{self, Button};

/// What the screen knows about the rehearsal it is showing.
#[derive(Default)]
pub struct State {
    /// The job being rehearsed, and the name to show before the daemon has
    /// answered.
    pub job: Option<(Uuid, String)>,
    /// The run the daemon accepted, once it has. Everything else is read from
    /// `Data` by this id, so the screen has no second copy of the run to keep
    /// in step.
    pub run_id: Option<Uuid>,
    /// True between asking and the daemon accepting.
    pub requested: bool,
    pub error: Option<String>,
}

impl State {
    /// A preview was asked for.
    pub fn started(&mut self, job: Uuid, name: String) {
        self.job = Some((job, name));
        self.run_id = None;
        self.requested = true;
        self.error = None;
    }

    /// The daemon accepted it and named the run.
    pub fn accepted(&mut self, job: Uuid, run_id: Uuid) {
        if self.job.as_ref().is_none_or(|(id, _)| *id == job) {
            self.run_id = Some(run_id);
        }
        self.requested = false;
    }

    pub fn failed(&mut self, job: Uuid, payload: ErrorPayload) {
        if self.job.as_ref().is_none_or(|(id, _)| *id == job) {
            self.error = Some(payload.message);
        }
        self.requested = false;
    }

    /// True while the interface is waiting, so the app keeps asking for
    /// frames.
    pub fn busy(&self) -> bool {
        self.requested
    }
}

impl App {
    pub(crate) fn show_preview(&mut self, ui: &mut Ui, job_id: Uuid) {
        let t = theme::tokens(ui.ctx());
        let Some(job) = self.data.job(&job_id).cloned() else {
            widgets::banner(
                ui,
                widgets::BannerKind::Warning,
                copy::err::JOB_NOT_FOUND,
                None,
                |_| {},
            );
            return;
        };

        // The run record is the single source of truth, live or finished, and
        // it is read fresh every frame rather than copied into this screen —
        // which is what makes the cards fill in as the rehearsal progresses.
        let report = self.screens.preview.run_id.and_then(|run_id| {
            self.data
                .active_runs()
                .iter()
                .chain(self.data.history.iter())
                .find(|r| r.run_id == run_id)
                .map(|run| viewmodel::preview_report(run, &self.data.destinations))
        });

        let mut rerun = false;
        let mut run_for_real = false;

        widgets::scroll_area(ui, ("preview", job_id), |ui| {
            widgets::banner(
                ui,
                widgets::BannerKind::Info,
                copy::preview::NOTHING_WRITTEN,
                Some(copy::preview::NOTHING_WRITTEN_BODY),
                |_| {},
            );
            ui.add_space(space::XL);

            // The header already carries "Preview of <job>"; repeating it here
            // would be the second-largest thing on screen saying what the
            // largest thing already said.
            ui.horizontal_top(|ui| {
                widgets::paragraph_at(
                    ui,
                    copy::preview::PER_DESTINATION,
                    Type::Small,
                    t.text_muted,
                    520.0,
                );
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if Button::primary(copy::preview::RUN_FOR_REAL).show(ui).clicked() {
                        run_for_real = true;
                    }
                    if Button::secondary(copy::preview::RERUN).icon(Icon::Repeat).show(ui).clicked()
                    {
                        rerun = true;
                    }
                });
            });
            ui.add_space(space::XL);

            if let Some(error) = self.screens.preview.error.clone() {
                widgets::banner(ui, widgets::BannerKind::Danger, &error, None, |_| {});
                return;
            }

            match &report {
                None => {
                    widgets::empty_state(
                        ui,
                        Icon::Search,
                        if self.screens.preview.requested {
                            &copy::empty::PREVIEW_WAITING
                        } else {
                            &copy::empty::PREVIEW_NONE
                        },
                        None,
                    );
                }
                Some(report) if report.rows.is_empty() => {
                    widgets::empty_state(ui, Icon::Search, &copy::empty::PREVIEW_WAITING, None);
                }
                Some(report) => {
                    if report.all_failed {
                        widgets::banner(
                            ui,
                            widgets::BannerKind::Danger,
                            copy::preview::NOT_REHEARSABLE,
                            None,
                            |_| {},
                        );
                        ui.add_space(space::L);
                    }
                    show_report(ui, report);
                }
            }
        });

        if rerun {
            self.request_preview(&job);
        }
        if run_for_real {
            self.request_run(&job);
            self.go(Route::Jobs);
        }
    }
}

/// One card per destination, in configuration order.
fn show_report(ui: &mut Ui, report: &PreviewReport) {
    for row in &report.rows {
        destination_card(ui, row);
        ui.add_space(space::L);
    }
}

fn destination_card(ui: &mut Ui, row: &viewmodel::PreviewRow) {
    let t = theme::tokens(ui.ctx());
    widgets::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            widgets::text(ui, &row.destination_name, Type::BodyStrong, t.text_primary);
            ui.add_space(space::M);
            widgets::status_badge(ui, row.status);
            if let Some(source) = &row.replicated_from {
                ui.add_space(space::M);
                widgets::neutral_badge(ui, &format!("copy of {source}"), Some(Icon::Copy));
            }
        });

        if row.failed {
            ui.add_space(space::M);
            widgets::text(ui, copy::preview::NOT_REHEARSABLE, Type::Small, t.danger.mark);
            for note in &row.notes {
                ui.add_space(space::XS);
                widgets::paragraph_at(ui, note, Type::Small, t.text_secondary, 600.0);
            }
            return;
        }

        ui.add_space(space::L);
        let column = ((ui.available_width() - space::XL * 2.0) / 3.0).clamp(160.0, 260.0);
        ui.horizontal_top(|ui| {
            figure(ui, column, copy::preview::FILES, &format::count(row.files));
            figure(ui, column, copy::preview::TOTAL_SIZE, &format::bytes(row.bytes));
            match row.split {
                ChangeSplit::Known { would_copy_files, would_copy_bytes, .. } => figure(
                    ui,
                    column,
                    copy::preview::WOULD_COPY,
                    &format!(
                        "{} · {}",
                        format::count(would_copy_files),
                        format::bytes(would_copy_bytes)
                    ),
                ),
                // Not a zero and not a blank: a sentence saying the figure
                // does not exist.
                ChangeSplit::Unknown => {
                    figure(ui, column, copy::preview::WOULD_COPY, copy::state::UNKNOWN)
                }
            }
        });

        match row.split {
            ChangeSplit::Known { unchanged_files, .. } => {
                ui.add_space(space::S);
                widgets::text(
                    ui,
                    format!("{}: {}", copy::preview::UNCHANGED, format::count(unchanged_files)),
                    Type::Small,
                    t.text_secondary,
                );
            }
            ChangeSplit::Unknown => {
                ui.add_space(space::S);
                widgets::paragraph_at(
                    ui,
                    copy::preview::UNKNOWN_SPLIT,
                    Type::Small,
                    t.text_muted,
                    600.0,
                );
            }
        }

        ui.add_space(space::L);
        match &row.listing {
            PathListing::Unavailable(reason) => {
                widgets::text(ui, copy::preview::NO_PATHS, Type::Small, t.text_secondary);
                ui.add_space(space::XS);
                widgets::paragraph_at(ui, *reason, Type::Small, t.text_muted, 600.0);
            }
            PathListing::Available => {
                for path in &row.paths {
                    widgets::text(ui, path, Type::MonoSmall, t.text_secondary);
                }
                if row.more_paths > 0 {
                    ui.add_space(space::XS);
                    widgets::text(
                        ui,
                        copy::preview_and_more(row.more_paths),
                        Type::Small,
                        t.text_muted,
                    );
                }
            }
        }

        if !row.notes.is_empty() {
            ui.add_space(space::M);
            for note in &row.notes {
                widgets::paragraph_at(ui, note, Type::Small, t.text_muted, 600.0);
            }
        }
    });
}

/// A label above a number, in a fixed-width column so three of them line up
/// rather than crowding together at the left edge.
///
/// `set_min_width` inside the allocation is the load-bearing part: an
/// `allocate_ui_with_layout` shrinks to its content, so without it the columns
/// are only as wide as the shortest figure and the three read as one run-on
/// sentence.
fn figure(ui: &mut Ui, width: f32, label: &str, value: &str) {
    let t = theme::tokens(ui.ctx());
    ui.allocate_ui_with_layout(Vec2::new(width, 48.0), Layout::top_down(Align::Min), |ui| {
        ui.set_min_width(width);
        widgets::text(ui, label, Type::Small, t.text_muted);
        ui.add_space(space::XS);
        widgets::text(ui, value, Type::H3, t.text_primary);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_for_a_different_job_does_not_overwrite_the_screen() {
        let mine = Uuid::new_v4();
        let other = Uuid::new_v4();
        let mut state = State::default();
        state.started(mine, "Dev folders".into());

        // A stale reply for a preview the user has already navigated away
        // from must not attach its run id to the job now on screen — the
        // cards would fill with another job's figures.
        state.accepted(other, Uuid::new_v4());
        assert!(state.run_id.is_none());

        let run = Uuid::new_v4();
        state.accepted(mine, run);
        assert_eq!(state.run_id, Some(run));
        assert!(!state.busy());
    }

    #[test]
    fn starting_a_second_preview_clears_the_first_ones_result() {
        let job = Uuid::new_v4();
        let mut state = State::default();
        state.started(job, "Dev folders".into());
        state.accepted(job, Uuid::new_v4());
        state.started(job, "Dev folders".into());
        assert!(state.run_id.is_none(), "stale figures must not linger under a spinner");
        assert!(state.busy());
    }

    #[test]
    fn a_failure_is_remembered_and_clears_the_waiting_state() {
        let job = Uuid::new_v4();
        let mut state = State::default();
        state.started(job, "Dev folders".into());
        state.failed(
            job,
            ErrorPayload::new(
                superbackup_core::error::ErrorCode::Validation,
                "no usable destination",
            ),
        );
        assert!(!state.busy());
        assert!(state.error.as_deref().is_some_and(|e| e.contains("destination")));
    }
}
