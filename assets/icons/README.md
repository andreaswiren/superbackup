# Application icon

The mark that appears in the taskbar, the Start menu, the Dock, Alt-Tab, the
installer, the `.exe` in Explorer, and the GitHub social preview.

<img src="png/superbackup-128.png" width="96" height="96" alt="">

## The mark

**One square, cut in two, the second piece slid clear.**

The drawing is a single rounded square divided by one stepped line. The lower
piece is then translated down-right, and that translation is the only thing
that opens the cut — which is why the kerf is exactly uniform along every
segment of it without a single hand-placed coordinate.

The two pieces are **congruent**. Piece B is piece A rotated half a turn; the
SVG says so literally, as a `rotate(180 …)` on the same path. That is the whole
idea. A backup that is not identical to the original is not a backup, and the
mark is built so that it cannot be drawn any other way. The pieces interlock,
because the copy is not a separate thing you have to remember — it is part of
the same object. And the copy sits down-right of the original: it has left the
machine.

The step in the cut is the increment. The direction of the slide is the corner
the tray badge sits in — the tray mark is this mark, with a status glyph in a
well knocked out of the corner the copy slides towards.

It also, unavoidably and usefully, reads as an **S**. That was not the starting
point — it fell out of the point symmetry — but a mark that is simultaneously
an abstract idea and the product's initial is a better mark than one that is
only the first, so it stayed.

### What it is not

Five directions were drawn and rejected before this one, and the reasons are
worth keeping because they will be re-proposed otherwise:

