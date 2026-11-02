# Batch, a window with no tree, hearing, QA scenarios

Read this when you send several known hand steps as one `batch`, drive a window that shows only pixels (a mirrored phone, a remote desktop), listen for a sound, or walk a QA scenario to a `verdict`. The text was moved here word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## Batch — one judgement, many actions

A person does not stop to think between the keystrokes of a familiar
sequence. When you know the next several hand steps and would not look
between them, send them as one batch — one call, and one look after:

```text
zerocode-computer batch --commands '[["mouse-click","--x","640","--y","412","--foreground"],["type","--text","hello","--foreground"],["key","--key","tab","--foreground"],["type","--text","world","--foreground"],["key","--key","return"]]' --json
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
- On macOS an element's number lasts until the app removes that control, so a
  step may name one after any other step. On Windows numbers follow the walk: after
  an action or a `wait-for` that looks through an app's elements, name places by
  coordinates, or batch from a fresh `get-app-state`.
- `parallel` runs several batches at once, one per app; see `SKILL.md`. Batches
  at once have no one focus, so `type`, `key` and `hold-key` that name no app are
  refused there: use `type-text --app A` and `press-key --app A`.
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
nothing but the window. Look with `observe --app <app> --ocr`: the words it
shows come back with their screen points (`text.lines`), so there is no
picture to open. Plan a whole screen from one look and send it as one
batch — every step naming its control by the words the screen shows, read
again at the press, so a step may follow a scroll or a sheet the step before
it opened. Pixels have no background road: every press by words on them takes
the person's pointer for a moment, so each such `click` says `--foreground`
(send it only when the person asked for this app; keys go to the phone in the
background with `type-text --app` and `press-key --app`):

```text
zerocode-computer batch --commands '[["click","--app","iPhone Mirroring","--ocr","--text","Nationality","--foreground"],["wait-for","--app","iPhone Mirroring","--ocr","--text","Search","--timeout-ms","3000"],["click","--app","iPhone Mirroring","--ocr","--text","Search","--foreground"],["type-text","--app","iPhone Mirroring","--text","Korea"],["click","--app","iPhone Mirroring","--ocr","--text","Korea, Republic of","--foreground"],["wait-for","--app","iPhone Mirroring","--ocr","--text","Korea, Republic of","--timeout-ms","3000"]]' --json
```

A batch may cross screens too. When the task already names words the next
screens will show (an app's name on the home screen, a place's name in a list
of results), put `wait-for --app <app> --ocr --text <those words>` between the
presses and send the whole path as one batch: it stops at the first step that
fails and says which, and only then do you look again.

Text for a mirrored iPhone (`type-text --app "iPhone Mirroring"`) goes as the
keys of the phone's own keyboard, in the background: the phone takes key
positions, not characters. Korean, English, digits and the US punctuation keys
are typed, and the keyboard's language is read by two probe keys it deletes and
changed with Caps Lock or Control-Space. The answer's `phoneKeyboard` says what
it did, and its verification what the screen showed. Emoji, other scripts and
line breaks are refused before a key goes (type each line, `press-key Return`
yourself); `phone_keyboard_unknown` means the language could not be read and
nothing stayed typed. The field must already be focused on the phone.


- `click --app A --ocr --text <words>` presses the line OCR reads as those
  words; words that recur ("Yes", "No", "Select") take `--after-text <the
  line they follow>`, and `--dx/--dy` nudge the press off the words (a field
  under its label). One line presses; none or several stops the batch and
  names what it read. Words that name a payment, a transfer or a delete are
  held for the person as that step.
- Keys name their app: `type-text --app A`, `press-key --app A`. A press by
  words gives the person's app back when it ends, so keys with no app could go
  to whatever they have in front. As a net, a batch's or a recipe's `type` or
  `key` goes to the app the nearest step before it acted in or brought up (a
  `wait-for --app B` hands nothing to B); after a `mouse-click`, a `mouse-drag`
  or a `window-focus`, or with no app named before it, it goes where the focus
  is.
- These are refused when the batch is checked, before anything moves: keys with
  no app after a press on a mark, which leaves the person's app in front (name
  the app); a `hold-key` after a step in an app (it has no app of its own:
  `activate --app A` first); and a chord only the system hears, like `cmd+space`
  or `cmd+tab`, after a step in an app (send it by itself as `key`, or after
  `activate --app A`).
- Keys into a window with no tree are read back off its pixels. `inputCheck`
  says `match` (the screen shows the value, which it may have shown already),
  `mismatch` (nothing on the screen comes near the value: the batch stops at
  that step, code `unfinished`, and `at`/`of` say which character parted, never
  the value; nothing is typed again, so look before you do) or `unread` (`why`
  says why, and the batch goes on: check it yourself). `close` means the screen
  reads something near the value, which may be OCR's slip or a few wrong keys;
  `already_shown` that the press before the keys read the value there already;
  `shown_twice` that it stands twice in a row. OCR's `1`/`l` and `O`/`0` are not
  counted as differences in a value with letters; a value of digits is read as
  digits. A secret is not compared. A window with a tree says `has_tree` and its
  pixels are not read. Windows has no such reading yet, so it says `read_failed`.
- A password, a card number or a one-time code on a phone's screen is the
  person's. The check that refuses keys into a password field reads the Mac's
  tree and its secure input, and it cannot see a field on a phone's pixels, so
  nothing stops you: do not type it. Hand it over with `handoff`, then look.
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
