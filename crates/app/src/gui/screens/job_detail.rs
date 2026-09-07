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
use egui::{Align, Layout, Sense, Ui, Vec2};
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

/// The snapshot table's geometry. Shared by its header and its rows, and kept
/// by `fixed_cell` rather than requested and quietly given up.
const SNAPSHOT_HEADER_H: f32 = 18.0;
const SNAPSHOT_ROW_H: f32 = 26.0;
const SNAP_WHEN_W: f32 = 150.0;
const SNAP_SIZE_W: f32 = 90.0;
const SNAP_FILES_W: f32 = 90.0;

/// What the job's own snapshot list is showing.
///
/// Kept per job rather than globally: opening one job's page and then
/// another's must not leave the second showing the first's snapshots while it
/// waits, which is the shape of bug where somebody restores from the wrong
/// backup.
#[derive(Default)]
pub struct State {
    /// The job whose snapshots `snapshots` holds.
    pub job: Option<Uuid>,
    /// Which of the job's destinations is being listed.
    pub destination: Option<Uuid>,
    pub snapshots: Vec<superbackup_core::ipc::protocol::SnapshotInfo>,
    pub loading: bool,
    pub error: Option<String>,
    /// Free text over the date, the time and the snapshot id.
    pub search: String,
}

impl State {
    /// Switching jobs clears the list rather than leaving the last one on
    /// screen under a new heading.
    pub fn focus(&mut self, job: Uuid) -> bool {
        if self.job == Some(job) {
            return false;
        }
        *self = State { job: Some(job), ..State::default() };
        true
    }

    pub fn requested(&mut self, destination: Uuid) {
        self.destination = Some(destination);
        self.loading = true;
        self.error = None;
    }

    pub fn arrived(
        &mut self,
        snapshots: Vec<superbackup_core::ipc::protocol::SnapshotInfo>,
    ) {
        self.snapshots = snapshots;
        self.loading = false;
        self.error = None;
    }

    pub fn failed(&mut self, why: String) {
        self.loading = false;
        self.error = Some(why);
    }

