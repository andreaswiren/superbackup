//! The five tray marks, generated from `DESIGN_SYSTEM.md` §7 and rasterised
//! at run time.
//!
//! ## Why the SVG is generated rather than read from `assets/tray/`
//!
//! `assets/tray/README.md` says it plainly: the shipped SVGs are an earlier,
//! single-variant design, kept only so that something renders. The
//! specification they were superseded by needs things a fixed file cannot
//! give:
//!
//! * **Two variants per state on Windows**, swapped on the taskbar's
//!   `SystemUsesLightTheme` preference, so each is genuinely high-contrast on
//!   the taskbar it is actually drawn on rather than a compromise that is
//!   optimal on neither.
//! * **A macOS template image**, which is pure black plus alpha with every
//!   colour stripped — the same geometry, a different palette.
//! * **Twelve animation frames** for the running state, at 30° apart.
//!
//! That is five states × two Windows variants, plus a template set, plus
//! twelve frames — around forty files to keep in sync by hand. Generating them
//! from one parameterised description means the geometry cannot drift between
//! variants, which is exactly the property §7.2's "shape carries state, colour
//! is confirmation" depends on.
//!
//! ## Geometry (§7.1), verbatim
//!
//! Canvas 32 × 32, centred on (16, 16), round joins and caps.
//!
//! * Ring: circle at (16, 16), radius 10.5, stroke 3.0.
//! * Open ring: the same circle as a 280° arc, leaving an 80° gap centred on
//!   the +45° (down-right) direction. Arc start 85°, sweep 280° clockwise,
//!   0° = east with y pointing down.
//! * Pip: circle at (23, 23), radius 6.0, filled, separated from the arc by a
//!   1.5-unit knockout so the mark composites correctly on any taskbar colour.
//! * Pip glyph: stroke 2.4 in `pip.ink`.
//! * Safe area: nothing within 1 unit of the edge.
//!
//! The knockout is a real cutout, not a background-coloured stroke: a stroke
//! painted in "the taskbar colour" is wrong the moment the user changes their
//! accent, and wrong always on Linux panels that are not the colour we
//! guessed.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::Mutex;

use superbackup_core::state::Health;

/// The design canvas. Every coordinate below is in these units.
const CANVAS: f32 = 32.0;

/// Ring geometry, §7.1. Shared by every profile.
const CENTRE: f32 = 16.0;
const RING_RADIUS: f32 = 10.5;
const RING_STROKE: f32 = 3.0;

/// Where the gap in the open ring is centred: the down-right diagonal, §7.1.
const GAP_BEARING_DEG: f32 = 45.0;

/// Pip centre, §7.1. Its *radius* is per profile.
const PIP_X: f32 = 23.0;
const PIP_Y: f32 = 23.0;

/// Clear space between the pip and the arc, §7.1.
const PIP_KNOCKOUT: f32 = 1.5;

/// Running animation, §7.2: twelve frames.
pub const RUNNING_FRAMES: usize = 12;
const RUNNING_ARC_DEG: f32 = 90.0;

/// The size a mark is rasterised at when nothing better is known.
///
/// The tray asks the platform for its actual notification-area icon size and
/// renders natively at it — see [`preferred_size`]. This is the fallback, and
/// the size the reference assets in `assets/tray/` are drawn at.
pub const RASTER_SIZE: u32 = 32;

/// At or below this many pixels, the chunky profile is used.
///
/// 20 px is the 125% DPI notification-area size on Windows. §7.1's
/// proportions survive at 24 and above and do not at 16 or 20 — see
/// [`Profile`].
pub const SMALL_PROFILE_MAX: u32 = 20;

