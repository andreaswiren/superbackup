"""Geometry for every superbackup mark.

Two independent families live here.

**The application icon** (`app_svg`) is *The Interlock*: one rounded square cut
in two along a single stepped line, the lower piece slid down-right so the cut
becomes a visible kerf. The two pieces are congruent — piece B is literally
piece A rotated 180° — which is the whole idea: a backup that is not identical
to the original is not a backup. The step in the cut is the increment; the
down-right slide is the copy leaving the machine, and it lands on the same
diagonal the tray pip occupies.

**The tray marks** (`tray_svg`) are not a design decision made here. They are
`design/DESIGN_SYSTEM.md` §7.1/§7.2 transcribed, and deliberately mirror
`crates/app/src/tray/icons.rs`, which generates the same geometry at run time.
Where this file knowingly departs from that code it says so in a comment and
`assets/tray/README.md` records it.
"""

from __future__ import annotations

import math

# ---------------------------------------------------------------------------
# Palette. Every value is a design-system token; nothing here is invented.
# ---------------------------------------------------------------------------

PLATE = "#1A202A"  # between bg.rail #101318 and bg.raised #232833
PLATE_EDGE = "#5D6A7D"  # rim so the plate keeps its shape on a dark desktop
KEEP = "#F2F5FA"  # the original half — text.primary #E8ECF2, lifted for area
COPY = "#5B9BFF"  # the backup half — `accent`
MONO = "#000000"

# ---------------------------------------------------------------------------
# Path helpers
# ---------------------------------------------------------------------------


def _fmt(value: float) -> str:
    text = f"{value:.3f}".rstrip("0").rstrip(".")
    return "0" if text in ("-0", "") else text


def rounded_polygon(points: list[tuple[float, float, float]]) -> str:
    """Path for a closed polygon with a per-vertex corner radius.

    Handles reflex corners — the mark's inner corners turn the other way, and a
    naive implementation sweeps those arcs backwards and produces a bow tie.
    """
    n = len(points)
    segments: list[str] = []
    for i in range(n):
        px, py, radius = points[i]
        ax, ay, _ = points[(i - 1) % n]
        bx, by, _ = points[(i + 1) % n]

        v1 = (ax - px, ay - py)
        v2 = (bx - px, by - py)
        l1 = math.hypot(*v1) or 1.0
        l2 = math.hypot(*v2) or 1.0
        u1 = (v1[0] / l1, v1[1] / l1)
        u2 = (v2[0] / l2, v2[1] / l2)

        # Clamp so neighbouring corners cannot eat each other's edge.
        radius = min(radius, l1 / 2, l2 / 2)
        if radius <= 0.0005:
            segments.append(("L" if segments else "M") + f" {_fmt(px)} {_fmt(py)}")
            continue

        angle = math.acos(max(-1.0, min(1.0, u1[0] * u2[0] + u1[1] * u2[1])))
        setback = radius / math.tan(angle / 2) if angle > 1e-6 else 0.0
        setback = min(setback, l1 / 2, l2 / 2)
        # Recover the radius the clamped setback actually implies.
        radius = setback * math.tan(angle / 2)

        start = (px + u1[0] * setback, py + u1[1] * setback)
        end = (px + u2[0] * setback, py + u2[1] * setback)
        # Cross product sign gives the turn direction, which is the arc sweep.
        cross = u1[0] * u2[1] - u1[1] * u2[0]
        sweep = 0 if cross > 0 else 1

        if not segments:
            segments.append(f"M {_fmt(start[0])} {_fmt(start[1])}")
        else:
            segments.append(f"L {_fmt(start[0])} {_fmt(start[1])}")
        segments.append(
            f"A {_fmt(radius)} {_fmt(radius)} 0 0 {sweep} {_fmt(end[0])} {_fmt(end[1])}"
        )
    segments.append("Z")
    return " ".join(segments)


