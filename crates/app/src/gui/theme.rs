//! Colour tokens, type scale, spacing and the `egui::Visuals` derived from
//! them.
//!
//! This is the **only** module in the interface allowed to name a colour
//! (`DESIGN_SYSTEM.md` §11). Every value below is transcribed from
//! `DESIGN_SYSTEM.md` §2, including the measured contrast ratios kept in the
//! comments so a future edit cannot quietly drop below AA.
//!
//! Tokens are published into `egui::Context` memory once per frame and read
//! back by every component, which is how egui code avoids threading a theme
//! reference through several hundred function signatures.

use egui::{Color32, Context, CornerRadius, FontFamily, FontId, Id, Margin, Stroke, TextStyle};
use superbackup_core::model::Theme;

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// One status colour in its three roles: the mark (icon, dot, bar), the tint
/// background a badge sits on, and the text colour that reads on that tint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub mark: Color32,
    pub tint_bg: Color32,
    pub tint_text: Color32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    pub dark: bool,

    // Surfaces
    pub bg_canvas: Color32,
    pub bg_rail: Color32,
    pub bg_surface: Color32,
    pub bg_surface_hover: Color32,
    pub bg_raised: Color32,
    pub bg_input: Color32,
    pub bg_code: Color32,
    pub bg_scrim: Color32,
    pub bg_selected: Color32,

    // Lines
    pub border_subtle: Color32,
    pub border_strong: Color32,
    pub border_control: Color32,
    pub border_focus: Color32,

    // Text
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub text_disabled: Color32,
    pub text_oncolor: Color32,
    pub text_link: Color32,

    // Accent
    pub accent: Color32,
    pub accent_fill: Color32,
    pub accent_fill_hover: Color32,
    pub accent_fill_active: Color32,
    pub accent_fill_border: Color32,

    // Status
    pub success: Status,
    pub warning: Status,
    pub danger: Status,
    pub info: Status,
    pub neutral: Status,

    // Danger fills
    pub danger_fill: Color32,
    pub danger_fill_hover: Color32,
    pub danger_fill_active: Color32,
    pub danger_fill_border: Color32,

    // Misc
    pub progress_track: Color32,
    pub progress_fill: Color32,
    pub progress_fill_warn: Color32,
    pub progress_fill_error: Color32,
    pub rail_selected_bg: Color32,
    pub rail_selected_marker: Color32,
    pub overlay_locked: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}
/// Premultiplied at compile time: `Color32::from_rgba_unmultiplied` is not a
/// `const fn`, and every token in this file is a constant.
const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_premultiplied(
        ((r as u32 * a as u32 + 127) / 255) as u8,
        ((g as u32 * a as u32 + 127) / 255) as u8,
        ((b as u32 * a as u32 + 127) / 255) as u8,
        a,
    )
}

