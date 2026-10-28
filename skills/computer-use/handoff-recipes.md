# Stuck, undo, the person's turn, recipes, memory

Read this when a look shows no progress, an action must be undone, the person must act (`handoff`, a one-time code), or you save or run a recipe. These are the rest of the list that starts with `observe` in `observation.md`; the text was moved here word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

- **Stuck** (`observe --diff` answers `stuck: true`, or `status` shows it):
  the same action twice with nothing changing is a loop. Stop, look at the
  whole screen, try the other road (a menu instead of a button, a key instead
  of a click, `activate` the app first), and if there is none, say so.
- **Undo before retry**: `key cmd+z`, the browser's back, `key escape` on a
  dialog, `window-close` on a stray window — the inverse of the last action,
  then look again.
- **The person's turn** (`handoff`): a CAPTCHA, a biometric prompt, an
  approval they tap on their own phone, a password, or a last step you may not
  press — `zerocode-computer handoff --reason "휴대폰의 승인 알림" --json` shows
  the person a card and waits (10 minutes by default) until they press 「다
  했어요」; then look again and go on. A cancelled or unanswered handoff is a
  `confirmation_refused`/`confirmation_timeout` answer: report, do not retry.
- **A one-time code** (`handoff --ask-code --into`): when the site sent a
  verification code to the person's phone or mailbox, ask for it on the same
  card instead of asking them to type it on their own device. The card gets
  one field and the seconds left; they type and press Send, and the **window
  types the code into the field you named** — you are never handed it. It is
  in no answer, no step line, no log and no ledger, and not in the echo of the
  field either: the answer says how many characters went in and whether the
  field read back as written. Say where it goes in `--into`, the words of one
  input command as a JSON array; say what the code is for in `--reason`
  ("카카오톡 인증번호"); and give it the time the site gives its codes
  (`--timeout-ms 180000` for the usual three minutes):

  ```text
  # an app's field, by its element index (a fresh get-app-state first)
  zerocode-computer handoff --ask-code --reason "카카오톡 인증번호" --timeout-ms 180000 \
    --into '["set-value","--app","<app>","--element-index","<n>"]'
  # the focused field of an app
  zerocode-computer handoff --ask-code --reason "…" --into '["type-text","--app","<app>","--restore-window"]'
  # a tab of the window's browser: its label, and the field's selector or the handle `fields` printed
  zerocode-computer handoff --ask-code --reason "…" --into '["browser","type","<pane-label>","<css>","--workspace-revision","<revision>"]'
  # a phone
  zerocode-computer handoff --ask-code --reason "…" --into '["emulator","text","--platform","ios","--device","<id>"]'
  ```

  The answer is one line, `entered 6 characters into set-value (verified)`, or
  with `--json` `{"result": {"resumed": true, "codeLength": 6, "entered": true,
  "into": {"tool": "computer", "verb": "set-value"}, "verification":
  "verified"}}` — `verified` or `unverified` for an app (the field was read
  back or it could not be), `none` for a tab or a phone. The place is checked
  before the card is shown: `--ask-code` without `--into` is refused, `--into`
  takes no `--value`, `--text` or `--json`, and not `paste-text` or the
  clipboard (it would stay there for `clipboard-read`). If the field cannot
  take the code, the person's code is dropped, not kept: you get the refusal's
  code and a new handoff is the way on, the person typing again. A code is four
  to ten letters or digits; if the person cancels or nobody answers you get
  `confirmation_refused` / `confirmation_timeout`: report, do not ask again. A
  password, a card number or a security code (CVC, PIN) is never asked this
  way: name one in `--reason` and the window shows the plain card and types
  nothing — the person types it themselves, as before, and the answer says so:
  `"entered": false, "codeRefused": "secret_reason"` with `--json`, and in text
  mode an error (`no code was taken`). After the code is in, look at the
  screen: the site says whether it took it, and a site that shows the digits it
  was given shows them in what you read. The last step (a payment, a submit you
  may not press) is still the person's. While the card stands the field shows
  what the person types, so a picture of the whole display (`screenshot`,
  `zoom`, `compare`, a desktop `observe` that keeps its picture) and a look that
  names ZeroCode itself (`--app ZeroCode`, one of its windows) answer
  `person_asked`; every other look answers.
- **Recipes** (`recipe-save --name <n> [--note …]`, `recipe-list`,
  `recipe-show --name <n>`, `recipe-run --name <n>`): when a procedure
  worked, save it — this session's steps become a document (a command line
  per step, the person's points called out, what you typed a named
  `{{text-N}}`) a person can read and edit, kept with the artifacts. Verify
  with `wait-for` before saving: a recipe keeps its checks, and a replay
  judges them.
  - `recipe-show` answers the recipe's `params`; `recipe-run --name <n>
    --params '{"text-3":"…"}'` walks every step in one call. A walk that
    ends answers ok; one that stops is refused `recipe_stopped` with the
    report — `result.stop.kind` says why and `result.next` where to go on:
    `persons_turn` / `persons_last_step` (the person's step; nothing was
    pressed), `needs_a_look` (a step names the saved screen's element,
    window or `pid:`: do it on this screen), `check_failed`, `step_failed`
    (refused, or its work did not happen — a `run` that failed, a `launch`
    with no window, a `quit` that did not quit), `nothing_changed` (a
    click changed nothing about its point), `person_moved` (a hand moved
    the pointer during a recipe that borrows the shared desktop —
    `handWatched: false` means an isolated recipe or an unreadable pointer),
    `budget` (the next step would outlast the call;
    nothing is cut short), `stopped`, `money_step` (the step marked
    `— money`: a `dry` Flow rehearses up to it and never presses it; a
    `guarded` Flow presses it only after the person confirms the
    transaction the card names — `recipe-run --name <n> --start <next>
    --confirm <txn> --params …`; the Flow's ledger beside the recipe keeps
    every transaction, and one already acting or acted is refused
    `flow_txn_seen`). Do that step with a lone command (or wait for the
    person), look, then `recipe-run --name <n> --start <next>`.
    If the answer never comes (the call timed out), `evidence --json` lists
    the steps it ran — resume after the last one, never from the start.
  - A line is one command in the grammar the reader splits (quote a word
    with spaces); delete a line to drop a step. `--start` counts lines from
    1, not the numbers written in front of them. A `{{name}}` fills a
    flag's value, never a flag; a saved line never carries `--allow-self`.
  - A saved `zerocode-browser upload` names its files `$file_N` (`$file_N_2`
    for a step's second file), never their folder: give each with
    `--params '{"file_3":"/abs/path"}'`, and a walk that reaches one with no
    value stops as `persons_turn` — never guess a path. A recipe saved before
    this keeps its path and still runs; saving again turns it into `$file_N`.
    The evidence log keeps an upload's file names, sizes and count only.
- **Memory across a cut context**: `status --json` carries `state` — the
  last verb, whether it worked, failures in a row, recent actions — and the
  session's evidence folder holds `state.json` and `steps.jsonl`. Read them
  before you continue a task you do not remember starting.
