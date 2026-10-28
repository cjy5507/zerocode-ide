# Recovering: killed panes, the coordinator seat, dead workers, restarts

Read this before you stop or reap a pane, when a seat is held by somebody
else, when a worker died in a checkout of its own, and after a window
restart. The text was moved here word for word from the former single-file
guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## Panes, seats and deaths

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
`run-takeover --run <id> --from <seat> --reason-file <file> --retry-request
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

## After a restart

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
`worker-stop --worker <id> --reason-file <file>` (what you saw) ends the attempt through
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
