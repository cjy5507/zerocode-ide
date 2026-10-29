# Driving the verbs: the session, mail, delegate, send, dispatch

Read this when you write the first verbs of a run, read a report, answer a
question, or use `delegate`, a `send` group, `dispatch --inject` or a remote
worker. The text was moved here word for word from the former single-file
guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## The shape of a session

The shape of a session is always the same:

1. `run-create --name <run> --retry-request open-<run>` — open a run, bind
   YOU to it, and sit in its coordinator seat. The run you are working in
   follows your session, not the pane you happen to be sitting in: close the
   window, come back, and `run-current` still answers the run you were in.
   `run-use <run>` moves you; nothing else does, and a retry of an old request
   never moves you back. Binding and sitting are two facts: `run-use` answers
   `"seated": true` only when the chair was empty (see the seat rule above).
2. `task-create --spec-file <file> --retry-request task-<run>-1` — write each
   slice down BEFORE summoning anyone. A task carries its own spec, and
   `--deps` holds a slice back until the slices it needs have finished.
   Write `--title` in English: checkout and branch names are ASCII-only, so
   Korean words fall away and a title led by `t-<id>` keeps only that id.
3. `worker-start --agent <name> --task <id> --prompt-file <file> --retry-request
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
   So: say the slice in `--prompt` (or `--prompt-file`) every time, even when the spec already
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
   The window now keeps a copy itself (t-32798): when a `worker_done` names a
   `reportPath` (with `"reportKind"` — report, review, brief, handover or proposal —
   when it is one of the last four), or files and folders under
   `"evidencePaths":["/abs/file","/abs/dir"]` (an entry may be
   `{"path":"/abs/red.log","expected":"fails at the assertion","expect":"fail"}`, so an
   intended failure is said and not guessed from the file name; a report that
   cites a file it named writes that file's absolute path in its text, not only
   its name — a log called `red.log` cannot be found afterwards),
   the ledger copies them into the artifact store — masked of home paths,
   mailboxes, private addresses and credentials, text and pictures only, at most
   24 MB a hand-in — and the task's row on the board says what was kept, what was
   left out and why, with a door to open it. It keeps them again before any
   cleanup or release takes the checkout, and a cleanup refuses while a keeping
   has not finished or failed. The same road serves every agent CLI. Read the
   kept copy from the Artifacts view (or the row's door) rather than the
   worker's path, which is gone once the checkout is.
   A hand-in with `"reportKind":"review"` — or a report (no commit) on a task
   whose title has the word 검수 / 읽기 전용 / review / read-only / audit (a
   whole word: 「preview」 is not one) — has no code to land: your
   `verified` of it is written with `nothingToLand` added (the answer says
   why under `nothingToLandBecause`) and, when it handed in a commit, that
   commit as `reviewedHead`, and `landing_stalled` never asks it to merge —
   unless your result says something of landing itself (`merged`,
   `nothingToLand: false`): your word stands over the worker's.
   **A review brief requires `"reportKind":"review"` in the worker's
   `worker_done` payload**: a reviewer that hands in the head it reviewed is
   told from code waiting to land by that word alone.
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
`ask --body-file <file> --retry-request <name>` — and the verb itself waits
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
`check --types question`, then `reply --to-message <id> --body-file <file>`.
A question from ANOTHER run's coordinator (`send --run <run> --to run:<run> --type question`) is answered the same way: one `reply --to-message <id>` closes it here and is delivered to the run that asked (`deliveredTo` in its answer) — do not answer with a `send --type status`, which leaves the question standing on the board. A restart that moved the asking chair to a new pane changes nothing: the answer still goes to the run that asked — the one whose chair the asking pane sat in when it asked — where its chair now sits.
A question nobody will answer is closed, not left standing: `question-close --message <id> --reason <one line> --retry-request <name>`, from the coordinator seat of the run asked or of the run that asked (found by its id from either side). The ledger writes the close into the question's thread, every count leaves it, a waiting `ask` wakes as closed with the close's `closedBy` and `note`, and `reply` refuses it; closing it again answers `already: true` with the first close.

`delegate` is the three steps of a new piece of work in one: it writes the task,
summons a worker on it (`worker-start`'s own gates: the quota wall, `--on-quota-wall`, the
disk check for `--worktree`) and leaves the worker a letter that tells it its own worker,
dispatch and task ids, which a worker cannot learn by itself. `zerocode-orc delegate --spec-file
<file> --agent <a> [--title <t>] [--deps a,b] [--prompt-file <file>] [--model <id> [--effort
<level>]] [--on-quota-wall …] [--worktree] --retry-request <name>`. The worker's first words
are the spec unless you give `--prompt`. A refusal anywhere leaves no task, worker or letter
behind; the same `--retry-request` answers with the first answer and makes nothing twice.
With `--wait --timeout-ms <ms>` (up to ten minutes) the call holds its answer until that
worker reports (`worker_done`), asks you something, or its attempt ends silently — the answer
then carries a `waited` object with the outcome and, for a report, the worker's own summary.
It never reads or acknowledges your inbox, so other workers' mail waits for your `check` as
usual. A worker that is already on a task, a replacement for an ended attempt, a bare pane
and a federated worker still go through `worker-start`.

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
(this project's runs' mail, nothing moves; `--since <ms>` and `--cursor
<nextCursor>` walk a whole day page by page) and `reset` (exactly one scope,
refused if it would touch another project's runs; receipts survive it).

The ledger is shared by the window, but a pane's commands and mail stay in
its project. Worktrees sharing one repository are one project; its runs can
write to each other. `--run` does not cross that boundary. Direct pane
recipients and replies are checked against the recipient's current project
as well. A refusal says which project does not match; ask from that project's
pane rather than changing a scope switch. Legacy runs whose project is not
known are visible only to their own seated participants.

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
