//! The eighteen components of `DESIGN_SYSTEM.md` §8, and nothing else.
//!
//! Every component reads its colours from [`crate::gui::theme::Tokens`], draws
//! its own focus ring 2px outside the control, and supplies a `WidgetInfo` so
//! that custom painters are not invisible to AccessKit (L7). A screen that
//! wants a control not in this file is a screen that has drifted from the
//! design system.

use std::sync::Arc;

use egui::{
    Align, Color32, CornerRadius, FontId, Galley, Id, Layout, Pos2, Rect, Response, Sense,
    Stroke, StrokeKind, TextWrapMode, Ui, Vec2, WidgetInfo, WidgetType,
};

use super::icons::Icon;
use super::theme::{self, radius, size, space, Status, Tokens, Type};

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// Lay out a single line in one of the named type styles.
pub fn galley(ui: &Ui, text: impl Into<String>, ty: Type, color: Color32) -> Arc<Galley> {
    ui.fonts(|f| f.layout_no_wrap(text.into(), ty.font(), color))
}

/// Lay out wrapped text at a fixed measure. Body paragraphs are capped at 68
/// characters by the caller passing the right width.
pub fn galley_wrapped(
    ui: &Ui,
    text: impl Into<String>,
    ty: Type,
    color: Color32,
    width: f32,
) -> Arc<Galley> {
    ui.fonts(|f| f.layout(text.into(), ty.font(), color, width))
}

/// A plain run of text in a named style. Returns the response so callers can
/// attach a tooltip.
pub fn text(ui: &mut Ui, value: impl Into<String>, ty: Type, color: Color32) -> Response {
    let g = galley(ui, value, ty, color);
    let (rect, response) = ui.allocate_exact_size(g.size(), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.min, g, color);
    }
    response
}

/// Wrapped text at the current available width.
pub fn paragraph(ui: &mut Ui, value: impl Into<String>, ty: Type, color: Color32) -> Response {
    let width = ui.available_width().max(1.0);
    paragraph_at(ui, value, ty, color, width)
}

pub fn paragraph_at(
    ui: &mut Ui,
    value: impl Into<String>,
    ty: Type,
    color: Color32,
    width: f32,
) -> Response {
    let g = galley_wrapped(ui, value, ty, color, width);
    let (rect, response) = ui.allocate_exact_size(g.size(), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.min, g, color);
    }
    response
}

/// Draw text clipped to `width`, middle-eliding when it does not fit, with the
/// full value restored in a zero-delay tooltip (L3, `DESIGN_SYSTEM.md` §8.14).
pub fn elided(
    ui: &mut Ui,
    value: &str,
    ty: Type,
    color: Color32,
    width: f32,
    from_left: bool,
) -> Response {
    let shown = elide_to_width(ui, value, ty, width, from_left);
    let g = galley(ui, shown.clone(), ty, color);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, g.size().y), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.min, g, color);
    }
    if shown != value {
        response.on_hover_text(value)
    } else {
        response
    }
}

/// The pixel-aware half of `format::elide_middle`. Binary-searches the
/// character budget against the real font metrics rather than guessing an
/// average glyph width.
pub fn elide_to_width(ui: &Ui, value: &str, ty: Type, width: f32, from_left: bool) -> String {
    let font = ty.font();
    let measure = |s: &str| ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x);
    if measure(value) <= width {
        return value.to_string();
    }
    let total = value.chars().count();
    let (mut lo, mut hi) = (4usize, total);
    let mut best = String::from("…");
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let candidate = if from_left {
            super::format::elide_left(value, mid)
        } else {
            super::format::elide_middle(value, mid)
        };
        if measure(&candidate) <= width {
            best = candidate;
            lo = mid + 1;
        } else {
            if mid == 0 {
                break;
            }
            hi = mid - 1;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Focus ring (DESIGN_SYSTEM.md §2.3)
// ---------------------------------------------------------------------------

/// 2px, 2px outside the control, radius + 2, never animated.
pub fn focus_ring(ui: &Ui, rect: Rect, cr: CornerRadius) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_stroke(
        rect.expand(2.0),
        theme::ring_radius(cr),
        Stroke::new(2.0_f32, t.border_focus),
        StrokeKind::Outside,
    );
}

// ---------------------------------------------------------------------------
// 8.1 Button
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Ghost,
    Danger,
    DangerGhost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonSize {
    Normal,
    Compact,
    Onboarding,
}

impl ButtonSize {
    fn height(self) -> f32 {
        match self {
            ButtonSize::Normal => size::CONTROL_H,
            ButtonSize::Compact => size::CONTROL_H_COMPACT,
            ButtonSize::Onboarding => size::CONTROL_H_ONBOARDING,
        }
    }
    fn label_style(self) -> Type {
        match self {
            ButtonSize::Compact => Type::SmallStrong,
            _ => Type::BodyStrong,
        }
    }
}