impl Tokens {
    /// `DESIGN_SYSTEM.md` §2.1. The default theme.
    pub const fn dark() -> Tokens {
        Tokens {
            dark: true,

            bg_canvas: rgb(0x14, 0x17, 0x1C),
            bg_rail: rgb(0x10, 0x13, 0x18),
            bg_surface: rgb(0x1B, 0x1F, 0x26),
            bg_surface_hover: rgb(0x20, 0x25, 0x2E),
            bg_raised: rgb(0x23, 0x28, 0x33),
            bg_input: rgb(0x0F, 0x12, 0x16),
            bg_code: rgb(0x12, 0x16, 0x1B),
            bg_scrim: rgba(0x06, 0x08, 0x0B, 158), // 62%
            bg_selected: rgb(0x23, 0x40, 0x65),

            border_subtle: rgb(0x2A, 0x30, 0x3B),
            border_strong: rgb(0x3A, 0x42, 0x50),
            border_control: rgb(0x6A, 0x75, 0x83), // 3.53 : bg.surface
            border_focus: rgb(0x7F, 0xB4, 0xFF),   // 8.45 : bg.canvas

            text_primary: rgb(0xE8, 0xEC, 0xF2),   // 13.94 : bg.surface
            text_secondary: rgb(0xA3, 0xAD, 0xBB), // 7.28
            text_muted: rgb(0x8A, 0x94, 0xA1),     // 5.38
            text_disabled: rgb(0x5E, 0x66, 0x74),  // deliberately sub-AA
            text_oncolor: Color32::WHITE,
            text_link: rgb(0x7F, 0xB4, 0xFF),

            accent: rgb(0x5B, 0x9B, 0xFF),
            accent_fill: rgb(0x27, 0x61, 0xD0),
            accent_fill_hover: rgb(0x2F, 0x6F, 0xE0),
            accent_fill_active: rgb(0x21, 0x58, 0xBE),
            accent_fill_border: rgb(0x4C, 0x86, 0xEA),

            success: Status {
                mark: rgb(0x4F, 0xBF, 0x6B),
                tint_bg: rgb(0x16, 0x30, 0x1F),
                tint_text: rgb(0x7E, 0xE2, 0x9A),
            },
            warning: Status {
                mark: rgb(0xE0, 0xA8, 0x3A),
                tint_bg: rgb(0x33, 0x26, 0x0C),
                tint_text: rgb(0xF2, 0xC5, 0x66),
            },
            danger: Status {
                mark: rgb(0xFF, 0x6B, 0x72),
                tint_bg: rgb(0x3A, 0x17, 0x1A),
                tint_text: rgb(0xFF, 0x9A, 0x9F),
            },
            info: Status {
                mark: rgb(0x69, 0xAE, 0xFF),
                tint_bg: rgb(0x12, 0x23, 0x3C),
                tint_text: rgb(0x9C, 0xC7, 0xFF),
            },
            neutral: Status {
                mark: rgb(0x8B, 0x93, 0xA5),
                tint_bg: rgb(0x23, 0x28, 0x33),
                tint_text: rgb(0xB6, 0xBF, 0xCC),
            },

            danger_fill: rgb(0xA8, 0x20, 0x26),
            danger_fill_hover: rgb(0xC2, 0x31, 0x3A),
            danger_fill_active: rgb(0x8F, 0x1B, 0x21),
            danger_fill_border: rgb(0xE0, 0x45, 0x4E),

            progress_track: rgb(0x2B, 0x31, 0x3C),
            progress_fill: rgb(0x5B, 0x9B, 0xFF),
            progress_fill_warn: rgb(0xE0, 0xA8, 0x3A),
            progress_fill_error: rgb(0xFF, 0x6B, 0x72),
            rail_selected_bg: rgb(0x1E, 0x25, 0x30),
            rail_selected_marker: rgb(0x5B, 0x9B, 0xFF),
            overlay_locked: rgba(0x14, 0x17, 0x1C, 179), // 70%
        }
    }

