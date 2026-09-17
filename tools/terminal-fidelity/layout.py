"""The fixture both terminals draw, as ONE table.

The bytes a terminal is shown (`fixture_bytes`) and the probes the analyser
reads (`probes`) come out of the same walk over `ROW_PLAN`, so a row cannot be
moved in one and not the other. Everything is placed with absolute cursor
moves — no newline is ever printed — so neither terminal scrolls and row *n*
of the fixture is screen row *n* in both.

96 × 22, because that is what both screens give without a person rearranging
either: Terminal.app's profile opens at 120 × 30, and the pane the window
splits off a worker's tab at the person's 15 px measured 96 × 24
(output/terminal-fidelity/captures/env-zerocode-15.txt, 2026-09-17).

Standard library only: this module is imported by the fixture writer, which
runs inside Terminal.app's own shell as well as ours.
"""

from __future__ import annotations

import math
import unicodedata

COLS = 96
ROWS = 22
# Columns 0 and COLS-1 carry the row markers and the ones beside them stay
# blank, so content never touches a marker and a marker's edge is always a
# background edge.
LEFT = 2
RIGHT = COLS - 3

ESC = "\x1b"

# Calibration colours are xterm cube corners: every 256-colour palette defines
# them as 0/255 components, so they need no truecolor support and no theme can
# rebind them. Two pairs, so a row boundary and a column boundary are never an
# edge between two cells of the same colour.
CAL_COL = (201, 46)  # magenta, green — first and last rows, alternating by column
CAL_ROW = (21, 226)  # blue, yellow — first and last columns, alternating by row

# Claude Code 2.1.274's dark theme, as the binary spells it
# (output/terminal-fidelity/evidence/claude-code-apple-terminal-256.txt,
# `claude:"rgb(215,119,87)"` at offset 173632421). The colours a person sees
# most in that TUI, in swatch order.
CLAUDE_CODE_DARK = (
    ("claude", (215, 119, 87)),
    ("text", (255, 255, 255)),
    ("inactive", (153, 153, 153)),
    ("subtle", (80, 80, 80)),
    ("promptBorder", (136, 136, 136)),
    ("success", (78, 186, 101)),
    ("error", (255, 107, 128)),
    ("warning", (255, 193, 7)),
    ("permission", (177, 185, 249)),
    ("planMode", (72, 150, 140)),
    ("ide", (71, 130, 200)),
    ("merged", (175, 135, 255)),
    ("userMessageBackground", (55, 55, 55)),
    ("bashMessageBackgroundColor", (65, 60, 65)),
    ("diffAdded", (34, 92, 43)),
    ("diffRemoved", (122, 41, 54)),
)
CC = dict(CLAUDE_CODE_DARK)

# Symbols Claude Code draws that SF Mono has no glyph for (checked against
# SFNSMono.ttf's cmap — output/terminal-fidelity/evidence/fallback-font-advances.txt),
# then ones it does have, as controls.
FALLBACK_SYMBOLS = "⏺✻❯⎿⠋✶⚠★✓✗→↓●▶…·"

STROKE_CHARS = "|lHm0a-=g"
STROKE_RUN = 6
GAP = 2
SWATCH = 2
BLOCK = "█"
GREY_STEPS = 16
VERTICAL_LINE_COL = LEFT + 72
SEAM_LAST_COL = LEFT + 67

ATTRIBUTE_GROUPS = (
    ("default", ()),
    ("default-bold", (1,)),
    ("default-dim", (2,)),
    ("white-on-black", (38, 5, 231, 48, 5, 16)),
    ("white-on-black-bold", (1, 38, 5, 231, 48, 5, 16)),
    ("white-on-black-dim", (2, 38, 5, 231, 48, 5, 16)),
    ("red", (31,)),
    ("red-bold", (1, 31)),
    ("red-dim", (2, 31)),
    ("claude", (38, 2, *CC["claude"])),
    ("claude-dim", (2, 38, 2, *CC["claude"])),
    ("inverse", (7,)),
)
WHITE_ON_BLACK = (38, 5, 231, 48, 5, 16)


