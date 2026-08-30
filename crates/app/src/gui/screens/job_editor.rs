//! `J-2`. Five tabs over one job: folders, destinations, schedule, exclusions
//! and advanced. Editing is allowed while the vault is locked, because none of
//! it needs a secret resolved.

use chrono::Utc;
use egui::{Align, Layout, Sense, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::ipc::protocol::Request;
use superbackup_core::model::{
    ExclusionPreset, ExclusionSet, Job, RetentionPolicy, Schedule, Source, TimeOfDay,
};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::daemon::Intent;
use crate::gui::data::Action;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::modals::{self, Modal};
use crate::gui::nav::Route;
use crate::gui::theme::{self, size, space, Type};
use crate::gui::validation::{self, Field};
use crate::gui::viewmodel;
use crate::gui::widgets::{self, Button};

pub const TABS: [&str; 5] = [
    copy::job::TAB_SOURCES,
    copy::job::TAB_DESTINATIONS,
    copy::job::TAB_SCHEDULE,
    copy::job::TAB_EXCLUSIONS,
    copy::job::TAB_ADVANCED,
];

#[derive(Default)]
pub struct State {
    pub tab: usize,
    /// The job being edited, held in memory so a cross-screen trip to create a
    /// destination does not lose unsaved work.
    pub draft: Option<Job>,
    pub original: Option<Job>,
    pub tag_input: String,
    pub patterns_text: String,
    pub expanded_preset: Option<usize>,
    pub show_effective: bool,
    pub schedule_time: (u32, u32),
}

impl State {
    pub fn open_tab(&mut self, tab: usize) {
        self.tab = tab.min(TABS.len() - 1);
    }

    fn load(&mut self, job: &Job) {
        if self.original.as_ref().map(|o| o.id) != Some(job.id) {
            self.draft = Some(job.clone());
            self.original = Some(job.clone());
            self.patterns_text = job.exclusions.patterns.join("\n");
            self.tag_input.clear();
            self.schedule_time = (2, 0);
        }
    }

    pub fn dirty(&self) -> bool {
        match (&self.draft, &self.original) {
            (Some(draft), Some(original)) => !same_job(draft, original),
            _ => false,
        }
    }

    /// Which tabs carry unsaved changes, for the 6px dot after their label.
    pub fn dirty_tabs(&self) -> Vec<usize> {
        let (Some(draft), Some(original)) = (&self.draft, &self.original) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if draft.name != original.name
            || draft.description != original.description
            || !same_sources(&draft.sources, &original.sources)
            || draft.tags != original.tags
            || draft.project_id != original.project_id
        {
            out.push(0);
        }
        if draft.destination_ids != original.destination_ids
            || draft.continue_on_destination_error != original.continue_on_destination_error
        {
            out.push(1);
        }
        if draft.schedule != original.schedule || draft.timeout_minutes != original.timeout_minutes
        {
            out.push(2);
        }
        if !same_exclusions(&draft.exclusions, &original.exclusions) {
            out.push(3);
        }
        if draft.bandwidth.is_some() != original.bandwidth.is_some()
            || draft.retention.is_some() != original.retention.is_some()
            || !same_hooks(&draft.hooks, &original.hooks)
        {
            out.push(4);
        }
        out
    }
}

fn same_job(a: &Job, b: &Job) -> bool {
    a.name == b.name
        && a.description == b.description
        && a.project_id == b.project_id
        && same_sources(&a.sources, &b.sources)
        && a.destination_ids == b.destination_ids
        && a.schedule == b.schedule
        && same_exclusions(&a.exclusions, &b.exclusions)
        && a.enabled == b.enabled
        && a.timeout_minutes == b.timeout_minutes
        && same_hooks(&a.hooks, &b.hooks)
        && a.continue_on_destination_error == b.continue_on_destination_error
        && a.tags == b.tags
        && a.retention.is_some() == b.retention.is_some()
        && a.bandwidth.is_some() == b.bandwidth.is_some()
}

/// `Source` and `JobHooks` are plain data in the core with no `PartialEq`, so
/// the editor compares them field by field rather than deriving one there.
fn same_sources(a: &[Source], b: &[Source]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(x, y)| {
            x.path == y.path
                && x.follow_symlinks == y.follow_symlinks
                && x.one_filesystem == y.one_filesystem
        })
}

fn same_hooks(a: &superbackup_core::model::JobHooks, b: &superbackup_core::model::JobHooks) -> bool {
    a.before == b.before
        && a.after_success == b.after_success
        && a.after_failure == b.after_failure
        && a.abort_on_before_failure == b.abort_on_before_failure
}

fn same_exclusions(a: &ExclusionSet, b: &ExclusionSet) -> bool {
    a.presets == b.presets
        && a.patterns == b.patterns
        && a.use_gitignore == b.use_gitignore
        && a.max_file_size_mb == b.max_file_size_mb
        && a.respect_cachedir_tag == b.respect_cachedir_tag
}

impl App {
    pub fn save_open_job(&mut self) {
        let Some(draft) = self.screens.job_editor.draft.clone() else {
            return;
        };
        self.ask(
            Intent::SaveJob(draft.name.clone()),
            Request::JobUpdate { job: Box::new(draft.clone()) },
        );
        self.screens.job_editor.original = Some(draft);
    }

    pub fn discard_open_job(&mut self) {
        self.screens.job_editor.draft = self.screens.job_editor.original.clone();
    }

