//! Toasts: bottom-right of the content area, at most three, stacked upward.
//!
//! `DESIGN_SYSTEM.md` §8.11. Danger toasts never auto-dismiss, hovering pauses
//! the timer, and an identical toast inside sixty seconds is suppressed rather
//! than stacked — a second copy of the same sentence tells the user nothing.

use std::time::{Duration, Instant};

use egui::{Align, Layout, Rect, Sense, Stroke, StrokeKind, Vec2};

use super::icons::Icon;
use super::theme::{self, radius, size, space, Type};
use super::widgets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Info,
    Warning,
    Danger,
}

impl ToastKind {
    fn ttl(self) -> Option<Duration> {
        match self {
            ToastKind::Success => Some(Duration::from_secs(4)),
            ToastKind::Info => Some(Duration::from_secs(5)),
            ToastKind::Warning => Some(Duration::from_secs(8)),
            // A failure the user has not read is not a failure they have seen.
            ToastKind::Danger => None,
        }
    }
    fn icon(self) -> Icon {
        match self {
            ToastKind::Success => Icon::CheckCircle,
            ToastKind::Info => Icon::Info,
            ToastKind::Warning => Icon::AlertTriangle,
            ToastKind::Danger => Icon::XOctagon,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub title: String,
    pub body: Option<String>,
    /// A single action link, right-aligned in the footer.
    pub action: Option<String>,
    created: Instant,
    /// Set while the pointer is over the toast, which pauses the timer.
    paused_at: Option<Instant>,
    elapsed_before_pause: Duration,
}

impl Toast {
    pub fn new(kind: ToastKind, title: impl Into<String>) -> Toast {
        Toast {
            kind,
            title: title.into(),
            body: None,
            action: None,
            created: Instant::now(),
            paused_at: None,
            elapsed_before_pause: Duration::ZERO,
        }
    }
    pub fn body(mut self, body: impl Into<String>) -> Toast {
        self.body = Some(body.into());
        self
    }
    pub fn action(mut self, label: impl Into<String>) -> Toast {
        self.action = Some(label.into());
        self
    }
    fn age(&self) -> Duration {
        match self.paused_at {
            Some(_) => self.elapsed_before_pause,
            None => self.elapsed_before_pause + self.created.elapsed(),
        }
    }
    fn expired(&self) -> bool {
        match self.kind.ttl() {
            Some(ttl) => self.age() >= ttl,
            None => false,
        }
    }
    fn same_as(&self, other: &Toast) -> bool {
        self.kind == other.kind && self.title == other.title && self.body == other.body
    }
}

/// The stack. Owned by the app and drawn last, over everything else.
#[derive(Debug, Default)]
pub struct Toasts {
    items: Vec<Toast>,
    /// The action the user clicked on this frame, for the app to route.
    pub clicked_action: Option<String>,
    /// The single live-region string, updated at most once per second (L7).
    announcement: Option<(String, Instant)>,
}

impl Toasts {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Show a toast, unless an identical one is already on screen — in which
    /// case its timer resets and nothing new appears.
    pub fn push(&mut self, toast: Toast) {
        if let Some(existing) = self.items.iter_mut().find(|t| t.same_as(&toast)) {
            existing.created = Instant::now();
            existing.elapsed_before_pause = Duration::ZERO;
            return;
        }
        let announce = super::copy::a11y_toast(&toast.title, toast.body.as_deref().unwrap_or(""));
        self.items.push(toast);
        // A fourth replaces the oldest rather than growing the stack.
        while self.items.len() > 3 {
            self.items.remove(0);
        }
        let now = Instant::now();
        let recent = self
            .announcement
            .as_ref()
            .map(|(_, at)| now.duration_since(*at) < Duration::from_secs(1))
            .unwrap_or(false);
        if !recent {
            self.announcement = Some((announce, now));
        }
    }

    pub fn success(&mut self, title: impl Into<String>) {
        self.push(Toast::new(ToastKind::Success, title));
    }
    pub fn info(&mut self, title: impl Into<String>) {
        self.push(Toast::new(ToastKind::Info, title));
    }
    pub fn warning(&mut self, title: impl Into<String>) {
        self.push(Toast::new(ToastKind::Warning, title));
    }
    pub fn danger(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.push(Toast::new(ToastKind::Danger, title).body(body));
    }

    /// True while any toast is still counting down, so the app knows to keep
    /// asking for repaints — and to stop asking once they are gone.
    pub fn animating(&self) -> bool {
        self.items.iter().any(|t| t.kind.ttl().is_some())
    }

    /// Draw the stack inside `area`, bottom-right, stacked upward.
    pub fn show(&mut self, ui: &mut egui::Ui, area: Rect) {
        self.items.retain(|t| !t.expired());
        if self.items.is_empty() {
            return;
        }
        let t = theme::tokens(ui.ctx());
        let width = size::TOAST_W.min(area.width() - 40.0);
        let mut bottom = area.bottom() - 16.0;
        let mut dismissed: Option<usize> = None;
        let mut clicked: Option<String> = None;
        let mut hovered_index: Option<usize> = None;

        // Newest at the bottom, older ones rising above it.
        for (index, toast) in self.items.iter().enumerate().rev() {
            let body_lines = toast
                .body
                .as_ref()
                .map(|b| {
                    widgets::galley_wrapped(ui, b.clone(), Type::Small, t.text_secondary, width - 76.0)
                        .size()
                        .y
                })
                .unwrap_or(0.0);
            let height = (52.0_f32).max(24.0 + 20.0 + body_lines);
            let rect = Rect::from_min_size(
                egui::Pos2::new(area.right() - 16.0 - width, bottom - height),
                Vec2::new(width, height),
            );
            bottom = rect.top() - 8.0;

            let response = ui.interact(
                rect,
                egui::Id::new("sb-toast").with(index),
                Sense::click(),
            );
            if response.hovered() {
                hovered_index = Some(index);
            }

            let painter = ui.painter();
            painter.rect(
                rect,
                radius::CARD,
                t.bg_raised,
                Stroke::new(1.0_f32, t.border_control),
                StrokeKind::Inside,
            );
            let status = match toast.kind {
                ToastKind::Success => t.success,
                ToastKind::Info => t.info,
                ToastKind::Warning => t.warning,
                ToastKind::Danger => t.danger,
            };
            toast.kind.icon().paint(
                painter,
                Rect::from_min_size(
                    egui::Pos2::new(rect.left() + 12.0, rect.top() + 14.0),
                    Vec2::splat(16.0),
                ),
                status.mark,
            );

            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(
                        egui::Pos2::new(rect.left() + 40.0, rect.top() + 12.0),
                        egui::Pos2::new(rect.right() - 12.0, rect.bottom() - 10.0),
                    ))
                    .layout(Layout::top_down(Align::Min)),
            );
            child.spacing_mut().item_spacing.y = space::XS;
            child.horizontal(|ui| {
                widgets::text(ui, toast.title.clone(), Type::BodyStrong, t.text_primary);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button_compact(
                        ui,
                        Icon::X,
                        super::copy::action::CLOSE,
                        true,
                    )
                    .clicked()
                    {
                        dismissed = Some(index);
                    }
                });
            });
            if let Some(body) = &toast.body {
                widgets::paragraph_at(
                    &mut child,
                    body.clone(),
                    Type::Small,
                    t.text_secondary,
                    width - 76.0,
                );
            }
            if let Some(action) = &toast.action {
                child.with_layout(Layout::right_to_left(Align::Max), |ui| {
                    if widgets::link(ui, action).clicked() {
                        clicked = Some(action.clone());
                    }
                });
            }
        }

        // Hovering pauses the dismiss timer; moving away resumes it.
        for (index, toast) in self.items.iter_mut().enumerate() {
            let is_hovered = hovered_index == Some(index);
            match (is_hovered, toast.paused_at) {
                (true, None) => {
                    toast.elapsed_before_pause += toast.created.elapsed();
                    toast.paused_at = Some(Instant::now());
                }
                (false, Some(_)) => {
                    toast.paused_at = None;
                    toast.created = Instant::now();
                }
                _ => {}
            }
        }

        if let Some(index) = dismissed {
            if index < self.items.len() {
                self.items.remove(index);
            }
        }
        if clicked.is_some() {
            self.clicked_action = clicked;
        }

        // One hidden live region for the whole stack.
        if let Some((text, _)) = &self.announcement {
            let response = ui.interact(
                Rect::from_min_size(area.left_bottom(), Vec2::splat(1.0)),
                egui::Id::new("sb-toast-live-region"),
                Sense::focusable_noninteractive(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_three_toasts_are_kept() {
        let mut toasts = Toasts::default();
        for i in 0..5 {
            toasts.push(Toast::new(ToastKind::Info, format!("toast {i}")));
        }
        assert_eq!(toasts.len(), 3);
        assert_eq!(toasts.items[0].title, "toast 2", "the oldest is replaced");
    }

    #[test]
    fn an_identical_toast_is_suppressed_rather_than_stacked() {
        let mut toasts = Toasts::default();
        toasts.push(Toast::new(ToastKind::Danger, "Dev code failed").body("no route to host"));
        toasts.push(Toast::new(ToastKind::Danger, "Dev code failed").body("no route to host"));
        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn danger_toasts_never_expire_on_their_own() {
        assert!(ToastKind::Danger.ttl().is_none());
        assert_eq!(ToastKind::Success.ttl(), Some(Duration::from_secs(4)));
        assert_eq!(ToastKind::Warning.ttl(), Some(Duration::from_secs(8)));
    }

    #[test]
    fn a_stack_of_only_danger_toasts_stops_asking_for_repaints() {
        let mut toasts = Toasts::default();
        toasts.danger("Dev code failed", "the endpoint rejected these credentials");
        assert!(!toasts.animating());
        toasts.success("Documents finished");
        assert!(toasts.animating());
    }
}