    /// `DESIGN_SYSTEM.md` §2.2.
    pub const fn light() -> Tokens {
        Tokens {
            dark: false,

            bg_canvas: rgb(0xF4, 0xF6, 0xF9),
            bg_rail: rgb(0xED, 0xF0, 0xF4),
            bg_surface: Color32::WHITE,
            bg_surface_hover: rgb(0xF7, 0xF9, 0xFB),
            bg_raised: rgb(0xF0, 0xF3, 0xF7),
            bg_input: Color32::WHITE,
            bg_code: rgb(0xF2, 0xF4, 0xF7),
            bg_scrim: rgba(0x17, 0x1B, 0x21, 107), // 42%
            bg_selected: rgb(0xD5, 0xE4, 0xFB),

            border_subtle: rgb(0xDD, 0xE2, 0xE9),
            border_strong: rgb(0xC2, 0xCA, 0xD5),
            border_control: rgb(0x7C, 0x86, 0x97), // 3.68 : white
            border_focus: rgb(0x15, 0x5F, 0xCC),   // 5.48 : canvas

            text_primary: rgb(0x17, 0x1B, 0x21),
            text_secondary: rgb(0x59, 0x62, 0x6F),
            text_muted: rgb(0x64, 0x6D, 0x7A),
            text_disabled: rgb(0x98, 0xA1, 0xAE),
            text_oncolor: Color32::WHITE,
            text_link: rgb(0x15, 0x5F, 0xCC),

            accent: rgb(0x15, 0x5F, 0xCC),
            accent_fill: rgb(0x15, 0x5F, 0xCC),
            accent_fill_hover: rgb(0x1A, 0x6B, 0xE0),
            accent_fill_active: rgb(0x0F, 0x4F, 0xAD),
            accent_fill_border: rgb(0x0F, 0x4F, 0xAD),

            success: Status {
                mark: rgb(0x12, 0x79, 0x3B),
                tint_bg: rgb(0xE3, 0xF5, 0xE9),
                tint_text: rgb(0x0D, 0x5C, 0x2C),
            },
            warning: Status {
                mark: rgb(0x8A, 0x5B, 0x00),
                tint_bg: rgb(0xFB, 0xF0, 0xD8),
                tint_text: rgb(0x6E, 0x47, 0x00),
            },
            danger: Status {
                mark: rgb(0xC3, 0x28, 0x2F),
                tint_bg: rgb(0xFC, 0xE6, 0xE7),
                tint_text: rgb(0x9E, 0x1F, 0x26),
            },
            info: Status {
                mark: rgb(0x15, 0x5F, 0xCC),
                tint_bg: rgb(0xE4, 0xEE, 0xFC),
                tint_text: rgb(0x12, 0x53, 0x9E),
            },
            neutral: Status {
                mark: rgb(0x5E, 0x67, 0x74),
                tint_bg: rgb(0xED, 0xF0, 0xF4),
                tint_text: rgb(0x4A, 0x54, 0x62),
            },

            danger_fill: rgb(0xB3, 0x24, 0x2B),
            danger_fill_hover: rgb(0xC8, 0x2C, 0x34),
            danger_fill_active: rgb(0x98, 0x1E, 0x24),
            danger_fill_border: rgb(0x98, 0x1E, 0x24),

            progress_track: rgb(0xDC, 0xE1, 0xE8),
            progress_fill: rgb(0x15, 0x5F, 0xCC),
            progress_fill_warn: rgb(0x8A, 0x5B, 0x00),
            progress_fill_error: rgb(0xC3, 0x28, 0x2F),
            rail_selected_bg: rgb(0xE1, 0xEB, 0xFB),
            rail_selected_marker: rgb(0x15, 0x5F, 0xCC),
            overlay_locked: rgba(0xF4, 0xF6, 0xF9, 184), // 72%
        }
    }

    /// The palette for a `Theme` setting, given what the OS reports.
    pub fn for_theme(theme: Theme, system_is_dark: bool) -> Tokens {
        match theme {
            Theme::Light => Tokens::light(),
            Theme::Dark => Tokens::dark(),
            Theme::System => {
                if system_is_dark {
                    Tokens::dark()
                } else {
                    Tokens::light()
                }
            }
        }
    }

    /// The status palette for a run status. Used by badges, spines and bars, so
    /// that one status can never be two colours in two places.
    pub fn status_for(&self, status: superbackup_core::state::RunStatus) -> Status {
        use superbackup_core::state::RunStatus as R;
        match status {
            R::Succeeded => self.success,
            R::SucceededWithWarnings => self.warning,
            R::Failed => self.danger,
            R::Running | R::Preparing | R::Finalising => self.info,
            R::Queued | R::Cancelled | R::Skipped => self.neutral,
        }
    }

    pub fn status_for_health(&self, health: superbackup_core::state::Health) -> Status {
        use superbackup_core::state::Health as H;
        match health {
            H::Idle => self.success,
            H::Running => self.info,
            H::Attention => self.warning,
            H::Paused => self.neutral,
            H::Failed => self.danger,
        }
    }

    pub fn severity(&self, severity: superbackup_core::state::Severity) -> Status {
        use superbackup_core::state::Severity as S;
        match severity {
            S::Debug => self.neutral,
            S::Info => self.info,
            S::Warning => self.warning,
            S::Error => self.danger,
        }
    }
}