    pub(crate) fn job_editor_actions(&mut self, ui: &mut Ui, id: Uuid) {
        let Some(job) = self.data.job(&id).cloned() else {
            return;
        };
        self.screens.job_editor.load(&job);
        let report = self.job_report();
        let dirty = self.screens.job_editor.dirty();

        let mut save = false;
        let mut cancel = false;
        let mut run = false;
        let mut menu: Option<&'static str> = None;

        let mut button = Button::primary(copy::action::SAVE_CHANGES).enabled(dirty && report.ok());
        if let Some(summary) = report.summary() {
            button = button.disabled_because(Box::leak(summary.into_boxed_str()));
        }
        if button.show(ui).clicked() {
            save = true;
        }
        if Button::ghost(copy::action::CANCEL).enabled(dirty).show(ui).clicked() {
            cancel = true;
        }
        widgets::overflow_menu(ui, ("job-editor", id), "More actions for this job", |ui| {
            if widgets::menu_item(ui, copy::action::DUPLICATE, true) {
                menu = Some("duplicate");
            }
            if widgets::menu_item(ui, "Browse snapshots…", true) {
                menu = Some("browse");
            }
            if widgets::menu_item(ui, "View history", true) {
                menu = Some("history");
            }
            widgets::divider(ui);
            if widgets::menu_item_danger(ui, copy::job::DANGER_DELETE, true) {
                menu = Some("delete");
            }
        });
        let gate = self.data.gate(Action::RunJob);
        let mut run_button = Button::secondary(copy::action::RUN_NOW)
            .icon(Icon::Play)
            .a11y(format!("Run job \"{}\" now", job.name));
        if let Some(reason) = gate.reason() {
            run_button = run_button.disabled_because(reason);
        }
        if run_button.show(ui).clicked() {
            run = true;
        }

        if save {
            self.save_open_job();
        }
        if cancel {
            self.discard_open_job();
        }
        if run {
            self.request_run(&job);
        }
        match menu {
            Some("delete") => {
                self.open_modal(Modal::Confirm(modals::delete_job_confirm(Some(&job))))
            }
            Some("history") => {
                self.screens.activity.filter_job(id);
                self.go(Route::Activity);
            }
            Some("browse") => self.go(Route::Restore),
            Some("duplicate") => {
                let mut copy_job = job.clone();
                let taken: Vec<String> = self.data.jobs.iter().map(|j| j.name.clone()).collect();
                copy_job.name = validation::unique_name(&format!("{} copy", job.name), &taken);
                self.ask(
                    Intent::SaveJob(copy_job.name.clone()),
                    Request::JobCreate { job: Box::new(copy_job) },
                );
            }
            _ => {}
        }
    }

    fn job_report(&self) -> validation::Report {
        match &self.screens.job_editor.draft {
            Some(draft) => {
                let others: Vec<Job> =
                    self.data.jobs.iter().filter(|j| j.id != draft.id).cloned().collect();
                validation::validate_job(draft, &others, &self.data.destinations)
            }
            None => validation::Report::default(),
        }
    }