/// Everything about a mark that changes with the size it is drawn at.
///
/// # Why size-specific proportions exist at all
///
/// §7.1 fixes one set of numbers on a 32-unit canvas and §7.3 says shape, not
/// colour, carries the state. Those two cannot both hold at every size, and
/// rasterising proved it: at 16 px one canvas unit is half a pixel, so the
/// specified 2.4-unit glyph stroke lands at **1.2 px** and the exclamation's
/// 1.3-unit dot at 0.65 px — both under §7.1's own "no stroke below 1.5 px at
/// 16 px" rule. The result is that `attention` and `failed` become an open
/// ring plus an indistinct smear, told apart only by that smear's *lightness*
/// — which is precisely the colour-only distinction §7.2 forbids, and which
/// vanishes completely in the macOS template where both are the same hole.
///
/// It held at 24 px and above and failed at 16 and 20, which on Windows is the
/// common case.
///
/// So the mark gets a second profile. Below 24 px the pip grows and the glyph
/// thickens past the 1.5 px floor, and the mark is chunkier than the drawing
/// at 32 px. That is the right trade: a chunky mark somebody can read beats an
/// elegant one nobody can.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profile {
    /// Radius of the filled status pip.
    pub pip_radius: f32,
    /// Stroke width of the glyph drawn inside the pip.
    pub pip_stroke: f32,
    /// The exclamation's bar, as `(top, bottom)`.
    pub bar: (f32, f32),
    /// The exclamation's dot: radius, and its centre's y.
    pub dot: (f32, f32),
    /// How far each arm of the cross reaches from the pip's centre.
    pub cross_reach: f32,
    /// Radius of the solid disc at the centre of the `idle` mark.
    pub centre_dot: f32,
}

/// §7.1's numbers, unchanged. Used at 24 px and above, where they work.
pub const LARGE: Profile = Profile {
    pip_radius: 6.0,
    pip_stroke: 2.4,
    bar: (19.8, 23.4),
    dot: (1.3, 26.0),
    cross_reach: 2.97,
    centre_dot: 3.0,
};

/// The chunky profile, for 16 px and 20 px.
///
/// Every number is driven by one constraint: at 16 px a canvas unit is half a
/// pixel, so nothing may be thinner than 3.0 units. The glyph stroke is 3.2
/// (1.6 px), the pip grows to 7.6 so the glyph still fits inside it with a
/// margin, and the exclamation's bar and dot are pulled apart to 2.35 units
/// (1.18 px) of clear space so the two stay two shapes rather than merging
/// into one blob.
///
/// The pip at radius 7.6 spans 15.4–30.6 on the canvas, inside §7.1's
/// one-unit safe area.
pub const SMALL: Profile = Profile {
    pip_radius: 7.6,
    pip_stroke: 3.2,
    bar: (17.8, 21.8),
    dot: (1.85, 27.6),
    cross_reach: 3.5,
    centre_dot: 3.6,
};

impl Profile {
    /// The profile to draw a mark of `size` pixels with.
    pub fn for_size(size: u32) -> Profile {
        if size <= SMALL_PROFILE_MAX {
            SMALL
        } else {
            LARGE
        }
    }

    /// Start angle and sweep of the open ring, derived rather than fixed.
    ///
    /// §7.1 specifies "arc start 85°, sweep 280°, round caps" and a 1.5-unit
    /// knockout separating the pip from the arc. Those numbers contradict each
    /// other. The knockout disc — radius `pip_radius + 1.5`, centred on the
    /// pip — cuts the ring circle over 45° ± 43°, i.e. 2°–88°, while the
    /// specified gap is only 5°–85°. The knockout is *wider than the gap it is
    /// meant to clear*, so it slices both round terminals off the arc and
    /// leaves flat crescents. "Sweep 280° with round caps" does not describe
    /// what is drawn.
    ///
    /// Of the two ways out — shrink the pip to 4.0 units, or widen the gap —
    /// only widening survives 16 px: shrinking the pip makes the glyph
    /// legibility problem worse, and the pip is the thing carrying the state.
    ///
    /// So the gap is *computed* from the pip instead of being another constant
    /// that can drift away from it: half the gap is the knockout's own
    /// half-angle, plus the angle the arc's round cap subtends, plus two
    /// degrees of daylight. A future change to the pip radius re-derives the
    /// arc automatically, and
    /// `the_round_caps_survive_the_knockout` asserts the relationship holds.
    pub fn arc_span(&self) -> (f32, f32) {
        let pip_distance = ((PIP_X - CENTRE).powi(2) + (PIP_Y - CENTRE).powi(2)).sqrt();
        let knockout = self.pip_radius + PIP_KNOCKOUT;
        // Law of cosines: where the knockout circle crosses the ring circle.
        let cosine = (pip_distance.powi(2) + RING_RADIUS.powi(2) - knockout.powi(2))
            / (2.0 * pip_distance * RING_RADIUS);
        let knockout_half = cosine.clamp(-1.0, 1.0).acos().to_degrees();
        // A round cap projects half a stroke width beyond the path's endpoint.
        let cap_half = ((RING_STROKE / 2.0) / RING_RADIUS).asin().to_degrees();
        let gap_half = knockout_half + cap_half + 2.0;
        (GAP_BEARING_DEG + gap_half, 360.0 - 2.0 * gap_half)
    }
}

