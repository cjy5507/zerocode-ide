# Running other agents from inside this window

This checkout's terminals run inside the ZeroCode window, which has its own
orchestration ledger. When work calls for another agent's hands — a parallel
analysis, a second implementation pass, a reviewer — do NOT run it as a
background pipe (`claude -p`, `codex exec`, `orca …`). A piped
agent has no terminal of its own: its raw event stream floods whatever pane
mirrors it, nobody can type to it, and the person watching sees noise instead
of a colleague.

Summon it through the ledger instead:

    zerocode-orc task-create --spec "<what needs doing>"
    zerocode-orc worker-start --agent <requested-agent> --task <task-id> --prompt "<the slice>"
    zerocode-orc check --wait
    zerocode-orc send --to <pane-or-@group> --body "<message>"

Each worker opens as a real CLI in its own terminal pane — a Codex window for
Codex, a Claude window for Claude — and the ledger carries its briefing, its
mail, and its completion report. `zerocode-orc help` lists every verb;
`skills/orchestration/SKILL.md` is the full guide.

An agent identity, model, or effort chosen by the user is a binding constraint,
not a preference. Resolve the identity through the ZeroCode catalog and pass
the exact requested values to `worker-start`. Never substitute the current
provider, another model, a provider-native teammate, or cross-session
messaging; if that exact launch cannot start, report the blocker without a
fallback. Check the `worker-start` response before claiming what began.

The `orca` CLI on this machine belongs to a different product. Never use it
here — its workers land in the other product's windows, not this one.


The main checkout is reserved for merges and the release lane. Do all edits,
builds, tests, commits, and version bumps in a worktree; never do task work on
main. Coordinators sharing the checkout merge and gate in a detached coordinator
worktree, without stashing or committing another pane's changes.
Exception: update the main checkout's release lane driver from the landed commit
when the lane needs it; replace the file atomically (git checkout or temp + rename).
