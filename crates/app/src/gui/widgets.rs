//! The eighteen components of `DESIGN_SYSTEM.md` §8, and nothing else.
//!
//! Every component reads its colours from [`crate::gui::theme::Tokens`], draws
//! its own focus ring 2px outside the control, and supplies a `WidgetInfo` so
//! that custom painters are not invisible to AccessKit (L7). A screen that
//! wants a control not in this file is a screen that has drifted from the
//! design system.

// The interface is a library-shaped tree inside a binary crate. Its components,
// view models and fixtures are also compiled by `crates/app/tests/gui_app.rs`
// as a separate crate, so items that are used and tested there look unused from
// the binary's side. The allow is scoped to this module rather than the crate.
#![allow(dead_code)]
use std::sync::Arc;

use egui::{
    Align, Color32, CornerRadius, FontId, Galley, Id, Layout, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, TextWrapMode, Ui, Vec2, WidgetInfo, WidgetType,
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

/// Wrapped text at a declared measure, capped at what the caller actually has.
///
/// The design system's 68-character measure is a maximum, not a minimum: a
/// paragraph asked to be 560px wide inside a 533px settings pane must wrap at
/// 533, not spill out of it.
pub fn paragraph_at(
    ui: &mut Ui,
    value: impl Into<String>,
    ty: Type,
    color: Color32,
    width: f32,
) -> Response {
    let width = width.min(ui.available_width().max(80.0));
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
    let width = width.min(ui.available_width().max(40.0));
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

/// Text that elides if it must, but allocates only the width it uses — so an
/// icon after a short name sits next to it rather than at the column edge.
pub fn text_capped(ui: &mut Ui, value: &str, ty: Type, color: Color32, max_width: f32) -> Response {
    let shown = elide_to_width(ui, value, ty, max_width.min(ui.available_width()), false);
    let g = galley(ui, shown.clone(), ty, color);
    let (rect, response) = ui.allocate_exact_size(g.size(), Sense::hover());
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
    let measure = |s: &str| {
        ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x)
    };
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

        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(w, h),
            if self.enabled { Sense::click() } else { Sense::hover() },
        );

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
            ui.painter().galley(Pos2::new(x, rect.center().y - g.size().y / 2.0), g, fg);
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
                Variant::Primary | Variant::Danger => {
                    (t.bg_raised, Stroke::new(1.0_f32, t.border_subtle), t.text_disabled)
                }
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
                let border =
                    if hovered { theme::alpha(t.border_focus, 0.6) } else { t.border_control };
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
                let fill = if hovered || pressed { t.danger.tint_bg } else { Color32::TRANSPARENT };
                let fg = if hovered || pressed { t.danger.tint_text } else { t.danger.mark };
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
    let (rect, response) = ui
        .allocate_exact_size(Vec2::splat(s), if enabled { Sense::click() } else { Sense::hover() });
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
            let ir = Rect::from_min_size(Pos2::new(x, rect.center().y - 7.0), Vec2::splat(14.0));
            if spin {
                let turns = ui.input(|i| i.time as f32) * 0.75;
                icon.paint_rotated(ui.painter(), ir, status.tint_text, turns);
            } else {
                icon.paint(ui.painter(), ir, status.tint_text);
            }
            x += 14.0 + space::S;
        }
        ui.painter().galley(Pos2::new(x, rect.center().y - g.size().y / 2.0), g, status.tint_text);
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
        let ir = Rect::from_min_size(
            Pos2::new(rect.left() + 8.0, rect.center().y - 7.0),
            Vec2::splat(14.0),
        );
        icon.paint(ui.painter(), ir, t.text_muted);
        ui.painter().galley(
            Pos2::new(rect.left() + 8.0 + 14.0 + space::S, rect.center().y - g.size().y / 2.0),
            g,
            t.text_secondary,
        );
        if let Some(s) = problem {
            ui.painter().circle_filled(
                Pos2::new(rect.right() - 3.0, rect.top() + 3.0),
                3.0,
                s.mark,
            );
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

/// A list row: the card treatment with the tighter padding a 56px row needs.
/// A preset list of twelve items must not need three screens.
pub fn row_card<R>(
    ui: &mut Ui,
    fill: Option<Color32>,
    add: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let t = theme::tokens(ui.ctx());
    egui::Frame::new()
        .fill(fill.unwrap_or(t.bg_surface))
        .stroke(Stroke::new(1.0_f32, t.border_subtle))
        .corner_radius(radius::CONTROL)
        .inner_margin(egui::Margin::symmetric(12, 9))
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
        WidgetInfo::labeled(WidgetType::Label, true, format!("{title}. {}", body.unwrap_or("")))
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
                let band_rect =
                    Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(band, height))
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
    if helper.is_none() {
        return checkbox_row(ui, checked, label, enabled);
    }
    let response = ui
        .vertical(|ui| {
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
                        Pos2::new(
                            rect.left() + 16.0 + space::M,
                            rect.center().y - g.size().y / 2.0,
                        ),
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

/// The single row of a checkbox, without the helper line beneath it.
fn checkbox_row(ui: &mut Ui, checked: &mut bool, label: &str, enabled: bool) -> Response {
    let t = theme::tokens(ui.ctx());
    let g = galley(ui, label, Type::Body, t.text_primary);
    let width = 16.0 + if label.is_empty() { 0.0 } else { space::M + g.size().x };
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(width, g.size().y.max(20.0)),
        if enabled { Sense::click() } else { Sense::hover() },
    );
    if response.clicked() && enabled {
        *checked = !*checked;
    }
    if ui.is_rect_visible(rect) {
        let box_rect =
            Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - 8.0), Vec2::splat(16.0));
        paint_check_box(ui, box_rect, *checked, enabled, false);
        if response.has_focus() {
            focus_ring(ui, box_rect, CornerRadius::same(4));
        }
        if !label.is_empty() {
            let fg = if enabled { t.text_primary } else { t.text_disabled };
            let g = galley(ui, label, Type::Body, fg);
            let h = g.size().y;
            ui.painter().galley(
                Pos2::new(rect.left() + 16.0 + space::M, rect.center().y - h / 2.0),
                g,
                fg,
            );
        }
    }
    let is_checked = *checked;
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, is_checked, label));
    response
}