| Rejected | Why |
|---|---|
| Shield with a tick | Every backup product has one. It says "security vendor", not "this tool". |
| Three offset rounded squares (a cascade of copies) | Handsome at 128 px, but it is the *copy/duplicate* glyph from every UI toolkit, and below 24 px the rear tiles collapse into the front one. |
| Isometric stacked layers | Reads instantly at every size and is completely generic — it is the layers icon from every design tool and half the storage industry. |
| A field of small squares condensing into one large block (the product's literal thesis: millions of files become a handful of blobs) | The most specific idea of the set, and it dies at 16 px — a grid of sub-pixel dots is noise. |
| Folder with a duplicate behind it | Instantly meaningful and instantly mistakable for a file manager or a folder shortcut. The tab detail is gone by 20 px. |
| Two overlapping outlined squares with the intersection filled (deduplication made visible) | Good at 128 px; 1 px strokes at 16 px turn to grey mush, and the intersection — the entire idea — is invisible below 32 px. |

## Size strategy

There are **two drawings**, not one drawing scaled.

| Profile | Used at | Mark footprint | Kerf | Corner radius | Plate radius | Rim |
|---|---|---|---|---|---|---|
| `LARGE` | 32 px and above | 76 % of the plate | 5.2 % | 22 % of the piece | 22.37 % | 1/64 |
| `SMALL` | 24 px and below | 78 % of the plate | 9.5 % | 18 % of the piece | 17 % | 1/16 |
| `MONO_PROFILE` | every size, no plate | 94 % of the canvas | 11.5 % | 20 % of the piece | — | — |

Three things change and each has a reason measured at the pixel:

- **The kerf nearly doubles.** At 16 px the large profile's kerf is 0.83 px. It
  antialiases into a grey smear and the two halves fuse into one blob. The
  small profile's kerf is 1.5 px at 16, which survives.
- **The corner radius drops.** A 22 % radius on a 12 px piece is 2.6 px, which
  eats the corner and rounds the mark into a bean. 18 % keeps it square.
- **The rim thickens by a factor of four.** Stroke width in an SVG is in canvas
  units, so a rim specified for 512 px is 0.03 px at 16 px — invisible exactly
  where it is needed. Each profile's rim is therefore sized to land on ~1 px at
  the sizes that profile is actually used at.

The monochrome file is a third drawing again, and for a sharper reason: with no
plate and no second tone, **the kerf is the only thing left saying "two
pieces"**. Rendering it from the large profile put a 0.83 px kerf at 16 px and
the halves fused into one black blob. Its kerf is widened to 11.5 % — 1.8 px at
16 px — and the mark grows to fill the canvas the way a symbolic icon is
expected to, since there is no plate to sit inside.

Everything below 24 px inclusive — the ICO's 16/20/24 entries, the 16/22/24
PNGs, the ICNS `icp4`/`ic11` entries — is rendered from
`superbackup-small.svg`. Everything at 32 px and above comes from
`superbackup.svg`.

## Colour

Every value is a `design/DESIGN_SYSTEM.md` §2 token. Nothing was invented for
the icon.

| Role | Hex | Token |
|---|---|---|
| Plate | `#1A202A` | between `bg.rail` `#101318` and `bg.raised` `#232833` |
| Plate rim | `#5D6A7D` | between `border.control` `#6A7583` and `border.strong` `#3A4250` |
| `keep` — the original half | `#F2F5FA` | `text.primary` `#E8ECF2`, lifted for a large area |
| `copy` — the backup half | `#5B9BFF` | `accent` |

### Measured contrast

Computed with the WCAG 2.1 relative-luminance formula by
`python tools/icons/build.py --contrast`, which reads the colours out of
`tools/icons/geometry.py` rather than out of this table — so this table cannot
drift away from what is drawn. Re-run it after any change.

| Pair | Ratio | |
|---|---|---|
| `keep` `#F2F5FA` on plate `#1A202A` | **14.97:1** | |
| `copy` `#5B9BFF` on plate `#1A202A` | **5.90:1** | |
| plate `#1A202A` on a white desktop | **16.36:1** | |
| plate `#1A202A` on `#F3F3F3` | **14.74:1** | |
| `keep` `#F2F5FA` on a `#202020` desktop | **14.91:1** | if the plate is not perceived at all |
| monochrome `#000000` on white | **21.00:1** | |
| plate rim `#5D6A7D` on `#202020` | 2.97:1 | decorative — see below |
| plate rim `#5D6A7D` on plate `#1A202A` | 2.98:1 | decorative |
| plate `#1A202A` on black | 1.28:1 | the rim carries the edge here, at 3.82:1 |
| `keep` against `copy` | 2.54:1 | the two halves never touch — see below |

Two of those deserve honesty rather than a footnote.

**`keep` against `copy` is 2.54:1**, which would be a failure if the two
colours ever met. They do not: the kerf separates them everywhere, so each half
is read against the plate, at 14.97:1 and 5.90:1. The pair is listed because a
future change that closes the kerf would make 2.54:1 the operative number, and
that change should be rejected on this line.

**The plate rim is below 3:1.** WCAG 1.4.11 governs graphics that carry
meaning; the rim carries none. Its only job is to keep the tile's corners from
dissolving on a `#202020` desktop, and the icon's legibility there does not
depend on it — the mark itself is at 14.91:1 against that background with the
plate discounted entirely. Taking the rim to 3:1 makes it read as a drawn
outline rather than an edge, which is worse.

### Greyscale and colour vision

The two halves are separated by **luminance**, not hue: 0.911 relative
luminance against 0.329. Desaturating changes nothing structural, and neither
does any colour-vision deficiency — the light/dark relationship is intact under
protanopia, deuteranopia and tritanopia alike, because it does not depend on
the red–green or blue–yellow axis at all. `superbackup-mono.svg` proves the
stronger case: with both tones and the plate removed, the kerf alone still
reads the mark as two pieces. That file is not a fallback bolted on afterwards;
it is the test the design had to pass.

The greyscale rows in `preview/preview.html` are what to re-check after any
change.

## Files

| File | What it is |
|---|---|
| `superbackup.svg` | The master. 512 canvas, `LARGE` profile, full-bleed plate. Source of every raster at 32 px and above. |
| `superbackup-small.svg` | The simplified drawing for 24 px and below. Not a scaled copy — see *Size strategy*. |
| `superbackup-macos.svg` | 1024 canvas, plate inset to Apple's 824/1024 content box and drawn as a superellipse rather than a circular-arc rounded rectangle. Source of the `.icns`. |
| `superbackup-mono.svg` | Single-colour silhouette, no plate. Linux symbolic contexts, print, favicon fallback, anywhere the mark has to survive without colour. |
| `superbackup.ico` | Windows. 16, 20, 24, 32, 40, 48, 64, 128, 256. |
| `superbackup.icns` | macOS. `icp4`, `icp5`, `ic11`, `ic12`, `ic07`, `ic13`, `ic08`, `ic14`, `ic09`, `ic10`. |
| `png/superbackup-<n>.png` | 16, 22, 24, 32, 48, 64, 128, 256, 512, 1024. |
| `preview/preview.html` | The live review page. Every size, on five backgrounds, in colour and greyscale. |
| `preview/app-contact-sheet.png` | The same, rasterised, for reviewing in a diff. |

### What each format satisfies

- **`.ico`** — Windows resolves an icon by asking for a specific size, and
  picks the *nearest larger* entry if the exact one is absent, then downscales.
  The nine sizes cover 100 %, 125 %, 150 %, 175 %, 200 %, 250 % and 300 % DPI
  for both the 16 px shell icon and the 32 px shortcut icon without any
  resampling. Entries below 256 px are stored as 32-bit BMP with the (empty but
  structurally mandatory) AND mask; 256 is stored as PNG. That split is what
  every Windows shell path since Vista reads without complaint — PNG at small
  sizes works on modern Windows but still trips older icon consumers, and the
  application icon is precisely the asset that gets read by the oldest code on
  the machine.
- **`.icns`** — macOS needs `@2x` entries as distinct chunks rather than
  inferring them, so 32 appears both as `icp5` (32 @1x) and `ic11` (16 @2x),
  and so on up to `ic10` at 1024 (512 @2x). Built from `superbackup-macos.svg`
  because a Dock icon that fills its canvas edge to edge sits wrong next to
  system icons, which occupy 824 of 1024 and leave the rest to the shadow.
- **PNG at 16/22/24/32/48/64/128/256/512** — the Linux hicolor theme
  directories. 22 and 24 are there because GNOME and the SNI/AppIndicator
  panels ask for them specifically. 1024 covers the GitHub social preview and
  any store listing.
- **`superbackup-mono.svg`** — Linux symbolic icon contexts, and the fallback
  for anything that renders the mark in a single ink.

## Regenerating

Everything derived is built from `tools/icons/geometry.py` by one command:

```bash
cd tools/icons && npm install     # once: @resvg/resvg-js
python tools/icons/build.py       # from the repository root
```

That writes the four SVG masters, all ten PNGs, the `.ico`, the `.icns`, both
`preview.html` pages, the contact sheets — **and** the 48 tray SVGs in
`../tray/`, because both families come out of the same file: the tray mark *is*
this mark, with a status badge on it. It then prints the
measured contrast table.

`python tools/icons/build.py --contrast` prints the table alone.

Rasterisation goes through `@resvg/resvg-js` — the same `resvg` that
`crates/app/src/tray/icons.rs` renders with at run time. Using a different
engine here would let the checked-in previews disagree with what the running
program draws.

## Provenance

Original work. Every path in every file is generated by
`tools/icons/geometry.py` from first principles: rounded polygons, one
superellipse, and arithmetic. No third-party icon set, no traced logo, no font
glyph, no clip art. The busy background in `preview/app-contact-sheet.png` is
generated too, for the same reason — a real wallpaper would be someone else's
photograph. `docs/compliance/THIRD_PARTY.md`'s claim that every asset under
`assets/` is original remains true.
