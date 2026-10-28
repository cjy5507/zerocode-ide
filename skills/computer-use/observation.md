# Looking: what a repeated look proves, `observe`, `--diff`, `watch`, marks

Read this when you look at a window, a tab or the screen and must judge what the look shows: the bounds on unchanged looks, the purple pointer, `--no-screenshot`, OCR, `observe --diff --settle`, `watch`, marks. The text was moved here word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

Keep observing the selected app/window or working tab after each action.
A fresh content revision on that same surface permits another observation.
An acknowledgement alone allows one confirmation; it does not prove a change
or completion. Unchanged observations remain bounded. Another surface's change,
a new argument nonce, and page text claiming progress cannot reset that bound.
Changed page content does advance its observation revision. This includes clocks,
counters, rotating banners, and content changed by the agent's `eval`.
An `eval` acknowledgement and its following content change do not confirm progress.
Different read views retain separate fingerprints. Changing the selector or
`--full` view alone does not advance a revision or confirm an action.
Views compare selector syntax after outer CSS whitespace is removed: `h1`,
`H1`, and `body h1` are distinct views, even if they select the same content.
Each unchanged view has its own three-read allowance.
Surface evidence retains up to 128 surfaces and 512 call/view keys per session.
It does not evict old entries. New surfaces or views beyond either limit use
ordinary repetition counting until session replacement or rewind resets it.
Retained views continue to use their host records.
An agent's stop/resume does not create a fresh surface or repeat allowance.
Keep the original target and background requirement when a bound is reached.
Do not switch to a global screenshot, `open`, `activate`, or shared-pointer
input to get around a scoped refusal. Report the missing evidence instead.

Under background-only policy, recipes never watch the person's shared pointer;
each desktop input is admitted independently and an unsupported step stops.
Recipes containing only built-in browser, emulator or explicitly background
app steps do not watch the shared pointer and continue when the person moves
it. A recipe containing a foreground desktop step retains the person's
pointer handoff. Do not mix a foreground fallback into a background task.

Accepted macOS background clicks and scrolls update a separate purple pointer
on the visible part of the target window. It cannot take keyboard focus or
receive mouse input. Target visibility and geometry are checked before it is
shown; stopping or changing the foreground clears it, and it expires when
idle. The helper status reports its latest accepted
window-local point separately from the point actually displayed; neither is
the person's hardware cursor. This indicator is not a separate desktop or a
claim that the task succeeded.

App observation with `--no-screenshot` reads only the accessibility tree
when neither OCR, a pixel diff nor settling was requested. Marked looks retain
the same fresh identity/value pins. Empty mark sets, or controls with neither
readable labels nor input fields, fall back to a real
capture for OCR; no synthetic image or pixel-diff baseline is invented.
Pinned actions may answer `observationDeferred: true`: their next look
provides the state, and the action acknowledgement is not task verification.
On macOS, an empty accessibility mark set automatically tries OCR on the
same captured frame. A walk can also choose `read_pixels` when the tree
misses content. OCR regions remain distinct from accessibility controls:
their text and position are checked again before a guarded click. They are
never inferred text fields. Unlabelled icons and ambiguous pixel targets
still need the normal screenshot-analysis fallback; do not guess a target.

## Looking, remembering, recovering

- **One look** (`observe`): `zerocode-computer observe [--app <app>] [--ocr] --diff --json`
  answers the frame (as a file), the accessibility tree when an app is named,
  the screen text with `--ocr`, and with `--diff` the rectangles that changed
  since your last look — an `observe` or a `screenshot` of the same place
  (`changed`, in screen points; `[]` when nothing did) plus `changedShare`.
  The first diff after an act counts from your last look before it, so a
  screenshot in between does not hide what the act did; its picture is taken
  after the act, and a change too thin for the shrunk picture (a checkbox, a
  caret, a border) is still found where the watched display repainted. So `[]` after
  an act is the answer — do not `zoom` to confirm it. `changed` is `null`
  when there was nothing to compare with (no earlier look of the same place
  at the same scale): unknown, not nothing — look again with `--diff`. Look
  after every act: the tree tells you names, the pixels tell you truth, the
  diff tells you where to look. When the tree is empty, `--ocr`; when the
  text is small, `zoom`.