    pub(crate) fn show_job_editor(&mut self, ui: &mut Ui, id: Uuid) {
        let t = theme::tokens(ui.ctx());
        let Some(job) = self.data.job(&id).cloned() else {
            widgets::banner(
                ui,
                widgets::BannerKind::Warning,
                copy::err::JOB_NOT_FOUND,
                Some("It may have been deleted in another window."),
                |ui| {
                    if Button::secondary(copy::jobs::TITLE).compact().show(ui).clicked() {
                        // Navigation happens after the borrow ends.
                    }
                },
            );
            return;
        };
        self.screens.job_editor.load(&job);

        let dirty_tabs = self.screens.job_editor.dirty_tabs();
        let mut tab = self.screens.job_editor.tab;
        widgets::segmented_marked(ui, &mut tab, &TABS, &dirty_tabs);
        self.screens.job_editor.tab = tab;
        ui.add_space(space::XL);

        let report = self.job_report();
        if !report.warnings.is_empty() {
            for warning in report.warnings.iter().take(2) {
                widgets::banner(ui, widgets::BannerKind::Warning, warning, None, |_| {});
                ui.add_space(space::L);
            }
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            match tab {
                0 => self.job_tab_sources(ui, &report),
                1 => self.job_tab_destinations(ui, &report),
                2 => self.job_tab_schedule(ui, &report),
                3 => self.job_tab_exclusions(ui, &report),
                _ => self.job_tab_advanced(ui, &report),
            }
            ui.add_space(space::H2);
            let _ = t;
        });
    }

    // -- tab 1: folders -----------------------------------------------------

    fn job_tab_sources(&mut self, ui: &mut Ui, report: &validation::Report) {
        let t = theme::tokens(ui.ctx());
        let Some(draft) = &mut self.screens.job_editor.draft else {
            return;
        };
        widgets::Field::new()
            .label(copy::job::NAME)
            .placeholder(copy::job::NAME_PLACEHOLDER)
            .char_limit(64)
            .error(report.for_field(Field::Name))
            .show(ui, &mut draft.name);
        ui.add_space(space::XL);
        widgets::Field::new()
            .label(copy::job::DESCRIPTION)
            .placeholder(copy::job::DESCRIPTION_PLACEHOLDER)
            .show(ui, &mut draft.description);

        ui.add_space(space::XL);
        widgets::text(ui, copy::job::PROJECT, Type::H3, t.text_primary);
        ui.add_space(space::S);
        let mut project_options = vec![copy::state::NONE.to_string()];
        if let Some(id) = draft.project_id {
            project_options.push(format!("Project {}", format::short_uuid(&id)));
        }
        project_options.push(copy::job::PROJECT_NEW.to_string());
        let mut index = if draft.project_id.is_some() { 1 } else { 0 };
        let last = project_options.len() - 1;
        let mut new_project = false;
        if widgets::combo(ui, "job-project", &mut index, &project_options, 400.0, true) {
            if index == last {
                new_project = true;
            } else if index == 0 {
                draft.project_id = None;
            }
        }

        ui.add_space(space::XL);
        widgets::text(ui, copy::job::TAGS, Type::H3, t.text_primary);
        ui.add_space(space::S);
        let tags = draft.tags.clone();
        let mut remove_tag: Option<usize> = None;
        let mut commit_tag = false;
        ui.horizontal_wrapped(|ui| {
            for (index, tag) in tags.iter().enumerate() {
                if widgets::destination_chip(ui, Icon::Plus, tag, None, 160.0)
                    .interact(egui::Sense::click())
                    .clicked()
                {
                    remove_tag = Some(index);
                }
            }
            let entered = widgets::Field::new()
                .width(140.0)
                .placeholder(copy::job::TAGS_PLACEHOLDER)
                .show(ui, &mut self.screens.job_editor.tag_input);
            if entered.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit_tag = true;
            }
        });
        if commit_tag {
            let value = self.screens.job_editor.tag_input.trim().to_lowercase();
            self.screens.job_editor.tag_input.clear();
            if !value.is_empty() {
                if let Some(draft) = &mut self.screens.job_editor.draft {
                    if !draft.tags.contains(&value) {
                        draft.tags.push(value);
                    }
                }
            }
        }
        if let Some(index) = remove_tag {
            if let Some(draft) = &mut self.screens.job_editor.draft {
                draft.tags.remove(index);
            }
        }

        widgets::form_group(ui, copy::job::SOURCES_TITLE, Some(copy::job::SOURCES_HINT));
        self.source_table(ui, report);

        if new_project {
            self.open_modal(Modal::NewProject(modals::NewProjectState::default()));
        }
    }

    fn source_table(&mut self, ui: &mut Ui, report: &validation::Report) {
        let t = theme::tokens(ui.ctx());
        let sources: Vec<Source> = self
            .screens
            .job_editor
            .draft
            .as_ref()
            .map(|d| d.sources.clone())
            .unwrap_or_default();

        if sources.is_empty() {
            widgets::table_frame(ui, |ui| {
                ui.set_min_height(160.0);
                let (add, _) = widgets::empty_state(ui, Icon::Folder, &copy::empty::SOURCES, None);
                if add {
                    self.add_source_via_picker();
                }
            });
            return;
        }

        let mut remove: Option<usize> = None;
        let mut toggle_symlinks: Option<usize> = None;
        let mut toggle_onefs: Option<usize> = None;

        widgets::table_frame(ui, |ui| {
            egui_extras::TableBuilder::new(ui)
                .id_salt("job-sources")
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(egui_extras::Column::remainder().at_least(200.0))
                .column(egui_extras::Column::exact(110.0))
                .column(egui_extras::Column::exact(90.0))
                .column(egui_extras::Column::exact(90.0))
                .column(egui_extras::Column::exact(32.0))
                .header(size::TABLE_HEADER_H, |mut header| {
                    header.col(|ui| {
                        widgets::table_header(ui, "Path", None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::col::SIZE, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, "Symlinks", None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, "One FS", None);
                    });
                    header.col(|_| {});
                })
                .body(|body| {
                    body.rows(size::TABLE_ROW_H, sources.len(), |mut row| {
                        let index = row.index();
                        let Some(source) = sources.get(index) else {
                            return;
                        };
                        let display = source.path.to_string_lossy().into_owned();
                        let exists = source.path.exists();
                        row.col(|ui| {
                            if !exists {
                                let (rect, response) =
                                    ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                                Icon::AlertTriangle.paint(ui.painter(), rect, t.warning.mark);
                                response.on_hover_text(copy::job::SOURCES_MISSING);
                                ui.add_space(space::XS);
                            }
                            let width = ui.available_width();
                            widgets::elided(ui, &display, Type::Mono, t.text_primary, width, false);
                        });
                        row.col(|ui| {
                            // The walk is a daemon-side job; until the protocol
                            // exposes it, the interface says so rather than
                            // inventing a number.
                            widgets::text(ui, "—", Type::MonoSmall, t.text_muted);
                        });
                        row.col(|ui| {
                            let mut on = source.follow_symlinks;
                            if widgets::toggle(ui, &mut on, "", None, true)
                                .on_hover_text(copy::job::FOLLOW_TOOLTIP)
                                .clicked()
                            {
                                toggle_symlinks = Some(index);
                            }
                        });
                        row.col(|ui| {
                            let mut on = source.one_filesystem;
                            if widgets::toggle(ui, &mut on, "", None, true)
                                .on_hover_text(copy::job::ONE_FS_TOOLTIP)
                                .clicked()
                            {
                                toggle_onefs = Some(index);
                            }
                        });
                        row.col(|ui| {
                            if widgets::icon_button_compact(
                                ui,
                                Icon::Trash,
                                copy::action::REMOVE,
                                true,
                            )
                            .clicked()
                            {
                                remove = Some(index);
                            }
                        });
                    });
                });
        });

        if let Some(draft) = &mut self.screens.job_editor.draft {
            if let Some(index) = remove {
                draft.sources.remove(index);
            }
            if let Some(index) = toggle_symlinks {
                if let Some(source) = draft.sources.get_mut(index) {
                    source.follow_symlinks = !source.follow_symlinks;
                }
            }
            if let Some(index) = toggle_onefs {
                if let Some(source) = draft.sources.get_mut(index) {
                    source.one_filesystem = !source.one_filesystem;
                }
            }
        }

        ui.add_space(space::L);
        ui.horizontal(|ui| {
            if Button::secondary(copy::job::SOURCES_ADD).icon(Icon::Plus).show(ui).clicked() {
                self.add_source_via_picker();
            }
        });
        if let Some(message) = report.for_field(Field::Sources) {
            ui.add_space(space::M);
            widgets::text(ui, message, Type::Small, t.danger.tint_text);
        }
    }

    fn add_source_via_picker(&mut self) {
        let picked = rfd::FileDialog::new().set_title(copy::job::SOURCES_ADD).pick_folders();
        let Some(paths) = picked else {
            return;
        };
        let existing: Vec<Source> = self
            .screens
            .job_editor
            .draft
            .as_ref()
            .map(|d| d.sources.clone())
            .unwrap_or_default();
        let mut rejected: Vec<String> = Vec::new();
        let mut added: Vec<Source> = Vec::new();
        for path in paths {
            if existing.iter().any(|s| s.path == path) {
                rejected.push(copy::job::SOURCES_DUP.to_string());
                continue;
            }
            if let Some(parent) = existing.iter().find(|s| path.starts_with(&s.path)) {
                rejected.push(copy::job_sources_child(&parent.path.to_string_lossy()));
                continue;
            }
            added.push(Source::new(path));
        }
        if let Some(draft) = &mut self.screens.job_editor.draft {
            draft.sources.extend(added);
        }
        for message in rejected {
            self.toasts.warning(message);
        }
    }

    /// Folders dropped onto the window land in the open job's source list.
    /// An additive affordance only (L15) — never the only way to add one.
    pub fn accept_dropped_folders(&mut self, paths: Vec<std::path::PathBuf>) {
        let Some(draft) = &mut self.screens.job_editor.draft else {
            return;
        };
        let mut added = 0;
        for path in paths {
            if path.is_dir() && !draft.sources.iter().any(|s| s.path == path) {
                draft.sources.push(Source::new(path));
                added += 1;
            }
        }
        if added > 0 {
            self.toasts.success(format!("{added} folders added"));
        }
    }

    // -- tab 2: destinations, the fan-out ----------------------------------

    fn job_tab_destinations(&mut self, ui: &mut Ui, report: &validation::Report) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();
        widgets::text(ui, copy::job::DEST_TITLE, Type::H3, t.text_primary);
        ui.add_space(space::XS);
        widgets::paragraph_at(ui, copy::job::DEST_LEAD, Type::Small, t.text_secondary, 560.0);
        ui.add_space(space::XL);

        if self.data.destinations.is_empty() {
            let (add, _) =
                widgets::empty_state(ui, Icon::HardDrive, &copy::empty::DESTINATIONS_INJOB, None);
            if add {
                self.go(Route::NewDestination);
            }
            return;
        }

        let ticked: Vec<Uuid> = self
            .screens
            .job_editor
            .draft
            .as_ref()
            .map(|d| d.destination_ids.clone())
            .unwrap_or_default();
        let ordered: Vec<superbackup_core::model::Destination> =
            viewmodel::order_destinations(&self.data.destinations, &ticked)
                .into_iter()
                .cloned()
                .collect();

        let mut toggled: Option<Uuid> = None;
        let mut menu: Option<(&'static str, Uuid)> = None;
        for destination in &ordered {
            let checked = ticked.contains(&destination.id);
            let enabled = destination.enabled;
            let alpha = if enabled { 1.0 } else { 0.6 };
            let frame = widgets::card_tinted(ui, None, None, |ui| {
                ui.set_height(32.0);
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut on = checked;
                    if widgets::checkbox(ui, &mut on, "", None, enabled).clicked() {
                        toggled = Some(destination.id);
                    }
                    ui.add_space(space::M);
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                    Icon::for_destination_kind(&destination.kind).paint(
                        ui.painter(),
                        rect,
                        theme::alpha(t.text_secondary, alpha),
                    );
                    ui.add_space(space::L);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = space::XXS;
                        widgets::text(
                            ui,
                            &destination.name,
                            Type::BodyStrong,
                            theme::alpha(t.text_primary, alpha),
                        );
                        let location = destination_location(&self.data, destination);
                        let width = (ui.available_width() - 220.0).max(120.0);
                        widgets::elided(
                            ui,
                            &location,
                            Type::MonoSmall,
                            t.text_muted,
                            width,
                            false,
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        widgets::overflow_menu(
                            ui,
                            ("job-dest", destination.id),
                            "More actions for this destination",
                            |ui| {
                                if widgets::menu_item(ui, copy::action::VERIFY_NOW, true) {
                                    menu = Some(("verify", destination.id));
                                }
                                if widgets::menu_item(ui, "Edit destination…", true) {
                                    menu = Some(("edit", destination.id));
                                }
                                if widgets::menu_item(ui, "Browse snapshots…", true) {
                                    menu = Some(("browse", destination.id));
                                }
                            },
                        );
                        if !enabled {
                            widgets::neutral_badge(ui, copy::badge::DISABLED, Some(Icon::Pause));
                        } else {
                            let failed = self.screens.destinations.failed_probe(destination.id);
                            match viewmodel::verification(destination, now, failed) {
                                viewmodel::Verification::Recent(label) => {
                                    widgets::badge(
                                        ui,
                                        t.success,
                                        Some(Icon::CheckCircle),
                                        &label,
                                    );
                                }
                                viewmodel::Verification::Old(label) => {
                                    widgets::neutral_badge(ui, &label, None);
                                }
                                viewmodel::Verification::Never => {
                                    widgets::badge(
                                        ui,
                                        t.warning,
                                        Some(Icon::AlertTriangle),
                                        copy::job::DEST_NEVER_VERIFIED,
                                    );
                                }
                                viewmodel::Verification::Unreachable => {
                                    widgets::badge(
                                        ui,
                                        t.danger,
                                        Some(Icon::XOctagon),
                                        copy::job::DEST_UNREACHABLE,
                                    );
                                }
                            }
                        }
                    });
                });
            });
            if !enabled {
                frame.response.on_hover_text(copy::job::DEST_DISABLED_ROW);
            }
            ui.add_space(space::M);
        }

        if let Some(id) = toggled {
            if let Some(draft) = &mut self.screens.job_editor.draft {
                if draft.destination_ids.contains(&id) {
                    draft.destination_ids.retain(|d| d != &id);
                } else {
                    draft.destination_ids.push(id);
                }
            }
        }

        if let Some(message) = report.for_field(Field::Destinations) {
            ui.add_space(space::M);
            widgets::banner(ui, widgets::BannerKind::Danger, message, None, |_| {});
        }

        // A job that writes to both a repository and a mirror is worth one
        // sentence, because the two are not the same kind of copy.
        let ticked_kinds: Vec<&superbackup_core::model::DestinationKind> = self
            .data
            .destinations
            .iter()
            .filter(|d| ticked.contains(&d.id))
            .map(|d| &d.kind)
            .collect();
        let has_repo = ticked_kinds.iter().any(|k| k.is_repository());
        let has_mirror = ticked_kinds.iter().any(|k| !k.is_repository());
        if has_repo && has_mirror {
            ui.add_space(space::L);
            widgets::banner(
                ui,
                widgets::BannerKind::Info,
                copy::job::DEST_MIXED_WARNING,
                None,
                |_| {},
            );
        }

        ui.add_space(space::XL);
        if Button::secondary(copy::job::DEST_NEW).icon(Icon::Plus).show(ui).clicked() {
            self.go(Route::NewDestination);
        }

        widgets::form_group(ui, "When a destination fails", None);
        if let Some(draft) = &mut self.screens.job_editor.draft {
            let mut on = draft.continue_on_destination_error;
            if widgets::toggle(
                ui,
                &mut on,
                copy::job::DEST_CONTINUE_ON_ERROR,
                Some(copy::job::DEST_CONTINUE_BODY),
                true,
            )
            .clicked()
            {
                draft.continue_on_destination_error = on;
            }
        }

        match menu {
            Some(("verify", id)) => self.request_verify(id),
            Some(("edit", id)) => self.go(Route::DestinationEditor(id)),
            Some(("browse", _)) => self.go(Route::Restore),
            _ => {}
        }
    }

    // -- tab 3: schedule ----------------------------------------------------

    fn job_tab_schedule(&mut self, ui: &mut Ui, report: &validation::Report) {
        let t = theme::tokens(ui.ctx());
        let now = Utc::now();
        let global_metered = self.data.settings.skip_on_metered;
        let global_battery = self.data.settings.skip_on_battery;
        let capabilities = superbackup_core::platform::capabilities();
        let limitations = superbackup_core::platform::limitations();
        let mut cron_help = false;

        let Some(draft) = &mut self.screens.job_editor.draft else {
            return;
        };

        let options: [(&str, Option<&str>); 6] = [
            (copy::job::SCHEDULE_MANUAL, Some(copy::job::SCHEDULE_MANUAL_BODY)),
            (copy::job::SCHEDULE_INTERVAL, None),
            (copy::job::SCHEDULE_DAILY, None),
            (copy::job::SCHEDULE_WEEKLY, None),
            (copy::job::SCHEDULE_CRON, None),
            (copy::job::SCHEDULE_ONCHANGE, Some(copy::job::SCHEDULE_ONCHANGE_BODY)),
        ];
        let current = schedule_index(&draft.schedule);

        for (index, (label, helper)) in options.iter().enumerate() {
            if widgets::radio(ui, current == index, label, *helper, true).clicked() {
                draft.schedule = default_schedule(index, &draft.schedule);
            }
            if current == index {
                ui.horizontal(|ui| {
                    ui.add_space(28.0);
                    ui.vertical(|ui| {
                        schedule_controls(
                            ui,
                            &mut draft.schedule,
                            report,
                            &mut self.screens.job_editor.schedule_time,
                            &mut cron_help,
                        );
                    });
                });
            }
            ui.add_space(space::M);
        }

        // The always-visible summary strip: computed from the edited schedule,
        // not the saved one.
        ui.add_space(space::L);
        let runs = viewmodel::next_runs(&draft.schedule, now, 5);
        let summary = if runs.is_empty() {
            copy::job::SCHEDULE_NEXT_NONE.to_string()
        } else {
            copy::job_schedule_next_five(
                &runs.iter().map(|r| format::absolute(*r)).collect::<Vec<_>>().join(", "),
            )
        };
        egui::Frame::new()
            .fill(t.bg_raised)
            .corner_radius(crate::gui::theme::radius::CONTROL)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::paragraph(ui, summary, Type::Small, t.text_secondary);
            });

        widgets::form_group(ui, copy::job::CONDITIONS_TITLE, None);

        // Honest platform behaviour: a switch that does nothing here is greyed
        // out with the reason, not offered.
        let metered_reason = if capabilities.metered_detection {
            None
        } else {
            limitations
                .iter()
                .find(|l| l.area == "power")
                .map(|l| l.message.clone())
        };
        let mut metered = global_metered;
        match &metered_reason {
            Some(reason) => {
                ui.add_enabled_ui(false, |ui| {
                    widgets::toggle(
                        ui,
                        &mut metered,
                        copy::job::CONDITIONS_METERED,
                        Some(reason),
                        false,
                    );
                });
            }
            None => {
                widgets::toggle(
                    ui,
                    &mut metered,
                    copy::job::CONDITIONS_METERED,
                    Some(copy::job::CONDITIONS_USING_GLOBAL),
                    true,
                );
            }
        }
        ui.add_space(space::L);
        let mut battery = global_battery;
        widgets::toggle(
            ui,
            &mut battery,
            copy::job::CONDITIONS_BATTERY,
            Some(copy::job::CONDITIONS_USING_GLOBAL),
            capabilities.battery_detection,
        );

        ui.add_space(space::XL);
        let mut timeout_on = draft.timeout_minutes.is_some();
        if widgets::checkbox(ui, &mut timeout_on, copy::job::TIMEOUT, None, true).clicked() {
            draft.timeout_minutes = if timeout_on { Some(120) } else { None };
        }
        if let Some(minutes) = &mut draft.timeout_minutes {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                widgets::number(ui, minutes, 1..=1440, copy::job::TIMEOUT_UNIT, true, copy::job::TIMEOUT);
            });
        }
        ui.add_space(space::S);
        widgets::paragraph_at(ui, copy::job::TIMEOUT_BODY, Type::Small, t.text_muted, 560.0);

        if cron_help {
            self.open_modal(Modal::CronHelp);
        }
    }

    // -- tab 4: exclusions --------------------------------------------------

    fn job_tab_exclusions(&mut self, ui: &mut Ui, report: &validation::Report) {
        let t = theme::tokens(ui.ctx());
        widgets::text(ui, copy::job::EXCL_TITLE, Type::H3, t.text_primary);
        ui.add_space(space::XS);
        widgets::paragraph_at(ui, copy::job::EXCL_LEAD, Type::Small, t.text_secondary, 560.0);
        ui.add_space(space::XL);

        let mut apply_defaults = false;
        let mut clear_all = false;
        ui.horizontal(|ui| {
            if Button::ghost(copy::job::EXCL_SELECT_DEFAULTS).show(ui).clicked() {
                apply_defaults = true;
            }
            if Button::ghost(copy::job::EXCL_CLEAR_ALL).show(ui).clicked() {
                clear_all = true;
            }
        });
        ui.add_space(space::L);

        let expanded = self.screens.job_editor.expanded_preset;
        let mut toggle_expanded: Option<usize> = None;
        let mut toggle_preset: Option<ExclusionPreset> = None;

        let selected: Vec<ExclusionPreset> = self
            .screens
            .job_editor
            .draft
            .as_ref()
            .map(|d| d.exclusions.presets.clone())
            .unwrap_or_default();

        for (index, preset) in ExclusionPreset::all().iter().enumerate() {
            let checked = selected.contains(preset);
            let risky = preset.is_risky();
            widgets::card_tinted(
                ui,
                risky.then(|| theme::alpha(t.warning.tint_bg, 0.3)),
                None,
                |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_top(|ui| {
                        let mut on = checked;
                        if widgets::checkbox(ui, &mut on, "", None, true).clicked() {
                            toggle_preset = Some(*preset);
                        }
                        ui.add_space(space::M);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = space::XS;
                            ui.horizontal(|ui| {
                                if risky {
                                    let (rect, response) = ui
                                        .allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                                    Icon::AlertTriangle.paint(
                                        ui.painter(),
                                        rect,
                                        t.warning.mark,
                                    );
                                    response.on_hover_text(copy::job::EXCL_RISKY);
                                    ui.add_space(space::XS);
                                }
                                widgets::text(
                                    ui,
                                    preset.title(),
                                    Type::BodyStrong,
                                    t.text_primary,
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let label =
                                        copy::job_excl_patterns_count(preset.patterns().len());
                                    if widgets::link(ui, &label).clicked() {
                                        toggle_expanded = Some(index);
                                    }
                                });
                            });
                            // The rationale is the model's own string, verbatim,
                            // so the CLI and the window never disagree.
                            widgets::paragraph_at(
                                ui,
                                preset.rationale(),
                                Type::Small,
                                t.text_muted,
                                (ui.available_width() - 8.0).max(200.0),
                            );
                            if expanded == Some(index) {
                                ui.add_space(space::XS);
                                for pattern in preset.patterns() {
                                    widgets::text(ui, *pattern, Type::MonoSmall, t.text_muted);
                                }
                            }
                        });
                    });
                },
            );
            ui.add_space(space::M);
        }

        if let Some(index) = toggle_expanded {
            self.screens.job_editor.expanded_preset =
                if expanded == Some(index) { None } else { Some(index) };
        }

        if let Some(draft) = &mut self.screens.job_editor.draft {
            if let Some(preset) = toggle_preset {
                if draft.exclusions.presets.contains(&preset) {
                    draft.exclusions.presets.retain(|p| p != &preset);
                } else {
                    draft.exclusions.presets.push(preset);
                }
            }
            if apply_defaults {
                let patterns = draft.exclusions.patterns.clone();
                draft.exclusions = ExclusionSet::developer_defaults();
                draft.exclusions.patterns = patterns;
            }
            if clear_all {
                draft.exclusions.presets.clear();
            }
        }
        if apply_defaults {
            let count = self
                .screens
                .job_editor
                .draft
                .as_ref()
                .map(|d| d.exclusions.effective_patterns().len())
                .unwrap_or(0);
            self.toasts.success(copy::job_excl_defaults_applied(count));
        }

        widgets::form_group(ui, "Additional options", None);
        if let Some(draft) = &mut self.screens.job_editor.draft {
            let mut gitignore = draft.exclusions.use_gitignore;
            if widgets::toggle(
                ui,
                &mut gitignore,
                copy::job::EXCL_GITIGNORE,
                Some(copy::job::EXCL_GITIGNORE_BODY),
                true,
            )
            .clicked()
            {
                draft.exclusions.use_gitignore = gitignore;
            }
            ui.add_space(space::L);
            let mut cachedir = draft.exclusions.respect_cachedir_tag;
            if widgets::toggle(
                ui,
                &mut cachedir,
                copy::job::EXCL_CACHEDIR,
                Some(copy::job::EXCL_CACHEDIR_BODY),
                true,
            )
            .clicked()
            {
                draft.exclusions.respect_cachedir_tag = cachedir;
            }
            ui.add_space(space::L);
            let mut limit_on = draft.exclusions.max_file_size_mb.is_some();
            if widgets::checkbox(
                ui,
                &mut limit_on,
                copy::job::EXCL_MAX_SIZE,
                Some(copy::job::EXCL_MAX_SIZE_BODY),
                true,
            )
            .clicked()
            {
                draft.exclusions.max_file_size_mb = if limit_on { Some(1024) } else { None };
            }
            if let Some(mb) = &mut draft.exclusions.max_file_size_mb {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    let mut value = (*mb).min(u32::MAX as u64) as u32;
                    widgets::number(
                        ui,
                        &mut value,
                        1..=1_048_576,
                        copy::job::EXCL_MAX_SIZE_UNIT,
                        true,
                        copy::job::EXCL_MAX_SIZE,
                    );
                    *mb = value as u64;
                });
            }
        }

        widgets::form_group(ui, copy::job::EXCL_CUSTOM, Some(copy::job::EXCL_CUSTOM_BODY));
        let mut patterns_text = self.screens.job_editor.patterns_text.clone();
        widgets::Field::new()
            .width(ui.available_width().min(819.0))
            .rows(6)
            .mono()
            .placeholder(copy::job::EXCL_CUSTOM_PLACEHOLDER)
            .error(report.for_field(Field::Patterns))
            .show(ui, &mut patterns_text);
        if patterns_text != self.screens.job_editor.patterns_text {
            self.screens.job_editor.patterns_text = patterns_text.clone();
            if let Some(draft) = &mut self.screens.job_editor.draft {
                draft.exclusions.patterns = patterns_text
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
            }
        }

        let effective: Vec<String> = self
            .screens
            .job_editor
            .draft
            .as_ref()
            .map(|d| d.exclusions.effective_patterns())
            .unwrap_or_default();
        ui.add_space(space::XL);
        let label = copy::job_excl_show_effective(effective.len());
        if widgets::link(ui, &label).clicked() {
            self.screens.job_editor.show_effective = !self.screens.job_editor.show_effective;
        }
        if self.screens.job_editor.show_effective {
            ui.add_space(space::M);
            widgets::code_block(ui, &effective.join("\n"), 240.0, None);
        }

        // The impact strip. The estimate is a daemon-side walk, so until the
        // protocol carries one this states what it can rather than guessing.
        ui.add_space(space::XL);
        egui::Frame::new()
            .fill(t.bg_raised)
            .corner_radius(crate::gui::theme::radius::CONTROL)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::text(ui, copy::job::EXCL_IMPACT_FAILED, Type::Small, t.text_muted);
            });
    }

    // -- tab 5: advanced ----------------------------------------------------

    fn job_tab_advanced(&mut self, ui: &mut Ui, report: &validation::Report) {
        let t = theme::tokens(ui.ctx());
        let global = self.data.settings.bandwidth.clone();
        let mut delete = false;

        let Some(draft) = &mut self.screens.job_editor.draft else {
            return;
        };

        widgets::text(ui, copy::job::BANDWIDTH_TITLE, Type::H3, t.text_primary);
        ui.add_space(space::M);
        let overriding = draft.bandwidth.is_some();
        if widgets::radio(ui, !overriding, copy::job::BANDWIDTH_GLOBAL, None, true).clicked() {
            draft.bandwidth = None;
        }
        if widgets::radio(ui, overriding, copy::job::BANDWIDTH_CUSTOM, None, true).clicked() {
            draft.bandwidth = Some(global.clone());
        }
        if let Some(bandwidth) = &mut draft.bandwidth {
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                ui.vertical(|ui| {
                    bandwidth_row(ui, copy::job::BANDWIDTH_UPLOAD, &mut bandwidth.upload_kbps);
                    ui.add_space(space::M);
                    bandwidth_row(ui, copy::job::BANDWIDTH_DOWNLOAD, &mut bandwidth.download_kbps);
                });
            });
        }
        ui.add_space(space::M);
        widgets::text(
            ui,
            copy::job_bandwidth_current_global(
                &format::kbps(global.upload_kbps),
                &format::kbps(global.download_kbps),
            ),
            Type::Small,
            t.text_muted,
        );
        ui.add_space(space::XS);
        widgets::paragraph_at(ui, copy::job::BANDWIDTH_NO_WINDOW, Type::Small, t.text_muted, 560.0);

        widgets::form_group(ui, copy::job::RETENTION_TITLE, None);
        let per_destination = draft.retention.is_none();
        if widgets::radio(ui, per_destination, copy::job::RETENTION_PER_DEST, None, true).clicked()
        {
            draft.retention = None;
        }
        if widgets::radio(ui, !per_destination, copy::job::RETENTION_CUSTOM, None, true).clicked() {
            draft.retention = Some(RetentionPolicy::default());
        }
        if let Some(policy) = &mut draft.retention {
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                ui.vertical(|ui| {
                    retention_grid(ui, policy);
                    ui.add_space(space::M);
                    widgets::paragraph_at(
                        ui,
                        copy::job_retention_summary(
                            policy.keep_latest,
                            policy.keep_hourly,
                            policy.keep_daily,
                            policy.keep_weekly,
                            policy.keep_monthly,
                            policy.keep_annual,
                        ),
                        Type::Small,
                        t.text_secondary,
                        520.0,
                    );
                    if let Some(message) = report.for_field(Field::Retention) {
                        ui.add_space(space::S);
                        widgets::text(ui, message, Type::Small, t.danger.tint_text);
                    }
                    ui.add_space(space::S);
                    widgets::paragraph_at(
                        ui,
                        copy::job::RETENTION_MIRROR_NOTE,
                        Type::Small,
                        t.text_muted,
                        520.0,
                    );
                });
            });
        }

        widgets::form_group(ui, copy::job::HOOKS_TITLE, None);
        widgets::banner(ui, widgets::BannerKind::Warning, copy::job::HOOKS_WARNING, None, |_| {});
        ui.add_space(space::L);
        let mut before = draft.hooks.before.clone().unwrap_or_default();
        widgets::Field::new()
            .label(copy::job::HOOKS_BEFORE)
            .mono()
            .show(ui, &mut before);
        draft.hooks.before = (!before.trim().is_empty()).then_some(before);
        ui.add_space(space::M);
        let mut abort = draft.hooks.abort_on_before_failure;
        if widgets::checkbox(ui, &mut abort, copy::job::HOOKS_ABORT, None, true).clicked() {
            draft.hooks.abort_on_before_failure = abort;
        }
        ui.add_space(space::L);
        let mut success = draft.hooks.after_success.clone().unwrap_or_default();
        widgets::Field::new()
            .label(copy::job::HOOKS_AFTER_SUCCESS)
            .mono()
            .show(ui, &mut success);
        draft.hooks.after_success = (!success.trim().is_empty()).then_some(success);
        ui.add_space(space::L);
        let mut failure = draft.hooks.after_failure.clone().unwrap_or_default();
        widgets::Field::new()
            .label(copy::job::HOOKS_AFTER_FAILURE)
            .mono()
            .show(ui, &mut failure);
        draft.hooks.after_failure = (!failure.trim().is_empty()).then_some(failure);
        ui.add_space(space::L);
        widgets::paragraph_at(ui, copy::job::HOOKS_ENV, Type::Small, t.text_muted, 560.0);

        ui.add_space(space::H2);
        widgets::card_tinted(ui, None, Some(theme::alpha(t.danger.mark, 0.4)), |ui| {
            ui.set_width(ui.available_width());
            widgets::text(ui, copy::job::DANGER_TITLE, Type::H3, t.text_primary);
            ui.add_space(space::M);
            widgets::paragraph_at(ui, copy::job::DANGER_BODY, Type::Small, t.text_secondary, 560.0);
            ui.add_space(space::L);
            if Button::danger(copy::job::DANGER_DELETE).icon(Icon::Trash).show(ui).clicked() {
                delete = true;
            }
        });

        if delete {
            let job = self.screens.job_editor.draft.clone();
            self.open_modal(Modal::Confirm(modals::delete_job_confirm(job.as_ref())));
        }
    }
}

