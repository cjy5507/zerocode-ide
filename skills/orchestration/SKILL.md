---
name: orchestration
description: >-
  Use ZeroCode orchestration to coordinate work across several coding agents in
  this IDE: opening a run, writing tasks down, summoning worker agents into
  their own panes, watching them while they run, and collecting their reports
  over a real message channel. Load it for "coordinate", "split this across
  agents", "run these in parallel", "supervise", "worker", "child agent",
  "summon", "orchestrate", "merge their results" — and for any request to have
  one agent start or drive another. Do NOT load it for a plain ownership
  handoff — "hand off", "handover", "hand this to another agent", "another
  worktree" — when nobody asked to supervise or wait for results: that is one
  window action (새 워크트리), not a coordination problem. Do not load it for
  ordinary terminal or shell work either.
---

# ZeroCode Orchestration

You are one agent among several working on the same codebase, inside an IDE
that already owns the things coordination needs. This skill is about using
them, not about re-inventing them in prose.

**What this product gives you.** The long form is in `workers.md`. Read this
first — advice that names nothing in the product is advice for some other
product.

- **Worktrees are real.** `zerocode-orchestrator` owns them: a workspace is a
  real `git worktree`, made and removed by the window, and `worker-start
  --worktree` starts the worker in a fresh one. A worker that changes git
  state or builds always gets `--worktree`; a reader that only reads gets none
  and is told not to change git state and not to build. Count the disk before
  you summon: one worktree that runs this repo's gate measured about 10 GB, and
  a full disk stops the ledger itself until the window restarts.
- **Panes are where agents live.** A worker runs in a terminal pane of this
  window, and the sidebar draws the parent/child lineage.
- **The window IS a channel.** There is a run, there are tasks, dispatch ids
  and a `worker_done` you can send. You need no file to hand a result back.

## Reference files — open one when its row says so

They sit in this folder. Each holds text moved word for word from the former
single-file guide; nothing was cut. They are not read at boot, only when the
moment calls for them.

| File | Read it when |
| --- | --- |
| `workers.md` | you are about to summon: worktree or shared tree, the disk count, zo's delegation tools, choosing agent, model and effort, headroom, `--agent auto` |
| `verbs.md` | you write the first verbs of a run, read a `worker_done` payload, answer an `ask`, use `delegate`, a `send` group, `dispatch --inject` or a remote worker |
| `recovery.md` | you are about to stop or reap a pane, a seat is held by somebody else, a worker died in its checkout, or the window restarted |
| `news.md` | a notice arrives (`went_quiet`, `worker_died`, `quota_walled`, `resumed`, `classifier_declined`) or a pane was touched by the person |
| `handover.md` | a worker is at a quota wall or was declined by a classifier and the work should go on elsewhere; `run-auto` |
| `landing.md` | closing a task that is neither done nor failed, `task-take`, `hand-in`, `task-backfill`, landing records after a merge, decision gates |
| `briefing.md` | you write a worker's briefing: the acceptance first, `zerocode-find`, the template to copy, how reports read |

## Rules at a glance

Each line is the short form of a paragraph below or in a reference file.

1. **A verb's text goes in as a file** (`--prompt-file`, `--body-file`,
   `--spec-file`, `--question-file`, `--resolution-file`, `--reason-file`),
   written with a quoted heredoc. No backtick, `$(…)` or `$VAR` inside an inline
   double-quoted text: the shell runs it before the verb sees a word.
2. **Every verb that changes anything needs `--retry-request <name>`**, the
   same name on a retry and a new one for a new request.
3. **Ack a delivery, then wait.** `check --ack <deliveryId>` before the next
   `check --wait`, or the same delivery comes back and real news stays hidden.
4. **`worker_done` says `ok`, and carries the commit as a top-level `"head"`
   key** (the 40-character hash) when the work is code. The coordinator
   reviews the commit named there.
5. **A review briefing requires `"reportKind":"review"`** in the worker's
   `worker_done` payload; the word alone tells a reviewed head from code
   waiting to land.
6. **Builds take turns.** A worker starts no rustc, clippy or test run before
   the coordinator's letter says cargo is free; after it, only the targeted
   tests and the gate the briefing names. The full gate is the coordinator's.
7. **A worker never pushes a branch and never opens a pull request.** Landing
   and pushing belong to the coordinator.
8. **Read your mail before `worker_done`.** Run `check --peek` first: a
   letter that arrived while you worked can change the scope, and the
   hand-in closes the attempt.
9. **Ask your coordinator, not the person.** A prompt nobody watches blocks
   the pane. Use `ask` and wait for its answer.

