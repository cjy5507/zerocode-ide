#!/usr/bin/env python3
"""Terminal fidelity: one fixture, two terminals, numbers.

  fidelity.py fixture                         the fixture's bytes on stdout (stdlib only)
  fidelity.py palette-zerocode [--prefs P]    the window's terminal palette as JSON
  fidelity.py palette-terminal-app --profile NAME
                                              a Terminal.app profile's palette as JSON
  fidelity.py analyze MANIFEST --out DIR      captures -> report.json + report.md
  fidelity.py synth --out PNG                 a synthetic capture that must analyse clean

`analyze` needs numpy and Pillow (run it with
`uv run --offline --no-project --python 3.12 --with numpy --with pillow`).

Colour is compared in CIELAB (D65) through a Display P3 hub: every capture is
converted from its embedded ICC profile to Display P3 by ColorSync's own
profile, and every expected colour from the space its terminal defines it in
(sRGB for CSS and xterm definitions, the capture's display profile for
Terminal.app's device-RGB palette, Generic RGB for its calibrated colours).
"""

from __future__ import annotations

import argparse
import io
import json
import plistlib
import statistics
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import layout  # noqa: E402

REPO = HERE.parent.parent
THEMES = REPO / "crates" / "zerocode-shell" / "src" / "terminal_themes.json"
PREFS = Path.home() / "Library" / "Application Support" / "dev.zerocode.app" / "preferences.json"
PROFILES = Path("/System/Library/ColorSync/Profiles")
DISPLAY_P3 = PROFILES / "Display P3.icc"
SRGB = PROFILES / "sRGB Profile.icc"
GENERIC_RGB = PROFILES / "Generic RGB Profile.icc"

# The sixteen ANSI names in palette order, as terminal_themes.json spells them.
ANSI_KEYS = (
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    "brightBlack", "brightRed", "brightGreen", "brightYellow",
    "brightBlue", "brightMagenta", "brightCyan", "brightWhite",
)
# Terminal.app's archive keys for the same sixteen, same order.
TERMINAL_APP_ANSI = tuple(
    "ANSI" + ("Bright" if key.startswith("bright") else "") + key.removeprefix("bright").capitalize() + "Color"
    for key in ANSI_KEYS
)
# NSColorSpace codes in an NSColor archive.
NS_CALIBRATED_RGB = 1
NS_DEVICE_RGB = 2
NS_CALIBRATED_WHITE = 3
NS_DEVICE_WHITE = 4

# Display P3 -> XYZ (D65), and the D65 white — CSS Color 4's constants.
P3_TO_XYZ = (
    (0.48657094864821626, 0.26566769316909294, 0.1982172852343625),
    (0.22897456406974884, 0.6917385218365062, 0.079286914093745),
    (0.0, 0.04511338185890257, 1.0439443689009757),
)
D65 = (0.95047, 1.0, 1.08883)

# How much of a cell's box the swatch sampler ignores on each side: text
# edges, background snapping and anti-aliasing all live in the outer quarter.
SWATCH_INSET = 0.25
# Coverage a glyph pixel must reach to count as ink when finding a stroke.
INK_THRESHOLD = 0.5
# A pixel between these two coverages is an anti-aliased edge, not a core or a hole.
PARTIAL_LOW, PARTIAL_HIGH = 0.1, 0.9
# ΔE00 a seam scanline must exceed to be a visible line between two rows of
# one background. 2.3 is CIE's just-noticeable difference.
SEAM_JND = 2.3
# The part of a row's height a bar `|` must ink from end to end to be told
# apart from the symbols beside it (every face draws `|` taller than this).
BAR_BAND = (0.15, 0.85)
# How close to the strongest column's score another column must come to count
# as a candidate for the bar.
BAR_TIE = 0.9
# The percentile of a pixel column's coverage down the band that scores it.
BAR_PERCENTILE = 20


# --------------------------------------------------------------------------- palettes

def palette_zerocode(prefs_path: Path) -> dict:
    prefs = json.loads(prefs_path.read_text())["data"]
    term = prefs["terminal_prefs"]
    dark = prefs.get("theme", "dark") != "light" or not term.get("use_separate_light_theme", True)
    theme_name = term["theme_dark"] if dark else term["theme_light"]
    themes = json.loads(THEMES.read_text())
    theme = {**themes[theme_name], **(term.get("color_overrides") or {})}

    def hex_rgb(value: str) -> list[int]:
        value = value.lstrip("#")
        return [int(value[i:i + 2], 16) for i in (0, 2, 4)]

    return {
        "terminal": "zerocode",
        "theme": theme_name,
        "font_px": term["font_size"],
        "weight": term["weight"],
        "leading": term["leading"],
        "inactive_pane_opacity": term.get("inactive_pane_opacity"),
        "fg": {"space": "srgb", "rgb": hex_rgb(theme["foreground"])},
        "bg": {"space": "srgb", "rgb": hex_rgb(theme["background"])},
        "ansi": [{"space": "srgb", "rgb": hex_rgb(theme[key])} for key in ANSI_KEYS],
    }


def _ns_color(blob: bytes) -> dict:
    archive = plistlib.loads(blob)
    objects = archive["$objects"]

    def deref(value):
        return objects[value.data] if isinstance(value, plistlib.UID) else value

    root = deref(archive["$top"]["root"])
    space = root["NSColorSpace"]
    if space in (NS_CALIBRATED_RGB, NS_DEVICE_RGB):
        parts = [float(x) for x in deref(root["NSRGB"]).rstrip(b"\x00").split()]
    elif space in (NS_CALIBRATED_WHITE, NS_DEVICE_WHITE):
        parts = [float(x) for x in deref(root["NSWhite"]).rstrip(b"\x00").split()]
        parts = [parts[0]] * 3 + parts[1:]
    else:
        raise ValueError(f"NSColorSpace {space} is not one this reader knows")
    alpha = parts[3] if len(parts) > 3 else 1.0
    name = {
        NS_CALIBRATED_RGB: "generic-rgb", NS_DEVICE_RGB: "device-rgb",
        NS_CALIBRATED_WHITE: "generic-rgb", NS_DEVICE_WHITE: "device-rgb",
    }[space]
    return {"space": name, "rgb": [round(c * 255) for c in parts[:3]], "unit": parts[:3], "alpha": alpha}