/// A tri-state box: `Some(true)`, `Some(false)`, or `None` for "some of the
/// things below". Used by the restore browser's select-all.
pub fn tri_checkbox(ui: &mut Ui, state: Option<bool>, label: &str, enabled: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::splat(16.0),
        if enabled { Sense::click() } else { Sense::hover() },
    );
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
        ui.painter().rect(
            rect,
            cr,
            fill,
            Stroke::new(1.0_f32, if enabled { t.accent_fill_border } else { t.border_subtle }),
            StrokeKind::Inside,
        );
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
        .vertical(|ui| {
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
                                Stroke::new(
                                    1.0_f32,
                                    if enabled { t.accent } else { t.border_subtle },
                                ),
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
                                Stroke::new(
                                    1.0_f32,
                                    if enabled { t.border_control } else { t.border_subtle },
                                ),
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
                            Pos2::new(
                                rect.left() + 16.0 + space::M,
                                rect.center().y - g.size().y / 2.0,
                            ),
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
    if helper.is_none() {
        return toggle_row(ui, on, label, enabled);
    }
    ui.vertical(|ui| {
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
                        let fill =
                            if enabled { t.bg_raised } else { theme::alpha(t.bg_raised, 0.5) };
                        ui.painter().rect(
                            track,
                            cr,
                            fill,
                            Stroke::new(
                                1.0_f32,
                                if enabled { t.border_control } else { t.border_subtle },
                            ),
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
                        Pos2::new(
                            rect.left() + 36.0 + space::M,
                            rect.center().y - g.size().y / 2.0,
                        ),
                        g,
                        fg,
                    );
                }
                let is_on = *on;
                response.widget_info(|| {
                    WidgetInfo::selected(WidgetType::Checkbox, enabled, is_on, label)
                });
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

/// The single row of a toggle, without the helper line beneath it.
fn toggle_row(ui: &mut Ui, on: &mut bool, label: &str, enabled: bool) -> Response {
    let t = theme::tokens(ui.ctx());
    let g = galley(ui, label, Type::Body, t.text_primary);
    let width = 36.0 + if label.is_empty() { 0.0 } else { space::M + g.size().x };
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(width, 20.0_f32.max(g.size().y)),
        if enabled { Sense::click() } else { Sense::hover() },
    );
    if response.clicked() && enabled {
        *on = !*on;
    }
    if ui.is_rect_visible(rect) {
        paint_toggle(ui, rect, *on, enabled, response.has_focus());
        if !label.is_empty() {
            let fg = if enabled { t.text_primary } else { t.text_disabled };
            let g = galley(ui, label, Type::Body, fg);
            let h = g.size().y;
            ui.painter().galley(
                Pos2::new(rect.left() + 36.0 + space::M, rect.center().y - h / 2.0),
                g,
                fg,
            );
        }
    }
    let is_on = *on;
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, is_on, label));
    response
}

