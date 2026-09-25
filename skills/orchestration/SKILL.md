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

**What this product actually gives you.** Read this first — advice that names
nothing in the product is advice for some other product.

- **Worktrees are real, and a worker can be summoned into one.**
  `zerocode-orchestrator` owns that lifetime — a workspace is a real
  `git worktree`, made and removed by the window — and `worker-start
  --worktree` cuts a fresh one from the leader's repository and starts the
  worker THERE. Without the flag a worker lands in the tree you are already
  in. A leader outside any repository is refused the flag rather than quietly
  handed the shared tree.

  What decides the flag is not "is this analysis or implementation" but WHAT
  THE WORKER TOUCHES:

  - **Changes git state — commits, branches, stash, reset — always
    `--worktree`.** This is the one that bites. Exclusive file ranges do not
    protect it: a worker given a slice of files still runs `git checkout -b`
    in the tree it is standing in, and the shared checkout moves under
    everyone. That is a real accident from 2026-08-28, and the file ranges
    were exclusive when it happened.
  - **Builds — `cargo`, `npm` — always `--worktree`.** Not for isolation but
    for the lock and the disk: a build in the shared tree contends with your
    own on cargo's lock, and its `target/` is tens of gigabytes over a day.
    (Do NOT hand it a `CARGO_TARGET_DIR` of its own: cargo already puts
    `target/` inside the worktree, and `git worktree remove` takes it along.
    A target directory outside the worktree is an orphan nobody sweeps.)
  - **Count the disk in worktrees, not in workers, and count before you
    summon.** A worktree that runs this repo's gate measured about **10 GB**
    on 2026-08-29 — mostly `target/debug` — so two parallel builders want
    20 GB that nobody warned you about. Run `df` first and multiply — and
    the ledger now does the first half of that for you: a `--worktree`
    summons is refused, before anything is written, when the volume the
    ledger lives on holds less than one budget, and its receipt carries a
    `diskNotice` when the checkouts live workers hold could not all grow to
    a budget at once. The refusal is the ledger's call; the notice is yours,
    because a reader never builds. `null` there means the disk had nothing
    to say and "not measured" means the window could not look. Getting
    this wrong is not a failed build: filling the disk poisons the ledger
    runtime itself, and every verb keeps refusing with "the authority store
    is unavailable" long after you have freed the space. Only restarting the
    window brings it back. If your workers are VERIFYING rather than
    building, tell them so — the coordinator runs the gate, and a reader
    that never builds costs a fraction of a builder.
  - **Reads only — files, `grep`, docs, an analysis — no worktree.** Many
    readers in one tree are safe, and a tree cut for one buys nothing: a
    checkout, a `target/` that grows the moment anyone forgets the no-build
    rule, and a reclaim later. Say so in the briefing, though: tell it not to
    change git state and not to build, or it will do one of them for its own
    reasons and the shared tree is where it lands — and several agents
    building in one tree WOULD contend on cargo's lock, which is why the ban
    and the shared tree travel together. Tell it to keep its notes outside
    the repo (`/tmp`), so nothing it leaves behind is yours to clean.
- **Panes are where agents live.** A worker runs in a terminal pane of this
  window, and the sidebar draws the parent/child lineage — you can see which
  agent started which, and click into one to watch it work.
- **The window IS a channel.** There is a run, there are tasks, there are
  dispatch ids, and there is a `worker_done` you can send. You do not need a
  file to hand a result back, and you do not need a person to carry a message
  between two agents.

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

**When the user did not name one, choose by difficulty — and always name
it.** A slice that is real surgery on a large, well-tested file wants the
strongest model you have and a high effort; a mechanical slice does not, and
paying frontier prices for it wastes quota someone else needs. Say the choice
out loud in `--model`, because an omitted flag does not mean "cheap": it means
the agent's own CLI default, which on a configured machine can be the most
expensive model at its highest effort.

Classify before you summon, in one line each, and let the answer pick the
effort. What raises difficulty is not the size of the diff: it is how much of
the work is JUDGEMENT. Surgery on a large well-tested file, a race whose order
decides the answer, a design trade-off with a real cost on both sides — those
want your strongest and highest. An audit that is mostly `grep` and reading, a
survey comparing two roads, a mechanical follow-through — those do not, and
paying top effort for them takes quota from the slice that needed it. An
analysis is not automatically cheap and an implementation is not automatically
dear; a read-only slice can be the hardest thing in a round. Write the
classification where the person can see it, so a wrong call is theirs to
correct rather than something they have to infer from a bill.