def palette_terminal_app(profile: str, plist: bytes) -> dict:
    prefs = plistlib.loads(plist)
    settings = prefs.get("Window Settings", {}).get(profile)
    if settings is None:
        raise SystemExit(f"Terminal.app has no stored profile named {profile!r}")
    font = plistlib.loads(settings["Font"])
    font_root = font["$objects"][font["$top"]["root"].data]
    font_name = font["$objects"][font_root["NSName"].data]
    return {
        "terminal": "terminal-app",
        "profile": profile,
        "font": font_name,
        "font_px": font_root["NSSize"],
        "antialias": settings.get("FontAntialias"),
        "width_spacing": settings.get("FontWidthSpacing"),
        "height_spacing": settings.get("FontHeightSpacing"),
        "background_blur": settings.get("BackgroundBlur"),
        "fg": _ns_color(settings["TextColor"]),
        "bold_fg": _ns_color(settings["TextBoldColor"]) if "TextBoldColor" in settings else None,
        "bg": _ns_color(settings["BackgroundColor"]),
        "ansi": [_ns_color(settings[key]) for key in TERMINAL_APP_ANSI],
    }


# --------------------------------------------------------------------------- colour

def _np():
    import numpy
    return numpy


def _cms():
    from PIL import Image, ImageCms
    return Image, ImageCms


def to_p3(image, profile_bytes: bytes | None):
    """A capture in Display P3, as float 0..1 [H, W, 3]."""
    Image, ImageCms = _cms()
    np = _np()
    source = (
        ImageCms.ImageCmsProfile(io.BytesIO(profile_bytes))
        if profile_bytes
        else ImageCms.getOpenProfile(str(SRGB))
    )
    rgb = image.convert("RGB")
    converted = ImageCms.profileToProfile(
        rgb, source, ImageCms.getOpenProfile(str(DISPLAY_P3)),
        renderingIntent=ImageCms.Intent.RELATIVE_COLORIMETRIC, outputMode="RGB",
    )
    return np.asarray(converted, dtype=np.float64) / 255.0


def expected_p3(colors: list[dict], display_profile: bytes | None):
    """Each {space, rgb} as Display P3 floats, through ColorSync profiles."""
    Image, ImageCms = _cms()
    np = _np()
    out = np.zeros((len(colors), 3))
    p3 = ImageCms.getOpenProfile(str(DISPLAY_P3))
    by_space: dict[str, list[int]] = {}
    for at, color in enumerate(colors):
        by_space.setdefault(color["space"], []).append(at)
    for space, indices in by_space.items():
        if space == "srgb":
            source = ImageCms.getOpenProfile(str(SRGB))
        elif space == "generic-rgb":
            source = ImageCms.getOpenProfile(str(GENERIC_RGB))
        elif space == "device-rgb":
            # Measured, not assumed (2026-09-17, Terminal.app 2.15 on macOS
            # 26.3): its device-RGB palette lands on screen as sRGB — read
            # through the capture's own profile it was off by ΔE00 7, read as
            # sRGB by 0.2. AppKit resolves device RGB against sRGB now.
            source = ImageCms.getOpenProfile(str(SRGB))
        else:
            raise ValueError(f"unknown colour space {space}")
        strip = Image.new("RGB", (len(indices), 1))
        strip.putdata([tuple(colors[i]["rgb"]) for i in indices])
        converted = ImageCms.profileToProfile(
            strip, source, p3, renderingIntent=ImageCms.Intent.RELATIVE_COLORIMETRIC, outputMode="RGB",
        )
        values = np.asarray(converted, dtype=np.float64)[0] / 255.0
        for slot, i in enumerate(indices):
            out[i] = values[slot]
    return out


def linear(p3):
    np = _np()
    p3 = np.asarray(p3, dtype=np.float64)
    return np.where(p3 <= 0.04045, p3 / 12.92, ((p3 + 0.055) / 1.055) ** 2.4)


def luminance(p3):
    np = _np()
    return linear(p3) @ np.array(P3_TO_XYZ[1])


def lab(p3):
    np = _np()
    xyz = linear(p3) @ np.array(P3_TO_XYZ).T
    ratio = xyz / np.array(D65)
    epsilon, kappa = 216 / 24389, 24389 / 27
    f = np.where(ratio > epsilon, np.cbrt(ratio), (kappa * ratio + 16) / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], axis=-1)


def delta_e2000(lab1, lab2):
    """CIEDE2000 (Sharma, Wu & Dalal 2005), vectorised."""
    np = _np()
    L1, a1, b1 = np.moveaxis(np.asarray(lab1, dtype=np.float64), -1, 0)
    L2, a2, b2 = np.moveaxis(np.asarray(lab2, dtype=np.float64), -1, 0)
    C1 = np.hypot(a1, b1)
    C2 = np.hypot(a2, b2)
    Cbar = (C1 + C2) / 2
    G = 0.5 * (1 - np.sqrt(Cbar**7 / (Cbar**7 + 25**7)))
    a1p, a2p = (1 + G) * a1, (1 + G) * a2
    C1p, C2p = np.hypot(a1p, b1), np.hypot(a2p, b2)
    h1p = np.degrees(np.arctan2(b1, a1p)) % 360
    h2p = np.degrees(np.arctan2(b2, a2p)) % 360
    dLp = L2 - L1
    dCp = C2p - C1p
    dh = h2p - h1p
    dh = np.where(C1p * C2p == 0, 0, np.where(dh > 180, dh - 360, np.where(dh < -180, dh + 360, dh)))
    dHp = 2 * np.sqrt(C1p * C2p) * np.sin(np.radians(dh / 2))
    Lbarp = (L1 + L2) / 2
    Cbarp = (C1p + C2p) / 2
    hsum = h1p + h2p
    hbarp = np.where(
        C1p * C2p == 0, hsum,
        np.where(np.abs(h1p - h2p) <= 180, hsum / 2, np.where(hsum < 360, (hsum + 360) / 2, (hsum - 360) / 2)),
    )
    T = (1 - 0.17 * np.cos(np.radians(hbarp - 30)) + 0.24 * np.cos(np.radians(2 * hbarp))
         + 0.32 * np.cos(np.radians(3 * hbarp + 6)) - 0.20 * np.cos(np.radians(4 * hbarp - 63)))
    dtheta = 30 * np.exp(-(((hbarp - 275) / 25) ** 2))
    Rc = 2 * np.sqrt(Cbarp**7 / (Cbarp**7 + 25**7))
    Sl = 1 + 0.015 * (Lbarp - 50) ** 2 / np.sqrt(20 + (Lbarp - 50) ** 2)
    Sc = 1 + 0.045 * Cbarp
    Sh = 1 + 0.015 * Cbarp * T
    Rt = -np.sin(np.radians(2 * dtheta)) * Rc
    return np.sqrt((dLp / Sl) ** 2 + (dCp / Sc) ** 2 + (dHp / Sh) ** 2 + Rt * (dCp / Sc) * (dHp / Sh))