## Reaching it

Every verb is `zerocode-orc <verb>`, and the list of verbs is:

```
zerocode-orc help
```

**The user's agent, model, and effort choices are binding.** Resolve every
requested identity through the ZeroCode agent catalog and pass the exact
requested values to `worker-start --agent <requested-id>` with `--model` and
`--effort` where specified. Do not replace them with the provider running the
coordinator, another model, a provider-native subagent/team, cross-session
messaging, or a background pipe. When several agents are named, preserve each
identity. Read the returned worker record and confirm its agent and launch
settings before saying it started; if that exact launch is unsupported or
unavailable, report the blocker and do not fall back. A name mentioned only
for discussion or comparison is not a launch request.

**When the person named no model or effort, leave `--model` and `--effort`
out** — the ledger asks Jev, and a dial you pin always wins. How that works,
and how to spread a round over the agents that are installed, is in
`workers.md`. Still say the classification and the assignment out loud, one
line each, where the person reads the round's plan.

**Whoever implemented a slice does not verify it.** Give the check to a
different agent, and give it the power to come back with "no". A verifier that
reads the implementer's own report instead of the code confirms the mistake.

**Ask the binary, not this file.** The verb table is deliberately not written
here, so it can never drift from the program that will actually run your
commands. If `zerocode-orc` is not on your `PATH`, this pane is not part of a
run and you are not orchestrating — fall back to the handoff file below.

The index below names the verbs this guide teaches, one line each. It is not
the table: flags, arguments and every verb not named here are in `help`.

- Runs and seats: `run-create` opens a run, binds you and sits you in its
  coordinator seat. `run-use` binds you to another run. `run-takeover`
  replaces a live seat. `run-current` says which run you are bound to and
  whether you are seated. `run-show` counts one run out. `run-list` lists the
  runs this window holds. `run-auto` keeps n workers fed from the ready tasks.
  `handover-policy` declares what the run does at a quota wall, a transient
  error or a classifier decline.
- Tasks: `task-create` writes a slice down. `task-list` shows what is written
  down. `task-update` corrects the record, from the coordinator seat.
  `task-settle` lists old open work and closes it `outdated` with `--apply`.
  `task-take` opens a coordinator attempt on work done by hand. `hand-in`
  hands a commit in on that attempt. `task-backfill` writes the records of
  work that landed without one.
- Gates: `gate-create` holds a task on a decision. `gate-resolve` answers it.
  `gate-list` shows what stands and what was decided.
- Workers: `agent-list` says which agents this machine has and their
  headroom. `worker-start` summons an agent into a pane. `delegate` writes the
  task, summons the worker and leaves it a letter, in one step. `worker-list`
  and `worker-show` read workers. `worker-read` reads a worker's screen and
  `worker-transcript` its conversation. `worker-stop` ends an attempt.
  `worker-abandon` stops tracking a pane. `worker-return` hands a pane the
  person took back to the ledger. `worker-release` archives a screen and
  retires the terminal. `dispatch` hands a written task to a pane you
  summoned, and `dispatch-show` reads the latest attempt back.
- Mail: `check` reads your inbox. `send` writes a letter to an address or a
  group. `reply` answers in thread. `ask` is a question a blocked worker waits
  on. `inbox` is an audit of the project's mail. `reset` is recovery only.
  `help` is the verb table.
- Federation, for a worker on another machine: `federation-invite`,
  `federation-join`, `federation-servers` and `federation-forget`.

**Every verb that CHANGES anything needs `--retry-request <name>`.** Reads do
not (`run-list`, `task-list`, `worker-list`, a plain `check`); a `check --ack`
does, because it spends a delivery. The name is yours to choose and its whole
job is to be the SAME on a retry: the effect happens before the record of it
reaches disk, so a verb can honestly answer "not durable" after the pane really
opened. Retrying that answer with the same name replays it; retrying it without
one cuts a second pane. Use a new name for a new request — a repeated name with
different arguments is refused rather than guessed at.

**A delivery you never acked is the mail you keep getting.** `check` hands you
a delivery and waits to be told it arrived; until you `--ack` it, every later
`check` answers with that same delivery — including `check --wait`, which comes
back instantly instead of waiting, holding a message you have already read.
Two ways that misleads you: an old message looks like news, and real news
queued behind it stays invisible. Ack each delivery by its id before you wait
again. And when the ledger answers with an ERROR rather than a list, that is
not an empty run — a watcher that parses the error as "no workers" will report
that everyone finished at the moment the ledger died. Read the failure as a
failure.

