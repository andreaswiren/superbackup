# Tray icons

> **Status: superseded, pending reimplementation.**
>
> [`design/DESIGN_SYSTEM.md`](../../design/DESIGN_SYSTEM.md) §"Tray icons" is the
> authoritative specification. The SVGs in this directory are an earlier
> single-variant design and are kept only so the tray has something to render
> until the specified set is built.
>
> The specified design is better, and the reason is worth recording. These icons
> solve the light-taskbar/dark-taskbar problem by compromise: one mid-luminance
> accent that scores an identical 3.83:1 against both backgrounds and is
> therefore optimal against neither. The design system instead ships **two
> variants per state** on Windows and swaps them on the `SystemUsesLightTheme`
> preference, so each variant can be genuinely high-contrast on the taskbar it
> is actually drawn on. It also moves state onto a neutral ring silhouette with
> a small chromatic pip, which survives macOS template rendering — where colour
> is stripped entirely — rather than merely surviving greyscale.
>
> What carries over unchanged is the principle below: shape, not colour, is what
> distinguishes the states.

Five states, one per variant of `superbackup_core::state::Health`. The stem of
each file matches `Health::icon_stem()`, so the tray code resolves an icon by
asking the health value for its name rather than by matching on it again.

| File | `Health` | Meaning |
|---|---|---|
| `idle.svg` | `Idle` | Every job succeeded recently. Nothing to do. |
| `running.svg` | `Running` | At least one job is backing up right now. |
| `attention.svg` | `Attention` | Nothing failed, but something needs the user: a locked vault, or a job that has not succeeded in a while. |
| `paused.svg` | `Paused` | The user turned backups off, temporarily or indefinitely. |
| `failed.svg` | `Failed` | A job or the service reported a failure. |

## Design rules

The icons are read at 16×16 in a Windows notification area that is often
cluttered, sometimes on a light taskbar and sometimes on a dark one. That
constrains them hard:

1. **One silhouette.** All five share the same shield outline. Only the
   interior mark and the accent colour change, so a glance registers *state*
   rather than *is that even my app*.
2. **Shape carries the meaning, not just colour.** Roughly one man in twelve
   has a colour vision deficiency, and red/green is the common axis. Each state
   therefore has a distinct interior glyph — check, arc, exclamation, bars,
   cross — that survives being rendered in greyscale. Test by desaturating.
3. **No gradients, no shadows, no strokes under 1.5px at 16×16.** They turn to
   mush. Everything is flat fill with generous negative space.
4. **Monochrome-safe.** macOS renders template images in the menu bar; the
   shield plus glyph reads correctly as a solid single-colour mask.

## Colours

A tray icon has to survive on a light taskbar (`#F3F3F3`) *and* a dark one
(`#202020`), and the app does not get to know which. That is a genuine
constraint rather than a preference: a colour dark enough to pop on light is
invisible on dark, and vice versa. Each accent was therefore solved for rather
than picked — hue fixed to keep the state's identity, lightness moved until the
contrast ratio against *both* backgrounds is as high as it can simultaneously
be, subject to the white interior glyph staying above 4:1 against the shield.

The result is a balanced ~3.8:1 in both directions. WCAG 2.1 SC 1.4.11 asks
3:1 for non-text graphics; these clear it on either taskbar, which a
better-on-one-worse-on-the-other palette would not.

| State | Accent | On `#F3F3F3` | On `#202020` | White glyph on accent |
|---|---|---|---|---|
| Idle | `#468769` | 3.83:1 | 3.83:1 | 4.25:1 |
| Running | `#377BD4` | 3.83:1 | 3.83:1 | 4.25:1 |
| Attention | `#AB6D11` | 3.83:1 | 3.83:1 | 4.25:1 |
| Paused | `#747B87` | 3.83:1 | 3.83:1 | 4.25:1 |
| Failed | `#EB2D36` | 3.83:1 | 3.83:1 | 4.25:1 |

Open `preview.html` in a browser to see all five at 16, 20 and 32 px on both
taskbar colours and in greyscale. Re-check it after any change to the icons —
the greyscale rows are the ones that catch a colour-only distinction creeping
back in.

## Rasterisation

SVG is the source of truth. The tray backends need bitmaps:

- **Windows** — `.ico` containing 16, 20, 24, 32, 40 and 48 px, so the icon
  stays sharp across the DPI scales Windows actually uses (100–300%).
- **Linux** — 22 and 24 px PNG for the AppIndicator, plus 32/48 for the
  window icon.
- **macOS** — 18 and 36 px (`@2x`) PNG, rendered as template images.

Rasterisation happens at build time from these sources rather than checking
generated bitmaps into the repository, so the two can never drift apart.