def xterm256(index: int) -> tuple[int, int, int] | None:
    """xterm's definition of indexed colours 16–255; 0–15 belong to the theme."""
    if index < 16:
        return None
    if index < 232:
        cube = (0, 95, 135, 175, 215, 255)
        index -= 16
        return (cube[index // 36], cube[(index // 6) % 6], cube[index % 6])
    grey = 8 + 10 * (index - 232)
    return (grey, grey, grey)


def chalk_ansi256(red: int, green: int, blue: int) -> int:
    """ansi-styles `rgbToAnsi256` — what chalk at level 2 sends for `rgb()`.

    `Math.round` rounds halves up, which Python's `round` does not.
    """

    def js_round(value: float) -> int:
        return math.floor(value + 0.5)

    if red == green == blue:
        if red < 8:
            return 16
        if red > 248:
            return 231
        return js_round(((red - 8) / 247) * 24) + 232
    return (
        16
        + 36 * js_round(red / 255 * 5)
        + 6 * js_round(green / 255 * 5)
        + js_round(blue / 255 * 5)
    )


def cell_width(ch: str) -> int:
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


def text_width(text: str) -> int:
    return sum(cell_width(ch) for ch in text)


def ansi_fg(index: int) -> tuple[int, ...]:
    return (30 + index,) if index < 8 else (90 + index - 8,)


def ansi_bg(index: int) -> tuple[int, ...]:
    return (40 + index,) if index < 8 else (100 + index - 8,)


class Screen:
    """Segments placed on rows, and what each one is there to measure."""

    def __init__(self) -> None:
        self.rows: list[list[tuple[int, str, tuple[int, ...]]]] = [[] for _ in range(ROWS)]
        self.probes: list[dict] = []

    def put(self, row: int, col: int, text: str, *sgr: int) -> int:
        width = text_width(text)
        if col < 0 or col + width > COLS:
            raise ValueError(f"row {row}: {text!r} at {col} leaves the {COLS}-column grid")
        for at, other, _ in self.rows[row]:
            if col < at + text_width(other) and at < col + width:
                raise ValueError(f"row {row}: {text!r} at {col} overlaps {other!r} at {at}")
        self.rows[row].append((col, text, tuple(sgr)))
        return col + width

    def probe(self, kind: str, **fields) -> None:
        self.probes.append({"kind": kind, **fields})

    def to_bytes(self) -> bytes:
        out = [f"{ESC}[0m{ESC}[?25l{ESC}[2J"]
        for row, segments in enumerate(self.rows):
            for col, text, sgr in sorted(segments):
                params = ";".join(str(part) for part in (0, *sgr))
                out.append(f"{ESC}[{row + 1};{col + 1}H{ESC}[{params}m{text}")
        # Park the (hidden) cursor on the last calibration row's first cell.
        out.append(f"{ESC}[0m{ESC}[{ROWS};1H")
        return "".join(out).encode("utf-8")


def _calibration(screen: Screen, row: int) -> int:
    for col in range(COLS):
        screen.put(row, col, " ", 48, 5, CAL_COL[col % 2])
    return row + 1


def _row_markers(screen: Screen) -> None:
    for row in range(1, ROWS - 1):
        for col in (0, COLS - 1):
            screen.put(row, col, " ", 48, 5, CAL_ROW[row % 2])


def _swatch_run(screen, row, col, group, entries, layer, width):
    """Place `entries` = [(sgr, expect, extra)] side by side, one probe each."""
    fill = " " if layer == "bg" else BLOCK
    for sgr, expect, extra in entries:
        screen.put(row, col, fill * width, *sgr)
        screen.probe("swatch", group=group, layer=layer, row=row, col=col, width=width, expect=expect, **extra)
        col += width
    return col


def _colours(screen: Screen, row: int) -> int:
    col = _swatch_run(screen, row, LEFT, "ansi", [(ansi_bg(i), {"ansi": i}, {}) for i in range(16)], "bg", SWATCH)
    col = _swatch_run(screen, row, col + GAP, "ansi", [(ansi_fg(i), {"ansi": i}, {}) for i in range(16)], "fg", SWATCH)
    greys = [round(step * 255 / (GREY_STEPS - 1)) for step in range(GREY_STEPS)]
    _swatch_run(screen, row, col + GAP, "grey", [((48, 2, v, v, v), {"rgb": (v, v, v)}, {"name": f"grey-{v}"}) for v in greys], "bg", 1)
    row += 1
    indices = list(range(16, 256))
    per_row = RIGHT - LEFT + 1
    truecolor_bg = [((48, 2, *rgb), {"rgb": rgb}, {"name": name}) for name, rgb in CLAUDE_CODE_DARK]
    while len(indices) * 1 + GAP + len(truecolor_bg) * SWATCH > per_row:
        take, indices = indices[:per_row], indices[per_row:]
        _swatch_run(screen, row, LEFT, "xterm", [((48, 5, i), {"xterm": i}, {}) for i in take], "bg", 1)
        row += 1
    col = _swatch_run(screen, row, LEFT, "xterm", [((48, 5, i), {"xterm": i}, {}) for i in indices], "bg", 1)
    _swatch_run(screen, row, col + GAP, "cc-truecolor", truecolor_bg, "bg", SWATCH)
    row += 1
    truecolor_fg = [((38, 2, *rgb), {"rgb": rgb}, {"name": name}) for name, rgb in CLAUDE_CODE_DARK]
    col = _swatch_run(screen, row, LEFT, "cc-truecolor", truecolor_fg, "fg", SWATCH)
    reduced = [(name, rgb, chalk_ansi256(*rgb)) for name, rgb in CLAUDE_CODE_DARK]
    _swatch_run(screen, row, col + GAP, "cc-ansi256", [((48, 5, i), {"xterm": i}, {"name": n, "intended": rgb}) for n, rgb, i in reduced], "bg", SWATCH)
    row += 1
    return _swatch_run(screen, row, LEFT, "cc-ansi256", [((38, 5, i), {"xterm": i}, {"name": n, "intended": rgb}) for n, rgb, i in reduced], "fg", SWATCH), row


def _attributes(screen: Screen, placed) -> int:
    col, row = placed
    col += GAP
    for name, sgr in ATTRIBUTE_GROUPS:
        screen.put(row, col, BLOCK * SWATCH, *sgr)
        screen.probe("attribute", row=row, col=col, width=SWATCH, name=name, sgr=list(sgr))
        col += SWATCH + 1
    return row + 1


def _strokes(screen: Screen, row: int) -> int:
    for ink, sgr in (("default", ()), ("white-on-black", WHITE_ON_BLACK), ("white-on-black-bold", (1, *WHITE_ON_BLACK))):
        col = LEFT
        groups = []
        for ch in STROKE_CHARS:
            screen.put(row, col, ch * STROKE_RUN, *sgr)
            groups.append({"char": ch, "col": col, "count": STROKE_RUN})
            col += STROKE_RUN
            # The gaps wear the run's SGR, so an explicit ground is one ground.
            screen.put(row, col, " " * GAP, *sgr)
            if ch == STROKE_CHARS[-1] and ink in ("default", "white-on-black"):
                screen.probe("ground", name="default" if ink == "default" else "black", row=row, col=col, width=GAP)
            col += GAP
        screen.probe("strokes", row=row, ink=ink, groups=groups)
        row += 1
    screen.put(row, LEFT, "The quick brown fox jumps over the lazy dog. 0123456789 {}[]()<>=+-*/\\|~")
    return row + 1


def _alignment(screen: Screen, row: int) -> int:
    pieces = ("|", "가나다라마바", "|", "HHH", "|", "한글 English 섞인 줄", "|")
    col = LEFT
    bars = []
    hangul = latin = None
    for piece in pieces:
        if piece == "|":
            bars.append(col)
        if piece == "가나다라마바":
            hangul = (col, text_width(piece))
        if piece == "HHH":
            latin = (col, text_width(piece))
        col = screen.put(row, col, piece)
    col = screen.put(row, col + GAP, "✻", 38, 2, *CC["claude"])
    screen.put(row, col, " Welcome to Claude Code!")
    col = LEFT
    for piece in pieces:
        col = screen.put(row + 1, col, "".join("WW" if cell_width(ch) == 2 else ch for ch in piece))
    col = screen.put(row + 1, col + GAP, "⏺", 38, 2, *CC["success"])
    screen.put(row + 1, col, " Bash(git status)")
    screen.probe("align", row=row, reference_row=row + 1, bars=bars, label="hangul")
    screen.probe("wide-size", row=row, hangul=hangul, latin=latin)
    row += 2

    col = LEFT
    bars = []
    for ch in FALLBACK_SYMBOLS:
        col = screen.put(row, col, ch)
        bars.append({"col": col, "after": ch})
        col = screen.put(row, col, "|")
    screen.put(row, col + GAP, "  ⎿  On branch main · nothing to commit", 38, 2, *CC["inactive"])
    col = LEFT
    for _ in FALLBACK_SYMBOLS:
        col = screen.put(row + 1, col, "W|")
    col = screen.put(row + 1, col + GAP, "✶", 38, 2, *CC["claude"])
    screen.put(row + 1, col, " Thinking… (esc to interrupt · 12s · ↓ 1.2k tokens)", 38, 2, *CC["subtle"])
    screen.probe("align", row=row, reference_row=row + 1, bars=[bar["col"] for bar in bars], symbols=bars, label="fallback")
    return row + 2


def _boxes(screen: Screen, row: int) -> int:
    border = (38, 2, *CC["promptBorder"])
    inner = 28
    screen.put(row, LEFT, "╭" + "─" * inner + "╮", *border)
    screen.put(row + 1, LEFT, "│", *border)
    screen.put(row + 1, LEFT + 1, ' > Try "write a test"'.ljust(inner))
    screen.put(row + 1, LEFT + 1 + inner, "│", *border)
    screen.put(row + 2, LEFT, "│" + " " * inner + "│", *border)
    screen.put(row + 3, LEFT, "╰" + "─" * inner + "╯", *border)
    screen.probe("hline", row=row, cols=(LEFT + 1, LEFT + inner), ink={"rgb": CC["promptBorder"]}, name="rounded-top")
    screen.probe("hline", row=row + 3, cols=(LEFT + 1, LEFT + inner), ink={"rgb": CC["promptBorder"]}, name="rounded-bottom")

    white = (38, 5, 231)
    table = LEFT + 32
    for offset, text in enumerate(("┌────┬────┐", "│ ab │ cd │", "├────┼────┤", "└────┴────┘")):
        screen.put(row + offset, table, text, *white)
    screen.probe("hline", row=row, cols=(table, table + 10), ink={"xterm": 231}, name="table-top")
    screen.probe("vline", col=table, rows=(row, row + 3), ink={"xterm": 231}, name="table-left")

    heavy = LEFT + 45
    for offset, text in enumerate(("┏━━┓ ╔══╗", "┃  ┃ ║  ║", "┃  ┃ ║  ║", "┗━━┛ ╚══╝")):
        screen.put(row + offset, heavy, text, *white)
    screen.put(row + 1, LEFT + 56, "▀▄█▌▐░▒▓ ⣿⣿⣿", *white)

    ground = (48, 2, *CC["userMessageBackground"])
    words = "> hello there, gyp"
    seam_top = row + 4
    col = screen.put(seam_top, LEFT, words, 38, 2, *CC["text"], *ground)
    screen.put(seam_top, col, " " * (SEAM_LAST_COL + 1 - col), *ground)
    screen.put(seam_top + 1, LEFT, " " * (SEAM_LAST_COL + 1 - LEFT), *ground)
    screen.probe("seam", rows=(seam_top, seam_top + 1), cols=(LEFT + text_width(words) + GAP, SEAM_LAST_COL), ground={"rgb": CC["userMessageBackground"]})

    for line in range(row, seam_top + 2):
        screen.put(line, VERTICAL_LINE_COL, "│", *white)
    screen.probe("vline", col=VERTICAL_LINE_COL, rows=(row, seam_top + 1), ink={"xterm": 231}, name="long-vertical")
    return seam_top + 2


ROW_PLAN = (_colours, _attributes, _strokes, _alignment, _boxes)


def build() -> Screen:
    screen = Screen()
    row = _calibration(screen, 0)
    _row_markers(screen)
    for step in ROW_PLAN:
        row = step(screen, row)
    if row != ROWS - 1:
        raise ValueError(f"the plan fills {row} rows; the last calibration row is {ROWS - 1}")
    _calibration(screen, ROWS - 1)
    return screen


def fixture_bytes() -> bytes:
    return build().to_bytes()


def probes() -> list[dict]:
    return build().probes
