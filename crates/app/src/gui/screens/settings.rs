//! `S-1` … `S-9`. Two panes: a 200px section list on the left, the section on
//! the right. Everything applies immediately and is saved on change, which is
//! why this screen uses toggles rather than checkboxes.

use egui::{Align, Layout, Sense, Ui, Vec2};

use uuid::Uuid;

use superbackup_core::ipc::protocol::{
    CheckStatus, DoctorReply, ErrorPayload, KopiaProbeReply, Request,
};
use superbackup_core::model::{BandwidthWindow, LogLevel, Theme, UpdatePolicy};

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::daemon::Intent;
use crate::gui::data::Action;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::modals::{self, Modal};
use crate::gui::nav::{Route, SettingsSection};
use crate::gui::theme::{self, radius, space, Type};
use crate::gui::widgets::{self, Button, StepState};

#[derive(Default)]
pub struct State {
    pub doctor: Option<DoctorReply>,
    pub doctor_running: bool,
    pub pause_reason: String,
    pub kopia_path: String,
    /// The last `kopia.probe` result, or `None` before anything was run. The
    /// screen distinguishes the two: "not run yet" and "ran and found nothing"
    /// are different answers and must not look the same.
    pub kopia_probe: Option<KopiaProbeReply>,
    pub kopia_probing: bool,
    pub kopia_probe_error: Option<String>,
    /// Which destination the repository half of the check runs against.
    /// `None` means "version only", which needs no unlocked vault.
    pub kopia_probe_destination: Option<Uuid>,
    /// The machine label being edited.
    ///
    /// This has to be held here rather than rebuilt from the snapshot each
    /// frame. In an immediate-mode interface a local `let mut label =
    /// data.machine_label()` is recreated before every pass, so the character
    /// the user just typed is thrown away and the field snaps back — which is
    /// exactly what it did. `None` means "not being edited"; the field then
    /// shows what the daemon reports.
    pub machine_label: Option<String>,
}

impl State {
    pub fn doctor_arrived(&mut self, reply: DoctorReply) {
        self.doctor_running = false;
        self.doctor = Some(reply);
    }
    pub fn kopia_probe_started(&mut self) {
        self.kopia_probing = true;
        self.kopia_probe_error = None;
    }
    pub fn kopia_probe_arrived(&mut self, reply: KopiaProbeReply) {
        self.kopia_probing = false;
        self.kopia_probe_error = None;
        self.kopia_probe = Some(reply);
    }
    pub fn kopia_probe_failed(&mut self, payload: ErrorPayload) {
        self.kopia_probing = false;
        self.kopia_probe_error = Some(payload.message);
    }
    pub fn busy(&self) -> bool {
        self.doctor_running || self.kopia_probing
    }
}