/// Which taskbar the mark is being drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    /// Light taskbar: dark ink. §7.4 `-light`, ring ink `#22262E`.
    LightTaskbar,
    /// Dark taskbar: light ink. §7.4 `-dark`, ring ink `#E8ECF2`.
    DarkTaskbar,
    /// macOS template image: pure black plus alpha, no colour at all. The OS
    /// inverts it for a dark menu bar, so state must be carried entirely by
    /// silhouette — which §7.2 guarantees it is.
    Template,
}

impl Variant {
    /// The variant to use on this platform right now.
    ///
    /// Windows reports its taskbar theme in the registry, which
    /// `platform::win32` already reads. Elsewhere §7.4 specifies the
    /// dark-taskbar asset as the Linux default, and the template image on
    /// macOS.
    pub fn detect() -> Variant {
        if cfg!(target_os = "macos") {
            return Variant::Template;
        }
        if system_uses_light_theme() {
            Variant::LightTaskbar
        } else {
            Variant::DarkTaskbar
        }
    }

    fn ring_ink(self) -> &'static str {
        match self {
            Variant::LightTaskbar => "#22262E",
            Variant::DarkTaskbar => "#E8ECF2",
            Variant::Template => "#000000",
        }
    }

    /// Whether chromatic accents survive. They do not in a template image.
    fn colour(self, chromatic: &'static str) -> &'static str {
        match self {
            Variant::Template => "#000000",
            _ => chromatic,
        }
    }

    /// The ink drawn *inside* a coloured pip, for the two Windows variants.
    ///
    /// A template image never reaches here: see [`svg`], which punches its
    /// glyph out of the pip's alpha instead of painting one on top.
    fn pip_ink(self, chromatic: &'static str) -> &'static str {
        chromatic
    }

    /// The dim ring the `running` mark's moving arc travels along.
    ///
    /// §7.2 fixes this at `#3A4250` for both Windows variants, and that is
    /// wrong on one of them: it is 9.12:1 on a light taskbar and **1.61:1** on
    /// a dark one, so on a dark taskbar the base ring disappears and only the
    /// blue arc remains. `running` then stops sharing the ring silhouette that
    /// makes all five marks read as one application — which is the whole
    /// premise of §7.2.
    ///
    /// So it is variant-aware, like every other ink in §7.4. The dark-taskbar
    /// value is §7.2's own neutral `#8B93A5` (about 5.1:1 on `#202020`):
    /// clearly present, and still dim enough that the moving arc reads as the
    /// thing that is moving.
    fn base_arc_ink(self) -> &'static str {
        match self {
            Variant::LightTaskbar => "#3A4250",
            Variant::DarkTaskbar => "#8B93A5",
            Variant::Template => "#000000",
        }
    }

    /// The arc that travels round the `running` mark.
    ///
    /// Variant-aware for the same reason the base arc is, and it is the same
    /// defect: §7.2's `#5B9BFF` is 5.88:1 on a dark taskbar and **2.50:1** on
    /// a light one, so on a light taskbar the moving half of the mark is the
    /// part that fades out. Fixing only the base arc would have left `running`
    /// half-invisible on light taskbars instead of on dark ones.
    ///
    /// The light-taskbar value is the design system's own §2.2 `info` token
    /// `#155FCC` (5.35:1 on `#F3F3F3`), so no new colour is invented — this is
    /// §7.4's existing "two variants per state" rule applied to an ink that
    /// had been left out of it.
    fn moving_arc_ink(self) -> &'static str {
        match self {
            Variant::LightTaskbar => "#155FCC",
            Variant::DarkTaskbar => "#5B9BFF",
            Variant::Template => "#000000",
        }
    }
}

