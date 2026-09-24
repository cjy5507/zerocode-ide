# Controlled speed probes

These are opt-in measurement tools, never part of the product input loop.
Use a simulator created for the run and a browser tab containing `page.html`.
Never pass another session's device. The programs do not enumerate devices,
open a desktop window, or change credentials. Outputs belong outside git.

Requirements: macOS, Xcode, Python with Pillow, the window's `zerocode-browser`
and `zerocode-emulator` shims. `build.py --output <scratch>` builds a small
SpriteKit board, an OCR line server, and the current production iOS helper.
Create and boot a dedicated simulator with `xcrun simctl`, install
`SpeedProbe.app`, and launch `test.zerocode.speedprobe`. Its data container's
`Documents/oracle.json` is the scorekeeper. It is read only for scoring, never
as the policy input. Delete that simulator after the run.

Run `probe.py --helper <scratch>/helper --ocr <scratch>/ocr --device <id>
--oracle <oracle> --output <results>`. `board.json` defines the fixture's
geometry and colours for both the app and reader. The reader is specific to
that fixture; its accuracy is not evidence of recognizing arbitrary games.

Run `web_probe.py --pane <owned-pane> --output <results> --large-model <id>`
with `ZO_CONFIG_HOME` pointing to a temporary home and `TYPESAFE_API_KEY`
present only in the command environment. It uses the existing value latency
probe's authenticated model transport without writing its normal ledger.
The key is never printed. A run is capped at 200 Jev requests. Select the large
model explicitly; the small model comes from the product's type-value table.
The optional `--read-full` models an outer loop that also reads page text;
the product's browser goal walk only asks for marks. `--wait-paint` includes
two animation frames, which can stall in hidden tabs. Default verification
checks the independent DOM outcome. Do not compare those completion criteria.

`image_loop.py` takes the same helper, device and oracle, an explicit `--model`,
and `--output`. It alternates a minimal cloud image request and a local policy.
This is not a whole agent turn. The fixture is portrait: rotation is measured
as an additional stage, and the correctly oriented original goes to the model.
The cloud arm uses the screenshot CLI, while the local arm keeps the helper.

Every row records machine load. The iOS hand comparison uses ABBA order and
scores actual input count plus rendered state, separately from input receipts.
The web comparison reverses arm order every round, keeps the same controls,
and uses a probe-local state-keyed memo; it does not claim to benchmark the
product's memo implementation or its whole walking engine. Rows contain no
credentials. Raw outputs may include the owned device's paths and identifiers:
keep them in private scratch, never in a commit.

For independent response timing, launch the fixture with `--stimuli` and run
`stimulus_probe.py --helper <helper> --device <id> --receipts <data-container>/Documents/reactions.jsonl
--output <results.json>`. The app changes the board every 250 ms and scores
the touch location and response delay on its own clock. The policy reads only
pixels. `paint_probe.py --pane <owned-pane> --output <results.json>` records
two-rAF latency with a seven-second deadline; null latency means censored,
not zero or seven seconds. `web_probe.py --extra` adds measured timing state
and a two-click macro with per-click verification. `--types-only --select-field`
includes a real Jev operation/target request before text generation.

Validation: `python3 -m unittest discover -s tools/speed-probe -v`.