# --------------------------------------------------------------------------- grid

# The calibration indices of layout.CAL_COL / CAL_ROW, by the name `_classify` reads.
CAL_NAMES = {201: "magenta", 46: "green", 21: "blue", 226: "yellow"}


def _classify(p3, marker):
    """Boolean mask of pixels that read as one calibration colour."""
    r, g, b = p3[..., 0], p3[..., 1], p3[..., 2]
    hi, lo = 0.62, 0.45
    return {
        "magenta": (r > hi) & (b > hi) & (g < lo),
        "green": (g > hi) & (r < hi) & (b < lo),
        "blue": (b > hi) & (r < 0.3) & (g < 0.3),
        "yellow": (r > hi) & (g > hi) & (b < lo),
    }[marker]


def _runs(labels):
    """[(label, start, end_exclusive)] of equal consecutive labels, labels may be None."""
    runs = []
    start = 0
    for at in range(1, len(labels) + 1):
        if at == len(labels) or labels[at] != labels[start]:
            runs.append((labels[start], start, at))
            start = at
    return runs


def _boundaries(line, first, second, count):
    """Edges between `count` alternating first/second cells along a 1-D pixel line.

    An edge with a blended pixel between the two colours is placed inside that
    pixel by how far its value travelled from one colour to the other, on the
    channel that differs most — the rectangle a renderer snapped is recovered
    to a tenth of a pixel instead of rounded to a whole one.
    """
    np = _np()
    a = _classify(line, first)
    b = _classify(line, second)
    labels = [("A" if a[i] else "B" if b[i] else None) for i in range(len(line))]
    runs = [run for run in _runs(labels)]
    best = None
    for start in range(len(runs)):
        chain = []
        at = start
        expect = None
        while at < len(runs):
            label, lo, hi = runs[at]
            if label is None:
                if hi - lo <= 2 and chain:
                    at += 1
                    continue
                break
            if expect is not None and label != expect:
                break
            chain.append(runs[at])
            expect = "B" if label == "A" else "A"
            at += 1
        if len(chain) >= count and (best is None or len(chain) > len(best)):
            best = chain
    if best is None:
        return None
    best = best[:count]
    edges = [float(best[0][1])]
    for left, right in zip(best, best[1:]):
        gap_lo, gap_hi = left[2], right[1]
        colour_a = line[left[1]:left[2]].mean(axis=0)
        colour_b = line[right[1]:right[2]].mean(axis=0)
        channel = int(np.argmax(np.abs(colour_b - colour_a)))
        span = colour_b[channel] - colour_a[channel]
        travelled = sum(
            float(np.clip((line[x][channel] - colour_a[channel]) / span, 0, 1)) for x in range(gap_lo, gap_hi)
        )
        edges.append(gap_lo + (gap_hi - gap_lo) - travelled if gap_hi > gap_lo else float(gap_lo))
    edges.append(float(best[-1][2]))
    return edges


