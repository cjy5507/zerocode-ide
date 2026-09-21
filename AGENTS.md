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


# What never goes into git

This repository publishes a release snapshot, so the tree is public. A value
that points at a real person, a real machine or a real network does not belong
in a commit — not in a test, not in a fixture, not in a comment, not "just for
now". Neither do the private notes: design documents, plans and analyses stay
on the machine (`.gitignore` refuses `docs/` and `zo-ide/docs/`).

What must stay out:

  · **A person** — a home directory (`/Users/<name>`, `C:\Users\<name>`, and
    the same path slugged into a session id), a real mailbox, an account name,
    a colleague's or a customer's or a company's name.
  · **A machine or a network** — an office address on a private subnet, an
    internal host or its ssh alias, a server, a port behind a VPN.
  · **A credential** — a token, a key, a password, a cookie, a signed URL.
    Never, in any shape, even expired.
  · **The private notes** — design documents, plans, analyses, briefs.

What to write instead: `/Users/dev` for a home directory, the RFC 5737
documentation addresses (`192.0.2.x`, `198.51.100.x`) for an address, an
`example.com` / `.test` / `.invalid` domain for a mailbox, `acme` for a company.
A fixture reads better this way anyway — it says "this is a shape", not "this
is my laptop".

`just pii-check` is the gate. It is the first recipe `verify` names, so it
refuses before anything compiles, and everything it knows is the `RULES` table
at the top of `tools/release/pii-scan.py` (`--table` prints it). A new
exception is an entry in that row's `allow`, or — for a single site that cannot
be rewritten — a `pii-scan: allow <category> — <why>` comment where it stands.
Run it before you commit; do not silence it to get a commit through.

A note is not a fixture. If code reads a file under `docs/`, that file is a
measurement, not a note: move it beside the code that reads it (`tests/fixtures/`,
`bench/`, `tools/`) and scrub it on the way in — a terminal capture carries the
machine it was taken on. Proving that nothing reads a folder means deleting it
from the checkout and running every gate, not grepping for it.


The main checkout is reserved for merges and the release lane. Do all edits,
builds, tests, commits, and version bumps in a worktree; never do task work on
main. Coordinators sharing the checkout merge and gate in a detached coordinator
worktree, without stashing or committing another pane's changes.
Exception: update the main checkout's release lane driver from the landed commit
when the lane needs it; replace the file atomically (git checkout or temp + rename).