/// The 36 × 20 track and its knob, shared by both toggle shapes.
fn paint_toggle(ui: &Ui, rect: Rect, on: bool, enabled: bool, focused: bool) {
    let t = theme::tokens(ui.ctx());
    let track =
        Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - 10.0), Vec2::new(36.0, 20.0));
    let cr = CornerRadius::same(10);
    if on && enabled {
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
        let knob_x = if on { track.right() - 10.0 } else { track.left() + 10.0 };
        ui.painter().circle_filled(
            Pos2::new(knob_x, track.center().y),
            8.0,
            if enabled { t.text_secondary } else { t.text_disabled },
        );
    }
    if focused {
        focus_ring(ui, track, cr);
    }
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
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(total, size::CONTROL_H), Sense::click());
    let was = *selected;

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
            ui.painter().galley(
                Pos2::new(seg.left() + pad, seg.center().y - g.size().y / 2.0),
                g,
                fg,
            );
            if marked.contains(&i) {
                ui.painter().circle_filled(
                    Pos2::new(seg.right() - pad + 2.0, seg.center().y),
                    3.0,
                    t.accent,
                );
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
    // `changed()` has to be true when the selection changed, or it is a lie.
    //
    // It was never set, so a caller that trusted it saw every click as no
    // change: the git repository dialog wrote the new tab back only
    // `if segmented(..).changed()`, and its Branches, Working trees and
    // Documents tabs did nothing at all. The other five call sites read
    // `selected` directly and so were unaffected, which is exactly why this
    // went unnoticed — the bug was in the one place that used the documented
    // return value.
    if *selected != was {
        response.mark_changed();
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
    announce: Option<&'a str>,
    id: Option<egui::Id>,
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
            announce: None,
            id: None,
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
    /// The name a screen reader reads, for a field whose label is drawn by the
    /// caller — beside it in a row, say — rather than above it by `Field`.
    /// Without this such a field announces itself as an unnamed text box.
    pub fn announce(mut self, name: &'a str) -> Self {
        self.announce = Some(name);
        self
    }

    /// Show the field under a caller-chosen id, so the caller can ask egui
    /// whether it currently has focus. A field whose text is derived from
    /// something else — a number held as a `u32` — has to know that, or it
    /// overwrites what is being typed on the very next frame.
    pub fn show_with_id(mut self, ui: &mut Ui, id: egui::Id, value: &mut String) -> Response {
        self.id = Some(id);
        self.show(ui, value)
    }

    pub fn show(self, ui: &mut Ui, value: &mut String) -> Response {
        let t = theme::tokens(ui.ctx());
        let mut response = None;
        // `ui.scope` would inherit the caller's layout, which lays a label,
        // an input and its helper text out side by side inside a horizontal
        // row. A field is always a vertical stack of a known width.
        let bare = self.label.is_none()
            && self.helper.is_none()
            && self.error.is_none()
            && self.char_limit.is_none()
            && self.multiline_rows.is_none();
        let block = Vec2::new(
            self.width.max(if self.label.is_some() { 120.0 } else { 40.0 }),
            // A bare field is exactly one control tall, so a centred layout —
            // a header bar — puts it on the same baseline as the buttons.
            if bare { size::CONTROL_H } else { 0.0 },
        );
        ui.allocate_ui_with_layout(block, Layout::top_down(Align::Min), |ui| {
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
                edit =
                    edit.hint_text(egui::RichText::new(p).color(t.text_muted).font(font.clone()));
            }
            if let Some(n) = self.char_limit {
                edit = edit.char_limit(n);
            }
            if let Some(id) = self.id {
                edit = edit.id(id);
            }

            // Both the widget's own response rect and the rect its frame was
            // drawn at are needed. `TextEdit` reports the *content* rect, but
            // draws its frame at the outer rect that includes the margin — so
            // painting our border at the response rect put it a few pixels
            // inside egui's, and the field appeared to have a second rounded
            // ring floating inside it. The border must go on the outer rect.
            // Declared without a value: both branches below assign it, and an
            // initialiser here would be a dead store that clippy is right to
            // object to.
            let frame_rect;
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
                    multi = multi
                        .hint_text(egui::RichText::new(p).color(t.text_muted).font(font.clone()));
                }
                {
                    let scoped = ui.scope(|ui| {
                        suppress_builtin_frame_stroke(ui);
                        ui.add_enabled(self.enabled, multi)
                    });
                    frame_rect = Some(scoped.response.rect);
                    scoped.inner
                }
            } else {
                {
                    let scoped = ui.scope(|ui| {
                        ui.set_min_height(size::CONTROL_H);
                        suppress_builtin_frame_stroke(ui);
                        ui.add_enabled(self.enabled, edit)
                    });
                    frame_rect = Some(scoped.response.rect);
                    scoped.inner
                }
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
            let border_rect = frame_rect.unwrap_or(r.rect);
            ui.painter().rect_stroke(border_rect, radius::CONTROL, stroke, StrokeKind::Inside);

            if let Some(u) = self.unit {
                let g = galley(ui, u, Type::Small, t.text_muted);
                ui.painter().galley(
                    Pos2::new(
                        r.rect.right() - 10.0 - g.size().x,
                        r.rect.center().y - g.size().y / 2.0,
                    ),
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
            let label = self.announce.or(self.label).unwrap_or("");
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

/// Stop egui drawing its own outline inside a text field.
///
/// `TextEdit` paints a frame — a background *and* a stroke — and this widget
/// paints its own border over the top. The result was two concentric rounded
/// rectangles a couple of pixels apart, most obvious when focused, where
/// egui's `selection.stroke` is a bright accent: the field appeared to have a
/// stray ring floating inside it.
///
/// The strokes are cleared rather than the whole frame disabled, because the
/// frame is also what fills the background, and painting that ourselves after
/// the widget would cover the text.
fn suppress_builtin_frame_stroke(ui: &mut Ui) {
    let v = ui.visuals_mut();
    v.selection.stroke = Stroke::NONE;
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        w.bg_stroke = Stroke::NONE;
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
    let t = theme::tokens(ui.ctx());
    let mut response = None;
    // The label is drawn here rather than by `Field`, and that is the whole
    // fix. With the label inside the field, the row contained a two-line block
    // (label above input) beside a one-line button, and egui centred the button
    // against the *whole* block — putting the eye up beside the label instead
    // of in the input. With the label outside, the row holds an input and a
    // button that are both exactly `CONTROL_H` tall, so they align by
    // construction rather than by a hand-tuned offset.
    ui.vertical(|ui| {
        text(ui, label, Type::H3, t.text_primary);
        ui.add_space(space::S);
        ui.horizontal_top(|ui| {
            let mut field = Field::new().width(width - 34.0).error(error);
            if !*revealed {
                field = field.password();
            }
            let r = field.show(ui, value);
            response = Some(r);
            ui.add_space(space::XS);
            let icon = if *revealed { Icon::EyeOff } else { Icon::Eye };
            let hint = if *revealed { "Hide the passphrase" } else { "Show the passphrase" };
            if icon_button(ui, icon, hint, true).clicked() {
                *revealed = !*revealed;
            }
        });
    });
    response.expect("the vertical layout always runs")
}

/// One bandwidth limit: checkbox, a Mbit/s box, and a notched slider — laid
/// out on a fixed label column so upload and download line up.
///
/// **Megabits per second everywhere.** The value is stored as kB/s, because
/// that is what kopia's `--max-upload-speed` takes, but that unit is never
/// shown: connections are sold in Mbit/s, every speed test reports Mbit/s, and
/// a user who wants "half of my 100/100 line" should be able to type `50`. The
/// conversion is this control's job, not theirs.
///
/// The box and the slider edit the same number in the same unit. The box is
/// authoritative: typing 37 leaves 37, and only the slider snaps to a 10 Mbit
/// notch. Rounding what somebody deliberately typed is the behaviour that
/// makes a control feel like it is arguing.
/// kB/s -> Mbit/s. 1 Mbit/s is 125 kB/s.
///
/// Rounded to a whole Mbit and never to zero: a limit read back as `0` would
/// say "no bandwidth" when it means "less than one megabit", and the checkbox
/// is what turns a limit off.
pub fn kbps_to_mbit(kbps: u32) -> u32 {
    (kbps / 125).max(1)
}

/// Mbit/s -> kB/s, the unit the limit is stored and passed to kopia in.
pub fn mbit_to_kbps(mbit: u32) -> u32 {
    mbit.saturating_mul(125).max(125)
}

pub fn bandwidth_control(ui: &mut Ui, id: &str, label: &str, value: &mut Option<u32>) -> bool {
    const LABEL_W: f32 = 150.0;
    let t = theme::tokens(ui.ctx());
    let mut changed = false;

    ui.horizontal(|ui| {
        let mut on = value.is_some();
        // The checkbox carries the label, and the pair is padded to a fixed
        // width so everything after it starts at the same x on every row.
        let before_x = ui.cursor().left();
        if checkbox(ui, &mut on, label, None, true).clicked() {
            *value = if on { Some(mbit_to_kbps(50)) } else { None };
            changed = true;
        }
        let used = ui.cursor().left() - before_x;
        if used < LABEL_W {
            ui.add_space(LABEL_W - used);
        }

        if let Some(v) = value.as_mut() {
            // Edited in Mbit/s and converted back, so the round trip is
            // exact for anything the box can hold and the stored kB/s never
            // surfaces.
            let mut mbit = kbps_to_mbit(*v);
            number(ui, &mut mbit, 1..=10_000, super::copy::set::BW_UNIT, true, label);
            let as_kbps = mbit_to_kbps(mbit);
            if as_kbps != *v {
                *v = as_kbps;
                changed = true;
            }
        } else {
            text(ui, super::copy::set::BW_UNLIMITED, Type::Small, t.text_muted);
        }
    });

    if let Some(v) = value.as_mut() {
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            ui.add_space(LABEL_W);
            ui.vertical(|ui| {
                if mbit_slider(ui, id, v) {
                    changed = true;
                }
            });
        });
    }
    changed
}

/// The application's own mark, at any size.
///
/// The About page used to draw `health_mark(Health::Idle)` here — the status
/// dot from the tray, which at 64px is a plain blue circle. It is the one
/// screen whose entire job is to say what this program is, and it was showing
/// a shape that belongs to something else. This is the same artwork the window
/// icon, the taskbar and the installer use, so the application looks like one
/// product rather than three.
///
/// Decoded once and kept as a texture in egui's memory: the About page is
/// rebuilt every frame, and decoding a 256px PNG sixty times a second to draw
/// one logo is work nobody asked for.
pub fn app_logo(ui: &mut Ui, size: f32) -> Response {
    const PNG: &[u8] = include_bytes!("../../../../assets/icons/png/superbackup-256.png");
    let key = egui::Id::new("superbackup-app-logo");

    let cached: Option<egui::TextureHandle> = ui.ctx().data_mut(|d| d.get_temp(key));
    let texture = match cached {
        Some(handle) => Some(handle),
        None => image::load_from_memory(PNG).ok().map(|decoded| {
            let rgba = decoded.to_rgba8();
            let (w, h) = rgba.dimensions();
            let image =
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
            let handle =
                ui.ctx().load_texture("superbackup-app-logo", image, egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|d| d.insert_temp(key, handle.clone()));
            handle
        }),
    };

    match texture {
        Some(handle) => ui.add(egui::Image::new(&handle).fit_to_exact_size(Vec2::splat(size))),
        // A PNG that will not decode is not a reason to leave a hole in the
        // page; the accent mark still reads as "this application".
        None => {
            let t = theme::tokens(ui.ctx());
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
            super::icons::health_mark(
                ui.painter(),
                rect,
                superbackup_core::state::Health::Idle,
                t.accent,
                None,
                0.0,
            );
            response
        }
    }
}

/// Resample a polyline into a smooth curve through the same points.
///
/// Centripetal-ish Catmull-Rom: the curve passes through every sample rather
/// than being pulled off them the way a Bézier control polygon would, which
/// matters because these *are* the measurements and a line that misses them is
/// a line that lies about them.
///
/// The ends are handled by reflecting the first and last points outwards, so
/// the curve starts and finishes at the real data instead of drifting.
/// Overshoot — Catmull-Rom can bulge past a local maximum — is clamped to the
/// band the caller gives, because a throughput graph that dips below zero or
/// escapes its own box looks broken rather than smooth.
pub fn smooth_curve(points: &[Pos2], per_segment: usize, min_y: f32, max_y: f32) -> Vec<Pos2> {
    if points.len() < 3 || per_segment < 2 {
        return points.to_vec();
    }
    let at = |i: isize| -> Pos2 {
        let last = points.len() as isize - 1;
        if i < 0 {
            // Reflect: p(-1) = 2*p0 - p1.
            let (a, b) = (points[0], points[1]);
            Pos2::new(2.0 * a.x - b.x, 2.0 * a.y - b.y)
        } else if i > last {
            let (a, b) = (points[last as usize], points[(last - 1) as usize]);
            Pos2::new(2.0 * a.x - b.x, 2.0 * a.y - b.y)
        } else {
            points[i as usize]
        }
    };

    let mut out = Vec::with_capacity(points.len() * per_segment);
    for i in 0..points.len() - 1 {
        let (p0, p1, p2, p3) =
            (at(i as isize - 1), at(i as isize), at(i as isize + 1), at(i as isize + 2));
        for step in 0..per_segment {
            let t = step as f32 / per_segment as f32;
            let (t2, t3) = (t * t, t * t * t);
            let x = 0.5
                * ((2.0 * p1.x)
                    + (-p0.x + p2.x) * t
                    + (2.0 * p0.x - 5.0 * p1.x + 4.0 * p2.x - p3.x) * t2
                    + (-p0.x + 3.0 * p1.x - 3.0 * p2.x + p3.x) * t3);
            let y = 0.5
                * ((2.0 * p1.y)
                    + (-p0.y + p2.y) * t
                    + (2.0 * p0.y - 5.0 * p1.y + 4.0 * p2.y - p3.y) * t2
                    + (-p0.y + 3.0 * p1.y - 3.0 * p2.y + p3.y) * t3);
            out.push(Pos2::new(x, y.clamp(min_y, max_y)));
        }
    }
    out.push(*points.last().expect("points is not empty"));
    out
}

/// A filled line graph of recent transfer rates.
///
/// Answers the question a single "89 MB/s" cannot: *is it still moving?* A
/// number that has stopped changing looks identical to a healthy one, and on a
/// backup of a large tree the difference between "slow" and "stalled" is the
/// difference between waiting and intervening.
///
/// Scaled to its own peak rather than to a fixed ceiling, because a backup to a
/// local disk and one to a metered uplink differ by two orders of magnitude and
/// a shared scale would flatten one of them into a straight line. The peak is
/// labelled so the shape cannot be mistaken for an absolute reading.
///
/// Drawn as a gradient mesh under a smoothed curve. The fill used to be
/// `Shape::convex_polygon`, and a throughput curve is not convex — egui
/// triangulates that assuming it is, which produced the crossing slabs and
/// stray wedges the graph was full of. A triangle strip is correct for any
/// shape, and lets the fill fade out downwards instead of being a flat block.
///
/// Returns the peak it scaled to, so a caller can label it consistently.
pub fn throughput_graph(
    ui: &mut Ui,
    samples: &std::collections::VecDeque<f64>,
    width: f32,
    height: f32,
) -> f64 {
    let t = theme::tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let radius = CornerRadius::same(6);
    let painter = ui.painter().with_clip_rect(rect);

    painter.rect_filled(rect, radius, t.bg_rail);
    let peak = samples.iter().copied().fold(0.0_f64, f64::max);
    // Below a byte a second there is nothing to draw and no sensible scale.
    if samples.len() < 2 || peak <= 1.0 {
        let g = galley(ui, super::copy::dash::GRAPH_WAITING, Type::MonoSmall, t.text_muted);
        painter.galley(rect.center() - g.size() / 2.0, g, t.text_muted);
        return peak;
    }

    let inset = 6.0;
    let top = rect.top() + inset;
    let bottom = rect.bottom() - 1.0;
    let n = samples.len();
    let x_at = |i: usize| rect.left() + rect.width() * (i as f32 / (n - 1) as f32);
    let y_at = |v: f64| bottom - (bottom - top) * (v / peak).clamp(0.0, 1.0) as f32;

    // A quarter and a half line, so the height of the shape can be read
    // against something. Faint enough to stay behind the data.
    for fraction in [0.5_f32, 0.25] {
        let y = bottom - (bottom - top) * fraction;
        painter.hline(
            rect.left()..=rect.right(),
            y,
            Stroke::new(1.0_f32, t.border_subtle.gamma_multiply(0.5)),
        );
    }

    let raw: Vec<Pos2> =
        samples.iter().enumerate().map(|(i, v)| Pos2::new(x_at(i), y_at(*v))).collect();
    let curve = smooth_curve(&raw, 12, top, bottom);

    // The fill: a triangle strip from the curve down to the baseline, fading
    // out as it goes, so the shape reads as volume without becoming a slab.
    let mut mesh = egui::Mesh::default();
    for (index, point) in curve.iter().enumerate() {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(*point, t.accent.gamma_multiply(0.38));
        mesh.colored_vertex(Pos2::new(point.x, bottom), t.accent.gamma_multiply(0.02));
        if index > 0 {
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i, i + 1);
        }
    }
    painter.add(egui::Shape::mesh(mesh));

    painter.add(egui::Shape::line(curve.clone(), Stroke::new(1.75_f32, t.accent)));

    // Where the latest reading is. On a graph whose right edge is "now", the
    // eye needs somewhere to land — and when the rate has just dropped to
    // zero, the dot sitting on the baseline says so unmistakably.
    if let Some(last) = curve.last() {
        painter.circle_filled(*last, 3.0, t.accent);
        painter.circle_filled(*last, 1.25, t.bg_rail);
    }

    // The peak, so the shape is not mistaken for an absolute scale.
    let g = galley(ui, super::format::rate(peak), Type::MonoSmall, t.text_muted);
    painter.galley(Pos2::new(rect.right() - g.size().x - 6.0, rect.top() + 3.0), g, t.text_muted);
    peak
}

/// A bandwidth slider marked off in 10 Mbit/s notches, 0 to 1000.
///
/// The stored unit is kB/s, because that is what kopia's
/// `--upload-bytes-per-second` takes and what the number box has always shown.
/// The slider works in Mbit/s because that is the unit an internet connection
/// is sold in — nobody knows their line as 12500 kB/s.
///
/// **The number box stays authoritative.** Dragging snaps to a 10 Mbit notch,
/// but a typed value is left exactly as typed: the slider moves to show it and
/// does not round it. Otherwise typing 2000 kB/s would silently become 1875,
/// which is a worse answer than the one the user gave.
///
/// Returns true when the drag changed the value.
pub fn mbit_slider(ui: &mut Ui, id: impl std::hash::Hash, kbps: &mut u32) -> bool {
    // Scope the interaction id, so the upload and download sliders keep their
    // own drag and focus state rather than sharing whatever egui derives from
    // position.
    ui.push_id(id, |ui| mbit_slider_inner(ui, kbps)).inner
}

fn mbit_slider_inner(ui: &mut Ui, kbps: &mut u32) -> bool {
    const MAX_MBIT: f32 = 1000.0;
    const STEP_MBIT: f32 = 10.0;
    // kB/s -> Mbit/s is *8/1000; a 10 Mbit notch is therefore 1250 kB/s.
    const KBPS_PER_MBIT: f32 = 125.0;

    let t = theme::tokens(ui.ctx());
    let width = ui.available_width().min(560.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 46.0), Sense::click_and_drag());
    let painter = ui.painter();

    let track_y = rect.top() + 10.0;
    let track = Rect::from_min_max(
        Pos2::new(rect.left() + 8.0, track_y - 3.0),
        Pos2::new(rect.right() - 8.0, track_y + 3.0),
    );

    let mbit = (*kbps as f32) * 8.0 / 1000.0;
    let fraction = (mbit / MAX_MBIT).clamp(0.0, 1.0);

    let mut changed = false;
    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let raw = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0) * MAX_MBIT;
            // Snap to the nearest notch, and never to zero: a limit of zero
            // would read as "no bandwidth" rather than "no limit", and the
            // checkbox is what turns the limit off.
            let snapped = (raw / STEP_MBIT).round() * STEP_MBIT;
            let value = (snapped.max(STEP_MBIT) * KBPS_PER_MBIT).round() as u32;
            if value != *kbps {
                *kbps = value;
                changed = true;
            }
        }
    }
    if response.has_focus() {
        let step = |mult: f32| (STEP_MBIT * mult * KBPS_PER_MBIT).round() as i64;
        let mut delta = 0i64;
        ui.input(|i| {
            if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp) {
                delta += step(1.0);
            }
            if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown) {
                delta -= step(1.0);
            }
        });
        if delta != 0 {
            let next = (*kbps as i64 + delta)
                .clamp((STEP_MBIT * KBPS_PER_MBIT) as i64, (MAX_MBIT * KBPS_PER_MBIT) as i64);
            if next as u32 != *kbps {
                *kbps = next as u32;
                changed = true;
            }
        }
    }

    // Re-read: a drag may have moved it this frame.
    let fraction =
        if changed { ((*kbps as f32) * 8.0 / 1000.0 / MAX_MBIT).clamp(0.0, 1.0) } else { fraction };
    let handle_x = track.left() + track.width() * fraction;

    // Notches. Every 10 Mbit as a hairline, every 100 taller, so the step the
    // slider actually moves in is visible rather than merely documented.
    let notches = (MAX_MBIT / STEP_MBIT) as i32;
    for i in 0..=notches {
        let at = track.left() + track.width() * (i as f32 / notches as f32);
        let major = i % 10 == 0;
        let height = if major { 7.0 } else { 3.5 };
        let colour = if major { t.border_strong } else { t.border_subtle };
        painter.line_segment(
            [Pos2::new(at, track.bottom() + 4.0), Pos2::new(at, track.bottom() + 4.0 + height)],
            Stroke::new(1.0_f32, colour),
        );
    }

    painter.rect_filled(track, CornerRadius::same(3), t.border_subtle);
    painter.rect_filled(
        Rect::from_min_max(track.left_top(), Pos2::new(handle_x, track.bottom())),
        CornerRadius::same(3),
        t.accent,
    );
    let handle = Pos2::new(handle_x, track_y);
    painter.circle_filled(handle, 9.0, t.accent);
    painter.circle_stroke(handle, 9.0, Stroke::new(2.0_f32, t.bg_surface));
    if response.has_focus() {
        focus_ring(ui, Rect::from_center_size(handle, Vec2::splat(26.0)), CornerRadius::same(13));
    }

    // The value, on the handle, while it is being moved or hovered. A slider
    // whose position is its only readout makes the user look somewhere else to
    // find out what they just chose.
    if response.dragged() || response.hovered() || response.has_focus() {
        let mbit_now = (*kbps as f32) * 8.0 / 1000.0;
        let label =
            if mbit_now < 10.0 { format!("{mbit_now:.1}") } else { format!("{mbit_now:.0}") };
        let g = galley(ui, label, Type::MonoSmall, t.text_oncolor);
        let pad = Vec2::new(8.0, 4.0);
        let bubble =
            Rect::from_center_size(Pos2::new(handle_x, track_y - 20.0), g.size() + pad * 2.0);
        painter.rect_filled(bubble, CornerRadius::same(5), t.accent_fill);
        painter.galley(bubble.min + pad, g, t.text_oncolor);
    }

    // End labels only. A number under every hundredth notch is noise when the
    // exact value is already in the box above.
    for (at, label) in [(0.0_f32, "0"), (0.25, "250"), (0.5, "500"), (0.75, "750"), (1.0, "1000")] {
        let x = track.left() + track.width() * at;
        let g = galley(ui, label, Type::MonoSmall, t.text_muted);
        painter.galley(Pos2::new(x - g.size().x / 2.0, track.bottom() + 14.0), g, t.text_muted);
    }

    changed
}

