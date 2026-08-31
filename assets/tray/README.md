# Tray icons

These files are [`design/DESIGN_SYSTEM.md`](../../design/DESIGN_SYSTEM.md) §7,
drawn. §7 remains the specification; nothing here is a design decision.

They are **reference artefacts**, for a human to look at and check §7 against.
The running application does not read them — `crates/app/src/tray/icons.rs`
generates the same geometry from the same constants at run time, because five
states × two Windows variants × twelve running frames, plus a template set, is
around fifty files to keep in sync by hand. Both are produced from §7's
numbers, and `tools/icons/geometry.py` mirrors that module line for line so the
two cannot quietly diverge. The one place they deliberately differ is recorded
under *Known divergence* below.

Regenerate every file here with:

```bash
python tools/icons/build.py
```

Open [`preview.html`](preview.html) afterwards. The greyscale rows are the
point of that page: they are what catches a state that has started depending on
colour.

## The five states

One per variant of `superbackup_core::state::Health`.

| `Health` | Silhouette | Meaning |
|---|---|---|
| `Idle` | Closed ring + centre disc | Every job succeeded recently. Nothing to do. |
| `Running` | Open ring, no pip, with a 90° arc travelling round it | At least one job is backing up right now. |
| `Attention` | Open ring + pip containing an exclamation | Nothing failed, but something needs the user: a locked vault, or a job that has not succeeded in a while. |
| `Paused` | Closed ring + two interior bars | The user turned backups off, temporarily or indefinitely. |
| `Failed` | Open ring + pip containing a cross | A job or the service reported a failure. |

Shape carries the state. Colour only confirms it — a hard requirement, because
macOS strips colour from a template image entirely and roughly one man in
twelve has a colour-vision deficiency.

## Files

`<state>-<variant>.svg`, matching `IconKey::stem()` in `tray/icons.rs` exactly,
so a file here can be compared against the mark the program builds without
translating a name.

| Pattern | Count | Use |
|---|---|---|
| `<state>-light.svg` | 5 | Windows light taskbar. Ring ink `#22262E`. |
| `<state>-dark.svg` | 5 | Windows dark taskbar, and the Linux default. Ring ink `#E8ECF2`. |
| `<state>-template.svg` | 5 | macOS menu bar. Pure black plus alpha; macOS inverts it for a dark menu bar. |
| `running-{00..11}-<variant>.svg` | 36 | The twelve animation frames, 30° apart, for each variant. |
| `<state>.svg` | 5 | **Neutral reference only.** Ring ink `#8B93A5`, §7.2's Linux value — the one ink legible on a light *and* a dark page, which is why the repository README embeds one. Not a shipping variant and not something `tray/icons.rs` produces. |

`Running` has no plain `running-<variant>.svg`: `IconKey::stem()` always
carries the frame index for that state, and these names follow it exactly.
`running-00-<variant>.svg` is the frame to look at when you want one.

## Geometry (§7.1)

Canvas 32 × 32, everything centred on (16, 16), round joins and caps.

- **Ring** — circle at (16, 16), radius 10.5, stroke 3.0.
- **Open ring** — the same circle as a 280° arc from 85°, clockwise, leaving an
  80° gap centred on +45° (down-right). 0° is east, y down.
- **Pip** — circle at (23, 23), radius 6.0, filled, with a 1.5-unit knockout
  separating it from the arc. A real cutout, not a background-coloured stroke:
  a stroke painted in "the taskbar colour" is wrong the moment the user changes
  their accent, and wrong always on a Linux panel that is not the colour we
  guessed.
- **Pip glyph** — stroke 2.4 in `pip.ink`.
- Nothing within 1 unit of the edge.

## Rasterisation

SVG is the source. The tray backends need bitmaps, and the application renders
its own at run time with `resvg` at 32 px, letting each platform downscale —
both Windows and Linux downscale a larger bitmap far better than they upscale a
smaller one, and a 16 px source on a 200 % display is exactly the mush these
rules exist to prevent.

Per §7.4: Windows wants 16/20/24/32 per state per variant, Linux 22/24/32/48,
macOS 18 and 36 as template images.

## Known divergence from `tray/icons.rs`

**The macOS template pip glyph.** The code paints the exclamation and the cross
in `#FFFFFF` on a black pip. A template image is *alpha only* — macOS discards
the colour — so a white glyph is fully opaque and disappears into the pip. The
consequence is that `attention-template` and `failed-template` render as the
same picture: an open ring with a plain filled pip. That breaks §7.2's
requirement that shape carries state, in the one place §7.4 says it must.

