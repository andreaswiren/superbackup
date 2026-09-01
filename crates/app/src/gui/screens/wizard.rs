//! `W-1` … `W-6`. A large modal, six steps, no left step list — at 760px it
//! would cost 180px of width for six labels, so the header sub-line carries
//! the same information.

use chrono::Utc;
use egui::{Align, Layout, Sense, Ui, Vec2};
use uuid::Uuid;

use superbackup_core::ipc::protocol::Request;
use superbackup_core::model::{ExclusionPreset, ExclusionSet, Job, Schedule, Source, TimeOfDay};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::daemon::Intent;
use crate::gui::data::Data;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::nav::Route;
use crate::gui::theme::{self, radius, space, Type};
use crate::gui::validation::{self, WizardStep};
use crate::gui::viewmodel;
use crate::gui::widgets::{self, Button, ModalSize};

/// The four templates. `Development folder` is preselected and is the one that
/// names the problem in the user's own terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    Development,
    Documents,
    Home,
    Blank,
}

impl Template {
    pub const ALL: [Template; 4] =
        [Template::Development, Template::Documents, Template::Home, Template::Blank];

    pub fn title(self) -> &'static str {
        match self {
            Template::Development => copy::template::DEV_TITLE,
            Template::Documents => copy::template::DOCS_TITLE,
            Template::Home => copy::template::HOME_TITLE,
            Template::Blank => copy::template::BLANK_TITLE,
        }
    }
    pub fn body(self) -> &'static str {
        match self {
            Template::Development => copy::template::DEV_BODY,
            Template::Documents => copy::template::DOCS_BODY,
            Template::Home => copy::template::HOME_BODY,
            Template::Blank => copy::template::BLANK_BODY,
        }
    }
    pub fn detail(self) -> &'static str {
        match self {
            Template::Development => copy::template::DEV_DETAIL,
            Template::Documents => copy::template::DOCS_DETAIL,
            Template::Home => copy::template::HOME_DETAIL,
            Template::Blank => copy::template::BLANK_DETAIL,
        }
    }
    pub fn icon(self) -> Icon {
        match self {
            Template::Development => Icon::Terminal,
            Template::Documents => Icon::FileText,
            Template::Home => Icon::Folder,
            Template::Blank => Icon::Plus,
        }
    }
    pub fn default_name(self) -> &'static str {
        match self {
            Template::Development => "Development",
            Template::Documents => "Documents",
            Template::Home => "Everything",
            Template::Blank => "",
        }
    }

    /// The folders this template proposes, filtered to those that exist.
    pub fn sources(self) -> Vec<Source> {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(std::path::PathBuf::from);
        let Some(home) = home else {
            return Vec::new();
        };
        let candidates: Vec<std::path::PathBuf> = match self {
            Template::Development => ["dev", "source", "repos", "Projects", "source/repos"]
                .iter()
                .map(|p| home.join(p))
                .collect(),
            Template::Documents => ["Documents", "Desktop"].iter().map(|p| home.join(p)).collect(),
            Template::Home => vec![home.clone()],
            Template::Blank => Vec::new(),
        };
        let existing: Vec<Source> =
            candidates.iter().filter(|p| p.exists()).map(Source::new).collect();
        match self {
            // The development template proposes the first folder that exists,
            // not every one of them.
            Template::Development => existing.into_iter().take(1).collect(),
            _ => existing,
        }
    }

    /// The exclusions this template applies. The developer templates call the
    /// model's own constructor rather than assembling a preset list here.
    pub fn exclusions(self) -> ExclusionSet {
        match self {
            Template::Development => ExclusionSet::developer_defaults(),
            Template::Documents => ExclusionSet {
                presets: vec![ExclusionPreset::OsJunk, ExclusionPreset::LogsAndTemp],
                ..ExclusionSet::default()
            },
            Template::Home => {
                let mut set = ExclusionSet::developer_defaults();
                set.presets.push(ExclusionPreset::VirtualMachineImages);
                set
            }
            Template::Blank => ExclusionSet { presets: Vec::new(), ..ExclusionSet::default() },
        }
    }

    pub fn schedule(self) -> Schedule {
        match self {
            Template::Blank => Schedule::Manual,
            _ => Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] },
        }
    }
}

#[derive(Debug, Clone)]
pub struct WizardState {
    pub step: WizardStep,
    pub template: Template,
    pub draft: Job,
    pub run_after: bool,
    pub patterns_text: String,
}

