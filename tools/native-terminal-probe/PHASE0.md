# Phase 0: one recording, two renderers

Phase 0 feeds one reproducible set of terminal frames to the window's existing DOM
terminal and to a GPUI grid that draws every cell. For each renderer it checks what
was drawn against the grid's own screen and times the same spans of a frame. It
does not replace the shell or change the product: everything here lives in this
folder, builds outside the root workspace, and leaves `ui/` and `crates/` as they are.

## The recording

`recorder/` (`phase0-record`, needs only `zerocode-pty`) feeds bytes into
`zerocode_pty::Terminal` and, after every step, takes frames the way the window's
`term_pull` does: `FrameReaders::pull` for one reader. The first pull is that
reader's floor, a full frame; every later pull is the grid's delta since the one
before. The recording is exactly what one window would be handed.

Per case `<name>`:

| file | what it holds |
|---|---|
| `<name>.deltas.jsonl` | one frame a line: the `serde_json` of a `zerocode_pty::GridDelta`, the bytes `term_pull` puts in an answer's `frames` array (packed rows: `text`, `runs`, `zw`) |
| `<name>.reference.jsonl` | `{"frame": k, "screen": <GridDelta>}` at each checkpoint: `TerminalGrid::snapshot()` after frame `k`, the grid's own screen, never rebuilt from the frames |
| `<name>.ansi` | every byte fed, in order |
| `<name>.meta.json` | size, steps, which step made which frame (`made`), the checkpoints |

`run.sh record` writes the eight built-in cases at 50 × 170, the size both
renderers can show whole on a 14-inch display (see "The whole grid" below);
`phase0-record cases` alone defaults to 60 × 220 (the window harness's
`REAL_PANE`) and takes `--rows` and `--cols`. `phase0-record bytes --file F --name N [--rows R --cols C --chunk BYTES]`
records any pty byte file, one pull per chunk. The recorder reads no clock into a
frame (a synchronized-output block is opened and closed inside one step), and
`run.sh record` writes every case twice and requires the two sets of files to be
byte-identical.

| case | frames | what it exercises |
|---|---:|---|
| `sparse-log` | 241 | a build log, three lines a frame, two colour runs a line, most cells blank; `scrolled_lines` |
| `dense-color` | 91 | the alternate screen repainted whole every frame inside `CSI ?2026`, a colour run every four columns, box drawing, bold |
| `wrap` | 101 | lines longer than the pane; a wide glyph at the last column wraps early; an address and an OSC 8 link across the edge |
| `hangul` | 121 | Korean prose, bold, colour and underline; decomposed jamo (`ᄒ ᅡ ᆫ`) as one cluster; Korean wrapping at the edge |
| `emoji` | 121 | presentation selectors, ZWJ sequences, flags, skin tones, keycaps, combining marks; emoji at the last column |
| `cursor` | 161 | a still screen with small edits: moves, erase, insert and delete characters, save and restore, hide and show, reverse and italic cells |
| `scroll` | 120 | a scroll region under a fixed header and status line, floods of half and two screens a frame, reverse index, the view moving back and forth through history (`view_shift`) |
| `resync-resize` | 54 | a full frame mid-stream (`term_pull` with `resync`), a shrink by 10 rows and 20 columns and a grow back |

## The two renderers

**DOM** (`phase0/dom-replay.mjs`): the window itself, in the window harness's
headless Chromium (`ui/tests/window-boot.mjs`), a terminal tab in front. One
answer is replaced: `term_pull` returns, for one frame at a time, the bytes the
backend would have written — `{"screens":[{"term":…,"frames":[<the recording's line>]}],"missing":[]}` —
after the stub's own notice (`term:dirty:<label>`). The window then pays the
decode, the parse and the apply it pays in the app.

**GPUI** (`src/bin/phase0-gpui/`): a screen model that follows the shift contract
on `GridDelta` (full → move by `scrolled_lines` or `view_shift` → overwrite rows),
and a grid element that paints every cell at its own column each frame: grounds as
runs of one colour, each glyph shaped once per text and face and kept, colour
applied when painted, a wide glyph centred on its two columns. It paints with the
palette, ink, ground, face size and cell size the page resolved
(`dom-theme.json`). It refuses, by name, a frame with folds or a size change
without a full frame. It ignores `row_lines` (slot → line numbers for selection),
`links` (an OSC 8 link draws as its text), `title`, `bell` and the mouse and focus
modes, none of which change a drawn cell. It does not apply the window's
legibility lift (a minimum contrast between ink and ground); the comparison counts
the cells the lift changed.

