# Landing: closing work, hand-ins by the coordinator, decision gates

Read this when work is neither done nor failed (folded, handed over,
outdated), when the coordinator did a task by hand or took one over, when
you write landing records after a merge, or when a decision has to hold a
task. The text was moved here word for word from the former single-file
guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## Closing work that is neither done nor failed

Work folded into another task, handed to the other run, or overtaken by events
is not a completion and not a failure — it is `closed`, with the reason and the
target that make it a fact. The coordinator writes it with `task-update`, and
nothing else writes it:

```
task-update --task <id> --status closed --closed-as folded --into <task>
task-update --task <id> --status closed --closed-as handed-over --to <run-id>
task-update --task <id> --status closed --closed-as outdated --why '<one line>'
```

Each form is refused without its own flag, a fold into a task the run does not
hold, into itself or in a circle is refused, and so is closing a task a worker
is carrying (stop the attempt first) or one with a decision standing. A closed
task leaves `task-list --open`, counts as neither completed nor failed
(`run-show` counts `closed` on its own), and the board reads 닫힘 with the
reason (접힘 → t-…, 넘김 → run-…, 낡음). A task that depends on a folded one
waits for the task it was folded into; one that depends on a handed-over or
outdated task lists it under `blockedBy`, because nothing in this run can
produce it. `--status ready` (or any other status) puts a closed task back to
work and drops its reason.

`task-settle [--days <n>]` lists, and only lists, two piles of old work: the
open tasks (pending, ready, blocked) whose every attempt ended handing nothing
in and that nobody has touched for a week (the default is `SETTLE_QUIET_DAYS`),
and — apart, as `unreviewableCompleted` — the completed tasks that can never
take a review record. `task-settle --apply --retry-request settle-<run>` closes
the first pile `outdated`, from the coordinator seat only; a completed task is
never changed by it. Run it without `--apply` first and read both counts.

The board does not wait on those completed tasks either. A completed task whose
every attempt ended handing nothing in has no source a review could name —
`task-update` refuses any review of it — so `task-list` carries
`review.unreviewable: true` on its row, and the board, the worker rows and the
sidebar read 완료 — 검토 기록 없음 instead of 검증 대기 from the moment it is so,
without the quiet week `task-settle` waits. It is the ledger's reading of the
attempts each time it is asked and is never written down: a new attempt that
hands something in takes it back, and a task with a review written, a decision
standing or an attempt still open is never read this way.

### Work the coordinator took over or did by hand

A review names an attempt and the source it handed in, so work a worker left
without a hand-in — or work the coordinator did itself — needs an attempt of the
coordinator's own before it can be reviewed. Three verbs, all from the live
coordinator seat (a worker's pane is refused):

- `task-take --task <id> [--from <attempt>] [--why '<line>'] --retry-request take-<id>`
  opens an attempt carried by the seat (its `workerId` reads `coordinator@g<generation>`).
  A worker still carrying the task is refused — `worker-stop` it first. An attempt an
  earlier sitting of the seat took (a window restart, a `run-takeover`) is ended by the
  take. A task with no attempt at all takes one too. A closed task is refused; use
  `task-backfill` for closed work that landed.
- `hand-in --task <id> --head <commit> --body-file <file> [--base main]
  [--checkout <path>] --retry-request handin-<id>` hands one commit in on the seat's own
  open attempt, once. The window asks git in `--checkout` (an absolute path; default: the
  pane's project) that the commit exists — and, with `--base`, that the ref holds it —
  before the ledger writes it; a commit git does not know, or one the base does not hold,
  is refused and nothing is written. The attempt's source is the full hash git resolved; the letter it
  leaves is a `worker_done` from the run's address, in the log and in no inbox.
- Then review it exactly as any other: `task-update --task <id> --attempt <the take's
  dispatchId> --source <the hand-in's source> --result '{"verified":true,…}'`.

Once a stack is on main, write its landing on every member in the same turn:
`task-update --members t-1,t-2 --result '{"merged":true,"mergeHead":"<sha>"}'
--retry-request landed-<sha>`, and after the release
`--result '{"deployed":true,"release":"v1.2.3"}'`. Each member's record is bound
to the attempt and source its verified review names; a member with no verified
review, or whose attempt handed nothing in, refuses the whole call (a line each)
and nothing is written.

`task-backfill [--base main] [--tags 'v*'] [--checkout <path>]` lists the completed and
closed tasks no review stands on whose result names a commit (`mergeHead`, `merged`,
`landed.mergeHead`, `commits[]` …), with what git proves for each: `merged` when the base
holds the commit, `deployed` when a release tag holds it (the tag is named), `verified`
only when the result names a review task whose own review passed — otherwise
`"verified":"unknown"`, never a pass nobody recorded — and, for every task it cannot
write, why (no commit named, not in the base, git does not know it, an attempt handed
something in). Read the list first; `--apply --retry-request backfill-<run>` writes the
listed rows (attempt, hand-in, review) from the coordinator seat, one task's failure
never stopping the others. The original result is kept: its keys stay, a review key it
held moves under `backfill.replaced`, and `backfill.by` reads `task-backfill`.

## Decision gates

When a slice reaches a decision that is a person's or the coordinator's to
make — an API shape, a destructive step, a tradeoff the spec left open — hold
the work instead of guessing. `gate-create --task <id> --question-file <file> --options '["a","b"]' --retry-request gate-<task>` blocks the task:
it leaves `task-list --ready`, `worker-start` refuses it, and it stays held
until `gate-resolve --gate <id> --resolution-file <file> --retry-request
answer-<gate>` frees it. `gate-list` shows what is standing and what was
decided. A resolved gate is not re-resolved — the first answer stands, and a
different second one is refused rather than written over it.

Workers do not open gates. A worker that hits a decision sends it up —
`send --type decision_gate --body-file <file>` — and stops; whether the
question deserves to hold the DAG is the coordinator's call, made from the
coordinator's own pane.