/// A numeric field: a box you type into, with the unit drawn beside it.
///
/// This used to be an egui `DragValue`, which changes its value when the
/// pointer is dragged *across the box*. Nothing on screen says so, so the
/// number moved when someone meant to select the text in it — a control that
/// silently edits itself while you are trying to read it. Where a value wants
/// dragging there is a real slider next to the box ([`mbit_slider`]); the box
/// itself now does one thing, which is take what is typed.
///
/// The text being edited lives in egui's memory rather than in the caller's
/// `u32`, because a half-typed number is not a number: clearing the box to
/// retype it passes through `""`, and `"0"` on the way to `"0…"`. Writing
/// those back would fight the typist. The buffer is authoritative while the
/// field has focus and is re-synced from `value` the moment it loses focus, so
/// what is left behind is always the clamped value the caller holds.
pub fn number(
    ui: &mut Ui,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
    unit: &str,
    enabled: bool,
    label: &str,
) -> Response {
    let t = theme::tokens(ui.ctx());
    // Wide enough for the largest value the range allows, so the digits never
    // scroll out of a box that had room for them.
    let digits = range.end().to_string().len() as f32;
    let width = (34.0 + digits * 9.0).clamp(56.0, 130.0);

    let id = ui.auto_id_with(("number", label));
    let mut buffer: String =
        ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| (*value).to_string());

    let mut response = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        let focused = ui.memory(|m| m.has_focus(id));
        if !focused {
            // Not being edited: the caller's value wins, including any clamp
            // it applied since the last frame.
            buffer = (*value).to_string();
        }

        let r = Field::new().width(width).enabled(enabled).announce(label).show_with_id(
            ui,
            id,
            &mut buffer,
        );

        if r.changed() {
            // Digits only. Rejecting the character is quieter than accepting
            // it and then showing an error about it.
            buffer.retain(|c| c.is_ascii_digit());
            // A partial entry — empty, or still being extended — leaves the
            // caller's value alone until it parses inside the range.
            if let Ok(parsed) = buffer.parse::<u32>() {
                if range.contains(&parsed) {
                    *value = parsed;
                }
            }
        }
        if r.lost_focus() {
            // Whatever was left in the box is now settled: an out-of-range or
            // unfinished entry snaps back to the value actually held.
            let settled =
                buffer.parse::<u32>().unwrap_or(*value).clamp(*range.start(), *range.end());
            *value = settled;
            buffer = settled.to_string();
        }

        if !unit.is_empty() {
            text(ui, unit, Type::Small, t.text_muted);
        }
        let v = *value;
        r.widget_info(|| {
            let mut info =
                WidgetInfo::labeled(WidgetType::TextEdit, enabled, format!("{label}, {v} {unit}"));
            info.value = Some(v as f64);
            info
        });
        response = Some(r);
    });
    ui.data_mut(|d| d.insert_temp(id, buffer));
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
    combo_labelled(ui, id, None, selected, options, width, enabled)
}

