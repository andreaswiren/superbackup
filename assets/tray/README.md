# Tray icons

These files are [`design/DESIGN_SYSTEM.md`](../../design/DESIGN_SYSTEM.md) §7,
drawn. §7 remains the specification; nothing here is a design decision.

They are **reference artefacts**, for a human to look at and check §7 against.
The running application does not read them — `crates/app/src/tray/icons.rs`
generates the same geometry from the same constants at run time, because five
states × three variants × twelve running frames is around fifty files to keep
in sync by hand.

That the two agree is no longer a claim. `tools/icons/geometry.py` mirrors the
Rust module constant for constant, and
`the_checked_in_reference_svgs_match_what_the_program_draws` rasterises every
file here alongside what `tray/icons.rs` draws and compares the pixels at 16 px
and 32 px. It exists because the two *did* diverge once — over whether the
macOS template's glyph was painted or punched — and that divergence was found
by reading, which is not a method.

Regenerate every file here with:

```bash
python tools/icons/build.py
```

Open [`preview.html`](preview.html) afterwards. The greyscale rows are the
point of that page: they are what catches a state that has started depending on
colour.

## The mark

**Every tray icon is the application mark with a status badge on it.**

The mark is the interlock from
[`../icons/superbackup-mono.svg`](../icons/README.md) — one rounded square cut
along a stepped line into two congruent halves, the lower one slid down-right
so the cut opens into a kerf — drawn in a single ink at 24.5 units on a 32-unit
canvas. A circular well is knocked out of its bottom-right corner, and one bold
glyph sits in that well.

This replaced an abstract ring with a status pip, which encoded the five states
well and carried no brand identity at all. §7's "What this replaced, and why"
has the reasoning.

## The five states

One per variant of `superbackup_core::state::Health`. The state is carried by
the **badge's silhouette**, never by an interior detail and never by colour —
macOS strips colour from a template image entirely, and roughly one man in
twelve has a colour-vision deficiency.

| `Health` | Badge | What makes it unmistakable at 16 px |
|---|---|---|
| `Idle` | a filled disc | solid, with no interior structure at all |
| `Running` | the same circle opened into a ring, with a gap that travels round it | the only badge with a hole in the middle |
| `Attention` | a triangle | the only badge with a flat base and a point |
| `Paused` | two bars | the only badge that is not one connected shape |
| `Failed` | a cross | the only badge made of diagonals |

`Idle` and `Running` are the same circle on purpose: whole, then opened and
turning.

## Files

`<state>-<variant>.svg`, matching `IconKey::stem()` in `tray/icons.rs` exactly,
so a file here can be compared against the mark the program builds without
translating a name.

| Pattern | Count | Use |
|---|---|---|
| `<state>-light.svg` | 5 | Windows light taskbar. Mark ink `#22262E`. |
| `<state>-dark.svg` | 5 | Windows dark taskbar, and the Linux default. Mark ink `#E8ECF2`. |
| `<state>-template.svg` | 5 | macOS menu bar. Pure black plus alpha; macOS inverts it for a dark menu bar. |
| `running-{00..11}-<variant>.svg` | 36 | The twelve animation frames, 30° apart, for each variant. |

`Running` has no plain `running-<variant>.svg`: `IconKey::stem()` always
carries the frame index for that state, and these names follow it exactly.
`running-00-<variant>.svg` is the frame to look at when you want one.

The bare `<state>.svg` files that used to sit here are gone. They were
documented as a neutral reference "which is why the repository README embeds
one" — the README embedded no such thing and nothing else read them, so they
were five files kept in sync for nobody.

## Geometry (§7.1)

Canvas 32 × 32. One canvas unit is half a pixel at 16 px, which is the number
every constant was chosen against.

- **Mark** — bounding square 24.5, top-left at (0.7, 0.7); kerf 3.1; corner
  radius 0.20 × side, cut step 0.30 × side, inner corner 0.045 × side.
- **Badge** — centred (24.5, 24.5); every glyph inscribed in a circle of radius
  6.8; stroke 3.4, and 4.08 on diagonals.
- **Clear space** — a disc of radius 6.8 + 2.8 knocked out of the mark's alpha.
  A real cutout, not a stroke painted in the background colour: a stroke in
  "the taskbar colour" is wrong the moment the user changes their accent, and
  wrong always on a Linux panel that is not the colour we guessed.
- The composition spans 0.7 … 31.3 in both axes.

## Rasterisation

SVG is the source. The tray backends need bitmaps, and the application renders
its own at run time with `resvg` **at the size the shell will actually draw**,
read from `GetSystemMetrics(SM_CXSMICON)`. It does not render at 32 px and let
the platform downscale: the mark is drawn at the 16 px floor, and a 3.1-unit
kerf that is 1.55 px at 16 px does not survive being shrunk out of a 32 px
bitmap.

Per §7.4: Windows wants 16/20/24/32 per state per variant, Linux 22/24/32/48,
macOS 18 and 36 as template images.

## What 16 px actually rejected

Everything below was found by rasterising and looking, not by reading — see
[`../icons/preview/tray-contact-sheet.png`](../icons/preview/tray-contact-sheet.png)
and the `pixel_check`, `running_frames` and `contact_sheet` generators in
`crates/app/tests/tray_state.rs`. Each one is now a test, so it cannot come
back quietly.

1. **A 25.5-unit mark is severed by the badge's well.** The well is bitten out
   of the lower half's inner corner, and at 25.5 the neck that survives is
   0.36 units wide. The vector is still connected; the *raster* is not. At
   16 px and 20 px the mark comes out as **three fragments**, so it stops
   reading as two congruent pieces — which is the entire idea of the mark.
   24.5 is the largest span that holds at every size and every alpha
   threshold. → `the_mark_is_two_pieces_at_every_size`

