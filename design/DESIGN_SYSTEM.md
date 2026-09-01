# superbackup — Design System

Version 1.0 · Target: `egui` / `eframe` (immediate mode, pure Rust)
Companion documents: `UX_SPEC.md`, `COPY.md`, `WIREFRAMES.md`

This document is the single source of truth for every colour, size and state in
the application. Anything not listed here does not exist. If an implementer
needs a value that is missing, that is a bug in this document, not licence to
invent one.

---

## 0. Design principles

1. **Quiet until it matters.** The default state of a backup tool is "nothing
   is wrong". Neutral greys carry 90% of the interface; chromatic colour is
   reserved for status, and status only.
2. **Never colour alone.** Every state that is expressed in colour is also
   expressed in shape (icon) and words (label). This is required for the macOS
   monochrome tray, for colour-blind users, and for screenshots in bug reports.
3. **Density with air.** Dense tables, generous panel padding. A 720px-tall
   window must show eight job cards or eighteen table rows without scrolling.
4. **The destructive path is always slower than the safe path.** Confirmations
   that require typing exist only where data is genuinely unrecoverable.
5. **One typeface, one accent.** Inter and a single blue. Everything else is
   grey plus four status hues.

---

## 1. egui constraints that shape this system

These are hard limits of the toolkit. Every one of them has changed a design
decision, and each is cross-referenced from `UX_SPEC.md`.

| # | egui limitation | Design consequence |
|---|---|---|
| L1 | No CSS, no cascade. Every widget's colours come from `egui::Visuals` or an explicit `Frame`/`Painter` call each frame. | Tokens are compiled into a `Tokens` struct with a `light()` and `dark()` constructor. Components read tokens; they never hard-code hex. |
| L2 | No layout engine beyond horizontal/vertical/grid stacks. No flex `space-between` on wrapped content, no absolute positioning inside a flow. | All layouts are built from fixed-width left columns plus `ui.add_space(remaining)`. Every screen in `WIREFRAMES.md` declares explicit column widths for this reason. |
| L3 | No text ellipsis on arbitrary widgets; long strings either wrap or blow out the layout. | Paths use a **middle-ellipsis helper** (`elide_middle(text, max_px, font)`): keeps the drive letter and the last two path segments, replaces the middle with `…`. Full value always available in the tooltip. |
| L4 | No native tree view; no virtualised tree. | The snapshot browser is a **flat, virtualised, single-level list with breadcrumb navigation**, not an expanding tree. Rendered with `ScrollArea::show_rows` at a fixed 28px row height so a 400,000-entry directory costs the same as a 10-entry one. |
| L5 | No native table; `egui_extras::TableBuilder` requires fixed or remainder columns and cannot auto-size to content. | Every table in this spec declares exact column widths in px and which column is `remainder`. Column sets change at the 1000px breakpoint by dropping named columns, never by squeezing them. |
| L6 | Tab order is *widget instantiation order*, not visual order. | Every screen in `UX_SPEC.md` lists its focus order explicitly. Implementers must instantiate widgets in that order even when it complicates the layout code. |
| L7 | Custom-painted content (progress bars, sparklines, the health ring, strength meters) is invisible to AccessKit unless a `WidgetInfo` is supplied. | Every custom painter is wrapped in `ui.allocate_response(...)` followed by `response.widget_info(|| WidgetInfo::labeled(Role::ProgressIndicator, true, text))`. The exact strings are in `COPY.md` §12. |
| L8 | No runtime SVG rendering. | Icons are rasterised once at startup with `resvg` into a texture atlas at 14/16/20/24/32 px for the active DPI, and re-rasterised on DPI change. No arbitrary vector art at runtime. |
| L9 | No OpenType feature control (no `tnum`). | All right-aligned numeric columns (sizes, durations, counts, percentages) are rendered in JetBrains Mono, which is tabular by construction. Proportional Inter is never used for a number in a column. |
| L10 | Shadows are a single `epaint::Shadow` per frame; no layered shadows, no blur radius animation. | Cards use borders, not shadows. Shadows appear only on popup menus and modals, at the two fixed values in §6. |
| L11 | `TextEdit` has no input masks, no formatted numeric entry, no spinners. | Numeric fields are plain text with an on-blur parse, an inline unit suffix drawn to the right of the field, and a validation message below. Time-of-day entry is two `DragValue`s (hour, minute), not a masked field. |
| L12 | Bundled fonts only; no system font fallback for glyphs Inter lacks. | Inter + JetBrains Mono are embedded. At startup the app additionally attempts to load one CJK face from a per-platform path list (Windows `msyh.ttc` / `meiryo.ttc`, macOS `PingFang.ttc`, Linux `NotoSansCJK-Regular.ttc`) and appends it to both font families. If loading fails, non-Latin paths render as `□`; this is logged once at `warn` and never blocks a backup. |
| L13 | No modal stack; `egui::Modal` is a single overlay. | The UI never nests modals. A flow that needs two decisions (e.g. repository creation) is a **multi-step modal with its own internal step state**, never modal-on-modal. |
| L14 | Repaints are on-demand; a static screen costs nothing, but any animation forces a continuous repaint. | Animation is allowed only where something is genuinely happening: running progress bars, the tray running arc, and toast enter/exit. A screen with no running job must reach 0 fps at rest. |
| L15 | No drag-and-drop from the OS shell into an egui window without platform glue. | Folder pickers use `rfd` native dialogs. Drag-and-drop of folders onto the window **is** supported (eframe surfaces `RawInput::dropped_files`) and is specified as an *additive* affordance only — never the only way to add a source. |

---

## 2. Colour tokens

Two complete palettes. `Theme::System` picks one at startup and on OS theme
change. All contrast ratios below are measured, not estimated (WCAG 2.1
relative-luminance formula), and are stated for the background the token is
actually used on.

Normal text must reach **4.5:1**. Non-text UI boundaries (control borders,
focus rings, status marks) must reach **3:1** per WCAG 1.4.11. Disabled
controls are exempt by WCAG and are deliberately below threshold so that
"disabled" reads as disabled.

### 2.1 Dark theme (default)

**Surfaces**

| Token | Hex | Use |
|---|---|---|
| `bg.canvas` | `#14171C` | Window background, content area |
| `bg.rail` | `#101318` | Left navigation rail, title bar |
| `bg.surface` | `#1B1F26` | Cards, panels, table body |
| `bg.surface.hover` | `#20252E` | Card / row hover |
| `bg.raised` | `#232833` | Secondary buttons, chips, popover body, table header |
| `bg.input` | `#0F1216` | Text fields, combo boxes, search |
| `bg.code` | `#12161B` | Monospace blocks: patterns, stderr, JSON |
| `bg.scrim` | `#06080B` @ 62% | Modal scrim |
| `bg.selected` | `#234065` | Selected table rows, text selection |

**Lines**