/// A button, specified rather than built, so that every call site states the
/// same set of decisions.
pub struct Button<'a> {
    label: &'a str,
    variant: Variant,
    size: ButtonSize,
    icon: Option<Icon>,
    enabled: bool,
    busy: bool,
    /// Why the button is disabled. Rendered as the tooltip and appended to the
    /// AccessKit label, so a disabled control always explains itself.
    disabled_reason: Option<&'a str>,
    tooltip: Option<&'a str>,
    min_width: Option<f32>,
    /// Overrides the AccessKit label, which must make sense out of context
    /// (`Run job "Dev code" now`, not `Run now`).
    a11y: Option<String>,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str, variant: Variant) -> Self {
        Button {
            label,
            variant,
            size: ButtonSize::Normal,
            icon: None,
            enabled: true,
            busy: false,
            disabled_reason: None,
            tooltip: None,
            min_width: None,
            a11y: None,
        }
    }
    pub fn primary(label: &'a str) -> Self {
        Self::new(label, Variant::Primary)
    }
    pub fn secondary(label: &'a str) -> Self {
        Self::new(label, Variant::Secondary)
    }
    pub fn ghost(label: &'a str) -> Self {
        Self::new(label, Variant::Ghost)
    }
    pub fn danger(label: &'a str) -> Self {
        Self::new(label, Variant::Danger)
    }
    pub fn danger_ghost(label: &'a str) -> Self {
        Self::new(label, Variant::DangerGhost)
    }
    pub fn compact(mut self) -> Self {
        self.size = ButtonSize::Compact;
        self
    }
    pub fn onboarding(mut self) -> Self {
        self.size = ButtonSize::Onboarding;
        self
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }
    /// Disable with a reason. The reason is the tooltip and is appended to the
    /// screen-reader label; `DESIGN_SYSTEM.md` forbids a silently dead control.
    pub fn disabled_because(mut self, reason: &'a str) -> Self {
        self.enabled = false;
        self.disabled_reason = Some(reason);
        self
    }
    pub fn blocked_when(mut self, blocked: bool, reason: &'a str) -> Self {
        if blocked {
            self.enabled = false;
            self.disabled_reason = Some(reason);
        }
        self
    }
    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = Some(w);
        self
    }
    pub fn a11y(mut self, label: impl Into<String>) -> Self {
        self.a11y = Some(label.into());
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let t = theme::tokens(ui.ctx());
        let h = self.size.height();
        let label_style = self.size.label_style();
        let g = galley(ui, self.label, label_style, Color32::WHITE);

        let icon_size = if self.size == ButtonSize::Compact { 14.0 } else { 16.0 };
        let pad = if self.size == ButtonSize::Compact { 10.0 } else { 12.0 };
        // The busy spinner's 22px is reserved at layout time so a button never
        // changes width when it starts working (§8.1).
        let leading = if self.icon.is_some() { icon_size + space::M } else { 0.0 };
        let trailing = if self.busy { 22.0 } else { 0.0 };
        let mut w = pad * 2.0 + leading + g.size().x + trailing;
        if let Some(min) = self.min_width {
            w = w.max(min);
        }

        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(w, h), if self.enabled { Sense::click() } else { Sense::hover() });

        if ui.is_rect_visible(rect) {
            let hovered = response.hovered() && self.enabled;
            let pressed = response.is_pointer_button_down_on() && self.enabled;
            let (fill, stroke, fg) = self.colours(&t, hovered, pressed);

            ui.painter().rect(rect, radius::CONTROL, fill, stroke, StrokeKind::Inside);
            if response.has_focus() {
                focus_ring(ui, rect, radius::CONTROL);
            }

            let mut x = rect.left() + pad;
            if let Some(icon) = self.icon {
                let ir = Rect::from_min_size(
                    Pos2::new(x, rect.center().y - icon_size / 2.0),
                    Vec2::splat(icon_size),
                );
                icon.paint(ui.painter(), ir, fg);
                x += icon_size + space::M;
            }
            let g = galley(ui, self.label, label_style, fg);
            ui.painter()
                .galley(Pos2::new(x, rect.center().y - g.size().y / 2.0), g, fg);
            if self.busy {
                let sr = Rect::from_center_size(
                    Pos2::new(rect.right() - pad - 7.0, rect.center().y),
                    Vec2::splat(14.0),
                );
                let turns = ui.input(|i| i.time as f32) * 0.75;
                Icon::RefreshCw.paint_rotated(ui.painter(), sr, fg, turns);
                ui.ctx().request_repaint();
            }
        }

        let a11y_label = self.a11y.clone().unwrap_or_else(|| self.label.to_string());
        let a11y_label = if self.busy {
            super::copy::a11y_busy(&a11y_label)
        } else if let Some(reason) = self.disabled_reason {
            format!("{a11y_label}, {reason}")
        } else {
            a11y_label
        };
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, self.enabled, &a11y_label));

        if let Some(reason) = self.disabled_reason {
            response.on_hover_text(reason)
        } else if let Some(tip) = self.tooltip {
            response.on_hover_text(tip)
        } else {
            response
        }
    }

    fn colours(&self, t: &Tokens, hovered: bool, pressed: bool) -> (Color32, Stroke, Color32) {
        if !self.enabled {
            return match self.variant {
                Variant::Primary | Variant::Danger => (
                    t.bg_raised,
                    Stroke::new(1.0_f32, t.border_subtle),
                    t.text_disabled,
                ),
                Variant::Secondary => (
                    theme::alpha(t.bg_raised, 0.5),
                    Stroke::new(1.0_f32, t.border_subtle),
                    t.text_disabled,
                ),
                Variant::Ghost | Variant::DangerGhost => {
                    (Color32::TRANSPARENT, Stroke::NONE, t.text_disabled)
                }
            };
        }
        match self.variant {
            Variant::Primary => {
                let fill = if pressed {
                    t.accent_fill_active
                } else if hovered {
                    t.accent_fill_hover
                } else {
                    t.accent_fill
                };
                (fill, Stroke::new(1.0_f32, t.accent_fill_border), t.text_oncolor)
            }
            Variant::Danger => {
                let fill = if pressed {
                    t.danger_fill_active
                } else if hovered {
                    t.danger_fill_hover
                } else {
                    t.danger_fill
                };
                (fill, Stroke::new(1.0_f32, t.danger_fill_border), Color32::WHITE)
            }
            Variant::Secondary => {
                let fill = if pressed {
                    t.bg_canvas
                } else if hovered {
                    t.bg_surface_hover
                } else {
                    t.bg_raised
                };
                let border = if hovered {
                    theme::alpha(t.border_focus, 0.6)
                } else {
                    t.border_control
                };
                (fill, Stroke::new(1.0_f32, border), t.text_primary)
            }
            Variant::Ghost => {
                let fill = if pressed {
                    t.bg_canvas
                } else if hovered {
                    t.bg_raised
                } else {
                    Color32::TRANSPARENT
                };
                let fg = if hovered { t.text_primary } else { t.text_secondary };
                (fill, Stroke::NONE, fg)
            }
            Variant::DangerGhost => {
                let fill = if hovered || pressed {
                    t.danger.tint_bg
                } else {
                    Color32::TRANSPARENT
                };
                let fg = if hovered || pressed {
                    t.danger.tint_text
                } else {
                    t.danger.mark
                };
                (fill, Stroke::NONE, fg)
            }
        }
    }
}

/// 30 × 30 ghost icon button. The tooltip and the AccessKit label are
/// mandatory, so both are ordinary arguments rather than options.
pub fn icon_button(ui: &mut Ui, icon: Icon, label: &str, enabled: bool) -> Response {
    icon_button_sized(ui, icon, label, enabled, size::CONTROL_H)
}

pub fn icon_button_compact(ui: &mut Ui, icon: Icon, label: &str, enabled: bool) -> Response {
    icon_button_sized(ui, icon, label, enabled, size::CONTROL_H_COMPACT)
}

fn icon_button_sized(ui: &mut Ui, icon: Icon, label: &str, enabled: bool, s: f32) -> Response {
    let t = theme::tokens(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(s), if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() && enabled;
        let fill = if hovered { t.bg_raised } else { Color32::TRANSPARENT };
        ui.painter().rect_filled(rect, radius::CONTROL, fill);
        if response.has_focus() {
            focus_ring(ui, rect, radius::CONTROL);
        }
        let fg = if !enabled {
            t.text_disabled
        } else if hovered {
            t.text_primary
        } else {
            t.text_secondary
        };
        let inner = Rect::from_center_size(rect.center(), Vec2::splat(16.0));
        icon.paint(ui.painter(), inner, fg);
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    response.on_hover_text(label)
}

// ---------------------------------------------------------------------------
// 8.9 Badge
// ---------------------------------------------------------------------------

/// A 20px status pill: tint background, tint text, a leading 14px icon, and a
/// word. Never colour alone.
pub fn badge(ui: &mut Ui, status: Status, icon: Option<Icon>, label: &str) -> Response {
    badge_spinning(ui, status, icon, label, false)
}

pub fn badge_spinning(
    ui: &mut Ui,
    status: Status,
    icon: Option<Icon>,
    label: &str,
    spin: bool,
) -> Response {
    let g = galley(ui, label, Type::SmallStrong, status.tint_text);
    let icon_w = if icon.is_some() { 14.0 + space::S } else { 0.0 };
    let w = 8.0 * 2.0 + icon_w + g.size().x;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(w, size::BADGE_H), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, radius::BADGE, status.tint_bg);
        let mut x = rect.left() + 8.0;
        if let Some(icon) = icon {
            let ir = Rect::from_min_size(
                Pos2::new(x, rect.center().y - 7.0),
                Vec2::splat(14.0),
            );
            if spin {
                let turns = ui.input(|i| i.time as f32) * 0.75;
                icon.paint_rotated(ui.painter(), ir, status.tint_text, turns);
            } else {
                icon.paint(ui.painter(), ir, status.tint_text);
            }
            x += 14.0 + space::S;
        }
        ui.painter().galley(
            Pos2::new(x, rect.center().y - g.size().y / 2.0),
            g,
            status.tint_text,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, label));
    response
}

/// The badge for a run status, with the word and the shape the design system
/// pairs with it.
pub fn status_badge(ui: &mut Ui, status: superbackup_core::state::RunStatus) -> Response {
    use superbackup_core::state::RunStatus as R;
    let t = theme::tokens(ui.ctx());
    let label = match status {
        R::SucceededWithWarnings => super::copy::badge::WARNINGS_SHORT,
        other => other.title(),
    };
    let spin = matches!(status, R::Running | R::Preparing | R::Finalising);
    badge_spinning(ui, t.status_for(status), Some(Icon::for_status(status)), label, spin)
}

/// `Never run` / `Disabled` and the other neutral markers.
pub fn neutral_badge(ui: &mut Ui, label: &str, icon: Option<Icon>) -> Response {
    let t = theme::tokens(ui.ctx());
    badge(ui, t.neutral, icon, label)
}