// ---------------------------------------------------------------------------
// Publishing and reading tokens
// ---------------------------------------------------------------------------

fn tokens_id() -> Id {
    Id::new("superbackup::tokens")
}

/// Publish the palette for this frame. Called once, at the top of `update`.
pub fn install(ctx: &Context, tokens: Tokens) {
    ctx.data_mut(|d| d.insert_temp(tokens_id(), tokens));
}

/// The palette for this frame. Falls back to dark rather than panicking, so a
/// component drawn before `install` still renders something sane.
pub fn tokens(ctx: &Context) -> Tokens {
    ctx.data(|d| d.get_temp::<Tokens>(tokens_id())).unwrap_or_else(Tokens::dark)
}

// ---------------------------------------------------------------------------
// Type scale (DESIGN_SYSTEM.md §3.2)
// ---------------------------------------------------------------------------

/// The nine named text styles. Anything not in this list does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Display,
    H1,
    H2,
    H3,
    Body,
    BodyStrong,
    Small,
    SmallStrong,
    Micro,
    Mono,
    MonoStrong,
    MonoSmall,
}

impl Type {
    pub fn font(self) -> FontId {
        match self {
            Type::Display => FontId::new(24.0, FontFamily::Name("bold".into())),
            Type::H1 => FontId::new(20.0, FontFamily::Name("bold".into())),
            Type::H2 => FontId::new(16.0, FontFamily::Name("bold".into())),
            Type::H3 => FontId::new(14.0, FontFamily::Name("bold".into())),
            Type::Body => FontId::proportional(14.0),
            Type::BodyStrong => FontId::new(14.0, FontFamily::Name("medium".into())),
            Type::Small => FontId::proportional(12.0),
            Type::SmallStrong => FontId::new(12.0, FontFamily::Name("medium".into())),
            Type::Micro => FontId::new(11.0, FontFamily::Name("medium".into())),
            Type::Mono => FontId::monospace(13.0),
            Type::MonoStrong => FontId::monospace(13.0),
            Type::MonoSmall => FontId::monospace(12.0),
        }
    }