| Token | Hex | Contrast | Use |
|---|---|---|---|
| `border.subtle` | `#2A303B` | 1.25 : `bg.surface` | Card outlines, table row rules, dividers |
| `border.strong` | `#3A4250` | 1.63 : `bg.surface` | Section separators, card headers |
| `border.control` | `#6A7583` | **3.53** : `bg.surface`, 3.15 : `bg.raised`, 4.01 : `bg.input` | Every interactive control boundary. Meets 1.4.11. |
| `border.focus` | `#7FB4FF` | **8.45** : `bg.canvas`, 7.77 : `bg.surface` | Focus ring |

**Text**

| Token | Hex | Contrast | Use |
|---|---|---|---|
| `text.primary` | `#E8ECF2` | 15.15 canvas / 13.94 surface / 12.45 raised | Body, headings, values |
| `text.secondary` | `#A3ADBB` | 7.91 / 7.28 / 6.50 | Labels, meta, helper text |
| `text.muted` | `#8A94A1` | 5.84 / 5.38 / 4.80 | Timestamps, counts, placeholder |
| `text.disabled` | `#5E6674` | 2.55 : raised — **intentionally sub-AA** | Disabled control labels only |
| `text.oncolor` | `#FFFFFF` | see fills | Text on filled accent/danger buttons |
| `text.link` | `#7FB4FF` | 7.77 : surface | Inline links |

**Accent**

| Token | Hex | Contrast | Use |
|---|---|---|---|
| `accent` | `#5B9BFF` | 6.48 canvas / 5.96 surface | Accent text, icons, selected rail marker |
| `accent.fill` | `#2761D0` | 5.67 with `#FFFFFF` | Primary button rest |
| `accent.fill.hover` | `#2F6FE0` | 4.70 with `#FFFFFF` | Primary button hover |
| `accent.fill.active` | `#2158BE` | 6.55 with `#FFFFFF` | Primary button pressed |
| `accent.fill.border` | `#4C86EA` | 4.03 : `bg.surface` | 1px border on filled buttons so the button boundary itself meets 3:1 |

**Status** — each has a *mark* colour (icon/dot/bar), a *tint* background, and
a *tint text* colour for badges.

| State | Mark | Tint bg | Tint text | Tint contrast |
|---|---|---|---|---|
| Success | `#4FBF6B` (7.08 : surface) | `#16301F` | `#7EE29A` | **8.96** |
| Warning | `#E0A83A` (7.74) | `#33260C` | `#F2C566` | **9.10** |
| Danger | `#FF6B72` (5.98) | `#3A171A` | `#FF9A9F` | **7.88** |
| Info / running | `#69AEFF` (7.16) | `#12233C` | `#9CC7FF` | **9.03** |
| Neutral / paused | `#8B93A5` (5.36) | `#232833` | `#B6BFCC` | **7.95** |

**Danger fills** (destructive buttons)

| Token | Hex | With `#FFFFFF` |
|---|---|---|
| `danger.fill` | `#A82026` | 7.24 |
| `danger.fill.hover` | `#C2313A` | 5.53 |
| `danger.fill.active` | `#8F1B21` | 8.95 |
| `danger.fill.border` | `#E0454E` | 3.32 : `bg.surface` |

**Misc**

| Token | Hex |
|---|---|
| `progress.track` | `#2B313C` |
| `progress.fill` | `#5B9BFF` (4.71 : track) |
| `progress.fill.warn` | `#E0A83A` |
| `progress.fill.error` | `#FF6B72` |
| `rail.item.selected.bg` | `#1E2530` |
| `rail.item.selected.marker` | `#5B9BFF` (3px left bar) |
| `overlay.locked` | `#14171C` @ 70% |

### 2.2 Light theme

**Surfaces**

| Token | Hex | Use |
|---|---|---|
| `bg.canvas` | `#F4F6F9` | Window background |
| `bg.rail` | `#EDF0F4` | Navigation rail |
| `bg.surface` | `#FFFFFF` | Cards, panels, table body |
| `bg.surface.hover` | `#F7F9FB` | Row hover |
| `bg.raised` | `#F0F3F7` | Secondary buttons, chips, table header |
| `bg.input` | `#FFFFFF` | Text fields |
| `bg.code` | `#F2F4F7` | Monospace blocks |
| `bg.scrim` | `#171B21` @ 42% | Modal scrim |
| `bg.selected` | `#D5E4FB` | Selected rows (13.43 with `text.primary`) |

**Lines**

| Token | Hex | Contrast |
|---|---|---|
| `border.subtle` | `#DDE2E9` | 1.30 : `bg.surface` |
| `border.strong` | `#C2CAD5` | 1.65 : `bg.surface` |
| `border.control` | `#7C8697` | **3.68** : `#FFFFFF`, 3.39 : canvas, 3.30 : raised |
| `border.focus` | `#155FCC` | **5.48** : canvas |

**Text**

| Token | Hex | Contrast (canvas / surface / raised) |
|---|---|---|
| `text.primary` | `#171B21` | 15.96 / 17.28 / 15.53 |
| `text.secondary` | `#59626F` | 5.70 / 6.17 / 5.55 |
| `text.muted` | `#646D7A` | 4.84 / 5.24 / 4.70 |
| `text.disabled` | `#98A1AE` | 2.35 — intentionally sub-AA |
| `text.oncolor` | `#FFFFFF` | see fills |
| `text.link` | `#155FCC` | 5.48 / 5.93 |

**Accent**

| Token | Hex | Contrast |
|---|---|---|
| `accent` | `#155FCC` | 5.48 canvas / 5.93 surface |
| `accent.fill` | `#155FCC` | 5.93 with `#FFFFFF` |
| `accent.fill.hover` | `#1A6BE0` | 4.97 with `#FFFFFF` |
| `accent.fill.active` | `#0F4FAD` | 7.66 with `#FFFFFF` |
| `accent.fill.border` | `#0F4FAD` | — |

**Status**

| State | Mark | Tint bg | Tint text | Tint contrast |
|---|---|---|---|---|
| Success | `#12793B` (5.49 : white) | `#E3F5E9` | `#0D5C2C` | **7.16** |
| Warning | `#8A5B00` (5.87) | `#FBF0D8` | `#6E4700` | **7.24** |
| Danger | `#C3282F` (5.73) | `#FCE6E7` | `#9E1F26` | **6.58** |
| Info / running | `#155FCC` (5.93) | `#E4EEFC` | `#12539E` | **6.51** |
| Neutral / paused | `#5E6774` (5.72) | `#EDF0F4` | `#4A5462` | **6.72** |

**Danger fills**: `#B3242B` (6.55 white) / hover `#C82C34` (5.43) / active `#981E24` (8.25).

**Misc**: `progress.track` `#DCE1E8`, `progress.fill` `#155FCC` (4.51 : track),
`rail.item.selected.bg` `#E1EBFB`, `rail.item.selected.marker` `#155FCC`,
`overlay.locked` `#F4F6F9` @ 72%.

### 2.3 Rules

- A status colour never appears as a large area fill. Maximum chromatic area in
  one screen: the progress bar fill plus badges. Health banners use the tint
  background, not the mark colour.