2. **A 2.4-unit clear space lets the badge fuse into the mark.** In the macOS
   template the mark and the badge are the same black and only the gap tells
   them apart. At 2.4 the gap closes at 16 px for `running`, `paused` and
   `failed`. 2.6 is where it stops closing; 2.8 is used, for margin. That is
   1.4 px — under the 1.5 px stroke floor, deliberately, because a gap only has
   to break contact, not show its shape. Reaching 3.0 would cost the mark 2.8
   units of span. → `the_badge_never_touches_the_mark`

3. **`paused`'s bars merged.** Two bars 0.92 × stroke either side of centre
   leave a 2.86-unit gap — 1.43 px — and two bars that merge are one bar. The
   offset is now set by the gap rather than by eye.
   → `no_feature_falls_below_the_stroke_floor`

4. **A diagonal at the same stroke width reads lighter than a vertical.** It
   spreads its coverage over two pixel columns. `failed` is a diagonal cross
   and was the faintest of the five until its stroke was taken to 1.20 × the
   others.

5. **`idle` as a small dot was too close to `attention`.** A disc well inside
   the triangle's own circle differs from it over very little of the badge, and
   at 16 px that is a handful of pixels. `idle`'s disc was grown to 0.88 of the
   badge radius, where it also pairs properly with `running`'s ring.
   → `each_badge_is_a_distinct_silhouette`

6. **`idle` without a badge made `idle` look small.** Leaving the mark whole
   for the idle state is the most brand-forward option and it was drawn first.
   Beside the real 16 px shell icons on a Windows taskbar it reads as a
   noticeably smaller icon than its neighbours, because the badged states fill
   the canvas to 31.3 and an unbadged mark stops at 25.2. All five states are
   badged, and the well is knocked out of all five.

### What still does not survive 16 px

Honesty rather than a footnote:

- **The five states are not readable at a glance at 16 px from across a desk.**
  They are readable when looked at. At 16 px the badge is 6.8 px across, and no
  design puts five distinguishable shapes into 6.8 px that resolve
  peripherally. What the icon does at a glance is say *superbackup*, which is
  what it is for; the state is the second read, and the tooltip is the third.
- **`paused` and `failed` are the closest pair**, differing over 12.8 % of the
  badge region on alpha alone (at 24 px and 32 px; more at 16 and 20). Two
  vertical bars against a diagonal cross is a real distinction and it holds in
  greyscale and in the template, but it is the narrowest margin in the set and
  the first thing to re-measure after any change.
  `attention` against `failed` — the pair the previous design actually lost —
  is 16.9 % at its worst.
  → `each_badge_is_a_distinct_silhouette`,
  `attention_and_failed_are_told_apart_by_shape_at_16px`
- **The two halves of the mark are one ink, not two.** The application icon
  distinguishes `keep` `#F2F5FA` from `copy` `#5B9BFF`; at 16 px that second
  tone drops the copied half to 2.50:1 on a light taskbar. The tray mark is
  monochrome per variant, like `superbackup-mono.svg`, and the kerf alone says
  "two pieces".

### Measured contrast

WCAG 2.1 relative luminance, computed by
`python tools/icons/build.py --contrast`, which reads the values out of
`tools/icons/geometry.py` rather than out of this table — so this table cannot
drift away from what is drawn. An earlier version of this file asserted ratios
that had stopped being true; that is what the generator is for. Non-text
graphics need 3:1 under SC 1.4.11.

| Ink | On `#F3F3F3` | On `#202020` |
|---|---|---|
| mark `-light` `#22262E` | **13.67:1** | — |
| mark `-dark` `#E8ECF2` | — | **13.74:1** |
| `idle` badge `#12793B` / `#4FBF6B` | **4.95:1** | **6.98:1** |
| `running` badge `#155FCC` / `#5B9BFF` | **5.35:1** | **5.88:1** |
| `attention` badge `#8A5B00` / `#E0A83A` | **5.29:1** | **7.63:1** |
| `paused` badge `#5E6774` / `#8B93A5` | **5.16:1** | **5.29:1** |
| `failed` badge `#B3242B` / `#FF6B72` | **5.91:1** | **5.90:1** |
| template `#000000` | **18.93:1** | — |
| template inverted `#FFFFFF` | — | **16.29:1** |

Every ink clears 1.4.11 on the taskbar it is drawn on, and the worst of the ten
badge inks is 4.95:1. That is a change: the previous design fixed one colour
per state for both taskbars, and three of six failed —

| Previous ink | | |
|---|---|---|
| `attention` pip `#E0A83A` | **1.92:1** on a light taskbar | the pip's own edge was invisible |
| `running` arc `#5B9BFF` | **2.50:1** on a light taskbar | |
| `failed` pip `#C2313A` | **2.94:1** on a dark taskbar | and this one is the *failure* state |

#### The badge against the mark

| Pair | Ratio |
|---|---|
| `idle` badge against the mark ink, light / dark | 2.76:1 / 1.97:1 |
| `attention` badge against the mark ink, light / dark | 2.58:1 / 1.80:1 |
| `failed` badge against the mark ink, light / dark | 2.31:1 / 2.33:1 |

These are below 3:1 and it does not matter, for the same reason `keep` against
`copy` does not matter in the application icon: **the two never touch.** The
2.8-unit clear space separates them everywhere, so each is read against the
taskbar, at the ratios in the table above. The pair is listed because a future
change that closed the gap would make these the operative numbers, and that
change should be rejected on this line — and because
`the_badge_never_touches_the_mark` is what keeps the premise true.
