# Game-state probe

Draws the owned game-state scenes and times the helper's real perception
kernel on them. It opens no window, captures no screen and posts no input:
every frame is its own drawing in an IOSurface-backed BGRA buffer, the kind of
buffer the display stream hands the helper. Outputs belong outside git.

- `scenes.py` writes `crates/zerocode-core/fixtures/game-state/scenes.json`:
  what each scene draws, what is true of it (every cell's class and the first
  red cell) and its split. The spec was written from the tune scenes and fixed
  before any held-out scene was drawn; held-out scenes use other seeds and add
  cover, other themes, turns, stretches and frame metadata the spec does not
  read.
- `build.py --output <scratch>/probe` compiles `probe.swift` with the helper's
  Core sources as one optimised module, so the probe runs the product's code.
- `probe render <scenes.json> <dir>` draws every scene into `<id>.png` (sRGB).
  The committed PNGs beside `scenes.json` are those drawings; the Swift test
  `testHeldOutScenesReadWithoutAWrongAction` reads them with the kernel and
  looks at the truth only after each observation.
- `probe measure <scenes.json> <dir> <out.json>` times, per series, the first
  observation apart and then warm observations (a new capture each): the
  fixture board on every clean held-out scene, the largest blobs detector the
  limits allow, a tick of them up to the tick's sample limit, and the template
  and motion kernels no plan runs yet over fixed ROIs. Every row records the
  load before and after.
- `summarize.py <out.json>` reports nearest-rank p50/p95 and the maximum in
  milliseconds and nanoseconds per sample. `python3 -m unittest discover -s
  tools/game-state-probe` checks it.

Timings are this machine's under the load each row records; they bound the
limits table (`game_state::LIMITS`) and are not a claim about other machines,
about capture or input, or about any game but the fixture.