- `border.control` is mandatory on every focusable control. `border.subtle` is
  for decoration and must never be the only boundary of something clickable.
- Focus rings are drawn **2px outside** the control bounds, 2px thick, in
  `border.focus`, radius = control radius + 2. Never inside: `#7FB4FF` against
  the primary fill is only 2.21:1, so an inset ring would fail 1.4.11.
- Hover in dark theme *brightens* fills; the rest state is deliberately the
  darker end of the ramp so that brightening never drops white text below 4.5:1.

---

## 3. Typography

### 3.1 Families

| Family | Face | Weights | Licence | egui family |
|---|---|---|---|---|
| UI | Inter | 400, 500, 600 | OFL 1.1 | `Proportional` |
| Code | JetBrains Mono | 400, 500 | OFL 1.1 | `Monospace` |
| CJK fallback | first available OS face (see L12) | 400 | system | appended to both |

Inter is embedded as three static instances rather than a variable font: egui's
`FontDefinitions` has no variable-axis support.

### 3.2 Scale

All sizes in logical px. Line heights are set via `egui::TextStyle` row height
overrides; egui applies them per-row, so multi-line paragraphs need
`ui.spacing_mut().item_spacing.y` set to `line_height - font_size`.

| Style | Size / Line | Weight | Family | Use |
|---|---|---|---|---|
| `display` | 24 / 32 | 600 | Inter | Onboarding headlines, "write this down" screen only |
| `h1` | 20 / 28 | 600 | Inter | Screen title in the header bar |
| `h2` | 16 / 24 | 600 | Inter | Card titles, section headers, modal titles |
| `h3` | 14 / 20 | 600 | Inter | Sub-section headers, form group labels |
| `body` | 14 / 20 | 400 | Inter | Default text style |
| `body.strong` | 14 / 20 | 500 | Inter | Values in key/value pairs, selected rail item |
| `small` | 12 / 16 | 400 | Inter | Helper text under fields, table metadata, timestamps |
| `small.strong` | 12 / 16 | 500 | Inter | Badge labels, button labels in compact toolbars |
| `micro` | 11 / 14 | 500 | Inter | Table column headers, eyebrow labels. Rendered in `text.muted`, **not** uppercased — egui has no letter-spacing, and uppercase without tracking is unreadable (see L9). |
| `mono` | 13 / 20 | 400 | JetBrains Mono | Paths, glob patterns, endpoints, buckets |
| `mono.strong` | 13 / 20 | 500 | JetBrains Mono | The generated repository passphrase on the write-it-down screen |
| `mono.small` | 12 / 16 | 400 | JetBrains Mono | Log lines, stderr, UUIDs, hashes, all numeric table cells |

### 3.3 Rules

- Maximum three type styles in one card.
- Numbers in a column: `mono.small`, right-aligned. Numbers in prose: `body`.
- Sentence case everywhere, including buttons and column headers. No Title Case,
  no ALL CAPS.
- Measure: body paragraphs are capped at **68 characters** (`≈ 560px` at 14px
  Inter). Helper text under a field is capped at the field width.
- Never rely on italics; Inter's italic is not embedded.

---

## 4. Spacing, sizing, layout

### 4.1 Scale

`2, 4, 6, 8, 12, 16, 20, 24, 32, 40, 48, 64`. Nothing else.

### 4.2 egui `Spacing` configuration

```
item_spacing        = (8, 6)
button_padding      = (12, 7)
menu_margin         = 6
indent              = 20
interact_size       = (44, 30)      // min hit target height 30
slider_width        = 180
combo_width         = 220
text_edit_width     = 280
icon_width          = 16
scroll_bar_width    = 10
scroll_bar_inner_margin = 4
```

Minimum hit target is **30 × 30 px** for icon-only controls and **30px tall** for
everything else. Table row hit targets are the full row width.

### 4.3 Window frame (1100 × 720 default, 900 × 600 minimum)

| Region | Size | Notes |
|---|---|---|
| Title bar | 40px | Native OS decorations. No custom chrome. |
| Left rail | 208px | Collapses to 64px icon-only when window width < 1000px |
| Rail divider | 1px `border.subtle` | |
| Header bar | 56px | Screen title (h1), contextual actions right-aligned |
| Content area | remainder | Padding 24px all sides; 20px when width < 1000px |
| Status strip | 28px | Bottom. Daemon state · Kopia version · vault state · last event |

At 1100 × 720: content area is **867 × 596** px, giving **819 × 596** inside
padding.
At 900 × 600: rail collapses to 64px, padding drops to 20px, content is
**835 × 476**, giving **795 × 476** inside padding.

### 4.4 Reflow rules (1100 → 900)

1. Rail collapses to 64px, icon-only, with tooltips carrying the label. Rail
   labels are still announced to AccessKit.
2. Content padding 24 → 20.
3. Two-column card grids become one column. The dashboard job grid goes from
   2 × N to 1 × N and each card keeps its 96px height.
4. Tables drop columns in this documented priority order (per-table lists are in
   `UX_SPEC.md`); the remainder column absorbs the freed width.
5. Two-pane screens (Restore browser, Job editor) keep both panes but the
   left pane shrinks from 280px to 220px. Below 820px total width the left pane
   becomes a dropdown; this is below the enforced minimum and only occurs if the
   OS forces a smaller size.
6. Modals: width `min(560, window_width - 80)`, max height
   `window_height - 96`, internal `ScrollArea` when content exceeds it.
7. Never reflow below 900 × 600. `eframe::ViewportBuilder::with_min_inner_size`
   is set to `(900.0, 600.0)`.

### 4.5 Corner radii

| Element | Radius |
|---|---|
| Button, input, combo, chip, checkbox container | 6 |
| Card, panel, banner, table container | 10 |
| Modal, popup menu | 14 |
| Badge / pill | 5 |
| Progress bar | 3 (height 6) / 4 (height 8) |
| Avatar-free; no circles except status dots (r = 4) and the tray mark |

---

## 5. Iconography

### 5.1 Source and rendering

Lucide icon set (ISC licence), 24 × 24 source grid, 1.5px stroke, round caps
and joins. Rasterised at startup with `resvg` into a single texture atlas at
14, 16, 20, 24, 32 px for the current `pixels_per_point`, re-rasterised on DPI
change (L8). Icons are tinted at draw time by multiplying the atlas alpha with
the target colour, so one atlas serves both themes.

### 5.2 Sizes

| Context | Size |
|---|---|
| Inline in body text | 14 |
| Buttons, rail, table cells, badges | 16 |
| Card headers, section headers | 20 |
| Modal headers | 24 |
| Empty states | 32 |

### 5.3 Rules

- Icons never appear without a label except in: the collapsed rail, table row
  action buttons, and the window toolbar — and all three carry tooltips and
  AccessKit labels.
- One icon per concept, application-wide. The mapping below is normative.
- Status icons are shape-distinct so they survive greyscale.

### 5.4 Concept → icon map