fn bandwidth_row(ui: &mut Ui, label: &str, value: &mut Option<u32>) {
    ui.horizontal(|ui| {
        let mut on = value.is_some();
        if widgets::checkbox(ui, &mut on, label, None, true).clicked() {
            *value = if on { Some(2000) } else { None };
        }
        if let Some(v) = value {
            ui.add_space(space::M);
            widgets::number(ui, v, 1..=10_000_000, copy::job::BANDWIDTH_UNIT, true, label);
            ui.add_space(space::M);
            let t = theme::tokens(ui.ctx());
            widgets::text(ui, format::kbps_as_mbit(*v), Type::Small, t.text_muted);
        }
    });
}

pub fn retention_grid(ui: &mut Ui, policy: &mut RetentionPolicy) {
    let fields: [(&str, &mut u32); 6] = [
        (copy::job::RETENTION_LATEST, &mut policy.keep_latest),
        (copy::job::RETENTION_HOURLY, &mut policy.keep_hourly),
        (copy::job::RETENTION_DAILY, &mut policy.keep_daily),
        (copy::job::RETENTION_WEEKLY, &mut policy.keep_weekly),
        (copy::job::RETENTION_MONTHLY, &mut policy.keep_monthly),
        (copy::job::RETENTION_ANNUAL, &mut policy.keep_annual),
    ];
    let t = theme::tokens(ui.ctx());
    let mut iter = fields.into_iter();
    for _ in 0..2 {
        ui.horizontal(|ui| {
            for _ in 0..3 {
                if let Some((label, value)) = iter.next() {
                    ui.allocate_ui_with_layout(
                        Vec2::new(120.0, 52.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            widgets::text(ui, label, Type::Small, t.text_secondary);
                            widgets::number(ui, value, 0..=10_000, "", true, label);
                        },
                    );
                }
            }
        });
        ui.add_space(space::M);
    }
    ui.horizontal(|ui| {
        widgets::number(
            ui,
            &mut policy.maintenance_every_n_runs,
            0..=1_000,
            copy::job::RETENTION_MAINTENANCE_UNIT,
            true,
            copy::job::RETENTION_MAINTENANCE,
        );
    });
}

