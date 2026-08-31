//! `S-1` … `S-9`. Two panes: a 200px section list on the left, the section on
//! the right. Everything applies immediately and is saved on change, which is
//! why this screen uses toggles rather than checkboxes.

use egui::{Align, Layout, Sense, Ui, Vec2};

use superbackup_core::ipc::protocol::{CheckStatus, DoctorReply, Request};
use superbackup_core::model::{BandwidthWindow, LogLevel, Theme};

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
}

impl State {
    pub fn doctor_arrived(&mut self, reply: DoctorReply) {
        self.doctor_running = false;
        self.doctor = Some(reply);
    }
    pub fn busy(&self) -> bool {
        self.doctor_running
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
        let mut label = self.data.machine_label().to_string();
        widgets::Field::new().label(copy::set::MACHINE_LABEL).char_limit(64).show(ui, &mut label);
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

        let mut upload = self.data.settings.bandwidth.upload_kbps;
        ui.horizontal(|ui| {
            let mut on = upload.is_some();
            if widgets::checkbox(ui, &mut on, copy::set::BW_UPLOAD, None, true).clicked() {
                upload = if on { Some(2000) } else { None };
                changed = true;
            }
            if let Some(value) = &mut upload {
                ui.add_space(space::M);
                let before = *value;
                widgets::number(
                    ui,
                    value,
                    1..=10_000_000,
                    copy::set::BW_UNIT,
                    true,
                    copy::set::BW_UPLOAD,
                );
                if *value != before {
                    changed = true;
                }
                ui.add_space(space::M);
                widgets::text(ui, format::kbps_as_mbit(*value), Type::Small, t.text_muted);
            }
        });
        self.data.settings.bandwidth.upload_kbps = upload;

        ui.add_space(space::XL);
        let mut download = self.data.settings.bandwidth.download_kbps;
        ui.horizontal(|ui| {
            let mut on = download.is_some();
            if widgets::checkbox(ui, &mut on, copy::set::BW_DOWNLOAD, None, true).clicked() {
                download = if on { Some(2000) } else { None };
                changed = true;
            }
            if let Some(value) = &mut download {
                ui.add_space(space::M);
                widgets::number(
                    ui,
                    value,
                    1..=10_000_000,
                    copy::set::BW_UNIT,
                    true,
                    copy::set::BW_DOWNLOAD,
                );
                ui.add_space(space::M);
                widgets::text(ui, format::kbps_as_mbit(*value), Type::Small, t.text_muted);
            }
        });
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
            self.toasts.info(copy::set::NOTIF_TEST_BODY);
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
                self.toasts.warning(copy::set::SEC_EXPORT_BODY);
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

    fn settings_kopia(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        match self.data.snapshot.as_ref().and_then(|s| s.kopia_version.clone()) {
            Some(version) => {
                widgets::banner(
                    ui,
                    widgets::BannerKind::Success,
                    &copy::set_kopia_found(&version),
                    None,
                    |_| {},
                );
            }
            None => {
                widgets::banner(
                    ui,
                    widgets::BannerKind::Danger,
                    copy::err::KOPIA_MISSING,
                    Some(copy::onboarding::KOPIA_MISSING_BODY),
                    |_| {},
                );
            }
        }

        widgets::form_group(ui, "Where superbackup looks", None);
        let automatic = self.data.settings.kopia_path.is_none();
        let mut changed = false;
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

        ui.add_space(space::XL);
        ui.horizontal(|ui| {
            if Button::secondary(copy::set::KOPIA_CHECK).show(ui).clicked() {
                self.refresh();
            }
            if Button::secondary(copy::set::KOPIA_DOWNLOAD).icon(Icon::Download).show(ui).clicked()
            {
                self.toasts.info(copy::onboarding::KOPIA_MISSING_BODY);
            }
        });

        ui.add_space(space::XL);
        widgets::paragraph_at(ui, copy::set::KOPIA_FOLDERS, Type::Small, t.text_muted, 560.0);

        if changed {
            self.save_settings();
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

fn keychain_name() -> &'static str {
    if cfg!(windows) {
        "the Windows Credential Manager"
    } else if cfg!(target_os = "macos") {
        "the macOS Keychain"
    } else {
        "the Secret Service"
    }
}