| Concept | Lucide name | Notes |
|---|---|---|
| Dashboard | `layout-dashboard` | |
| Job | `repeat` | |
| Destination | `hard-drive` | |
| Local repository | `hard-drive` | |
| OneDrive repository | `cloud` | |
| S3 bucket | `database` | |
| Folder mirror | `folder-sync` | |
| Storage provider | `key-round` | |
| Restore | `history` | |
| Activity | `list` | |
| Settings | `settings` | |
| About | `info` | |
| Source folder | `folder` | |
| Exclusion | `filter-x` | |
| Schedule | `clock` | |
| Bandwidth | `gauge` | |
| Retention | `archive` | |
| Hook | `terminal` | |
| Run now | `play` | |
| Stop | `square` | Filled square, not a circle-x |
| Pause | `pause` | |
| Resume | `play` | |
| Vault locked | `lock` | |
| Vault unlocked | `lock-open` | |
| Success | `check-circle-2` | |
| Warning / attention | `alert-triangle` | |
| Failure | `x-octagon` | Octagon, not a circle — distinct silhouette from success |
| Running | `refresh-cw` | Rotated 0.75 rev/s while active |
| Skipped | `minus-circle` | |
| Queued | `clock-4` | |
| Verify / test connection | `plug-zap` | |
| Copy to clipboard | `copy` | |
| Reveal secret | `eye` / `eye-off` | |
| Open folder | `folder-open` | |
| Delete | `trash-2` | |
| Edit | `pencil` | |
| Add | `plus` | |
| External link | `external-link` | |
| Diagnostics | `stethoscope` | |
| Remote config | `git-branch` | |

---

## 6. Elevation and motion

### 6.1 Shadows (`epaint::Shadow`)

| Level | Offset | Blur | Spread | Colour (dark) | Colour (light) | Used by |
|---|---|---|---|---|---|---|
| 0 | — | — | — | — | — | Cards, panels, banners (border only) |
| 1 | (0, 4) | 16 | 0 | `#000000` @ 45% | `#171B21` @ 14% | Popup menus, combo dropdowns, tooltips |
| 2 | (0, 10) | 40 | −4 | `#000000` @ 55% | `#171B21` @ 20% | Modals |

### 6.2 Motion

| Motion | Duration | Curve | Notes |
|---|---|---|---|
| Hover / press colour | 110 ms | `animate_bool_with_time` (linear-ish) | Applied via token lerp |
| Focus ring appear | 0 ms | — | Instant. Never animate focus. |
| Collapsing section | 140 ms | egui default | `CollapsingHeader` |
| Progress bar value | 220 ms lerp toward target | linear | Damps kopia's noisy per-file stream (L14) |
| Indeterminate progress | 1600 ms cycle | linear sweep | A 30%-wide band traversing the track |
| Toast enter | 140 ms | fade + 8px rise | |
| Toast exit | 180 ms | fade | |
| Tray running arc | 12 frames × 80 ms | linear | ≈1 revolution / second |
| Everything else | 0 ms | — | No page transitions, no slide-in panels, no parallax. |

If the OS reports "reduce motion" (`eframe` exposes this on Windows and macOS),
all durations above collapse to 0 ms except the progress value lerp, and the
tray running icon becomes a static 3-frame cycle at 500 ms.

---

## 7. The five tray icons

The tray mark is the most-seen part of this application, so it is **the
application mark** — the interlock from `assets/icons/superbackup.svg`, in a
single ink — with a status badge on it. People find a program in a crowded
notification area by its icon; Dropbox, Docker Desktop and OneDrive all show
the brand mark with status layered onto it, and for the same reason.

> **What this replaced, and why.** §7 previously specified an abstract ring
> with a status pip. It encoded five states well and carried no brand identity
> at all — it looked nothing like the application icon, so the one place the
> program is seen all day did not say which program it was. The reasoning
> behind it was sound but stopped a step short: five states must stay
> distinguishable at 16 px *and* under macOS's monochrome template rendering,
> and a brand mark alone cannot encode status. The conclusion drawn was that
> the mark had to be abandoned; the conclusion available was that it had to be
> **combined** with a status indicator.

It is specified here at implementation precision.
`crates/app/src/tray/icons.rs` generates it at run time and
`tools/icons/geometry.py` writes the reference SVGs in `assets/tray/`; the two
are checked against each other pixel for pixel by
`the_checked_in_reference_svgs_match_what_the_program_draws`.

### 7.1 Shared geometry

Design canvas **32 × 32** units (the 2× raster of a 16 pt tray slot). One
canvas unit is half a pixel at 16 px, which is the number every constant below
was chosen against.

**The mark.** The same drawing as `superbackup-mono.svg`: one rounded square
cut along a stepped line into two congruent halves, the lower half slid
down-right by the kerf so the cut opens. The second piece is the first rotated
180°; the translation is the only thing that opens the cut, so the kerf is
uniform along every segment of it without a hand-placed coordinate.

| | Units | At 16 px |
|---|---|---|
| Bounding square | **24.5**, top-left at (0.7, 0.7) | 12.25 px |
| Kerf | **3.1** | 1.55 px |
| Corner radius | 0.20 × side | |
| Cut step | 0.30 × side | |
| Inner corner radius | 0.045 × side | |
| Thinnest limb (the bar above the cut) | 4.28 | 2.14 px |

**The badge.** One bold glyph in a circular well knocked out of the mark's
bottom-right corner — the corner the copied half slides towards, so the mark
already points at it.

| | Units | At 16 px |
|---|---|---|
| Centre | (**24.5**, **24.5**) | |
| Glyph radius (every glyph is inscribed in this) | **6.8** | 6.8 px across |
| Stroke | **3.4** | 1.70 px |
| Stroke, diagonals | **4.08** (3.4 × 1.20) | 2.04 px |
| Clear space, knocked out of the mark's alpha | **2.8** | 1.40 px |

Diagonals are drawn 20 % heavier because a diagonal spreads its coverage over
two pixel columns and reads lighter than an axis-aligned stroke of the same
width. `failed` is a diagonal cross, and `failed` must never be the faintest
thing on the taskbar.

The clear space is a **real cutout in the alpha**, not a stroke painted in "the
taskbar colour": such a stroke is wrong the moment the user changes their
accent, and always wrong on a Linux panel whose colour we did not guess.

Inscribing every glyph in a single radius is what makes the clear space true
for all five states without measuring each shape.

The composition spans 0.7 … 31.3 in both axes — the same margin on all four
sides — so the icon fills its slot the way the shell's own icons do.

#### Two invariants, both found by rasterising

Neither of these is an aesthetic preference. Both were found by rendering and
looking, and both are now tests.

1. **The mark must stay two pieces.** The badge's well is bitten out of the
   lower half's inner corner. Reach too far and that half is cut in two, the
   mark reads as three unrelated blocks, and "one square, cut in two, the
   second piece slid clear" is gone. A 25.5-unit mark is connected in the
   vector and **three fragments once rasterised at 16 px and 20 px**. 24.5 is
   the largest span that holds. → `the_mark_is_two_pieces_at_every_size`

