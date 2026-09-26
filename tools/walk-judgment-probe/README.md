# Walk judgment probe

Times the product's own goal walk before a change and after it, on a page of
our own, turn about: the question, the Jev wire, the goal world and — in the
build after — the value seat, with the one road the walk drives handed the
window's browser CLI. A typed value reaches the CLI on stdin
(`type … --value-stdin`), never on a process's argv.

    TYPESAFE_API_KEY="$(security find-generic-password -s dev.zerocode.key.TYPESAFE_API_KEY -a "$(id -un)" -w)" \
      python3 tools/walk-judgment-probe/run.py --before <sha> --out <scratch dir> [--walks 8]

The checkout must be clean: the build before swaps the product files to
`<sha>`'s under this harness, builds, and puts HEAD back. Each build's run
walks twice in one process and the table reports the second walk; the first
(cold connections) is reported on its own. The page's snapshot the browser
door does not carry yet (t-6721 U4) is stood in for by `snapshot.js`, timed
apart and taken out of every step. Rows, `table.md` and `summary.json` land in
`--out`; keep them outside git. A zo home of the probe's own takes every
ledger row; the person's own is never written. The value seat in the build
after asks with `ANTHROPIC_API_KEY` from the command's environment when a
person has one — the way their own key reaches it in the window — and offers
no entry without it; a subscription login is never used.

Arms (`--arms`, default `before,after`): `before`/`after` walk as a plain
`walk`; `before-ahead`/`after-ahead` walk as `walk --overlap`, asking ahead
(t-9712). `--scenarios` adds `steps` (`steps.html`, three presses that each
change the page at once) to `press,repeat,type,observe`, and `later` — the
same walk on `steps.html?delay=120`, whose step answers 120 ms after the
press, busy until then, so the page a press leaves is the page it was pressed
on, as on a page that fetches (t-9712 r2). That answer comes from a worker's
timer: a hidden pane holds the page's own timers to the next whole second.

The running window's door may be older than the build's (a v1.1.25 window
does not settle a press by number, and knows no `--settle-later`), so the
probe stands the build's door in front of it: a press by number settles by
the product's own loop (`settle_with`), each poll one `eval` of the door's
settle script, stillness counted from the first poll; `--settle-later`
answers the page the press changed and the next `marks` finishes the settle —
or, in a build that has the door's rule (t-9876), settles before it answers
when that page still reads the legend the press was made on, compared by the
core's own `same_legend`, and answers the page read after. What that stand-in
did inside a call is on the call (`inside`), and the table says it: settle
p50/p95 and how many ended ready, how many a press finished before it
answered (settled in the press), the look a settle-later press answered with
first (preview), the gap from one hand to the next, judgments begun ahead
that were used, dropped or cancelled, and the questions a walk asked in turn
and ahead. The `// after-only` lines of `probe.rs` are what the build before
does not have; everything else stands in front of both builds alike.

    TYPESAFE_API_KEY=… python3 tools/walk-judgment-probe/run.py --before <sha> --out <dir> \
      --walks 8 --scenarios press,repeat,type,observe,steps --arms before,before-ahead,after-ahead

Validation: `python3 -m unittest discover -s tools/walk-judgment-probe -v`.