## The whole grid, in both

Both renderers must show every row and column of the recording. macOS keeps a
titled window inside the screen's visible frame (the screen less the menu bar
and the Dock), and GPUI culls what falls outside its window, so on a small
display a large grid would be drawn in part: fewer glyphs painted, and a picture
cut off at the window's edge. Neither GPUI pass draws such a grid: before the
first frame each checks the window it was given and stops, saying how many rows
and columns would fit. The DOM pass holds its page to the same rule, and the
comparison refuses a capture in which either picture holds only part of the grid.

The phase 0 set is recorded at 50 × 170. At the page's cell (Menlo 14 px,
8.43 × 16.09 points) that is 1433 × 805 points, inside the 1512 × 888 points a
14-inch MacBook's built-in display leaves a window with the Dock below (about
1450 wide with the Dock at a side). The window harness's 60 × 220 pane needs
1854 × 966 points.

## What is timed

`timing` feeds one frame per display frame. Every frame keeps its raw sample; the
summaries leave out frame 0 (the full floor onto an empty grid).

| interval | DOM | GPUI |
|---|---|---|
| frame build — the paired number | from handing the bytes over, inside the display frame's rAF, to the message posted in that rAF, which runs after the frame's style, layout and paint (the definition `ui/tests/terminal-pacing.mjs` uses) | from applying the frame, inside GPUI's frame callback, to the end of that frame's `Window::draw` (layout, prepaint, paint into the scene), from GPUI's own frame journal (`gpui::profiler`) |
| apply | the window's `applyFrames` for that frame (model and DOM writes) | the screen model update |
| decode | bytes handed over → apply start (`TextDecoder`, `JSON.parse`) | none: a native screen reads the grid in-process |
| submit | not visible to the page; headless Chromium does not composite on the GPU | frame applied → end of the platform submission (`present`); CPU time, not GPU execution |
| gap | rAF to rAF while frames are fed | frame callback to frame callback |

Each sample also says whether the frame was applied and drawn in the display frame
it was fed in (DOM: the apply ended before that frame's message; GPUI: the draw
started in the same turn). A late frame is counted, not dropped.

Controls, in every round: `idle` frames with nothing fed (the frame supply of the
level), and for GPUI `redraw` frames that draw the unchanged final screen (GPUI
keeps no painted scene between frames, so this is what a frame that changes nothing
costs it; the DOM rewrites only the rows a frame changed). The original probe's
mode — the final text of a recording as one string — is a lower bound of text
drawing only and is not used for any terminal number.

## Conditions

As `tools/low-spec/profile.mjs` defines them:

- `normal`: as the machine is.
- `low`: `taskpolicy -b` on both renderers' processes (efficiency cores at
  background QoS). It also coalesces timers: frame gaps under `low` say what a
  starved process does, not what a slow display does.
- `cpu4x`: the page's CPU slowed 4x over CDP (`WINDOW_CPU_THROTTLE=4`). DOM only —
  there is no GPUI equivalent.

Each process times the plain CPU loop of `tools/low-spec/spin.mjs` (calibration),
so the slowdown a level put on each renderer is read, not assumed. The rounds flip
order (DOM first, then GPUI first, …). Load average and the core count are written
before and after each case; a case started above the core count is flagged
`loud`.

## When a round runs, and when a pass counts

GPUI stops a window's frames while macOS reports it not visible (covered,
minimised, hidden, on another Space, a sleeping display: one AppKit signal for
all of them). On a machine a person uses, an ordinary window gets covered, so the
first 5-round run lost three of its GPUI passes that way. `timing` now runs under
the conditions the coordinators set (m-118419, m-118430):

- The GPUI pass opens its window as GPUI's pop-up (`--window-kind popup`): a
  panel above ordinary windows, on every Space, never taking the keyboard; no
  input is sent to it. Its opaque title bar reads "ZeroCode 측정 창 — 자동으로
  닫힙니다". It covers 1434 × 806 points of a 1512 × 982-point screen for about
  half a minute a pass.