2. **The badge must not touch the mark.** In the macOS template the mark and
   the badge are the same black, and only the gap tells them apart. A 2.4-unit
   clear space is not enough: at 16 px `running`, `paused` and `failed` fuse
   into the mark. 2.6 is the threshold; **2.8** is used.
   → `the_badge_never_touches_the_mark`

The clear space is therefore 1.4 px at 16 px, under the 1.5 px floor below —
deliberately, because it is a *gap*, not a stroke. A stroke has to show its
shape; a gap only has to break contact. Taking it to 3.0 works too, but only by
shrinking the mark from 24.5 units to 21.7 to keep invariant 1 — a tenth of the
mark's width paid for a gap that is already sufficient.

#### The 1.5 px floor

**Nothing is drawn thinner than 3.0 units — 1.5 px at 16 px.** The previous
design put a glyph stroke at 2.4 units, which is 1.2 px, and `attention` and
`failed` became the same smear. Every feature is measured against the floor by
`no_feature_falls_below_the_stroke_floor`, which is not decorative: it rejected
the first `paused`, whose two bars had a 2.86-unit gap between them, and two
bars that merge are one bar.

#### One drawing, not two

The previous design needed a `LARGE` profile for 24 px and up and a `SMALL` one
for 16 and 20, because its proportions did not survive 16 px. This one is
*drawn* at the floor, so **the same geometry is used at every size** and there
is no profile switch to get wrong.

**The renderer must rasterise at the size the tray will display.** Handing
Windows a 32 px bitmap to shrink throws the floor away and fuses the two halves
back together. The tray reads `GetSystemMetrics(SM_CXSMICON)` and re-rasterises
on a DPI change.

### 7.2 The five states

The state is carried by the **badge's silhouette** — never by an interior
detail, and never by colour. An interior detail is precisely what failed
before: a 2.4-unit glyph inside a 6-unit pip is 1.2 px at 16 px.

| `Health` | Asset stem | Badge | What makes it unmistakable | Ink, light taskbar | Ink, dark taskbar |
|---|---|---|---|---|---|
| `Idle` | `idle` | a filled disc, r = 5.98 | solid, with no interior structure at all | `#12793B` §2.2 success | `#4FBF6B` §2.1 success |
| `Running` | `running` | the same circle opened into a ring, stroke 3.4, with a 110° gap that advances 30° a frame over 12 frames | the only badge with a hole in the middle | `#155FCC` §2.2 info | `#5B9BFF` §2.1 accent |
| `Attention` | `attention` | an equilateral triangle inscribed in r = 6.8 — 1.73 r across by 1.5 r tall | the only badge with a flat base and a point | `#8A5B00` §2.2 warning | `#E0A83A` §2.1 warning |
| `Paused` | `paused` | two bars, stroke 3.4, 3.30 either side of centre, 7.40 tall | the only badge that is not one connected shape | `#5E6774` §2.2 neutral | `#8B93A5` §2.1 neutral |
| `Failed` | `failed` | a cross, stroke 4.08, reaching 4.76 diagonally | the only badge made of diagonals | `#B3242B` §2.2 danger.fill | `#FF6B72` §2.1 danger |

`Idle` and `Running` are deliberately the same circle: whole, then opened and
turning. The mark ink is `#22262E` on a light taskbar and `#E8ECF2` on a dark
one, in every state.

The running animation is carried by **alpha** — the ring's gap travels — so it
survives the macOS template, where colour is discarded. It cannot seal a
silhouette another state relies on, because the ring's hole is what
distinguishes `running` from `idle` and the gap only moves along the ring.

**The well is knocked out in all five states, including `idle`.** `Idle` could
have kept its corner and shown the mark whole, and that was drawn and rejected:
it makes `idle` — the state the taskbar is in nearly all the time — visibly
smaller than the other four, and it means the five marks are not the same
picture outside the badge. Knocking it out everywhere is what lets
`the_base_mark_is_common_to_all_five_states` compare them pixel for pixel.

> **Colour is confirmation, never the message,** because macOS strips it (§7.4)
> and roughly one man in twelve has a colour-vision deficiency. Every ink above
> is variant-aware and clears SC 1.4.11's 3:1 on the taskbar it is drawn on;
> the worst of the ten is 4.95:1. The previous design fixed `#E0A83A` and
> `#C2313A` for both taskbars, which are **1.92:1** on a light one and
> **2.94:1** on a dark one — and the second was the failure state. Measured
> figures are in `assets/tray/README.md`, printed by
> `python tools/icons/build.py --contrast`.

### 7.3 Precedence

The icon shown is `StatusSnapshot::derive_health(...)`:
`Failed` > `Running` > `Paused` > `Attention` (locked vault or stale job) >
`Idle`. The GUI must never compute this independently.

### 7.4 Per-platform assets

| Platform | Format | Sizes | Colour handling |
|---|---|---|---|
| Windows | rendered at run time | 16, 20, 24, 32 — whichever `SM_CXSMICON` reports | Two variants: `-light` (mark ink `#22262E`) and `-dark` (`#E8ECF2`), each with its own badge inks. The app watches `SystemUsesLightTheme` and swaps. |
| macOS | Template image | 18 (@1x), 36 (@2x) | **Pure black + alpha, no colour.** macOS inverts it for dark menu bars. Both the state and the running animation are conveyed entirely by silhouette and alpha. |
| Linux (SNI / AppIndicator) | PNG | 22, 24, 32, 48 | Full colour, dark-taskbar variant. Falls back to the 24 px asset if the panel does not report a size. |

The running frames are `running-{00..11}-<variant>`.

Source of truth is `crates/app/src/tray/icons.rs`, which draws every mark at
run time. `assets/tray/<stem>.svg` are reference copies for a human to look at,
generated from the same numbers by `tools/icons/geometry.py` and asserted equal
to what the program draws.

### 7.5 Tooltip

Hovering the tray icon shows two lines (one line on Linux panels that do not
support multi-line):

```
superbackup — <Health::title()>
<contextual second line>
```

Second line by state: Idle → `Next run <relative time>`; Running →
`<job name> — <percent>%`; Attention → `<reason>` (locked vault takes
precedence over stale jobs); Paused → `Paused until <time>` or
`Paused until you resume`; Failed → `<job name> failed <relative time>`.

---

## 8. Component inventory

Every component is specified with its geometry and every state. States are:
**rest, hover, active (pressed), focused, disabled, loading** (where
applicable), plus component-specific states.

### 8.1 Button

Height **30px** (compact **26px** in table rows and toolbars; **36px** for the
single primary action on onboarding screens). Radius 6. Padding 12px
horizontal, plus 8px between a leading icon and the label. Label `body.strong`
(14/500), or `small.strong` in compact.