**Give a verb its text as a file, never inside double quotes.** `--prompt`,
`--body`, `--spec`, `--question`, `--resolution` and `--reason` each have a
file form — `--prompt-file`, `--body-file`, `--spec-file`, `--question-file`,
`--resolution-file`, `--reason-file` — that takes the ABSOLUTE path of a UTF-8
file and reads it as it lies, with no shell in between. Do not write a
backtick, `$(…)` or `$VAR` inside an inline double-quoted text: the shell runs
or replaces it before `zerocode-orc` sees a word. Twice (2026-10-28) a `pkill`
inside a briefing ran that way and took the window's panes down. Write the
text with a quoted heredoc and name the file:

```sh
cat > /tmp/slice-1.md <<'EOF'
Any characters at all: backticks, $(not run), $HOME, "quotes", 한글.
EOF
zerocode-orc worker-start --agent <name> --task <id> --prompt-file /tmp/slice-1.md --retry-request start-<id>
```

A text and its file form together are refused, and so is a file that is
missing, empty, a link or not UTF-8. One plain word inline (`--resolution harvest`,
`--reason quota-wall`) is safe; an inline text with a backtick or `$(` still goes through, with
a warning line on stderr. Every example below that takes text is written in the
file form: `--spec-file <file>` stands for a file written like this one.

The shape of a session, one line each (the full text, with every trap, is in
`verbs.md`):

1. `run-create` opens a run and sits you in its seat.
2. `task-create` writes each slice down BEFORE you summon anyone.
3. `worker-start` cuts a pane and starts the agent in it — and say the slice
   in `--prompt-file` every time: `--task` alone opens an empty composer.
4. `check --wait`, in the background, reads your inbox; ack what you act on.
5. `worker-read` shows what a worker is doing rather than what it said.

**What survives a window restart is your agent's own conversation.** Receipts
— and your run binding — are filed under your session, not under the pane you
sit in: resume the same conversation and your old `--retry-request` names
still replay, and `run-current` still answers your run. Two callers have no
such thread to hold: an agent whose vendor never reports a session (`cursor`,
`amp`, `copilot`, `zo`, and every catalog agent beyond the hook set) lives on
the launch this window gave it, and a pane that comes back as a NEW
conversation is a new caller — for both, yesterday's names will not replay,
they will RUN. So after any restart you are not sure your conversation
survived, look before you re-run a mutation: `run-list`, `task-list` and
`worker-list` cost nothing and are the difference between resuming a run and
starting a second copy of it.

## Five rules that are not optional

These are the ones that cost you a run when you get them wrong.

- **Acknowledge what you read.** `check` hands you a batch and hands you the
  SAME batch again until you `check --ack <deliveryId> --retry-request
  ack-<deliveryId>`. That is what makes a
  coordinator recoverable — but it also means an unacknowledged batch is an
  infinite loop. Ack after you have acted, never before.
- **Report finishing; do not announce it twice.** As a worker, `send --type
  worker_done --retry-request done-<your task id> --body-file <file>` (the file holds
  `{"ok":true,"summary":"…"}`)
  closes your task and your dispatch. You do not name the task,
  the dispatch, or the address — the run already knows what you were carrying,
  and naming it yourself is how you close somebody else's work. Do not follow
  it with `task-update`.
- **Say whether you succeeded — you must, in so many words.** A `worker_done`
  carries a body (`--body-file`) of `{"ok":true,"summary":"…"}` or `{"ok":false}`, and
  a body with no readable verdict is REFUSED rather than read as a success. It
  used to be read as one, and that was the last road by which a failure got
  written down as a completion. A refusal costs you one more command and
  nothing else: your dispatch stays open, so answer again and say which it was.
  `"ok":false` is a failed attempt — the task goes back to ready and can be
  tried again, and three failures on one task ends it rather than spending a
  fourth agent on the same wall.
- **Nothing wakes a busy coordinator.** The pointer that says "you have N
  orchestration messages" is typed into a pane the window knows is IDLE —
  deliberately, because typing at a pane mid-turn would land in whatever its
  agent is composing. So a coordinator running a long turn of its own gets no
  signal at all, however many reports arrive. Two consequences, both learned
  the hard way. **Check between your own slices**, not only when you finish:
  a `check --peek` costs nothing and needs no retry name. And if you watch
  workers with a loop, **watch the SET of active workers change, never wait
  for the count to reach zero** — with three running, the first to finish
  moves nothing your loop is looking at, and its report sits unread behind
  any other queued notices.