/// A 24px destination chip. Carries a 6px danger dot at the top right when the
/// last run to that destination failed — the fan-out is visible even here.
pub fn destination_chip(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    problem: Option<Status>,
    max_width: f32,
) -> Response {
    let t = theme::tokens(ui.ctx());
    let shown = elide_to_width(ui, label, Type::Small, max_width - 40.0, false);
    let g = galley(ui, shown.clone(), Type::Small, t.text_secondary);
    let w = 8.0 + 14.0 + space::S + g.size().x + 8.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(w, size::CHIP_H), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect(
            rect,
            radius::BADGE,
            t.bg_raised,
            Stroke::new(1.0_f32, t.border_subtle),
            StrokeKind::Inside,
        );
        let ir = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.center().y - 7.0), Vec2::splat(14.0));
        icon.paint(ui.painter(), ir, t.text_muted);
        ui.painter().galley(
            Pos2::new(rect.left() + 8.0 + 14.0 + space::S, rect.center().y - g.size().y / 2.0),
            g,
            t.text_secondary,
        );
        if let Some(s) = problem {
            ui.painter().circle_filled(Pos2::new(rect.right() - 3.0, rect.top() + 3.0), 3.0, s.mark);
        }
    }
    let a11y = match problem {
        Some(_) => format!("{label}, needs attention"),
        None => label.to_string(),
    };
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &a11y));
    if shown != label {
        response.on_hover_text(label)
    } else {
        response
    }
}

/// `used by 3 destinations` — a count with no icon and no colour.
pub fn count_pill(ui: &mut Ui, label: &str) -> Response {
    let t = theme::tokens(ui.ctx());
    let g = galley(ui, label, Type::SmallStrong, t.text_secondary);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(g.size().x + 16.0, size::BADGE_H), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, radius::BADGE, t.bg_raised);
        ui.painter().galley(
            Pos2::new(rect.left() + 8.0, rect.center().y - g.size().y / 2.0),
            g,
            t.text_secondary,
        );
    }
    response
}

// ---------------------------------------------------------------------------
// 8.6 Card
// ---------------------------------------------------------------------------

/// `bg.surface`, 1px `border.subtle`, radius 10, 16px padding. Cards use
/// borders rather than shadows (L10).
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    card_tinted(ui, None, None, add)
}

pub fn card_tinted<R>(
    ui: &mut Ui,
    fill: Option<Color32>,
    border: Option<Color32>,
    add: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let t = theme::tokens(ui.ctx());
    egui::Frame::new()
        .fill(fill.unwrap_or(t.bg_surface))
        .stroke(Stroke::new(1.0_f32, border.unwrap_or(t.border_subtle)))
        .corner_radius(radius::CARD)
        .inner_margin(egui::Margin::same(16))
        .show(ui, add)
}

/// A section header inside a form: `h3` plus an optional `small` description,
/// preceded by a divider with 20px above and 16px below (§6.2).
pub fn form_group(ui: &mut Ui, title: &str, description: Option<&str>) {
    let t = theme::tokens(ui.ctx());
    ui.add_space(space::XXL);
    divider(ui);
    ui.add_space(space::XL);
    text(ui, title, Type::H3, t.text_primary);
    if let Some(d) = description {
        ui.add_space(space::XS);
        paragraph_at(ui, d, Type::Small, t.text_muted, ui.available_width().min(560.0));
    }
    ui.add_space(space::L);
}

/// A 1px horizontal rule in `border.subtle`.
pub fn divider(ui: &mut Ui) {
    let t = theme::tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0, t.border_subtle);
}

pub fn vertical_rule(ui: &mut Ui, height: f32) {
    let t = theme::tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
    ui.painter().rect_filled(rect, 0, t.border_subtle);
}

/// A section title with an optional count pill and right-aligned actions.
pub fn section_header<R>(
    ui: &mut Ui,
    title: &str,
    count: Option<usize>,
    actions: impl FnOnce(&mut Ui) -> R,
) -> R {
    let t = theme::tokens(ui.ctx());
    let mut out = None;
    ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        text(ui, title, Type::H2, t.text_primary);
        if let Some(c) = count {
            ui.add_space(space::M);
            count_pill(ui, &c.to_string());
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            out = Some(actions(ui));
        });
    });
    out.expect("the right-to-left layout always runs its closure")
}

// ---------------------------------------------------------------------------
// 8.10 Banner
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerKind {
    Info,
    Warning,
    Danger,
    Success,
}

impl BannerKind {
    fn status(self, t: &Tokens) -> Status {
        match self {
            BannerKind::Info => t.info,
            BannerKind::Warning => t.warning,
            BannerKind::Danger => t.danger,
            BannerKind::Success => t.success,
        }
    }
    fn icon(self) -> Icon {
        match self {
            BannerKind::Info => Icon::Info,
            BannerKind::Warning => Icon::AlertTriangle,
            BannerKind::Danger => Icon::XOctagon,
            BannerKind::Success => Icon::CheckCircle,
        }
    }
}

/// A full-width in-content notice: tint background, 1px status border at 40%,
/// 20px leading icon, and optional trailing ghost actions.
pub fn banner<R>(
    ui: &mut Ui,
    kind: BannerKind,
    title: &str,
    body: Option<&str>,
    actions: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let t = theme::tokens(ui.ctx());
    let s = kind.status(&t);
    let mut out = None;
    let response = egui::Frame::new()
        .fill(s.tint_bg)
        .stroke(Stroke::new(1.0_f32, theme::alpha(s.mark, 0.4)))
        .corner_radius(radius::CARD)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_height(size::BANNER_MIN_H - 32.0);
            ui.horizontal(|ui| {
                let (ir, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                kind.icon().paint(ui.painter(), ir, s.mark);
                ui.add_space(space::L);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = space::XS;
                    let measure = (ui.available_width() - 200.0).max(240.0);
                    paragraph_at(ui, title, Type::BodyStrong, t.text_primary, measure);
                    if let Some(b) = body {
                        paragraph_at(ui, b, Type::Small, t.text_secondary, measure);
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    out = Some(actions(ui));
                });
            });
        });
    // An alert must be announced when it appears (§9.3.6).
    response.response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Label,
            true,
            format!("{title}. {}", body.unwrap_or("")),
        )
    });
    out
}

// ---------------------------------------------------------------------------
// 8.8 Progress
// ---------------------------------------------------------------------------

/// A determinate or indeterminate bar. `fraction` of `None` means kopia is
/// still estimating, which renders as a sweeping band and the word
/// `Estimating…` in the caller's label — never as a stuck bar at zero.
pub fn progress_bar(
    ui: &mut Ui,
    width: f32,
    height: f32,
    fraction: Option<f32>,
    fill: Color32,
    a11y_label: &str,
) -> Response {
    let t = theme::tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if ui.is_rect_visible(rect) {
        let cr = CornerRadius::same((height / 2.0).round().clamp(0.0, 255.0) as u8);
        ui.painter().rect_filled(rect, cr, t.progress_track);
        match fraction {
            Some(f) => {
                let f = f.clamp(0.0, 1.0);
                if f > 0.0 {
                    // Minimum visible width of 3px, so "just started" is visible.
                    let w = (rect.width() * f).max(3.0);
                    ui.painter().rect_filled(
                        Rect::from_min_size(rect.min, Vec2::new(w, height)),
                        cr,
                        fill,
                    );
                }
            }
            None => {
                // A 30%-wide band traversing the track over 1600ms.
                let phase = (ui.input(|i| i.time) % 1.6) as f32 / 1.6;
                let band = rect.width() * 0.3;
                let x = rect.left() + phase * (rect.width() + band) - band;
                let band_rect = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(band, height))
                    .intersect(rect);
                if band_rect.width() > 0.0 {
                    ui.painter().rect_filled(band_rect, cr, fill);
                }
                ui.ctx().request_repaint();
            }
        }
    }
    response.widget_info(|| {
        let mut info = WidgetInfo::labeled(WidgetType::ProgressIndicator, true, a11y_label);
        info.value = fraction.map(|f| f as f64);
        info
    });
    response
}

/// The spinner used inside checklists and loading rows.
pub fn spinner(ui: &mut Ui, size_px: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size_px), Sense::hover());
    if ui.is_rect_visible(rect) {
        let turns = ui.input(|i| i.time as f32) * 0.75;
        Icon::RefreshCw.paint_rotated(ui.painter(), rect, color, turns);
        ui.ctx().request_repaint();
    }
    response
}

// ---------------------------------------------------------------------------
// 8.4 Checkbox, radio, toggle
// ---------------------------------------------------------------------------

