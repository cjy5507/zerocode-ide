#!/usr/bin/env python3
"""Writes crates/zerocode-core/fixtures/game-state/spec_cases.json.

    python3 tools/game-state-probe/spec_cases.py

The verdicts and sample counts are worked out here, apart from the Rust and
Swift validators, so both are pinned to numbers neither of them produced. The
limits are read from limits.json, the core table's own wire.
"""
import copy
import json
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/zerocode-core/fixtures/game-state"

LIMITS = json.loads((OUT / "limits.json").read_text())

ROI = {"x": 24, "y": 189, "width": 352, "height": 504, "space": "pixel"}
RED = {"r": 255, "g": 0, "b": 0, "tolerance": 24}
GREEN = {"r": 0, "g": 255, "b": 0, "tolerance": 24}
BLUE = {"r": 0, "g": 0, "b": 255, "tolerance": 24}
YELLOW = {"r": 255, "g": 255, "b": 0, "tolerance": 24}
BLACK = {"r": 0, "g": 0, "b": 0, "tolerance": 24}
CELLS = {
    "kind": "cells",
    "rows": 4,
    "columns": 4,
    "tile_width_permille": 864,
    "tile_height_permille": 714,
    "inset_permille": 250,
    "lattice": 3,
    "min_share_permille": 800,
    "readout": {"op": "first", "class": 1},
}
BLOBS = {"kind": "blobs", "class": 1, "step": 4, "min_samples": 3, "gate": 24, "max_blobs": 4}
SPEC = {
    "space": "srgb",
    "classes": [RED, GREEN, BLUE, YELLOW],
    "ground": BLACK,
    "reference_width": 400,
    "reference_height": 900,
    "layout": CELLS,
    "confirm": 1,
}


def cells(**change):
    layout = dict(CELLS)
    layout.update(change)
    return layout


def blobs(**change):
    layout = dict(BLOBS)
    layout.update(change)
    return layout


def colour(r, g, b, tolerance=24):
    return {"r": r, "g": g, "b": b, "tolerance": tolerance}


# ---- an independent reading of the rules -------------------------------

def apart(a, b):
    reach = a["tolerance"] + b["tolerance"]
    return any(abs(a[k] - b[k]) > reach for k in "rgb")


