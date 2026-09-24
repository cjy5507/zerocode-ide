#!/usr/bin/env python3
"""Writes the owned game-state scenes: what to draw, what is true, how it is split.

`python3 tools/game-state-probe/scenes.py` rewrites
`crates/zerocode-core/fixtures/game-state/scenes.json`. The probe draws each
scene into a PNG beside it (`probe.swift --render`); the Swift tests and the
probe read the PNGs, run the real kernel on the pixels alone, and only then
look at `truth`. The kernel never sees a label.

The spec was written from the tune scenes and fixed before any held-out scene
was drawn. Held-out scenes come from other seeds and add what the tune scenes
never showed: other boards, other offsets and sizes, twice and three quarters
the size, and the negatives — cover, other themes, turns, stretches and frame
metadata the spec does not read.
"""
import json
import random
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/zerocode-core/fixtures/game-state/scenes.json"

WIDTH, HEIGHT = 360, 640
RED, GREEN, BLUE, YELLOW, HUD = 1, 2, 3, 4, 5
PALETTE = {
    RED: [230, 40, 40],
    GREEN: [40, 200, 70],
    BLUE: [40, 90, 230],
    YELLOW: [240, 200, 40],
    HUD: [96, 64, 168],
}
GROUND = [18, 18, 24]
TOLERANCE = 28
SPEC = {
    "space": "srgb",
    "classes": [dict(zip("rgb", PALETTE[k]), tolerance=TOLERANCE) for k in (RED, GREEN, BLUE, YELLOW, HUD)],
    "ground": dict(zip("rgb", GROUND), tolerance=12),
    "reference_width": WIDTH,
    "reference_height": HEIGHT,
    "layout": {
        "kind": "cells",
        "rows": 4,
        "columns": 4,
        "tile_width_permille": 800,
        "tile_height_permille": 714,
        "inset_permille": 250,
        "lattice": 3,
        # A digit sits in every tile: tune-05 showed it can take 2 of the 9 samples.
        "min_share_permille": 750,
        "readout": {"op": "first", "class": RED},
    },
    "confirm": 1,
    # The HUD bar is on top only while the frame is upright.
    "anchors": [{"x": 180, "y": 30, "class": HUD}, {"x": 180, "y": 620, "class": 0}],
}
ROI = {"x": 22, "y": 134, "width": 316, "height": 358, "space": "pixel"}
UPRIGHT = {"orientation": "up", "color_space": "srgb"}


def board(rng):
    return [rng.choice((RED, GREEN, BLUE, YELLOW)) for _ in range(16)]


def first(cells):
    return next((at + 1 for at, cell in enumerate(cells) if cell == RED), 0)


def draw(rng, cells, offset, size, **extra):
    """A board moved by up to `offset` pixels and sized within `size` of its own, as one
    renderer's frames vary."""
    return dict(
        cells=cells,
        offset=[round(rng.uniform(-offset, offset), 2), round(rng.uniform(-offset, offset), 2)],
        size=round(rng.uniform(1 - size, 1 + size), 4),
        theme="standard",
        digits=True,
        pixels_per_point=1,
        turn=0,
        stretch=[1, 1],
        occluders=[],
        tiles={},
        **extra,
    )


def scene(id, group, split, cells, drawing, expect, frame=UPRIGHT):
    # A tile drawn in a colour of its own is no class of the palette.
    truth = [0 if str(at + 1) in drawing["tiles"] else cell for at, cell in enumerate(cells)]
    return dict(id=id, group=group, split=split, frame=frame, draw=drawing,
                truth=dict(cells=truth, first=first(truth)), expect=expect)


def main():
    scenes = []
    tune = random.Random(6768)
    for at in range(1, 7):
        cells = board(tune)
        scenes.append(scene(f"tune-{at:02}", f"tune-{at:02}", "tune", cells, draw(tune, cells, 1.0, 0.003), "read"))

    # The first held-out draw (seed 20260925) was scored against a spec that asked 800 permille;
    # when the tune scenes moved that to 750, the held-out scenes were drawn again from a seed
    # no spec had seen.
    held = random.Random(20260926)
    for at in range(1, 21):
        cells = board(held)
        scenes.append(scene(f"heldout-{at:02}", f"heldout-{at:02}", "heldout", cells, draw(held, cells, 1.5, 0.005), "read"))
    for name, ratio in (("twice-a", 2), ("twice-b", 2)):
        cells = board(held)
        drawing = draw(held, cells, 1.5, 0.005)
        drawing["pixels_per_point"] = ratio
        scenes.append(scene(f"heldout-{name}", f"heldout-{name}", "heldout", cells, drawing, "read"))
    cells = board(held)
    drawing = draw(held, cells, 1.5, 0.005)
    drawing["pixels_per_point"] = 0.75
    scenes.append(scene("heldout-three-quarters", "heldout-three-quarters", "heldout", cells, drawing, "read"))

    def negative(name, expect, frame=UPRIGHT, **change):
        cells = board(held)
        drawing = draw(held, cells, 1.5, 0.005)
        drawing.update(change)
        scenes.append(scene(f"heldout-{name}", f"heldout-{name}", "heldout", cells, drawing, expect, frame))

    # Cover: a light popup over the first two rows and their gaps, a hand, a
    # cursor on a tile, a popup in a palette colour over two tiles and their
    # gaps, and a banner over the HUD.
    negative("popup", "safe", occluders=[dict(shape="rect", x=40, y=150, w=280, h=170, rgb=[236, 236, 240])])
    negative("hand", "safe", occluders=[dict(shape="ellipse", x=120, y=250, w=150, h=110, rgb=[224, 172, 140])])
    negative("cursor", "safe", occluders=[dict(shape="arrow", x=205, y=268, w=22, h=34, rgb=[250, 250, 250])])
    negative("red-popup", "safe", occluders=[dict(shape="rect", x=30, y=230, w=160, h=100, rgb=PALETTE[RED])])
    negative("banner", "abstain", occluders=[dict(shape="rect", x=0, y=0, w=360, h=64, rgb=[48, 48, 52])])
    # Another piece the spec never named.
    negative("new-piece", "safe", tiles={"5": [170, 90, 200], "10": [170, 90, 200]})
    # The same game in other themes.
    negative("dark-theme", "abstain", theme="dark")
    negative("light-theme", "abstain", theme="light")
    # Turned, stretched, or said to be turned or wide-gamut.
    negative("turned-half", "abstain", turn=180)
    negative("turned-quarter", "abstain", turn=90)
    negative("stretched", "abstain", stretch=[1, 1.125])
    negative("said-turned", "abstain", frame={"orientation": "right", "color_space": "srgb"})
    negative("said-p3", "abstain", frame={"orientation": "up", "color_space": "display_p3"})

    fixture = dict(spec=SPEC, roi=ROI, palette={str(k): v for k, v in PALETTE.items()}, ground=GROUND,
                   reference=[WIDTH, HEIGHT], scenes=scenes)
    OUT.write_text(json.dumps(fixture, indent=1, sort_keys=True) + "\n")
    counts = {}
    for row in scenes:
        key = (row["split"], row["expect"])
        counts[key] = counts.get(key, 0) + 1
    print(json.dumps({f"{split}/{expect}": n for (split, expect), n in sorted(counts.items())}))


if __name__ == "__main__":
    main()
