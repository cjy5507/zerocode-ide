# Browser state probe

Times the browser door's look and a press's settle before a change and after
it, on pages of our own, in a hidden tab of the running window (t-6721):

    python3 tools/browser-state-probe/probe.py --before <rev> --out <scratch dir> [--rounds 12]

`--before` names the revision whose page scripts (`cmd/browser.rs`) are the
"before"; the "after" is this checkout. Both run through the window's own
`zerocode-browser eval` road, turn about (ABBA), in the same tab — so they pay
the same CLI, bridge and callback, and differ only in the script. The
installed `marks --json` verb runs beside them as the calibration of that
road. No build is needed, and nothing is installed or restarted.

What the table says:

- **Look** — one call's wall and the page's own milliseconds, the answer's
  bytes and what it carried: numbers, fields, containers/images/rows and a
  document epoch.
- **Settle after a press** — the product's settle policy, poll by poll
  (the core's `settle_verdict`, its constants read from the Rust, the copy
  held to the core's cases by `test_probe.py`): its verdict, wall and
  page-clock time and polls, and whether the look right after the press saw
  what the press made — against a look taken at once, the road before. Each
  poll here crosses the CLI; in the product it is an in-process callback, so
  the wall here is an upper bound and the page-clock time is the settle's own.
- **Document replaced** — a press settled on another document is
  `invalidated`, and a press carrying the old document is refused.
- **Focus** — the frontmost app around every call, and what each look,
  settle poll and press did to the tab's focus, active element and scroll.
- **Timers** — how late the hidden tab's own `setTimeout` fires: what a page
  that answers later waits on. A timer that does not fire in the deadline is
  censored (counted as not fired), never given the deadline as its time.

The probe opens its own tabs on `pages/` and closes them; it never touches a
tab it did not open, uses no model and no key, and writes rows, `summary.json`
and `table.md` into `--out` — keep them outside git. Local pages say nothing
about a network site's timing.

Validation: `python3 -m unittest discover -s tools/browser-state-probe -v`.