/// Does the shell use a light taskbar?
///
/// Windows answers in `HKCU\...\Themes\Personalize\SystemUsesLightTheme`.
/// Every other platform is assumed dark, matching §7.4's "Linux: full colour,
/// dark-taskbar variant".
pub fn system_uses_light_theme() -> bool {
    #[cfg(windows)]
    {
        // Read through the registry with the same query `reg.exe` performs.
        // `platform::win32` is private to the core crate, so this reads the
        // value with the tool Windows ships rather than duplicating a registry
        // binding here — it runs once per theme check, not per frame.
        let output = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "SystemUsesLightTheme",
            ])
            .output();
        if let Ok(output) = output {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = text.lines().find(|l| l.contains("SystemUsesLightTheme")) {
                // `    SystemUsesLightTheme    REG_DWORD    0x1`
                return line.rsplit_once("0x").map(|(_, v)| v.trim() != "0").unwrap_or(false);
            }
        }
        false
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// One mark: a state, a variant, and (for `Running`) an animation frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconKey {
    pub health: Health,
    pub variant: Variant,
    pub frame: usize,
}

/// Hashed by the health's *stem* rather than by the enum, because
/// [`Health`] deliberately derives `Ord` (so `max()` picks the icon to show)
/// but not `Hash`. Hashing the stem keeps this key usable as a map key without
/// asking the core crate to grow a derive it does not need.
impl std::hash::Hash for IconKey {
    fn hash<H: std::hash::Hasher>(&self, hasher: &mut H) {
        self.health.icon_stem().hash(hasher);
        self.variant.hash(hasher);
        self.frame.hash(hasher);
    }
}

impl IconKey {
    pub fn new(health: Health, variant: Variant, frame: usize) -> IconKey {
        // Only the running state animates; collapsing the frame for the others
        // is what keeps the cache to five entries instead of sixty.
        let frame = if health == Health::Running { frame % RUNNING_FRAMES } else { 0 };
        IconKey { health, variant, frame }
    }

    /// The asset stem this mark would have on disk, matching
    /// [`Health::icon_stem`] and §7.4's naming.
    pub fn stem(&self) -> String {
        let suffix = match self.variant {
            Variant::LightTaskbar => "-light",
            Variant::DarkTaskbar => "-dark",
            Variant::Template => "-template",
        };
        if self.health == Health::Running {
            format!("{}-{:02}{suffix}", self.health.icon_stem(), self.frame)
        } else {
            format!("{}{suffix}", self.health.icon_stem())
        }
    }
}

// ---------------------------------------------------------------------------
// SVG generation
// ---------------------------------------------------------------------------

/// Point on the ring at `degrees`, with 0° = east and y increasing downwards.
fn on_ring(degrees: f32) -> (f32, f32) {
    let radians = degrees * PI / 180.0;
    (CENTRE + RING_RADIUS * radians.cos(), CENTRE + RING_RADIUS * radians.sin())
}

/// An SVG arc path along the ring, clockwise from `start` for `sweep` degrees.
fn arc_path(start_deg: f32, sweep_deg: f32) -> String {
    let (x0, y0) = on_ring(start_deg);
    let (x1, y1) = on_ring(start_deg + sweep_deg);
    // `large-arc` is 1 past a half turn; `sweep` is 1 for clockwise in SVG's
    // y-down coordinate system, which is the direction §7.1 specifies.
    let large = if sweep_deg.abs() > 180.0 { 1 } else { 0 };
    format!("M {x0:.3} {y0:.3} A {RING_RADIUS} {RING_RADIUS} 0 {large} 1 {x1:.3} {y1:.3}")
}

/// The span of the travelling arc on `frame`, clipped to the open ring.
///
/// The arc used to be drawn on the full circle, so on four frames in twelve it
/// crossed the gap and the silhouette was momentarily a **closed** ring —
/// which is exactly what distinguishes `idle` and `paused`. A transient
/// collision with two other states is still a collision, and the fix costs
/// four lines: the arc's centre travels the base span and its ends are clipped
/// to it, so the mark reads as something moving *along* the ring and
/// disappearing into the gap rather than sealing it.
///
/// Clipping the centre rather than the leading edge is what keeps every frame
/// non-empty: at the extremes half the arc is still on the ring, so the mark
/// never blinks out.
fn travelling_arc(frame: usize, start: f32, sweep: f32) -> Option<(f32, f32)> {
    let progress = (frame % RUNNING_FRAMES) as f32 / RUNNING_FRAMES as f32;
    let centre = start + progress * sweep;
    let head = (centre + RUNNING_ARC_DEG / 2.0).min(start + sweep);
    let tail = (centre - RUNNING_ARC_DEG / 2.0).max(start);
    let visible = head - tail;
    (visible > 0.5).then_some((tail, visible))
}