impl WizardState {
    pub fn new(data: &Data) -> WizardState {
        let template = Template::Development;
        let mut draft = blank_job();
        apply_template(&mut draft, template, data);
        WizardState {
            step: WizardStep::Template,
            template,
            draft,
            run_after: true,
            patterns_text: String::new(),
        }
    }
}

fn blank_job() -> Job {
    Job {
        id: Uuid::new_v4(),
        name: String::new(),
        project_id: None,
        description: String::new(),
        sources: Vec::new(),
        destination_ids: Vec::new(),
        schedule: Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] },
        exclusions: ExclusionSet::default(),
        bandwidth: None,
        retention: None,
        enabled: true,
        timeout_minutes: None,
        hooks: Default::default(),
        continue_on_destination_error: true,
        created_at: Utc::now(),
        tags: Vec::new(),
    }
}

fn apply_template(draft: &mut Job, template: Template, data: &Data) {
    draft.sources = template.sources();
    draft.exclusions = template.exclusions();
    draft.schedule = template.schedule();
    let taken: Vec<String> = data.jobs.iter().map(|j| j.name.clone()).collect();
    draft.name = if template.default_name().is_empty() {
        String::new()
    } else {
        validation::unique_name(template.default_name(), &taken)
    };
}

/// Draw the wizard. Returns `Some` to keep it open.
pub fn show(app: &mut App, ctx: &egui::Context, mut state: WizardState) -> Option<WizardState> {
    let t = theme::tokens(ctx);
    let mut advance = false;
    let mut back = false;
    let mut cancel = false;
    let mut create = false;
    let blocked = validation::wizard_blocked(state.step, &state.draft, &app.data.destinations);

    let subtitle = format!(
        "Step {} of {} · {}",
        state.step.index() + 1,
        WizardStep::ALL.len(),
        state.step.title()
    );

    let (close, _) = widgets::modal(
        ctx,
        "sb-wizard",
        ModalSize::Large,
        copy::jobs::NEW,
        Some((Icon::Repeat, t.accent)),
        false,
        |m| {
            m.body(|ui| {
                widgets::text(ui, &subtitle, Type::Small, t.text_muted);
                ui.add_space(space::XL);
                match state.step {
                    WizardStep::Template => step_template(ui, &mut state, &app.data),
                    WizardStep::Sources => step_sources(ui, &mut state),
                    WizardStep::Destinations => step_destinations(ui, &mut state, &app.data),
                    WizardStep::Schedule => step_schedule(ui, &mut state),
                    WizardStep::Exclusions => step_exclusions(ui, &mut state),
                    WizardStep::Review => step_review(ui, &mut state, &app.data),
                }
            });
            m.footer(|ui| {
                let last = state.step == WizardStep::Review;
                let label = if last { "Create job" } else { copy::action::CONTINUE };
                let mut button = Button::primary(label);
                if let Some(reason) = &blocked {
                    button = button.disabled_because(Box::leak(reason.clone().into_boxed_str()));
                }
                if button.show(ui).clicked() {
                    if last {
                        create = true;
                    } else {
                        advance = true;
                    }
                }
                if Button::ghost(copy::action::CANCEL).show(ui).clicked() {
                    cancel = true;
                }
                if state.step.previous().is_some()
                    && Button::ghost(copy::action::BACK).show(ui).clicked()
                {
                    back = true;
                }
            });
        },
    );

    if advance {
        if let Some(next) = state.step.next() {
            state.step = next;
        }
    }
    if back {
        if let Some(previous) = state.step.previous() {
            state.step = previous;
        }
    }
    if create {
        let job = state.draft.clone();
        let name = job.name.clone();
        // Run it now only if it was asked for *and* the vault can actually run
        // it. The job is created either way; the run is the part that waits.
        let run_after = state.run_after && app.data.unlocked();
        if state.run_after && !app.data.unlocked() {
            app.toasts.warning(copy::locked::ACTION_BLOCKED);
        }
        // The run is issued from the reply, not here.
        //
        // `job.id` is the wizard's draft id, and `job.create` on the daemon
        // assigns a fresh one on purpose — so firing `JobRun` with the draft id
        // asked to run a job that had never existed, and the answer came back
        // as "That job no longer exists" over a job that had just been created
        // perfectly well. Even with the right id it would have been a race:
        // this ran before the create had been answered.
        let intent = if run_after {
            Intent::SaveJobAndRun(name.clone())
        } else {
            Intent::SaveJob(name.clone())
        };
        app.ask(intent, Request::JobCreate { job: Box::new(job) });
        app.go(Route::Dashboard);
        return None;
    }
    if cancel || close {
        // Discarding before anything was chosen needs no confirmation.
        if state.step == WizardStep::Template {
            return None;
        }
        return None;
    }
    Some(state)
}