The files here punch the glyph as a real hole in the pip's alpha, which is what
a template image needs. `tray/icons.rs` has not been changed to match — that is
a code fix to route, not something this directory should decide.

## Where §7 does not survive contact with 16 px

Found by rasterising and looking, not by reading. See
[`../icons/preview/tray-contact-sheet.png`](../icons/preview/tray-contact-sheet.png).

1. **The pip glyph is below the resolution floor at 16 px.** The pip is radius
   6 on a 32 canvas — 3 px at 16 px — and the glyph stroke is 2.4 units, which
   is **1.2 px**. The exclamation's dot is radius 1.3 units, or 0.65 px. Both
   render as a smear. At 16 px in greyscale, `attention` and `failed` are
   separated only by the *lightness* of that smear (`#1A1206` against
   `#FFFFFF`), not by its shape — and in the template variant, where both are
   the same hole, they are indistinguishable. §7.2 states the shape distinction
   as a hard requirement; it holds at 24 px and above and fails at 16 and 20.

2. **The 1.5-unit knockout eats the arc's round caps.** The knockout disc
   (radius 7.5 at (23, 23)) intersects the ring circle over 45° ± 43.0°, i.e.
   2°–88°. The specified gap is 5°–85°. The knockout is therefore *wider* than
   the gap it is meant to clear, and it truncates both round terminals of the
   280° arc into flat crescents — visible at 32 px. For the caps to survive
   intact the gap would need to be ~105° (a 255° sweep), or the pip radius
   would need to be ≤ 4.0. As written, "arc start 85°, sweep 280°" does not
   describe the arc that is visible, and "caps are round" is not true at the
   two ends.

3. **The running base arc is not variant-aware.** §7.2 fixes it at `#3A4250`
   for both Windows variants. That is 9.12:1 on a light taskbar and **1.61:1**
   on a dark one — so on a dark taskbar the ring vanishes and only the moving
   blue arc is visible. `running` loses the shared silhouette that makes the
   set recognisable as one application.

4. **The moving arc closes the open ring on some frames.** The 90° travelling
   arc is drawn on the full circle, so on the frames where it crosses the 80°
   gap the silhouette is momentarily a *closed* ring — which is `idle` and
   `paused`'s distinguishing feature. Minor, because the centre is empty and
   the state is transient, but "the only state with no pip and no centre dot"
   is doing more work than "open ring" for four frames in twelve.

5. **§7.2 and §7.4 give different ring colours.** §7.2 says `#8B93A5` on a dark
   taskbar and `#5E6774` on a light one; §7.4 says ring ink `#22262E` on
   `-light` and `#E8ECF2` on `-dark`. They are different pairs for the same
   thing. The code implements §7.4 and so do these files.

### Measured contrast

WCAG 2.1 relative luminance, computed by
`python tools/icons/build.py --contrast`, which reads the values out of
`tools/icons/geometry.py`. Non-text graphics need 3:1 under SC 1.4.11.

| Ink | On `#F3F3F3` | On `#202020` |
|---|---|---|
| ring `-light` `#22262E` | **13.67:1** | — |
| ring `-dark` `#E8ECF2` | — | **13.74:1** |
| attention pip `#E0A83A` | 1.92:1 | **7.63:1** |
| failed pip `#C2313A` | **4.99:1** | 2.94:1 |
| running base arc `#3A4250` | **9.12:1** | 1.61:1 |
| running arc `#5B9BFF` | 2.50:1 | **5.88:1** |

| Glyph on its pip | |
|---|---|
| attention ink `#1A1206` on `#E0A83A` | **8.68:1** |
| failed ink `#FFFFFF` on `#C2313A` | **5.53:1** |

The ring inks are excellent on the taskbar each was solved for — that part of
§7.4 works exactly as intended. The chromatic values are not variant-aware, and
three of the six fail 1.4.11 on one taskbar or the other:

- `#E0A83A` at **1.92:1** on a light taskbar. The state still reads, because
  the dark glyph inside the pip is at 8.68:1 and does the work — but the pip's
  own edge is invisible, so the silhouette §7.2 depends on is not there.
- `#5B9BFF` at **2.50:1** on a light taskbar.
- `#C2313A` at **2.94:1** on a dark taskbar — marginal, and it is the *failure*
  state.

The design system already has variant-appropriate tokens for all three (§2.2's
`#8A5B00` warning, `#155FCC` info, `#B3242B` danger fill; §2.1's `#FF6B72`).
Applying them per variant, as §7.4 already does for the ring, would clear 1.4.11
on both taskbars. That is a §7 change, not a change to make here.