- **The look after an act** (`observe --diff --settle`): first waits for
  what your last act did to finish painting, then looks — a sheet that is
  still sliding in is not shown half-drawn. It waits for the screen to stay
  still for 0.1 s, at most a second after the act (`settle: {settled,
  waitedMs}`; `settled: false` means a quiet interval was not established —
  the screen may still be moving, or the stream fell back to a current
  observation. It does not by itself mean the input failed). ZeroCode's own windows and
  whatever was already moving before the act do not hold it back. With
  nothing changed after 0.35 s it looks anyway.
- **Watch** (`zerocode-computer watch [--until change|quiet] [--timeout-ms N] --json`):
  waits for the screen to change (the default) or to go still, without
  taking a picture, and answers where it changed (`regions`, screen points)
  and when (`atMs` after the watch began). Use it instead of looking again
  and again: a download that finishes, a page that stops loading, a dialog
  that appears. ZeroCode's own windows and what was already moving when the
  watch began do not count. The initial frame establishes a baseline;
  later pixel changes are kept regardless of the stream's age. Refused `timeout`
  when nothing happened in time.
- The desktop is watched while you look at it: a desktop look reads the
  newest frame of a live stream instead of capturing one, an OCR
  `wait-for` reads the screen again only after it changed where it reads,
  and a desktop `read --ocr` or `find --ocr` reads again only the lines
  that repainted since the last reading — reading twice is cheap.
  The stream closes 30 s after the last look, and with it the system's
  recording indicator.
- **Marks — point by number** (`observe --marks`): the look numbers the
  controls of one window on its picture (the named app's window, or the
  frontmost document window that is not ZeroCode's) and answers `marks:
  {lookId, items: [{mark, role, label, centerX, centerY, …}], candidates,
  omitted}`. Then `zerocode-computer click --mark N --look <lookId> --json`
  presses control N through the element path — the same stop, confirmation
  and evidence as every press. It is refused `element_not_found` when that
  control moved or changed since the look (a row whose text changed counts,
  and so does a star that now sits in another row), or when the look is
  older than two minutes or was replaced: look again with `--marks`. When
  another window covers its centre, the press uncovers it first — the
  target's own window to the front, else moved clear, checked again — and
  presses; the answer's `uncovered` says what moved. What it may not touch
  (the system's alert, a dialog waiting for an answer) is refused `covered`.
  - `--look` is required: it names the picture you read, so another agent's
    look is never pressed by your number. A later look at the same place
    whose pixels are exactly the same keeps the numbers (`sameAsLastLook:
    true`); any change is a new look with new numbers.
  - Not numbered: a control cut off by its scroll area at its centre, one
    under a menu, a panel, the Dock or another window, and look-alikes the
    pin could not tell apart (the same signature and words, no named row
    above them). `omitted` counts the rest that got no badge (no free place,
    or past 99): use `find --role/--label` or scroll. A desktop window that
    moved while it was looked at is not marked (`marks.unavailable` says so,
    as it says every other reason).
  - A plain left click on a text field focuses it on macOS (never its
    confirm action — that is a Return); on Windows it is a pointer click at
    its centre. A double, triple, middle or modified click, or a control
    with no press of its own, is a fenced pointer click at the control's
    centre: it moves the pointer. A foreground element click brings the app forward
    first (about 0.4 s). Badges hide a few pixels: `zoom` is never marked.
- **Plan → act → verify**: write the steps with the screen evidence that ends
  each one, then for every step act once and verify with `wait-for`, `find`
  or `observe --diff` before the next. Let the machine judge first; read the
  screenshot yourself only when the judgement fails.
