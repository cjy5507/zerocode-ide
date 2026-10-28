# Writing a worker's briefing

Read this when you write a worker's briefing: the acceptance goes in
first, `zerocode-find`, the template to copy, and how reports are written.
The text was moved here word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## Worker gates and briefing template — the rest

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
  briefing's acceptance items ticked in the report. When the task's spec writes
  pass conditions (lines that begin `통과 전`), an `ok:true` report also needs
  one `evidence` entry per condition (build-line job, exit code, numbers); the
  ledger refuses it without them and the same `--retry-request` sends it again.
  A job named as a condition's evidence ends with a non-zero exit code when that
  condition fails — its last command is the check itself, never a closing
  `exit 0` after printing the stages: the receipt is the script's last exit code.
- Close in the same task: review findings are fixed by the same worker in the
  same checkout and merged once. "Accept with follow-ups" spawns no new task
  unless the follow-up is a different unit by design.
- The coordinator writes no claims of effect into comments or docs; it writes
  what the code does and what was measured.

### Find the files first — `zerocode-find`

Every pane in this window has `zerocode-find` on its PATH. Before the first
read of code you do not know yet, run it with what you are about to change or
debug:

```sh
zerocode-find the parser crash on an empty line
```

It prints the files most likely involved, each with its first comment line —
the window's search for the task's own words, ranked by the file pick
judgment where that judgment acts for the pane. Start from those files
instead of searching; they are suggestions, so verify them. In the last seven
days before it existed, a Claude worker spent a median of 20 reads and
searches before its first edit (t-14656).

Copy into each implementation briefing:

```text
Work in a worktree. Run the affected tests and cargo fmt --all --check.
Before the first read of unfamiliar code: zerocode-find <what you are changing>
Required root gate: cargo clippy --all-targets -- -D warnings
Report command exit codes without pipes, commits, and anything left before worker_done.
```


The main checkout is reserved for merges and the release lane. Do all edits,
builds, tests, commits, and version bumps in a worktree; never do task work on
main. Coordinators sharing the checkout merge and gate in a detached coordinator
worktree, without stashing or committing another pane's changes.
Exception: update the main checkout's release lane driver from the landed commit
when the lane needs it; replace the file atomically (git checkout or temp + rename).

### Reports are for a person: the plain-report skill

A person reads a worker's summary, a report and a notice. The person asked
for plain writing. Put the result first, write one fact in a sentence, give
every number a unit and use no codebase metaphors. Where you explain a
structure, add a diagram.

The shared worker briefing says so once, at launch. The plain-report skill
holds the rule and its word list. An agent reads it when it writes such a text,
not every turn. When you report to the person yourself, follow it too.

The window counts a worker's summary and a report against the rule and shows
the counts beside them. It only counts. It never refuses a report.