/// The same, with the control's name carried in the closed state — a filter
/// that reads `All` tells the user nothing; `Filter: All` tells them what it
/// filters.
pub fn combo_labelled(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    label: Option<&str>,
    selected: &mut usize,
    options: &[String],
    width: f32,
    enabled: bool,
) -> bool {
    let mut changed = false;
    let value = options.get(*selected).cloned().unwrap_or_default();
    let current = match label {
        Some(label) => format!("{label}: {value}"),
        None => value,
    };
    ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt(id).selected_text(current).width(width).height(320.0).show_ui(
            ui,
            |ui| {
                for (i, option) in options.iter().enumerate() {
                    if ui.selectable_label(*selected == i, option).clicked() {
                        *selected = i;
                        changed = true;
                    }
                }
            },
        );
    });
    changed
}

/// A vertical scroll area whose content stops short of the scroll bar.
///
/// egui draws the bar over the content, so a table's last column or a card's
/// right border would otherwise be clipped the moment a list grew past the
/// fold. Reserving the width here keeps every screen's right edge stable
/// whether or not it scrolls.
pub fn scroll_area<R>(ui: &mut Ui, id: impl std::hash::Hash, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = (ui.available_width() - SCROLL_RESERVE).max(120.0);
            ui.set_max_width(width);
            add(ui)
        })
        .inner
}

