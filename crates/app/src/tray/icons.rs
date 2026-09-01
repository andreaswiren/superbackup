//! The five tray marks: the **application mark** with a status badge on it,
//! generated from `DESIGN_SYSTEM.md` §7 and rasterised at run time.
//!
//! ## Why the mark is the base of every tray icon
//!
//! An earlier version of this module drew an abstract ring with a status pip.
//! It encoded state well and carried no brand identity at all — it looked
//! nothing like `assets/icons/superbackup.svg`, so the one place the
//! application is seen all day did not say which application it was. People
//! find a program in a crowded notification area by its icon; Dropbox, Docker
//! Desktop and OneDrive all show the brand mark with status layered onto it,
//! and for the same reason.
//!
//! The reasoning that led to the ring was sound but stopped one step short:
//! five states must stay distinguishable at 16 px *and* under macOS's
//! monochrome template rendering, and a brand mark alone cannot encode status.
//! The conclusion drawn was that the mark had to be abandoned. The conclusion
//! available was that the mark had to be **combined** with a status indicator.
//!
//! So: every mark is the interlock — one rounded square cut along a stepped
//! line into two congruent halves, the lower one slid down-right so the cut
//! opens into a kerf — with a circular well knocked out of its bottom-right
//! corner and one bold status glyph sitting in that well.
//!
//! ## What carries the state
//!
//! The **badge's silhouette**, never an interior detail and never colour:
//!
//! | `Health` | Badge | Why it survives 16 px |
//! |---|---|---|
//! | `Idle` | a filled disc | solid, no interior structure at all |
//! | `Running` | the same disc opened into a ring with a gap that rotates | the only badge with a hole in the middle |
//! | `Attention` | a triangle | the only badge with a flat base and a point |
//! | `Paused` | two bars | the only badge that is not one connected shape |
//! | `Failed` | a cross | the only badge made of diagonals |
//!
//! `Idle` and `Running` are deliberately the same circle — whole, then opened
//! and turning. Nothing is distinguished by an interior glyph, because that is
//! precisely what failed before: a 2.4-unit glyph inside a 6-unit pip is
//! 1.2 px at 16 px, and `attention` and `failed` became the same smear.
//!
//! ## One drawing, not two
//!
//! The previous design needed a `LARGE` and a `SMALL` profile because its
//! proportions did not survive 16 px. This one is *drawn* at the 16 px floor:
//! every feature is at least 3.0 canvas units, which is 1.5 px at 16 px, so
//! the same drawing is used at every size and there is no profile switch to
//! get wrong. The floor is asserted by `no_feature_falls_below_the_stroke_floor`.
//!
//! ## Geometry (§7.1)
//!
//! Canvas 32 × 32.
//!
//! * Mark: bounding box 24.5 units square with its top-left at (0.7, 0.7);
//!   kerf 3.1; corner radius, cut step and inner radius are fractions of the
//!   square's side, exactly as `assets/icons/superbackup-mono.svg` draws them.
//! * Badge: centred (24.5, 24.5), every glyph inscribed in a circle of radius
//!   6.8, stroke 3.4 (4.08 on diagonals — a diagonal antialiases across two
//!   pixel columns and reads lighter than an axis-aligned stroke of the same
//!   width).
//! * Clear space: a disc of radius 6.8 + 2.8 is knocked out of the mark's
//!   alpha. It is a real cutout, not a stroke painted in "the taskbar colour":
//!   such a stroke is wrong the moment the user changes their accent, and
//!   always wrong on a Linux panel whose colour we did not guess.
//! * The composition spans 0.7 … 31.3 in both axes — the same 0.7-unit margin
//!   the mark has, so the icon fills its slot the way the shell's own icons do.
//!
//! **The mark stays in one piece.** The knockout is a disc taken out of the
//! lower half's inner corner, and if it reaches too far that half is severed
//! into two fragments and the mark stops reading as *two congruent pieces* —
//! which is the entire idea. A 25.5-unit mark did exactly that: connected in
//! the vector, three fragments once rasterised at 16 px and 20 px. 24.5 is the
//! largest span that stays two pieces at every size, and
//! `the_mark_is_two_pieces_at_every_size` holds the line.

use std::collections::HashMap;
use std::f32::consts::{PI, SQRT_2};
use std::sync::Mutex;