/// 16 × 16 checkbox with the label in the hit target and optional helper text
/// on the following line, indented to align with the label.
pub fn checkbox(
    ui: &mut Ui,
    checked: &mut bool,
    label: &str,
    helper: Option<&str>,
    enabled: bool,
) -> Response {
    let t = theme::tokens(ui.ctx());
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().item_spacing.y = space::XS;
            let hit = ui.horizontal(|ui| {
                let g = galley(ui, label, Type::Body, t.text_primary);
                let w = 16.0 + space::M + g.size().x;
                let h = g.size().y.max(20.0);
                let (rect, response) = ui.allocate_exact_size(
                    Vec2::new(w, h),
                    if enabled { Sense::click() } else { Sense::hover() },
                );
                if response.clicked() && enabled {
                    *checked = !*checked;
                }
                if ui.is_rect_visible(rect) {
                    let box_rect = Rect::from_min_size(
                        Pos2::new(rect.left(), rect.center().y - 8.0),
                        Vec2::splat(16.0),
                    );
                    paint_check_box(ui, box_rect, *checked, enabled, false);
                    if response.has_focus() {
                        focus_ring(ui, box_rect, CornerRadius::same(4));
                    }
                    let fg = if enabled { t.text_primary } else { t.text_disabled };
                    let g = galley(ui, label, Type::Body, fg);
                    ui.painter().galley(
                        Pos2::new(rect.left() + 16.0 + space::M, rect.center().y - g.size().y / 2.0),
                        g,
                        fg,
                    );
                }
                let checked_now = *checked;
                response.widget_info(|| {
                    WidgetInfo::selected(WidgetType::Checkbox, enabled, checked_now, label)
                });
                response
            });
            if let Some(h) = helper {
                ui.horizontal(|ui| {
                    ui.add_space(16.0 + space::M);
                    paragraph_at(
                        ui,
                        h,
                        Type::Small,
                        t.text_muted,
                        (ui.available_width() - 8.0).max(120.0),
                    );
                });
            }
            hit.inner
        })
        .inner;
    response
}

/// A tri-state box: `Some(true)`, `Some(false)`, or `None` for "some of the
/// things below". Used by the restore browser's select-all.
pub fn tri_checkbox(ui: &mut Ui, state: Option<bool>, label: &str, enabled: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(16.0), if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        paint_check_box(ui, rect, state.unwrap_or(false), enabled, state.is_none());
        if response.has_focus() {
            focus_ring(ui, rect, CornerRadius::same(4));
        }
    }
    let announced = match state {
        Some(true) => format!("{label}, all selected"),
        Some(false) => format!("{label}, none selected"),
        None => format!("{label}, some selected"),
    };
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::Checkbox, enabled, state.unwrap_or(false), &announced)
    });
    response.on_hover_text(label)
}

fn paint_check_box(ui: &Ui, rect: Rect, checked: bool, enabled: bool, partial: bool) {
    let t = theme::tokens(ui.ctx());
    let cr = CornerRadius::same(4);
    if checked || partial {
        let fill = if enabled { t.accent_fill } else { t.bg_raised };
        ui.painter().rect(rect, cr, fill, Stroke::new(1.0_f32, if enabled { t.accent_fill_border } else { t.border_subtle }), StrokeKind::Inside);
        let ink = if enabled { Color32::WHITE } else { t.text_disabled };
        if partial {
            // A filled square, not a tick: "everything inside" (R-3).
            ui.painter().rect_filled(rect.shrink(4.0), CornerRadius::same(1), ink);
        } else {
            Icon::Check.paint(ui.painter(), rect.shrink(2.0), ink);
        }
    } else {
        let fill = if enabled { t.bg_input } else { t.bg_raised };
        let border = if enabled { t.border_control } else { t.border_subtle };
        ui.painter().rect(rect, cr, fill, Stroke::new(1.0_f32, border), StrokeKind::Inside);
    }
}

/// A radio option with its own helper line. The whole row is the hit target.
pub fn radio(
    ui: &mut Ui,
    selected: bool,
    label: &str,
    helper: Option<&str>,
    enabled: bool,
) -> Response {
    let t = theme::tokens(ui.ctx());
    let inner = ui
        .scope(|ui| {
            ui.spacing_mut().item_spacing.y = space::XS;
            let response = ui
                .horizontal(|ui| {
                    let g = galley(ui, label, Type::Body, t.text_primary);
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(16.0 + space::M + g.size().x, g.size().y.max(20.0)),
                        if enabled { Sense::click() } else { Sense::hover() },
                    );
                    if ui.is_rect_visible(rect) {
                        let c = Pos2::new(rect.left() + 8.0, rect.center().y);
                        if selected {
                            ui.painter().circle(
                                c,
                                8.0,
                                if enabled { t.bg_input } else { t.bg_raised },
                                Stroke::new(1.0_f32, if enabled { t.accent } else { t.border_subtle }),
                            );
                            ui.painter().circle_filled(
                                c,
                                4.0,
                                if enabled { t.accent } else { t.text_disabled },
                            );
                        } else {
                            ui.painter().circle(
                                c,
                                8.0,
                                if enabled { t.bg_input } else { t.bg_raised },
                                Stroke::new(1.0_f32, if enabled { t.border_control } else { t.border_subtle }),
                            );
                        }
                        if response.has_focus() {
                            focus_ring(
                                ui,
                                Rect::from_center_size(c, Vec2::splat(16.0)),
                                CornerRadius::same(8),
                            );
                        }
                        let fg = if enabled { t.text_primary } else { t.text_disabled };
                        let g = galley(ui, label, Type::Body, fg);
                        ui.painter().galley(
                            Pos2::new(rect.left() + 16.0 + space::M, rect.center().y - g.size().y / 2.0),
                            g,
                            fg,
                        );
                    }
                    let announce = match helper {
                        Some(h) => format!("{label}. {h}"),
                        None => label.to_string(),
                    };
                    response.widget_info(|| {
                        WidgetInfo::selected(WidgetType::RadioButton, enabled, selected, &announce)
                    });
                    response
                })
                .inner;
            if let Some(h) = helper {
                ui.horizontal(|ui| {
                    ui.add_space(16.0 + space::M);
                    paragraph_at(
                        ui,
                        h,
                        Type::Small,
                        t.text_muted,
                        (ui.available_width() - 8.0).max(120.0),
                    );
                });
            }
            response
        })
        .inner;
    inner
}

/// 36 × 20 toggle. Toggles are for settings that take effect immediately;
/// checkboxes are for forms with a Save action (§8.4).
pub fn toggle(
    ui: &mut Ui,
    on: &mut bool,
    label: &str,
    helper: Option<&str>,
    enabled: bool,
) -> Response {
    let t = theme::tokens(ui.ctx());
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = space::XS;
        let response = ui
            .horizontal(|ui| {
                let g = galley(ui, label, Type::Body, t.text_primary);
                let (rect, response) = ui.allocate_exact_size(
                    Vec2::new(36.0 + space::M + g.size().x, 20.0_f32.max(g.size().y)),
                    if enabled { Sense::click() } else { Sense::hover() },
                );
                if response.clicked() && enabled {
                    *on = !*on;
                }
                if ui.is_rect_visible(rect) {
                    let track = Rect::from_min_size(
                        Pos2::new(rect.left(), rect.center().y - 10.0),
                        Vec2::new(36.0, 20.0),
                    );
                    let cr = CornerRadius::same(10);
                    if *on && enabled {
                        ui.painter().rect_filled(track, cr, t.accent_fill);
                        ui.painter().circle_filled(
                            Pos2::new(track.right() - 10.0, track.center().y),
                            8.0,
                            Color32::WHITE,
                        );
                    } else {
                        let fill = if enabled { t.bg_raised } else { theme::alpha(t.bg_raised, 0.5) };
                        ui.painter().rect(
                            track,
                            cr,
                            fill,
                            Stroke::new(1.0_f32, if enabled { t.border_control } else { t.border_subtle }),
                            StrokeKind::Inside,
                        );
                        let knob_x = if *on { track.right() - 10.0 } else { track.left() + 10.0 };
                        ui.painter().circle_filled(
                            Pos2::new(knob_x, track.center().y),
                            8.0,
                            if enabled { t.text_secondary } else { t.text_disabled },
                        );
                    }
                    if response.has_focus() {
                        focus_ring(ui, track, cr);
                    }
                    let fg = if enabled { t.text_primary } else { t.text_disabled };
                    let g = galley(ui, label, Type::Body, fg);
                    ui.painter().galley(
                        Pos2::new(rect.left() + 36.0 + space::M, rect.center().y - g.size().y / 2.0),
                        g,
                        fg,
                    );
                }
                let is_on = *on;
                response
                    .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, is_on, label));
                response
            })
            .inner;
        if let Some(h) = helper {
            ui.horizontal(|ui| {
                ui.add_space(36.0 + space::M);
                paragraph_at(
                    ui,
                    h,
                    Type::Small,
                    t.text_muted,
                    (ui.available_width() - 8.0).max(120.0),
                );
            });
        }
        response
    })
    .inner
}