fn schedule_index(schedule: &Schedule) -> usize {
    match schedule {
        Schedule::Manual => 0,
        Schedule::Interval { .. } => 1,
        Schedule::Daily { .. } => 2,
        Schedule::Weekly { .. } => 3,
        Schedule::Cron { .. } => 4,
        Schedule::OnChange { .. } => 5,
    }
}

fn default_schedule(index: usize, current: &Schedule) -> Schedule {
    let times = match current {
        Schedule::Daily { times } | Schedule::Weekly { times, .. } => times.clone(),
        _ => vec![TimeOfDay { hour: 2, minute: 0 }],
    };
    match index {
        0 => Schedule::Manual,
        1 => Schedule::Interval { minutes: 60 },
        2 => Schedule::Daily { times },
        3 => Schedule::Weekly { weekdays: vec![0], times },
        4 => Schedule::Cron { expression: "0 2 * * *".into() },
        _ => Schedule::OnChange { debounce_seconds: 120, min_interval_minutes: 30 },
    }
}

fn schedule_controls(
    ui: &mut Ui,
    schedule: &mut Schedule,
    report: &validation::Report,
    time_input: &mut (u32, u32),
    cron_help: &mut bool,
) {
    let t = theme::tokens(ui.ctx());
    match schedule {
        Schedule::Manual => {}
        Schedule::Interval { minutes } => {
            ui.horizontal(|ui| {
                widgets::number(
                    ui,
                    minutes,
                    1..=10_080,
                    copy::job::SCHEDULE_INTERVAL_UNIT,
                    true,
                    copy::job::SCHEDULE_INTERVAL,
                );
                for (label, value) in [("15m", 15u32), ("30m", 30), ("1h", 60), ("4h", 240)] {
                    if Button::ghost(label).compact().show(ui).clicked() {
                        *minutes = value;
                    }
                }
            });
            if *minutes < 15 {
                ui.add_space(space::S);
                widgets::text(
                    ui,
                    copy::job::SCHEDULE_INTERVAL_WARN,
                    Type::Small,
                    t.warning.tint_text,
                );
            }
        }
        Schedule::Daily { times } => time_chips(ui, times, time_input, report),
        Schedule::Weekly { weekdays, times } => {
            ui.horizontal(|ui| {
                for (index, label) in format::WEEKDAY_SHORT.iter().enumerate() {
                    let on = weekdays.contains(&(index as u8));
                    let button = if on {
                        Button::primary(&label[..2])
                    } else {
                        Button::secondary(&label[..2])
                    };
                    if button.min_width(36.0).show(ui).clicked() {
                        if on {
                            weekdays.retain(|d| *d != index as u8);
                        } else {
                            weekdays.push(index as u8);
                        }
                    }
                }
            });
            if let Some(message) = report.for_field(Field::ScheduleWeekdays) {
                ui.add_space(space::S);
                widgets::text(ui, message, Type::Small, t.danger.tint_text);
            }
            ui.add_space(space::M);
            time_chips(ui, times, time_input, report);
        }
        Schedule::Cron { expression } => {
            widgets::Field::new()
                .width(400.0)
                .mono()
                .error(report.for_field(Field::Cron))
                .show(ui, expression);
            ui.add_space(space::S);
            match validation::parse_cron(expression) {
                Ok(()) => {
                    let runs = viewmodel::next_runs(schedule, Utc::now(), 5);
                    let text = if runs.is_empty() {
                        "The daemon works out when a cron schedule fires.".to_string()
                    } else {
                        copy::job_schedule_next_five(
                            &runs.iter().map(|r| format::absolute(*r)).collect::<Vec<_>>().join(", "),
                        )
                    };
                    widgets::paragraph_at(ui, text, Type::Small, t.text_muted, 520.0);
                }
                Err(message) => {
                    widgets::paragraph_at(ui, message, Type::Small, t.danger.tint_text, 520.0);
                }
            }
            ui.add_space(space::S);
            if widgets::link(ui, copy::job::SCHEDULE_CRON_HELP).clicked() {
                *cron_help = true;
            }
        }
        Schedule::OnChange { debounce_seconds, min_interval_minutes } => {
            ui.horizontal(|ui| {
                widgets::text(ui, copy::job::SCHEDULE_DEBOUNCE, Type::Small, t.text_secondary);
                widgets::number(
                    ui,
                    debounce_seconds,
                    5..=3_600,
                    copy::job::SCHEDULE_DEBOUNCE_UNIT,
                    true,
                    copy::job::SCHEDULE_DEBOUNCE,
                );
            });
            ui.add_space(space::M);
            ui.horizontal(|ui| {
                widgets::text(ui, copy::job::SCHEDULE_MIN_INTERVAL, Type::Small, t.text_secondary);
                widgets::number(
                    ui,
                    min_interval_minutes,
                    1..=1_440,
                    copy::job::SCHEDULE_MIN_UNIT,
                    true,
                    copy::job::SCHEDULE_MIN_INTERVAL,
                );
            });
        }
    }
}