    /// The row height the design system pairs with the size, used where a
    /// vertical rhythm has to be exact.
    pub fn line_height(self) -> f32 {
        match self {
            Type::Display => 32.0,
            Type::H1 => 28.0,
            Type::H2 | Type::Mono | Type::MonoStrong => 24.0,
            Type::H3 | Type::Body | Type::BodyStrong => 20.0,
            Type::Small | Type::SmallStrong | Type::MonoSmall => 16.0,
            Type::Micro => 14.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Spacing (DESIGN_SYSTEM.md §4)
// ---------------------------------------------------------------------------

pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const S: f32 = 6.0;
    pub const M: f32 = 8.0;
    pub const L: f32 = 12.0;
    pub const XL: f32 = 16.0;
    pub const XXL: f32 = 20.0;
    pub const H3: f32 = 24.0;
    pub const H2: f32 = 32.0;
    pub const H1: f32 = 40.0;
    pub const HUGE: f32 = 48.0;
    pub const MASSIVE: f32 = 64.0;
}

pub mod radius {
    use egui::CornerRadius;
    /// Buttons, inputs, combos, chips.
    pub const CONTROL: CornerRadius = CornerRadius::same(6);
    /// Cards, panels, banners, table containers.
    pub const CARD: CornerRadius = CornerRadius::same(10);
    /// Modals and popup menus.
    pub const MODAL: CornerRadius = CornerRadius::same(14);
    /// Badges and pills.
    pub const BADGE: CornerRadius = CornerRadius::same(5);
}

pub mod size {
    /// Left navigation rail, and its collapsed width below 1000px.
    pub const RAIL: f32 = 208.0;
    pub const RAIL_COLLAPSED: f32 = 64.0;
    pub const HEADER: f32 = 56.0;
    pub const STATUS_STRIP: f32 = 28.0;
    pub const CONTENT_PAD: f32 = 24.0;
    pub const CONTENT_PAD_NARROW: f32 = 20.0;
    /// The width at which the rail collapses and tables shed columns.
    pub const BREAKPOINT: f32 = 1000.0;

    pub const CONTROL_H: f32 = 30.0;
    pub const CONTROL_H_COMPACT: f32 = 26.0;
    pub const CONTROL_H_ONBOARDING: f32 = 36.0;
    pub const RAIL_ITEM_H: f32 = 36.0;
    pub const TABLE_HEADER_H: f32 = 32.0;
    pub const TABLE_ROW_H: f32 = 36.0;
    pub const TABLE_ROW_H_COMPACT: f32 = 28.0;
    pub const JOB_CARD_H: f32 = 96.0;
    pub const BADGE_H: f32 = 20.0;
    pub const CHIP_H: f32 = 24.0;
    pub const BANNER_MIN_H: f32 = 44.0;
    pub const TOAST_W: f32 = 360.0;
    pub const KV_LABEL_W: f32 = 160.0;
    pub const MODAL_SMALL: f32 = 420.0;
    pub const MODAL_MEDIUM: f32 = 560.0;
    pub const MODAL_LARGE: f32 = 760.0;
}

// ---------------------------------------------------------------------------
// Visuals
// ---------------------------------------------------------------------------

/// Derive `egui::Visuals` from the tokens so that stock widgets — sliders,
/// scroll areas, collapsing headers — match without per-widget work (L1).
pub fn visuals(t: &Tokens) -> egui::Visuals {
    let mut v = if t.dark { egui::Visuals::dark() } else { egui::Visuals::light() };

    v.dark_mode = t.dark;
    v.override_text_color = Some(t.text_primary);
    v.panel_fill = t.bg_canvas;
    v.window_fill = t.bg_surface;
    v.window_stroke = Stroke::new(1.0_f32, t.border_strong);
    v.window_corner_radius = radius::MODAL;
    v.extreme_bg_color = t.bg_input;
    v.faint_bg_color = t.bg_surface_hover;
    v.code_bg_color = t.bg_code;
    v.hyperlink_color = t.text_link;
    v.selection.bg_fill = t.bg_selected;
    v.selection.stroke = Stroke::new(1.0_f32, t.text_primary);

    v.widgets.noninteractive.bg_fill = t.bg_surface;
    v.widgets.noninteractive.weak_bg_fill = t.bg_surface;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, t.border_subtle);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, t.text_primary);
    v.widgets.noninteractive.corner_radius = radius::CONTROL;

    v.widgets.inactive.bg_fill = t.bg_raised;
    v.widgets.inactive.weak_bg_fill = t.bg_raised;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, t.border_control);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, t.text_primary);
    v.widgets.inactive.corner_radius = radius::CONTROL;

    v.widgets.hovered.bg_fill = t.bg_surface_hover;
    v.widgets.hovered.weak_bg_fill = t.bg_surface_hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, t.border_control);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, t.text_primary);
    v.widgets.hovered.corner_radius = radius::CONTROL;

    v.widgets.active.bg_fill = t.bg_canvas;
    v.widgets.active.weak_bg_fill = t.bg_canvas;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, t.border_focus);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, t.text_primary);
    v.widgets.active.corner_radius = radius::CONTROL;

    v.widgets.open.bg_fill = t.bg_raised;
    v.widgets.open.weak_bg_fill = t.bg_raised;
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, t.border_control);
    v.widgets.open.fg_stroke = Stroke::new(1.0_f32, t.text_primary);
    v.widgets.open.corner_radius = radius::CONTROL;

    // Focus rings are drawn by this crate, 2px outside the control, so egui's
    // own 1px inset highlight is switched off (DESIGN_SYSTEM.md §2.3).
    v.widgets.hovered.expansion = 0.0;
    v.widgets.active.expansion = 0.0;

    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: if t.dark {
            Color32::from_black_alpha(115)
        } else {
            rgba(0x17, 0x1B, 0x21, 36)
        },
    };
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 40,
        spread: 0,
        color: if t.dark {
            Color32::from_black_alpha(140)
        } else {
            rgba(0x17, 0x1B, 0x21, 51)
        },
    };

    v.striped = false;
    v.indent_has_left_vline = false;
    v.slider_trailing_fill = true;
    v
}

