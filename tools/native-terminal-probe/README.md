# Native terminal text probe

This stand-alone macOS/Windows probe feeds a raw terminal recording through
`zerocode-pty` and draws the final visible text in a GPUI window. With no file,
it uses a small in-memory sample containing Hangul, a combining accent and an
emoji. It opens no shell or agent session.

From the repository root, run:

```sh
cargo +1.98.1 run --locked --manifest-path tools/native-terminal-probe/Cargo.toml
cargo +1.98.1 run --locked --manifest-path tools/native-terminal-probe/Cargo.toml -- recording.ansi
```

For a Windows runtime test, run the same build on the Windows machine. In
debug builds this GPUI snapshot opens its HLSL file from its build-time source
directory. A cross-linked debug executable verifies linking only.

To capture the probe's own offscreen frame on macOS:

```sh
GPUI_PROBE_IMAGE=frame.png cargo +1.98.1 run --locked --features capture --manifest-path tools/native-terminal-probe/Cargo.toml
```

The probe uses 24 rows and 80 columns. It reads the terminal grid's visible
text after all bytes have arrived. ANSI color, per-cell positioning, live
input, selection, accessibility actions and frame pacing need separate tests
before comparing the native window with a production terminal.

The dependencies match the exact GPUI snapshot used by GPUI Kit.
The probe's manifest and lockfile are independent of the application's
workspace. The probe uses the compiler pinned by the matching upstream Zed revision (d2a4fb6).

## Phase 0: the same frames, both renderers

`PHASE0.md` describes the phase 0 comparison built on this probe: a recorder
(`recorder/`, `phase0-record`) that writes the frames a terminal hands one window,
the window's own DOM terminal fed those frames (`phase0/dom-replay.mjs`), a GPUI
grid that draws every cell of them (`src/bin/phase0-gpui/`, built with
`--features capture,timing`), the equivalence check (`phase0/compare.mjs`) and the
timing report (`phase0/report.mjs`). `phase0/run.sh` runs the stages in order.
The text-only window above is a lower bound of text drawing and is not a
terminal's frame time.