def superellipse(cx: float, cy: float, half: float, exponent: float = 5.0, steps: int = 96) -> str:
    """A squircle path — the corner continuity macOS icons are drawn with.

    A plain `rx` rounded rectangle reads as subtly wrong in the Dock next to
    system icons, whose corners are curvature-continuous rather than
    circular-arc.
    """
    points = []
    for i in range(steps):
        theta = 2 * math.pi * i / steps
        ct, st = math.cos(theta), math.sin(theta)
        x = cx + half * math.copysign(abs(ct) ** (2 / exponent), ct)
        y = cy + half * math.copysign(abs(st) ** (2 / exponent), st)
        points.append(f"{_fmt(x)} {_fmt(y)}")
    return "M " + " L ".join(points) + " Z"


# ---------------------------------------------------------------------------
# The application mark
# ---------------------------------------------------------------------------


def interlock_pieces(
    box: float, side: float, kerf: float, radius: float, step: float, inner: float
) -> tuple[str, str]:
    """The two congruent halves of the mark, as path data on a `box` canvas.

    `side` is the side of the square before the cut; the union of the two
    pieces is `side + kerf` across, so that is what gets centred. Piece B is
    piece A rotated 180° about the square's centre and translated by
    (kerf, kerf) — the translation is the only thing that opens the cut, which
    is why the kerf is uniform along every segment of it without a single
    hand-placed coordinate.
    """
    span = side + kerf
    x0 = (box - span) / 2
    y0 = x0
    x1 = x0 + side
    y1 = y0 + side
    cx = x0 + side / 2
    cy = y0 + side / 2

    piece_a = rounded_polygon(
        [
            (x0, y0, radius),  # outer top-left
            (x1, y0, radius),  # outer top-right
            (x1, cy - step, inner),  # cut leaves the right edge
            (cx, cy - step, inner),  # inner corner (reflex)
            (cx, cy + step, inner),  # inner corner
            (x0, cy + step, inner),  # cut leaves the left edge
        ]
    )
    # B is A, rotated half a turn about the square's centre, then slid clear.
    transform = f"translate({_fmt(kerf)} {_fmt(kerf)}) rotate(180 {_fmt(cx)} {_fmt(cy)})"
    return piece_a, transform


# Proportions, as fractions of the plate. The large and small drawings differ
# on purpose: below 24 px the kerf has to be wider than a pixel or the two
# halves fuse, and the mark has to grow into the plate's margin or it turns
# into a smudge with a frame round it.
LARGE = dict(mark=0.760, kerf=0.052, radius=0.220, step=0.300, inner=0.060,
             plate_radius=0.2237, edge=1 / 64)
SMALL = dict(mark=0.780, kerf=0.095, radius=0.180, step=0.300, inner=0.040,
             plate_radius=0.1700, edge=1 / 16)
# Monochrome has no plate to sit in and no second tone, so the kerf is the only
# thing left saying "two pieces". It is widened again, and the mark grows to
# fill the canvas the way a symbolic icon is expected to.
MONO_PROFILE = dict(mark=0.940, kerf=0.115, radius=0.200, step=0.300, inner=0.045,
                    plate_radius=0.1700, edge=1 / 16)


