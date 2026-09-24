# Game-state cases

A reflex plan's colour detector carries a colour spec. The window checks it
with `game_state::validate_color`; the helper checks it again with
`PerceptionSpecs.validate` before its kernel reads a pixel. Both read the
cases here and must answer the same verdict and, for a valid spec, the same
sample count. `limits.json` is the one table of perception limits, written
exactly as `game_state::limits_wire` sends it (plus a final newline).

## Spec cases

`spec_cases.json` holds one base ROI and spec. A case replaces top-level
fields of the spec (`patch`; a null removes the field) and may replace the
ROI. `expected` is `ok`, a `SpecError` code, or `wire` when neither decoder
may accept the shape (an unknown field, a value outside its type, a missing
field). A valid case names its `cost`: the samples one observation reads.

## The rules, in the order both validators apply them

1. The ROI is a pixel ROI with a non-negative origin and a positive size,
   inside the reference extent, which is not empty (`geometry`).
2. There are 1 to `max_classes` classes (`budget`); no class or ground
   accepts more than `max_tolerance` on a channel (`budget`); and no two of
   them, ground included, can hold the same sample — on some channel they lie
   further apart than their tolerances reach together (`overlap`).
3. `confirm` is 1 to `max_confirm` (`budget`). There are at most
   `max_anchors` anchors (`budget`), each inside the reference extent
   (`geometry`) and naming an existing class, or the ground (class 0) only
   when the spec has one (`reference`).
4. Cells: at least one row and column and at most `max_cells` cells
   (`budget`); tiles of 1 to 1000 permille of their pitch and an inset under
   500 permille (`geometry`); a lattice of 1 to `max_lattice` giving at least
   `min_cell_samples` samples (`budget`); a strict-majority share, 501 to
   1000 permille (`threshold`); a readout naming an existing cell or class
   (`reference`); every cell's tile less its inset at least `lattice` pixels
   on each axis (`geometry`). With ground, some axis must leave a gap between
   tiles (`unsupported`) and every gap must be at least a pixel on both sides
   (`geometry`).
5. Blobs: an existing class (`reference`); a step of 1 to the ROI's shorter
   side (`geometry`); at least one sample per blob (`threshold`); a gate of 1
   to `max_gate` and 1 to `max_blobs` blobs (`budget`).
6. The cost — the layout's samples plus one per anchor — is at most
   `max_detector_samples` (`budget`).

An optional field is left out or written in full: `ground` is never `null`
and `anchors` is never an empty list, so both decoders read the same set of
wires (`wire`).

Pitch boxes split the ROI's span at `floor(i × span / count)`, so every pitch
along an axis is the floor or the ceiling of `span / count`; checking both
checks every cell. A tile is `floor(pitch × tile / 1000)` wide, centred with
the spare split `floor(spare / 2)` before and the rest after; its inset is
`floor(tile × inset / 1000)` on each side. A cells observation reads
`lattice²` samples per cell, plus `lattice` gap samples on each side that has
a gap when the spec has ground. A blobs observation reads
`ceil(width / step) × ceil(height / step)` samples. An anchor at `(x, y)` is
read at `(floor(x × frame_width / reference_width), floor(y × frame_height /
reference_height))`; when one misses its class, the detector reads nothing.

## Frame ROI

`frame_roi` cases place a detector's ROI, written against the reference
extent, in a frame's pixel extent. Only a uniform rescale is followed: when
`|frame_width × reference_height − frame_height × reference_width|` exceeds
the reference's longer side, the frame is another shape and there is no ROI.
Edges map by `floor(edge × frame / reference)` on their own axis; an ROI that
collapses to nothing has no place. The scale is the frame width over the
reference width, in lowest terms. The kernel and the runtime's check of what
the kernel answered both place ROIs through this one function.