impl App {
    pub(crate) fn show_settings(&mut self, ui: &mut Ui, section: SettingsSection) {
        let t = theme::tokens(ui.ctx());
        let mut go: Option<SettingsSection> = None;

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(200.0, ui.available_height()),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for candidate in SettingsSection::ALL {
                        let selected = candidate == section;
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 32.0),
                            Sense::click(),
                        );
                        if selected {
                            ui.painter().rect_filled(rect, radius::CONTROL, t.rail_selected_bg);
                        } else if response.hovered() {
                            ui.painter().rect_filled(rect, radius::CONTROL, t.bg_surface_hover);
                        }
                        if response.has_focus() {
                            widgets::focus_ring(ui, rect.shrink(2.0), radius::CONTROL);
                        }
                        let colour = if selected { t.text_primary } else { t.text_secondary };
                        let style = if selected { Type::BodyStrong } else { Type::Body };
                        let g = widgets::galley(ui, candidate.title(), style, colour);
                        let h = g.size().y;
                        ui.painter().galley(
                            egui::Pos2::new(rect.left() + 12.0, rect.center().y - h / 2.0),
                            g,
                            colour,
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::SelectableLabel,
                                true,
                                selected,
                                candidate.title(),
                            )
                        });
                        if response.clicked() {
                            go = Some(candidate);
                        }
                    }
                },
            );
            ui.add_space(space::H3);
            widgets::vertical_rule(ui, ui.available_height());
            ui.add_space(space::H3);
            ui.vertical(|ui| {
                widgets::scroll_area(ui, ("settings", section), |ui| {
                    match section {
                        SettingsSection::General => self.settings_general(ui),
                        SettingsSection::Scheduling => self.settings_scheduling(ui),
                        SettingsSection::Bandwidth => self.settings_bandwidth(ui),
                        SettingsSection::Notifications => self.settings_notifications(ui),
                        SettingsSection::Security => self.settings_security(ui),
                        SettingsSection::Kopia => self.settings_kopia(ui),
                        SettingsSection::Remote => self.settings_remote(ui),
                        SettingsSection::Advanced => self.settings_advanced(ui),
                        SettingsSection::Reset => self.settings_reset(ui),
                    }
                    ui.add_space(space::H2);
                });
            });
        });

        if let Some(section) = go {
            self.go(Route::Settings(section));
        }
    }

    fn settings_general(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let mut changed = false;
        let current = self.data.machine_label().to_string();
        // Start from what the daemon reports, then keep the user's edit until
        // it is saved or abandoned.
        let mut label =
            self.screens.settings.machine_label.clone().unwrap_or_else(|| current.clone());
        let response = widgets::Field::new()
            .label(copy::set::MACHINE_LABEL)
            .char_limit(64)
            .show(ui, &mut label);
        if response.changed() {
            self.screens.settings.machine_label = Some(label.clone());
        }

        let trimmed = label.trim().to_string();
        let dirty = self.screens.settings.machine_label.is_some() && trimmed != current;
        if dirty {
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                let valid = !trimmed.is_empty();
                let mut save = Button::primary(copy::action::SAVE_CHANGES).enabled(valid);
                if !valid {
                    save = save.disabled_because(copy::set::MACHINE_LABEL_EMPTY);
                }
                if save.show(ui).clicked() {
                    self.ask(
                        Intent::RenameMachine(trimmed.clone()),
                        Request::MachineRename { label: trimmed.clone() },
                    );
                    self.screens.settings.machine_label = None;
                }
                if Button::ghost(copy::action::CANCEL).show(ui).clicked() {
                    self.screens.settings.machine_label = None;
                }
            });
        }
        ui.add_space(space::S);
        widgets::text(
            ui,
            copy::set_machine_slug(self.data.machine_slug()),
            Type::MonoSmall,
            t.text_muted,
        );
        ui.add_space(space::XS);
        widgets::paragraph_at(ui, copy::set::MACHINE_SLUG_NOTE, Type::Small, t.text_muted, 560.0);

        widgets::form_group(ui, "This machine", None);
        if let Some(version) = &self.data.version {
            widgets::kv(ui, copy::set::OS, &version.target_os.to_string(), false);
            widgets::kv(ui, copy::set::ARCH, &version.target_arch, false);
        }
        widgets::kv(
            ui,
            "superbackup",
            &self.data.version.as_ref().map(|v| v.version.clone()).unwrap_or_default(),
            false,
        );

        widgets::form_group(ui, copy::set::THEME, None);
        let mut theme_index = match self.data.settings.theme {
            Theme::System => 0,
            Theme::Light => 1,
            Theme::Dark => 2,
        };
        let before = theme_index;
        widgets::segmented(
            ui,
            &mut theme_index,
            &[copy::set::THEME_SYSTEM, copy::set::THEME_LIGHT, copy::set::THEME_DARK],
        );
        if theme_index != before {
            self.data.settings.theme = match theme_index {
                1 => Theme::Light,
                2 => Theme::Dark,
                _ => Theme::System,
            };
            changed = true;
        }

        widgets::form_group(ui, "At each destination", None);
        let mut manifest = self.data.settings.write_machine_manifest;
        if widgets::toggle(
            ui,
            &mut manifest,
            copy::machines::SETTING,
            Some(copy::machines::SETTING_BODY),
            true,
        )
        .clicked()
        {
            self.data.settings.write_machine_manifest = manifest;
            changed = true;
        }
        ui.add_space(space::S);
        // Stated next to the control rather than discovered later: object
        // storage genuinely cannot hold this file, and a setting that silently
        // does nothing for half a user's destinations is worse than one that
        // says so.
        widgets::paragraph_at(ui, copy::machines::SETTING_S3, Type::Small, t.text_muted, 560.0);

        widgets::form_group(ui, "Starting up", None);
        let mut autostart = self.data.settings.start_at_login;
        if widgets::toggle(ui, &mut autostart, copy::set::AUTOSTART, None, true).clicked() {
            self.data.settings.start_at_login = autostart;
            changed = true;
        }
        ui.horizontal(|ui| {
            ui.add_space(28.0);
            let mut minimised = self.data.settings.start_minimised;
            if widgets::toggle(ui, &mut minimised, copy::set::START_MINIMISED, None, autostart)
                .clicked()
            {
                self.data.settings.start_minimised = minimised;
                changed = true;
            }
        });

        ui.add_space(space::XL);
        let capabilities = superbackup_core::platform::capabilities();
        let mut service = self.data.settings.run_as_service;
        let service_reason = if capabilities.system_service {
            None
        } else {
            Some("This platform has no machine-wide service.")
        };
        match service_reason {
            Some(reason) => {
                ui.add_enabled_ui(false, |ui| {
                    widgets::toggle(ui, &mut service, copy::set::SERVICE, Some(reason), false);
                });
            }
            None => {
                if widgets::toggle(
                    ui,
                    &mut service,
                    copy::set::SERVICE,
                    Some(copy::onboarding::SERVICE_BODY),
                    true,
                )
                .clicked()
                {
                    self.data.settings.run_as_service = service;
                    changed = true;
                }
            }
        }
        ui.add_space(space::M);
        let status = match &self.data.service {
            Some(s) if s.installed && s.running => copy::set::SERVICE_INSTALLED_RUNNING,
            Some(s) if s.installed => copy::set::SERVICE_INSTALLED_STOPPED,
            _ => copy::set::SERVICE_NOT_INSTALLED,
        };
        ui.horizontal(|ui| {
            widgets::text(ui, status, Type::Small, t.text_secondary);
            ui.add_space(space::M);
            let installed = self.data.service.as_ref().map(|s| s.installed).unwrap_or(false);
            if installed {
                if Button::secondary(copy::set::SERVICE_UNINSTALL).compact().show(ui).clicked() {
                    self.ask(Intent::Service, Request::ServiceUninstall {});
                }
            } else if Button::secondary(copy::set::SERVICE_INSTALL).compact().show(ui).clicked() {
                self.ask(Intent::Service, Request::ServiceInstall {});
            }
        });
        ui.add_space(space::S);
        widgets::paragraph_at(
            ui,
            copy::onboarding::SERVICE_ELEVATE,
            Type::Small,
            t.text_muted,
            560.0,
        );

        widgets::form_group(ui, copy::set::PARALLEL, Some(copy::set::PARALLEL_BODY));
        let mut parallel = self.data.settings.max_parallel_jobs;
        widgets::number(ui, &mut parallel, 1..=8, "", true, copy::set::PARALLEL);
        if parallel != self.data.settings.max_parallel_jobs {
            self.data.settings.max_parallel_jobs = parallel;
            changed = true;
        }

        ui.add_space(space::H2);
        if Button::danger_ghost(copy::set::QUIT).show(ui).clicked() {
            self.ask(Intent::Fire, Request::ControlShutdown { stop_runs: false });
        }
        ui.add_space(space::S);
        widgets::paragraph_at(ui, copy::set::QUIT_BODY, Type::Small, t.text_muted, 560.0);

        if changed {
            self.save_settings();
        }
    }

    fn settings_scheduling(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let mut changed = false;
        let capabilities = superbackup_core::platform::capabilities();
        let limitations = superbackup_core::platform::limitations();

        let mut catchup = self.data.settings.run_missed_on_start;
        if widgets::toggle(
            ui,
            &mut catchup,
            copy::set::CATCHUP,
            Some(copy::set::CATCHUP_BODY),
            true,
        )
        .clicked()
        {
            self.data.settings.run_missed_on_start = catchup;
            changed = true;
        }

        ui.add_space(space::XL);
        let mut metered = self.data.settings.skip_on_metered;
        // Greyed out with the platform's own reason rather than offered as a
        // switch that does nothing.
        let metered_reason = (!capabilities.metered_detection)
            .then(|| {
                limitations
                    .iter()
                    .find(|l| l.code.contains("metered") || l.code.contains("no_metered_api"))
                    .map(|l| l.message.clone())
            })
            .flatten();
        match &metered_reason {
            Some(reason) => {
                ui.add_enabled_ui(false, |ui| {
                    widgets::toggle(ui, &mut metered, copy::set::METERED, Some(reason), false);
                });
            }
            None => {
                if widgets::toggle(
                    ui,
                    &mut metered,
                    copy::set::METERED,
                    Some(copy::set::METERED_BODY),
                    true,
                )
                .clicked()
                {
                    self.data.settings.skip_on_metered = metered;
                    changed = true;
                }
            }
        }

        ui.add_space(space::XL);
        let mut battery = self.data.settings.skip_on_battery;
        if widgets::toggle(
            ui,
            &mut battery,
            copy::set::BATTERY,
            None,
            capabilities.battery_detection,
        )
        .clicked()
        {
            self.data.settings.skip_on_battery = battery;
            changed = true;
        }

        widgets::form_group(ui, copy::set::PAUSE_TITLE, Some(copy::set::PAUSE_BODY));
        if self.data.paused() {
            let body = match self.data.paused_until() {
                Some(until) => copy::set_pause_active(&format::clock(until)),
                None => copy::set::PAUSE_ACTIVE_FOREVER.to_string(),
            };
            let mut resume = false;
            let mut extend = false;
            widgets::banner(ui, widgets::BannerKind::Warning, &body, None, |ui| {
                if Button::ghost(copy::set::PAUSE_EXTEND).compact().show(ui).clicked() {
                    extend = true;
                }
                if Button::secondary(copy::set::PAUSE_RESUME).compact().show(ui).clicked() {
                    resume = true;
                }
            });
            if resume {
                self.resume();
            }
            if extend {
                self.pause(Some(3600), self.data.pause_reason());
            }
        } else {
            let mut chosen: Option<Option<u64>> = None;
            ui.horizontal(|ui| {
                for (label, seconds) in [
                    (copy::set::PAUSE_1H, Some(3_600u64)),
                    (copy::set::PAUSE_2H, Some(7_200)),
                    (copy::set::PAUSE_4H, Some(14_400)),
                    (copy::set::PAUSE_8H, Some(28_800)),
                    (copy::set::PAUSE_FOREVER, None),
                ] {
                    if Button::secondary(label).compact().show(ui).clicked() {
                        chosen = Some(seconds);
                    }
                }
            });
            ui.add_space(space::L);
            widgets::Field::new()
                .label(copy::set::PAUSE_REASON)
                .placeholder(copy::set::PAUSE_REASON_PLACEHOLDER)
                .show(ui, &mut self.screens.settings.pause_reason);
            if let Some(seconds) = chosen {
                let reason = self.screens.settings.pause_reason.trim().to_string();
                self.pause(seconds, (!reason.is_empty()).then_some(reason));
            }
        }

        widgets::form_group(ui, copy::set::UPCOMING, None);
        let upcoming = self.upcoming_runs();
        if upcoming.is_empty() {
            widgets::text(ui, copy::set::UPCOMING_NONE, Type::Small, t.text_muted);
        } else {
            widgets::table_frame(ui, |ui| {
                egui_extras::TableBuilder::new(ui)
                    .id_salt("upcoming")
                    .cell_layout(Layout::left_to_right(Align::Center))
                    // The settings pane is 200px narrower than a full screen,
                    // so these are sized for it rather than for the content
                    // column the other tables live in.
                    .column(egui_extras::Column::exact(150.0))
                    .column(egui_extras::Column::remainder().at_least(120.0))
                    .column(egui_extras::Column::exact(80.0))
                    .column(egui_extras::Column::exact(110.0))
                    .header(crate::gui::theme::size::TABLE_HEADER_H, |mut header| {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::col::WHEN, None);
                        });
                        header.col(|ui| {
                            widgets::table_header(ui, copy::col::JOB, None);
                        });
                        header.col(|ui| {
                            widgets::table_header(ui, copy::col::TRIGGER, None);
                        });
                        header.col(|ui| {
                            widgets::table_header(ui, copy::set::UPCOMING_BLOCKED_BY, None);
                        });
                    })
                    .body(|body| {
                        body.rows(
                            crate::gui::theme::size::TABLE_ROW_H,
                            upcoming.len(),
                            |mut row| {
                                let index = row.index();
                                let Some((when, job, blocked)) = upcoming.get(index) else {
                                    return;
                                };
                                row.col(|ui| {
                                    widgets::text(ui, when, Type::MonoSmall, t.text_secondary);
                                });
                                row.col(|ui| {
                                    widgets::elided(
                                        ui,
                                        job,
                                        Type::Small,
                                        t.text_primary,
                                        ui.available_width(),
                                        false,
                                    );
                                });
                                row.col(|ui| {
                                    widgets::text(ui, "Schedule", Type::Small, t.text_muted);
                                });
                                row.col(|ui| match blocked {
                                    Some(reason) => {
                                        widgets::text(
                                            ui,
                                            *reason,
                                            Type::Small,
                                            t.warning.tint_text,
                                        );
                                    }
                                    None => {
                                        widgets::text(ui, "", Type::Small, t.text_muted);
                                    }
                                });
                            },
                        );
                    });
            });
        }

        if changed {
            self.save_settings();
        }
    }

    /// The answer to "why did nothing run last night".
    fn upcoming_runs(&self) -> Vec<(String, String, Option<&'static str>)> {
        let now = chrono::Utc::now();
        let mut rows: Vec<(chrono::DateTime<chrono::Utc>, String, Option<&'static str>)> =
            Vec::new();
        for job in &self.data.jobs {
            let blocked = if !job.enabled {
                Some(copy::set::UPCOMING_BLOCKED_DISABLED)
            } else if self.data.paused() {
                Some(copy::set::UPCOMING_BLOCKED_PAUSED)
            } else if !self.data.unlocked() {
                Some(copy::set::UPCOMING_BLOCKED_LOCKED)
            } else {
                None
            };
            for at in crate::gui::viewmodel::next_runs(&job.schedule, now, 3) {
                rows.push((at, job.name.clone(), blocked));
            }
        }
        rows.sort_by_key(|(at, _, _)| *at);
        rows.truncate(10);
        rows.into_iter()
            .map(|(at, name, blocked)| {
                (
                    format!("{} · {}", format::absolute(at), format::relative_future(at, now)),
                    name,
                    blocked,
                )
            })
            .collect()
    }

    fn settings_bandwidth(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let mut changed = false;

        // Both rows share one label column, so the boxes, units and Mbit
        // readouts line up in columns rather than each starting wherever its
        // own label happened to end.
        let mut upload = self.data.settings.bandwidth.upload_kbps;
        if widgets::bandwidth_control(ui, "bw-up", copy::set::BW_UPLOAD, &mut upload) {
            changed = true;
        }
        self.data.settings.bandwidth.upload_kbps = upload;

        ui.add_space(space::L);
        let mut download = self.data.settings.bandwidth.download_kbps;
        if widgets::bandwidth_control(ui, "bw-down", copy::set::BW_DOWNLOAD, &mut download) {
            changed = true;
        }
        self.data.settings.bandwidth.download_kbps = download;
        ui.add_space(space::S);
        widgets::paragraph_at(ui, copy::set::BW_DOWNLOAD_BODY, Type::Small, t.text_muted, 560.0);

        // The group is named for what it is; the toggle carries the sentence.
        widgets::form_group(ui, "Daily window", None);
        let mut window_on = self.data.settings.bandwidth.schedule.is_some();
        if widgets::toggle(ui, &mut window_on, copy::set::BW_WINDOW, None, true).clicked() {
            self.data.settings.bandwidth.schedule = window_on.then(|| BandwidthWindow {
                start_minute: 9 * 60,
                end_minute: 18 * 60,
                upload_kbps: Some(500),
                download_kbps: None,
                weekdays: vec![0, 1, 2, 3, 4],
            });
            changed = true;
        }

        if let Some(window) = self.data.settings.bandwidth.schedule.clone() {
            let mut window = window;
            ui.add_space(space::L);
            ui.horizontal(|ui| {
                widgets::text(ui, copy::set::BW_FROM, Type::Small, t.text_secondary);
                let mut start_h = window.start_minute / 60;
                let mut start_m = window.start_minute % 60;
                widgets::number(ui, &mut start_h, 0..=23, "h", true, copy::set::BW_FROM);
                widgets::number(ui, &mut start_m, 0..=59, "m", true, copy::set::BW_FROM);
                window.start_minute = start_h * 60 + start_m;
                ui.add_space(space::L);
                widgets::text(ui, copy::set::BW_TO, Type::Small, t.text_secondary);
                let mut end_h = window.end_minute / 60;
                let mut end_m = window.end_minute % 60;
                widgets::number(ui, &mut end_h, 0..=23, "h", true, copy::set::BW_TO);
                widgets::number(ui, &mut end_m, 0..=59, "m", true, copy::set::BW_TO);
                window.end_minute = end_h * 60 + end_m;
            });

            ui.add_space(space::L);
            ui.horizontal(|ui| {
                for (index, label) in format::WEEKDAY_SHORT.iter().enumerate() {
                    let on = window.weekdays.contains(&(index as u8));
                    let button = if on {
                        Button::primary(&label[..2])
                    } else {
                        Button::secondary(&label[..2])
                    };
                    if button.min_width(36.0).show(ui).clicked() {
                        if on {
                            window.weekdays.retain(|d| *d != index as u8);
                        } else {
                            window.weekdays.push(index as u8);
                        }
                        changed = true;
                    }
                }
            });
            if window.weekdays.is_empty() {
                ui.add_space(space::S);
                widgets::text(ui, copy::set::BW_DAYS_NONE, Type::Small, t.text_muted);
            }

            ui.add_space(space::L);
            self.bandwidth_strip(ui, &window);

            ui.add_space(space::L);
            if window.end_minute < window.start_minute {
                widgets::text(ui, copy::set::BW_WRAPS, Type::Small, t.text_muted);
                ui.add_space(space::S);
            }
            widgets::paragraph_at(
                ui,
                copy::set_bw_summary(
                    &format::minutes_of_day(window.start_minute),
                    &format::minutes_of_day(window.end_minute),
                    &format::weekdays(&window.weekdays),
                    &format::kbps(window.upload_kbps),
                    &format::kbps(self.data.settings.bandwidth.upload_kbps),
                ),
                Type::Small,
                t.text_secondary,
                600.0,
            );
            self.data.settings.bandwidth.schedule = Some(window);
        }

        ui.add_space(space::XL);
        widgets::paragraph_at(ui, copy::set::BW_PER_DESTINATION, Type::Small, t.text_muted, 560.0);

        if changed {
            self.save_settings();
        }
    }

    /// The 24-hour strip: the window drawn as an accent block on a track, with
    /// a wrap past midnight rendered as two blocks rather than one wrong one.
    fn bandwidth_strip(&self, ui: &mut Ui, window: &BandwidthWindow) {
        let t = theme::tokens(ui.ctx());
        let width = ui.available_width().min(819.0);
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 40.0), Sense::hover());
        ui.painter().rect_filled(rect, radius::CONTROL, t.bg_raised);
        let x_for = |minute: u32| rect.left() + rect.width() * (minute.min(1440) as f32 / 1440.0);
        for hour in 0..=24 {
            let x = x_for(hour * 60);
            ui.painter().line_segment(
                [egui::Pos2::new(x, rect.bottom() - 8.0), egui::Pos2::new(x, rect.bottom())],
                egui::Stroke::new(1.0_f32, t.border_subtle),
            );
        }
        let fill = theme::alpha(t.accent, 0.3);
        let mut blocks = Vec::new();
        if window.end_minute >= window.start_minute {
            blocks.push((window.start_minute, window.end_minute));
        } else {
            blocks.push((window.start_minute, 1440));
            blocks.push((0, window.end_minute));
        }
        for (start, end) in blocks {
            let block = egui::Rect::from_min_max(
                egui::Pos2::new(x_for(start), rect.top() + 4.0),
                egui::Pos2::new(x_for(end), rect.bottom() - 10.0),
            );
            ui.painter().rect_filled(block, egui::CornerRadius::same(4), fill);
        }
        let label = format::kbps(window.upload_kbps);
        let g = widgets::galley(ui, label, Type::MonoSmall, t.text_primary);
        ui.painter().galley(
            egui::Pos2::new(x_for(window.start_minute) + 6.0, rect.top() + 8.0),
            g,
            t.text_primary,
        );
        // The limit outside the window, labelled at both ends, so the strip
        // states both halves of the rule rather than only the exception.
        let outside = format::kbps(self.data.settings.bandwidth.upload_kbps);
        let left = widgets::galley(ui, outside.clone(), Type::MonoSmall, t.text_muted);
        let right = widgets::galley(ui, outside, Type::MonoSmall, t.text_muted);
        let right_width = right.size().x;
        ui.painter().galley(
            egui::Pos2::new(rect.left() + 6.0, rect.top() + 8.0),
            left,
            t.text_muted,
        );
        ui.painter().galley(
            egui::Pos2::new(rect.right() - 6.0 - right_width, rect.top() + 8.0),
            right,
            t.text_muted,
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Label,
                true,
                format!(
                    "Bandwidth window from {} to {}",
                    format::minutes_of_day(window.start_minute),
                    format::minutes_of_day(window.end_minute)
                ),
            )
        });
    }

    fn settings_notifications(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let mut changed = false;
        let n = &mut self.data.settings.notifications;

        let mut enabled = n.enabled;
        if widgets::toggle(ui, &mut enabled, copy::set::NOTIF_ENABLED, None, true).clicked() {
            n.enabled = enabled;
            changed = true;
        }
        ui.add_space(space::XL);

        // Everything below is disabled, not hidden, when the master is off.
        let on = n.enabled;
        let mut failure = n.on_failure;
        if widgets::toggle(ui, &mut failure, copy::set::NOTIF_ON_FAILURE, None, on).clicked() {
            n.on_failure = failure;
            changed = true;
        }
        ui.add_space(space::L);
        let mut success = n.on_success;
        if widgets::toggle(
            ui,
            &mut success,
            copy::set::NOTIF_ON_SUCCESS,
            Some(copy::set::NOTIF_ON_SUCCESS_BODY),
            on,
        )
        .clicked()
        {
            n.on_success = success;
            changed = true;
        }
        ui.add_space(space::L);
        ui.horizontal(|ui| {
            widgets::text(ui, copy::set::NOTIF_STALE, Type::Body, t.text_primary);
            ui.add_space(space::M);
            let mut days = n.stale_after_days;
            widgets::number(
                ui,
                &mut days,
                0..=90,
                copy::set::NOTIF_STALE_UNIT,
                on,
                copy::set::NOTIF_STALE,
            );
            if days != n.stale_after_days {
                n.stale_after_days = days;
                changed = true;
            }
        });
        ui.add_space(space::XS);
        widgets::paragraph_at(ui, copy::set::NOTIF_STALE_BODY, Type::Small, t.text_muted, 560.0);
        ui.add_space(space::L);
        let mut service = n.on_service_error;
        if widgets::toggle(ui, &mut service, copy::set::NOTIF_SERVICE, None, on).clicked() {
            n.on_service_error = service;
            changed = true;
        }
        ui.add_space(space::L);
        ui.horizontal(|ui| {
            widgets::text(ui, copy::set::NOTIF_DEDUPE, Type::Body, t.text_primary);
            ui.add_space(space::M);
            let mut minutes = n.dedupe_minutes;
            widgets::number(
                ui,
                &mut minutes,
                0..=1_440,
                copy::set::NOTIF_DEDUPE_UNIT,
                on,
                copy::set::NOTIF_DEDUPE,
            );
            if minutes != n.dedupe_minutes {
                n.dedupe_minutes = minutes;
                changed = true;
            }
        });

        ui.add_space(space::XL);
        if Button::secondary(copy::set::NOTIF_TEST).enabled(on).show(ui).clicked() {
            // This used to show only an in-app toast reading "A test
            // notification was sent" — while sending nothing at all. The whole
            // point of the button is to find out whether the operating system
            // will actually show one, so it now sends a real notification and
            // reports what the platform said rather than assuming success.
            use superbackup_core::platform::notify::{
                Notification, NotificationKind, Notifier, NotifyOutcome,
            };
            let notifier = Notifier::new(self.data.settings.notifications.clone());
            let outcome = notifier.notify(&Notification::new(
                NotificationKind::Info,
                copy::set::NOTIF_TEST_TITLE,
                copy::set::NOTIF_TEST_SENT,
            ));
            match outcome {
                NotifyOutcome::Shown => self.toasts.info(copy::set::NOTIF_TEST_BODY),
                NotifyOutcome::Unavailable { reason } => {
                    self.toasts.warning(format!("{} {reason}", copy::set::NOTIF_TEST_UNAVAILABLE))
                }
                NotifyOutcome::Failed { reason } => {
                    self.toasts.warning(format!("{} {reason}", copy::set::NOTIF_TEST_FAILED))
                }
                // The test bypasses the dedupe window and the per-kind
                // switches deliberately: the user asked for one now.
                other => {
                    self.toasts.warning(format!("{} ({other:?})", copy::set::NOTIF_TEST_SUPPRESSED))
                }
            }
        }
        if !superbackup_core::platform::capabilities().notifications {
            ui.add_space(space::L);
            widgets::banner(
                ui,
                widgets::BannerKind::Warning,
                copy::set::NOTIF_BLOCKED,
                None,
                |_| {},
            );
        }

        if changed {
            self.save_settings();
        }
    }

    fn settings_security(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let mut changed = false;
        let unlocked = self.data.unlocked();

        widgets::text(ui, copy::set::SEC_VAULT, Type::H3, t.text_primary);
        ui.add_space(space::M);
        let mut act: Option<&'static str> = None;
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                let (icon, colour) = if unlocked {
                    (Icon::LockOpen, t.success.mark)
                } else {
                    (Icon::Lock, t.danger.mark)
                };
                icon.paint(ui.painter(), rect, colour);
                ui.add_space(space::L);
                widgets::text(
                    ui,
                    if unlocked { copy::vault::UNLOCKED } else { copy::vault::LOCKED },
                    Type::BodyStrong,
                    t.text_primary,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if unlocked {
                        if Button::secondary(copy::action::LOCK_NOW).compact().show(ui).clicked() {
                            act = Some("lock");
                        }
                    } else if Button::primary(copy::action::UNLOCK).compact().show(ui).clicked() {
                        act = Some("unlock");
                    }
                });
            });
        });

        ui.add_space(space::XL);
        ui.horizontal(|ui| {
            widgets::text(ui, copy::set::SEC_AUTOLOCK, Type::Body, t.text_primary);
            ui.add_space(space::M);
            let mut minutes = self.data.settings.auto_lock_minutes;
            widgets::number(
                ui,
                &mut minutes,
                0..=1_440,
                copy::set::SEC_AUTOLOCK_UNIT,
                true,
                copy::set::SEC_AUTOLOCK,
            );
            if minutes != self.data.settings.auto_lock_minutes {
                self.data.settings.auto_lock_minutes = minutes;
                changed = true;
            }
        });
        ui.add_space(space::XS);
        widgets::paragraph_at(ui, copy::set::SEC_AUTOLOCK_BODY, Type::Small, t.text_muted, 560.0);
        if self.data.settings.auto_lock_minutes == 0
            && (self.data.settings.run_as_service || self.data.settings.start_at_login)
            && !self.data.settings.use_os_keychain
        {
            ui.add_space(space::M);
            widgets::banner(
                ui,
                widgets::BannerKind::Warning,
                copy::set::SEC_AUTOLOCK_CONFLICT,
                None,
                |_| {},
            );
        }

        widgets::form_group(ui, "Credential store", None);
        let keychain_name = keychain_name();
        let mut keychain = self.data.settings.use_os_keychain;
        if widgets::toggle(ui, &mut keychain, &copy::set_sec_keychain(keychain_name), None, true)
            .clicked()
        {
            if keychain {
                act = Some("keychain-on");
            } else {
                self.data.settings.use_os_keychain = false;
                changed = true;
                self.toasts.info(copy::set_sec_keychain_off(keychain_name));
            }
        }

        widgets::form_group(ui, "Passphrases", None);
        ui.horizontal(|ui| {
            let mut change = Button::secondary(copy::set::SEC_CHANGE).icon(Icon::KeyRound);
            if !unlocked {
                change = change.disabled_because(copy::locked::ACTION_BLOCKED);
            }
            if change.show(ui).clicked() {
                act = Some("change");
            }
            let mut export = Button::danger_ghost(copy::set::SEC_EXPORT);
            if !unlocked {
                export = export.disabled_because(copy::locked::ACTION_BLOCKED);
            }
            if export.show(ui).clicked() {
                act = Some("export");
            }
        });
        ui.add_space(space::S);
        widgets::paragraph_at(ui, copy::set::SEC_EXPORT_BODY, Type::Small, t.text_muted, 560.0);

        widgets::form_group(ui, copy::set::SEC_BACKUPS, Some(copy::set::SEC_BACKUPS_BODY));
        widgets::empty_state(ui, Icon::Archive, &copy::empty::VAULT_BACKUPS, None);

        ui.add_space(space::H2);
        widgets::card_tinted(ui, None, Some(theme::alpha(t.danger.mark, 0.4)), |ui| {
            ui.set_width(ui.available_width());
            widgets::text(ui, copy::job::DANGER_TITLE, Type::H3, t.text_primary);
            ui.add_space(space::M);
            widgets::paragraph_at(
                ui,
                copy::set::SEC_RESET_BODY,
                Type::Small,
                t.text_secondary,
                560.0,
            );
            ui.add_space(space::L);
            if Button::danger(copy::set::SEC_RESET).show(ui).clicked() {
                act = Some("reset-vault");
            }
        });

        match act {
            Some("lock") => self.lock(),
            Some("unlock") => {
                self.open_modal(Modal::Unlock(modals::UnlockState::voluntary()));
            }
            Some("change") => {
                self.open_modal(Modal::ChangePassphrase(Default::default()));
            }
            Some("export") => {
                self.open_modal(Modal::ExportKeys(Default::default()));
            }
            Some("keychain-on") => {
                self.open_modal(Modal::ChangePassphrase(Default::default()));
            }
            Some("reset-vault") => {
                let confirm = modals::reset_vault_confirm(&self.data);
                self.open_modal(Modal::Confirm(confirm));
            }
            _ => {}
        }
        if changed {
            self.save_settings();
        }
    }

    /// `S-6`. The page a user goes to when they want to satisfy themselves
    /// that this thing actually works.
    ///
    /// Four blocks, in the order a doubt occurs: *which* kopia is being run,
    /// *why* that one, *what it printed when we just ran it*, and the managed
    /// build's update policy. Everything else on this screen is a preference;
    /// the first three are evidence.
    fn settings_kopia(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let probe = self.screens.settings.kopia_probe.clone();
        let mut changed = false;
        let mut reveal: Option<String> = None;
        let mut run_checks = false;
        let mut check_update = false;

        // -- where the binary is ------------------------------------------
        widgets::text(ui, copy::kopia::WHERE_TITLE, Type::H3, t.text_primary);
        ui.add_space(space::S);
        widgets::paragraph_at(ui, copy::kopia::WHERE_LEAD, Type::Small, t.text_secondary, 600.0);
        ui.add_space(space::M);

        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            match probe.as_ref().and_then(|p| p.path.clone()) {
                Some(path) => {
                    widgets::kv(ui, copy::kopia::PATH, &path, true);
                    if let Some(p) = probe.as_ref() {
                        widgets::kv(ui, copy::kopia::PROVENANCE, p.provenance.title(), false);
                        widgets::kv(
                            ui,
                            copy::kopia::VERSION,
                            p.version.as_deref().unwrap_or(copy::state::UNKNOWN),
                            false,
                        );
                        if let Some(banner) = &p.banner {
                            widgets::kv(ui, copy::kopia::BANNER, banner, true);
                        }
                        widgets::kv(ui, copy::kopia::MINIMUM, &p.minimum_version, false);
                    }
                    ui.add_space(space::M);
                    if Button::secondary(copy::kopia::REVEAL)
                        .icon(Icon::FolderOpen)
                        .compact()
                        .show(ui)
                        .clicked()
                    {
                        reveal = Some(path);
                    }
                }
                None => {
                    // Before the first probe the daemon's own version string
                    // is all there is, and it is labelled as second-hand
                    // rather than dressed up as a resolved path.
                    match self.data.snapshot.as_ref().and_then(|s| s.kopia_version.clone()) {
                        Some(version) => {
                            widgets::kv(ui, copy::kopia::VERSION, &version, false);
                            ui.add_space(space::S);
                            widgets::paragraph_at(
                                ui,
                                copy::kopia::VERIFY_EMPTY,
                                Type::Small,
                                t.text_muted,
                                600.0,
                            );
                        }
                        None => {
                            widgets::banner(
                                ui,
                                widgets::BannerKind::Danger,
                                copy::kopia::NOT_FOUND,
                                Some(copy::kopia::NOT_FOUND_BODY),
                                |_| {},
                            );
                        }
                    }
                }
            }
            if let Some(detail) = probe.as_ref().and_then(|p| p.detail.clone()) {
                ui.add_space(space::M);
                widgets::paragraph_at(ui, &detail, Type::Small, t.danger.mark, 600.0);
            }
        });

        // -- which route found it -----------------------------------------
        widgets::form_group(ui, copy::kopia::ROUTES_TITLE, Some(copy::kopia::ROUTES_LEAD));
        match probe.as_ref() {
            Some(p) if !p.routes.is_empty() => {
                for route in &p.routes {
                    widgets::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            widgets::text(
                                ui,
                                route.provenance.title(),
                                Type::BodyStrong,
                                t.text_primary,
                            );
                            if route.chosen {
                                ui.add_space(space::M);
                                widgets::badge(
                                    ui,
                                    t.success,
                                    Some(Icon::Check),
                                    copy::kopia::ROUTE_CHOSEN,
                                );
                            }
                        });
                        if let Some(path) = &route.path {
                            ui.add_space(space::XS);
                            widgets::text(ui, path, Type::MonoSmall, t.text_secondary);
                        }
                        ui.add_space(space::XS);
                        widgets::paragraph_at(ui, &route.outcome, Type::Small, t.text_muted, 600.0);
                    });
                    ui.add_space(space::S);
                }
            }
            _ => {
                widgets::paragraph_at(
                    ui,
                    copy::kopia::VERIFY_EMPTY,
                    Type::Small,
                    t.text_muted,
                    600.0,
                );
            }
        }

        ui.add_space(space::L);
        let automatic = self.data.settings.kopia_path.is_none();
        if widgets::radio(ui, automatic, copy::set::KOPIA_AUTO, None, true).clicked() {
            self.data.settings.kopia_path = None;
            changed = true;
        }
        if widgets::radio(ui, !automatic, copy::set::KOPIA_SPECIFIC, None, true).clicked() {
            self.data.settings.kopia_path =
                Some(std::path::PathBuf::from(self.screens.settings.kopia_path.clone()));
            changed = true;
        }
        if !automatic {
            ui.horizontal_top(|ui| {
                ui.add_space(28.0);
                widgets::Field::new()
                    .width(400.0)
                    .mono()
                    .show(ui, &mut self.screens.settings.kopia_path);
                if Button::secondary(copy::action::BROWSE).show(ui).clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        self.screens.settings.kopia_path = path.to_string_lossy().into_owned();
                        self.data.settings.kopia_path = Some(path);
                        changed = true;
                    }
                }
            });
        }
        let mut prefer = self.data.settings.kopia.prefer_system_binary;
        if widgets::toggle(ui, &mut prefer, copy::kopia::PREFER_SYSTEM, None, automatic).clicked() {
            self.data.settings.kopia.prefer_system_binary = prefer;
            changed = true;
        }

        // -- prove it ------------------------------------------------------
        widgets::form_group(ui, copy::kopia::VERIFY_TITLE, Some(copy::kopia::VERIFY_LEAD));
        let repositories: Vec<(Uuid, String)> = self
            .data
            .destinations
            .iter()
            .filter(|d| d.kind.is_repository())
            .map(|d| (d.id, d.name.clone()))
            .collect();
        ui.horizontal(|ui| {
            let mut run = Button::primary(copy::kopia::VERIFY_BUTTON).icon(Icon::Play);
            if self.screens.settings.kopia_probing {
                run = run.busy(true);
            }
            if run.show(ui).clicked() {
                run_checks = true;
            }
            ui.add_space(space::M);
            // Index 0 is deliberately "version only": that half needs no
            // unlocked vault, so it is the one option that always works.
            let mut options = vec![copy::kopia::VERIFY_AGAINST_NONE.to_string()];
            options.extend(repositories.iter().map(|(_, n)| n.clone()));
            let selected = self.screens.settings.kopia_probe_destination;
            let mut index = selected
                .and_then(|id| repositories.iter().position(|(d, _)| *d == id).map(|i| i + 1))
                .unwrap_or(0);
            if widgets::combo(ui, "kopia-probe-destination", &mut index, &options, 260.0, true) {
                self.screens.settings.kopia_probe_destination =
                    index.checked_sub(1).and_then(|i| repositories.get(i)).map(|(id, _)| *id);
            }
        });
        ui.add_space(space::S);
        widgets::paragraph_at(ui, copy::kopia::COMMAND_LINE_NOTE, Type::Small, t.text_muted, 600.0);

        ui.add_space(space::L);
        if let Some(error) = &self.screens.settings.kopia_probe_error {
            widgets::banner(ui, widgets::BannerKind::Danger, error, None, |_| {});
            ui.add_space(space::L);
        }
        match probe.as_ref() {
            Some(p) if !p.invocations.is_empty() => {
                for run in &p.invocations {
                    invocation_block(ui, &crate::gui::viewmodel::invocation_view(run));
                    ui.add_space(space::L);
                }
            }
            _ => {
                widgets::paragraph_at(
                    ui,
                    copy::kopia::VERIFY_EMPTY,
                    Type::Small,
                    t.text_muted,
                    600.0,
                );
            }
        }

        // -- the managed build --------------------------------------------
        widgets::form_group(ui, copy::kopia::MANAGED_TITLE, Some(copy::kopia::MANAGED_LEAD));
        if let Some(p) = probe.as_ref() {
            widgets::kv(ui, copy::kopia::MANAGED_PATH, &p.managed_path, true);
            widgets::kv(
                ui,
                copy::kopia::MANAGED_VERSION,
                p.managed_version.as_deref().unwrap_or(copy::kopia::MANAGED_NONE),
                false,
            );
            ui.add_space(space::M);
            match (&p.update_available, &p.update_summary) {
                (Some(version), _) => {
                    widgets::banner(
                        ui,
                        widgets::BannerKind::Info,
                        &copy::kopia_update_available(version),
                        None,
                        |_| {},
                    );
                }
                (None, Some(summary)) => {
                    widgets::text(ui, summary, Type::Small, t.text_secondary);
                }
                (None, None) => {
                    widgets::text(ui, copy::kopia::UPDATE_NONE, Type::Small, t.text_muted);
                }
            }
        } else {
            widgets::text(ui, copy::kopia::UPDATE_NONE, Type::Small, t.text_muted);
        }

        ui.add_space(space::L);
        widgets::text(ui, copy::kopia::UPDATE_POLICY, Type::BodyStrong, t.text_primary);
        ui.add_space(space::S);
        for (policy, label) in [
            (UpdatePolicy::Off, copy::kopia::UPDATE_OFF),
            (UpdatePolicy::Notify, copy::kopia::UPDATE_NOTIFY),
            (UpdatePolicy::Automatic, copy::kopia::UPDATE_AUTOMATIC),
        ] {
            let selected = self.data.settings.kopia.auto_update == policy;
            if widgets::radio(ui, selected, label, None, true).clicked() && !selected {
                self.data.settings.kopia.auto_update = policy;
                changed = true;
            }
        }
        ui.add_space(space::L);
        if Button::secondary(copy::kopia::UPDATE_CHECK).icon(Icon::Download).show(ui).clicked() {
            check_update = true;
        }

        ui.add_space(space::XL);
        widgets::paragraph_at(ui, copy::set::KOPIA_FOLDERS, Type::Small, t.text_muted, 560.0);

        if let Some(path) = reveal {
            self.reveal_in_file_browser(&path);
        }
        if run_checks || check_update {
            let destination = self.screens.settings.kopia_probe_destination;
            self.request_kopia_probe(destination, check_update);
        }
        if changed {
            self.save_settings();
        }
    }

    /// Open the platform's file browser at a file, selecting it where the
    /// platform supports that.
    ///
    /// Best effort by nature — a headless session or a locked-down desktop has
    /// no file browser to open — so a failure is a toast rather than an error
    /// state on a page whose subject is something else.
    fn reveal_in_file_browser(&mut self, path: &str) {
        let target = std::path::Path::new(path);
        // Selecting the file is nicer than opening its folder, but only some
        // platforms can. Falling back to the parent directory is better than
        // failing, because the user's actual goal is to see the file.
        let opened = if cfg!(target_os = "macos") {
            open::that_detached(target).is_ok()
        } else {
            match target.parent() {
                Some(dir) => open::that_detached(dir).is_ok(),
                None => open::that_detached(target).is_ok(),
            }
        };
        if !opened {
            self.toasts.warning(copy::kopia::REVEAL_FAILED);
        }
    }

    fn settings_remote(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        widgets::paragraph_at(ui, copy::set::REMOTE_LEAD, Type::Small, t.text_secondary, 600.0);
        widgets::form_group(ui, copy::set::REMOTE_ENABLED, None);

        // `Config.remote` is not carried by any IPC reply, so the window is
        // honest about what it can show rather than rendering empty fields as
        // though they were the truth.
        widgets::banner(
            ui,
            widgets::BannerKind::Info,
            "Remote configuration is set up from the command line in this build.",
            Some("The daemon does not yet publish the remote settings over IPC, so this screen cannot show or change them."),
            |_| {},
        );

        ui.add_space(space::XL);
        let gate = self.data.gate(Action::PullRemote);
        ui.horizontal(|ui| {
            let mut pull = Button::secondary(copy::set::REMOTE_PULL);
            if let Some(reason) = gate.reason() {
                pull = pull.disabled_because(reason);
            }
            if pull.show(ui).clicked() {
                self.ask(Intent::Fire, Request::RemotePull {});
            }
            let mut publish = Button::secondary(copy::set::REMOTE_PUBLISH);
            if let Some(reason) = gate.reason() {
                publish = publish.disabled_because(reason);
            }
            if publish.show(ui).clicked() {
                self.ask(Intent::Fire, Request::RemotePush { message: None });
            }
        });
        ui.add_space(space::L);
        widgets::paragraph_at(
            ui,
            copy::set::REMOTE_PULL_KEEPS_LOCAL,
            Type::Small,
            t.text_muted,
            560.0,
        );
    }

    fn settings_advanced(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let mut changed = false;

        widgets::text(ui, copy::set::ADV_LOG_LEVEL, Type::H3, t.text_primary);
        ui.add_space(space::S);
        let levels =
            [LogLevel::Error, LogLevel::Warn, LogLevel::Info, LogLevel::Debug, LogLevel::Trace];
        let labels: Vec<String> = levels.iter().map(|l| format!("{l:?}")).collect();
        let mut index = levels
            .iter()
            .position(|l| format!("{l:?}") == format!("{:?}", self.data.settings.log_level))
            .unwrap_or(2);
        if widgets::combo(ui, "log-level", &mut index, &labels, 200.0, true) {
            self.data.settings.log_level = levels[index];
            changed = true;
        }
        ui.add_space(space::S);
        widgets::paragraph_at(ui, copy::set::ADV_LOG_LEVEL_BODY, Type::Small, t.text_muted, 560.0);

        ui.add_space(space::XL);
        ui.horizontal(|ui| {
            widgets::text(ui, copy::set::ADV_LOG_DAYS, Type::Body, t.text_primary);
            ui.add_space(space::M);
            let mut days = self.data.settings.log_retention_days;
            widgets::number(
                ui,
                &mut days,
                1..=365,
                copy::set::ADV_LOG_DAYS_UNIT,
                true,
                copy::set::ADV_LOG_DAYS,
            );
            if days != self.data.settings.log_retention_days {
                self.data.settings.log_retention_days = days;
                changed = true;
            }
        });

        widgets::form_group(ui, copy::set::ADV_LOCATIONS, None);
        widgets::paragraph_at(
            ui,
            "The daemon owns these paths; open them from the command line with `superbackup doctor`.",
            Type::Small,
            t.text_muted,
            560.0,
        );

        widgets::form_group(ui, copy::set::ADV_BUNDLE, None);
        widgets::paragraph_at(
            ui,
            copy::set::ADV_BUNDLE_INCLUDES,
            Type::Small,
            t.text_secondary,
            600.0,
        );
        ui.add_space(space::S);
        widgets::paragraph_at(ui, copy::set::ADV_BUNDLE_EXCLUDES, Type::Small, t.text_muted, 600.0);
        ui.add_space(space::L);
        ui.horizontal(|ui| {
            if Button::secondary(copy::set::ADV_BUNDLE_PREVIEW).show(ui).clicked() {
                self.toasts.info(copy::set::ADV_BUNDLE_INCLUDES);
            }
            if Button::secondary(copy::set::ADV_CLEAR_CACHE).show(ui).clicked() {
                self.toasts.info(copy::set::ADV_CACHE_BODY);
            }
        });

        widgets::form_group(ui, copy::set::ADV_DOCTOR, None);
        let mut run_doctor = false;
        if Button::secondary(copy::set::ADV_DOCTOR)
            .icon(Icon::Stethoscope)
            .busy(self.screens.settings.doctor_running)
            .show(ui)
            .clicked()
        {
            run_doctor = true;
        }
        if let Some(report) = self.screens.settings.doctor.clone() {
            ui.add_space(space::L);
            for check in &report.checks {
                let state = match check.status {
                    CheckStatus::Pass => StepState::Done,
                    CheckStatus::Warn => StepState::Pending,
                    CheckStatus::Fail => StepState::Failed,
                    CheckStatus::Skipped => StepState::Pending,
                };
                widgets::checklist_row(ui, state, &check.title, check.detail.as_deref());
                if let Some(hint) = &check.hint {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        widgets::paragraph_at(ui, hint, Type::Small, t.text_muted, 520.0);
                    });
                }
            }
        }
        if run_doctor {
            self.screens.settings.doctor_running = true;
            self.ask(Intent::Doctor, Request::Doctor { fix: false });
        }

        if changed {
            self.save_settings();
        }
    }

    fn settings_reset(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let mut act: Option<&'static str> = None;
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::text(ui, copy::set::RESET_SETTINGS, Type::H3, t.text_primary);
            ui.add_space(space::M);
            widgets::paragraph_at(
                ui,
                copy::set::RESET_SETTINGS_BODY,
                Type::Small,
                t.text_secondary,
                560.0,
            );
            ui.add_space(space::L);
            if Button::secondary(copy::set::RESET_SETTINGS).show(ui).clicked() {
                act = Some("settings");
            }
        });

        ui.add_space(space::H2);
        widgets::card_tinted(ui, None, Some(theme::alpha(t.danger.mark, 0.4)), |ui| {
            ui.set_width(ui.available_width());
            widgets::text(ui, copy::set::RESET_ALL, Type::H3, t.text_primary);
            ui.add_space(space::M);
            widgets::paragraph_at(
                ui,
                copy::set::RESET_ALL_BODY,
                Type::Small,
                t.text_secondary,
                560.0,
            );
            ui.add_space(space::L);
            if Button::danger(copy::set::RESET_ALL).show(ui).clicked() {
                act = Some("all");
            }
        });

        match act {
            Some("settings") => {
                self.open_modal(Modal::Confirm(
                    modals::Confirm::new(
                        copy::set::RESET_SETTINGS,
                        copy::set::RESET_SETTINGS_BODY,
                        copy::set::RESET_SETTINGS,
                    )
                    .action(modals::ConfirmAction::ResetSettings),
                ));
            }
            Some("all") => {
                self.open_modal(Modal::Confirm(
                    modals::Confirm::new(
                        copy::set::RESET_ALL,
                        copy::set::RESET_ALL_BODY,
                        copy::set::RESET_ALL,
                    )
                    .typed_confirmation("superbackup")
                    .action(modals::ConfirmAction::RemoveAllConfiguration),
                ));
            }
            _ => {}
        }
    }
}

