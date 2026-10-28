# One app's window and the whole desktop: every `zerocode-computer` verb

Read this when you act in one app's window (the core loop, `set-value`, `type-text`, `paste-text`, verification) or on the whole desktop with no app named (screen points, `launch`, `window-*`, `clipboard-*`, `find`, `wait-for`, `read`). The text was moved here word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## Core loop

```text
zerocode-computer list-apps --json
zerocode-computer list-windows --app <bundle-id-or-name> --json
zerocode-computer get-app-state --app <bundle-id-or-name> --json
zerocode-computer click --app <bundle-id-or-name> --element-index <n> --json
```

Use bundle IDs from `list-apps` when possible. If an app has several windows,
select a fresh `window-id` from `list-windows` and keep passing it until the UI
changes. Element indexes are short-lived: every action may repaint the target,
so use the returned state or run `get-app-state` again before the next action.
Never infer indexes from `elementCount`; use the numeric labels in `treeText`.

## Actions

`press-key` accepts a single key or a modifier combination. For combinations,
it runs `hotkey` and reports `ranAs: "hotkey"`. For example,
`press-key --key cmd+v` and `hotkey --key Cmd+V` use the same input path.
`command+v` and `Ctrl+Shift+T` work too. Keep the app and window scope.

```text
zerocode-computer get-app-state --app <app> [--window-id <id>|--window-index <n>] [--restore-window] [--no-screenshot] --json
zerocode-computer click --app <app> (--element-index <n>|--x <x> --y <y>) [--mouse-button left|right|middle] [--modifiers CmdOrCtrl+Shift] --json
zerocode-computer perform-secondary-action --app <app> --element-index <n> --action <advertised-name> --json
zerocode-computer set-value --app <app> --element-index <n> --value <text> --json
zerocode-computer type-text --app <app> --text <text> --json
zerocode-computer press-key --app <app> --key Return --json
zerocode-computer press-key --app <app> --key cmd+v --json
zerocode-computer hotkey --app <app> --key CmdOrCtrl+A --json
zerocode-computer paste-text --app <app> --text <text> --json
zerocode-computer scroll --app <app> (--element-index <n>|--x <x> --y <y>) --direction down --json
zerocode-computer drag --app <app> (--from-element-index <n> --to-element-index <n>|--from-x <x> --from-y <y> --to-x <x> --to-y <y>) --json
```

Prefer `set-value` for settable fields and advertised accessibility actions for
controls. Use synthetic typing or coordinates only when semantic actions are
unavailable. Coordinates are window-local; when screenshot `scale` is not 1,
divide screenshot pixels by that scale.

For sensitive or multiline payloads, keep the text out of process listings:

```sh
printf '%s' "$TEXT" | zerocode-computer paste-text --app <app> --text-stdin --json
printf '%s' "$VALUE" | zerocode-computer set-value --app <app> --element-index <n> --value-stdin --json
```

Read `action.verification` separately from transport success. `verified` means
the provider read the expected value back. `unverified` means the input was
attempted; inspect the returned state before claiming it worked — its
`reason` says why: `value_unchanged` (typed, and after the settle the field
still reads what it did: it does not show the text — the keys may have
missed it, or it hides what is typed, like a terminal's password prompt;
look first, and never type it again blind),
`value_mismatch` (the field rewrote what it got), `readback_unsupported`,
`synthetic_input`, `clipboard_paste`, `secret_field` (a `set-value` into a
password field, never read back). A desktop `type` reports the same.

## The desktop — no app named

Everything above addresses one app's window. A person also has the whole
screen, the mouse and the keys wherever they are: launching what is not
running yet, dragging between two apps, a dialog no app claims. Those are the
desktop verbs (design: `docs/design/computer-use-full-operator.md`). They
work in screen points — the desktop's coordinates, origin at the main
display's top-left — and a screenshot's `origin` and `scale` say how to read
a point off the picture (`desktop = origin + pixel / scale`). Inside zo, the
`Computer` tool does this for you: every position it takes and answers is in
the last screenshot's pixels, and a region is its two corners `[x0, y0, x1, y1]`.
A display left of or above the main one has negative points:
`--region -2560,-38,560,1340` and `--region=-2560,-38,560,1340` name the same
region, as any flag's value may follow it after `=`.