/// Spacing, hit targets and the text-style table (`DESIGN_SYSTEM.md` §4.2).
pub fn style(t: &Tokens) -> egui::Style {
    let mut s = egui::Style::default();
    s.visuals = visuals(t);

    s.spacing.item_spacing = egui::vec2(8.0, 6.0);
    s.spacing.button_padding = egui::vec2(12.0, 7.0);
    s.spacing.menu_margin = Margin::same(6);
    s.spacing.indent = 20.0;
    s.spacing.interact_size = egui::vec2(44.0, 30.0);
    s.spacing.slider_width = 180.0;
    s.spacing.combo_width = 220.0;
    s.spacing.text_edit_width = 280.0;
    s.spacing.icon_width = 16.0;
    s.spacing.icon_width_inner = 10.0;
    s.spacing.scroll.bar_width = 10.0;
    s.spacing.scroll.bar_inner_margin = 4.0;
    s.spacing.scroll.floating = false;
    s.spacing.menu_spacing = 4.0;

    s.text_styles = [
        (TextStyle::Small, Type::Small.font()),
        (TextStyle::Body, Type::Body.font()),
        (TextStyle::Button, Type::BodyStrong.font()),
        (TextStyle::Heading, Type::H2.font()),
        (TextStyle::Monospace, Type::MonoSmall.font()),
    ]
    .into();

    s.interaction.selectable_labels = true;
    s.wrap_mode = Some(egui::TextWrapMode::Extend);
    s
}

// ---------------------------------------------------------------------------
// Fonts (DESIGN_SYSTEM.md §3.1 and L12)
// ---------------------------------------------------------------------------

/// Load the UI and code faces.
///
/// The design system asks for Inter and JetBrains Mono. Neither is vendored in
/// this repository, so the loader walks a per-platform candidate list — Inter
/// and JetBrains Mono first, then the platform's own UI and code faces, then
/// egui's built-in fonts. Whatever is found is registered under `regular`,
/// `medium` and `bold` families so the type scale's three weights resolve, and
/// one CJK face is appended to both families so non-Latin paths render as text
/// rather than as boxes.
///
/// A missing font is logged once and never blocks a backup.
pub fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();

    let ui_regular = load_first(&candidates_ui_regular());
    let ui_medium = load_first(&candidates_ui_medium());
    let ui_bold = load_first(&candidates_ui_bold());
    let mono = load_first(&candidates_mono());
    let cjk = load_first(&candidates_cjk());

    if let Some((name, data)) = ui_regular {
        insert_font(&mut fonts, "sb-ui", name, data);
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "sb-ui".to_owned());
    }
    if let Some((name, data)) = mono {
        insert_font(&mut fonts, "sb-mono", name, data);
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "sb-mono".to_owned());
    }

    // The two synthetic weight families. When a real medium or bold face is
    // missing they fall back to the regular one, which is legible but flat —
    // logged so a packager knows to ship the weights.
    let mut medium_family = Vec::new();
    if let Some((name, data)) = ui_medium {
        insert_font(&mut fonts, "sb-ui-medium", name, data);
        medium_family.push("sb-ui-medium".to_owned());
    }
    let mut bold_family = Vec::new();
    if let Some((name, data)) = ui_bold {
        insert_font(&mut fonts, "sb-ui-bold", name, data);
        bold_family.push("sb-ui-bold".to_owned());
    }
    for family in [&mut medium_family, &mut bold_family] {
        family.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    }
    fonts.families.insert(FontFamily::Name("medium".into()), medium_family);
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);

    if let Some((name, data)) = cjk {
        insert_font(&mut fonts, "sb-cjk", name, data);
        for family in [
            FontFamily::Proportional,
            FontFamily::Monospace,
            FontFamily::Name("medium".into()),
            FontFamily::Name("bold".into()),
        ] {
            fonts.families.entry(family).or_default().push("sb-cjk".to_owned());
        }
    } else {
        tracing::warn!(
            "no CJK font was found; paths containing CJK characters will render as boxes"
        );
    }

    fonts
}

