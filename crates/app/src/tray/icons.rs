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

/// Ring geometry, §7.1.
const CENTRE: f32 = 16.0;
const RING_RADIUS: f32 = 10.5;
const RING_STROKE: f32 = 3.0;

/// Open-ring arc, §7.1.
const ARC_START_DEG: f32 = 85.0;
const ARC_SWEEP_DEG: f32 = 280.0;

/// Pip geometry, §7.1.
const PIP_X: f32 = 23.0;
const PIP_Y: f32 = 23.0;
const PIP_RADIUS: f32 = 6.0;
const PIP_KNOCKOUT: f32 = 1.5;
const PIP_STROKE: f32 = 2.4;

/// Running animation, §7.2: twelve frames, 30° apart.
pub const RUNNING_FRAMES: usize = 12;
const RUNNING_ARC_DEG: f32 = 90.0;

/// The size the tray bitmaps are rasterised at.
///
/// 32 px is the largest size §7.4 lists for Windows and Linux, and both
/// platforms downscale a larger bitmap far better than they upscale a smaller
/// one — a 16 px source on a 200% display is the mush the design rules exist
/// to prevent.
pub const RASTER_SIZE: u32 = 32;

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

    /// The ink drawn *inside* a coloured pip. In a template image the pip is
    /// solid black, so its glyph has to be a hole rather than a mark.
    fn pip_ink(self, chromatic: &'static str) -> &'static str {
        match self {
            // Punched out of the black pip, so the exclamation or cross reads
            // as a gap. `#FFFFFF` would be invisible after templating.
            Variant::Template => "#FFFFFF",
            _ => chromatic,
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

/// The complete SVG document for one mark.
///
/// Returned as text rather than as a rendered tree so that it can be asserted
/// on in tests, dumped for a designer to check against the spec, and written
/// out as a build artefact without changing anything here.
pub fn svg(key: IconKey) -> String {
    let variant = key.variant;
    let ring = variant.ring_ink();
    let mut body = String::new();

    // The knockout mask: white keeps, black cuts. Only the states that carry
    // a pip need it, and applying it unconditionally would eat into the ring
    // of the states that do not.
    let has_pip = matches!(key.health, Health::Attention | Health::Failed);
    let mask = if has_pip {
        body.push_str(&format!(
            "<mask id=\"pip-knockout\">\
               <rect x=\"0\" y=\"0\" width=\"{CANVAS}\" height=\"{CANVAS}\" fill=\"#fff\"/>\
               <circle cx=\"{PIP_X}\" cy=\"{PIP_Y}\" r=\"{:.2}\" fill=\"#000\"/>\
             </mask>",
            PIP_RADIUS + PIP_KNOCKOUT
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
                 <circle cx=\"{CENTRE}\" cy=\"{CENTRE}\" r=\"3\" fill=\"{ring}\"/>"
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
            let base = variant.colour("#3A4250");
            let moving = variant.colour("#5B9BFF");
            body.push_str(&format!(
                "<path d=\"{}\" fill=\"none\" stroke=\"{base}\" stroke-width=\"{RING_STROKE}\" \
                 stroke-linecap=\"round\"/>",
                arc_path(ARC_START_DEG, ARC_SWEEP_DEG)
            ));
            let offset = (key.frame as f32) * (360.0 / RUNNING_FRAMES as f32);
            body.push_str(&format!(
                "<path d=\"{}\" fill=\"none\" stroke=\"{moving}\" stroke-width=\"{RING_STROKE}\" \
                 stroke-linecap=\"round\"/>",
                arc_path(ARC_START_DEG + offset, RUNNING_ARC_DEG)
            ));
        }
        Health::Attention => {
            body.push_str(&format!(
                "<path d=\"{}\" fill=\"none\" stroke=\"{ring}\" stroke-width=\"{RING_STROKE}\" \
                 stroke-linecap=\"round\"{mask}/>",
                arc_path(ARC_START_DEG, ARC_SWEEP_DEG)
            ));
            let fill = variant.colour("#E0A83A");
            let ink = variant.pip_ink("#1A1206");
            body.push_str(&format!(
                "<circle cx=\"{PIP_X}\" cy=\"{PIP_Y}\" r=\"{PIP_RADIUS}\" fill=\"{fill}\"/>\
                 <path d=\"M {PIP_X} 19.8 L {PIP_X} 23.4\" stroke=\"{ink}\" \
                 stroke-width=\"{PIP_STROKE}\" stroke-linecap=\"round\"/>\
                 <circle cx=\"{PIP_X}\" cy=\"26\" r=\"1.3\" fill=\"{ink}\"/>"
            ));
        }
        Health::Failed => {
            body.push_str(&format!(
                "<path d=\"{}\" fill=\"none\" stroke=\"{ring}\" stroke-width=\"{RING_STROKE}\" \
                 stroke-linecap=\"round\"{mask}/>",
                arc_path(ARC_START_DEG, ARC_SWEEP_DEG)
            ));
            let fill = variant.colour("#C2313A");
            let ink = variant.pip_ink("#FFFFFF");
            body.push_str(&format!(
                "<circle cx=\"{PIP_X}\" cy=\"{PIP_Y}\" r=\"{PIP_RADIUS}\" fill=\"{fill}\"/>\
                 <path d=\"M 20.9 20.9 L 25.1 25.1 M 25.1 20.9 L 20.9 25.1\" stroke=\"{ink}\" \
                 stroke-width=\"{PIP_STROKE}\" stroke-linecap=\"round\"/>"
            ));
        }
    }

    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{CANVAS}\" height=\"{CANVAS}\" \
         viewBox=\"0 0 {CANVAS} {CANVAS}\" fill=\"none\" stroke-linejoin=\"round\" \
         stroke-linecap=\"round\">{body}</svg>"
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

/// Build the `tray-icon` bitmap for a mark.
pub fn icon(key: IconKey) -> Result<tray_icon::Icon, String> {
    let rgba = rasterise(key, RASTER_SIZE)?;
    tray_icon::Icon::from_rgba(rgba, RASTER_SIZE, RASTER_SIZE)
        .map_err(|e| format!("the tray bitmap was rejected: {e}"))
}

/// Rasterised marks, kept so the running animation does not re-render an SVG
/// twelve times a second.
///
/// Bounded by construction: five states × three variants × twelve frames is
/// the entire key space, and only the frames actually shown are ever built.
#[derive(Default)]
pub struct IconCache {
    icons: Mutex<HashMap<IconKey, tray_icon::Icon>>,
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

    /// The bitmap for one mark, rendering it on first use.
    pub fn get(&self, key: IconKey) -> Result<tray_icon::Icon, String> {
        let mut cache = self.icons.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = cache.get(&key) {
            return Ok(existing.clone());
        }
        let built = icon(key)?;
        cache.insert(key, built.clone());
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
        let light = rasterise(IconKey::new(Health::Idle, Variant::LightTaskbar, 0), 32)
            .expect("light");
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
            let document = svg(IconKey::new(health, Variant::Template, 0));
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
        assert!(IconKey::new(Health::Failed, Variant::LightTaskbar, 0).stem().starts_with("failed"));
        assert_eq!(
            IconKey::new(Health::Running, Variant::DarkTaskbar, 3).stem(),
            "running-03-dark"
        );
        assert_eq!(IconKey::new(Health::Idle, Variant::Template, 0).stem(), "idle-template");
    }

    #[test]
    fn the_pip_knockout_only_exists_where_there_is_a_pip() {
        for health in [Health::Attention, Health::Failed] {
            assert!(svg(IconKey::new(health, Variant::DarkTaskbar, 0)).contains("pip-knockout"));
        }
        for health in [Health::Idle, Health::Paused, Health::Running] {
            assert!(!svg(IconKey::new(health, Variant::DarkTaskbar, 0)).contains("pip-knockout"));
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