/// One kopia invocation, rendered as evidence rather than as a verdict.
///
/// The command line, the exit code and both streams, in monospace with a copy
/// button — the same thing a user would see in a terminal, so they can compare
/// the two. Nothing here is summarised, because a summary is precisely what a
/// person checking the application's claims does not want.
fn invocation_block(ui: &mut Ui, view: &crate::gui::viewmodel::InvocationView) {
    let t = theme::tokens(ui.ctx());
    widgets::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            widgets::text(ui, &view.label, Type::BodyStrong, t.text_primary);
            ui.add_space(space::M);
            let status = if view.not_attempted {
                t.warning
            } else if view.ok {
                t.success
            } else {
                t.danger
            };
            widgets::badge(ui, status, None, &view.status);
            if !view.not_attempted {
                ui.add_space(space::M);
                widgets::text(ui, &view.duration, Type::Small, t.text_muted);
            }
        });

        if view.not_attempted {
            ui.add_space(space::M);
            widgets::paragraph_at(ui, &view.stderr, Type::Small, t.text_secondary, 600.0);
            return;
        }

        ui.add_space(space::M);
        widgets::text(ui, copy::kopia::COMMAND_LINE, Type::Small, t.text_muted);
        ui.add_space(space::XS);
        widgets::code_block(ui, &view.command_line, 80.0, None);
        ui.add_space(space::S);
        widgets::kv(ui, copy::kopia::SECRET_ENV, &view.secret_env, true);

        ui.add_space(space::M);
        widgets::text(ui, copy::kopia::STDOUT, Type::Small, t.text_muted);
        ui.add_space(space::XS);
        widgets::code_block(ui, &view.stdout, 220.0, None);

        ui.add_space(space::M);
        widgets::text(ui, copy::kopia::STDERR, Type::Small, t.text_muted);
        ui.add_space(space::XS);
        widgets::code_block(
            ui,
            &view.stderr,
            160.0,
            (!view.ok).then_some(theme::tokens(ui.ctx()).danger),
        );
    });
}

fn keychain_name() -> &'static str {
    if cfg!(windows) {
        "the Windows Credential Manager"
    } else if cfg!(target_os = "macos") {
        "the macOS Keychain"
    } else {
        "the Secret Service"
    }
}
