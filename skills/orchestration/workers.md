# Summoning a worker: placement, disk, and which agent

Read this before you summon: whether a worker gets a worktree, what the disk
can bear, how zo's delegation tools reach the ledger, and how to choose the
agent, model and effort. The text was moved here word for word from the
former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## What this product actually gives you

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

## zo delegation tools in this window

With a team grant and `zerocode-orc` on `PATH`, zo's `Agent`, members of
`SpawnMultiAgent`, and `Workflow` phases use the same ledger door as a
Claude Code or Codex coordinator. Set `agent` to an exact catalog id and
carry any requested `model` and `effort` unchanged. The tools validate the
catalog and the launch receipt; a refusal is returned without a provider
substitution. Each ledger worker defaults to `worktree: true`. Set it false
only for a reader that neither builds nor changes git state.

The tool reuses the pane's current run, including when this pane is a
worker, and creates a run on first summon only when none is bound. The
ledger's `startedBy` edge routes each child's `worker_done` to its direct
parent; all commands for that child keep the captured run id. Ledger reports return through zo's
ordinary completion channel, including detached notifications. Cancellation
and an explicit time budget stop the ledger worker; no implicit one-hour
budget is added. A quota wall is an error, never an agent switch.

Native conversation forks, restricted permission harnesses, and native
workflow isolation directories cannot be transferred to another CLI; those
requests return a blocker. Use an explicit task brief and the ledger's
`worktree` field. With `worktree: true`, each Workflow phase gets a fresh
checkout: inspect the prior phase's reported commit and give dependent phases
that source explicitly. The coordinator integrates the returned commits.
Outside this window, zo retains its native helpers.

## Choosing the agent, model and effort


**When the person did not name a model or an effort, leave `--model` and
`--effort` out — the ledger asks Jev.** An omitted dial is not the CLI's
default: `worker-start` puts the work's title and the head of its spec to
Jev's difficulty question (`low`, `mid`, `high`), waits up to two seconds, and
launches on that difficulty's row of the person's table
(`smart.summonProfiles`, the settings card's 난이도별 워커 모델·사고 깊이 — rows for
Claude and Codex; another agent's omitted model is its own CLI default, and
its effort follows the difficulty where its CLI takes one). With Jev switched
on the question is carried out from the first summons; when it does not answer
in time or refuses, or a person set it to `shadow` or `off`, an omitted model
launches on the middle row. A dial you do pin always wins, and one pinned
alone keeps the answer for the other (`--model` without `--effort` takes the
answer's effort). Pin a dial only when the person named it, or when a quota
handover names it (`--on-quota-wall <agent:model:effort>` carries exactly what
it says).

Pinning out of habit costs more than the dial: every answer is written down
with whether it was carried out (`~/.zo/jev/summon-difficulty.jsonl`,
`applied`), and what became of the work — whether its first attempt landed,
what it cost in tokens and time — is the only evidence that keeps the
question acting or takes it back. A pinned summons grades nothing. Read what
a summons came to from the reply's `model` and `effort` and from
`worker-show`; the settings card and the Jev tab say whether the question is
acting or still recording.

Still say the classification out loud, in one line each, where the person
reads the round's plan — which agent, which slice, and how hard you judge it.
What raises difficulty is not the size of the diff: it is how much of the
work is JUDGEMENT. Surgery on a large well-tested file, a race whose order
decides the answer, a design trade-off with a real cost on both sides are
hard; an audit that is mostly `grep` and reading, a survey comparing two
roads, a mechanical follow-through are not. An analysis is not automatically
cheap and an implementation is not automatically dear; a read-only slice can
be the hardest thing in a round. When the launch you read back disagrees
with your line — a slice you called light launched on the high row, or the
reverse — say so to the person rather than re-summoning with a pin: that is
how a wrong answer is caught while it can still be weighed.

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
   same reason you say the classification: so a bad split is visible while it
   can still be changed, instead of being inferred later from a bill.

A person naming an agent overrides all four, and their choice is binding.

**`--agent auto` leaves the agent to Jev.** With Jev switched on, the
summon question picks one of the agents this machine could start right now
— the set the quota gate offers, each with its headroom and this ledger's
record of it — for the summons' brief, waits up to ten seconds, and the
summons lands on its pick: the reply's `agent` is the agent chosen, never
the word `auto`, and the rest of the summons (the dials above, the quota
gate) is read for that agent. When the question is not acting — Jev off, a
person's `shadow` or `off` for it, or its judge took it back — or it does not
answer, the summons is refused by name and nothing is written: name the agent
then. An agent the person named is never replaced by `auto`.

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