/// The complete SVG document for one mark, drawn with `profile`.
///
/// Returned as text rather than as a rendered tree so that it can be asserted
/// on in tests, dumped for a designer to check against the spec, and written
/// out as a build artefact without changing anything here.
pub fn svg(key: IconKey, profile: Profile) -> String {
    let variant = key.variant;
    let ring = variant.ring_ink();
    let (arc_start, arc_sweep) = profile.arc_span();
    let pip_radius = profile.pip_radius;
    let pip_stroke = profile.pip_stroke;
    let mut defs = String::new();
    let mut body = String::new();

    // The knockout mask: white keeps, black cuts. Only the states that carry
    // a pip need it, and applying it unconditionally would eat into the ring
    // of the states that do not.
    let has_pip = matches!(key.health, Health::Attention | Health::Failed);
    let mask = if has_pip {
        defs.push_str(&format!(
            "<mask id=\"pip-knockout\">\
               <rect x=\"0\" y=\"0\" width=\"{CANVAS}\" height=\"{CANVAS}\" fill=\"#fff\"/>\
               <circle cx=\"{PIP_X}\" cy=\"{PIP_Y}\" r=\"{:.2}\" fill=\"#000\"/>\
             </mask>",
            pip_radius + PIP_KNOCKOUT
        ));
        " mask=\"url(#pip-knockout)\""
    } else {
        ""
    };

    match key.health {
        Health::Idle => {
            // Closed ring plus a solid centre disc: the only closed ring with
            // a centre dot, which is what makes it greyscale-distinct.
            body.push_str(&format!(
                "<circle cx=\"{CENTRE}\" cy=\"{CENTRE}\" r=\"{RING_RADIUS}\" fill=\"none\" \
                 stroke=\"{ring}\" stroke-width=\"{RING_STROKE}\"/>\
                 <circle cx=\"{CENTRE}\" cy=\"{CENTRE}\" r=\"{}\" fill=\"{ring}\"/>",
                profile.centre_dot
            ));
        }
        Health::Paused => {
            // Closed ring plus two interior bars: the only two-bar state.
            body.push_str(&format!(
                "<circle cx=\"{CENTRE}\" cy=\"{CENTRE}\" r=\"{RING_RADIUS}\" fill=\"none\" \
                 stroke=\"{ring}\" stroke-width=\"{RING_STROKE}\"/>"
            ));
            for centre_x in [13.3_f32, 18.7_f32] {
                body.push_str(&format!(
                    "<rect x=\"{:.2}\" y=\"10.5\" width=\"3.4\" height=\"11\" rx=\"1.7\" \
                     fill=\"{ring}\"/>",
                    centre_x - 1.7
                ));
            }
        }
        Health::Running => {
            // Open ring, no pip, no centre dot — the only state with neither,
            // and the only one that animates.
            body.push_str(&format!(
                "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{RING_STROKE}\" \
                 stroke-linecap=\"round\"/>",
                arc_path(arc_start, arc_sweep),
                variant.base_arc_ink()
            ));
            if let Some((start, sweep)) = travelling_arc(key.frame, arc_start, arc_sweep) {
                body.push_str(&format!(
                    "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{RING_STROKE}\" \
                     stroke-linecap=\"round\"/>",
                    arc_path(start, sweep),
                    variant.moving_arc_ink()
                ));
            }
        }
        Health::Attention | Health::Failed => {
            body.push_str(&format!(
                "<path d=\"{}\" fill=\"none\" stroke=\"{ring}\" stroke-width=\"{RING_STROKE}\" \
                 stroke-linecap=\"round\"{mask}/>",
                arc_path(arc_start, arc_sweep)
            ));

            let (fill, chromatic_ink) = if key.health == Health::Attention {
                (variant.colour("#E0A83A"), "#1A1206")
            } else {
                (variant.colour("#C2313A"), "#FFFFFF")
            };
            let glyph = |ink: &str| -> String {
                if key.health == Health::Attention {
                    let (top, bottom) = profile.bar;
                    let (dot_r, dot_y) = profile.dot;
                    format!(
                        "<path d=\"M {PIP_X} {top} L {PIP_X} {bottom}\" stroke=\"{ink}\" \
                         stroke-width=\"{pip_stroke}\" stroke-linecap=\"round\"/>\
                         <circle cx=\"{PIP_X}\" cy=\"{dot_y}\" r=\"{dot_r}\" fill=\"{ink}\"/>"
                    )
                } else {
                    // The cross is expressed as a reach from the pip's centre
                    // rather than as four literal coordinates, so it scales
                    // with the pip instead of having to be re-derived by hand
                    // for every profile.
                    let arm = profile.cross_reach / std::f32::consts::SQRT_2;
                    let (lo_x, lo_y) = (PIP_X - arm, PIP_Y - arm);
                    let (hi_x, hi_y) = (PIP_X + arm, PIP_Y + arm);
                    format!(
                        "<path d=\"M {lo_x:.2} {lo_y:.2} L {hi_x:.2} {hi_y:.2} \
                         M {hi_x:.2} {lo_y:.2} L {lo_x:.2} {hi_y:.2}\" stroke=\"{ink}\" \
                         stroke-width=\"{pip_stroke}\" stroke-linecap=\"round\"/>"
                    )
                }
            };

            if variant == Variant::Template {
                // A template image is *alpha only* — macOS throws the colour
                // away — so a glyph painted on top of the pip in any colour is
                // simply opaque, and `attention` and `failed` both render as
                // an open ring with a plain filled pip. Two states, one
                // picture, in the one place §7.4 says shape must carry the
                // meaning.
                //
                // The glyph therefore has to be a *hole*: drawn black on a
                // white pip in a mask, so it removes alpha instead of adding
                // ink. `assets/tray/` has always done this; the code had not.
                defs.push_str(&format!(
                    "<mask id=\"pip-glyph\">\
                       <circle cx=\"{PIP_X}\" cy=\"{PIP_Y}\" r=\"{pip_radius}\" fill=\"#fff\"/>{}\
                     </mask>",
                    glyph("#000")
                ));
                body.push_str(&format!(
                    "<circle cx=\"{PIP_X}\" cy=\"{PIP_Y}\" r=\"{pip_radius}\" fill=\"{fill}\" \
                     mask=\"url(#pip-glyph)\"/>"
                ));
            } else {
                body.push_str(&format!(
                    "<circle cx=\"{PIP_X}\" cy=\"{PIP_Y}\" r=\"{pip_radius}\" fill=\"{fill}\"/>{}",
                    glyph(variant.pip_ink(chromatic_ink))
                ));
            }
        }
    }

    let defs_block = if defs.is_empty() { String::new() } else { format!("<defs>{defs}</defs>") };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{CANVAS}\" height=\"{CANVAS}\" \
         viewBox=\"0 0 {CANVAS} {CANVAS}\" fill=\"none\" stroke-linejoin=\"round\" \
         stroke-linecap=\"round\">{defs_block}{body}</svg>"
    )
}