def locate_grid(p3):
    """Column and row edges in device pixels, read off the calibration markers."""
    np = _np()
    height = p3.shape[0]
    first, second = ("magenta", "green")
    alternations = []
    for y in range(height):
        edges = _boundaries_fast_count(p3[y], first, second)
        alternations.append(edges)
    band = [y for y, n in enumerate(alternations) if n >= layout.COLS - 2]
    if not band:
        raise SystemExit("no calibration row found: is the fixture on screen and unobstructed?")
    top_band = [y for y in band if y - band[0] < (band[-1] - band[0]) / 2]
    bottom_band = [y for y in band if y not in top_band]
    y_mid_top = top_band[len(top_band) // 2]
    y_mid_bottom = bottom_band[len(bottom_band) // 2] if bottom_band else None
    cols_top = _boundaries(p3[y_mid_top], first, second, layout.COLS)
    cols_bottom = _boundaries(p3[y_mid_bottom], first, second, layout.COLS) if y_mid_bottom else None
    if cols_top is None:
        raise SystemExit(f"the top calibration row did not resolve into {layout.COLS} cells")
    x_left = int((cols_top[0] + cols_top[1]) / 2)
    x_right = int((cols_top[-2] + cols_top[-1]) / 2)
    rows = {"left": _row_edges(p3[:, x_left], CAL_NAMES[layout.CAL_COL[0]])}
    # The right-hand markers sit after every glyph of their row, so a row whose
    # glyphs advance off the grid carries its marker away with it. That is a
    # finding, not a broken capture: the right side is read when it resolves
    # and reported as unresolved when it does not.
    try:
        rows["right"] = _row_edges(p3[:, x_right], CAL_NAMES[layout.CAL_COL[(layout.COLS - 1) % 2]])
    except SystemExit:
        rows["right"] = None
    return {
        "cols": cols_top,
        "cols_bottom": cols_bottom,
        "rows": rows["left"],
        "rows_right": rows["right"],
    }


def _boundaries_fast_count(line, first, second):
    a = _classify(line, first)
    b = _classify(line, second)
    np = _np()
    labels = np.where(a, 1, np.where(b, 2, 0))
    nonzero = labels[labels != 0]
    if nonzero.size == 0:
        return 0
    return int(np.count_nonzero(np.diff(nonzero))) + 1


def _row_edges(column, end_marker):
    """Row edges down column 0 or 79: the calibration rows' colour at both ends,
    the row markers' alternation between them."""
    np = _np()
    masks = {name: _classify(column, name) for name in CAL_NAMES.values()}
    labels = [next((name for name, mask in masks.items() if mask[y]), None) for y in range(len(column))]
    runs = _runs(labels)
    wanted = [end_marker] + [CAL_NAMES[layout.CAL_ROW[r % 2]] for r in range(1, layout.ROWS - 1)] + [end_marker]
    for start in range(len(runs)):
        chain = []
        at = start
        while at < len(runs) and len(chain) < len(wanted):
            label, lo, hi = runs[at]
            if label is None and hi - lo <= 2 and chain:
                at += 1
                continue
            if label != wanted[len(chain)]:
                break
            chain.append(runs[at])
            at += 1
        if len(chain) == len(wanted):
            edges = [float(chain[0][1])]
            for above, below in zip(chain, chain[1:]):
                gap_lo, gap_hi = above[2], below[1]
                colour_a = column[above[1]:above[2]].mean(axis=0)
                colour_b = column[below[1]:below[2]].mean(axis=0)
                channel = int(np.argmax(np.abs(colour_b - colour_a)))
                span = colour_b[channel] - colour_a[channel]
                travelled = sum(
                    float(np.clip((column[y][channel] - colour_a[channel]) / span, 0, 1)) for y in range(gap_lo, gap_hi)
                )
                edges.append(gap_lo + (gap_hi - gap_lo) - travelled if gap_hi > gap_lo else float(gap_lo))
            edges.append(float(chain[-1][2]))
            return edges
    raise SystemExit(f"the row markers did not resolve into {layout.ROWS} rows")


def cell_box(grid, row, col, width=1):
    return grid["cols"][col], grid["cols"][col + width], grid["rows"][row], grid["rows"][row + 1]


# --------------------------------------------------------------------------- measures

def sample(p3, box, inset=SWATCH_INSET):
    np = _np()
    x0, x1, y0, y1 = box
    w, h = x1 - x0, y1 - y0
    xa, xb = int(round(x0 + w * inset)), int(round(x1 - w * inset))
    ya, yb = int(round(y0 + h * inset)), int(round(y1 - h * inset))
    patch = p3[ya:max(yb, ya + 1), xa:max(xb, xa + 1)].reshape(-1, 3)
    return np.median(patch, axis=0)


def spacing(edges):
    np = _np()
    steps = np.diff(np.asarray(edges))
    mean = float(steps.mean())
    return {
        "mean_px": round(mean, 4),
        "min_px": round(float(steps.min()), 3),
        "max_px": round(float(steps.max()), 3),
        "stdev_px": round(float(steps.std()), 4),
        "fraction": round(mean - int(mean), 4),
        "distinct_whole_px": sorted({int(round(s)) for s in steps}),
        "span_px": round(float(edges[-1] - edges[0]), 3),
    }


def coverage(p3, box, ground, ink):
    """Per-pixel ink coverage in a box, from luminance between ground and ink."""
    np = _np()
    x0, x1, y0, y1 = (int(round(v)) for v in box)
    patch = luminance(p3[y0:y1, x0:x1])
    return np.clip((patch - luminance(ground)) / (luminance(ink) - luminance(ground)), -0.2, 1.2)


def stroke_metrics(cov, char):
    """Stem width, peak and edge softness of one glyph's coverage map."""
    np = _np()
    h, w = cov.shape
    inked = cov > PARTIAL_LOW
    partial = inked & (cov < PARTIAL_HIGH)
    result = {
        "ink_px2": float(np.clip(cov, 0, 1).sum()),
        "partial_ratio": float(partial.sum() / max(inked.sum(), 1)),
        "peak": float(np.percentile(cov[inked], 95)) if inked.any() else 0.0,
    }
    ys = np.nonzero(inked.any(axis=1))[0]
    xs = np.nonzero(inked.any(axis=0))[0]
    if ys.size and xs.size:
        result["ink_top"], result["ink_bottom"] = int(ys[0]), int(ys[-1] + 1)
        result["ink_left"], result["ink_right"] = int(xs[0]), int(xs[-1] + 1)
    if char in "|l" and ys.size:
        lo, hi = ys[0] + (ys[-1] - ys[0]) // 4, ys[0] + 3 * (ys[-1] - ys[0]) // 4
        widths = np.clip(cov[lo:hi + 1], 0, 1).sum(axis=1)
        result["stem_px"] = float(np.median(widths))
        result["stem_peak"] = float(np.median(cov[lo:hi + 1].max(axis=1)))
        weights = np.clip(cov[lo:hi + 1], 0, 1)
        centre = (weights * np.arange(w)).sum() / max(weights.sum(), 1e-9)
        result["stem_centre_px"] = float(centre)
    if char in "-" and xs.size:
        lo, hi = xs[0] + (xs[-1] - xs[0]) // 4, xs[0] + 3 * (xs[-1] - xs[0]) // 4
        result["bar_px"] = float(np.median(np.clip(cov[:, lo:hi + 1], 0, 1).sum(axis=0)))
        result["bar_peak"] = float(np.median(cov[:, lo:hi + 1].max(axis=0)))
    return result


def summarise(values):
    values = [v for v in values if v is not None]
    if not values:
        return None
    return round(statistics.median(values), 4)


class Capture:
    def __init__(self, spec: dict, base: Path):
        Image, _ = _cms()
        self.name = spec["name"]
        self.scale = float(spec["scale"])
        self.palette = json.loads((base / spec["palette"]).read_text())
        self.font_px = float(spec.get("font_px") or self.palette["font_px"])
        self.path = base / spec["png"]
        image = Image.open(self.path)
        self.icc = image.info.get("icc_profile")
        self.alpha_min = None
        if image.mode == "RGBA":
            self.alpha_min = int(_np().asarray(image)[..., 3].min())
        self.p3 = to_p3(image, self.icc)
        self.grid = locate_grid(self.p3)

    def expect(self, spec: dict, layer: str) -> dict:
        if "ansi" in spec:
            return self.palette["ansi"][spec["ansi"]]
        if "xterm" in spec:
            return {"space": "srgb", "rgb": list(layout.xterm256(spec["xterm"]))}
        if "rgb" in spec:
            return {"space": "srgb", "rgb": list(spec["rgb"])}
        if spec.get("default") == "fg":
            return self.palette["fg"]
        return self.palette["bg"]


def analyse(manifest_path: Path, out: Path) -> dict:
    np = _np()
    manifest = json.loads(manifest_path.read_text())
    base = manifest_path.parent
    captures = [Capture(spec, base) for spec in manifest["captures"]]
    probes = layout.probes()
    report: dict = {"captures": {}, "pairs": []}

    for cap in captures:
        entry: dict = {
            "png": str(cap.path),
            "scale": cap.scale,
            "font_px": cap.font_px,
            "icc_profile_bytes": len(cap.icc or b""),
            "alpha_min": cap.alpha_min,
            "palette": {k: v for k, v in cap.palette.items() if k not in ("ansi",)},
        }
        grid = cap.grid
        entry["grid"] = {
            "cell_width": spacing(grid["cols"]),
            "cell_height": spacing(grid["rows"]),
            "cell_width_css_px": round(spacing(grid["cols"])["mean_px"] / cap.scale, 4),
            "cell_height_css_px": round(spacing(grid["rows"])["mean_px"] / cap.scale, 4),
            "col_edges_px": [round(v, 2) for v in grid["cols"]],
            "row_edges_px": [round(v, 2) for v in grid["rows"]],
            "right_column_row_drift_px": (
                round(float(np.max(np.abs(np.array(grid["rows"]) - np.array(grid["rows_right"])))), 3)
                if grid["rows_right"] is not None else "unresolved"
            ),
        }
        if grid["cols_bottom"]:
            entry["grid"]["bottom_row_col_drift_px"] = round(float(np.max(np.abs(np.array(grid["cols"]) - np.array(grid["cols_bottom"])))), 3)

        grounds = {probe["name"]: sample(cap.p3, cell_box(grid, probe["row"], probe["col"], probe["width"]))
                   for probe in probes if probe["kind"] == "ground"}
        ground_default = grounds["default"]
        swatches = []
        wanted = []
        for probe in probes:
            if probe["kind"] != "swatch":
                continue
            measured = sample(cap.p3, cell_box(grid, probe["row"], probe["col"], probe["width"]))
            swatches.append((probe, measured))
            wanted.append(cap.expect(probe["expect"], probe["layer"]))
        expected = expected_p3(wanted, cap.icc)
        rows = []
        for (probe, measured), exp, spec in zip(swatches, expected, wanted):
            rows.append({
                "row": probe["row"], "col": probe["col"], "layer": probe["layer"], "group": probe["group"],
                "expect": probe["expect"], "name": probe.get("name"), "road": probe.get("road"),
                "space": spec["space"],
                "measured_p3": [round(float(v) * 255, 1) for v in measured],
                "expected_p3": [round(float(v) * 255, 1) for v in exp],
                "de00": round(float(delta_e2000(lab(measured), lab(exp))), 2),
            })
        entry["swatches"] = rows
        entry["ground_default_p3"] = [round(float(v) * 255, 1) for v in ground_default]
        exp_ground = expected_p3([cap.palette["bg"]], cap.icc)[0]
        entry["ground_default_de00"] = round(float(delta_e2000(lab(ground_default), lab(exp_ground))), 2)

        attr = {}
        for probe in probes:
            if probe["kind"] == "attribute":
                attr[probe["name"]] = sample(cap.p3, cell_box(grid, probe["row"], probe["col"], probe["width"]))
        entry["attributes_p3"] = {k: [round(float(c) * 255, 1) for c in v] for k, v in attr.items()}
        black_ground = grounds["black"]

        def dim_ratio(normal, dimmed, ground):
            n, d, g = luminance(normal), luminance(dimmed), luminance(ground)
            return round(float((d - g) / (n - g)), 3) if abs(n - g) > 1e-6 else None

        entry["dim_luminance_ratio"] = {
            "default": dim_ratio(attr["default"], attr["default-dim"], ground_default),
            "white-on-black": dim_ratio(attr["white-on-black"], attr["white-on-black-dim"], black_ground),
            "red": dim_ratio(attr["red"], attr["red-dim"], ground_default),
            "claude": dim_ratio(attr["claude"], attr["claude-dim"], ground_default),
        }
        entry["bold_colour_de00"] = {
            "default": round(float(delta_e2000(lab(attr["default"]), lab(attr["default-bold"]))), 2),
            "red": round(float(delta_e2000(lab(attr["red"]), lab(attr["red-bold"]))), 2),
        }

        strokes = {}
        for probe in probes:
            if probe["kind"] != "strokes":
                continue
            ink = attr[probe["ink"]] if probe["ink"] in attr else attr["default"]
            ground = black_ground if probe["ink"] != "default" else ground_default
            per_char = {}
            for group in probe["groups"]:
                metrics = []
                for k in range(group["count"]):
                    box = cell_box(grid, probe["row"], group["col"] + k)
                    metrics.append(stroke_metrics(coverage(cap.p3, box, ground, ink), group["char"]))
                keys = sorted({key for m in metrics for key in m})
                per_char[group["char"]] = {key: summarise([m.get(key) for m in metrics]) for key in keys}
                if "stem_centre_px" in keys:
                    centres = [m["stem_centre_px"] for m in metrics if "stem_centre_px" in m]
                    per_char[group["char"]]["stem_centre_spread_px"] = round(max(centres) - min(centres), 3)
                for key in ("stem_px", "bar_px", "ink_px2"):
                    value = per_char[group["char"]].get(key)
                    if value is not None:
                        per_char[group["char"]][key.replace("_px2", "_em2").replace("_px", "_em")] = round(
                            value / (cap.scale * cap.font_px) ** (2 if key.endswith("px2") else 1), 5)
            strokes[probe["ink"]] = per_char
        entry["strokes"] = strokes

        def measured_ink(spec):
            # The ink a line is drawn in, as THIS terminal drew it: a solid
            # block of the same request, never the definition — a terminal
            # whose colour is off would otherwise read as a thinner line.
            if spec == {"xterm": 231}:
                return attr["white-on-black"]
            for (probe, measured) in swatches:
                if probe["layer"] == "fg" and probe["expect"] == {k: list(v) if isinstance(v, tuple) else v for k, v in spec.items()}:
                    return measured
                if probe["layer"] == "fg" and probe["expect"] == spec:
                    return measured
            raise ValueError(f"no solid swatch for ink {spec}")

        lines = {}
        for probe in probes:
            if probe["kind"] == "vline":
                first, last = probe["rows"]
                x0, x1, _, _ = cell_box(grid, first, probe["col"])
                y0 = (grid["rows"][first] + grid["rows"][first + 1]) / 2
                y1 = (grid["rows"][last] + grid["rows"][last + 1]) / 2
                cov = coverage(cap.p3, (x0, x1, y0, y1), ground_default, measured_ink(probe["ink"]))
                peak = cov.max(axis=1)
                width = np.clip(cov, 0, 1).sum(axis=1)
                gaps = int((peak < INK_THRESHOLD).sum())
                lines[probe["name"]] = {
                    "scanlines": int(len(peak)),
                    "gap_scanlines": gaps,
                    "min_width_px": round(float(width.min()), 3),
                    "median_width_px": round(float(np.median(width)), 3),
                    "width_stdev_px": round(float(width.std()), 3),
                }
            if probe["kind"] == "hline":
                c0, c1 = probe["cols"]
                x0 = (grid["cols"][c0] + grid["cols"][c0 + 1]) / 2
                x1 = (grid["cols"][c1] + grid["cols"][c1 + 1]) / 2
                _, _, y0, y1 = cell_box(grid, probe["row"], c0)
                cov = coverage(cap.p3, (x0, x1, y0, y1), ground_default, measured_ink(probe["ink"]))
                peak = cov.max(axis=0)
                height = np.clip(cov, 0, 1).sum(axis=0)
                lines[probe["name"]] = {
                    "columns": int(len(peak)),
                    "gap_columns": int((peak < INK_THRESHOLD).sum()),
                    "min_height_px": round(float(height.min()), 3),
                    "median_height_px": round(float(np.median(height)), 3),
                    "height_stdev_px": round(float(height.std()), 3),
                }
        entry["lines"] = lines

        aligned = {}
        pitch = spacing(grid["cols"])["mean_px"]
        for probe in probes:
            if probe["kind"] != "align":
                continue
            symbols = probe.get("symbols") or [{} for _ in probe["bars"]]
            found = {probe["row"]: [], probe["reference_row"]: []}
            for r in found:
                # A bar is the one glyph here whose ink runs the whole middle of
                # the row: score each pixel column by a LOW percentile of its
                # coverage over that band (not the minimum — one anti-aliased
                # scanline would zero it), so a round symbol or an arrow's short
                # shaft beside it cannot outscore it. Bars are tracked left to right, each
                # searched within a cell of where the previous one's shift puts
                # it, so a drift of several cells is followed, not jumped.
                top = grid["rows"][r] + (grid["rows"][r + 1] - grid["rows"][r]) * BAR_BAND[0]
                bottom = grid["rows"][r] + (grid["rows"][r + 1] - grid["rows"][r]) * BAR_BAND[1]
                shift = 0.0
                for at, col in enumerate(probe["bars"]):
                    ink = attr["claude"] if symbols[at].get("styled") == "claude" else attr["default"]
                    expected_centre = (grid["cols"][col] + grid["cols"][col + 1]) / 2
                    predicted = expected_centre + shift
                    left = max(predicted - pitch, grid["cols"][0])
                    right = min(predicted + pitch, grid["cols"][-1])
                    cov = np.clip(coverage(cap.p3, (left, right, top, bottom), ground_default, ink), 0, 1)
                    score = np.percentile(cov, BAR_PERCENTILE, axis=0)
                    if score.max() < INK_THRESHOLD:
                        # Lost: the bar drifted further than a cell from where the
                        # previous one put it. Recorded as such, not guessed.
                        found[r].append(None)
                        continue
                    # Among the columns that ink the band nearly as fully as the
                    # best one, the bar is the one nearest where it was predicted:
                    # an arrow's shaft one cell earlier can tie with it.
                    strong = np.nonzero(score >= score.max() * BAR_TIE)[0]
                    centres = round(left) + strong + 0.5
                    peak = int(strong[np.argmin(np.abs(centres - predicted))])
                    lo, hi = max(peak - 2, 0), min(peak + 3, len(score))
                    weights = cov[:, lo:hi].sum(axis=0)
                    # coverage() crops from the rounded left edge; a pixel's centre is +0.5.
                    centre = round(left) + lo + float((weights * np.arange(len(weights))).sum() / max(weights.sum(), 1e-9)) + 0.5
                    found[r].append(centre)
                    shift = centre - expected_centre
            offsets = []
            for at, col in enumerate(probe["bars"]):
                expected_centre = (grid["cols"][col] + grid["cols"][col + 1]) / 2
                here, there = found[probe["row"]][at], found[probe["reference_row"]][at]
                if here is None or there is None:
                    offsets.append({"col": col, "after": symbols[at].get("after"), "lost": True})
                    continue
                offsets.append({
                    "col": col,
                    "after": symbols[at].get("after"),
                    "styled": symbols[at].get("styled"),
                    "row_px": round(here - expected_centre, 2),
                    "reference_px": round(there - expected_centre, 2),
                    "shift_px": round(here - there, 2),
                    "shift_cells": round((here - there) / pitch, 3),
                })
            aligned[probe["label"]] = offsets
        entry["alignment"] = aligned

        for probe in probes:
            if probe["kind"] == "wide-size":
                col, width = probe["hangul"]
                lcol, lwidth = probe["latin"]
                hbox = cell_box(grid, probe["row"], col, width)
                lbox = cell_box(grid, probe["row"], lcol, lwidth)
                hcov = coverage(cap.p3, hbox, ground_default, attr["default"])
                lcov = coverage(cap.p3, lbox, ground_default, attr["default"])
                hm = stroke_metrics(hcov, "가")
                lm = stroke_metrics(lcov, "H")
                entry["wide_glyphs"] = {
                    "hangul_ink_height_px": hm.get("ink_bottom", 0) - hm.get("ink_top", 0),
                    "latin_cap_height_px": lm.get("ink_bottom", 0) - lm.get("ink_top", 0),
                    "height_ratio": round((hm.get("ink_bottom", 0) - hm.get("ink_top", 0)) / max(lm.get("ink_bottom", 0) - lm.get("ink_top", 0), 1), 3),
                    "hangul_ink_per_cell_px2": round(hm["ink_px2"] / width, 2),
                    "latin_ink_per_cell_px2": round(lm["ink_px2"] / lwidth, 2),
                }
            if probe["kind"] == "seam":
                first, second = probe["rows"]
                c0, c1 = probe["cols"]
                ground_spec = cap.expect(probe["ground"], "bg")
                inside = sample(cap.p3, cell_box(grid, first, c0, c1 - c0))
                edge = grid["rows"][second]
                x0, x1 = int(grid["cols"][c0]), int(grid["cols"][c1 + 1])
                scan = []
                for y in range(int(edge) - 3, int(edge) + 4):
                    line = cap.p3[y, x0:x1]
                    de = delta_e2000(lab(line), lab(np.broadcast_to(inside, line.shape)))
                    scan.append({"y": y, "mean_de00": round(float(de.mean()), 2), "max_de00": round(float(de.max()), 2)})
                entry["seam"] = {
                    "ground_de00": round(float(delta_e2000(lab(inside), lab(expected_p3([ground_spec], cap.icc)[0]))), 2),
                    "scanlines": scan,
                    "visible_seam_scanlines": sum(1 for s in scan if s["mean_de00"] > SEAM_JND),
                }
        report["captures"][cap.name] = entry

    for pair in manifest.get("pairs", []):
        a, b = report["captures"][pair[0]], report["captures"][pair[1]]
        cross = []
        a_by = {(s["row"], s["col"]): s for s in a["swatches"]}
        for s in b["swatches"]:
            other = a_by[(s["row"], s["col"])]
            de = float(delta_e2000(lab(np.array(other["measured_p3"]) / 255), lab(np.array(s["measured_p3"]) / 255)))
            cross.append({"row": s["row"], "col": s["col"], "group": s["group"], "name": s.get("name"), "expect": s["expect"], "layer": s["layer"], "de00": round(de, 2)})
        report["pairs"].append({"a": pair[0], "b": pair[1], "swatches": cross})

    out.mkdir(parents=True, exist_ok=True)
    (out / "report.json").write_text(json.dumps(report, indent=1, ensure_ascii=False))
    (out / "report.md").write_text(render_markdown(report))
    return report


# --------------------------------------------------------------------------- report

def _stats(values):
    values = [v for v in values if v is not None]
    if not values:
        return "—"
    return f"{statistics.mean(values):.2f} / {statistics.median(values):.2f} / {max(values):.2f}"


def render_markdown(report: dict) -> str:
    lines = ["# 터미널 충실도 실측 표", ""]
    names = list(report["captures"])
    lines += ["## 격자", "", "| 캡처 | 배율 | 글꼴 px | 셀 폭 px(기기) | 폭 소수부 | 폭 정수 | 셀 높이 px(기기) | 높이 소수부 | 높이 정수 |", "|---|---|---|---|---|---|---|---|---|"]
    for name in names:
        c = report["captures"][name]
        w, h = c["grid"]["cell_width"], c["grid"]["cell_height"]
        lines.append(f"| {name} | {c['scale']} | {c['font_px']} | {w['mean_px']} | {w['fraction']} | {w['distinct_whole_px']} | {h['mean_px']} | {h['fraction']} | {h['distinct_whole_px']} |")
    lines += ["", "## 색 — 자기 정의 대비 ΔE00 (평균 / 중앙 / 최대)", "", "| 캡처 | ANSI 배경 | ANSI 글자 | xterm 16–255 | CC truecolor 배경 | CC truecolor 글자 | CC ansi256 배경 | CC ansi256 글자 | 회색 램프 | 기본 배경 |", "|---|---|---|---|---|---|---|---|---|---|"]
    for name in names:
        c = report["captures"][name]
        s = c["swatches"]

        def of(group, layer=None):
            return [x["de00"] for x in s if x["group"] == group and (layer is None or x["layer"] == layer)]

        ansi_bg, ansi_fg, xterm = of("ansi", "bg"), of("ansi", "fg"), of("xterm")
        cc_t_bg, cc_t_fg = of("cc-truecolor", "bg"), of("cc-truecolor", "fg")
        cc_a_bg, cc_a_fg = of("cc-ansi256", "bg"), of("cc-ansi256", "fg")
        grey = of("grey")
        lines.append(f"| {name} | {_stats(ansi_bg)} | {_stats(ansi_fg)} | {_stats(xterm)} | {_stats(cc_t_bg)} | {_stats(cc_t_fg)} | {_stats(cc_a_bg)} | {_stats(cc_a_fg)} | {_stats(grey)} | {c['ground_default_de00']} |")
    for pair in report["pairs"]:
        sw = pair["swatches"]
        lines += ["", f"## 같은 바이트, 두 화면 — {pair['a']} ↔ {pair['b']} ΔE00 (평균 / 중앙 / 최대)", "",
                  "| 행 | ΔE00 |", "|---|---|",
                  f"| ANSI 16 (테마 차이) | {_stats([x['de00'] for x in sw if x['group'] == 'ansi'])} |",
                  f"| xterm 16–255 | {_stats([x['de00'] for x in sw if x['group'] == 'xterm'])} |",
                  f"| CC truecolor | {_stats([x['de00'] for x in sw if x['group'] == 'cc-truecolor'])} |",
                  f"| CC ansi256 | {_stats([x['de00'] for x in sw if x['group'] == 'cc-ansi256'])} |",
                  f"| 회색 램프 | {_stats([x['de00'] for x in sw if x['group'] == 'grey'])} |"]
    lines += ["", "## 글리프 — 흰 글자·검은 바탕 (획 폭은 기기 px, em은 글꼴 크기로 나눈 값)", "",
              "| 캡처 | `|` 줄기 px | `|` 줄기 em | `|` 줄기 봉우리 | `-` 막대 px | `-` 막대 em | `H` 잉크 em² | 가장자리 부분 픽셀 비율(H) | 굵게 `|` 줄기 px | 굵게 H 잉크 em² |",
              "|---|---|---|---|---|---|---|---|---|---|"]
    for name in names:
        st = report["captures"][name]["strokes"]
        n, b = st.get("white-on-black", {}), st.get("white-on-black-bold", {})
        lines.append(
            f"| {name} | {n.get('|', {}).get('stem_px')} | {n.get('|', {}).get('stem_em')} | {n.get('|', {}).get('stem_peak')} | "
            f"{n.get('-', {}).get('bar_px')} | {n.get('-', {}).get('bar_em')} | {n.get('H', {}).get('ink_em2')} | {n.get('H', {}).get('partial_ratio')} | "
            f"{b.get('|', {}).get('stem_px')} | {b.get('H', {}).get('ink_em2')} |")
    lines += ["", "## 기본 글자색 (테마 그대로, 사람이 보는 대비)", "", "| 캡처 | `|` 줄기 px | 봉우리 | `H` 잉크 em² | 부분 픽셀 비율(H) |", "|---|---|---|---|---|"]
    for name in names:
        d = report["captures"][name]["strokes"].get("default", {})
        lines.append(f"| {name} | {d.get('|', {}).get('stem_px')} | {d.get('|', {}).get('stem_peak')} | {d.get('H', {}).get('ink_em2')} | {d.get('H', {}).get('partial_ratio')} |")
    lines += ["", "## 선 끊김·줄 이음매·흐림(dim)", "", "| 캡처 | 긴 세로선 끊긴 주사선 | 표 왼쪽 세로선 끊긴 주사선 | 둥근 상자 윗선 끊긴 열 | 이음매 주사선 | dim 휘도비(기본) | dim 휘도비(흰색) |", "|---|---|---|---|---|---|---|"]
    for name in names:
        c = report["captures"][name]
        ln = c["lines"]
        lines.append(
            f"| {name} | {ln['long-vertical']['gap_scanlines']}/{ln['long-vertical']['scanlines']} | {ln['table-left']['gap_scanlines']}/{ln['table-left']['scanlines']} | "
            f"{ln['rounded-top']['gap_columns']}/{ln['rounded-top']['columns']} | {c.get('seam', {}).get('visible_seam_scanlines')} | "
            f"{c['dim_luminance_ratio']['default']} | {c['dim_luminance_ratio']['white-on-black']} |")
    lines += ["", "## 정렬 — 같은 열의 `|`가 기준 줄보다 밀린 셀 수", "", "| 캡처 | 한글 줄 최대 | 대체 글리프 줄 최대 | 대체 글리프별 (앞 글자: 셀) |", "|---|---|---|---|"]
    for name in names:
        al = report["captures"][name]["alignment"]
        def worst_of(rows):
            if any(x.get("lost") for x in rows):
                return "lost"
            return f"{max((abs(x['shift_cells']) for x in rows), default=0):.3f}"

        hangul = worst_of(al.get("hangul", []))
        fall = al.get("fallback", [])
        worst = worst_of(fall)
        per = " ".join(f"{x['after']}:" + ("lost" if x.get("lost") else f"{x['shift_cells']:+.2f}") for x in fall)
        lines.append(f"| {name} | {hangul} | {worst} | {per} |")
    lines += ["", "## 한글 크기", "", "| 캡처 | 한글 잉크 높이 px | 라틴 H 높이 px | 비 |", "|---|---|---|---|"]
    for name in names:
        w = report["captures"][name].get("wide_glyphs", {})
        lines.append(f"| {name} | {w.get('hangul_ink_height_px')} | {w.get('latin_cap_height_px')} | {w.get('height_ratio')} |")
    return "\n".join(lines) + "\n"


# --------------------------------------------------------------------------- synthetic capture

def synth(out: Path, scale: int, font_px: float) -> None:
    """A capture drawn by construction: every cell on an integer grid, sRGB.

    Not a terminal — the analyser's own control. Swatches must come back at
    ΔE≈0, the grid at exactly the cell it was drawn with, and no alignment
    shift, or the analyser (not a terminal) is wrong.
    """
    from PIL import Image, ImageDraw, ImageFont

    palette = palette_zerocode(PREFS)
    font = ImageFont.truetype("/System/Library/Fonts/SFNSMono.ttf", int(round(font_px * scale)))
    cell_w = round(font.getlength("W"))
    cell_h = round(font_px * 1.15 * scale)
    margin = 20
    image = Image.new("RGB", (margin * 2 + cell_w * layout.COLS, margin * 2 + cell_h * layout.ROWS), tuple(palette["bg"]["rgb"]))
    draw = ImageDraw.Draw(image)
    screen = layout.build()

    def colour(parts, fg_default, bg_default):
        fg, bg = fg_default, None
        bold = dim = inverse = False
        at = 0
        while at < len(parts):
            p = parts[at]
            if p in (38, 48) and parts[at + 1] == 5:
                value = parts[at + 2]
                rgb = tuple(layout.xterm256(value)) if value >= 16 else tuple(palette["ansi"][value]["rgb"])
                fg, bg = (rgb, bg) if p == 38 else (fg, rgb)
                at += 3
                continue
            if p in (38, 48) and parts[at + 1] == 2:
                rgb = tuple(parts[at + 2:at + 5])
                fg, bg = (rgb, bg) if p == 38 else (fg, rgb)
                at += 5
                continue
            if 30 <= p <= 37:
                fg = tuple(palette["ansi"][p - 30]["rgb"])
            elif 90 <= p <= 97:
                fg = tuple(palette["ansi"][p - 90 + 8]["rgb"])
            elif 40 <= p <= 47:
                bg = tuple(palette["ansi"][p - 40]["rgb"])
            elif 100 <= p <= 107:
                bg = tuple(palette["ansi"][p - 100 + 8]["rgb"])
            elif p == 1:
                bold = True
            elif p == 2:
                dim = True
            elif p == 7:
                inverse = True
            at += 1
        if inverse:
            fg, bg = (bg or bg_default), fg
        if dim:
            ground = bg or bg_default
            fg = tuple(round(g + (f - g) * 0.5) for f, g in zip(fg, ground))
        return fg, bg

    for row, segments in enumerate(screen.rows):
        for col, text, sgr in segments:
            fg, bg = colour(list(sgr), tuple(palette["fg"]["rgb"]), tuple(palette["bg"]["rgb"]))
            x = margin + col * cell_w
            y = margin + row * cell_h
            for ch in text:
                width = layout.cell_width(ch)
                if bg is not None:
                    draw.rectangle([x, y, x + cell_w * width - 1, y + cell_h - 1], fill=bg)
                if ch == "█":
                    draw.rectangle([x, y, x + cell_w - 1, y + cell_h - 1], fill=fg)
                elif ch != " ":
                    draw.text((x, y), ch, font=font, fill=fg)
                x += cell_w * width
    buffer = io.BytesIO()
    srgb = SRGB.read_bytes()
    image.save(buffer, format="PNG", icc_profile=srgb)
    out.write_bytes(buffer.getvalue())


# --------------------------------------------------------------------------- cli

def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("fixture")
    zc = sub.add_parser("palette-zerocode")
    zc.add_argument("--prefs", type=Path, default=PREFS)
    ta = sub.add_parser("palette-terminal-app")
    ta.add_argument("--profile", required=True)
    ta.add_argument("--plist", type=Path, help="a `defaults export com.apple.Terminal` file; read live when absent")
    an = sub.add_parser("analyze")
    an.add_argument("manifest", type=Path)
    an.add_argument("--out", type=Path, required=True)
    sy = sub.add_parser("synth")
    sy.add_argument("--out", type=Path, required=True)
    sy.add_argument("--scale", type=int, default=2)
    sy.add_argument("--font-px", type=float, default=15)
    args = parser.parse_args(argv)

    if args.command == "fixture":
        sys.stdout.buffer.write(layout.fixture_bytes())
        sys.stdout.buffer.flush()
    elif args.command == "palette-zerocode":
        print(json.dumps(palette_zerocode(args.prefs), indent=1))
    elif args.command == "palette-terminal-app":
        if args.plist:
            blob = args.plist.read_bytes()
        else:
            import subprocess
            blob = subprocess.run(["defaults", "export", "com.apple.Terminal", "-"], check=True, capture_output=True).stdout
        print(json.dumps(palette_terminal_app(args.profile, blob), indent=1))
    elif args.command == "analyze":
        report = analyse(args.manifest, args.out)
        print((args.out / "report.md").read_text())
        del report
    elif args.command == "synth":
        synth(args.out, args.scale, args.font_px)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
