# The ledger's news: what each notice means and what you do

Read this when a notice arrives (`went_quiet`, `worker_died`,
`quota_walled`, a `resumed` receipt, `classifier_declined`) or a pane was
touched by the person. These are the rest of the rules list that starts in
`SKILL.md`; the text was moved here word for word from the former
single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## The rest of the rules

- **Silence is not failure.** An empty `check` (`{"count":0}`) and a `--wait`
  that times out are both checkpoints. Real coding tasks run fifteen to sixty
  minutes without a word. Do not restart a quiet worker; read its screen.
  The ledger says the same about its own news: a `went_quiet` message — a
  worker whose live pane stayed idle past the stall grace, or one that
  `never_spoke` inside its readiness window — is information, not a verdict.
  Turn boundaries remain audit rows and do not notify by themselves. Nothing
  was settled; reading the screen is still the next move.
- **A death is not a silence.** `worker_died` is the one piece of the
  ledger's own news that IS a settlement: the seat carrying an open dispatch
  is gone, the attempt is closed, and the task's status and remaining
  attempts are already written. It arrives once, and it carries the
  `dispatchId` that `worker-start --retry-of` takes. Deciding whether to
  retry is yours — a death caused by a loop only dies again — but you no
  longer have to go and look to learn that there was one.
- **A wall is not a silence either.** `quota_walled` is the ledger's news
  that a quiet worker stopped at its provider's quota wall — written only
  on TWO witnesses, the agent's own words (a measured marker on its screen
  or in its transcript: codex's `usage_limit_exceeded`, claude's "You've
  hit your session limit", zo's "usage limit … resets in") AND the
  provider's fresh number at the wall off the window's usage cache. A
  screen line alone is a line; a number alone says nothing about that
  pane. It arrives once per wall, carries the `dispatchId`, the
  provider's number and reset, the `checkout` the work sits in and the
  words that were seen, and settles NOTHING — the attempt is open and the
  task carried until you (or the handover beat) say `worker-stop`. A pane
  the person took over earns no such news. A wall stands until its reset
  and three minutes after it (the stall grace — Claude Code waits out its
  own reset and types its own continuation about a minute after it), or
  six hours from the news when the reset is unknown or further off (a
  weekly window). While it stands the worker's silence is the wall's;
  after it, the silence is `went_quiet` news again, and a wall in the
  next window is `quota_walled` news again.
- **A dropped response is not a silence either — if you say so.** A worker
  whose own transcript ends on a transient API error (claude's
  `server_error` — "The response stopped arriving", "529 Overloaded",
  "Connection lost mid-response"; codex's `server_overloaded` or "stream
  disconnected before completion") sits at an empty composer until somebody
  types. Declare `handover-policy --on-transient-error resume` (alone, or
  beside `--on-quota-wall`; one `handover-policy` names the whole order,
  `run-show.handover.onTransientError` reads it back, `--off` puts it down)
  and the beat types one continuation into that worker's composer — only
  when the error is the conversation's last word, only at a composer at rest
  (never a pane the person took over, one holding its own question, or one a
  person's hand interrupted), and never at a quota wall, whose road wins.
  Each try leaves a `resumed` receipt in your inbox — `{workerId,
  dispatchId, marker: {source, line, key}, attempt, attemptsMax, status}` —
  `submitted` when the provider reported the prompt going in, `not_submitted`
  with `typed` saying whether the words may be on the line, `interrupted` if
  the window restarted mid-try. One error is typed at once unless the door
  typed nothing; tries stand the stall grace apart; one attempt is resumed
  at most three times, and the last says `ceilingReached` — after it the
  silence is `went_quiet` news again and the resume is yours.
- **A classifier's decline is not a silence either.** The provider's safety
  classifier can decline a request (`stop_reason: "refusal"`, with a
  category: `cyber`, `bio`, `frontier_llm`, `reasoning_extraction`, …) —
  Claude Code says "<Model>'s safeguards flagged this message". Fable 5.x
  and Opus 5/5.5 carry these classifiers; Opus 4.8 does not. A summoned
  Claude worker launched with no model at all — no `--model`, and no row of
  the summon profiles to fill one — is launched so its own CLI
  continues the declined turn on the model the provider routes the category
  to (`cyber` → Opus 4.8) instead of pausing — the flag rides that worker's
  launch only; the person's settings, their panes and yours are untouched.
  A worker summoned with `--model` is pinned, and so is one launched on its
  profile row's model when you left the dial to Jev: its CLI is launched with
  that switch OFF (zo: `--classifier-fallback off`), whatever the person's
  file says, because the model it launched on is binding and the CLI's own route
  is neither that model nor a rung you declared; where its task goes is
  `handover-policy --on-classifier-decline`, exactly as declared, or your
  own hand. A worker that still stops is `classifier_declined` news —
  written only on TWO witnesses: the sentence on its screen AND its
  transcript's last record saying the same, the category read off THAT
  record's own request (never an earlier one's); or, for the pause dialog
  (which writes no record until a key answers it), the dialog in the CLI's
  own layout on screen AND the pane quiet ten minutes, longer than any
  dialog a person answered here — and that one is DIAGNOSTIC news
  (`screenOnly: true`, rung `notify`): a screen's words can be quoted by a
  tool result, so a screen alone ends no worker and walks no handover,
  whatever you declared; read the pane and hand it over yourself. The news
  arrives once per record — a dialog once per attempt, and each decline
  record the pane stands at after it once more, whether its hook came to
  rest or still says `working` — with the `category`, whether the provider
  routes it anywhere (`routed`), the `rung` it stands on and the
  `dispatchId`, and settles nothing. Every switch of model a worker's CLI
  made for a decline is a `model_deviated` row — the bound model, `from` →
  `to`, the category, how long the switch lasts (Claude Code keeps it for
  the rest of that conversation) — written once, for the attempt it was
  made in and never for the next task the same pane is handed, and kept
  until the ledger can hold it — because the model you summoned with is
  binding and leaving it, however well, is yours to read.
- **A silence the markers cannot name may be asked about, and only written
  down.** With `smart.stallCause` at `shadow` or `auto` in zo's
  `settings.json` and the worker's checkout consented under
  `smart.jev.workspaces`, the beat puts each such silence to Jev once — the
  pane's screen tail and its transcript tail, through the Jev door — and
  appends a row to `jev/stall-cause.jsonl` in zo's config home: the cause it
  chose (`transient_api_error`, `quota_wall`, `auth_failure`,
  `classifier_decline`, `waiting_on_own_cli_question`,
  `finished_without_report`, `long_running_tool`, `human_took_over`,
  `unknown`) and, later, what you did next (`mail`, `resumed`,
  `worker_done`, `worker_stop`, `worker_died`, or `none` within two hours).
  Your inbox does not change and nothing is typed: `went_quiet` is still the
  news, and reading the pane is still yours.
- **Say when you cannot tell.** If you do not know whether a worker finished,
  say you do not know. A report that guesses is worse than one that admits the
  gap, because the coordinator acts on it.
- **A pane the person touched is theirs.** Real keys in a worker's pane take
  it over: from then on `worker-stop`, `worker-release` and `dispatch` refuse
  — the refusal names the road left — and `worker-abandon`, which touches
  nothing, is how you stop tracking it. Reports from that pane still land,
  and hand nothing back — the person may still be sitting there. An Enter
  that only sent words the window placed (a resume nudge, a mail pointer)
  takes nothing over. `worker-return --worker <id>` hands a pane back by
  name, and is refused while the person's own words sit unsent on its line.