| Variant | Rest | Hover | Active | Focused | Disabled |
|---|---|---|---|---|---|
| **Primary** | fill `accent.fill`, 1px `accent.fill.border`, text `text.oncolor` | fill `accent.fill.hover` | fill `accent.fill.active` | + 2px `border.focus` ring, 2px outside | fill `bg.raised`, 1px `border.subtle`, text `text.disabled`, no pointer change |
| **Secondary** | fill `bg.raised`, 1px `border.control`, text `text.primary` | fill `bg.surface.hover`, border `border.focus` @ 60% | fill `bg.canvas` | + focus ring | fill `bg.raised` @ 50%, text `text.disabled` |
| **Ghost** | no fill, no border, text `text.secondary` | fill `bg.raised`, text `text.primary` | fill `bg.canvas` | + focus ring | text `text.disabled` |
| **Danger** | fill `danger.fill`, 1px `danger.fill.border`, text `#FFFFFF` | fill `danger.fill.hover` | fill `danger.fill.active` | + focus ring | as Primary disabled |
| **Danger ghost** | no fill, text `danger.mark` | fill `danger.tint.bg`, text `danger.tint.text` | fill `danger.tint.bg` darkened 6% | + focus ring | text `text.disabled` |

**Loading state** (used by Test connection, Verify, Create repository): the
label is replaced by the same label plus a trailing 14px `refresh-cw` rotating
at 0.75 rev/s; the button is `disabled` for interaction but keeps its rest
colours, and `WidgetInfo` reports `Role::Button` with the label suffixed
`", busy"`. Buttons never change width when entering the loading state —
reserve the icon's 22px at layout time.

**Icon-only button**: 30 × 30, radius 6, ghost variant, mandatory tooltip and
AccessKit label.

### 8.2 Text input (`TextEdit`)

Height **30px** (multiline: 3 rows = 72px, resizable to 10 rows). Radius 6.
Padding 10px horizontal, text `body`, mono variant uses `mono` (13px).

| State | Fill | Border | Text |
|---|---|---|---|
| Rest | `bg.input` | 1px `border.control` | `text.primary` |
| Hover | `bg.input` | 1px `border.control` lightened 8% | — |
| Focused | `bg.input` | 2px `border.focus`, inset (inputs are the one exception to the outside-ring rule: `border.focus` on `bg.input` is 8.27:1 dark / 5.48:1 light, so an inset ring meets 1.4.11) | — |
| Disabled | `bg.raised` @ 50% | 1px `border.subtle` | `text.disabled` |
| Error | `bg.input` | 2px `danger.mark` | `text.primary` |
| Placeholder | — | — | `text.muted` |

**Sub-parts**
- **Label**: `h3` (14/600), 6px above the field.
- **Helper text**: `small`, `text.muted`, 4px below, wraps to field width.
- **Error text**: `small`, `danger.tint.text`, replaces helper text, preceded by
  a 14px `alert-triangle`. Announced via `WidgetInfo` on the field itself, not
  as a separate label, so screen readers hear it while focused.
- **Unit suffix** (kbps, MB, minutes, days): `small`, `text.muted`, drawn inside
  the field, right-aligned, 10px from the edge; the text cursor is clamped to
  stop before it.
- **Character counter**: only on Name fields (max 64), shown as `small`
  `text.muted` right-aligned under the field once past 48 characters.

**Path field**: input + 30px `Browse…` secondary button, 8px gap. The input is
`mono`, elides in the middle (L3), and shows the full path on hover. A
`folder-open` icon button (30 × 30, ghost) appears to the right when the path
exists on disk.

**Passphrase field**: `TextEdit::password(true)`, plus a 30 × 30 ghost
`eye`/`eye-off` toggle inside the right edge. Revealing is never persistent: the
field re-masks on blur, on window focus loss, and after 15 seconds. Paste is
allowed; copy from a passphrase field is blocked except on the generated
passphrase screen (§ `UX_SPEC` T-5).

### 8.3 Combo box / dropdown

Height 30px, radius 6, `bg.input`, 1px `border.control`, trailing 16px
`chevron-down` in `text.secondary`. Popup: `bg.raised`, radius 14, shadow 1,
4px padding, items 28px tall with 10px horizontal padding, selected item shows
a leading 16px `check` and `bg.selected`. Max popup height 320px, then scrolls.
Type-ahead: typing jumps to the first item with that prefix.

### 8.4 Checkbox, radio, toggle

| Component | Geometry | Rest | Checked | Focus | Disabled |
|---|---|---|---|---|---|
| Checkbox | 16 × 16, radius 4 | `bg.input`, 1px `border.control` | fill `accent.fill`, white 12px `check` | 2px ring outside | `bg.raised`, `border.subtle` |
| Radio | 16 dia | `bg.input`, 1px `border.control` | 1px `accent`, inner disc r = 4 in `accent` | 2px ring outside | as checkbox |
| Toggle | 36 × 20, radius 10, knob 16 dia | track `bg.raised`, 1px `border.control`, knob `text.secondary` | track `accent.fill`, knob `#FFFFFF` | 2px ring outside | track `bg.raised` @ 50% |

Label sits 8px to the right, `body`, vertically centred, and is part of the
click target. Helper/rationale text (used heavily by the exclusion presets) is
`small` `text.muted` on the following line, indented to align with the label.

**Toggle vs checkbox**: toggles are used for settings that take effect
immediately; checkboxes are used inside forms that have a Save action. This
distinction is enforced across the spec.

### 8.5 Segmented control

Used for Job editor tabs and the Activity time-range picker. Height 30px, radius
6, container `bg.raised` with 1px `border.subtle`, 3px inner padding. Each
segment: `small.strong`, 12px horizontal padding. Selected segment: `bg.surface`
in dark / `#FFFFFF` in light, 1px `border.control`, text `text.primary`.
Unselected: no fill, text `text.secondary`. Keyboard: ←/→ moves selection,
Home/End jump.

### 8.6 Card

| Property | Value |
|---|---|
| Background | `bg.surface` |
| Border | 1px `border.subtle` |
| Radius | 10 |
| Padding | 16 |
| Header | `h2`, 20px icon, 12px below |
| Gutter between cards | 16 (vertical), 16 (horizontal in grids) |
| Hover (interactive cards only) | fill `bg.surface.hover`, border `border.control` |
| Focused | 2px `border.focus` ring outside |
| Selected | 1px `accent`, plus 3px `accent` bar on the left edge inset by 1px |

**Job card** (dashboard): fixed **96px** tall, full column width. Internal
layout: 3px status bar down the left edge in the status mark colour; 16px
padding; row 1 = job name (`h2`) + status badge + `⋯` menu button; row 2 =
`small` `text.muted` meta line; row 3 = either the destination chip row (idle)
or the progress bar + throughput line (running).

**Status bar rule**: the 3px left edge bar is the *only* place a status colour
touches the card, so a wall of cards reads as a calm grid with a colour spine.

### 8.7 Table

`egui_extras::TableBuilder`. Container: `bg.surface`, 1px `border.subtle`,
radius 10, clipped.