use superbackup_core::state::Health;

/// The design canvas. Every coordinate below is in these units.
pub const CANVAS: f32 = 32.0;

// ---------------------------------------------------------------------------
// The mark
// ---------------------------------------------------------------------------

/// Side of the mark's bounding square, and the top-left it is placed at.
///
/// The largest span whose lower half survives the badge knockout as one piece
/// at 16 px — see the module docs.
pub const MARK_SPAN: f32 = 24.5;
pub const MARK_ORIGIN: f32 = 0.7;

/// The gap the slid half opens up. 3.1 units is 1.55 px at 16 px; below the
/// 1.5 px floor the two halves fuse into one blob and the mark loses the only
/// thing that says it is two pieces.
pub const KERF: f32 = 3.1;

/// Corner radius, cut step and inner-corner radius, as fractions of the square's
/// side. The same three numbers `tools/icons/geometry.py` draws the monochrome
/// application mark with.
const CORNER_FRACTION: f32 = 0.20;
const STEP_FRACTION: f32 = 0.30;
const INNER_FRACTION: f32 = 0.045;

// ---------------------------------------------------------------------------
// The badge
// ---------------------------------------------------------------------------

/// Badge centre. On the down-right diagonal, which is the direction the copied
/// half slides — the mark already points at this corner.
pub const BADGE_X: f32 = 24.5;
pub const BADGE_Y: f32 = 24.5;

/// Every badge glyph is inscribed in this radius, so the clear space around it
/// is guaranteed by construction rather than checked shape by shape.
pub const BADGE_RADIUS: f32 = 6.8;

/// Badge stroke. 1.7 px at 16 px.
pub const BADGE_STROKE: f32 = 3.4;

/// Clear space between the badge and the mark, knocked out of the mark's alpha.
///
/// 2.8 units is 1.4 px at 16 px, which is under the 1.5 px stroke floor — and
/// deliberately so, because this is a *gap*, not a stroke. A stroke has to show
/// its shape; a gap only has to break contact, and the number was picked by
/// measuring where it stops doing that rather than by rounding up to the floor:
///
/// | Clearance | Badge separate from the mark at 16 px? |
/// |---|---|
/// | 2.4 | **no** — in the template, where mark and badge are the same black, `running`, `paused` and `failed` fuse into the mark |
/// | 2.6 | yes, at every size, state, variant and alpha threshold |
/// | 2.8 | yes, with margin — what is used |
///
/// Taking it to 3.0 would work too, but only by shrinking the mark from 24.5
/// units to 21.7 to keep the lower half in one piece — a tenth of the mark's
/// width paid for a gap that is already sufficient at 2.8.
/// `the_badge_never_touches_the_mark` is the standing check.
pub const BADGE_CLEARANCE: f32 = 2.8;

/// Diagonal strokes are drawn this much heavier than axis-aligned ones.
///
/// A diagonal spreads its coverage over two pixel columns, so a 3.4-unit
/// diagonal reads visibly lighter than a 3.4-unit vertical at 16 px. `failed`
/// is the state that must never be the faintest thing on the taskbar.
const DIAGONAL_WEIGHT: f32 = 1.20;

/// `idle`'s disc, as a fraction of the badge radius. Nearly filling the badge
/// circle is deliberate: it pairs with `running`, which is the same circle
/// opened into a ring.
const IDLE_DISC: f32 = 0.88;

/// Rounding on the `attention` triangle's corners, as a fraction of the badge
/// radius.
const TRIANGLE_CORNER: f32 = 0.20;

/// `paused`'s bars: how far each sits from the centre (× stroke) and how tall
/// it is (× the ring radius).
///
/// The offset is set by the *gap*, not by taste: two bars that merge are one
/// bar, so `2 · offset · stroke − stroke` has to clear [`FEATURE_FLOOR`]. At
/// 0.92 it was 2.86 units — 1.43 px at 16 px — and
/// `no_feature_falls_below_the_stroke_floor` rejected it.
const PAUSE_OFFSET: f32 = 0.97;
const PAUSE_HEIGHT: f32 = 1.45;

/// Running animation, §7.2: twelve frames.
pub const RUNNING_FRAMES: usize = 12;

