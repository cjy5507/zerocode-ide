# Batch, a window with no tree, hearing, QA scenarios

Read this when you send several known hand steps as one `batch`, drive a window that shows only pixels (a mirrored phone, a remote desktop), listen for a sound, or walk a QA scenario to a `verdict`. The text was moved here word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## Batch — one judgement, many actions

A person does not stop to think between the keystrokes of a familiar
sequence. When you know the next several hand steps and would not look
between them, send them as one batch — one call, and one look after:

```text
zerocode-computer batch --commands '[["mouse-click","--x","640","--y","412"],["type","--text","hello"],["key","--key","tab"],["type","--text","world"],["key","--key","return"]]' --json
```

- Each step is a command line exactly as you would send it alone, and each
  is stopped, counted, confirmed and logged as if alone: the hotkey, the
  person's last step (`--confirming payment|transfer|delete`) and the
  evidence all apply per step.
- Only the hand's actions and the waits: `wait`, and `wait-for` /
  `sound-wait` with `--timeout-ms` — gate on what the next step needs, so a
  miss stops the batch. No looks, no launch/quit/open/run, no batch inside.
- The batch stops at the first refusal and answers every step (`steps`,
  `refusedAt`, and the refused step's own code — the Recovery list below
  applies unchanged). Then `observe --diff` and decide from what you see.
- Element indexes change after every action, and after a `wait-for` that
  looks through an app's elements (`--app` without `--window`/`--ocr`): after
  such a step, name places by coordinates, or batch from a fresh
  `get-app-state`.
- It runs faster than anyone can reach the hotkey. Batch only what you have
  seen work; never secrets (typed text rides the command line — use
  `paste-text --text-stdin` alone). On a Windows cmd host the JSON's quotes do
  not survive; use PowerShell or Git Bash.
- In zo, the `Computer` tool's `batch` takes `steps: [...]` in the same
  action vocabulary and the same screenshot pixels as single actions.
- A `wait` right after a step that acts ends once what that act painted has
  held still (the batch adds `--settle`); it never ends before the screen
  moves, so the time you give it is the most it costs.

### A window with no tree: a mirrored phone, a remote desktop

An iPhone Mirroring window (or a remote desktop, or a game's menu) shows its
controls only as pixels: `observe --app` gives its picture and a tree with
nothing but the window. Plan a whole screen from one look and send it as one
batch — every step naming its control by the words the screen shows, read
again at the press, so a step may follow a scroll or a sheet the step before
it opened:

```text
zerocode-computer batch --commands '[["click","--app","iPhone Mirroring","--ocr","--text","Nationality"],["wait-for","--app","iPhone Mirroring","--ocr","--text","Search","--timeout-ms","3000"],["click","--app","iPhone Mirroring","--ocr","--text","Search"],["type","--text","Korea"],["click","--app","iPhone Mirroring","--ocr","--text","Korea, Republic of"],["wait-for","--app","iPhone Mirroring","--ocr","--text","Korea, Republic of","--timeout-ms","3000"]]' --json
```

- `click --app A --ocr --text <words>` presses the line OCR reads as those
  words; words that recur ("Yes", "No", "Select") take `--after-text <the
  line they follow>`, and `--dx/--dy` nudge the press off the words (a field
  under its label). One line presses; none or several stops the batch and
  names what it read. Words that name a payment, a transfer or a delete are
  held for the person as that step.
- Check each value inside the batch: `wait-for --app A --ocr --text <the
  value> --timeout-ms N` ends the moment the screen shows it and stops the
  batch when it does not — the check and the wait in one step.
- Name the app once, by the name `observe` answered (`app`); every name of
  the same window reaches it. A click brings its window forward: `activate`
  is only for keys into an app that is not in front.
- After the batch, look at that window once it has settled —
  `observe --app <app> --diff --settle` — or, when its words are enough to
  plan the next screen, `read --app <app> --ocr`: every line with its
  position, and no picture to open.
- In zo: `left_click` with `app`, `label` (the words), `ocr: true`,
  `after_text` and `offset` (pixels); `wait_for` with `app`, `ocr: true`,
  `text`. The batch's look after shows that app's window once it has settled
  — no need to look again before planning the next screen.

## Hearing

For continuous motion, zo's `Computer` action or batch can use `settle: false`:
it returns the latest captured frame without waiting for the scene to become
quiet. The CLI equivalent is an action or batch followed by `observe` without
`--settle` or `--diff`. This observation does not confirm that the action finished;
look again when its effect matters. Coordinates, stop, pacing and confirmation
follow the same action path. Use ordinary settling for forms and dialogs.


The operator hears the machine through the same Screen & System Audio
Recording permission the screenshots use — no new prompt.

- `zerocode-computer listen-start [--app <app>] --json` — the whole machine's
  sound, or one app's. `listen-stop` when done; a listener nobody reads from
  for ten minutes stops on its own.
- `zerocode-computer sound-wait [--label alarm_clock,siren] [--min-confidence 0.5] [--timeout-ms N] --json`
  — waits for a sound the system's classifier names (speech, music, knock,
  bell, beep, chime, siren, telephone, applause, typing and some 300 more);
  without `--label`, any sound. It answers the event (`label`, `confidence`,
  `seq`); pass `--after <seq>` to wait for the next one.
- `zerocode-computer sound-read [--after N] --json` — what was heard since.
  Hear instead of guessing from pixels: that a video plays sound, that a
  notification chimed, that a call rings, that an alert sounded.

## QA automation

A QA scenario is a person's sentence (「로그인 → 장바구니 → 결제 직전까지」) and
you walk it with the verbs above: the built-in browser for web apps, the
emulator for mobile, this operator for the desktop — one scenario may mix all
three. Every action already leaves a step and a frame in the run's evidence
folder. Assert with `wait-for`/`find`/`read` (their exit code is the
assertion), compare a screen against a baseline, and end with a verdict:

```text
zerocode-computer compare --baseline <png> [--against <png>] [--region x,y,w,h] [--max-diff 0.01] --json
zerocode-computer verdict --pass
zerocode-computer verdict --fail --reason "the cart stayed empty after 'Add'"
```

`compare` answers `pass`, `diffRatio`, and a `diffPath` picture (red where
the pixels differ, beyond the table's per-pixel drift). A `--fail` verdict is
a failed step in the log and is filed as a task for a person; write
`report.md` beside it naming the screenshots that show the failure.