| Part | Spec |
|---|---|
| Header row | 32px, `bg.raised`, `micro` in `text.muted`, 1px `border.strong` bottom, sticky |
| Body row | **36px** standard, **28px** compact (snapshot browser, log) |
| Row divider | 1px `border.subtle`, inset 12px from both ends |
| Cell padding | 12px horizontal |
| Row hover | `bg.surface.hover` |
| Row selected | `bg.selected`, text stays `text.primary` |
| Row focused | 2px `border.focus` inset ring on the row rect |
| Zebra striping | **None.** Row dividers only. |
| Sort indicator | 12px `chevron-up`/`chevron-down` after the header label, `text.secondary`; the sorted column header is `text.primary` |
| Empty | see §8.13 |

Column widths are declared per table in `UX_SPEC.md` (L5). Right-aligned numeric
cells use `mono.small` (L9). Row action buttons (26px compact ghost) appear on
hover and on keyboard focus — never hover-only, or they are unreachable by
keyboard.

**Keyboard**: ↑/↓ move the focused row, Enter opens it, Space toggles selection
where multi-select exists, Ctrl/Cmd+A selects all, Escape clears selection.

### 8.8 Progress

**Bar**: height 6 (in cards) or 8 (in run detail). Radius = height / 2. Track
`progress.track`, fill `progress.fill`. Fill has a minimum visible width of 3px
once fraction > 0, so "just started" is visible.

| State | Appearance |
|---|---|
| Determinate | Fill to `Progress::fraction()`. Value lerped over 220ms. |
| Indeterminate (`fraction() == None`, i.e. kopia is still estimating) | A 30%-wide band sweeping left→right over 1600ms, plus the label "Estimating…" |
| Warning | Fill `progress.fill.warn` — used when `errors_ignored > 0` |
| Error | Fill `progress.fill.error`, frozen at last value |
| Complete | Fill 100% in `success.mark` for 1.5s, then the bar is replaced by the result row |

Every bar is accompanied by a text line and both are announced (L7):
`"<files_processed> of <files_total> files · <bytes_processed> of <bytes_total> · <bytes_per_second>/s"`.
When `bytes_total` is `None`, the counts drop out and only processed values show.

**Per-destination progress**: a job running to three destinations shows three
stacked 6px bars, each 4px apart, each prefixed with a 90px destination name
column. The card's aggregate bar uses `JobRun::overall_fraction()`.

### 8.9 Badge / pill

Height 20px, radius 5, padding 8px horizontal, 6px gap to a leading 14px icon,
label `small.strong`. Background = status tint bg, text = status tint text.

| Badge | Icon | Tint | Text (see `COPY.md`) |
|---|---|---|---|
| Succeeded | `check-circle-2` | success | `Succeeded` |
| Completed with warnings | `alert-triangle` | warning | `Warnings` |
| Failed | `x-octagon` | danger | `Failed` |
| Running / Preparing / Finalising | `refresh-cw` (rotating) | info | `Running` / `Preparing` / `Finalising` |
| Queued | `clock-4` | neutral | `Queued` |
| Cancelled | `minus-circle` | neutral | `Cancelled` |
| Skipped | `minus-circle` | neutral | `Skipped` |
| Disabled (job/destination) | `pause` | neutral | `Disabled` |
| Never run | — | neutral | `Never run` |

**Destination chip** (used on job cards and in the job editor): 24px tall,
radius 5, `bg.raised`, 1px `border.subtle`, 14px kind icon, `small` label, and
— when the last run to that destination failed — a 6px `danger.mark` dot at the
chip's top-right.

**Count pill** ("used by 3 destinations"): 20px, `bg.raised`, `text.secondary`,
no icon.

### 8.10 Banner

Full-width in-content notice. Height auto, min 44px. Radius 10, 1px border in
the status mark colour at 40% alpha, background status tint bg, 16px padding,
20px leading icon, `body` text, optional trailing buttons (ghost, compact).

Four kinds: `info`, `warning`, `danger`, `success`. Banners are never
dismissible when they describe a persistent state (locked vault, paused,
kopia missing); they are dismissible when they describe an event (config pulled
from GitHub).

**Locked-vault banner** and **paused banner** are pinned directly under the
header bar on every screen and push content down. They are the only two banners
that can appear simultaneously; locked sorts above paused.

### 8.11 Toast

Bottom-right of the content area, 16px from the edges, stacked upward with 8px
gaps, maximum **3** visible; a fourth replaces the oldest. Width 360px (or
`content_width - 40` if smaller), min height 52px, radius 10, `bg.raised`,
1px `border.control`, shadow 1, 12px padding, 16px leading status icon.

- Title `body.strong`, optional body `small` `text.secondary` (max 2 lines,
  then elided).
- Auto-dismiss: success 4s, info 5s, warning 8s. **Danger toasts never
  auto-dismiss.**
- Hovering pauses the timer; moving away resumes it.
- Optional single action link (`small.strong`, `text.link`) right-aligned, plus
  a 20 × 20 ghost `x` close button.
- Toasts are announced once via a live-region label (L7); repeated identical
  toasts within 60s are suppressed and the existing toast's timer resets.

### 8.12 Modal

`egui::Modal` with a `bg.scrim` overlay. Width by size class: **small 420px**,
**medium 560px**, **large 760px**; capped at `window_width − 80`. Max height
`window_height − 96` with an internal `ScrollArea`. Radius 14, `bg.surface`,
1px `border.strong`, shadow 2.

| Region | Spec |
|---|---|
| Header | 56px, 20px padding, `h2` title, optional 24px icon, ghost `x` at the right (omitted for blocking modals such as Unlock) |
| Body | 20px padding, `body` |
| Footer | 60px, 20px padding, 1px `border.subtle` top, buttons right-aligned, 8px apart, cancel-style on the left of the primary |

**Rules**: Escape cancels (except blocking modals). Enter triggers the primary
action only when focus is not in a multiline field. Focus is trapped and moves
to the first interactive element on open, and returns to the invoking control on
close. Never nest modals (L13).

**Destructive confirmation modal**: small size, `alert-triangle` in
`danger.mark`, body states exactly what will be deleted and what will not,
primary button is Danger variant and carries the verb ("Delete job"), never
"OK". For **irreversible data loss** (deleting a repository's contents,
rotating a repository passphrase, resetting the vault) the modal adds a
confirmation `TextEdit` requiring the object's exact name; the primary button
stays disabled until it matches, and the match is case-sensitive.

### 8.13 Empty state

Centred in the available area, max width 420px. 32px icon in `text.muted`, 16px
gap, `h2` title in `text.primary`, 8px gap, `body` in `text.secondary` (max 2
lines), 20px gap, one Primary button and at most one Ghost secondary. Vertically
centred at 45% of the container height, not 50% — it reads better under a
header bar. Strings are in `COPY.md` §4.

### 8.14 Tooltip

`bg.raised`, 1px `border.control`, radius 6, shadow 1, 8px padding, `small`,
max width 280px, 500ms delay for informational tooltips and 0ms for
truncation tooltips (which merely restore hidden text). Never contains an
interactive element.

### 8.15 Navigation rail item