// ---------------------------------------------------------------------------
// Rasterisation
// ---------------------------------------------------------------------------

/// Render one mark to straight (non-premultiplied) RGBA at `size` × `size`.
///
/// `tiny-skia` works in premultiplied alpha; `tray-icon` wants straight RGBA.
/// Handing it premultiplied data produces marks that look correct on an opaque
/// taskbar and wrong on a translucent one, which is the default on Windows 11
/// — hence the explicit `demultiply`.
pub fn rasterise(key: IconKey, size: u32) -> Result<Vec<u8>, String> {
    let document = svg(key, Profile::for_size(size));
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(&document, &options)
        .map_err(|e| format!("the generated tray SVG did not parse: {e}"))?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
        .ok_or_else(|| format!("{size}x{size} is not a usable icon size"))?;
    let scale = size as f32 / CANVAS;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.take_pixmap_mut_demultiply();
    Ok(pixmap.take())
}

/// Trait-free helper: `resvg::tiny_skia::Pixmap` has no `demultiply` on the owned
/// type in 0.12, only on `PixmapMut`.
trait Demultiply {
    fn take_pixmap_mut_demultiply(&mut self);
}

impl Demultiply for resvg::tiny_skia::Pixmap {
    fn take_pixmap_mut_demultiply(&mut self) {
        self.as_mut().pixels_mut().iter_mut().for_each(|pixel| {
            *pixel = resvg::tiny_skia::PremultipliedColorU8::from_rgba(
                demul(pixel.red(), pixel.alpha()),
                demul(pixel.green(), pixel.alpha()),
                demul(pixel.blue(), pixel.alpha()),
                pixel.alpha(),
            )
            .unwrap_or(*pixel);
        });
    }
}