def app_svg(
    box: float = 512.0,
    profile: dict | None = None,
    plate: bool = True,
    macos: bool = False,
    mono: bool = False,
    label: str = "superbackup",
) -> str:
    """One application-icon document.

    `macos` insets the plate to Apple's 824/1024 content box and draws it as a
    squircle. `mono` drops the plate and both tones, leaving the silhouette —
    the kerf is a real gap, so the mark still reads as two pieces with the
    colour taken away.
    """
    profile = profile or LARGE
    content = box * (824 / 1024) if macos else box
    scale = content / box

    side = box * (profile["mark"] - profile["kerf"]) * scale
    kerf = box * profile["kerf"] * scale
    radius = side * profile["radius"]
    step = side * profile["step"]
    inner = side * profile["inner"]

    piece, transform = interlock_pieces(box, side, kerf, radius, step, inner)

    body: list[str] = []
    if plate and not mono:
        if macos:
            body.append(f'<path d="{superellipse(box / 2, box / 2, content / 2)}" fill="{PLATE}"/>')
        else:
            r = box * profile["plate_radius"]
            body.append(
                f'<rect width="{_fmt(box)}" height="{_fmt(box)}" rx="{_fmt(r)}" fill="{PLATE}"/>'
            )
            # The rim exists for one reason: on a #202020 desktop the plate
            # and the wallpaper are almost the same value, and without it the
            # tile loses its corners. Its width is a fraction of the canvas,
            # so it has to be specified per profile — one value cannot be a
            # pixel wide at 16 px and still a rim rather than a band at 512.
            hair = box * profile["edge"]
            body.append(
                f'<rect x="{_fmt(hair / 2)}" y="{_fmt(hair / 2)}" '
                f'width="{_fmt(box - hair)}" height="{_fmt(box - hair)}" '
                f'rx="{_fmt(r - hair / 2)}" fill="none" stroke="{PLATE_EDGE}" '
                f'stroke-width="{_fmt(hair)}"/>'
            )

    keep = MONO if mono else KEEP
    copy = MONO if mono else COPY
    body.append(f'<path d="{piece}" fill="{keep}"/>')
    body.append(f'<path d="{piece}" fill="{copy}" transform="{transform}"/>')

    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{_fmt(box)}" height="{_fmt(box)}" '
        f'viewBox="0 0 {_fmt(box)} {_fmt(box)}" role="img" aria-label="{label}">'
        f"<title>{label}</title>" + "".join(body) + "</svg>"
    )


# ---------------------------------------------------------------------------
# The five tray marks — DESIGN_SYSTEM.md §7, transcribed
# ---------------------------------------------------------------------------

CANVAS = 32.0
CENTRE = 16.0
RING_RADIUS = 10.5
RING_STROKE = 3.0
ARC_START_DEG = 85.0
ARC_SWEEP_DEG = 280.0
PIP_X = 23.0
PIP_Y = 23.0
PIP_RADIUS = 6.0
PIP_KNOCKOUT = 1.5
PIP_STROKE = 2.4
RUNNING_FRAMES = 12
RUNNING_ARC_DEG = 90.0

TRAY_STATES = ("idle", "running", "attention", "paused", "failed")
TRAY_VARIANTS = ("light", "dark", "template")

RING_INK = {"light": "#22262E", "dark": "#E8ECF2", "template": "#000000"}


def _chromatic(variant: str, colour: str) -> str:
    return "#000000" if variant == "template" else colour


def _on_ring(degrees: float) -> tuple[float, float]:
    radians = math.radians(degrees)
    return (CENTRE + RING_RADIUS * math.cos(radians), CENTRE + RING_RADIUS * math.sin(radians))


def _arc(start_deg: float, sweep_deg: float) -> str:
    x0, y0 = _on_ring(start_deg)
    x1, y1 = _on_ring(start_deg + sweep_deg)
    large = 1 if abs(sweep_deg) > 180 else 0
    return f"M {x0:.3f} {y0:.3f} A {RING_RADIUS} {RING_RADIUS} 0 {large} 1 {x1:.3f} {y1:.3f}"