fn step_template(ui: &mut Ui, state: &mut WizardState, data: &Data) {
    let t = theme::tokens(ui.ctx());
    let mut chosen: Option<Template> = None;
    let width = ((ui.available_width() - space::XL) / 2.0).floor();

    for row in Template::ALL.chunks(2) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = space::XL;
            for template in row {
                let selected = state.template == *template;
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 116.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        let frame =
                            widgets::card_tinted(ui, None, selected.then_some(t.accent), |ui| {
                                ui.set_width(ui.available_width());
                                ui.set_height(84.0);
                                ui.horizontal(|ui| {
                                    let (rect, _) =
                                        ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                                    template.icon().paint(ui.painter(), rect, t.text_secondary);
                                    ui.add_space(space::M);
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = space::XXS;
                                        if *template == Template::Development {
                                            widgets::text(
                                                ui,
                                                copy::template::DEV_EYEBROW,
                                                Type::SmallStrong,
                                                t.accent,
                                            );
                                        }
                                        widgets::text(
                                            ui,
                                            template.title(),
                                            Type::H2,
                                            t.text_primary,
                                        );
                                        widgets::paragraph_at(
                                            ui,
                                            template.body(),
                                            Type::Small,
                                            t.text_secondary,
                                            (ui.available_width() - 8.0).max(120.0),
                                        );
                                        widgets::paragraph_at(
                                            ui,
                                            template.detail(),
                                            Type::Small,
                                            t.text_muted,
                                            (ui.available_width() - 8.0).max(120.0),
                                        );
                                    });
                                });
                            });
                        let response = ui.interact(
                            frame.response.rect,
                            egui::Id::new("wizard-template").with(template.title()),
                            Sense::click(),
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::RadioButton,
                                true,
                                selected,
                                format!("{}. {}", template.title(), template.detail()),
                            )
                        });
                        if response.clicked() {
                            chosen = Some(*template);
                        }
                    },
                );
            }
        });
        ui.add_space(space::XL);
    }

    if let Some(template) = chosen {
        state.template = template;
        apply_template(&mut state.draft, template, data);
    }
}

fn step_sources(ui: &mut Ui, state: &mut WizardState) {
    let t = theme::tokens(ui.ctx());
    widgets::Field::new()
        .label(copy::onboarding::JOB_NAME)
        .placeholder(copy::job::NAME_PLACEHOLDER)
        .char_limit(64)
        .show(ui, &mut state.draft.name);
    ui.add_space(space::XL);
    widgets::text(ui, copy::job::SOURCES_TITLE, Type::H3, t.text_primary);
    ui.add_space(space::S);
    widgets::paragraph_at(ui, copy::job::SOURCES_HINT, Type::Small, t.text_muted, 640.0);
    ui.add_space(space::L);

    let mut remove: Option<usize> = None;
    if state.draft.sources.is_empty() {
        widgets::table_frame(ui, |ui| {
            // The empty state centres itself within the height it is *given*,
            // and inside a layout that grows to fit, `available_height` is the
            // space left in the parent rather than the height of this box. The
            // centring padding then inflated the box and left the content
            // sitting near its bottom edge. Allocating a fixed height first
            // makes "the middle" a real, known place.
            const BOX_H: f32 = 180.0;
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), BOX_H),
                Layout::top_down(Align::Center),
                |ui| {
                    ui.set_min_height(BOX_H);
                    widgets::empty_state(ui, Icon::Folder, &copy::empty::SOURCES, None);
                },
            );
        });
    } else {
        for (index, source) in state.draft.sources.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.set_min_height(28.0);
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                Icon::Folder.paint(ui.painter(), rect, t.text_muted);
                ui.add_space(space::M);
                let width = (ui.available_width() - 40.0).max(120.0);
                widgets::elided(
                    ui,
                    &source.path.to_string_lossy(),
                    Type::Mono,
                    t.text_primary,
                    width,
                    false,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button_compact(ui, Icon::Trash, copy::action::REMOVE, true)
                        .clicked()
                    {
                        remove = Some(index);
                    }
                });
            });
        }
    }
    if let Some(index) = remove {
        state.draft.sources.remove(index);
    }

    ui.add_space(space::L);
    if Button::secondary(copy::job::SOURCES_ADD).icon(Icon::Plus).show(ui).clicked() {
        if let Some(paths) = rfd::FileDialog::new().pick_folders() {
            for path in paths {
                if !state.draft.sources.iter().any(|s| s.path == path) {
                    state.draft.sources.push(Source::new(path));
                }
            }
        }
    }
}