**And spread the work across the agents that are actually installed.** A round
that sends every slice to one vendor burns that quota alone while the others
sit idle, and it also hands you one opinion where you could have had two — the
value of a second agent is that it reads the same code differently. Two of
this round's analyses each overturned a coordinator's hypothesis, which is the
whole argument.

Do it without being asked, and do it the same way every time:

1. **Ask what is here before you plan the round**, not at the summons. A name
   the catalog knows can be absent from this machine, and a summons for it
   cuts a pane and only then fails. Availability is the one thing about an
   agent a machine can honestly keep fresh — capability is not — and the
   ledger now answers it: `zerocode-orc agent-list` says, per agent, whether
   this machine has it (`installed: yes|no|unknown` — unknown means nobody
   looked, never "no") and whether it takes `--model` and `--effort`. It
   ranks nothing and recommends nothing, on purpose. Beside `installed`,
   `readiness` is what the window last observed of the agent's binary and
   login: `auth: authorized|unauthorized|unknown`, `binary`, `observedAtMs`,
   `ageMs`, and `evidence` naming the local witness that said so (an
   identity file, the keychain item, `gh auth status`). `unauthorized` is
   a summons that would open on a login screen — `worker-start` refuses it
   and names the same evidence; `unknown` is "nobody could look", never a
   verdict.
2. **Give the hardest slice to a different vendor than last round's hardest.**
   Rotate rather than rank. You are not deciding who is better; you are
   refusing to find out the hard way that you only ever asked one.
3. **Never let one vendor take a whole round while another is installed and
   idle.** A round of four with two vendors present is at worst three and one.
4. **Say the assignment out loud** — which agent, which slice, why — for the
   same reason you say the effort: so a bad split is visible while it can
   still be changed, instead of being inferred later from a bill.

A person naming an agent overrides all four, and their choice is binding.

**잔량으로 고르기 — choose by headroom, and let the ledger refuse a wall.**
`agent-list` answers `headroom` per installed agent — `usedPercent`,
`resetsAtMs`, `ageMs` — read off the window's own usage cache (the status
bar's numbers; nothing is fetched for the answer), and `null` where no gauge
was read: `null` is "nobody knows", never 0%. Read it before you assign a
round, the way you read `installed`: a slice sent to a provider at 95% is a
slice you will re-summon. `worker-start` reads the same cache before it
writes anything. At 90% it summons and says the number in the reply's
`quotaNotice`; at 97% on a snapshot under thirty minutes old it REFUSES
before the ledger moves — nothing reserved, the task still `ready` — and the
refusal names the installed agents that still have room and, when the reset
is under ten minutes away, says "ask again in N min". A snapshot older than
that warns instead of refusing: an old number does not block a summons the
provider may already have unblocked. Nothing is ever substituted unnamed. If
you want the wall handled for you, say the alternative yourself with
`--on-quota-wall <agent[:model[:effort]]>`: the summons lands on exactly
that agent, and the reply's `quotaNotice.redirected` names both halves. A
person's pinned agent is refused, never swapped.

**Do not maintain a list of models — a ranking, or even a plain roster.** The ledger passes
`--model` through unread, which is why it has never gone stale; a table of
which model is best rots the moment a provider ships a new one. What a machine
can honestly keep fresh is AVAILABILITY, not capability: whether a launch
starts, retries, or dies is already in the ledger's own record, so let it
answer "can this agent be summoned right now" and keep the judgement of who is
good at what where a person can correct it.

The test is not "is this a ranking" but "who updates it when a provider ships
something new". If the answer is a person, it is already stale — that is as
true of a bare list of ids as of a table saying which is best. Anything you
keep has to be DERIVED at the moment it is used: presence from `PATH`, the
launch flags from the agent's own configuration, the model from whatever the
person set as that CLI's default. A model id read out of this machine's
config does not rot, but it also cannot show you a model nobody here has run
yet, so it buys less than it looks like.

