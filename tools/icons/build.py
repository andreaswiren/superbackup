"""Generate every icon superbackup ships, plus the sheets used to review them.

    python tools/icons/build.py            # everything
    python tools/icons/build.py --contrast # just print the measured ratios

Nothing here is hand-drawn. The geometry lives in `geometry.py`; this file
turns it into the container formats each platform insists on, and into the
contact sheets that make it possible to judge the result rather than assume it.

Requires: Python 3.10+, Pillow, and `npm install` in this directory (for
`@resvg/resvg-js`, the renderer the application itself uses).
"""

from __future__ import annotations

import argparse
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

sys.path.insert(0, str(Path(__file__).parent))
import geometry as geo  # noqa: E402

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
ICONS = ROOT / "assets" / "icons"
PNGS = ICONS / "png"
PREVIEW = ICONS / "preview"
TRAY = ROOT / "assets" / "tray"

# The size at or below which the simplified drawing is used. Above it the mark
# has room for the full kerf and corner radii; at or below it does not, and a
# downscale of the large drawing loses the split entirely.
SMALL_MAX = 24

PNG_SIZES = [16, 22, 24, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
ICNS_ENTRIES = [
    ("icp4", 16),
    ("icp5", 32),
    ("ic11", 32),  # 16 @2x
    ("ic12", 64),  # 32 @2x
    ("ic07", 128),
    ("ic13", 256),  # 128 @2x
    ("ic08", 256),
    ("ic14", 512),  # 256 @2x
    ("ic09", 512),
    ("ic10", 1024),  # 512 @2x
]

# Backgrounds every mark is judged against.
WHITE = (255, 255, 255)
LIGHT_TASKBAR = (243, 243, 243)  # #F3F3F3
DARK_TASKBAR = (32, 32, 32)  # #202020
BLACK = (0, 0, 0)


# ---------------------------------------------------------------------------
# Contrast, measured rather than asserted
# ---------------------------------------------------------------------------


def _channel(value: float) -> float:
    value /= 255.0
    return value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4


def luminance(hex_colour: str) -> float:
    h = hex_colour.lstrip("#")
    r, g, b = (int(h[i : i + 2], 16) for i in (0, 2, 4))
    return 0.2126 * _channel(r) + 0.7152 * _channel(g) + 0.0722 * _channel(b)


def contrast(a: str, b: str) -> float:
    la, lb = luminance(a), luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


# Colours come from `geometry` so the published table cannot drift away from
# what is actually drawn — the previous tray README asserted ratios that had
# stopped being true.
CONTRAST_CHECKS = [
    ("app", f"mark `keep` {geo.KEEP}", f"plate {geo.PLATE}", geo.KEEP, geo.PLATE),
    ("app", f"mark `copy` {geo.COPY}", f"plate {geo.PLATE}", geo.COPY, geo.PLATE),
    ("app", "`keep` vs `copy` (across the kerf)", "", geo.KEEP, geo.COPY),
    ("app", f"plate {geo.PLATE}", "white desktop", geo.PLATE, "#FFFFFF"),
    ("app", f"plate {geo.PLATE}", "light taskbar #F3F3F3", geo.PLATE, "#F3F3F3"),
    ("app", f"plate rim {geo.PLATE_EDGE}", "dark desktop #202020", geo.PLATE_EDGE, "#202020"),
    ("app", f"plate rim {geo.PLATE_EDGE}", f"plate {geo.PLATE}", geo.PLATE_EDGE, geo.PLATE),
    ("app", "monochrome #000000", "white", "#000000", "#FFFFFF"),
    ("tray", f"mark ink -light {geo.MARK_INK['light']}", "light taskbar #F3F3F3", geo.MARK_INK["light"], "#F3F3F3"),
    ("tray", f"mark ink -dark {geo.MARK_INK['dark']}", "dark taskbar #202020", geo.MARK_INK["dark"], "#202020"),
    ("tray", f"idle badge -light {geo.BADGE_INK['light']['idle']}", "light taskbar #F3F3F3", geo.BADGE_INK["light"]["idle"], "#F3F3F3"),
    ("tray", f"running badge -light {geo.BADGE_INK['light']['running']}", "light taskbar #F3F3F3", geo.BADGE_INK["light"]["running"], "#F3F3F3"),
    ("tray", f"attention badge -light {geo.BADGE_INK['light']['attention']}", "light taskbar #F3F3F3", geo.BADGE_INK["light"]["attention"], "#F3F3F3"),
    ("tray", f"paused badge -light {geo.BADGE_INK['light']['paused']}", "light taskbar #F3F3F3", geo.BADGE_INK["light"]["paused"], "#F3F3F3"),
    ("tray", f"failed badge -light {geo.BADGE_INK['light']['failed']}", "light taskbar #F3F3F3", geo.BADGE_INK["light"]["failed"], "#F3F3F3"),
    ("tray", f"idle badge -dark {geo.BADGE_INK['dark']['idle']}", "dark taskbar #202020", geo.BADGE_INK["dark"]["idle"], "#202020"),
    ("tray", f"running badge -dark {geo.BADGE_INK['dark']['running']}", "dark taskbar #202020", geo.BADGE_INK["dark"]["running"], "#202020"),
    ("tray", f"attention badge -dark {geo.BADGE_INK['dark']['attention']}", "dark taskbar #202020", geo.BADGE_INK["dark"]["attention"], "#202020"),
    ("tray", f"paused badge -dark {geo.BADGE_INK['dark']['paused']}", "dark taskbar #202020", geo.BADGE_INK["dark"]["paused"], "#202020"),
    ("tray", f"failed badge -dark {geo.BADGE_INK['dark']['failed']}", "dark taskbar #202020", geo.BADGE_INK["dark"]["failed"], "#202020"),
    ("tray", f"idle badge -light vs its own mark ink", "never touch: see the clear space", geo.BADGE_INK["light"]["idle"], geo.MARK_INK["light"]),
    ("tray", f"attention badge -light vs its own mark ink", "never touch: see the clear space", geo.BADGE_INK["light"]["attention"], geo.MARK_INK["light"]),
    ("tray", f"failed badge -light vs its own mark ink", "never touch: see the clear space", geo.BADGE_INK["light"]["failed"], geo.MARK_INK["light"]),
    ("tray", f"idle badge -dark vs its own mark ink", "never touch: see the clear space", geo.BADGE_INK["dark"]["idle"], geo.MARK_INK["dark"]),
    ("tray", f"attention badge -dark vs its own mark ink", "never touch: see the clear space", geo.BADGE_INK["dark"]["attention"], geo.MARK_INK["dark"]),
    ("tray", f"failed badge -dark vs its own mark ink", "never touch: see the clear space", geo.BADGE_INK["dark"]["failed"], geo.MARK_INK["dark"]),
    ("tray", "template #000000", "light menu bar #F3F3F3", "#000000", "#F3F3F3"),
    ("tray", "template inverted #FFFFFF", "dark menu bar #202020", "#FFFFFF", "#202020"),
]


def print_contrast() -> None:
    width = max(len(row[1]) for row in CONTRAST_CHECKS)
    for group, label, against, a, b in CONTRAST_CHECKS:
        ratio = contrast(a, b)
        verdict = "ok " if ratio >= 3.0 else "LOW"
        print(f"{group:5} {label:<{width}}  vs {against:<26} {ratio:5.2f}:1  {verdict}")


# ---------------------------------------------------------------------------
# Rasterisation
# ---------------------------------------------------------------------------


def rasterise(jobs: list[dict]) -> None:
    """Render a batch of SVG files to PNG through resvg."""
    if not jobs:
        return
    if not (HERE / "node_modules" / "@resvg" / "resvg-js").exists():
        raise SystemExit(
            "the renderer is missing — run `npm install` in tools/icons first"
        )
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
        json.dump(jobs, handle)
        path = handle.name
    try:
        result = subprocess.run(
            ["node", str(HERE / "render.js"), path],
            capture_output=True,
            text=True,
            cwd=HERE,
        )
        if result.returncode != 0:
            raise SystemExit(f"render.js failed:\n{result.stderr}")
    finally:
        os.unlink(path)


# ---------------------------------------------------------------------------
# Container formats
# ---------------------------------------------------------------------------


def write_ico(images: list[Image.Image], destination: Path) -> None:
    """A Windows .ico containing every size in `images`.

    Sizes below 256 are stored as 32-bit BMP with an (empty but mandatory) AND
    mask, and 256 as PNG. That is the split every Windows shell path since
    Vista handles without argument; storing the small sizes as PNG works on
    modern Windows but still trips some older icon consumers, and an
    application icon is exactly the asset that gets read by the oldest code on
    the machine.
    """
    entries: list[tuple[Image.Image, bytes, bool]] = []
    for image in images:
        if image.width >= 256:
            from io import BytesIO

            buffer = BytesIO()
            image.save(buffer, format="PNG")
            entries.append((image, buffer.getvalue(), True))
        else:
            width, height = image.size
            header = struct.pack(
                "<IiiHHIIiiII", 40, width, height * 2, 1, 32, 0, 0, 0, 0, 0, 0
            )
            pixels = image.convert("RGBA").load()
            rows = []
            for y in range(height - 1, -1, -1):
                row = bytearray()
                for x in range(width):
                    r, g, b, a = pixels[x, y]
                    row += bytes((b, g, r, a))
                rows.append(bytes(row))
            xor = b"".join(rows)
            # 1bpp AND mask, rows padded to 4 bytes. Left at zero: the alpha
            # channel is what actually masks a 32-bit icon, but the structure
            # has to be there or the entry is malformed.
            mask_row = ((width + 31) // 32) * 4
            and_mask = b"\x00" * (mask_row * height)
            entries.append((image, header + xor + and_mask, False))

    offset = 6 + 16 * len(entries)
    directory = struct.pack("<HHH", 0, 1, len(entries))
    body = b""
    for image, payload, _ in entries:
        width = 0 if image.width >= 256 else image.width
        height = 0 if image.height >= 256 else image.height
        directory += struct.pack(
            "<BBBBHHII", width, height, 0, 0, 1, 32, len(payload), offset
        )
        offset += len(payload)
        body += payload
    destination.write_bytes(directory + body)


def write_icns(entries: list[tuple[str, Path]], destination: Path) -> None:
    """A macOS .icns. Every entry is a PNG payload in a typed chunk."""
    chunks = b""
    for icns_type, png_path in entries:
        payload = png_path.read_bytes()
        chunks += icns_type.encode("ascii") + struct.pack(">I", len(payload) + 8) + payload
    destination.write_bytes(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)


# ---------------------------------------------------------------------------
# Review sheets
# ---------------------------------------------------------------------------


def busy_background(size: tuple[int, int], seed: int = 7) -> Image.Image:
    """An original, deliberately hostile desktop background.

    A real wallpaper would be someone else's copyrighted photograph, and
    `docs/compliance/THIRD_PARTY.md` says everything under `assets/` is
    original. This is generated: overlapping blurred colour fields with a
    high-frequency speckle and a few hard edges, which is what actually breaks
    a weak icon — mid-luminance clutter at the icon's own spatial frequency.
    """
    import random

    rng = random.Random(seed)
    width, height = size
    image = Image.new("RGB", (width, height), (70, 80, 95))
    draw = ImageDraw.Draw(image)
    for _ in range(90):
        x, y = rng.randrange(width), rng.randrange(height)
        radius = rng.randrange(18, max(20, min(width, height) // 2))
        colour = (rng.randrange(20, 240), rng.randrange(20, 240), rng.randrange(20, 240))
        draw.ellipse((x - radius, y - radius, x + radius, y + radius), fill=colour)
    image = image.filter(ImageFilter.GaussianBlur(11))
    draw = ImageDraw.Draw(image)
    for _ in range(30):
        x0, y0 = rng.randrange(width), rng.randrange(height)
        draw.line(
            (x0, y0, x0 + rng.randrange(-90, 90), y0 + rng.randrange(-90, 90)),
            fill=(rng.randrange(255), rng.randrange(255), rng.randrange(255)),
            width=rng.choice((1, 2, 3)),
        )
    pixels = image.load()
    for y in range(height):
        for x in range(width):
            r, g, b = pixels[x, y]
            n = rng.randrange(-24, 25)
            pixels[x, y] = (
                max(0, min(255, r + n)),
                max(0, min(255, g + n)),
                max(0, min(255, b + n)),
            )
    return image


def _label(draw: ImageDraw.ImageDraw, xy, text, fill=(232, 236, 242)) -> None:
    draw.text(xy, text, fill=fill)


def app_contact_sheet(rendered: dict[int, Path], destination: Path) -> None:
    """Every app-icon size, on every background it has to survive."""
    sizes = PNG_SIZES[:-1]  # 1024 is not a viewing size
    backgrounds = [
        ("white", WHITE),
        ("#F3F3F3", LIGHT_TASKBAR),
        ("#202020", DARK_TASKBAR),
        ("black", BLACK),
        ("busy", None),
    ]
    pad, left, top = 18, 78, 34
    cell = max(sizes)
    row_height = cell + pad + 16
    width = left + sum(s + pad for s in sizes) + pad
    height = top + row_height * len(backgrounds) + 10
    sheet = Image.new("RGB", (width, height), (18, 20, 25))
    draw = ImageDraw.Draw(sheet)
    _label(draw, (left, 10), "superbackup application icon — " + "  ".join(str(s) for s in sizes))

    busy = busy_background((width, height))
    y = top
    for name, colour in backgrounds:
        _label(draw, (8, y + cell // 2), name)
        x = left
        for size in sizes:
            box = (x, y + (cell - size) // 2)
            if colour is None:
                tile = busy.crop((box[0], box[1], box[0] + size, box[1] + size)).convert("RGBA")
            else:
                tile = Image.new("RGBA", (size, size), colour + (255,))
            tile.alpha_composite(Image.open(rendered[size]).convert("RGBA"))
            sheet.paste(tile.convert("RGB"), box)
            x += size + pad
        y += row_height
    sheet.save(destination)


def tray_contact_sheet(directory: Path, destination: Path) -> None:
    """The five marks at every tray size, both taskbars, colour and greyscale."""
    sizes = [16, 20, 24, 32]
    rows = [
        ("light taskbar", LIGHT_TASKBAR, "light", False),
        ("light, greyscale", LIGHT_TASKBAR, "light", True),
        ("dark taskbar", DARK_TASKBAR, "dark", False),
        ("dark, greyscale", DARK_TASKBAR, "dark", True),
        ("macOS menu bar", (246, 246, 246), "template", False),
        ("macOS, inverted", (28, 28, 30), "template-inverted", False),
    ]
    pad, left, top, group = 14, 108, 34, 26
    cell = 32
    per_state = sum(s + pad for s in sizes) + group
    width = left + per_state * len(geo.TRAY_STATES) + pad
    height = top + (cell + pad + 14) * len(rows) + 10
    sheet = Image.new("RGB", (width, height), (18, 20, 25))
    draw = ImageDraw.Draw(sheet)

    x = left
    for state in geo.TRAY_STATES:
        _label(draw, (x, 12), state)
        x += per_state

    y = top
    for name, background, variant, grey in rows:
        _label(draw, (6, y + cell // 2), name)
        x = left
        for state in geo.TRAY_STATES:
            for size in sizes:
                source = variant.replace("-inverted", "")
                stem = geo.tray_stem(state, source, 0)
                tile = Image.new("RGBA", (size, size), background + (255,))
                mark = Image.open(directory / f"{stem}_{size}.png").convert("RGBA")
                if variant.endswith("-inverted"):
                    # macOS inverts a template image for a dark menu bar: the
                    # alpha is the mark, the colour comes from the OS.
                    alpha = mark.getchannel("A")
                    mark = Image.new("RGBA", mark.size, (255, 255, 255, 0))
                    mark.putalpha(alpha)
                    solid = Image.new("RGBA", mark.size, (255, 255, 255, 255))
                    solid.putalpha(alpha)
                    mark = solid
                tile.alpha_composite(mark)
                flat = tile.convert("RGB")
                if grey:
                    flat = flat.convert("L").convert("RGB")
                sheet.paste(flat, (x, y + (cell - size) // 2))
                x += size + pad
            x += group
        y += cell + pad + 14
    sheet.save(destination)


def running_sheet(directory: Path, destination: Path) -> None:
    """The twelve running frames, so the animation can be read as a strip."""
    size, pad, left, top = 32, 12, 78, 26
    rows = [("dark", DARK_TASKBAR), ("light", LIGHT_TASKBAR)]
    width = left + (size + pad) * geo.RUNNING_FRAMES + pad
    height = top + (size + pad + 12) * len(rows) + 8
    sheet = Image.new("RGB", (width, height), (18, 20, 25))
    draw = ImageDraw.Draw(sheet)
    _label(draw, (left, 6), "running — 12 frames, 30° apart")
    y = top
    for variant, background in rows:
        _label(draw, (8, y + size // 2), variant)
        x = left
        for frame in range(geo.RUNNING_FRAMES):
            stem = geo.tray_stem("running", variant, frame)
            tile = Image.new("RGBA", (size, size), background + (255,))
            tile.alpha_composite(Image.open(directory / f"{stem}_{size}.png").convert("RGBA"))
            sheet.paste(tile.convert("RGB"), (x, y))
            x += size + pad
        y += size + pad + 12
    sheet.save(destination)


# ---------------------------------------------------------------------------
# preview.html
# ---------------------------------------------------------------------------


def write_app_preview(destination: Path) -> None:
    large = geo.app_svg(512, geo.LARGE)
    small = geo.app_svg(512, geo.SMALL)
    mono = geo.app_svg(512, geo.MONO_PROFILE, mono=True)
    destination.write_text(
        """<!doctype html>
<meta charset="utf-8">
<title>superbackup — application icon</title>
<style>
  :root { color-scheme: dark; }
  body { background:#0C0E12; color:#E8ECF2; font:14px/1.5 system-ui,sans-serif; margin:0; padding:28px; }
  h1 { font-size:16px; margin:0 0 4px; }
  p  { color:#8A94A1; margin:0 0 24px; max-width:60ch; }
  h2 { font-size:12px; text-transform:uppercase; letter-spacing:.08em; color:#8A94A1;
       margin:28px 0 10px; }
  .strip { display:flex; align-items:flex-end; gap:18px; flex-wrap:wrap; padding:14px;
           border-radius:8px; }
  .strip.white   { background:#FFFFFF; }
  .strip.grey    { background:#F3F3F3; }
  .strip.dark    { background:#202020; }
  .strip.black   { background:#000000; }
  .strip.busy    { background:
      radial-gradient(circle at 20% 30%, #c2531f, transparent 40%),
      radial-gradient(circle at 70% 20%, #1f6fc2, transparent 45%),
      radial-gradient(circle at 45% 80%, #2f9d5b, transparent 40%),
      repeating-linear-gradient(52deg,#0000 0 6px,#ffffff22 6px 7px), #4a5568; }
  .cell { display:flex; flex-direction:column; align-items:center; gap:6px; }
  .cell span { font-size:10px; color:#7d8794; }
  .strip.white .cell span, .strip.grey .cell span { color:#555; }
  .grey-out { filter:grayscale(1); }
</style>
<h1>superbackup — application icon</h1>
<p>The mark is one rounded square cut in two along a single stepped line, the
second piece slid down-right so the cut opens into a kerf. The two pieces are
congruent: piece B is piece A rotated half a turn. Regenerate every derived
file with <code>python tools/icons/build.py</code>.</p>
__SECTIONS__
"""
        .replace("__SECTIONS__", _preview_sections(large, small, mono)),
        encoding="utf-8", newline="\n",
    )


def _sized(svg: str, size: int) -> str:
    return svg.replace('width="512"', f'width="{size}"', 1).replace(
        'height="512"', f'height="{size}"', 1
    )


def _preview_sections(large: str, small: str, mono: str) -> str:
    out = []
    big = [512, 256, 128, 64, 48, 32]
    tiny = [32, 24, 22, 20, 16]
    for title, css in [
        ("On white", "white"),
        ("On #F3F3F3", "grey"),
        ("On #202020", "dark"),
        ("On black", "black"),
        ("On a busy desktop", "busy"),
    ]:
        cells = "".join(
            f'<div class="cell">{_sized(large, s)}<span>{s}</span></div>' for s in big
        )
        cells += "".join(
            f'<div class="cell">{_sized(small, s)}<span>{s} small</span></div>' for s in tiny
        )
        out.append(f'<h2>{title}</h2><div class="strip {css}">{cells}</div>')

    cells = "".join(
        f'<div class="cell">{_sized(large, s)}<span>{s}</span></div>' for s in [128, 48, 32]
    ) + "".join(
        f'<div class="cell">{_sized(small, s)}<span>{s} small</span></div>' for s in tiny
    )
    out.append(f'<h2>Greyscale</h2><div class="strip dark grey-out">{cells}</div>')
    out.append(f'<h2>Greyscale, light</h2><div class="strip grey grey-out">{cells}</div>')

    mono_cells = "".join(
        f'<div class="cell">{_sized(mono, s)}<span>{s}</span></div>' for s in [128, 48, 32, 24, 16]
    )
    out.append(
        '<h2>Monochrome (no plate) — Linux symbolic, print, favicon fallback</h2>'
        f'<div class="strip white">{mono_cells}</div>'
    )
    return "\n".join(out)


def write_tray_preview(destination: Path) -> None:
    sizes = [16, 20, 24, 32]
    sections = []
    for title, css, variant in [
        ("Light taskbar (#F3F3F3) — the <code>-light</code> variant", "light", "light"),
        ("Dark taskbar (#202020) — the <code>-dark</code> variant", "dark", "dark"),
        ("macOS menu bar — the template image, as macOS draws it", "menubar", "template"),
    ]:
        rows = []
        for state in geo.TRAY_STATES:
            cells = "".join(
                f'<div class="cell">{_tray_sized(state, variant, s)}<span>{s}</span></div>'
                for s in sizes
            )
            rows.append(f'<div class="state"><b>{state}</b>{cells}</div>')
        sections.append(f'<h2>{title}</h2><div class="strip {css}">{"".join(rows)}</div>')
        sections.append(
            f'<h2>{title} — greyscale</h2>'
            f'<div class="strip {css} grey-out">{"".join(rows)}</div>'
        )

    frames = "".join(
        f'<div class="cell">{_tray_sized("running", "dark", 32, f)}<span>{f:02d}</span></div>'
        for f in range(geo.RUNNING_FRAMES)
    )
    sections.append(f'<h2>running — 12 frames, 30° apart</h2><div class="strip dark">{frames}</div>')

    destination.write_text(
        """<!doctype html>
<meta charset="utf-8">
<title>superbackup — tray marks</title>
<style>
  :root { color-scheme: dark; }
  body { background:#0C0E12; color:#E8ECF2; font:14px/1.5 system-ui,sans-serif; margin:0; padding:28px; }
  h1 { font-size:16px; margin:0 0 4px; }
  p  { color:#8A94A1; margin:0 0 8px; max-width:64ch; }
  h2 { font-size:12px; text-transform:uppercase; letter-spacing:.08em; color:#8A94A1;
       margin:26px 0 10px; }
  .strip { display:flex; gap:26px; flex-wrap:wrap; padding:16px; border-radius:8px; }
  .strip.light   { background:#F3F3F3; }
  .strip.dark    { background:#202020; }
  .strip.menubar { background:#F6F6F6; }
  .state { display:flex; align-items:flex-end; gap:10px; }
  .state b { font-size:11px; font-weight:600; width:66px; }
  .strip.light .state b, .strip.menubar .state b { color:#22262E; }
  .cell { display:flex; flex-direction:column; align-items:center; gap:4px; }
  .cell span { font-size:10px; color:#7d8794; }
  .grey-out { filter:grayscale(1); }
</style>
<h1>superbackup — the five tray marks</h1>
<p><a href="../../design/DESIGN_SYSTEM.md">DESIGN_SYSTEM.md</a> §7 is the
specification; these files are it, drawn. The application generates the same
geometry at run time from <code>crates/app/src/tray/icons.rs</code>. Regenerate
with <code>python tools/icons/build.py</code>.</p>
<p>The greyscale rows are the point of this page: they are what catches a
state that has quietly started depending on colour.</p>
__SECTIONS__
"""
        .replace("__SECTIONS__", "\n".join(sections)),
        encoding="utf-8", newline="\n",
    )


def _tray_sized(state: str, variant: str, size: int, frame: int = 0) -> str:
    stem = geo.tray_stem(state, variant, frame)
    return (
        f'<img src="{stem}.svg" width="{size}" height="{size}" alt="{state} {variant} {size}px">'
    )


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def build() -> None:
    for directory in (ICONS, PNGS, PREVIEW, TRAY):
        directory.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="superbackup-icons-"))

    try:
        # -- sources -------------------------------------------------------
        masters = {
            "superbackup.svg": geo.app_svg(512, geo.LARGE),
            "superbackup-small.svg": geo.app_svg(512, geo.SMALL),
            "superbackup-macos.svg": geo.app_svg(1024, geo.LARGE, macos=True),
            "superbackup-mono.svg": geo.app_svg(512, geo.MONO_PROFILE, plate=False, mono=True),
        }
        for name, svg in masters.items():
            (ICONS / name).write_text(svg + "\n", encoding="utf-8", newline="\n")

        tray_written: list[str] = []
        for variant in geo.TRAY_VARIANTS:
            for state in geo.TRAY_STATES:
                if state == "running":
                    for frame in range(geo.RUNNING_FRAMES):
                        stem = geo.tray_stem(state, variant, frame)
                        (TRAY / f"{stem}.svg").write_text(
                            geo.tray_svg(state, variant, frame) + "\n", encoding="utf-8", newline="\n"
                        )
                        tray_written.append(stem)
                else:
                    stem = geo.tray_stem(state, variant)
                    (TRAY / f"{stem}.svg").write_text(
                        geo.tray_svg(state, variant) + "\n", encoding="utf-8", newline="\n"
                    )
                    tray_written.append(stem)

        # Anything left over from an earlier naming — the bare `<state>.svg`
        # files, the old `running-NN.svg` — is removed rather than left to rot
        # next to the current set, where it would be mistaken for a variant
        # that still ships.
        current = {f"{stem}.svg" for stem in tray_written}
        for stale in sorted(TRAY.glob("*.svg")):
            if stale.name not in current:
                stale.unlink()

        # -- rasterise -----------------------------------------------------
        jobs = []
        app_png: dict[int, Path] = {}
        for size in sorted(set(PNG_SIZES + ICO_SIZES + [512, 1024])):
            source = ICONS / ("superbackup-small.svg" if size <= SMALL_MAX else "superbackup.svg")
            target = work / f"app-{size}.png"
            jobs.append({"svg": str(source), "png": str(target), "size": size})
            app_png[size] = target
        macos_png: dict[int, Path] = {}
        for _, size in ICNS_ENTRIES:
            if size in macos_png:
                continue
            target = work / f"macos-{size}.png"
            jobs.append(
                {"svg": str(ICONS / "superbackup-macos.svg"), "png": str(target), "size": size}
            )
            macos_png[size] = target
        for stem in tray_written:
            for size in (16, 20, 24, 32, 48):
                jobs.append(
                    {
                        "svg": str(TRAY / f"{stem}.svg"),
                        "png": str(work / f"{stem}_{size}.png"),
                        "size": size,
                    }
                )
        rasterise(jobs)

        # -- shipped bitmaps ----------------------------------------------
        for size in PNG_SIZES:
            shutil.copyfile(app_png[size], PNGS / f"superbackup-{size}.png")
        write_ico([Image.open(app_png[s]).convert("RGBA") for s in ICO_SIZES],
                  ICONS / "superbackup.ico")
        write_icns([(t, macos_png[s]) for t, s in ICNS_ENTRIES], ICONS / "superbackup.icns")

        # -- review sheets -------------------------------------------------
        app_contact_sheet(app_png, PREVIEW / "app-contact-sheet.png")
        tray_contact_sheet(work, PREVIEW / "tray-contact-sheet.png")
        running_sheet(work, PREVIEW / "tray-running-frames.png")
        write_app_preview(PREVIEW / "preview.html")
        write_tray_preview(TRAY / "preview.html")

        print(f"masters      {len(masters)}")
        print(f"tray svg     {len(tray_written)}")
        print(f"png          {len(PNG_SIZES)}")
        print(f"ico          {len(ICO_SIZES)} sizes, {(ICONS / 'superbackup.ico').stat().st_size} bytes")
        print(f"icns         {len(ICNS_ENTRIES)} entries, {(ICONS / 'superbackup.icns').stat().st_size} bytes")
        print("sheets       assets/icons/preview/")
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--contrast", action="store_true", help="print measured ratios and exit")
    arguments = parser.parse_args()
    if arguments.contrast:
        print_contrast()
    else:
        build()
        print()
        print_contrast()