fn step_destinations(ui: &mut Ui, state: &mut WizardState, data: &Data) {
    let t = theme::tokens(ui.ctx());
    widgets::text(ui, copy::job::DEST_TITLE, Type::H3, t.text_primary);
    ui.add_space(space::XS);
    widgets::paragraph_at(ui, copy::job::DEST_LEAD, Type::Small, t.text_secondary, 640.0);
    ui.add_space(space::XL);

    if data.destinations.is_empty() {
        // No destinations yet: the four kinds, each with its trade-off, and
        // the mirror's line saying plainly that it is not encrypted.
        for (index, (title, trade_off)) in
            crate::gui::screens::destinations::KIND_CHOICES.iter().enumerate()
        {
            widgets::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                    crate::gui::screens::destinations::kind_icon(index).paint(
                        ui.painter(),
                        rect,
                        t.text_secondary,
                    );
                    ui.add_space(space::L);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = space::XXS;
                        widgets::text(ui, *title, Type::BodyStrong, t.text_primary);
                        let colour = if index == 3 { t.warning.tint_text } else { t.text_muted };
                        widgets::paragraph_at(
                            ui,
                            *trade_off,
                            Type::Small,
                            colour,
                            (ui.available_width() - 8.0).max(200.0),
                        );
                    });
                });
            });
            ui.add_space(space::M);
        }
        ui.add_space(space::L);
        widgets::paragraph_at(
            ui,
            copy::empty::DESTINATIONS_INJOB.body,
            Type::Small,
            t.text_muted,
            640.0,
        );
        return;
    }

    let ticked = state.draft.destination_ids.clone();
    let ordered = viewmodel::order_destinations(&data.destinations, &ticked);
    let mut toggle: Option<Uuid> = None;
    for destination in ordered {
        let checked = ticked.contains(&destination.id);
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let mut on = checked;
                if widgets::checkbox(ui, &mut on, "", None, destination.enabled).clicked() {
                    toggle = Some(destination.id);
                }
                ui.add_space(space::M);
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                Icon::for_destination_kind(&destination.kind).paint(
                    ui.painter(),
                    rect,
                    t.text_secondary,
                );
                ui.add_space(space::L);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = space::XXS;
                    widgets::text(ui, &destination.name, Type::BodyStrong, t.text_primary);
                    let location =
                        crate::gui::screens::job_editor::destination_location(data, destination);
                    widgets::elided(
                        ui,
                        &location,
                        Type::MonoSmall,
                        t.text_muted,
                        (ui.available_width() - 8.0).max(160.0),
                        false,
                    );
                });
            });
        });
        ui.add_space(space::M);
    }
    if let Some(id) = toggle {
        if state.draft.destination_ids.contains(&id) {
            state.draft.destination_ids.retain(|d| d != &id);
        } else {
            state.draft.destination_ids.push(id);
        }
    }
}

/// Hourly, 08:00–17:00, Monday to Friday.
///
/// Expressed as cron rather than as a new `Schedule` variant: the scheduler
/// already evaluates cron in local time with the daylight-saving behaviour this
/// needs, so a new variant would be a second implementation of something that
/// already works.
const WORK_HOURS_CRON: &str = "0 8-17 * * 1-5";