// ---------------------------------------------------------------------------
// 8.5 Segmented control
// ---------------------------------------------------------------------------

/// Height 30, one selected segment on `bg.surface`, ←/→ to move. Used for the
/// job editor tabs and every two-or-three-way choice.
pub fn segmented(ui: &mut Ui, selected: &mut usize, labels: &[&str]) -> Response {
    segmented_marked(ui, selected, labels, &[])
}

/// The same, with a 6px accent dot after the labels whose index is in `marked`
/// — the job editor's "this tab has unsaved changes" affordance.
pub fn segmented_marked(
    ui: &mut Ui,
    selected: &mut usize,
    labels: &[&str],
    marked: &[usize],
) -> Response {
    let t = theme::tokens(ui.ctx());
    let pad = 12.0;
    let widths: Vec<f32> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let g = galley(ui, *l, Type::SmallStrong, t.text_primary);
            g.size().x + pad * 2.0 + if marked.contains(&i) { 12.0 } else { 0.0 }
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(total, size::CONTROL_H), Sense::click());

    if ui.is_rect_visible(rect) {
        ui.painter().rect(
            rect,
            radius::CONTROL,
            t.bg_raised,
            Stroke::new(1.0_f32, t.border_subtle),
            StrokeKind::Inside,
        );
        let mut x = rect.left() + 3.0;
        for (i, label) in labels.iter().enumerate() {
            let seg = Rect::from_min_size(
                Pos2::new(x, rect.top() + 3.0),
                Vec2::new(widths[i], rect.height() - 6.0),
            );
            let is_selected = *selected == i;
            let seg_response = ui.interact(seg, response.id.with(i), Sense::click());
            if seg_response.clicked() {
                *selected = i;
            }
            if is_selected {
                ui.painter().rect(
                    seg,
                    CornerRadius::same(4),
                    t.bg_surface,
                    Stroke::new(1.0_f32, t.border_control),
                    StrokeKind::Inside,
                );
            } else if seg_response.hovered() {
                ui.painter().rect_filled(seg, CornerRadius::same(4), t.bg_surface_hover);
            }
            let fg = if is_selected { t.text_primary } else { t.text_secondary };
            let g = galley(ui, *label, Type::SmallStrong, fg);
            ui.painter()
                .galley(Pos2::new(seg.left() + pad, seg.center().y - g.size().y / 2.0), g, fg);
            if marked.contains(&i) {
                ui.painter()
                    .circle_filled(Pos2::new(seg.right() - pad + 2.0, seg.center().y), 3.0, t.accent);
            }
            let announce = if marked.contains(&i) {
                super::copy::a11y_dirty_tab(label)
            } else {
                (*label).to_string()
            };
            seg_response.widget_info(|| {
                WidgetInfo::selected(WidgetType::RadioButton, true, is_selected, &announce)
            });
            x += widths[i];
        }
        if response.has_focus() {
            focus_ring(ui, rect, radius::CONTROL);
            let n = labels.len();
            if n > 0 {
                ui.input(|i| {
                    if i.key_pressed(egui::Key::ArrowRight) {
                        *selected = (*selected + 1) % n;
                    }
                    if i.key_pressed(egui::Key::ArrowLeft) {
                        *selected = (*selected + n - 1) % n;
                    }
                    if i.key_pressed(egui::Key::Home) {
                        *selected = 0;
                    }
                    if i.key_pressed(egui::Key::End) {
                        *selected = n - 1;
                    }
                });
            }
        }
    }
    response
}

// ---------------------------------------------------------------------------
// 8.2 Text input
// ---------------------------------------------------------------------------

/// A labelled field: `h3` label, the input, then helper text or, when set, the
/// error message with its `alert-triangle`. The error replaces the helper so
/// the layout never jumps.
pub struct Field<'a> {
    label: Option<&'a str>,
    helper: Option<&'a str>,
    error: Option<&'a str>,
    width: f32,
    mono: bool,
    password: bool,
    multiline_rows: Option<usize>,
    placeholder: Option<&'a str>,
    unit: Option<&'a str>,
    enabled: bool,
    char_limit: Option<usize>,
}

impl<'a> Field<'a> {
    pub fn new() -> Self {
        Field {
            label: None,
            helper: None,
            error: None,
            width: 400.0,
            mono: false,
            password: false,
            multiline_rows: None,
            placeholder: None,
            unit: None,
            enabled: true,
            char_limit: None,
        }
    }
    pub fn label(mut self, l: &'a str) -> Self {
        self.label = Some(l);
        self
    }
    pub fn helper(mut self, h: &'a str) -> Self {
        self.helper = Some(h);
        self
    }
    pub fn error(mut self, e: Option<&'a str>) -> Self {
        self.error = e;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
    pub fn password(mut self) -> Self {
        self.password = true;
        self
    }
    pub fn rows(mut self, rows: usize) -> Self {
        self.multiline_rows = Some(rows);
        self
    }
    pub fn placeholder(mut self, p: &'a str) -> Self {
        self.placeholder = Some(p);
        self
    }
    pub fn unit(mut self, u: &'a str) -> Self {
        self.unit = Some(u);
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn char_limit(mut self, n: usize) -> Self {
        self.char_limit = Some(n);
        self
    }

    pub fn show(self, ui: &mut Ui, value: &mut String) -> Response {
        let t = theme::tokens(ui.ctx());
        let mut response = None;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = space::S;
            if let Some(l) = self.label {
                text(ui, l, Type::H3, t.text_primary);
            }

            let font: FontId = if self.mono { Type::Mono.font() } else { Type::Body.font() };
            let mut edit = egui::TextEdit::singleline(value)
                .desired_width(self.width)
                .font(font.clone())
                .text_color(if self.enabled { t.text_primary } else { t.text_disabled })
                .margin(egui::Margin::symmetric(10, 6))
                .background_color(if self.enabled {
                    t.bg_input
                } else {
                    theme::alpha(t.bg_raised, 0.5)
                });
            if self.password {
                edit = edit.password(true);
            }
            if let Some(p) = self.placeholder {
                edit = edit.hint_text(egui::RichText::new(p).color(t.text_muted).font(font.clone()));
            }
            if let Some(n) = self.char_limit {
                edit = edit.char_limit(n);
            }

            let r = if let Some(rows) = self.multiline_rows {
                let mut multi = egui::TextEdit::multiline(value)
                    .desired_width(self.width)
                    .desired_rows(rows)
                    .font(font.clone())
                    .text_color(if self.enabled { t.text_primary } else { t.text_disabled })
                    .margin(egui::Margin::symmetric(10, 6))
                    .background_color(if self.enabled {
                        t.bg_input
                    } else {
                        theme::alpha(t.bg_raised, 0.5)
                    });
                if let Some(p) = self.placeholder {
                    multi = multi.hint_text(
                        egui::RichText::new(p).color(t.text_muted).font(font.clone()),
                    );
                }
                ui.add_enabled(self.enabled, multi)
            } else {
                ui.scope(|ui| {
                    ui.set_min_height(size::CONTROL_H);
                    ui.add_enabled(self.enabled, edit)
                })
                .inner
            };

            // The border: 2px danger on error, 2px focus inset when focused —
            // inputs are the one place the ring is inside (§8.2).
            let stroke = if self.error.is_some() {
                Stroke::new(2.0_f32, t.danger.mark)
            } else if r.has_focus() {
                Stroke::new(2.0_f32, t.border_focus)
            } else if !self.enabled {
                Stroke::new(1.0_f32, t.border_subtle)
            } else {
                Stroke::new(1.0_f32, t.border_control)
            };
            ui.painter()
                .rect_stroke(r.rect, radius::CONTROL, stroke, StrokeKind::Inside);

            if let Some(u) = self.unit {
                let g = galley(ui, u, Type::Small, t.text_muted);
                ui.painter().galley(
                    Pos2::new(r.rect.right() - 10.0 - g.size().x, r.rect.center().y - g.size().y / 2.0),
                    g,
                    t.text_muted,
                );
            }

            if let Some(e) = self.error {
                ui.horizontal(|ui| {
                    let (ir, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    Icon::AlertTriangle.paint(ui.painter(), ir, t.danger.mark);
                    ui.add_space(space::S);
                    paragraph_at(ui, e, Type::Small, t.danger.tint_text, self.width - 22.0);
                });
            } else if let Some(h) = self.helper {
                paragraph_at(ui, h, Type::Small, t.text_muted, self.width);
            }

            if let Some(limit) = self.char_limit {
                let used = value.chars().count();
                if used > limit * 3 / 4 {
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        ui.add_space((ui.available_width() - self.width).max(0.0));
                        text(ui, format!("{used}/{limit}"), Type::Small, t.text_muted);
                    });
                }
            }

            // The error is announced on the field itself, so a screen reader
            // hears it while focused (§8.2).
            let label = self.label.unwrap_or("");
            let announce = match self.error {
                Some(e) => format!("{label}. {e}"),
                None => label.to_string(),
            };
            r.widget_info(|| {
                let mut info = WidgetInfo::text_edit(self.enabled, "", value.as_str());
                info.label = Some(announce.clone());
                info
            });
            response = Some(r);
        });
        response.expect("scope always runs")
    }
}