def tray_svg(state: str, variant: str, frame: int = 0) -> str:
    ring = RING_INK[variant]
    defs: list[str] = []
    body: list[str] = []

    has_pip = state in ("attention", "failed")
    mask_attr = ""
    if has_pip:
        defs.append(
            '<mask id="pip-knockout">'
            f'<rect x="0" y="0" width="{CANVAS:g}" height="{CANVAS:g}" fill="#fff"/>'
            f'<circle cx="{PIP_X:g}" cy="{PIP_Y:g}" r="{PIP_RADIUS + PIP_KNOCKOUT:.2f}" '
            'fill="#000"/></mask>'
        )
        mask_attr = ' mask="url(#pip-knockout)"'

    if state == "idle":
        body.append(
            f'<circle cx="{CENTRE:g}" cy="{CENTRE:g}" r="{RING_RADIUS:g}" fill="none" '
            f'stroke="{ring}" stroke-width="{RING_STROKE:g}"/>'
            f'<circle cx="{CENTRE:g}" cy="{CENTRE:g}" r="3" fill="{ring}"/>'
        )
    elif state == "paused":
        body.append(
            f'<circle cx="{CENTRE:g}" cy="{CENTRE:g}" r="{RING_RADIUS:g}" fill="none" '
            f'stroke="{ring}" stroke-width="{RING_STROKE:g}"/>'
        )
        for centre_x in (13.3, 18.7):
            body.append(
                f'<rect x="{centre_x - 1.7:.2f}" y="10.5" width="3.4" height="11" rx="1.7" '
                f'fill="{ring}"/>'
            )
    elif state == "running":
        base = _chromatic(variant, "#3A4250")
        moving = _chromatic(variant, "#5B9BFF")
        body.append(
            f'<path d="{_arc(ARC_START_DEG, ARC_SWEEP_DEG)}" fill="none" stroke="{base}" '
            f'stroke-width="{RING_STROKE:g}" stroke-linecap="round"/>'
        )
        offset = frame * (360.0 / RUNNING_FRAMES)
        body.append(
            f'<path d="{_arc(ARC_START_DEG + offset, RUNNING_ARC_DEG)}" fill="none" '
            f'stroke="{moving}" stroke-width="{RING_STROKE:g}" stroke-linecap="round"/>'
        )
    elif state in ("attention", "failed"):
        body.append(
            f'<path d="{_arc(ARC_START_DEG, ARC_SWEEP_DEG)}" fill="none" stroke="{ring}" '
            f'stroke-width="{RING_STROKE:g}" stroke-linecap="round"{mask_attr}/>'
        )
        if state == "attention":
            fill = _chromatic(variant, "#E0A83A")
            glyph = (
                f'<path d="M {PIP_X:g} 19.8 L {PIP_X:g} 23.4" stroke="{{ink}}" '
                f'stroke-width="{PIP_STROKE:g}" stroke-linecap="round"/>'
                f'<circle cx="{PIP_X:g}" cy="26" r="1.3" fill="{{ink}}"/>'
            )
            chromatic_ink = "#1A1206"
        else:
            fill = _chromatic(variant, "#C2313A")
            glyph = (
                '<path d="M 20.9 20.9 L 25.1 25.1 M 25.1 20.9 L 20.9 25.1" stroke="{ink}" '
                f'stroke-width="{PIP_STROKE:g}" stroke-linecap="round"/>'
            )
            chromatic_ink = "#FFFFFF"

        if variant == "template":
            # DIVERGENCE from crates/app/src/tray/icons.rs, recorded in
            # assets/tray/README.md. A template image is alpha only: a white
            # glyph painted on a black pip is opaque, so it disappears and
            # `attention` and `failed` become the same picture. The glyph has
            # to be a hole. Drawn as a mask on the pip, in black-on-white.
            defs.append(
                f'<mask id="pip-glyph">'
                f'<circle cx="{PIP_X:g}" cy="{PIP_Y:g}" r="{PIP_RADIUS:g}" fill="#fff"/>'
                + glyph.replace("{ink}", "#000")
                + "</mask>"
            )
            body.append(
                f'<circle cx="{PIP_X:g}" cy="{PIP_Y:g}" r="{PIP_RADIUS:g}" fill="{fill}" '
                'mask="url(#pip-glyph)"/>'
            )
        else:
            body.append(
                f'<circle cx="{PIP_X:g}" cy="{PIP_Y:g}" r="{PIP_RADIUS:g}" fill="{fill}"/>'
                + glyph.replace("{ink}", chromatic_ink)
            )
    else:
        raise ValueError(f"unknown tray state {state!r}")

    title = f"superbackup — {state}"
    defs_block = f"<defs>{''.join(defs)}</defs>" if defs else ""
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{CANVAS:g}" height="{CANVAS:g}" '
        f'viewBox="0 0 {CANVAS:g} {CANVAS:g}" fill="none" stroke-linejoin="round" '
        f'stroke-linecap="round" role="img" aria-label="{title} ({variant})">'
        f"<title>{title}</title>{defs_block}{''.join(body)}</svg>"
    )


def tray_stem(state: str, variant: str, frame: int = 0) -> str:
    """Matches `IconKey::stem()` in crates/app/src/tray/icons.rs."""
    if state == "running":
        return f"running-{frame:02d}-{variant}"
    return f"{state}-{variant}"
