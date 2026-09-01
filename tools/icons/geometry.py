"""Geometry for every superbackup mark.

Two independent families live here.

**The application icon** (`app_svg`) is *The Interlock*: one rounded square cut
in two along a single stepped line, the lower piece slid down-right so the cut
becomes a visible kerf. The two pieces are congruent — piece B is literally
piece A rotated 180° — which is the whole idea: a backup that is not identical
to the original is not a backup. The step in the cut is the increment; the
down-right slide is the copy leaving the machine, and it lands on the same
diagonal the tray badge occupies.

**The tray marks** (`tray_svg`) are not a design decision made here. They are
`design/DESIGN_SYSTEM.md` §7.1/§7.2 transcribed, and deliberately mirror
`crates/app/src/tray/icons.rs`, which generates the same geometry at run time.
They are not a second family either: a tray mark is the *application* mark in
one ink, with a circular well knocked out of its bottom-right corner and one
status glyph in that well. The Rust module and this file are held to the same
pixels by `the_checked_in_reference_svgs_match_what_the_program_draws`.
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
#
# Not a design decision made here. This mirrors
# `crates/app/src/tray/icons.rs`, which generates the same geometry at run
# time, so that the reference SVGs in `assets/tray/` and the marks the running
# program draws are the same drawing. Every constant below has the same name as
# its Rust counterpart.
#
# The tray mark is the *application mark* — the same interlock `app_svg` draws,
# in a single ink — with a circular well knocked out of its bottom-right corner
# and one bold status glyph in that well. The state is carried by the badge's
# silhouette: a disc, a ring with a travelling gap, a triangle, two bars, a
# cross. Never by an interior detail, and never by colour.

CANVAS = 32.0

MARK_SPAN = 24.5
MARK_ORIGIN = 0.7
KERF = 3.1
CORNER_FRACTION = 0.20
STEP_FRACTION = 0.30
INNER_FRACTION = 0.045

BADGE_X = 24.5
BADGE_Y = 24.5
BADGE_RADIUS = 6.8
BADGE_STROKE = 3.4
BADGE_CLEARANCE = 2.8
DIAGONAL_WEIGHT = 1.20

IDLE_DISC = 0.88
TRIANGLE_CORNER = 0.20
PAUSE_OFFSET = 0.97
PAUSE_HEIGHT = 1.45

RUNNING_FRAMES = 12
RUNNING_SWEEP_DEG = 250.0
RUNNING_PHASE_DEG = 55.0

TRAY_STATES = ("idle", "running", "attention", "paused", "failed")
TRAY_VARIANTS = ("light", "dark", "template")

MARK_INK = {"light": "#22262E", "dark": "#E8ECF2", "template": "#000000"}

# Variant-aware for every state. The previous design fixed `#E0A83A` and
# `#C2313A` for both taskbars, which are 1.92:1 on a light one and 2.94:1 on a
# dark one — both under SC 1.4.11's 3:1, and the second is the *failure* state.
BADGE_INK = {
    "light": {
        "idle": "#12793B",       # §2.2 success
        "running": "#155FCC",    # §2.2 info
        "attention": "#8A5B00",  # §2.2 warning
        "paused": "#5E6774",     # §2.2 neutral
        "failed": "#B3242B",     # §2.2 danger.fill
    },
    "dark": {
        "idle": "#4FBF6B",       # §2.1 success
        "running": "#5B9BFF",    # §2.1 accent
        "attention": "#E0A83A",  # §2.1 warning
        "paused": "#8B93A5",     # §2.1 neutral
        "failed": "#FF6B72",     # §2.1 danger
    },
}


def badge_ink(variant: str, state: str) -> str:
    return "#000000" if variant == "template" else BADGE_INK[variant][state]


def _ring_radius() -> float:
    """Where the stroked badge glyphs are centred, so their outer edge lands
    exactly on BADGE_RADIUS."""
    return BADGE_RADIUS - BADGE_STROKE / 2


def _cross_reach() -> float:
    return (BADGE_RADIUS - BADGE_STROKE * DIAGONAL_WEIGHT / 2) / math.sqrt(2)


def _on_circle(cx: float, cy: float, radius: float, degrees: float) -> tuple[float, float]:
    r = math.radians(degrees)
    return (cx + radius * math.cos(r), cy + radius * math.sin(r))


def tray_mark() -> tuple[str, str]:
    """The application mark at tray proportions: path data, plus the transform
    that turns the first half into the second."""
    side = MARK_SPAN - KERF
    return interlock_pieces(
        MARK_SPAN,
        side,
        KERF,
        side * CORNER_FRACTION,
        side * STEP_FRACTION,
        side * INNER_FRACTION,
    )


def tray_badge(state: str, frame: int, ink: str) -> str:
    """One status badge, inscribed in a circle of BADGE_RADIUS about the badge
    centre — which is what makes BADGE_CLEARANCE true for every state without
    measuring each shape."""
    stroke = _fmt(BADGE_STROKE)

    if state == "idle":
        return (
            f'<circle cx="{_fmt(BADGE_X)}" cy="{_fmt(BADGE_Y)}" '
            f'r="{_fmt(BADGE_RADIUS * IDLE_DISC)}" fill="{ink}"/>'
        )

    if state == "running":
        # The same circle as `idle`, opened into a ring whose gap travels round
        # it. The animation is alpha, not colour, so it survives the template.
        radius = _ring_radius()
        start = RUNNING_PHASE_DEG + frame * 360.0 / RUNNING_FRAMES
        x0, y0 = _on_circle(BADGE_X, BADGE_Y, radius, start)
        x1, y1 = _on_circle(BADGE_X, BADGE_Y, radius, start + RUNNING_SWEEP_DEG)
        large = 1 if RUNNING_SWEEP_DEG > 180 else 0
        return (
            f'<path d="M {_fmt(x0)} {_fmt(y0)} A {_fmt(radius)} {_fmt(radius)} 0 {large} 1 '
            f'{_fmt(x1)} {_fmt(y1)}" fill="none" stroke="{ink}" stroke-width="{stroke}" '
            'stroke-linecap="round"/>'
        )

    if state == "attention":
        # The largest triangle that fits the badge circle: 1.73 r across by
        # 1.5 r tall, so its flat base and its point cannot be read as a disc.
        points = []
        for k in range(3):
            x, y = _on_circle(BADGE_X, BADGE_Y, BADGE_RADIUS, -90 + 120 * k)
            points.append((x, y, BADGE_RADIUS * TRIANGLE_CORNER))
        return f'<path d="{rounded_polygon(points)}" fill="{ink}" stroke-linejoin="round"/>'

    if state == "paused":
        # The only badge that is not one connected shape. The offset is set by
        # the gap between the bars, which has to clear the 1.5 px floor.
        offset = BADGE_STROKE * PAUSE_OFFSET
        half = _ring_radius() * PAUSE_HEIGHT / 2
        return (
            f'<path d="M {_fmt(BADGE_X - offset)} {_fmt(BADGE_Y - half)} '
            f'L {_fmt(BADGE_X - offset)} {_fmt(BADGE_Y + half)} '
            f'M {_fmt(BADGE_X + offset)} {_fmt(BADGE_Y - half)} '
            f'L {_fmt(BADGE_X + offset)} {_fmt(BADGE_Y + half)}" fill="none" stroke="{ink}" '
            f'stroke-width="{stroke}" stroke-linecap="round"/>'
        )

    if state == "failed":
        # Drawn heavier than the others: a diagonal spreads its coverage over
        # two pixel columns and reads lighter than an axis-aligned stroke of
        # the same width, and `failed` must never be the faintest thing there.
        a = _cross_reach()
        return (
            f'<path d="M {_fmt(BADGE_X - a)} {_fmt(BADGE_Y - a)} '
            f'L {_fmt(BADGE_X + a)} {_fmt(BADGE_Y + a)} '
            f'M {_fmt(BADGE_X + a)} {_fmt(BADGE_Y - a)} '
            f'L {_fmt(BADGE_X - a)} {_fmt(BADGE_Y + a)}" fill="none" stroke="{ink}" '
            f'stroke-width="{_fmt(BADGE_STROKE * DIAGONAL_WEIGHT)}" stroke-linecap="round"/>'
        )

    raise ValueError(f"unknown tray state {state!r}")


def tray_svg(state: str, variant: str, frame: int = 0) -> str:
    if state not in TRAY_STATES:
        raise ValueError(f"unknown tray state {state!r}")
    ink = MARK_INK[variant]
    piece, transform = tray_mark()

    # White keeps, black cuts. The well is knocked out in every state —
    # including `idle`, so that all five marks are pixel-identical outside the
    # badge and the set reads as one application rather than five.
    defs = (
        '<defs><mask id="badge-clearance">'
        f'<rect x="0" y="0" width="{_fmt(CANVAS)}" height="{_fmt(CANVAS)}" fill="#fff"/>'
        f'<circle cx="{_fmt(BADGE_X)}" cy="{_fmt(BADGE_Y)}" '
        f'r="{_fmt(BADGE_RADIUS + BADGE_CLEARANCE)}" fill="#000"/>'
        "</mask></defs>"
    )
    mark = (
        f'<g transform="translate({_fmt(MARK_ORIGIN)} {_fmt(MARK_ORIGIN)})" '
        'mask="url(#badge-clearance)">'
        f'<path d="{piece}" fill="{ink}"/>'
        f'<path d="{piece}" fill="{ink}" transform="{transform}"/>'
        "</g>"
    )
    badge = tray_badge(state, frame, badge_ink(variant, state))

    title = f"superbackup — {state}"
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{_fmt(CANVAS)}" '
        f'height="{_fmt(CANVAS)}" viewBox="0 0 {_fmt(CANVAS)} {_fmt(CANVAS)}" fill="none" '
        f'stroke-linejoin="round" stroke-linecap="round" role="img" '
        f'aria-label="{title} ({variant})">'
        f"<title>{title}</title>{defs}{mark}{badge}</svg>"
    )


def tray_stem(state: str, variant: str, frame: int = 0) -> str:
    """Matches `IconKey::stem()` in crates/app/src/tray/icons.rs."""
    if state == "running":
        return f"running-{frame:02d}-{variant}"
    return f"{state}-{variant}"