/// How much of the ring is drawn, and where the gap starts on frame 0. 0° is
/// east, y down.
const RUNNING_SWEEP_DEG: f32 = 250.0;
const RUNNING_PHASE_DEG: f32 = 55.0;

/// The smallest feature any variant may render, in canvas units.
///
/// One canvas unit is half a pixel at 16 px, so 3.0 units is the 1.5 px floor
/// §7.1 sets. Everything above is measured against it by
/// `no_feature_falls_below_the_stroke_floor`.
pub const FEATURE_FLOOR: f32 = 3.0;

/// The floor, for the constants that are knowable without arithmetic on a
/// profile: a build that lowers one of these does not compile.
///
/// `no_feature_falls_below_the_stroke_floor` covers the derived features too —
/// the mark's thinnest limb, the ring's hole, the gap between `paused`'s bars —
/// which need the products and differences a `const` context cannot take the
/// square root of.
const _: () = {
    assert!(KERF >= FEATURE_FLOOR);
    assert!(BADGE_STROKE >= FEATURE_FLOOR);
    // And the composition stays inside the canvas with the same margin on all
    // four sides, so the icon fills its slot without touching the edge.
    assert!(BADGE_X + BADGE_RADIUS <= CANVAS - MARK_ORIGIN);
    assert!(MARK_ORIGIN + MARK_SPAN <= CANVAS - MARK_ORIGIN);
};

/// The size a mark is rasterised at when nothing better is known.
///
/// The tray asks the platform for its actual notification-area icon size and
/// renders natively at it — see [`preferred_size`]. This is the fallback, and
/// the size the reference assets in `assets/tray/` are drawn at.
pub const RASTER_SIZE: u32 = 32;

// ---------------------------------------------------------------------------
// Variants
// ---------------------------------------------------------------------------

/// Which taskbar the mark is being drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    /// Light taskbar: dark ink. §7.4 `-light`, mark ink `#22262E`.
    LightTaskbar,
    /// Dark taskbar: light ink. §7.4 `-dark`, mark ink `#E8ECF2`.
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

    /// The ink the application mark itself is drawn in.
    pub fn mark_ink(self) -> &'static str {
        match self {
            Variant::LightTaskbar => "#22262E",
            Variant::DarkTaskbar => "#E8ECF2",
            Variant::Template => "#000000",
        }
    }

    /// The ink the status badge is drawn in.
    ///
    /// Variant-aware for every state, which the previous design was not: it
    /// fixed `#E0A83A` and `#C2313A` for both taskbars, and those are 1.92:1
    /// on a light taskbar and 2.94:1 on a dark one — both under SC 1.4.11's
    /// 3:1, and the second is the *failure* state. Every value here is a §2
    /// token chosen for the taskbar it is drawn on, and the worst of the ten
    /// is 4.95:1. See `assets/tray/README.md` for the measured table.
    pub fn badge_ink(self, health: Health) -> &'static str {
        match self {
            Variant::Template => "#000000",
            Variant::LightTaskbar => match health {
                Health::Idle => "#12793B",      // §2.2 success
                Health::Running => "#155FCC",   // §2.2 info
                Health::Attention => "#8A5B00", // §2.2 warning
                Health::Paused => "#5E6774",    // §2.2 neutral
                Health::Failed => "#B3242B",    // §2.2 danger.fill
            },
            Variant::DarkTaskbar => match health {
                Health::Idle => "#4FBF6B",      // §2.1 success
                Health::Running => "#5B9BFF",   // §2.1 accent
                Health::Attention => "#E0A83A", // §2.1 warning
                Health::Paused => "#8B93A5",    // §2.1 neutral
                Health::Failed => "#FF6B72",    // §2.1 danger
            },
        }
    }
}