```text
zerocode-computer screenshot [--display N] [--region x,y,w,h] [--full-res] --json
zerocode-computer zoom --region x,y,w,h --json
zerocode-computer displays --json
zerocode-computer cursor-position --json
zerocode-computer mouse-move --x X --y Y
zerocode-computer mouse-click --x X --y Y [--mouse-button left|right|middle] [--click-count 2] [--modifiers chord]
zerocode-computer mouse-drag --from-x X --from-y Y --to-x X --to-y Y [--steps N]
zerocode-computer mouse-scroll --x X --y Y [--dx N] [--dy N]
zerocode-computer key --key <key|modifier+key>
zerocode-computer hold-key --key <key> --ms N
zerocode-computer type --text <text>
zerocode-computer wait --ms N
```

Around an app — starting it, bringing it forward, quitting it, opening a file
or an address, running a program, placing a window, holding the clipboard:

```text
zerocode-computer launch --app <name|bundle-id|path> [--args "<words>"] [--wait-ready ms] --json
zerocode-computer quit --app <app> [--force]
zerocode-computer activate --app <app>
zerocode-computer open (--path <file> | --url <address>) [--with <app>]
zerocode-computer run --program <path> [--args "<words>"] [--cwd <dir>] [--timeout-ms N] --json
zerocode-computer list-all-windows --json
zerocode-computer window-focus|window-minimize|window-zoom|window-close --id <window-id>
zerocode-computer window-move --id <window-id> --x X --y Y
zerocode-computer window-resize --id <window-id> --width W --height H
zerocode-computer clipboard-read --json
zerocode-computer clipboard-write --text <text>
```

`launch` answers the process and whether a window appeared within
`--wait-ready`; `run` answers the program's head of output, its exit code and
whether the budget cut it — a program that needs a terminal of its own belongs
in a pane, not here. Window ids come from `list-all-windows` (front to back).

Finding, waiting and reading — the meaning layer over both roads:

```text
zerocode-computer find --app <app> (--text <t> | --role <r> [--label <l>]) --json
zerocode-computer find --ocr [--display N] [--region x,y,w,h] --text <t> --json
zerocode-computer wait-for (--text <t> | --role <r> [--label <l>]) (--app <app> | --ocr) [--timeout-ms N] [--absent]
zerocode-computer wait-for --window <title fragment> [--timeout-ms N] [--absent]
zerocode-computer read (--app <app> [--window-id N] | --ocr [--display N] [--region x,y,w,h]) --json
```

`find` answers screen-point frames (`centerX`/`centerY` are ready for
`mouse-click`), and an accessibility match also carries its element `index`
for `click --app`. `--ocr` reads the pixels: use it when the tree is empty
(games, canvases, remote desktops, plugin-heavy sites) or to confirm what a
person would see. A desktop `--ocr` never reads ZeroCode's own windows — your
own commands scroll there, and they are not the app's answer. `excludedRegions`
names the screen regions excluded from the result; their text and match counts
are unknown. If the app may be behind those regions, activate it and look again.
`wait-for --window` never matches ZeroCode's own window. `wait-for` ends with `satisfied` or a `timeout` error, never
a silent hang; an app or window that is not there yet is looked for again
until the budget.

The loop is the same as an app's: look (`screenshot`), act, look again. Prefer
the app verbs when the target has a window and an accessibility tree — they
verify what they did; a desktop click verifies nothing but the next
screenshot. `zoom` returns the region at the display's own resolution when
small text has to be read. `wait` is answered by the window and capped by its
table; anything longer belongs to a scheduler, not a pause. `screenshot` needs
the screen-recording permission and every input verb the accessibility one
(`zerocode-computer permissions`).