The ledger is that derived source, and it was there the whole time. Every
summons writes its agent, its model and its effort down, so `agent-list`
answers, per agent, what this ledger has launched it with (`launched`: each
`(model, effort)` pair and how many times) — no table to keep, growing by
itself every time anyone summons. A machine that had never heard of a provider's newest model
learns it the moment someone uses it once. Ask that before you ask a config
file: a CLI's config holds only its DEFAULT, which is why two models this
coordinator had used repeatedly were invisible to a search that stopped at
`~/.codex/config.toml`.

Read it beside `PATH` rather than instead of it — they answer different
questions, and the gap between them is information. An agent with launches
recorded and no binary today was removed; an installed one with no launches
has never been tried. Neither is "available" in the same sense, and flattening
them into one list throws away the difference the caller needs.

**Whoever implemented a slice does not verify it.** Give the check to a
different agent, and give it the power to come back with "no". A verifier that
reads the implementer's own report instead of the code confirms the mistake.

**A pane you kill takes its children with it.** Before you stop or reap any
pane, ask the ledger what that pane leads. A leader's exit dissolves its team,
and a leader's exit is NOT a fact about any child's pane: every child that
was carrying an open local dispatch becomes `orphaned` — alive as far as
anybody knows, still addressable, its attempt intact. The dispatch stays open,
the task stays `dispatched`, and its failure count does not move, whether or
not the row knows its checkout and whether or not a person had typed in it.
An orphan can still file `worker_done`, `ask` and `status` from its own pane,
and every one of those lands with the run's CURRENT coordinator seat. Only a
child carrying nothing is `abandoned`. `released` is written once the window
has PROVEN the pane gone — never on the strength of the leader leaving. The
ledger's notice tells the halves apart in separate fields rather than one
list, because the difference is the whole question.