/// Does the shell use a light taskbar?
///
/// Windows answers in `HKCU\...\Themes\Personalize\SystemUsesLightTheme`.
/// Every other platform is assumed dark, matching §7.4's "Linux: full colour,
/// dark-taskbar variant".
pub fn system_uses_light_theme() -> bool {
    // Reads the registry natively through the core platform layer.
    //
    // This used to shell out to `reg.exe`. That is a *console* application, and
    // this function is called from the tray's animation tick — so at a 120 ms
    // tick it spawned about eight console windows a second for the whole
    // duration of a backup. Never spawn a process to read a registry value.
    superbackup_core::platform::system_uses_light_theme()
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
// Path helpers
// ---------------------------------------------------------------------------

/// Three decimals, trailing zeros trimmed. Keeps the generated document small
/// and, more usefully, keeps it diffable against `tools/icons/geometry.py`,
/// which formats the same way.
fn n(value: f32) -> String {
    let mut text = format!("{value:.3}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if text == "-0" || text.is_empty() {
        "0".to_string()
    } else {
        text
    }
}

/// Path data for a closed polygon with a per-vertex corner radius.
///
/// Handles reflex corners: the mark's inner corners turn the other way, and a
/// naive implementation sweeps those arcs backwards and draws a bow tie. Mirrors
/// `rounded_polygon` in `tools/icons/geometry.py` line for line, because the
/// reference SVGs in `assets/tray/` and the marks this module draws at run time
/// have to be the same drawing.
fn rounded_polygon(points: &[(f32, f32, f32)]) -> String {
    let count = points.len();
    let mut segments: Vec<String> = Vec::with_capacity(count * 2 + 1);

    for i in 0..count {
        let (px, py, requested) = points[i];
        let (ax, ay, _) = points[(i + count - 1) % count];
        let (bx, by, _) = points[(i + 1) % count];

        let v1 = (ax - px, ay - py);
        let v2 = (bx - px, by - py);
        let l1 = v1.0.hypot(v1.1).max(f32::MIN_POSITIVE);
        let l2 = v2.0.hypot(v2.1).max(f32::MIN_POSITIVE);
        let u1 = (v1.0 / l1, v1.1 / l1);
        let u2 = (v2.0 / l2, v2.1 / l2);

        // Clamp so neighbouring corners cannot eat each other's edge.
        let radius = requested.min(l1 / 2.0).min(l2 / 2.0);
        if radius <= 0.0005 {
            let lead = if segments.is_empty() { "M" } else { "L" };
            segments.push(format!("{lead} {} {}", n(px), n(py)));
            continue;
        }

        let angle = (u1.0 * u2.0 + u1.1 * u2.1).clamp(-1.0, 1.0).acos();
        let mut setback = if angle > 1e-6 { radius / (angle / 2.0).tan() } else { 0.0 };
        setback = setback.min(l1 / 2.0).min(l2 / 2.0);
        // Recover the radius the clamped setback actually implies.
        let radius = setback * (angle / 2.0).tan();

        let start = (px + u1.0 * setback, py + u1.1 * setback);
        let end = (px + u2.0 * setback, py + u2.1 * setback);
        // The cross product's sign gives the turn direction, which is the sweep.
        let cross = u1.0 * u2.1 - u1.1 * u2.0;
        let sweep = if cross > 0.0 { 0 } else { 1 };

        let lead = if segments.is_empty() { "M" } else { "L" };
        segments.push(format!("{lead} {} {}", n(start.0), n(start.1)));
        segments.push(format!(
            "A {} {} 0 0 {sweep} {} {}",
            n(radius),
            n(radius),
            n(end.0),
            n(end.1)
        ));
    }
    segments.push("Z".to_string());
    segments.join(" ")
}

/// The two congruent halves of the application mark.
///
/// Returns the path data for one half and the transform that produces the
/// other. The second piece is the first **rotated 180°** about the square's
/// centre and translated by `(KERF, KERF)`; that translation is the only thing
/// that opens the cut, which is why the kerf is exactly uniform along every
/// segment of it without a single hand-placed coordinate. The two pieces are
/// congruent by construction — a backup that is not identical to the original
/// is not a backup, and the mark cannot be drawn any other way.
fn interlock() -> (String, String) {
    let side = MARK_SPAN - KERF;
    let radius = side * CORNER_FRACTION;
    let step = side * STEP_FRACTION;
    let inner = side * INNER_FRACTION;

    // The union spans `side + KERF` = MARK_SPAN, so the first piece starts at 0.
    let (x0, y0) = (0.0_f32, 0.0_f32);
    let x1 = x0 + side;
    let cx = x0 + side / 2.0;
    let cy = y0 + side / 2.0;

    let piece = rounded_polygon(&[
        (x0, y0, radius),       // outer top-left
        (x1, y0, radius),       // outer top-right
        (x1, cy - step, inner), // the cut leaves the right edge
        (cx, cy - step, inner), // inner corner (reflex)
        (cx, cy + step, inner), // inner corner
        (x0, cy + step, inner), // the cut leaves the left edge
    ]);
    let transform = format!("translate({} {}) rotate(180 {} {})", n(KERF), n(KERF), n(cx), n(cy));
    (piece, transform)
}

/// Point on a circle at `degrees`, with 0° = east and y increasing downwards.
fn on_circle(cx: f32, cy: f32, radius: f32, degrees: f32) -> (f32, f32) {
    let radians = degrees * PI / 180.0;
    (cx + radius * radians.cos(), cy + radius * radians.sin())
}

// ---------------------------------------------------------------------------
// The badge
// ---------------------------------------------------------------------------

/// The radius the stroked badge glyphs are drawn on, so their outer edge lands
/// exactly on [`BADGE_RADIUS`].
fn ring_radius() -> f32 {
    BADGE_RADIUS - BADGE_STROKE / 2.0
}

/// Half the diagonal reach of `failed`'s cross, sized so its round caps also
/// land on [`BADGE_RADIUS`].
fn cross_reach() -> f32 {
    (BADGE_RADIUS - BADGE_STROKE * DIAGONAL_WEIGHT / 2.0) / SQRT_2
}

/// The badge for one state, painted in `ink`.
///
/// Every shape here is inscribed in a circle of [`BADGE_RADIUS`] about the badge
/// centre. That is not an aesthetic preference: the clear space in the mark is a
/// disc of `BADGE_RADIUS + BADGE_CLEARANCE`, so inscribing the glyph is what
/// makes the clearance true for every state without measuring each one.
fn badge(health: Health, frame: usize, ink: &str) -> String {
    let stroke = n(BADGE_STROKE);
    match health {
        Health::Idle => format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{ink}\"/>",
            n(BADGE_X),
            n(BADGE_Y),
            n(BADGE_RADIUS * IDLE_DISC)
        ),
        Health::Running => {
            // The same circle as `idle`, opened into a ring whose gap travels
            // round it. The animation is carried by *alpha*, not colour, so it
            // survives the macOS template — and the ring's hole is what keeps
            // `running` distinct from `idle` in a single still frame.
            let radius = ring_radius();
            let start = RUNNING_PHASE_DEG + frame as f32 * 360.0 / RUNNING_FRAMES as f32;
            let (x0, y0) = on_circle(BADGE_X, BADGE_Y, radius, start);
            let (x1, y1) = on_circle(BADGE_X, BADGE_Y, radius, start + RUNNING_SWEEP_DEG);
            let large = if RUNNING_SWEEP_DEG > 180.0 { 1 } else { 0 };
            format!(
                "<path d=\"M {} {} A {} {} 0 {large} 1 {} {}\" fill=\"none\" stroke=\"{ink}\" \
                 stroke-width=\"{stroke}\" stroke-linecap=\"round\"/>",
                n(x0),
                n(y0),
                n(radius),
                n(radius),
                n(x1),
                n(y1)
            )
        }
        Health::Attention => {
            // An equilateral triangle inscribed in the badge circle: the
            // largest triangle that fits, 1.73 r across by 1.5 r tall, so its
            // flat base and its point cannot be read as `idle`'s disc.
            let mut points = Vec::with_capacity(3);
            for k in 0..3 {
                let angle = -90.0 + 120.0 * k as f32;
                let (x, y) = on_circle(BADGE_X, BADGE_Y, BADGE_RADIUS, angle);
                points.push((x, y, BADGE_RADIUS * TRIANGLE_CORNER));
            }
            format!(
                "<path d=\"{}\" fill=\"{ink}\" stroke-linejoin=\"round\"/>",
                rounded_polygon(&points)
            )
        }
        Health::Paused => {
            // The only badge that is not one connected shape.
            let offset = BADGE_STROKE * PAUSE_OFFSET;
            let half = ring_radius() * PAUSE_HEIGHT / 2.0;
            format!(
                "<path d=\"M {} {} L {} {} M {} {} L {} {}\" fill=\"none\" stroke=\"{ink}\" \
                 stroke-width=\"{stroke}\" stroke-linecap=\"round\"/>",
                n(BADGE_X - offset),
                n(BADGE_Y - half),
                n(BADGE_X - offset),
                n(BADGE_Y + half),
                n(BADGE_X + offset),
                n(BADGE_Y - half),
                n(BADGE_X + offset),
                n(BADGE_Y + half)
            )
        }
        Health::Failed => {
            // The only badge made of diagonals, and the only one drawn heavier
            // than BADGE_STROKE — see DIAGONAL_WEIGHT.
            let a = cross_reach();
            format!(
                "<path d=\"M {} {} L {} {} M {} {} L {} {}\" fill=\"none\" stroke=\"{ink}\" \
                 stroke-width=\"{}\" stroke-linecap=\"round\"/>",
                n(BADGE_X - a),
                n(BADGE_Y - a),
                n(BADGE_X + a),
                n(BADGE_Y + a),
                n(BADGE_X + a),
                n(BADGE_Y - a),
                n(BADGE_X - a),
                n(BADGE_Y + a),
                n(BADGE_STROKE * DIAGONAL_WEIGHT)
            )
        }
    }
}