fn insert_font(fonts: &mut egui::FontDefinitions, key: &str, path: String, data: Vec<u8>) {
    tracing::debug!(font = %path, "loaded font");
    fonts
        .font_data
        .insert(key.to_owned(), std::sync::Arc::new(egui::FontData::from_owned(data)));
}

fn load_first(candidates: &[String]) -> Option<(String, Vec<u8>)> {
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            if !bytes.is_empty() {
                return Some((path.clone(), bytes));
            }
        }
    }
    None
}

fn font_dirs() -> Vec<String> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        if let Ok(win) = std::env::var("SystemRoot") {
            dirs.push(format!("{win}\\Fonts"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(format!("{local}\\Microsoft\\Windows\\Fonts"));
        }
    }
    if cfg!(target_os = "macos") {
        dirs.push("/System/Library/Fonts".into());
        dirs.push("/Library/Fonts".into());
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(format!("{home}/Library/Fonts"));
        }
    }
    if cfg!(target_os = "linux") {
        dirs.push("/usr/share/fonts/truetype".into());
        dirs.push("/usr/share/fonts/TTF".into());
        dirs.push("/usr/share/fonts/opentype".into());
        dirs.push("/usr/local/share/fonts".into());
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(format!("{home}/.local/share/fonts"));
        }
    }
    dirs
}

fn expand(names: &[&str]) -> Vec<String> {
    let dirs = font_dirs();
    let mut out = Vec::new();
    for name in names {
        // A bare name may already be an absolute path from a package layout.
        if name.contains('/') || name.contains('\\') {
            out.push((*name).to_string());
            continue;
        }
        for dir in &dirs {
            out.push(format!("{dir}/{name}"));
        }
    }
    out
}

fn candidates_ui_regular() -> Vec<String> {
    expand(&[
        "Inter-Regular.ttf",
        "Inter.ttc",
        "InterVariable.ttf",
        "segoeui.ttf",
        "SFNSDisplay.ttf",
        "SFNS.ttf",
        "dejavu/DejaVuSans.ttf",
        "DejaVuSans.ttf",
        "liberation/LiberationSans-Regular.ttf",
        "noto/NotoSans-Regular.ttf",
    ])
}

fn candidates_ui_medium() -> Vec<String> {
    expand(&[
        "Inter-Medium.ttf",
        "segoeuisl.ttf",
        "seguisb.ttf",
        "dejavu/DejaVuSans.ttf",
        "DejaVuSans.ttf",
        "noto/NotoSans-Medium.ttf",
    ])
}

fn candidates_ui_bold() -> Vec<String> {
    expand(&[
        "Inter-SemiBold.ttf",
        "Inter-Bold.ttf",
        "seguisb.ttf",
        "segoeuib.ttf",
        "dejavu/DejaVuSans-Bold.ttf",
        "DejaVuSans-Bold.ttf",
        "liberation/LiberationSans-Bold.ttf",
        "noto/NotoSans-Bold.ttf",
    ])
}

fn candidates_mono() -> Vec<String> {
    expand(&[
        "JetBrainsMono-Regular.ttf",
        "consola.ttf",
        "SFNSMono.ttf",
        "Menlo.ttc",
        "dejavu/DejaVuSansMono.ttf",
        "DejaVuSansMono.ttf",
        "liberation/LiberationMono-Regular.ttf",
    ])
}

fn candidates_cjk() -> Vec<String> {
    expand(&[
        "msyh.ttc",
        "meiryo.ttc",
        "PingFang.ttc",
        "NotoSansCJK-Regular.ttc",
        "opentype/noto/NotoSansCJK-Regular.ttc",
    ])
}

// ---------------------------------------------------------------------------
// Small colour utilities
// ---------------------------------------------------------------------------

/// Blend two colours. Used for hover lerps and for tints drawn at a fraction of
/// their alpha, so that no component invents a colour of its own.
pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        f(a.r(), b.r()),
        f(a.g(), b.g()),
        f(a.b(), b.b()),
        f(a.a(), b.a()),
    )
}