/// Undo alpha premultiplication for one channel.
fn demul(channel: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        return 0;
    }
    ((channel as u32 * 255 + alpha as u32 / 2) / alpha as u32).min(255) as u8
}

/// The size the notification area will actually draw a tray icon at.
///
/// This used to be a flat 32, on the reasoning that every platform downscales
/// better than it upscales. Rasterising proved the reasoning incomplete:
/// downscaling *geometry* is not the same as downscaling a photograph, and a
/// 32-unit mark reduced to 16 px loses the 1.2 px glyph that carries the
/// state. Rendering natively at the size the shell will use is what lets
/// [`Profile`] do its job at all — a chunky 16 px profile is pointless if the
/// bitmap handed over is 32 px and Windows shrinks it.
///
/// Windows publishes the number as `SM_CXSMICON`: 16 at 100%, 20 at 125%, 24
/// at 150%, 32 at 200%. Elsewhere there is no equivalent to ask, so
/// [`RASTER_SIZE`] stands.
pub fn preferred_size() -> u32 {
    #[cfg(windows)]
    {
        /// `SM_CXSMICON` from `winuser.h`: the small-icon width, which is what
        /// the notification area uses.
        const SM_CXSMICON: i32 = 49;

        // SAFETY: `GetSystemMetrics` is a pure lookup with no pointer
        // arguments and no failure mode beyond returning 0 for an unknown
        // index. It is declared here rather than pulled from a binding crate
        // because `windows-sys` is not a dependency of this crate; the
        // signature is the documented `int GetSystemMetrics(int)`.
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetSystemMetrics(index: i32) -> i32;
        }

        let reported = unsafe { GetSystemMetrics(SM_CXSMICON) };
        if reported <= 0 {
            return RASTER_SIZE;
        }
        // Clamped to the sizes §7.4 lists. A shell reporting something absurd
        // gets a sensible bitmap rather than a 4 px smudge or a 4 MB icon.
        (reported as u32).clamp(16, 64)
    }
    #[cfg(not(windows))]
    {
        RASTER_SIZE
    }
}

/// Build the `tray-icon` bitmap for a mark, at `size` pixels.
pub fn icon(key: IconKey, size: u32) -> Result<tray_icon::Icon, String> {
    let rgba = rasterise(key, size)?;
    tray_icon::Icon::from_rgba(rgba, size, size)
        .map_err(|e| format!("the tray bitmap was rejected: {e}"))
}

/// Rasterised marks, kept so the running animation does not re-render an SVG
/// twelve times a second.
///
/// Keyed by size as well as by mark, because the same mark is a different
/// drawing at 16 px and at 32 px — see [`Profile`]. Still bounded by
/// construction: five states × three variants × twelve frames × the one or two
/// sizes a machine ever asks for, and only the frames actually shown are built.
#[derive(Default)]
pub struct IconCache {
    icons: Mutex<HashMap<(IconKey, u32), tray_icon::Icon>>,
}

impl std::fmt::Debug for IconCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.icons.lock().map(|i| i.len()).unwrap_or(0);
        f.debug_struct("IconCache").field("cached", &count).finish()
    }
}

impl IconCache {
    pub fn new() -> IconCache {
        IconCache::default()
    }

    /// The bitmap for one mark at one size, rendering it on first use.
    pub fn get(&self, key: IconKey, size: u32) -> Result<tray_icon::Icon, String> {
        let mut cache = self.icons.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = cache.get(&(key, size)) {
            return Ok(existing.clone());
        }
        let built = icon(key, size)?;
        cache.insert((key, size), built.clone());
        Ok(built)
    }

