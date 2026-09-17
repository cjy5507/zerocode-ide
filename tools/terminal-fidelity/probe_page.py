#!/usr/bin/env python3
"""The fixture as the window's terminal DOM, for the WKWebView probe.

  probe_page.py --variant NAME --out PAGE.html [--prefs P]

The page links the checkout's own `ui/tokens.css` and `ui/shell.css`, so the
cascade a probe glyph sits in is the window's — including the body's global
`-webkit-font-smoothing` — and builds the same element chain a terminal tab
builds (`termView` in ui/shell-term.js: section.terminal.terminal--stage >
pre.term > div.term-row > span). Runs are dressed with the renderer's own
class names (`finiteColorClass`): indexed colours by `term-fg-N`/`term-bg-N`,
the defaults by `term-fg-default`/`term-bg-default`, 24-bit colour inline.

What it leaves out, on purpose: the contrast lift and the wide-glyph growth,
which need the live renderer's measurements. Colour and Hangul are read off
the real window; the probe is for how glyphs are rasterised.
"""

from __future__ import annotations

import argparse
import html
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import fidelity  # noqa: E402
import layout  # noqa: E402

UI = fidelity.REPO / "ui"

# One row per variant: the rule it adds to the terminal it draws. `inherit`
# adds nothing and is the calibration against a live-window capture — if it
# does not measure like the window, no other row means anything.
VARIANTS = {
    "inherit": "",
    "auto": ".term { -webkit-font-smoothing: auto; }",
    "subpixel": ".term { -webkit-font-smoothing: subpixel-antialiased; }",
    "inherit-w400": ":root { --term-weight: 400; --term-bold-weight: 700; }",
    "auto-w400": ":root { --term-weight: 400; --term-bold-weight: 700; } .term { -webkit-font-smoothing: auto; }",
    "subpixel-w400": ":root { --term-weight: 400; --term-bold-weight: 700; } .term { -webkit-font-smoothing: subpixel-antialiased; }",
    "auto-layer": ".term { -webkit-font-smoothing: auto; } .terminal { will-change: transform; }",
}


def runs_for_row(segments) -> list[tuple[str, list[str], list[str]]]:
    """(text, classes, inline styles) per segment, blanks filled with plain runs."""
    cells: list[tuple[str, tuple[int, ...]] | None] = [None] * layout.COLS
    for col, text, sgr in segments:
        at = col
        for ch in text:
            cells[at] = (ch, sgr)
            width = layout.cell_width(ch)
            for extra in range(1, width):
                cells[at + extra] = ("", sgr)
            at += width
    runs: list[tuple[str, list[str], list[str]]] = []
    for cell in cells:
        ch, sgr = cell if cell is not None else (" ", ())
        classes, styles = dress(sgr)
        if runs and runs[-1][1] == classes and runs[-1][2] == styles:
            runs[-1] = (runs[-1][0] + ch, classes, styles)
        else:
            runs.append((ch, classes, styles))
    return runs


def dress(sgr: tuple[int, ...]) -> tuple[list[str], list[str]]:
    fg = bg = None
    bold = dim = reverse = False
    parts = list(sgr)
    at = 0
    while at < len(parts):
        p = parts[at]
        if p in (38, 48) and parts[at + 1] == 5:
            value = ("indexed", parts[at + 2])
            fg, bg = (value, bg) if p == 38 else (fg, value)
            at += 3
            continue
        if p in (38, 48) and parts[at + 1] == 2:
            value = ("rgb", tuple(parts[at + 2:at + 5]))
            fg, bg = (value, bg) if p == 38 else (fg, value)
            at += 5
            continue
        if 30 <= p <= 37:
            fg = ("indexed", p - 30)
        elif 90 <= p <= 97:
            fg = ("indexed", p - 90 + 8)
        elif 40 <= p <= 47:
            bg = ("indexed", p - 40)
        elif 100 <= p <= 107:
            bg = ("indexed", p - 100 + 8)
        elif p == 1:
            bold = True
        elif p == 2:
            dim = True
        elif p == 7:
            reverse = True
        at += 1
    classes: list[str] = []
    styles: list[str] = []
    if reverse:
        fg, bg = bg, fg
    # xterm's bold-is-bright over the lower half of the 16, as the renderer does.
    if bold and fg and fg[0] == "indexed" and fg[1] < 8:
        fg = ("indexed", fg[1] + 8)
    if bold:
        classes.append("bold")
    if dim:
        classes.append("dim")
    if fg is None:
        classes.append("term-fg-reverse-default" if reverse else "term-fg-default")
    elif fg[0] == "indexed":
        classes.append(f"term-fg-{fg[1]}")
    else:
        styles.append("color: rgb({} {} {})".format(*fg[1]))
    if bg is None:
        if reverse:
            classes.append("term-bg-default")
    elif bg[0] == "indexed":
        classes.append(f"term-bg-{bg[1]}")
    else:
        styles.append("background: rgb({} {} {})".format(*bg[1]))
    return classes, styles


def page(variant: str, prefs_path: Path) -> str:
    palette = fidelity.palette_zerocode(prefs_path)
    prefs = json.loads(prefs_path.read_text())["data"]["terminal_prefs"]
    root_vars = [
        f"--term-font-size: {prefs['font_size']}px",
        f"--term-leading: {prefs['leading']}",
        f"--term-weight: {prefs['weight']}",
        f"--term-padding-x: {prefs['padding_x']}px",
        f"--term-padding-y: {prefs['padding_y']}px",
        "--term-screen-bg: rgb({} {} {})".format(*palette["bg"]["rgb"]),
        "--term-screen-fg: rgb({} {} {})".format(*palette["fg"]["rgb"]),
    ]
    root_vars += ["--term-{}: rgb({} {} {})".format(i, *c["rgb"]) for i, c in enumerate(palette["ansi"])]
    screen = layout.build()
    rows = []
    for segments in screen.rows:
        spans = "".join(
            '<span class="{}"{}>{}</span>'.format(
                " ".join(classes),
                f' style="{"; ".join(styles)}"' if styles else "",
                html.escape(text),
            )
            for text, classes, styles in runs_for_row(segments)
        )
        rows.append(f'<div class="term-row">{spans}</div>')
    return f"""<!doctype html>
<html lang="en" data-theme="dark">
<head>
<meta charset="utf-8">
<link rel="stylesheet" href="{(UI / 'tokens.css').as_uri()}">
<link rel="stylesheet" href="{(UI / 'shell.css').as_uri()}">
<style>
:root {{ {"; ".join(root_vars)}; }}
/* The body keeps every property the window's body has — above all its font
   smoothing — and only stops being the app's grid, and stops its first-paint
   reveal, which in the window finished long before anyone reads a terminal. */
body {{ display: block; animation: none; }}
.probe {{ position: absolute; left: 0; top: 0; width: 100vw; height: 100vh; display: flex; }}
{VARIANTS[variant]}
</style>
</head>
<body>
<div class="probe" data-variant="{variant}">
<section class="terminal terminal--stage"><pre class="term">{"".join(rows)}</pre></section>
</div>
</body>
</html>
"""


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--variant", choices=sorted(VARIANTS), required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--prefs", type=Path, default=fidelity.PREFS)
    args = parser.parse_args(argv)
    args.out.write_text(page(args.variant, args.prefs))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