/// The same colour at a fraction of its opacity.
pub fn alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Radius + 2, for a focus ring drawn outside a control.
pub fn ring_radius(r: CornerRadius) -> CornerRadius {
    CornerRadius {
        nw: r.nw.saturating_add(2),
        ne: r.ne.saturating_add(2),
        sw: r.sw.saturating_add(2),
        se: r.se.saturating_add(2),
    }
}

// ---------------------------------------------------------------------------
// Contrast, kept honest by a test rather than by a comment
// ---------------------------------------------------------------------------

fn relative_luminance(c: Color32) -> f64 {
    let channel = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// WCAG 2.1 contrast ratio between two opaque colours.
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The design system states measured ratios. If an edit here drops one
    /// below its threshold, this fails rather than shipping unreadable text.
    #[test]
    fn text_meets_aa_on_every_surface_it_is_used_on() {
        for t in [Tokens::dark(), Tokens::light()] {
            for bg in [t.bg_canvas, t.bg_surface, t.bg_raised] {
                assert!(
                    contrast(t.text_primary, bg) >= 4.5,
                    "primary text on {bg:?} is {:.2}",
                    contrast(t.text_primary, bg)
                );
                assert!(
                    contrast(t.text_secondary, bg) >= 4.5,
                    "secondary text on {bg:?} is {:.2}",
                    contrast(t.text_secondary, bg)
                );
                assert!(
                    contrast(t.text_muted, bg) >= 4.5,
                    "muted text on {bg:?} is {:.2}",
                    contrast(t.text_muted, bg)
                );
            }
        }
    }

    #[test]
    fn control_boundaries_meet_the_three_to_one_rule() {
        for t in [Tokens::dark(), Tokens::light()] {
            for bg in [t.bg_surface, t.bg_raised, t.bg_input] {
                assert!(
                    contrast(t.border_control, bg) >= 3.0,
                    "control border on {bg:?} is {:.2}",
                    contrast(t.border_control, bg)
                );
            }
            assert!(contrast(t.border_focus, t.bg_canvas) >= 3.0);
        }
    }

    #[test]
    fn status_badges_are_readable_on_their_own_tint() {
        for t in [Tokens::dark(), Tokens::light()] {
            for s in [t.success, t.warning, t.danger, t.info, t.neutral] {
                assert!(
                    contrast(s.tint_text, s.tint_bg) >= 4.5,
                    "badge text {:?} on {:?} is {:.2}",
                    s.tint_text,
                    s.tint_bg,
                    contrast(s.tint_text, s.tint_bg)
                );
                assert!(
                    contrast(s.mark, t.bg_surface) >= 3.0,
                    "status mark {:?} is {:.2}",
                    s.mark,
                    contrast(s.mark, t.bg_surface)
                );
            }
        }
    }

    #[test]
    fn white_on_filled_buttons_meets_aa() {
        for t in [Tokens::dark(), Tokens::light()] {
            for fill in [
                t.accent_fill,
                t.accent_fill_hover,
                t.accent_fill_active,
                t.danger_fill,
                t.danger_fill_hover,
                t.danger_fill_active,
            ] {
                assert!(
                    contrast(Color32::WHITE, fill) >= 4.5,
                    "white on {fill:?} is {:.2}",
                    contrast(Color32::WHITE, fill)
                );
            }
        }
    }

    #[test]
    fn disabled_text_is_deliberately_below_threshold() {
        // Disabled controls are exempt from WCAG and must read as disabled.
        for t in [Tokens::dark(), Tokens::light()] {
            assert!(contrast(t.text_disabled, t.bg_raised) < 4.5);
        }
    }

    #[test]
    fn system_theme_follows_the_os() {
        assert!(Tokens::for_theme(Theme::System, true).dark);
        assert!(!Tokens::for_theme(Theme::System, false).dark);
        assert!(Tokens::for_theme(Theme::Dark, false).dark);
        assert!(!Tokens::for_theme(Theme::Light, true).dark);
    }
}