impl Default for Field<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// A passphrase field with the reveal toggle inside its right edge. Revealing
/// is never persistent: the caller re-masks on blur and on window focus loss.
pub fn passphrase_field(
    ui: &mut Ui,
    value: &mut String,
    label: &str,
    revealed: &mut bool,
    error: Option<&str>,
    width: f32,
) -> Response {
    let mut response = None;
    ui.horizontal(|ui| {
        let mut field = Field::new().label(label).width(width - 34.0).error(error);
        if !*revealed {
            field = field.password();
        }
        let r = field.show(ui, value);
        response = Some(r);
        ui.add_space(space::XS);
        let icon = if *revealed { Icon::EyeOff } else { Icon::Eye };
        let label = if *revealed { "Hide the passphrase" } else { "Show the passphrase" };
        if icon_button(ui, icon, label, true).clicked() {
            *revealed = !*revealed;
        }
    });
    response.expect("the horizontal layout always runs")
}

/// A numeric field. egui has no spinner and no input mask (L11), so this is a
/// `DragValue` with the unit drawn beside it and the range clamped.
pub fn number(
    ui: &mut Ui,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
    unit: &str,
    enabled: bool,
    label: &str,
) -> Response {
    let t = theme::tokens(ui.ctx());
    let mut response = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        let r = ui.add_enabled(
            enabled,
            egui::DragValue::new(value).range(range).speed(1.0).clamp_existing_to_range(true),
        );
        ui.painter().rect_stroke(
            r.rect,
            radius::CONTROL,
            Stroke::new(1.0_f32, if enabled { t.border_control } else { t.border_subtle }),
            StrokeKind::Inside,
        );
        if !unit.is_empty() {
            text(ui, unit, Type::Small, t.text_muted);
        }
        let v = *value;
        r.widget_info(|| {
            let mut info =
                WidgetInfo::labeled(WidgetType::DragValue, enabled, format!("{label}, {v} {unit}"));
            info.value = Some(v as f64);
            info
        });
        response = Some(r);
    });
    response.expect("the horizontal layout always runs")
}

// ---------------------------------------------------------------------------
// 8.3 Combo box
// ---------------------------------------------------------------------------

/// A combo over a list of labels. Returns true when the selection changed.
pub fn combo(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    selected: &mut usize,
    options: &[String],
    width: f32,
    enabled: bool,
) -> bool {
    let mut changed = false;
    let current = options.get(*selected).cloned().unwrap_or_default();
    ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(current)
            .width(width)
            .height(320.0)
            .show_ui(ui, |ui| {
                for (i, option) in options.iter().enumerate() {
                    if ui.selectable_label(*selected == i, option).clicked() {
                        *selected = i;
                        changed = true;
                    }
                }
            });
    });
    changed
}

// ---------------------------------------------------------------------------
// 8.13 Empty state
// ---------------------------------------------------------------------------

pub struct EmptyAction {
    pub primary: bool,
    pub clicked: bool,
}

/// Centred at 45% of the container height, max 420px wide. An empty state is a
/// state, not a placeholder: it explains the thing before offering the action.
pub fn empty_state(
    ui: &mut Ui,
    icon: Icon,
    empty: &super::copy::Empty,
    body_override: Option<&str>,
) -> (bool, bool) {
    let t = theme::tokens(ui.ctx());
    let mut primary_clicked = false;
    let mut secondary_clicked = false;
    let available = ui.available_size();
    let top = (available.y * 0.45 - 90.0).max(0.0);
    ui.allocate_ui_with_layout(available, Layout::top_down(Align::Center), |ui| {
        ui.add_space(top);
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
        icon.paint(ui.painter(), ir, t.text_muted);
        ui.add_space(space::XL);
        text(ui, empty.title, Type::H2, t.text_primary);
        ui.add_space(space::M);
        ui.allocate_ui_with_layout(
            Vec2::new(420.0, 0.0),
            Layout::top_down(Align::Center),
            |ui| {
                let body = body_override.unwrap_or(empty.body);
                let g = galley_wrapped(ui, body, Type::Body, t.text_secondary, 420.0);
                let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
                ui.painter().galley(rect.min, g, t.text_secondary);
            },
        );
        if empty.primary.is_some() || empty.secondary.is_some() {
            ui.add_space(space::XXL);
            ui.horizontal(|ui| {
                // Centre the button row inside the available width.
                let mut w = 0.0;
                if let Some(p) = empty.primary {
                    w += galley(ui, p, Type::BodyStrong, t.text_primary).size().x + 24.0;
                }
                if let Some(s) = empty.secondary {
                    w += galley(ui, s, Type::BodyStrong, t.text_primary).size().x + 24.0 + space::M;
                }
                ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                if let Some(p) = empty.primary {
                    primary_clicked = Button::primary(p).show(ui).clicked();
                }
                if let Some(s) = empty.secondary {
                    secondary_clicked = Button::ghost(s).show(ui).clicked();
                }
            });
        }
    });
    (primary_clicked, secondary_clicked)
}

// ---------------------------------------------------------------------------
// 8.16 Key/value list
// ---------------------------------------------------------------------------

/// A 160px label column and a remainder value column, 28px rows, no dividers.
pub fn kv(ui: &mut Ui, label: &str, value: &str, mono: bool) -> Response {
    kv_with(ui, label, |ui| {
        let t = theme::tokens(ui.ctx());
        let ty = if mono { Type::MonoSmall } else { Type::BodyStrong };
        let width = ui.available_width().max(40.0);
        elided(ui, value, ty, t.text_primary, width, false)
    })
}

pub fn kv_with<R>(ui: &mut Ui, label: &str, value: impl FnOnce(&mut Ui) -> R) -> R {
    let t = theme::tokens(ui.ctx());
    let mut out = None;
    ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        ui.spacing_mut().item_spacing.x = space::XL;
        ui.allocate_ui_with_layout(
            Vec2::new(size::KV_LABEL_W, 20.0),
            Layout::right_to_left(Align::Center),
            |ui| {
                text(ui, label, Type::Body, t.text_secondary);
            },
        );
        out = Some(value(ui));
    });
    out.expect("the horizontal layout always runs")
}

// ---------------------------------------------------------------------------
// 8.17 Passphrase strength meter
// ---------------------------------------------------------------------------