fn step_schedule(ui: &mut Ui, state: &mut WizardState) {
    let t = theme::tokens(ui.ctx());
    // Each option carries a tooltip, because the label alone cannot say what
    // "Weekly" or a cron expression will actually do, and a schedule the user
    // has guessed at is a schedule they will not trust.
    let options = [
        (copy::job::SCHEDULE_MANUAL, copy::job::SCHEDULE_MANUAL_TIP, 0usize),
        (copy::job::SCHEDULE_INTERVAL, copy::job::SCHEDULE_INTERVAL_TIP, 1),
        (copy::job::SCHEDULE_WORKHOURS, copy::job::SCHEDULE_WORKHOURS_TIP, 2),
        (copy::job::SCHEDULE_DAILY, copy::job::SCHEDULE_DAILY_TIP, 3),
        (copy::job::SCHEDULE_WEEKLY, copy::job::SCHEDULE_WEEKLY_TIP, 4),
        (copy::job::SCHEDULE_CRON, copy::job::SCHEDULE_CRON_TIP, 5),
        (copy::job::SCHEDULE_ONCHANGE, copy::job::SCHEDULE_ONCHANGE_TIP, 6),
    ];
    let current = match &state.draft.schedule {
        Schedule::Manual => 0,
        Schedule::Interval { .. } => 1,
        // Work hours is a cron expression underneath, so it has to be
        // recognised by its shape or selecting it would not stick.
        Schedule::Cron { expression } if expression == WORK_HOURS_CRON => 2,
        Schedule::Daily { .. } => 3,
        Schedule::Weekly { .. } => 4,
        Schedule::Cron { .. } => 5,
        Schedule::OnChange { .. } => 6,
    };
    for (label, tip, index) in options {
        if widgets::radio(ui, current == index, label, None, true).on_hover_text(tip).clicked() {
            state.draft.schedule = match index {
                0 => Schedule::Manual,
                1 => Schedule::Interval { minutes: 60 },
                2 => Schedule::Cron { expression: WORK_HOURS_CRON.into() },
                3 => Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] },
                4 => Schedule::Weekly {
                    weekdays: vec![0],
                    times: vec![TimeOfDay { hour: 2, minute: 0 }],
                },
                5 => Schedule::Cron { expression: "0 2 * * *".into() },
                _ => Schedule::OnChange { debounce_seconds: 120, min_interval_minutes: 30 },
            };
        }
        ui.add_space(space::S);
    }

    ui.add_space(space::XL);
    let runs = viewmodel::next_runs(&state.draft.schedule, Utc::now(), 5);
    let summary = if runs.is_empty() {
        copy::job::SCHEDULE_NEXT_NONE.to_string()
    } else {
        copy::job_schedule_next_five(
            &runs.iter().map(|r| format::absolute(*r)).collect::<Vec<_>>().join(", "),
        )
    };
    egui::Frame::new()
        .fill(t.bg_raised)
        .corner_radius(radius::CONTROL)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::paragraph(ui, summary, Type::Small, t.text_secondary);
        });
}

fn step_exclusions(ui: &mut Ui, state: &mut WizardState) {
    let t = theme::tokens(ui.ctx());
    widgets::paragraph_at(ui, copy::job::EXCL_LEAD, Type::Small, t.text_secondary, 640.0);
    ui.add_space(space::L);

    let selected = state.draft.exclusions.presets.clone();
    let mut toggle: Option<ExclusionPreset> = None;
    egui::ScrollArea::vertical()
        .id_salt("wizard-exclusions")
        .max_height(260.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for preset in ExclusionPreset::all() {
                let checked = selected.contains(preset);
                ui.horizontal_top(|ui| {
                    let mut on = checked;
                    if widgets::checkbox(ui, &mut on, "", None, true).clicked() {
                        toggle = Some(*preset);
                    }
                    ui.add_space(space::M);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = space::XXS;
                        ui.horizontal(|ui| {
                            if preset.is_risky() {
                                let (rect, response) =
                                    ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                                Icon::AlertTriangle.paint(ui.painter(), rect, t.warning.mark);
                                response.on_hover_text(copy::job::EXCL_RISKY);
                                ui.add_space(space::XS);
                            }
                            widgets::text(ui, preset.title(), Type::BodyStrong, t.text_primary);
                        });
                        let width = (ui.available_width() - 8.0).max(200.0);
                        // What it matches comes before why it is safe: a user
                        // deciding whether to tick a box needs to know what it
                        // does first.
                        widgets::paragraph_at(
                            ui,
                            preset.matches_description(),
                            Type::Small,
                            t.text_secondary,
                            width,
                        );
                        widgets::paragraph_at(
                            ui,
                            preset.rationale(),
                            Type::Small,
                            t.text_muted,
                            width,
                        );
                        // And the patterns themselves, verbatim. Nothing about
                        // what gets skipped should require trusting a summary.
                        widgets::paragraph_at(
                            ui,
                            preset.patterns().join("   "),
                            Type::MonoSmall,
                            t.text_muted,
                            width,
                        );
                    });
                });
                ui.add_space(space::M);
            }
        });
    // The whole point of this panel: the user can see, in one place, exactly
    // what will be skipped. Ticking boxes whose effect is described only in
    // prose is what makes excluding anything feel risky.
    ui.add_space(space::L);
    let total = state.draft.exclusions.effective_patterns();
    widgets::text(ui, copy::job::EXCL_TOTAL_TITLE, Type::BodyStrong, t.text_primary);
    ui.add_space(space::XS);
    widgets::paragraph_at(ui, copy::job::EXCL_TOTAL_BODY, Type::Small, t.text_muted, 640.0);
    ui.add_space(space::S);
    widgets::table_frame(ui, |ui| {
        ui.add_space(space::S);
        if total.is_empty() {
            widgets::paragraph_at(
                ui,
                copy::job::EXCL_TOTAL_EMPTY,
                Type::Small,
                t.text_muted,
                (ui.available_width() - 16.0).max(200.0),
            );
        } else {
            widgets::paragraph_at(
                ui,
                total.join("   "),
                Type::MonoSmall,
                t.text_secondary,
                (ui.available_width() - 16.0).max(200.0),
            );
            ui.add_space(space::XS);
            widgets::text(ui, copy::job::excl_total_count(total.len()), Type::Small, t.text_muted);
        }
        ui.add_space(space::S);
    });

    if let Some(preset) = toggle {
        if state.draft.exclusions.presets.contains(&preset) {
            state.draft.exclusions.presets.retain(|p| p != &preset);
        } else {
            state.draft.exclusions.presets.push(preset);
        }
    }

    ui.add_space(space::L);
    let mut gitignore = state.draft.exclusions.use_gitignore;
    if widgets::toggle(ui, &mut gitignore, copy::job::EXCL_GITIGNORE, None, true).clicked() {
        state.draft.exclusions.use_gitignore = gitignore;
    }
    ui.add_space(space::M);
    let mut cachedir = state.draft.exclusions.respect_cachedir_tag;
    if widgets::toggle(ui, &mut cachedir, copy::job::EXCL_CACHEDIR, None, true).clicked() {
        state.draft.exclusions.respect_cachedir_tag = cachedir;
    }

    ui.add_space(space::L);
    widgets::Field::new()
        .label(copy::job::EXCL_CUSTOM)
        .width(ui.available_width().min(640.0))
        .rows(4)
        .mono()
        .placeholder(copy::job::EXCL_CUSTOM_PLACEHOLDER)
        .show(ui, &mut state.patterns_text);
    state.draft.exclusions.patterns = state
        .patterns_text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
}