/// Scroll-bar width plus its inner margin (`DESIGN_SYSTEM.md` §4.2).
pub const SCROLL_RESERVE: f32 = 14.0;

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

    // Measure the block before placing it, so it sits in the true middle.
    //
    // This used to be `available.y * 0.45 - 90.0`, a guess calibrated for a
    // full-page empty state. Inside a short box — the folder list in the job
    // wizard is 140px — the expression goes negative, clamps to zero, and the
    // content lands hard against the top edge instead of centred.
    let body_text = body_override.unwrap_or(empty.body);
    let title_h = galley(ui, empty.title, Type::H2, t.text_primary).size().y;
    let body_h = galley_wrapped(ui, body_text, Type::Body, t.text_secondary, 420.0).size().y;
    let mut content_h = 32.0 + space::XL + title_h + space::M + body_h;
    if empty.primary.is_some() || empty.secondary.is_some() {
        content_h += space::XXL + size::CONTROL_H;
    }
    let top = ((available.y - content_h) / 2.0).max(0.0);

    ui.allocate_ui_with_layout(available, Layout::top_down(Align::Center), |ui| {
        ui.add_space(top);
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
        icon.paint(ui.painter(), ir, t.text_muted);
        ui.add_space(space::XL);
        text(ui, empty.title, Type::H2, t.text_primary);
        ui.add_space(space::M);
        ui.allocate_ui_with_layout(Vec2::new(420.0, 0.0), Layout::top_down(Align::Center), |ui| {
            let body = body_override.unwrap_or(empty.body);
            let g = galley_wrapped(ui, body, Type::Body, t.text_secondary, 420.0);
            let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
            ui.painter().galley(rect.min, g, t.text_secondary);
        });
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
                // Elided to the column. The label is drawn right-to-left, so
                // one longer than KV_LABEL_W grows *leftwards* out of its box
                // and out of whatever card it sits in — a destination named
                // `onedrive-superbackup-awpc34` pushed the whole page 40px
                // left, under the navigation rail. `text` draws its whole
                // string regardless of the space it was allocated.
                let full = elided(ui, label, Type::Body, t.text_secondary, size::KV_LABEL_W, false);
                full.on_hover_text(label);
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
                egui::ScrollArea::both().max_height(max_height).auto_shrink([false, true]).show(
                    ui,
                    |ui| {
                        ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
                        // Explicitly vertical. The scroll area inherits its
                        // layout from the `horizontal_top` above, so without
                        // this every line was laid out *beside* the previous
                        // one and a four-line block rendered as one long line.
                        ui.vertical(|ui| {
                            for line in content.lines() {
                                text(ui, line, Type::MonoSmall, t.text_primary);
                            }
                        });
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                    if icon_button_compact(ui, Icon::Copy, super::copy::action::COPY, true)
                        .clicked()
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
/// The card every table sits in.
///
/// The inner margin is not decoration. With `Margin::ZERO` the first column's
/// icon was jammed against the card's left border and the row's trailing "…"
/// button against its right, with the rounded corner cutting past them. Every
/// table in the application shares this frame, so the inset belongs here rather
/// than being re-invented per screen.
///
/// Callers that size a flexible column must subtract [`TABLE_GUTTER`], as they
/// measure available width *before* this frame is entered.
/// The height one line of `ty` occupies.
pub fn line_height(ui: &Ui, ty: Type) -> f32 {
    ui.fonts(|f| f.row_height(&ty.font()))
}

/// A cell holding two or more stacked lines, vertically centred in its row.
///
/// # Why this is not just `ui.vertical`
///
/// It was, and that is the bug it exists to fix. `Ui::vertical` builds its
/// child from `available_rect_before_wrap()` with `Layout::top_down(Align::Min)`
/// and then allocates only the child's `min_rect` — so the block is anchored to
/// the **top** of the row, and egui's own documentation says as much: "the
/// amount of space actually used (`min_rect`) will be allocated in the parent".
///
/// A single widget dropped straight into a table cell *does* centre, because
/// the cell's own layout is `left_to_right(Align::Center)` over the full row
/// rect. So a one-line column and a two-line column in the same table sit on
/// different baselines, and the two-line one looks roughly right while the
/// one-line one is visibly high — which is exactly how this was reported.
///
/// Wrapping the block in another `with_layout(…Align::Center)` does **not**
/// fix it: the new layout's own height is the block's height, so there is
/// nothing to centre within. The height has to be measured and the slack
/// padded explicitly, which is what this does.
///
/// `lines` is the type of each line, in order. Spacing between them is zero,
/// matching the callers, so the block's height is the sum of the row heights.
pub fn stacked_cell<R>(ui: &mut Ui, lines: &[Type], add: impl FnOnce(&mut Ui) -> R) -> R {
    let content: f32 = lines.iter().map(|ty| line_height(ui, *ty)).sum();
    stacked_cell_of_height(ui, content, add)
}

/// [`stacked_cell`] for a block whose height the caller already knows.
///
/// Split out so the measurement and the placement can be tested apart: the
/// test context has no fonts, so measuring text there would prove nothing
/// about where the block lands.
pub fn stacked_cell_of_height<R>(
    ui: &mut Ui,
    content_height: f32,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    // `max(0)`: a row shorter than its content must not be pushed upward out
    // of itself, which negative padding would do.
    let pad = ((ui.available_height() - content_height) / 2.0).max(0.0);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        if pad > 0.0 {
            ui.add_space(pad);
        }
        add(ui)
    })
    .inner
}

/// A fixed-width cell that actually keeps its width.
///
/// # Why this is not just `allocate_ui_with_layout`
///
/// Because that does not do what its name suggests. From egui's own
/// documentation: "you can request a lot of space and then use less" — the
/// parent's cursor advances by the child's `min_rect`, not by the size asked
/// for. A hand-built table whose columns are laid out that way therefore has
/// no columns at all: every cell collapses to its content, so each row's
/// boundaries land wherever that row's text happens to end, and the header
/// row — whose labels are a different length from the data — drifts furthest
/// of all.
///
/// `set_min_size` inside the child is what makes the requested size real.
pub fn fixed_cell<R>(
    ui: &mut Ui,
    width: f32,
    height: f32,
    layout: Layout,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.allocate_ui_with_layout(Vec2::new(width, height), layout, |ui| {
        ui.set_min_size(Vec2::new(width, height));
        add(ui)
    })
    .inner
}

pub fn table_frame<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let t = theme::tokens(ui.ctx());
    egui::Frame::new()
        .fill(t.bg_surface)
        .stroke(Stroke::new(1.0_f32, t.border_subtle))
        .corner_radius(radius::CARD)
        .inner_margin(egui::Margin {
            left: TABLE_INSET as i8,
            right: TABLE_INSET as i8,
            top: 0,
            bottom: TABLE_INSET as i8 / 2,
        })
        .show(ui, add)
}

/// The vertical space a [`divider`] occupies, so callers reserving room for one
/// do not have to guess.
pub const DIVIDER_H: f32 = 1.0;

/// Horizontal breathing room between a table's content and its card edge.
pub const TABLE_INSET: f32 = 14.0;

/// The table card's border.
pub const TABLE_BORDER: f32 = 1.0;

/// Everything a [`table_frame`] takes from its parent's width before content
/// starts: both insets and both borders.
///
/// A caller sizing a flexible column measures *before* entering the frame, so
/// it must subtract this. Subtracting only the insets left the last column two
/// pixels too wide, which is exactly enough for the card edge to slide under
/// the window's right edge at the 900-pixel minimum size.
pub const TABLE_GUTTER: f32 = TABLE_INSET * 2.0 + TABLE_BORDER * 2.0;

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
                            if icon_button(ui, Icon::X, super::copy::action::CLOSE, true).clicked()
                            {
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
                clicked =
                    Button::secondary(super::copy::action::UNLOCK).compact().show(ui).clicked();
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
                [Pos2::new(rect.left(), rect.bottom()), Pos2::new(rect.right(), rect.bottom())],
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
    response.response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, a11y));
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

    /// A clickable container must not swallow its own buttons' clicks.
    ///
    /// egui breaks a hit-test tie by taking the **last** widget registered —
    /// "in case of a tie, take the last one = the one on top". So a container
    /// made clickable by an `ui.interact` over its rect *after* its contents
    /// are drawn sits on top of every button inside it. That is what the
    /// dashboard's job cards did: "Run now" never saw the press, and the card
    /// opened the job instead of starting it.
    ///
    /// `UiBuilder::sense` registers the container's own widget when the Ui is
    /// created, before its children, so the children win. This drives a real
    /// click through a real context to prove which, because the difference is
    /// invisible in the source and I had assumed the opposite.
    #[test]
    fn a_clickable_container_does_not_steal_its_buttons_clicks() {
        // Where both the container and the button are.
        let click_at = egui::pos2(50.0, 50.0);

        // `true` = the inner button reported the click.
        fn run(late_interact: bool, click_at: egui::Pos2) -> (bool, bool) {
            let ctx = egui::Context::default();
            ctx.set_fonts(egui::FontDefinitions::empty());
            let mut button_clicked = false;
            let mut container_clicked = false;

            // Two passes: the first lays out, the second delivers the click.
            for pass in 0..2 {
                let mut input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(200.0, 200.0),
                    )),
                    ..Default::default()
                };
                if pass == 1 {
                    input.events = vec![
                        egui::Event::PointerMoved(click_at),
                        egui::Event::PointerButton {
                            pos: click_at,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: Default::default(),
                        },
                        egui::Event::PointerButton {
                            pos: click_at,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: Default::default(),
                        },
                    ];
                }
                let _ = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let area =
                            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(120.0, 120.0));
                        if late_interact {
                            // The broken shape: contents first, container after.
                            let inner = ui
                                .allocate_new_ui(egui::UiBuilder::new().max_rect(area), |ui| {
                                    ui.allocate_response(egui::vec2(100.0, 100.0), Sense::click())
                                });
                            button_clicked = inner.inner.clicked();
                            let card =
                                ui.interact(area, egui::Id::new("late-card"), Sense::click());
                            container_clicked = card.clicked();
                        } else {
                            // The fixed shape: the container senses itself.
                            let inner = ui.scope_builder(
                                egui::UiBuilder::new().max_rect(area).sense(Sense::click()),
                                |ui| ui.allocate_response(egui::vec2(100.0, 100.0), Sense::click()),
                            );
                            button_clicked = inner.inner.clicked();
                            container_clicked = inner.response.clicked();
                        }
                    });
                });
            }
            (button_clicked, container_clicked)
        }

        // The premise: registered late, the container takes the click and the
        // button never sees it. This is the bug, asserted so the test would
        // have failed against the old code.
        let (button, container) = run(true, click_at);
        assert!(
            !button && container,
            "a container interacted with after its contents steals the click \
             (button: {button}, container: {container})"
        );

        // The fix: the button wins.
        let (button, _container) = run(false, click_at);
        assert!(button, "a container that senses itself must leave its buttons clickable");
    }

    /// A fixed-width cell must occupy the width it asked for.
    ///
    /// `allocate_ui_with_layout` does not, and egui says so in its own
    /// documentation: "you can request a lot of space and then use less" — the
    /// parent advances by the child's `min_rect`. A hand-built table laid out
    /// that way has no columns: every cell collapses to its content, so each
    /// row's boundaries land wherever that row's text ends and the header,
    /// whose labels are a different length from the data, drifts furthest.
    /// That is what the recent-runs table's staggered header was.
    ///
    /// Asserted as geometry, because geometry is precisely what nothing was
    /// checking.
    #[test]
    fn a_fixed_cell_keeps_its_width_when_its_content_is_narrow() {
        use std::cell::Cell;
        let naive = Cell::new(0.0_f32);
        let fixed = Cell::new(0.0_f32);

        egui::__run_test_ui(|ui| {
            ui.horizontal(|ui| {
                let before = ui.cursor().min.x;
                // What every column in that table used to do.
                ui.allocate_ui_with_layout(
                    Vec2::new(200.0, 18.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
                    },
                );
                naive.set(ui.cursor().min.x - before);
            });
            ui.horizontal(|ui| {
                let before = ui.cursor().min.x;
                fixed_cell(ui, 200.0, 18.0, Layout::left_to_right(Align::Center), |ui| {
                    ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
                });
                fixed.set(ui.cursor().min.x - before);
            });
        });

        assert!(
            naive.get() < 100.0,
            "the premise of this test is that the plain call collapses to its content; \
             it advanced {}",
            naive.get()
        );
        assert!(
            fixed.get() >= 200.0,
            "a 200px cell must advance the row by 200px, not {}",
            fixed.get()
        );
    }

    /// A stacked cell sits in the middle of its row, not against the top.
    ///
    /// `Ui::vertical` builds its child from the available rect with
    /// `top_down(Align::Min)` and allocates only the content, so the block is
    /// top-anchored — while a single widget dropped straight into a table cell
    /// centres, because the cell's own layout is `left_to_right(Align::Center)`
    /// over the full row. A one-line column and a two-line column in the same
    /// table therefore sit on different baselines, which is the Storage
    /// providers name column, reported twice.
    ///
    /// Wrapping the block in another centred layout does **not** fix it: that
    /// layout's height is the block's height, so there is no slack to centre
    /// within. This asserts the placement, which is the only thing that
    /// distinguishes the fix from the version that looked like one.
    #[test]
    fn a_stacked_cell_is_centred_in_its_row_rather_than_pinned_to_the_top() {
        use std::cell::Cell;
        const ROW: f32 = 60.0;
        const LINE: f32 = 10.0;

        let naive_row = Cell::new(0.0_f32);
        let stacked_row = Cell::new(0.0_f32);
        let naive_top = Cell::new(0.0_f32);
        let stacked_top = Cell::new(0.0_f32);

        egui::__run_test_ui(|ui| {
            // Two rows of a known height, the way a table hands a cell one.
            for stacked in [false, true] {
                ui.allocate_ui_with_layout(
                    Vec2::new(300.0, ROW),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_min_size(Vec2::new(300.0, ROW));
                        if stacked {
                            stacked_row.set(ui.max_rect().top());
                        } else {
                            naive_row.set(ui.max_rect().top());
                        }
                        let record = |ui: &mut Ui| {
                            let (r, _) =
                                ui.allocate_exact_size(Vec2::new(40.0, LINE), Sense::hover());
                            if stacked {
                                stacked_top.set(r.top());
                            } else {
                                naive_top.set(r.top());
                            }
                            ui.allocate_exact_size(Vec2::new(40.0, LINE), Sense::hover());
                        };
                        if stacked {
                            // Two lines whose heights the helper is told about
                            // explicitly, so the test does not depend on the
                            // fontless test context measuring text.
                            stacked_cell_of_height(ui, 2.0 * LINE, record);
                        } else {
                            ui.vertical(record);
                        }
                    },
                );
            }
        });

        assert!(
            (naive_top.get() - naive_row.get()).abs() < 1.0,
            "the premise: a plain vertical is pinned to the row's top ({} vs {})",
            naive_top.get(),
            naive_row.get()
        );
        let expected = stacked_row.get() + (ROW - 2.0 * LINE) / 2.0;
        assert!(
            (stacked_top.get() - expected).abs() < 1.0,
            "a two-line block in a {ROW}px row starts at {expected}, not {}",
            stacked_top.get()
        );
    }

    /// The curve has to pass through the readings, not near them: these *are*
    /// the measurements, and a line that misses them is a line that lies.
    #[test]
    fn the_smoothed_curve_still_passes_through_every_reading() {
        let points = vec![
            Pos2::new(0.0, 40.0),
            Pos2::new(10.0, 10.0),
            Pos2::new(20.0, 30.0),
            Pos2::new(30.0, 0.0),
        ];
        let curve = smooth_curve(&points, 12, 0.0, 40.0);
        assert!(curve.len() > points.len() * 8, "it is actually subdivided");

        for p in &points {
            let hit = curve.iter().any(|q| (q.x - p.x).abs() < 0.01 && (q.y - p.y).abs() < 0.01);
            assert!(hit, "{p:?} is not on the curve");
        }
        // The ends are the real first and last readings, not a drift towards
        // some imagined neighbour.
        assert_eq!(curve.first().copied(), Some(points[0]));
        assert_eq!(curve.last().copied(), Some(points[3]));
    }

    /// Catmull-Rom bulges past a local maximum. On a graph that means a curve
    /// leaving its own box, or a transfer rate drawn below zero — both of
    /// which read as broken rather than smooth.
    #[test]
    fn overshoot_is_clamped_to_the_band_it_was_given() {
        // A spike that will make the interpolation overshoot on both sides.
        let points = vec![
            Pos2::new(0.0, 50.0),
            Pos2::new(10.0, 50.0),
            Pos2::new(20.0, 0.0),
            Pos2::new(30.0, 50.0),
            Pos2::new(40.0, 50.0),
        ];
        let curve = smooth_curve(&points, 16, 0.0, 50.0);
        for p in &curve {
            assert!(p.y >= 0.0 && p.y <= 50.0, "{p:?} escaped the band");
        }
    }

    /// Too few points to interpolate, or a degenerate subdivision, gives the
    /// input back rather than an empty graph.
    #[test]
    fn too_little_data_is_returned_unchanged() {
        let two = vec![Pos2::new(0.0, 1.0), Pos2::new(1.0, 2.0)];
        assert_eq!(smooth_curve(&two, 12, 0.0, 10.0), two);
        assert_eq!(smooth_curve(&[], 12, 0.0, 10.0), Vec::<Pos2>::new());

        let three = vec![Pos2::new(0.0, 1.0), Pos2::new(1.0, 2.0), Pos2::new(2.0, 1.0)];
        assert_eq!(smooth_curve(&three, 1, 0.0, 10.0), three, "no subdivision to do");
    }

    /// The interface shows megabits and stores kilobytes. Anything typed into
    /// the box has to survive the round trip, or a limit set to 50 reopens as
    /// 49 and drifts down every time the page is visited.
    #[test]
    fn a_bandwidth_limit_typed_in_megabits_comes_back_unchanged() {
        for mbit in [1u32, 7, 10, 37, 50, 100, 250, 1000, 10_000] {
            assert_eq!(kbps_to_mbit(mbit_to_kbps(mbit)), mbit, "{mbit} Mbit/s did not survive");
        }
        // 1 Mbit/s is 125 kB/s, and a limit never reads back as zero.
        assert_eq!(mbit_to_kbps(1), 125);
        assert_eq!(mbit_to_kbps(0), 125);
        assert_eq!(kbps_to_mbit(0), 1);
        assert_eq!(kbps_to_mbit(124), 1);
    }

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