/// Four segments and a word. The word is part of the announcement, so strength
/// is never carried by colour alone.
pub fn strength_meter(ui: &mut Ui, score: u8, width: f32) -> Response {
    let t = theme::tokens(ui.ctx());
    let (colour, label) = match score {
        0 | 1 => (t.danger.mark, super::copy::strength::TOO_WEAK),
        2 => (t.warning.mark, super::copy::strength::WEAK),
        3 => (t.info.mark, super::copy::strength::GOOD),
        _ => (t.success.mark, super::copy::strength::STRONG),
    };
    let lit = match score {
        0 | 1 => 1,
        2 => 2,
        3 => 3,
        _ => 4,
    };
    let mut response = None;
    ui.horizontal(|ui| {
        let g = galley(ui, label, Type::SmallStrong, colour);
        let meter_w = (width - g.size().x - space::L).max(40.0);
        let (rect, r) = ui.allocate_exact_size(Vec2::new(meter_w, 6.0), Sense::hover());
        if ui.is_rect_visible(rect) {
            let seg_w = (meter_w - 3.0 * 2.0) / 4.0;
            for i in 0..4 {
                let x = rect.left() + i as f32 * (seg_w + 2.0);
                let seg = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(seg_w, 6.0));
                let fill = if i < lit { colour } else { t.progress_track };
                ui.painter().rect_filled(seg, CornerRadius::same(3), fill);
            }
        }
        ui.add_space(space::L);
        let label_size = g.size();
        ui.painter().galley(
            Pos2::new(rect.right() + space::L, rect.center().y - label_size.y / 2.0),
            g,
            colour,
        );
        ui.advance_cursor_after_rect(Rect::from_min_size(
            Pos2::new(rect.right() + space::L, rect.top()),
            label_size,
        ));
        let announce = super::copy::a11y_strength(label);
        r.widget_info(|| {
            let mut info = WidgetInfo::labeled(WidgetType::ProgressIndicator, true, &announce);
            info.value = Some(score as f64 / 4.0);
            info
        });
        response = Some(r);
    });
    response.expect("the horizontal layout always runs")
}

// ---------------------------------------------------------------------------
// 8.18 Code / log block
// ---------------------------------------------------------------------------

/// `bg.code`, monospace, no wrapping, a floating copy button, and a severity
/// bar rather than tinted text.
pub fn code_block(ui: &mut Ui, content: &str, max_height: f32, severity: Option<Status>) -> bool {
    let t = theme::tokens(ui.ctx());
    let mut copied = false;
    egui::Frame::new()
        .fill(t.bg_code)
        .stroke(Stroke::new(1.0_f32, t.border_subtle))
        .corner_radius(radius::CONTROL)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                if let Some(s) = severity {
                    let (bar, _) = ui.allocate_exact_size(
                        Vec2::new(3.0, max_height.min(content.lines().count() as f32 * 16.0 + 4.0)),
                        Sense::hover(),
                    );
                    ui.painter().rect_filled(bar, CornerRadius::same(1), s.mark);
                    ui.add_space(space::M);
                }
                egui::ScrollArea::both()
                    .max_height(max_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
                        for line in content.lines() {
                            text(ui, line, Type::MonoSmall, t.text_primary);
                        }
                    });
                ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                    if icon_button_compact(ui, Icon::Copy, super::copy::action::COPY, true).clicked()
                    {
                        ui.ctx().copy_text(content.to_owned());
                        copied = true;
                    }
                });
            });
        });
    copied
}

// ---------------------------------------------------------------------------
// Table helpers (8.7)
// ---------------------------------------------------------------------------

/// A `micro` column header in `text.muted`, with a sort chevron when sorted.
pub fn table_header(ui: &mut Ui, label: &str, sorted: Option<bool>) -> Response {
    let t = theme::tokens(ui.ctx());
    let colour = if sorted.is_some() { t.text_primary } else { t.text_muted };
    let mut response = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::XS;
        let r = text(ui, label, Type::Micro, colour);
        if let Some(descending) = sorted {
            let (ir, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
            let icon = if descending { Icon::ChevronDown } else { Icon::ChevronUp };
            icon.paint(ui.painter(), ir, t.text_secondary);
        }
        response = Some(r);
    });
    response.expect("the horizontal layout always runs")
}

/// A right-aligned numeric cell in `mono.small` (L9).
pub fn numeric_cell(ui: &mut Ui, value: &str) {
    let t = theme::tokens(ui.ctx());
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        text(ui, value, Type::MonoSmall, t.text_primary);
    });
}

pub fn muted_cell(ui: &mut Ui, value: &str) {
    let t = theme::tokens(ui.ctx());
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let w = ui.available_width();
        elided(ui, value, Type::Small, t.text_muted, w, false);
    });
}

/// The 1px row divider, inset 12px from both ends. No zebra striping.
pub fn row_divider(ui: &Ui, rect: Rect) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(
        Rect::from_min_max(
            Pos2::new(rect.left() + 12.0, rect.bottom() - 1.0),
            Pos2::new(rect.right() - 12.0, rect.bottom()),
        ),
        0,
        t.border_subtle,
    );
}

/// Paint a table row's background for hover, selection and keyboard focus.
pub fn row_background(ui: &Ui, rect: Rect, hovered: bool, selected: bool, focused: bool) {
    let t = theme::tokens(ui.ctx());
    if selected {
        ui.painter().rect_filled(rect, 0, t.bg_selected);
    } else if hovered {
        ui.painter().rect_filled(rect, 0, t.bg_surface_hover);
    }
    if focused {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            CornerRadius::ZERO,
            Stroke::new(2.0_f32, t.border_focus),
            StrokeKind::Inside,
        );
    }
}

/// A container that gives a table the card treatment the design system asks
/// for: `bg.surface`, 1px `border.subtle`, radius 10, clipped.
pub fn table_frame<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let t = theme::tokens(ui.ctx());
    egui::Frame::new()
        .fill(t.bg_surface)
        .stroke(Stroke::new(1.0_f32, t.border_subtle))
        .corner_radius(radius::CARD)
        .inner_margin(egui::Margin::ZERO)
        .show(ui, add)
}

// ---------------------------------------------------------------------------
// Checklist rows (repository creation, provider test, rotation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    Done,
    Failed,
}

/// A 20px checklist row whose icon moves `circle` → spinner → `check-circle-2`.
pub fn checklist_row(ui: &mut Ui, state: StepState, label: &str, detail: Option<&str>) {
    let t = theme::tokens(ui.ctx());
    ui.horizontal(|ui| {
        ui.set_min_height(20.0);
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        match state {
            StepState::Pending => Icon::Circle.paint(ui.painter(), ir, t.text_muted),
            StepState::Running => {
                let turns = ui.input(|i| i.time as f32) * 0.75;
                Icon::RefreshCw.paint_rotated(ui.painter(), ir, t.info.mark, turns);
                ui.ctx().request_repaint();
            }
            StepState::Done => Icon::CheckCircle.paint(ui.painter(), ir, t.success.mark),
            StepState::Failed => Icon::XOctagon.paint(ui.painter(), ir, t.danger.mark),
        }
        ui.add_space(space::M);
        let colour = match state {
            StepState::Pending => t.text_muted,
            StepState::Failed => t.danger.tint_text,
            _ => t.text_primary,
        };
        text(ui, label, Type::Body, colour);
        if let Some(d) = detail {
            ui.add_space(space::M);
            text(ui, d, Type::Small, t.text_muted);
        }
    });
}

// ---------------------------------------------------------------------------
// 8.12 Modal
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalSize {
    Small,
    Medium,
    Large,
}

impl ModalSize {
    pub fn width(self) -> f32 {
        match self {
            ModalSize::Small => size::MODAL_SMALL,
            ModalSize::Medium => size::MODAL_MEDIUM,
            ModalSize::Large => size::MODAL_LARGE,
        }
    }
}

/// The modal shell: header, scrolling body, footer. Never nested (L13); a flow
/// that needs two decisions is a multi-step modal with its own step state.
///
/// The body and the footer are rendered through [`ModalShell`] rather than
/// through two closures, so both can borrow the same draft state — they run
/// one after the other, not at the same time.
pub struct ModalShell<'a> {
    ui: &'a mut Ui,
    width: f32,
    max_height: f32,
    footer_drawn: bool,
}