def axis_ok(span, count, tile, inset, lattice, need_gaps):
    for pitch in {span // count, -(-span // count)}:
        t = pitch * tile // 1000
        sampled = t - 2 * (t * inset // 1000)
        if sampled < lattice:
            return "geometry"
    return None


def gaps_ok(span, count, tile):
    for pitch in {span // count, -(-span // count)}:
        spare = pitch - pitch * tile // 1000
        if spare // 2 == 0 or spare - spare // 2 == 0:
            return False
    return True


def judge(spec, roi, limits=LIMITS):
    """Returns (verdict, cost)."""
    for key in ("r", "g", "b", "tolerance"):
        for c in spec["classes"] + ([spec["ground"]] if "ground" in spec else []):
            if not isinstance(c.get(key), int) or not 0 <= c[key] <= 255:
                return "wire", None
    if roi["space"] != "pixel" or roi["x"] < 0 or roi["y"] < 0 or roi["width"] <= 0 or roi["height"] <= 0:
        return "geometry", None
    rw, rh = spec["reference_width"], spec["reference_height"]
    if rw == 0 or rh == 0 or roi["x"] + roi["width"] > rw or roi["y"] + roi["height"] > rh:
        return "geometry", None
    classes = spec["classes"]
    if not classes or len(classes) > limits["max_classes"]:
        return "budget", None
    palette = classes + ([spec["ground"]] if "ground" in spec else [])
    if any(c["tolerance"] > limits["max_tolerance"] for c in palette):
        return "budget", None
    for i, a in enumerate(palette):
        for b in palette[i + 1:]:
            if not apart(a, b):
                return "overlap", None
    if spec["confirm"] == 0 or spec["confirm"] > limits["max_confirm"]:
        return "budget", None
    anchors = spec.get("anchors", [])
    if len(anchors) > limits["max_anchors"]:
        return "budget", None
    if any(a["x"] >= rw or a["y"] >= rh for a in anchors):
        return "geometry", None
    if any(a["class"] > len(classes) or (a["class"] == 0 and "ground" not in spec) for a in anchors):
        return "reference", None
    layout = spec["layout"]
    w, h = roi["width"], roi["height"]
    if layout["kind"] == "cells":
        n = layout["rows"] * layout["columns"]
        if layout["rows"] == 0 or layout["columns"] == 0 or n > limits["max_cells"]:
            return "budget", None
        tw, th, inset = layout["tile_width_permille"], layout["tile_height_permille"], layout["inset_permille"]
        if not 1 <= tw <= 1000 or not 1 <= th <= 1000 or inset > 499:
            return "geometry", None
        lat = layout["lattice"]
        if lat == 0 or lat > limits["max_lattice"] or lat * lat < limits["min_cell_samples"]:
            return "budget", None
        if not 501 <= layout["min_share_permille"] <= 1000:
            return "threshold", None
        readout = layout["readout"]
        if readout["op"] == "cell":
            if not 1 <= readout["index"] <= n:
                return "reference", None
        elif not 1 <= readout["class"] <= len(classes):
            return "reference", None
        if axis_ok(w, layout["columns"], tw, inset, lat, False) or axis_ok(h, layout["rows"], th, inset, lat, False):
            return "geometry", None
        sides = 0
        if "ground" in spec:
            if tw >= 1000 and th >= 1000:
                return "unsupported", None
            if (tw < 1000 and not gaps_ok(w, layout["columns"], tw)) or (th < 1000 and not gaps_ok(h, layout["rows"], th)):
                return "geometry", None
            sides = 2 * ((tw < 1000) + (th < 1000))
        cost = n * (lat * lat + sides * lat)
    else:
        if not 1 <= layout["class"] <= len(classes):
            return "reference", None
        if layout["step"] == 0 or layout["step"] > min(w, h):
            return "geometry", None
        if layout["min_samples"] == 0:
            return "threshold", None
        if not 1 <= layout["gate"] <= limits["max_gate"] or not 1 <= layout["max_blobs"] <= limits["max_blobs"]:
            return "budget", None
        cost = -(-w // layout["step"]) * -(-h // layout["step"])
    cost += len(anchors)
    if cost > limits["max_detector_samples"]:
        return "budget", None
    return "ok", cost


def frame_roi(roi, reference, frame):
    rw, rh = reference["width"], reference["height"]
    fw, fh = frame["width"], frame["height"]
    x, y, w, h = roi["x"], roi["y"], roi["width"], roi["height"]
    if roi["space"] != "pixel" or x < 0 or y < 0 or w <= 0 or h <= 0:
        return None
    if min(rw, rh, fw, fh) == 0 or x + w > rw or y + h > rh:
        return None
    if abs(fw * rh - fh * rw) > max(rw, rh):
        return None
    left, right = x * fw // rw, (x + w) * fw // rw
    top, bottom = y * fh // rh, (y + h) * fh // rh
    if right <= left or bottom <= top:
        return None
    g = math.gcd(fw, rw)
    return {
        "roi": {"x": left, "y": top, "width": right - left, "height": bottom - top, "space": "pixel"},
        "scale": {"numerator": fw // g, "denominator": rw // g},
    }


# ---- cases ---------------------------------------------------------------

REMOVE = None  # a patch value of null removes the key from the base spec

NINE = [colour(r, g, b, 8) for r, g, b in [
    (128, 0, 0), (0, 128, 0), (0, 0, 128), (128, 128, 0), (128, 0, 128),
    (0, 128, 128), (255, 0, 0), (0, 255, 0), (0, 0, 255)]]

cases = [
    ("valid_cells_first", {}, None),
    ("valid_cells_without_ground", {"ground": REMOVE}, None),
    ("valid_single_cell", {"ground": REMOVE, "layout": cells(rows=1, columns=1, tile_width_permille=1000,
                                                              tile_height_permille=1000, inset_permille=100,
                                                              lattice=4, min_share_permille=900,
                                                              readout={"op": "cell", "index": 1})}, None),
    ("valid_count_readout", {"layout": cells(readout={"op": "count", "class": 4})}, None),
    ("valid_last_cell_readout", {"layout": cells(readout={"op": "cell", "index": 16})}, None),
    ("valid_gaps_on_one_axis", {"layout": cells(tile_width_permille=1000)}, None),
    ("valid_apart_by_one", {"classes": [RED, colour(206, 0, 0)]}, None),
    ("valid_blobs", {"layout": BLOBS}, None),
    ("valid_blobs_every_pixel", {"layout": blobs(step=1)}, None),
    ("overlap_touching", {"classes": [RED, colour(207, 0, 0)]}, None),
    ("overlap_ground", {"ground": colour(230, 0, 0)}, None),
    ("no_classes", {"classes": []}, None),
    ("too_many_classes", {"classes": NINE}, None),
    ("tolerance_over_limit", {"classes": [colour(255, 0, 0, 65), GREEN, BLUE, YELLOW]}, None),
    ("ground_tolerance_over_limit", {"ground": colour(0, 0, 0, 65)}, None),
    ("confirm_zero", {"confirm": 0}, None),
    ("confirm_over_limit", {"confirm": 9}, None),
    ("rows_zero", {"layout": cells(rows=0)}, None),
    ("too_many_cells", {"layout": cells(rows=16, columns=17)}, None),
    ("tile_width_zero", {"layout": cells(tile_width_permille=0)}, None),
    ("tile_height_over", {"layout": cells(tile_height_permille=1001)}, None),
    ("inset_half", {"layout": cells(inset_permille=500)}, None),
    ("lattice_zero", {"layout": cells(lattice=0)}, None),
    ("lattice_over_limit", {"layout": cells(lattice=9)}, None),
    ("lattice_under_cell_samples", {"layout": cells(lattice=1)}, None),
    ("min_share_half", {"layout": cells(min_share_permille=500)}, None),
    ("min_share_over", {"layout": cells(min_share_permille=1001)}, None),
    ("readout_cell_zero", {"layout": cells(readout={"op": "cell", "index": 0})}, None),
    ("readout_cell_past_end", {"layout": cells(readout={"op": "cell", "index": 17})}, None),
    ("readout_first_class_zero", {"layout": cells(readout={"op": "first", "class": 0})}, None),
    ("readout_count_class_past", {"layout": cells(readout={"op": "count", "class": 5})}, None),
    ("cells_smaller_than_lattice", {"layout": cells(lattice=8, inset_permille=499)}, None),
    ("gaps_round_away", {"layout": cells(tile_width_permille=995)}, None),
    ("ground_without_gaps", {"layout": cells(tile_width_permille=1000, tile_height_permille=1000)}, None),
    ("blobs_class_zero", {"layout": blobs(**{"class": 0})}, None),
    ("blobs_class_past", {"layout": blobs(**{"class": 5})}, None),
    ("blobs_step_zero", {"layout": blobs(step=0)}, None),
    ("blobs_step_over_roi", {"layout": blobs(step=353)}, None),
    ("blobs_min_samples_zero", {"layout": blobs(min_samples=0)}, None),
    ("blobs_gate_zero", {"layout": blobs(gate=0)}, None),
    ("blobs_gate_over", {"layout": blobs(gate=257)}, None),
    ("blobs_max_blobs_zero", {"layout": blobs(max_blobs=0)}, None),
    ("blobs_max_blobs_over", {"layout": blobs(max_blobs=17)}, None),
    ("blobs_cost_over", {"layout": blobs(step=1)}, {"x": 0, "y": 0, "width": 400, "height": 900, "space": "pixel"}),
    ("valid_with_anchors", {"anchors": [{"x": 5, "y": 5, "class": 0}, {"x": 30, "y": 200, "class": 1}]}, None),
    ("anchors_over_limit", {"anchors": [{"x": i, "y": 5, "class": 0} for i in range(9)]}, None),
    ("anchor_outside_reference", {"anchors": [{"x": 400, "y": 5, "class": 0}]}, None),
    ("anchor_class_past", {"anchors": [{"x": 5, "y": 5, "class": 5}]}, None),
    ("anchor_ground_without_ground", {"ground": REMOVE, "anchors": [{"x": 5, "y": 5, "class": 0}]}, None),
    ("reference_zero", {"reference_width": 0}, None),
    ("roi_outside_reference", {}, dict(ROI, x=100)),
    ("roi_point_space", {}, dict(ROI, space="point")),
    ("roi_negative", {}, dict(ROI, x=-1)),
    ("roi_zero_width", {}, dict(ROI, width=0)),
]

# Shapes neither decoder may accept: the verdict is "wire".
wire = [
    ("wire_display_p3", {"space": "display_p3"}),
    ("wire_unknown_key", {"hue": 1}),
    ("wire_class_unknown_key", {"classes": [dict(RED, name="red"), GREEN, BLUE, YELLOW]}),
    ("wire_channel_over_byte", {"classes": [colour(256, 0, 0), GREEN, BLUE, YELLOW]}),
    ("wire_channel_negative", {"classes": [colour(-1, 0, 0), GREEN, BLUE, YELLOW]}),
    ("wire_cells_with_blob_field", {"layout": cells(step=4)}),
    ("wire_unknown_layout", {"layout": {"kind": "ring", "rows": 1}}),
    ("wire_unknown_readout", {"layout": cells(readout={"op": "last", "class": 1})}),
    ("wire_readout_extra_key", {"layout": cells(readout={"op": "cell", "index": 1, "class": 1})}),
    ("wire_missing_confirm", {"confirm": REMOVE}),
    ("wire_fractional_confirm", {"confirm": 1.5}),
    ("wire_ground_null_class", {"ground": {"r": 0, "g": 0, "b": 0}}),
    ("wire_empty_anchors", {"anchors": []}),
    ("wire_anchor_unknown_key", {"anchors": [{"x": 5, "y": 5, "class": 0, "hue": 1}]}),
]


def patched(patch):
    spec = copy.deepcopy(SPEC)
    for key, value in patch.items():
        if value is REMOVE:
            spec.pop(key, None)
        else:
            spec[key] = value
    return spec


rows = []
for name, patch, roi in cases:
    spec = patched(patch)
    verdict, cost = judge(spec, roi or ROI)
    row = {"name": name, "patch": patch, "expected": verdict}
    if roi is not None:
        row["roi"] = roi
    if cost is not None:
        row["cost"] = cost
    rows.append(row)
for name, patch in wire:
    rows.append({"name": name, "patch": patch, "expected": "wire"})

names = [r["name"] for r in rows]
assert len(names) == len(set(names)), "duplicate case names"
counts = {}
for r in rows:
    counts[r["expected"]] = counts.get(r["expected"], 0) + 1

frame_rows = []
for name, roi, reference, frame in [
    ("same_extent", ROI, {"width": 400, "height": 900}, {"width": 400, "height": 900}),
    ("uniform_double", ROI, {"width": 400, "height": 900}, {"width": 800, "height": 1800}),
    ("ladder_rounding", {"x": 100, "y": 50, "width": 300, "height": 200, "space": "pixel"},
     {"width": 1512, "height": 982}, {"width": 1280, "height": 831}),
    ("aspect_changed", ROI, {"width": 400, "height": 900}, {"width": 400, "height": 800}),
    ("turned_extent", ROI, {"width": 400, "height": 900}, {"width": 900, "height": 400}),
    ("outside_reference", dict(ROI, x=100), {"width": 400, "height": 900}, {"width": 400, "height": 900}),
    ("tiny_frame", ROI, {"width": 400, "height": 900}, {"width": 4, "height": 9}),
    ("collapsed_roi", {"x": 10, "y": 10, "width": 1, "height": 1, "space": "pixel"},
     {"width": 400, "height": 900}, {"width": 4, "height": 9}),
    ("empty_frame", ROI, {"width": 400, "height": 900}, {"width": 0, "height": 0}),
]:
    frame_rows.append({"name": name, "roi": roi, "reference": reference, "frame": frame,
                       "expected": frame_roi(roi, reference, frame)})

fixture = {"roi": ROI, "spec": SPEC, "cases": rows, "frame_roi": frame_rows}
OUT.mkdir(parents=True, exist_ok=True)
(OUT / "spec_cases.json").write_text(json.dumps(fixture, indent=1, sort_keys=True) + "\n")
print(json.dumps({"cases": len(rows), "verdicts": counts, "frame_roi": len(frame_rows)}, sort_keys=True))
for r in rows:
    print(r["name"], r["expected"], r.get("cost"))
for r in frame_rows:
    print(r["name"], json.dumps(r["expected"], sort_keys=True))