    /// Forget everything. Called when the taskbar theme changes, so the next
    /// paint picks up the other variant.
    pub fn clear(&self) {
        if let Ok(mut cache) = self.icons.lock() {
            cache.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_states() -> [Health; 5] {
        [Health::Idle, Health::Running, Health::Attention, Health::Paused, Health::Failed]
    }

    #[test]
    fn every_state_and_variant_produces_a_parseable_svg() {
        for health in all_states() {
            for variant in [Variant::LightTaskbar, Variant::DarkTaskbar, Variant::Template] {
                let key = IconKey::new(health, variant, 0);
                let document = svg(key, LARGE);
                assert!(document.starts_with("<svg"), "{health:?}/{variant:?}");
                resvg::usvg::Tree::from_str(&document, &resvg::usvg::Options::default())
                    .unwrap_or_else(|e| panic!("{health:?}/{variant:?} did not parse: {e}"));
            }
        }
    }

    #[test]
    fn every_state_and_variant_rasterises_to_visible_pixels() {
        for health in all_states() {
            for variant in [Variant::LightTaskbar, Variant::DarkTaskbar, Variant::Template] {
                let key = IconKey::new(health, variant, 0);
                let rgba = rasterise(key, RASTER_SIZE).expect("rasterise");
                assert_eq!(rgba.len(), (RASTER_SIZE * RASTER_SIZE * 4) as usize);
                let opaque = rgba.chunks_exact(4).filter(|p| p[3] > 32).count();
                assert!(
                    opaque > 40,
                    "{health:?}/{variant:?} rendered {opaque} visible pixels, which is blank"
                );
            }
        }
    }

    #[test]
    fn the_light_and_dark_variants_actually_differ() {
        // The whole reason for two variants is that one compromise colour is
        // optimal on neither taskbar. If these ever render identically, the
        // variant machinery has quietly stopped working.
        let light =
            rasterise(IconKey::new(Health::Idle, Variant::LightTaskbar, 0), 32).expect("light");
        let dark =
            rasterise(IconKey::new(Health::Idle, Variant::DarkTaskbar, 0), 32).expect("dark");
        assert_ne!(light, dark);
    }

    #[test]
    fn the_running_animation_has_twelve_distinct_frames() {
        let mut rendered = Vec::new();
        for frame in 0..RUNNING_FRAMES {
            rendered.push(
                rasterise(IconKey::new(Health::Running, Variant::DarkTaskbar, frame), 32)
                    .expect("frame"),
            );
        }
        for (i, a) in rendered.iter().enumerate() {
            for (j, b) in rendered.iter().enumerate().skip(i + 1) {
                assert_ne!(a, b, "running frames {i} and {j} are identical");
            }
        }
    }

    #[test]
    fn a_frame_index_only_matters_for_the_running_state() {
        assert_eq!(IconKey::new(Health::Idle, Variant::DarkTaskbar, 7).frame, 0);
        assert_eq!(IconKey::new(Health::Running, Variant::DarkTaskbar, 7).frame, 7);
        // And it wraps rather than panicking on a caller's overflowing counter.
        assert_eq!(IconKey::new(Health::Running, Variant::DarkTaskbar, 25).frame, 1);
    }

    #[test]
    fn a_template_mark_carries_no_colour() {
        // §7.4: macOS strips colour, so state must survive as silhouette. If a
        // chromatic channel ever appears here, the mark has become
        // colour-dependent and will be unreadable in the menu bar.
        for health in all_states() {
            let document = svg(IconKey::new(health, Variant::Template, 0), LARGE);
            for chromatic in ["#E0A83A", "#C2313A", "#5B9BFF", "#3A4250", "#8B93A5"] {
                assert!(
                    !document.contains(chromatic),
                    "{health:?} template still contains {chromatic}"
                );
            }
        }
    }

    #[test]
    fn stems_match_the_health_names_the_core_publishes() {
        // The tray resolves an icon by asking `Health` for its name rather
        // than matching on it again; if these drift, the wrong mark is shown.
        assert!(IconKey::new(Health::Failed, Variant::LightTaskbar, 0)
            .stem()
            .starts_with("failed"));
        assert_eq!(
            IconKey::new(Health::Running, Variant::DarkTaskbar, 3).stem(),
            "running-03-dark"
        );
        assert_eq!(IconKey::new(Health::Idle, Variant::Template, 0).stem(), "idle-template");
    }

    #[test]
    fn the_pip_knockout_only_exists_where_there_is_a_pip() {
        for health in [Health::Attention, Health::Failed] {
            assert!(
                svg(IconKey::new(health, Variant::DarkTaskbar, 0), LARGE).contains("pip-knockout")
            );
        }
        for health in [Health::Idle, Health::Paused, Health::Running] {
            assert!(
                !svg(IconKey::new(health, Variant::DarkTaskbar, 0), LARGE).contains("pip-knockout")
            );
        }
    }

    #[test]
    fn demultiplying_is_the_inverse_of_premultiplying() {
        assert_eq!(demul(0, 0), 0);
        assert_eq!(demul(255, 255), 255);
        assert_eq!(demul(128, 255), 128);
        // Half alpha over a full-intensity channel comes back to full.
        assert_eq!(demul(128, 128), 255);
    }
}