impl ModalShell<'_> {
    /// The scrolling body, 20px padding.
    pub fn body<R>(&mut self, add: impl FnOnce(&mut Ui) -> R) -> R {
        let width = self.width;
        let max_height = self.max_height;
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(20, 20))
            .show(self.ui, |ui| {
                ui.set_width(width - 40.0);
                egui::ScrollArea::vertical()
                    .max_height(max_height - 116.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(width - 40.0);
                        add(ui)
                    })
                    .inner
            })
            .inner
    }

    /// The 60px footer: a top rule, then buttons right-aligned, primary last.
    pub fn footer(&mut self, add: impl FnOnce(&mut Ui)) {
        let t = theme::tokens(self.ui.ctx());
        let width = self.width;
        let (line, _) = self.ui.allocate_exact_size(Vec2::new(width, 1.0), Sense::hover());
        self.ui.painter().rect_filled(line, 0, t.border_subtle);
        self.ui.allocate_ui_with_layout(
            Vec2::new(width, 60.0),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.add_space(space::XXL);
                ui.spacing_mut().item_spacing.x = space::M;
                add(ui);
            },
        );
        self.footer_drawn = true;
    }
}

/// Show a modal. Returns `true` when the user asked to close it (the `x`,
/// Escape, or a click outside), which a blocking modal ignores.
pub fn modal<R>(
    ctx: &egui::Context,
    id: &str,
    modal_size: ModalSize,
    title: &str,
    icon: Option<(Icon, Color32)>,
    blocking: bool,
    content: impl FnOnce(&mut ModalShell<'_>) -> R,
) -> (bool, R) {
    let t = theme::tokens(ctx);
    let screen = ctx.screen_rect();
    let width = modal_size.width().min(screen.width() - 80.0);
    let max_height = screen.height() - 96.0;

    let mut close = false;
    let response = egui::Modal::new(Id::new(id))
        .backdrop_color(t.bg_scrim)
        .frame(
            egui::Frame::new()
                .fill(t.bg_surface)
                .stroke(Stroke::new(1.0_f32, t.border_strong))
                .corner_radius(radius::MODAL)
                .inner_margin(egui::Margin::ZERO)
                .shadow(egui::epaint::Shadow {
                    offset: [0, 10],
                    blur: 40,
                    spread: 0,
                    color: if t.dark {
                        Color32::from_black_alpha(140)
                    } else {
                        Color32::from_rgba_unmultiplied(0x17, 0x1B, 0x21, 51)
                    },
                }),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.set_max_height(max_height);

            // Header, 56px.
            ui.allocate_ui_with_layout(
                Vec2::new(width, 56.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.add_space(space::XXL);
                    if let Some((ic, colour)) = icon {
                        let (ir, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                        ic.paint(ui.painter(), ir, colour);
                        ui.add_space(space::L);
                    }
                    text(ui, title, Type::H2, t.text_primary);
                    if !blocking {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(space::L);
                            if icon_button(ui, Icon::X, super::copy::action::CLOSE, true).clicked() {
                                close = true;
                            }
                        });
                    }
                },
            );

            let mut shell = ModalShell { ui, width, max_height, footer_drawn: false };
            content(&mut shell)
        });

    if !blocking {
        if response.should_close() {
            close = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
    }
    (close, response.inner)
}

// ---------------------------------------------------------------------------
// Inline unlock prompt (UX_SPEC §3.4)
// ---------------------------------------------------------------------------

/// A 44px row that stands in for credential fields while the vault is locked.
/// Returns true when the user asked to unlock.
pub fn inline_unlock(ui: &mut Ui) -> bool {
    let t = theme::tokens(ui.ctx());
    let mut clicked = false;
    egui::Frame::new()
        .fill(t.bg_raised)
        .stroke(Stroke::new(1.0_f32, t.border_control))
        .corner_radius(radius::CONTROL)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_height(28.0);
            ui.horizontal(|ui| {
                let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                Icon::Lock.paint(ui.painter(), ir, t.warning.mark);
                ui.add_space(space::M);
                text(ui, super::copy::locked::INLINE_PROMPT, Type::Small, t.text_primary);
                ui.add_space(space::L);
                clicked = Button::secondary(super::copy::action::UNLOCK).compact().show(ui).clicked();
            });
        });
    clicked
}

// ---------------------------------------------------------------------------
// Small shared pieces
// ---------------------------------------------------------------------------

/// A 6px status dot with an accessible label, used on rail items and in the
/// activity destination column. Never the only carrier of meaning.
pub fn status_dot(ui: &mut Ui, colour: Color32, label: &str, diameter: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().circle_filled(rect.center(), diameter / 2.0, colour);
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, label));
    response.on_hover_text(label)
}

/// An inline text link.
pub fn link(ui: &mut Ui, label: &str) -> Response {
    let t = theme::tokens(ui.ctx());
    let g = galley(ui, label, Type::Small, t.text_link);
    let (rect, response) = ui.allocate_exact_size(g.size(), Sense::click());
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.min, g, t.text_link);
        if response.hovered() {
            ui.painter().line_segment(
                [
                    Pos2::new(rect.left(), rect.bottom()),
                    Pos2::new(rect.right(), rect.bottom()),
                ],
                Stroke::new(1.0_f32, t.text_link),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.has_focus() {
            focus_ring(ui, rect, CornerRadius::same(2));
        }
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Link, true, label));
    response
}

/// The `⋯` overflow menu button, with its items supplied by the caller.
pub fn overflow_menu<R>(
    ui: &mut Ui,
    _id: impl std::hash::Hash,
    a11y: &str,
    items: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let t = theme::tokens(ui.ctx());
    let mut out = None;
    let response = egui::menu::menu_custom_button(
        ui,
        egui::Button::new("")
            .min_size(Vec2::splat(size::CONTROL_H_COMPACT))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE),
        |ui| {
            ui.set_min_width(190.0);
            out = Some(items(ui));
        },
    );
    let rect = response.response.rect;
    if ui.is_rect_visible(rect) {
        let fg = if response.response.hovered() { t.text_primary } else { t.text_secondary };
        Icon::MoreHorizontal.paint(
            ui.painter(),
            Rect::from_center_size(rect.center(), Vec2::splat(16.0)),
            fg,
        );
    }
    response
        .response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, a11y));
    out
}

/// A menu entry, styled like the rest of the system rather than like egui's
/// default button.
pub fn menu_item(ui: &mut Ui, label: &str, enabled: bool) -> bool {
    let t = theme::tokens(ui.ctx());
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).color(if enabled {
            t.text_primary
        } else {
            t.text_disabled
        }))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(ui.available_width(), 28.0)),
    );
    if response.clicked() {
        ui.close_menu();
        return true;
    }
    false
}

pub fn menu_item_danger(ui: &mut Ui, label: &str, enabled: bool) -> bool {
    let t = theme::tokens(ui.ctx());
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).color(if enabled {
            t.danger.mark
        } else {
            t.text_disabled
        }))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(ui.available_width(), 28.0)),
    );
    if response.clicked() {
        ui.close_menu();
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_sizes_match_the_design_system() {
        assert_eq!(ButtonSize::Normal.height(), 30.0);
        assert_eq!(ButtonSize::Compact.height(), 26.0);
        assert_eq!(ButtonSize::Onboarding.height(), 36.0);
    }

    #[test]
    fn modal_widths_are_the_three_size_classes() {
        assert_eq!(ModalSize::Small.width(), 420.0);
        assert_eq!(ModalSize::Medium.width(), 560.0);
        assert_eq!(ModalSize::Large.width(), 760.0);
    }

    #[test]
    fn a_blocked_button_carries_its_reason() {
        let b = Button::primary("Run now").blocked_when(true, "Unlock the vault to use this.");
        assert!(!b.enabled);
        assert_eq!(b.disabled_reason, Some("Unlock the vault to use this."));
        let b = Button::primary("Run now").blocked_when(false, "Unlock the vault to use this.");
        assert!(b.enabled);
    }
}