    /// The snapshots matching the search box.
    ///
    /// Matched against the *rendered* local date and time as well as the id,
    /// so typing "2026-08-15" or "14:" finds what the user is looking at
    /// rather than what the wire format happens to say.
    pub fn matching(&self) -> Vec<&superbackup_core::ipc::protocol::SnapshotInfo> {
        let needle = self.search.trim().to_lowercase();
        self.snapshots
            .iter()
            .filter(|s| {
                if needle.is_empty() {
                    return true;
                }
                let shown = crate::gui::format::absolute(s.created_at).to_lowercase();
                shown.contains(&needle) || s.id.to_lowercase().contains(&needle)
            })
            .collect()
    }
}

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
            self.job_detail_snapshots(ui, &job);
            ui.add_space(space::XL);
            self.job_detail_events(ui, id, now);
            ui.add_space(space::XL);
            let _ = t;
        });
    }

    /// The job's own snapshots, searchable, with a way into each.
    ///
    /// # Why here and not only in Restore
    ///
    /// Restore starts from a *destination* and asks you to work out which of
    /// its snapshots belong to the job you had in mind. Starting from the job
    /// is the other direction, and it is the one somebody has in mind when
    /// they are already looking at a job's page wondering what it holds.
    ///
    /// The list is asked for once per job per visit rather than on every
    /// frame: it is a call against remote storage, and a page that made one on
    /// every repaint would be unusable on exactly the setup it describes.
    fn job_detail_snapshots(&mut self, ui: &mut Ui, job: &superbackup_core::model::Job) {
        let t = theme::tokens(ui.ctx());

        // Which destination to list. The job's first usable one, unless the
        // user has picked another below.
        let usable: Vec<(Uuid, String)> = job
            .destination_ids
            .iter()
            .filter_map(|id| self.data.destination(id))
            .filter(|d| d.enabled && d.kind.is_repository())
            .map(|d| (d.id, d.name.clone()))
            .collect();

        widgets::section_header(
            ui,
            copy::job_detail::SNAPSHOTS,
            Some(self.screens.job_detail.snapshots.len()),
            |_| {},
        );
        ui.add_space(space::M);

        if usable.is_empty() {
            // A mirror has no snapshots to list, and saying "none" would read
            // as "this job has never run".
            widgets::paragraph(ui, copy::job_detail::SNAPSHOTS_NONE, Type::Small, t.text_muted);
            return;
        }

        // Fresh page, or a job switched under it: ask once.
        let switched = self.screens.job_detail.focus(job.id);
        let target = self
            .screens
            .job_detail
            .destination
            .filter(|id| usable.iter().any(|(d, _)| d == id))
            .unwrap_or(usable[0].0);
        if switched || self.screens.job_detail.destination.is_none() {
            self.ask_job_snapshots(job, target);
        }

        // Where from, when the job writes to more than one place. A
        // repository per destination means a different set of snapshots in
        // each, and "the job's snapshots" is not one list.
        let mut chosen = target;
        if usable.len() > 1 {
            let labels: Vec<&str> = usable.iter().map(|(_, name)| name.as_str()).collect();
            let mut index = usable.iter().position(|(id, _)| *id == target).unwrap_or(0);
            widgets::segmented(ui, &mut index, &labels);
            chosen = usable[index.min(usable.len() - 1)].0;
            ui.add_space(space::M);
        }

        widgets::Field::new()
            .width(280.0)
            .placeholder(copy::job_detail::SNAPSHOTS_SEARCH)
            .show(ui, &mut self.screens.job_detail.search);
        ui.add_space(space::M);

        if self.screens.job_detail.loading {
            widgets::spinner(ui, 18.0, t.accent);
            return;
        }
        if let Some(error) = self.screens.job_detail.error.clone() {
            widgets::paragraph(ui, error, Type::Small, t.danger.tint_text);
            return;
        }

        let matching = self.screens.job_detail.matching();
        if matching.is_empty() {
            let empty = if self.screens.job_detail.snapshots.is_empty() {
                copy::job_detail::SNAPSHOTS_EMPTY
            } else {
                copy::job_detail::SNAPSHOTS_NO_MATCH
            };
            widgets::paragraph(ui, empty, Type::Small, t.text_muted);
            return;
        }

        let mut restore: Option<String> = None;
        widgets::table_frame(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(SNAPSHOT_HEADER_H);
                widgets::fixed_cell(
                    ui,
                    SNAP_WHEN_W,
                    SNAPSHOT_HEADER_H,
                    Layout::left_to_right(Align::Center),
                    |ui| widgets::table_header(ui, copy::job_detail::COL_WHEN, None),
                );
                widgets::fixed_cell(
                    ui,
                    SNAP_SIZE_W,
                    SNAPSHOT_HEADER_H,
                    Layout::right_to_left(Align::Center),
                    |ui| widgets::table_header(ui, copy::job_detail::COL_SIZE, None),
                );
                widgets::fixed_cell(
                    ui,
                    SNAP_FILES_W,
                    SNAPSHOT_HEADER_H,
                    Layout::right_to_left(Align::Center),
                    |ui| widgets::table_header(ui, copy::job_detail::COL_FILES, None),
                );
                ui.add_space(space::M);
                let rest = ui.available_width().max(60.0);
                widgets::fixed_cell(
                    ui,
                    rest,
                    SNAPSHOT_HEADER_H,
                    Layout::left_to_right(Align::Center),
                    |ui| widgets::table_header(ui, copy::job_detail::COL_SNAPSHOT, None),
                );
            });
            widgets::divider(ui);

            for snapshot in matching {
                ui.horizontal(|ui| {
                    ui.set_min_height(SNAPSHOT_ROW_H + 6.0);
                    widgets::fixed_cell(
                        ui,
                        SNAP_WHEN_W,
                        SNAPSHOT_ROW_H,
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            widgets::text(
                                ui,
                                format::absolute(snapshot.created_at),
                                Type::Small,
                                t.text_primary,
                            )
                            .on_hover_text(format::absolute_zoned(snapshot.created_at));
                        },
                    );
                    widgets::fixed_cell(
                        ui,
                        SNAP_SIZE_W,
                        SNAPSHOT_ROW_H,
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            widgets::text(
                                ui,
                                snapshot
                                    .total_bytes
                                    .map(format::bytes)
                                    .unwrap_or_else(|| "—".to_string()),
                                Type::MonoSmall,
                                t.text_secondary,
                            );
                        },
                    );
                    widgets::fixed_cell(
                        ui,
                        SNAP_FILES_W,
                        SNAPSHOT_ROW_H,
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            widgets::text(
                                ui,
                                snapshot
                                    .file_count
                                    .map(format::count)
                                    .unwrap_or_else(|| "—".to_string()),
                                Type::MonoSmall,
                                t.text_muted,
                            );
                        },
                    );
                    ui.add_space(space::M);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Button::secondary(copy::job_detail::SNAPSHOT_OPEN)
                            .compact()
                            .show(ui)
                            .on_hover_text(copy::job_detail::SNAPSHOT_OPEN_HINT)
                            .clicked()
                        {
                            restore = Some(snapshot.id.clone());
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let room = ui.available_width().max(60.0);
                            widgets::elided(
                                ui,
                                &snapshot.id,
                                Type::MonoSmall,
                                t.text_muted,
                                room,
                                false,
                            );
                        });
                    });
                });
                widgets::divider(ui);
            }
        });

        if chosen != target {
            self.ask_job_snapshots(job, chosen);
        }
        if let Some(id) = restore {
            // Hand the restore browser the destination *and* the snapshot, so
            // it opens on the one that was clicked rather than on whatever it
            // was showing last — and then actually ask for its root listing.
            //
            // Setting the selection alone was not enough: the restore screen
            // requests a listing when *it* selects a snapshot, so arriving
            // pre-selected skipped the request and the browser sat on "Reading
            // directory…" for ever, reading nothing.
            self.screens.restore.select(target);
            // Both requests, because the restore screen makes each of them in
            // response to a *click it handled itself* — the snapshot list when
            // a destination is picked, the listing when a snapshot is. Arriving
            // pre-selected skips both, which left the browser reading nothing
            // beside a snapshot list that was never fetched.
            self.request_snapshots(target);
            self.screens.restore.selected_snapshot = Some(id.clone());
            self.request_browse(target, id, String::new());
            self.go(Route::Restore);
        }
    }

    /// Ask the daemon for this job's snapshots at one destination.
    fn ask_job_snapshots(&mut self, job: &superbackup_core::model::Job, destination: Uuid) {
        self.screens.job_detail.requested(destination);
        self.ask(
            crate::gui::daemon::Intent::JobSnapshots(destination),
            superbackup_core::ipc::protocol::Request::SnapshotList {
                destination: destination.to_string(),
                // Filtered by the daemon rather than here: a job that shares a
                // destination with five others would otherwise pull every
                // snapshot in the repository across the wire to show eight.
                job: Some(job.id.to_string()),
                limit: 0,
            },
        );
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
            if job.content.is_prepared() {
                // Not a job that is missing its folders: a job that has none
                // by design. Saying "no folders" here in warning colour would
                // report a correctly configured job as broken.
                widgets::paragraph(
                    ui,
                    job.content.summary(),
                    Type::Small,
                    t.text_secondary,
                );
            } else if job.sources.is_empty() {
                widgets::paragraph(
                    ui,
                    copy::job_detail::NO_SOURCES,
                    Type::Small,
                    t.warning.tint_text,
                );
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
    ///
    /// A table with columns that line up, not a stack of cards each holding a
    /// badge and a run-on line. Runs are compared down a column — did it get
    /// slower, is it uploading less, when did the failures start — and cards
    /// make every one of those comparisons an eye-scan across ragged text.
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

        // The header and the rows share these, and both must actually keep
        // them; see the note on `fixed_cell` below.
        const HEADER_H: f32 = 18.0;
        const ROW_H: f32 = 26.0;
        const WHEN_W: f32 = 130.0;
        const STATUS_W: f32 = 170.0;
        const UP_W: f32 = 110.0;
        const TOOK_W: f32 = 90.0;

        let mut open: Option<Uuid> = None;
        widgets::table_frame(ui, |ui| {
            // `fixed_cell`, not `allocate_ui_with_layout`, for every column
            // in both the header and the rows below.
            //
            // egui allocates the child's `min_rect` in the parent — "you can
            // request a lot of space and then use less" — so each of these
            // collapsed to the width of its own text. The table therefore had
            // no columns: each row's boundaries landed wherever that row's
            // content happened to end, and the header, whose labels are a
            // different length from the data, drifted furthest of all. It has
            // to be the same helper on both or they line up with nothing.
            ui.horizontal(|ui| {
                ui.set_min_height(HEADER_H);
                for (label, width) in [
                    (copy::job_detail::COL_WHEN, WHEN_W),
                    (copy::job_detail::COL_RESULT, STATUS_W),
                ] {
                    widgets::fixed_cell(
                        ui,
                        width,
                        HEADER_H,
                        Layout::left_to_right(Align::Center),
                        |ui| widgets::table_header(ui, label, None),
                    );
                }
                // Numbers are right-aligned to each other, which is the only
                // way a column of sizes can be compared at a glance.
                for (label, width) in [
                    (copy::job_detail::COL_UPLOADED, UP_W),
                    (copy::job_detail::COL_TOOK, TOOK_W),
                ] {
                    widgets::fixed_cell(
                        ui,
                        width,
                        HEADER_H,
                        Layout::right_to_left(Align::Center),
                        |ui| widgets::table_header(ui, label, None),
                    );
                }
                ui.add_space(space::M);
                // The last column takes the remainder, and is boxed for the
                // same reason: unboxed, `table_header` builds its own child
                // from the *panel's* remaining height and centres the label
                // in that, which is why "Destinations" sat on a different
                // line from every other header.
                let rest = ui.available_width().max(60.0);
                widgets::fixed_cell(
                    ui,
                    rest,
                    HEADER_H,
                    Layout::left_to_right(Align::Center),
                    |ui| widgets::table_header(ui, copy::job_detail::COL_WHERE, None),
                );
            });
            widgets::divider(ui);

            for run in &runs {
                let response = ui.horizontal(|ui| {
                    ui.set_min_height(32.0);
                    widgets::fixed_cell(
                        ui,
                        WHEN_W,
                        ROW_H,
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            widgets::text(
                                ui,
                                format::relative(run.started_at, now),
                                Type::Small,
                                t.text_primary,
                            )
                            .on_hover_text(format::absolute_zoned(run.started_at));
                        },
                    );
                    widgets::fixed_cell(
                        ui,
                        STATUS_W,
                        ROW_H,
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            widgets::status_badge(ui, run.status);
                        },
                    );
                    let uploaded: u64 =
                        run.destinations.iter().map(|d| d.progress.bytes_uploaded).sum();
                    widgets::fixed_cell(
                        ui,
                        UP_W,
                        ROW_H,
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            widgets::text(
                                ui,
                                format::bytes(uploaded),
                                Type::MonoSmall,
                                if uploaded == 0 { t.text_muted } else { t.text_secondary },
                            );
                        },
                    );
                    widgets::fixed_cell(
                        ui,
                        TOOK_W,
                        ROW_H,
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            let took = run.finished_at.map(|finished| {
                                format::duration((finished - run.started_at).num_seconds().max(0))
                            });
                            match took {
                                Some(took) => {
                                    widgets::text(ui, took, Type::MonoSmall, t.text_secondary);
                                }
                                None => widgets::muted_cell(ui, "—"),
                            }
                        },
                    );
                    ui.add_space(space::M);
                    // Which destinations, and whether any of them refused —
                    // "1 destination failed" was buried in a sentence before,
                    // and it is the reason anybody opens a run.
                    let failed: Vec<&str> = run
                        .destinations
                        .iter()
                        .filter(|d| d.status == RunStatus::Failed)
                        .map(|d| d.destination_name.as_str())
                        .collect();
                    if failed.is_empty() {
                        let names: Vec<&str> =
                            run.destinations.iter().map(|d| d.destination_name.as_str()).collect();
                        let room = ui.available_width().max(60.0);
                        widgets::elided(
                            ui,
                            &names.join(", "),
                            Type::Small,
                            t.text_muted,
                            room,
                            false,
                        );
                    } else {
                        let room = ui.available_width().max(60.0);
                        widgets::elided(
                            ui,
                            &copy::job_runs_failed(&failed),
                            Type::Small,
                            t.danger.tint_text,
                            room,
                            false,
                        );
                    }
                });
                if response.response.interact(Sense::click()).clicked() {
                    open = Some(run.run_id);
                }
                widgets::divider(ui);
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