fn step_review(ui: &mut Ui, state: &mut WizardState, data: &Data) {
    let t = theme::tokens(ui.ctx());
    widgets::kv(ui, copy::job::NAME, &state.draft.name, false);
    widgets::kv(ui, copy::job::PROJECT, copy::state::NONE, false);
    widgets::kv_with(ui, copy::job::SOURCES_TITLE, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = space::XXS;
            for source in &state.draft.sources {
                widgets::elided(
                    ui,
                    &source.path.to_string_lossy(),
                    Type::MonoSmall,
                    t.text_primary,
                    (ui.available_width() - 8.0).max(160.0),
                    false,
                );
            }
        });
    });
    let destinations: Vec<String> =
        state.draft.destination_ids.iter().map(|id| data.destination_name(id)).collect();
    widgets::kv(ui, copy::job::DEST_TITLE, &destinations.join(" · "), false);
    let schedule = viewmodel::schedule_string(&state.draft.schedule);
    let next = viewmodel::next_runs(&state.draft.schedule, Utc::now(), 1)
        .first()
        .map(|at| format!(" — next run {}", format::relative_future(*at, Utc::now())))
        .unwrap_or_default();
    widgets::kv(ui, copy::job::TAB_SCHEDULE, &format!("{schedule}{next}"), false);
    widgets::kv(
        ui,
        copy::job::EXCL_TITLE,
        &format!(
            "{} presets, {} patterns",
            state.draft.exclusions.presets.len(),
            state.draft.exclusions.effective_patterns().len()
        ),
        false,
    );
    widgets::kv(ui, copy::job::RETENTION_TITLE, copy::job::RETENTION_PER_DEST, false);
    widgets::kv(
        ui,
        copy::job::BANDWIDTH_TITLE,
        &copy::job_bandwidth_current_global(
            &format::kbps(data.settings.bandwidth.upload_kbps),
            &format::kbps(data.settings.bandwidth.download_kbps),
        ),
        false,
    );

    ui.add_space(space::XL);
    let mut run = state.run_after;
    if widgets::checkbox(
        ui,
        &mut run,
        "Run this job now",
        Some("The first run copies everything; later runs copy only what changed."),
        true,
    )
    .clicked()
    {
        state.run_after = run;
    }
}