The rest of the list is in `news.md`, and it is about the ledger's own news:
silence is not failure (`went_quiet` is information, read the screen), a death
is a settlement (`worker_died` carries the `dispatchId` a retry takes), a wall
is not a silence (`quota_walled`, written on two witnesses), a dropped
response and a classifier's decline each have their own notice, say when you
cannot tell, and a pane the person touched is theirs.

## When to orchestrate — and when not to

Split work only when the slices are genuinely independent: different files,
different subsystems, no shared state that both sides would edit. If two
slices would touch the same files, they are one slice. A task that fits in
one focused session should stay in one session — coordination has overhead,
and a split that saves no wall-clock time only adds it.

You choose the placement. A run summons nobody until you say so; `run-auto`
keeps it fed, and a quota wall or a classifier decline is handed over only
under an order you declared. Both are in `handover.md`.

If what you were asked for is "give this to another agent" and nobody asked
you to watch it or collect a result, you do not need this skill. Make a
workspace and start an agent in it; that is a window action.

## Splitting into child agents

As a coordinator, split a large task like this:

1. Write the slice list first — each slice with its goal, its files, and
   its done-condition. Check for overlaps before starting anything. Each
   slice becomes a `task-create`, so the list survives you.
2. Give each code-touching child `--worktree` on its `worker-start`.
   Parallel children in one checkout trample each other's diffs; one
   worktree per child keeps every diff reviewable on its own.
3. Prompt each child with only its slice — the goal, the files, the
   done-condition. A child that knows the whole plan starts editing outside
   its slice.
4. Have each child end with a `worker_done` carrying what changed and how it
   was verified. That report is the child's handoff back, and it arrives in
   your inbox rather than in a file you have to remember to read.

**Hand a child the constraint, not your number.** A threshold you carried
in from somewhere else — a width you measured in a different container, a
timeout you remember from another lane — is a guess wearing a measurement's
clothes. Say what has to be true and let the child measure where the change
actually lands; a child that comes back having refused your number, with the
number it measured instead, has done the job right.

**Never ask the person a question you could ask your coordinator.** A child
that opens a prompt nobody is watching blocks its own pane until somebody
happens to look at it. Send the blocker as `escalation` and stop; a stopped
child with a written reason is recoverable, a hung one is not.

## Closing work, decision gates, and the coordinator's own hand-ins

Work folded into another task, handed to the other run or overtaken by events
is `closed`, written by the coordinator with `task-update`. Work a worker left
without a hand-in, or the coordinator did itself, needs `task-take` and
`hand-in` before it can be reviewed. After a merge, write the landing on every
member of the stack in the same turn. A decision that is a person's to make
holds a task with `gate-create`; workers send a `decision_gate` letter and stop.
All of it is in `landing.md`.

## Worker gates and briefing template

For code changes in a Rust workspace, the worker gate must include
`cargo clippy --all-targets -- -D warnings` from the repository root, including
test targets across the workspace. Report each gate exit code without hiding it
behind a pipe. A package-only clippy run does not cover this gate.

How to write a worker's briefing — the acceptance first, `zerocode-find`, the
template to copy, and plain reports — is in `briefing.md`.

## Merging results

When the children are done, the coordinator reviews before merging:

- Read each report against its done-condition. A slice without its
  verification run is not done — rerun it, do not take its word.
- Merge one slice at a time and run the affected tests after each, so a
  break names the slice that caused it.
- Anything a child flagged as surprising gets read, not skimmed — surprises
  are where two slices turn out to have been one.

## Sequential chains

For work that cannot be parallel (a refactor that must land before the
feature that uses it), say so in the tasks themselves: `--deps` holds a task
at `pending` until every task it names has completed, and `task-list --ready`
is then the list of work that can actually start. The chain's whole value is
that each stage can be verified before the next begins — do not resolve a
dependency on an unverified stage.

## Handing off without a run

When there is no run — your context is ending, or `zerocode-orc` is not on
your `PATH` — write a handoff the next session can stand on. Put it in a file
the next agent will be pointed at (for example `HANDOFF.md` in the worktree
root), holding:

1. **Goal** — what the overall task is, in one or two sentences.
2. **State** — what is done and verified, with the commands you used to
   verify it. "Tests pass" names the test command; "implemented" names the
   files.
3. **Next** — the concrete next steps, most specific first. Name files,
   functions, and expected outcomes, not directions.
4. **Traps** — anything you learned the hard way: a test that is slow, a
   file that looks editable but is generated, a decision that was already
   made and should not be reopened.

Keep it under a page. A handoff longer than the work remaining is a sign the
work should not be handed off.