What no longer follows is the panic. An orphan is waiting, not lost. When the
next coordinator sits (`run-use`, `run-takeover`, or a restored tab mounting)
every orphan whose pane is still standing is adopted where it is — `active`
again, `adoptedBy` naming the seat's generation — and keeps working in the
pane it always had. An orphan whose pane the window has CONFIRMED gone (the
reconciler's five beats, written on the row as `pane_missing_since_ms`) is
seated again in a fresh pane on the attempt it never lost, exactly as a
sleeper is — or, when it has no checkout to be seated in, retired then and
only then, its task handed back. An orphan still holding a pane is never
re-seated, whatever anybody believes about it: cutting a second pane for an
agent already running is how one session becomes two. If you must act before
the window has looked, read the pane first (`worker-read` reads another
leader's pane too), and only then decide.

**A run has ONE coordinator seat, and it is a ledger fact.** `run-create`
sits you. `run-use <run>` binds you and sits you only if the chair is empty
or its holder's pane is gone; a chair somebody holds answers `"seated":
false` with the holder and the one verb that replaces a holder:
`run-takeover --run <id> --from <seat> --reason <why> --retry-request
<name>`. `--from` names the holder as `run-use`/`run-current` spell it
(`team/pane`, or its bare pane). A takeover raises the seat's generation,
files a `handoff` receipt in the old seat's own pane inbox, and ends the old
seat's `check --wait` with a refusal that names you. Only the seat signs and
reads as `run:<id>` — a bound pane that is not the seat reads its own pane
inbox — so two coordinators can no longer share one address and read each
other's mail. `run-current` says whether you are `seated`. Do not take over a
seat whose holder is merely quiet; read its pane first.

**A worker that dies in a checkout of its own does not hand its task back.**
Three times in one day a worker committed its slice, its terminal exited
before `worker_done`, and the task went back to `ready` with the commits
sitting on the branch where the ledger could not see them — the next summons
would have done the slice again. Now the death leaves the task `blocked` by a
gate the ledger opened itself; the `worker_died` notice names it
(`heldByGate`) and the checkout. The window's reclaimer examines that
checkout on its beat: nothing to harvest (clean and landed, or the
coordinator's own tree) resolves the gate alone and the task is `ready`
again; unlanded commits or uncommitted changes are written into the gate's
question and sent to you once. Land the branch, then
`gate-resolve --gate <id> --resolution harvest` — or `dispatch again` to start
over. If the checkout is already gone the window cannot look; read the branch
yourself (`git log main..wt/<task>`) and resolve the gate by hand.

**After a window restart, two panes can hold the same session.** Both answer to
the same run, and the second one is not idle — it summons, it acks your
deliveries, it acts on results you are also acting on. Tell them apart by
`ZEROCODE_PANE_KEY` and the team id in their environment, not by the session
id, which is identical. And before reaping anything that looks stray, read its
working directory (`lsof -a -p <pid> -d cwd`): a pane sitting in a different
worktree may be a person's own session, not your leftover.

**Ask the binary, not this file.** The verb table is deliberately not written
here, so it can never drift from the program that will actually run your
commands. If `zerocode-orc` is not on your `PATH`, this pane is not part of a
run and you are not orchestrating — fall back to the handoff file below.

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

The shape of a session is always the same:

1. `run-create --name <run> --retry-request open-<run>` — open a run, bind
   YOU to it, and sit in its coordinator seat. The run you are working in
   follows your session, not the pane you happen to be sitting in: close the
   window, come back, and `run-current` still answers the run you were in.
   `run-use <run>` moves you; nothing else does, and a retry of an old request
   never moves you back. Binding and sitting are two facts: `run-use` answers
   `"seated": true` only when the chair was empty (see the seat rule above).
2. `task-create --spec '<slice>' --retry-request task-<run>-1` — write each
   slice down BEFORE summoning anyone. A task carries its own spec, and
   `--deps` holds a slice back until the slices it needs have finished.
   Write `--title` in English: checkout and branch names are ASCII-only, so
   Korean words fall away and a title led by `t-<id>` keeps only that id.
3. `worker-start --agent <name> --task <id> --prompt '<slice>' --retry-request
   start-<id>` — this cuts a real pane and starts that agent in it, with your
   prompt already on its command line. Any agent in the catalog: `claude`,
   `codex`, and the rest.
   **`--task` is not a prompt, and forgetting `--prompt` is silent.** The
   task names where the work is WRITTEN; the prompt is what the worker is
   actually TOLD. Name the first without the second and the pane opens with an
   EMPTY composer — deliberately, because a briefing with no instruction
   behind it would be a worker told how to report work it was never given.
   From outside that agent looks exactly like one that is thinking, and the
   only way to learn otherwise is `worker-read`.
   It cannot be repaired in place either: the summons already minted the
   dispatch carrying that task, so a later `dispatch --task <id> --to <pane>
   --inject` is refused as *already carried on this very pane*. Abandon the
   worker and summon again WITH `--prompt`.
   So: say the slice in `--prompt` every time, even when the spec already
   reads like one. The deliberate road to a silent pane is `--bare` with no
   `--task` at all, spoken to afterwards by `dispatch --inject`.
4. `check --wait` — read your inbox. Workers report to you here. A plain look
   needs no retry name; `check --ack <deliveryId> --retry-request ack-<deliveryId>`
   does, because it spends the batch. `--wait` takes `--timeout-ms` when your
   patience has a number; `--peek` looks without taking a batch, `--all` reads
   history and never waits, and `--format` adds a human banner inside the same
   JSON answer.
   **Wait in the background, not in the foreground.** A foreground
   `check --wait` blocks your whole turn for as long as the workers take —
   the person cannot talk to you, and you cannot do anything else. Run it
   with your shell tool's `run_in_background` (Claude Code, zo) and go on:
   answer the person, review what has landed, prepare the next slice. The
   completion arrives as a task notification mid-turn with the delivery in
   it; nothing is lost by not standing at the door.
   **Read a report's `payload` before you clean anything up.** The briefing
   tells a worker to keep its summary short and to carry a longer answer as
   `--payload '{"reportPath":"...","lifetime":"ephemeral"}'`. The lifetime
   is explicit because a raw path is not an artifact: it usually lives in the
   worker's worktree, which means `worker-release` and `git worktree remove`
   destroy it. Read it, and copy anything worth keeping somewhere that
   outlives the worker, BEFORE you retire the seat. A worker that packs a
   whole analysis into one summary line has lost most of it; that happened
   here on 2026-08-28 and the only recovery was scraping the pane.
5. `worker-read --worker <id>` — look at a worker's actual screen when you
   need to see what it is doing rather than what it said. A released worker
   answers from the screen its release archived, same tail rules. The window
   is asked by the seat the LEDGER knows, so another leader's worker — an
   orphan whose leader exited, a worker of the coordinator you took over from
   — reads the same way; "unknown pane" is no longer what that answers.
   `worker-transcript --worker <id> [--turns <n> | --since <ms>] [--json]`
   is a separate read of the same worker's CONVERSATION — its newest steps
   (default 5: what it said, the tools it then ran and what came back, and
   when), which its screen does not show — out of the transcript its agent
   reported, masked and cut at fixed caps; it is not a cheaper screen, it
   never answers from the screen, and a worker whose agent reports no
   transcript says `transcript unavailable` instead.

When verification needs pixels, use the computer-use skill's
`zerocode-browser screenshot` or `zerocode-emulator screenshot`; both commands
are already on every launched worker pane's PATH and return a local PNG path.

`worker-show`/`worker-list` carry a three-valued `agentWait`: an object
means the pane is parked on a prompt only a person can answer (with the
evidence and when it began), `null` means the window looked and found no
wait, and an ABSENT field means it never looked — an agent that wires no
hooks, a pane before its first report — which never means "not waiting".

They also carry `seat`, the window's own word for the pane behind the row:
`live` (the terminal is there), `gone` (the window looked and it is not),
`unknown` (no table maps the seat to a terminal, so nobody has looked). Read
it beside `state`: an `orphaned` row with `seat: live` is an agent at work
under no leader, and `taskId` stays on the row for as long as its dispatch
is open. `released` on a row that is not yours means the window proved its
pane gone — another leader's pane that is merely not in YOUR table keeps its
stored word. Each row names its `team` too, so two leaders' `%2` cannot be
confused.

Three more doors, when you need them. A worker that is BLOCKED asks —
`ask --body '<question>' --retry-request <name>` — and the verb itself waits
for the answer (ten minutes unless `--timeout-ms` says otherwise, half an
hour at most). A timeout is not a failure: the answer names the question, and
`ask --resume <questionId>` walks back to it without asking twice. One
question takes ONE answer — a reply that repeats the standing answer lands AS
it, a different one is refused with its name — and a question whose dispatch
ended is closed. While you wait, the ledger tells you how the RECEIVER stands
— one `status` line from `ledger` threaded on your question in your inbox,
`reason: turn_ended | interrupted | awaiting_input | stalled | <stall cause> |
quota_walled | taken_over | finished | seat_vacated | cancelled | exited` —
and the timed-out answer carries the last of them under `receiver`. None of
those is an answer, and `cancelled`/`exited` are final: the seat will never
answer, the blocked `ask` wakes on them, and `cancelled` says in so many words
not to ask again and not to summon a replacement — that is its coordinator's
decision. On the answering side you are just reading mail:
`check --types question`, then `reply --to-message <id> --body '<answer>'`.
`worker-start` also takes launch tuning where the agent's own CLI does —
`--model <id>` (an opaque provider id, passed through unread), `--effort`
beside it where a ride exists — plus `--retry-of <dispatchId>` to link a
replacement to the ENDED attempt it replaces, `--inherit-checkout` beside
it to seat that replacement in the ended attempt's own checkout (the same
tree, uncommitted work and all — refused when nobody reported that
checkout or it is gone, and exclusive with `--worktree`; the reply's
`inheritCheckout` names the path), `--timeout-ms` as a
readiness window: a summons that stays silent past it is reported to you
once, as news, settling nothing — and `--on-quota-wall
<agent[:model[:effort]]>`, the one alternative the ledger may summon in
your agent's place when that agent's provider is at its quota wall (the
reply's `quotaNotice.redirected` says both; without the flag a walled
summons is refused by name, and nothing is written). The same flag is the
standing order for THAT worker's wall later on: it is written on the worker
row (`worker-show.onQuotaWall`), and if the worker is witnessed at its
wall the beat hands its task to exactly that alternative — see the
handover paragraph below. For auditing there are `run-show`, `inbox`
(every run's mail, nothing moves) and `reset` (exactly one scope, and
receipts survive it).

`send --to` takes one address or a whole set. `@all` is every live worker
but you, `@idle` every live worker with no dispatch in flight, and
`@<agent>` — `@codex`, `@claude` — every worker that IS that agent, the
roster's own word rather than a guess from a pane title. Groups resolve when
you SEND: a worker summoned afterwards was not among those asked and gets
nothing, an empty group is a refusal rather than a quiet "sent", and the
message stays ONE message — every reader answers to the same id, so replies
meet in one thread by construction. Questions are the exception: `ask`
refuses a group, because one question takes one answerer. And
`@worktree:<path>` reaches every worker whose pane the window has placed in
that checkout — the path `worker-show` prints as `checkout`, exact match. A
row the window has not placed yet is in NO checkout group, and an empty
checkout group's refusal says how many such unreported rows it is not
counting — absence of the fact is never a fact of absence.

A worker can live on ANOTHER machine: `worker-start --on <server> --task
<id>` claims the task here and seats the pane over there — the far window
runs the summons through its own rules, your dispatch settles here when its
`worker_done` rides home, and `send --to remote:<dispatchId>` reaches that
pane's inbox. One-time setup, by verb: on the worker machine
`federation-invite` prints its address and token; on yours,
`federation-join <name> --addr 127.0.0.1:<tunnelled port> --token <it>` —
loopback only (reach a remote machine with `ssh -L`), a public address is
refused where you can fix it. `federation-servers` lists the book with
tokens shortened, `federation-forget <name>` drops one, and the far window
names which team hosts borrowed panes in `federation/host-team` when it
runs more than one. Placement words
(`--worktree`, `--horizontal`, `--bare`) are refused with `--on` — where
the borrowed pane sits is the server window's decision.

A finished worker's pane is not spent. `dispatch --task <id> --to <pane>
--inject --retry-request re-<id>` hands the next written task to an idle
worker you already have — the same briefing `worker-start` gives, with the
task's own spec, typed into that pane as one paste. `--dry-run` previews the
exact words without taking the task, and `dispatch-show --task <id>` reads
the latest attempt back. The target must be one of THIS run's workers: a
pane the run did not summon answers to somebody else's authority, and
`dispatch` refuses it rather than typing at a stranger.

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

**Your workers survive a restart too — the window seats them again itself.**
A window restart keeps the seat; a worker that never comes back dies with a
dispatchId. A worker that was in a durable checkout comes back as `sleeping`:
its dispatch is still open and its task still carried, and the window resumes
its conversation into a restored pane and reseats it without you — the same
worker id, the same dispatch, no handover. The window's own exit puts the
seat to sleep BEFORE its panes go, so a pane exiting on the way out is not a
death and raises no gate; and a pane the person had taken over comes back the
same way, seated again by the window's own restored tab. The restored worker
is told its seat stands and where its checkout is, and continues from its last
tool result rather than re-running every gate. Only a sleeper nothing resumed
within the grace (ten minutes from boot) ends: `worker_died` with the
`dispatchId` a `--retry-of` needs. Do not summon a replacement for a
`sleeping` worker; wait for `worker-list` to show it `active` again (or its
`worker_died` if nothing brought it back). A restored
worker that is alive and at work is seated as it is, briefing or no briefing —
it picked its task straight back up, and the window does not tear it down for
being busy. The one case you end yourself is a `sleeping` worker with nothing
running behind it — you looked (`ps`, the roster) and found no process:
`worker-stop --worker <id> --reason <what you saw>` ends the attempt through
the sleeping road, closes nothing (that pane id may be somebody else's in this
window), and hands the task back `ready` — after which `worker-start --task …
--retry-of <dispatchId>` takes it (add `--inherit-checkout` to seat the
replacement in the checkout the ended attempt sat in). `worker-abandon` does
the same and says only that nobody is watching.

Those `--retry-request` names are examples of the SHAPE, not magic strings:
each one has to be unique to its request and identical if you run that request
again. Deriving them from the ids you already have — the run, the task, the
delivery — is the cheapest way to get both.

Two answers you may see once and never again. **"this pane has no session
identity yet"** means your agent has not reported to the window yet; wait for
your first turn to finish and ask again — a change cannot be made retryable by
somebody nobody can name. **"…was answered by an older window"** means that
retry name belongs to a receipt this build cannot check; choose a different
name. Both happen at most once, and neither is something you can work around by
dropping `--retry-request`.

## Five rules that are not optional

These are the ones that cost you a run when you get them wrong.

- **Acknowledge what you read.** `check` hands you a batch and hands you the
  SAME batch again until you `check --ack <deliveryId> --retry-request
  ack-<deliveryId>`. That is what makes a
  coordinator recoverable — but it also means an unacknowledged batch is an
  infinite loop. Ack after you have acted, never before.
- **Report finishing; do not announce it twice.** As a worker, `send --type
  worker_done --retry-request done-<your task id> --body '{"ok":true,"summary":"…"}'`
  closes your task and your dispatch. You do not name the task,
  the dispatch, or the address — the run already knows what you were carrying,
  and naming it yourself is how you close somebody else's work. Do not follow
  it with `task-update`.
- **Say whether you succeeded — you must, in so many words.** A `worker_done`
  carries `--body '{"ok":true,"summary":"…"}'` or `--body '{"ok":false}'`, and
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
  Claude worker that you did NOT pin a model on is launched so its own CLI
  continues the declined turn on the model the provider routes the category
  to (`cyber` → Opus 4.8) instead of pausing — the flag rides that worker's
  launch only; the person's settings, their panes and yours are untouched.
  A worker you summoned with `--model` is pinned: its CLI is launched with
  that switch OFF (zo: `--classifier-fallback off`), whatever the person's
  file says, because the model you named is binding and the CLI's own route
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
  nothing, is how you stop tracking it. Reports from that pane still land.

## When to orchestrate — and when not to

Split work only when the slices are genuinely independent: different files,
different subsystems, no shared state that both sides would edit. If two
slices would touch the same files, they are one slice. A task that fits in
one focused session should stay in one session — coordination has overhead,
and a split that saves no wall-clock time only adds it.

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

## Decision gates

When a slice reaches a decision that is a person's or the coordinator's to
make — an API shape, a destructive step, a tradeoff the spec left open — hold
the work instead of guessing. `gate-create --task <id> --question '<the
decision>' --options '["a","b"]' --retry-request gate-<task>` blocks the task:
it leaves `task-list --ready`, `worker-start` refuses it, and it stays held
until `gate-resolve --gate <id> --resolution '<the answer>' --retry-request
answer-<gate>` frees it. `gate-list` shows what is standing and what was
decided. A resolved gate is not re-resolved — the first answer stands, and a
different second one is refused rather than written over it.

Workers do not open gates. A worker that hits a decision sends it up —
`send --type decision_gate --body '<the question>'` — and stops; whether the
question deserves to hold the DAG is the coordinator's call, made from the
coordinator's own pane.

## Worker gates and briefing template

For code changes in a Rust workspace, the worker gate must include
`cargo clippy --all-targets -- -D warnings` from the repository root, including
test targets across the workspace. Report each gate exit code without hiding it
behind a pipe. A package-only clippy run does not cover this gate.

### Finish once — the briefing carries the acceptance, not the review

A landing that needs a second pass is a briefing that was missing something,
not a worker that was careless. On 2026-09-24 four of five landings came back
from the closing partner's review for criteria that could have been written
first: transient-state assertions instead of end-state ones, a pinned model's
contract, whether a "run once" actually runs, one series per rubric version.
So:

- Write the definition of done into the briefing before summoning: the
  red-first test names, the measurement table, and every contract, boundary
  and negative case the reviewer will hold the work to.
- Have the closing partner (the adversarial reviewer) read that briefing
  first — ten minutes — and fold what it adds. A rejection after
  implementation is the briefing's failure.
- `worker_done` is accepted only with red (failing before the fix) and green
  (passing after) receipts — command, sha, exit code, log path — and the
  briefing's acceptance items ticked in the report.
- Close in the same task: review findings are fixed by the same worker in the
  same checkout and merged once. "Accept with follow-ups" spawns no new task
  unless the follow-up is a different unit by design.
- The coordinator writes no claims of effect into comments or docs; it writes
  what the code does and what was measured.

Copy into each implementation briefing:

```text
Work in a worktree. Run the affected tests and cargo fmt --all --check.
Required root gate: cargo clippy --all-targets -- -D warnings
Report command exit codes without pipes, commits, and anything left before worker_done.
```


The main checkout is reserved for merges and the release lane. Do all edits,
builds, tests, commits, and version bumps in a worktree; never do task work on
main. Coordinators sharing the checkout merge and gate in a detached coordinator
worktree, without stashing or committing another pane's changes.
Exception: update the main checkout's release lane driver from the landed commit
when the lane needs it; replace the file atomically (git checkout or temp + rename).

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