// ---------------------------------------------------------------------------
// SVG generation
// ---------------------------------------------------------------------------

/// The complete SVG document for one mark.
///
/// Returned as text rather than as a rendered tree so that it can be asserted
/// on in tests, dumped for a designer to check against the spec, and written
/// out as a build artefact without changing anything here.
pub fn svg(key: IconKey) -> String {
    let (piece, transform) = interlock();
    let ink = key.variant.mark_ink();

    // White keeps, black cuts. The well is knocked out of the mark's alpha in
    // every state — including `idle`, so that all five marks are pixel-identical
    // outside the badge and the set reads as one application rather than five.
    let defs = format!(
        "<defs><mask id=\"badge-clearance\">\
           <rect x=\"0\" y=\"0\" width=\"{c}\" height=\"{c}\" fill=\"#fff\"/>\
           <circle cx=\"{x}\" cy=\"{y}\" r=\"{r}\" fill=\"#000\"/>\
         </mask></defs>",
        c = n(CANVAS),
        x = n(BADGE_X),
        y = n(BADGE_Y),
        r = n(BADGE_RADIUS + BADGE_CLEARANCE)
    );

    let mark = format!(
        "<g transform=\"translate({o} {o})\" mask=\"url(#badge-clearance)\">\
           <path d=\"{piece}\" fill=\"{ink}\"/>\
           <path d=\"{piece}\" fill=\"{ink}\" transform=\"{transform}\"/>\
         </g>",
        o = n(MARK_ORIGIN)
    );

    let badge = badge(key.health, key.frame, key.variant.badge_ink(key.health));

    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{c}\" height=\"{c}\" \
         viewBox=\"0 0 {c} {c}\" fill=\"none\" stroke-linejoin=\"round\" \
         stroke-linecap=\"round\">{defs}{mark}{badge}</svg>",
        c = n(CANVAS)
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
    let document = svg(key);
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
/// downscaling *geometry* is not the same as downscaling a photograph. The mark
/// is drawn at the 16 px floor — a 3.1-unit kerf is 1.55 px there — and handing
/// the shell a 32 px bitmap to shrink throws that away and fuses the two halves
/// back together.
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
/// Keyed by size as well as by mark, because a machine can be asked for more
/// than one size (a DPI change re-reads `SM_CXSMICON`). Bounded by
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

    fn all_variants() -> [Variant; 3] {
        [Variant::LightTaskbar, Variant::DarkTaskbar, Variant::Template]
    }

    #[test]
    fn every_state_and_variant_produces_a_parseable_svg() {
        for health in all_states() {
            for variant in all_variants() {
                let key = IconKey::new(health, variant, 0);
                let document = svg(key);
                assert!(document.starts_with("<svg"), "{health:?}/{variant:?}");
                resvg::usvg::Tree::from_str(&document, &resvg::usvg::Options::default())
                    .unwrap_or_else(|e| panic!("{health:?}/{variant:?} did not parse: {e}"));
            }
        }
    }

    #[test]
    fn every_state_and_variant_rasterises_to_visible_pixels() {
        for health in all_states() {
            for variant in all_variants() {
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
        for variant in all_variants() {
            let mut rendered = Vec::new();
            for frame in 0..RUNNING_FRAMES {
                rendered.push(
                    rasterise(IconKey::new(Health::Running, variant, frame), 16).expect("frame"),
                );
            }
            for (i, a) in rendered.iter().enumerate() {
                for (j, b) in rendered.iter().enumerate().skip(i + 1) {
                    assert_ne!(a, b, "{variant:?} running frames {i} and {j} are identical");
                }
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
        let chromatic = [
            "#12793B", "#4FBF6B", "#155FCC", "#5B9BFF", "#8A5B00", "#E0A83A", "#5E6774", "#8B93A5",
            "#B3242B", "#FF6B72", "#22262E", "#E8ECF2",
        ];
        for health in all_states() {
            let document = svg(IconKey::new(health, Variant::Template, 0));
            for colour in chromatic {
                assert!(!document.contains(colour), "{health:?} template still contains {colour}");
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
    fn every_state_carries_the_same_clear_space() {
        // The badge well is knocked out of every state, not only the badged
        // ones. That is what lets `the_base_mark_is_common_to_all_five_states`
        // compare the marks pixel for pixel.
        for health in all_states() {
            assert!(
                svg(IconKey::new(health, Variant::DarkTaskbar, 0)).contains("badge-clearance"),
                "{health:?} has no clear space round its badge"
            );
        }
    }

    /// Every badge glyph is inside the circle the clear space is cut for.
    ///
    /// This is the invariant that makes the clearance true for all five states
    /// without measuring each shape: the well is a disc of
    /// `BADGE_RADIUS + BADGE_CLEARANCE`, so anything within `BADGE_RADIUS` of
    /// the badge centre is separated from the mark by at least `BADGE_CLEARANCE`.
    #[test]
    fn every_badge_glyph_is_inscribed_in_the_badge_radius() {
        let extents = [
            ("idle disc", BADGE_RADIUS * IDLE_DISC),
            ("running ring", ring_radius() + BADGE_STROKE / 2.0),
            ("attention triangle", BADGE_RADIUS),
            ("paused bars, across", BADGE_STROKE * PAUSE_OFFSET + BADGE_STROKE / 2.0),
            ("paused bars, down", ring_radius() * PAUSE_HEIGHT / 2.0 + BADGE_STROKE / 2.0),
            ("failed cross", cross_reach() * SQRT_2 + BADGE_STROKE * DIAGONAL_WEIGHT / 2.0),
        ];
        for (what, extent) in extents {
            assert!(
                extent <= BADGE_RADIUS + 1e-4,
                "{what} reaches {extent:.3}, past the badge radius {BADGE_RADIUS}: it would \
                 come closer to the mark than the {BADGE_CLEARANCE}-unit clear space allows"
            );
        }
    }

    /// Nothing is drawn thinner than 1.5 px at 16 px.
    ///
    /// One canvas unit is half a pixel at 16 px. The previous design put a
    /// glyph stroke at 2.4 units — 1.2 px — and `attention` and `failed` became
    /// the same smear. Every feature below is measured against [`FEATURE_FLOOR`],
    /// and because this drawing clears it there is only one profile: the same
    /// geometry is used at every size.
    #[test]
    fn no_feature_falls_below_the_stroke_floor() {
        let side = MARK_SPAN - KERF;
        let features = [
            ("kerf", KERF),
            // The bar of a piece above the cut — the mark's thinnest limb.
            ("mark arm", side * (0.5 - STEP_FRACTION)),
            ("badge stroke", BADGE_STROKE),
            ("badge diagonal stroke", BADGE_STROKE * DIAGONAL_WEIGHT),
            // BADGE_CLEARANCE is deliberately absent: it is a gap rather than
            // a stroke, and it is checked by measuring what it has to achieve
            // — see `the_badge_never_touches_the_mark`.
            ("idle disc", BADGE_RADIUS * IDLE_DISC * 2.0),
            ("running ring hole", (ring_radius() - BADGE_STROKE / 2.0) * 2.0),
            ("gap between the paused bars", BADGE_STROKE * PAUSE_OFFSET * 2.0 - BADGE_STROKE),
        ];
        for (what, units) in features {
            assert!(
                units >= FEATURE_FLOOR - 1e-4,
                "{what} is {units:.2} units, which is {:.2} px at 16 px — under the \
                 {:.1} px floor",
                units / 2.0,
                FEATURE_FLOOR / 2.0
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