Height 36px, full rail width, 12px horizontal padding, 16px icon, 12px gap,
`body` label. Selected: `rail.item.selected.bg`, `body.strong`,
`rail.item.selected.marker` as a 3px full-height bar on the left edge. Hover:
`bg.raised`. Focus: 2px ring inset 2px. Collapsed (64px rail): icon centred, no
label, tooltip mandatory.

A rail item may carry a **status dot** (6px, right-aligned, 12px from the edge)
when its section needs attention: Destinations shows a `danger` dot when any
destination failed verification; Activity shows a `danger` dot when there are
unread failures since the user last opened it.

### 8.16 Key/value list

Used in run detail, destination detail, About. Label column fixed at **160px**,
`body` `text.secondary`, right-aligned to its column edge minus 16px; value
column `remainder`, `body.strong` `text.primary` (or `mono` for paths, IDs,
endpoints). Row height 28px, no dividers, 4px vertical gap between rows.

### 8.17 Passphrase strength meter

Custom painter. Track 6px tall, full field width, radius 3, four segments with
2px gaps. Segments fill left-to-right in the band colour:

| Score (zxcvbn-style 0–4, computed locally) | Segments lit | Colour | Label |
|---|---|---|---|
| 0–1 | 1 | `danger.mark` | `Too weak` |
| 2 | 2 | `warning.mark` | `Weak` |
| 3 | 3 | `info.mark` | `Good` |
| 4 | 4 | `success.mark` | `Strong` |

Label sits to the right of the meter, `small.strong`, in the band colour, and is
part of the meter's `WidgetInfo` announcement (L7) so it is not colour-only. The
meter never blocks submission on its own — the minimum policy is stated in
`UX_SPEC.md` O-2.

### 8.18 Code / log block

`bg.code`, 1px `border.subtle`, radius 6, 12px padding, `mono.small`, no line
wrapping (horizontal `ScrollArea`), max height 240px then vertical scroll. A
26px ghost `copy` button floats at the top-right inset 8px. Log severity is
carried by a 3px left bar in the severity mark colour, not by tinting the text.

---

## 9. Focus, keyboard, and screen readers

### 9.1 Global keyboard map

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Move focus in the order declared per screen (L6) |
| `Ctrl/Cmd + 1…7` | Jump to rail item 1–7 |
| `Ctrl/Cmd + N` | New job (from anywhere) |
| `Ctrl/Cmd + R` | Run the selected/current job now |
| `Ctrl/Cmd + F` | Focus the search field on the current screen |
| `Ctrl/Cmd + L` | Lock the vault now |
| `Ctrl/Cmd + ,` | Settings |
| `Ctrl/Cmd + W` | Hide the window to the tray (does not quit) |
| `F5` | Refresh status from the daemon |
| `Escape` | Close modal / clear search / clear selection, in that order |
| `Enter` | Activate the focused control; on a table row, open it |
| `Space` | Toggle the focused checkbox/toggle; on a table row, select it |
| `Alt+←` / `Alt+→` | Back / forward in the Restore browser breadcrumb |

Every keyboard shortcut is also reachable through a visible control. No
shortcut-only functionality.

### 9.2 Focus rules

- Focus order is declared per screen and matches visual reading order.
- Opening a modal moves focus into it and traps it; closing restores focus.
- After a destructive action, focus moves to the list that contained the deleted
  item, not to nothing.
- The focus ring is 2px `border.focus`, drawn outside the control (inside for
  text inputs, §8.2), radius = control radius + 2, never animated.
- egui's "click to focus" default is kept, but every interactive widget is also
  reachable by Tab. Custom painters that are interactive must call
  `ui.interact(...)` with `Sense::click()` so they enter the focus chain.

### 9.3 AccessKit labels

Rules for `WidgetInfo`:

1. Every control has a label that makes sense **without** its visual context.
   `Run now` becomes `Run job "Dev code" now`.
2. Icon-only controls get the label, not the icon name.
3. Status is spoken as words: a job card announces
   `"Dev code, succeeded, last run 2 hours ago, next run in 4 hours, 3 destinations"`.
4. Progress announces as
   `"Backing up Dev code to Local repo, 42 percent, 12,400 of 29,000 files"`,
   throttled to one announcement per 10 seconds per run.
5. Errors are announced on the field that owns them, and the form's Save button
   announces `", 2 problems to fix"` while invalid.
6. The locked-vault banner is an `alert` role and is announced on appearance.
7. Tables expose `Role::Table` with row/column counts, and each row exposes its
   full contents in reading order — the row label is the concatenation of its
   cells separated by ", ".
8. Live-region announcements (toasts, run completion) go through a single
   dedicated hidden label that is updated at most once per second.

---

## 10. Formatting rules

Consistent formatting is part of the visual system.

| Kind | Rule | Examples |
|---|---|---|
| Bytes | `bytesize` binary units, 1 decimal below 10, 0 above | `842 MB`, `1.4 GB`, `12 GB` |
| Rates | Same, per second | `18.2 MB/s` |
| Durations | Largest two units, no seconds above an hour | `4m 12s`, `1h 06m`, `2d 3h` |
| Relative time (past) | `just now`, `2 minutes ago`, `4 hours ago`, `yesterday 02:00`, `12 Mar 02:00` | Switches to absolute after 48 hours |
| Relative time (future) | `in 12 minutes`, `in 4 hours`, `tomorrow 02:00`, `Mon 02:00` | Switches to absolute after 24 hours |
| Absolute time | Local time, 24-hour, `DD MMM HH:MM`; the year is added only when it is not the current year | `12 Mar 02:00`, `12 Mar 2025 02:00` |
| Counts | Thousands separator per locale, mono in tables | `1,204,882 files` |
| Percent | Integer, no decimals | `42%` |
| Paths | Native separators, middle-elided (L3), `mono` | `C:\Users\andreas\…\web\src` |
| UUIDs | First 8 characters, `mono.small`, full value in tooltip | `a3f9c2d1…` |
| Snapshot ids | First 12 characters, `mono.small` | `k9f2ab7c31de…` |
| Never shown | Full secrets, access keys, passphrases (except on T-5), raw kopia argv | |

All timestamps are rendered in **local time**; the model stores UTC. Any place
the distinction could matter (Activity, run detail) shows the offset in the
tooltip.

---

## 11. Theming implementation notes

- Tokens live in `crates/app/src/theme.rs` as `struct Tokens { … }` with
  `Tokens::dark()` and `Tokens::light()` and are stored in `egui::Context`
  memory, retrieved once per frame.
- `egui::Visuals` is derived from the tokens so that stock widgets
  (`Slider`, `ScrollArea`, `CollapsingHeader`) match without per-widget work.
- `Theme::System` uses `eframe`'s system-theme reporting; a theme change
  rebuilds `Visuals` and re-rasterises the icon atlas only if DPI also changed.
- Colour values are authored as `Color32::from_rgb` constants; no runtime hex
  parsing.
- There is exactly one place in the codebase where a `Color32` literal is
  allowed: `theme.rs`. A CI grep enforces this.