fn time_chips(
    ui: &mut Ui,
    times: &mut Vec<TimeOfDay>,
    input: &mut (u32, u32),
    report: &validation::Report,
) {
    let t = theme::tokens(ui.ctx());
    let mut remove: Option<usize> = None;
    ui.horizontal_wrapped(|ui| {
        for (index, time) in times.iter().enumerate() {
            let label = time.to_string();
            if Button::secondary(&label).compact().icon(Icon::Clock).show(ui).clicked() {
                remove = Some(index);
            }
        }
    });
    if let Some(index) = remove {
        times.remove(index);
    }
    ui.add_space(space::M);
    ui.horizontal(|ui| {
        widgets::number(ui, &mut input.0, 0..=23, "h", true, "Hour");
        widgets::number(ui, &mut input.1, 0..=59, "m", true, "Minute");
        if Button::secondary(copy::job::SCHEDULE_ADD_TIME).show(ui).clicked() {
            let candidate = TimeOfDay { hour: input.0 as u8, minute: input.1 as u8 };
            if times.len() < 24 && !times.contains(&candidate) {
                times.push(candidate);
                times.sort();
            }
        }
    });
    if let Some(message) = report.for_field(Field::ScheduleTimes) {
        ui.add_space(space::S);
        widgets::text(ui, message, Type::Small, t.danger.tint_text);
    }
}

/// `S3 bucket · storj-backups / superbackup/andreas-pc/` or the path.
pub fn destination_location(
    data: &crate::gui::data::Data,
    destination: &superbackup_core::model::Destination,
) -> String {
    use superbackup_core::model::DestinationKind as K;
    match &destination.kind {
        K::S3 { provider_id, bucket, prefix, .. } => {
            let provider = data
                .provider(provider_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| copy::state::UNKNOWN.to_string());
            format!("{} · {} / {}{}", destination.kind.label(), provider, bucket, prefix)
        }
        other => match other.local_path() {
            Some(path) => format!("{} · {}", other.label(), path.to_string_lossy()),
            None => other.label().to_string(),
        },
    }
}
