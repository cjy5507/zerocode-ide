# Handing work to another agent: run-auto, quota walls, classifier declines

Read this when a worker is at its provider's quota wall or was declined by
a safety classifier and you want the work to go on elsewhere, or when you
want the run to keep itself fed with `run-auto`. The text was moved here
word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## Placement, `run-auto`, walls and declines

**You choose the placement.** By default nothing decides for you how many
workers to start or where: a run is a namespace and an inbox, and it summons
nobody until you say so. Start the number you can actually supervise.

If you DO want it to keep itself fed, that is `run-auto --agent <name> --max
<n>` — a standing order that summons at most one worker per beat, up to `n` at
once, taking the oldest ready task. It stops when there is no ready work and
stands down on a restart, and every worker it starts is the same
`worker-start` you would have typed. `run-auto --off` ends it.

**A quota wall is handed over only under a declared order.** By default a
walled worker is `quota_walled` news and the hand recipe is yours: commit
its WIP in its checkout, `worker-stop --worker <id> --reason quota-wall`,
then `worker-start --agent <alt> --task <same> --retry-of <dispatchId>
--inherit-checkout`. If you want the beat to walk that road for you, say
the alternative yourself — `--on-quota-wall <alt>` on the summons (that
worker only), or `handover-policy --on-quota-wall <agent[:model[:effort]]>
[--wip-commit]` on the run (every worker of the run; the summons' own word
wins; `run-show.handover` reads it back; `handover-policy --off` puts it
down). The order is refused whole, by name, when the alternative cannot be
summoned. When armed and a `quota_walled` witness arrives, the beat walks
ONE effect outside the ledger's locks, in this order because `--retry-of`
follows an ENDED attempt only: ① a WIP commit in the worker's checkout —
`wip(handover): <worker> stopped at <provider> wall, resets <hh:mm>` — made
only under `--wip-commit` and only when the tree is dirty (a clean tree is
"skipped", a commit git refuses ABORTS the handover with news and the
worker keeps its pane); ② `worker-stop --reason quota-wall`; ③ `worker-start
--agent <alt…> --task <same> --retry-of <dispatchId> --inherit-checkout`,
with a handover paragraph at the head of the replacement's briefing (who
stopped, at which wall, where the tree is, the WIP sha, "continue — do not
start over", and — read from the walled worker's transcript before ② closed
its pane — its last words and latest tool calls, fenced as untrusted data
and capped at 2 KiB, with the transcript's path); ④ a `handover` receipt in your inbox — `{from, to, steps:
[{name, ok, detail}], status}` — delivered when the walk settles, `done`
with the replacement's `workerId`/`dispatchId` or `failed` at the step that
refused (an alternative that is itself at the wall fails step ③ by the
gate's own sentence, with the attempt already ended and the task `ready`
for you to place). Every step is the same argv a person would type, through
the one door, from your coordinator seat. One task is handed over at most
twice (`QUOTA_POLICY.handover_max`); past that a wall is news only. A pane
the person took over is never handed over. If the window restarts mid-walk
the receipt arrives `interrupted`, naming the last step that walked, and no
later beat resumes it — read the steps and finish or undo by hand.

**The same conversation comes before a different model.** `--on-quota-wall`
also takes the closed word `wait` — alone, or beside the alternative as
`wait,<agent[:model[:effort]]>` in either order (a word nobody measured is
refused by name). The order is walked as a ladder, the wait first: while
the wall's reset (named by a fresh gauge when the wall was witnessed) and
the three minutes after it have not passed, nothing is handed over. Claude
Code waits out its own reset and continues the same conversation about a
minute after it — every wall on this machine did — and a handover walked at
the wall would have ended that conversation for a new one. Once the wall
stops standing, the handover walks as before, if the wall is witnessed
again. A wall whose reset is unknown or more than six hours away (a weekly
window) is not waited for: it is handed over at once. `wait` alone on a
summons is that worker's whole order, so the run's alternative never
reaches it — the way to keep a worker pinned to its model and effort.
`run-show.handover.ladder` reads the rungs back in walking order; the
`quota_walled` news carries `ladder` and `wait: {standsUntilMs}` (or
`{skipped: <why>}`), and a `handover` receipt carries `rung: "handover"`.
Under `wait` the beat also asks the window's usage gauge for a fresh
reading from the reset on (never forced — the status bar's own floor and
backoff hold), and if the worker is still stopped at its wall once the wall
stops standing — its own words still at the wall, the provider's number
read after the reset under it — you get ONE `went_quiet` notice with
`reason: "quota_lifted"` and `rung: "wait"`, the `wallId`, and the `gauge`
that says so: its own continuation did not come (a CLI that does not wait,
or a countdown somebody cancelled), so wake it with a line of mail or hand
it over. Nothing is typed for you. With no reading after the reset within
five minutes, or without `wait`, the silence is ordinary `went_quiet` news.

**A decline goes to the provider's route first, and to another worker only
under a declared order.** The ladder is one table, walked in order: ① an
UNPINNED worker's own CLI continues the declined turn on the category's
route — no wait, nothing typed, and a `model_deviated` row says so (a
worker you pinned with `--model` never takes this rung: its CLI is told
not to switch, and a declaration does not turn that switch back on); ② for
a routed category (`cyber`, `bio`, `frontier_llm`) that still stopped, the
handover you declared with `handover-policy --on-classifier-decline
<agent[:model[:effort]]> [--wip-commit]` (a part you leave out is the
declined worker's own) — the same three steps as a wall's, with `worker-stop
--reason classifier-decline`, and a `handover` receipt that names the
deviation; the walk ends the worker only on the decline it was planned
for — the same transcript record, in the same routed category, re-read at
every step and last inside the stop itself — and a decline that reads
differently there (its category gone, another request's) settles nothing
and is planned again from what the pane shows next; ③ the notice, for
everything else — an unrouted category, no order declared, and every
decline the screen alone witnessed. Not a retry on the same model first:
here the request after a decline went through on the same model 6 times in
15 and on the route 15 times in 15, and the provider's own guide says a
refused request re-sent unchanged usually earns another refusal. A
category the provider routes nowhere (`reasoning_extraction`, and a decline
with no category) is news only: nobody is handed the declined request. Do
not rewrite a brief to slip past the classifier, and do not grep briefs for
"risky" words — neither predicted a decline here (a brief carrying such
words was declined 2 times in 46, one without 1 in 106). Read a declined
pane's words (`worker-read`); never paste a picture of the declined screen
into a conversation — on this machine a refused message that carried a
screenshot was declined again for as long as the picture stayed in the
conversation. zo takes the same ladder in-process under
`smart.classifierFallback` (`off`, `ask` — the default: it asks before a
turn continues on the route, and a question nobody answers — the prompt
ceiling passed, or nobody at the keyboard to ask — is NOT a yes: the turn
stays on the chosen model and says so — or `auto`), announces every switch,
and asks before it sends a declined request's images again; a summoned zo
worker runs `auto` when you pinned no model, `off` when you did.