- A round starts only when the keyboard, mouse and trackpad have rested 600 s
  (`HIDIdleTime`, and the HID event source's own count), the one-minute load is
  under 12, macOS reports the thermal state nominal, the main display is awake and
  16 GiB are free. The stage waits up to 10 minutes for that, then skips and logs
  the round; it waits 30 minutes in all at most.
- `caffeinate -d` holds the display awake for the stage only; it is tied to the
  stage's shell and ends with it.
- An input event during a round breaks it. The GPUI replay checks twice a second,
  outside any frame, and ends its pass at once: samples written, window closed
  (exit 5). The DOM pass is headless and is judged at its end (the HID idle then
  is shorter than the pass). The next attempt waits for the full idle again.
- The GPUI replay records every visibility change GPUI reports and feeds the
  first frame only once GPUI has reported the window visible: the pop-up opens
  reported hidden, frames come at once, and the visible report follows 50–75 ms
  later (the first gated smoke fed at 11 and 29 ms and so did not count). It ends
  its pass when the window is reported not visible after the first fed frame, or
  is not reported visible within 5 s of opening (exit 6), when the stage's shell
  is gone (exit 7) or when no frame comes for 5 s (exit 4).
- `phase0/rounds.mjs` holds the one rule a pass is counted by, for `run.sh` while
  it runs and for `report.mjs` after: exit 0, no input during it, thermal state
  nominal before and after (and, for GPUI, for every case), every case run to its
  end once, no loud case, and for GPUI the window reported visible when the first
  frame was fed and never reported not visible after. Rounds are tried until 5
  passes of each renderer count (12 attempts at most); otherwise the stage exits 3
  and the report ranks nothing. Passes that do not count stay on disk and are
  listed with their reasons.

GPUI folds a burst of visibility changes into its last value and delivers it
after the turn in progress, so "visible throughout" means no change was reported
and no frame stopped coming, not that the window was never covered for an
instant.

## Memory

- Runtime: the resident set (`ps -o rss`) every 250 ms during a case — for the DOM
  the sum over the Chromium process tree (browser, renderer, GPU, utility), for
  GPUI its one process — and the peak `/usr/bin/time -l` reports for GPUI.
- Build: the GPUI build's peak resident set and wall time (`/usr/bin/time -l`
  around `cargo build`, a clean target), and the target folder's size.

## The equivalence check

`capture` replays the same frames without timing. At each checkpoint:

- The DOM pass reads the grid as the browser laid it out — each grapheme's box
  (its column is the box's left edge over the measured cell width), the run's SGR
  classes, its computed colour, each run's background, the caret element — and
  takes two screenshots of the grid at the page's device pixel ratio: one with the
  caret, scrollbar and fold handles hidden, one with the caret held still and
  shown. It reads, once per case, the fonts the browser actually drew with
  (`CSS.getPlatformFontsForNode`) and writes the family its ASCII runs used
  (`dom-fonts.json`).
- The GPUI pass paints in that family (loading a system family macOS hides from
  apps, such as SF Mono, from its file), in a window that is never shown, and
  reads each frame back with `Window::render_to_image` — once without the caret
  and once with it — beside what its painter placed.
- Both record the scale, the viewport, the grid's bounds and whether the grid is
  whole in them.

`phase0/compare.mjs` holds each renderer against the reference screen:

| layer | what is compared |
|---|---|
| glyphs | each row's glyphs aligned to the reference by text, then: `missing`, `extra`, `text` (a different glyph at a column) — characters lost or wrong; `misplaced` — a glyph drawn half a cell or more from its column, with the largest offset in cells; `face` — bold, italic, underline, strike from the run's SGR classes (a link's transparent underline is not a face); `colour` — the ink against the page's palette, with the DOM's legibility lift counted as `lifted` |
| cursor | the DOM's caret element and GPUI's model cursor, each against the reference; and in both renderers the cell found in pixels, where the picture with the caret differs from the one without |
| pixels | one analysis of both pictures, each cell read one device pixel in from its edges: a glyph or decoration must leave ink against the cell's own background, a blank cell must not, and the ground must be the cell's background unless ink covers most of it. Missing ink where the style asked for ink as dark as its ground is `faint`; ink beside a glyph in a blank cell is that glyph's `spill` |

## Not measured here

- Real input: key presses, the key echo, IME composition (Korean), paste, mouse,
  selection and copy, link activation, scrolling by wheel.
- Accessibility: no accessibility tree is built or compared.
- The product's own webview: the DOM numbers are headless Chromium's, not
  WKWebView's (the window on macOS), and not WebView2's (Windows).
- The GPU and the display: neither renderer's numbers include GPU execution or
  scanout; the DOM's include no compositing at all.
- The real IPC: the bytes arrive through the harness's stub, not Tauri.
- Several panes, the board's previews, folds, and the backend's reader shares.
- The window harness's 60 × 220 pane: a GPUI window cannot be that large on the
  14-inch display the runs were made on.
- Windows: no Windows run. GPUI's Direct3D 11 and DirectWrite back ends, ConPTY,
  and a low-spec Windows machine are all outside this phase.

## Reproduce

From this checkout, on the build line (`zo line add … -- <command>`):

```sh
bash tools/native-terminal-probe/phase0/run.sh record  OUT        # recordings at 50 × 170, twice, byte-identical
bash tools/native-terminal-probe/phase0/run.sh build             # the GPUI replay (Rust 1.98.1)
bash tools/native-terminal-probe/phase0/run.sh capture OUT/recordings-…-a OUT/capture
bash tools/native-terminal-probe/phase0/run.sh timing  OUT/recordings-…-a OUT/timing normal 5   # --quiet; waits for the gate
bash tools/native-terminal-probe/phase0/run.sh timing  OUT/recordings-…-a OUT/timing low 5      # --quiet
bash tools/native-terminal-probe/phase0/run.sh timing  OUT/recordings-…-a OUT/timing cpu4x 5    # --quiet, DOM only
bash tools/native-terminal-probe/phase0/run.sh report  OUT/timing
node --test tools/native-terminal-probe/phase0/rounds.test.mjs                                     # the counting rule
```

`run.sh` builds and runs in the probe's own `target/`, never in a build line's
slot folder (the line names one in `CARGO_TARGET_DIR`), so the replay one job
builds is the one the next job runs; `PHASE0_TARGET_DIR` names another folder.
A timing stage exits 3 when fewer passes than asked for count. The gate's numbers
can be changed for a trial run (`PHASE0_GATE_IDLE_S`, `PHASE0_GATE_LOAD`,
`PHASE0_GATE_FREE_GIB`, `PHASE0_GATE_WAIT_S`, `PHASE0_GATE_BUDGET_S`,
`PHASE0_ATTEMPTS`, `PHASE0_WINDOW_KIND`); a run with other numbers is not one the
coordinators approved.

The pieces run alone too: `phase0-record list`, `node phase0/dom-replay.mjs timing|capture …`,
`phase0-gpui timing|capture …`, `node phase0/compare.mjs …`, `node phase0/report.mjs …`.

## Where the GPUI grid comes from

Read before writing, at pinned revisions:

- sheprd `crates/sheprd-gpui/src/terminal_painter.rs` and `terminal_painter/glyphs.rs`
  ([fae8e9fed754](https://github.com/sheprd-labs/sheprd/blob/fae8e9fed754/crates/sheprd-gpui/src/terminal_painter.rs),
  Apache-2.0): cells painted at grid positions, glyphs shaped once per symbol and
  face with colour applied at paint, grounds as runs. Phase 0 follows this shape;
  no code is copied.
- nyaterm `crates/nyaterm-terminal-gpui/examples/renderer_probe.rs`
  ([9894eedd6b07](https://github.com/nyakang/nyaterm/blob/9894eedd6b07/crates/nyaterm-terminal-gpui/examples/renderer_probe.rs),
  Apache-2.0): GPUI's frame journal (dirty → draw → present) for a terminal
  element, an unfocused window with the inactive frame limit off, readback
  outside the measured frames. The timing pass measures the same way.
- Zed `crates/terminal_view/src/terminal_element.rs`
  ([621a6adf3096](https://github.com/zed-industries/zed/blob/621a6adf3096/crates/terminal_view/src/terminal_element.rs),
  GPL-3.0-or-later, read only): runs of one style shaped as one line with a forced
  cell width, grounds as rectangles. Not followed for glyphs: a run shaped as a
  line places a wide glyph or a joined emoji by the font's advance, and phase 0
  needs every glyph on the columns the grid gave it.
