#!/usr/bin/env python3
"""How often an idle zo wakes: the number `just tui-bench` reads over five seconds,
read over a minute, in the states a person actually leaves zo in (t-17057).

`tui_bench.py`'s idle window is five seconds long. At 3.2 wakeups a second that
is sixteen of them; at the 0.2 a second this slice aims for it is one, and the
row cannot tell 0.2 from 0. This probe starts one zo in a pty exactly as the
bench does (a hermetic home, an environment built from nothing, the bench's own
`Terminal` and its `proc_pid_rusage` sampler), waits for the composer, lets it
settle, and counts `ri_pkg_idle_wkups + ri_interrupt_wkups` of the process tree
over `--window` seconds.

States (`--state`):
  idle      the composer is up, no turn has run.
  events    the same, with the IDE events channel open, which is how the
            ZeroCode window launches every pane (`ZO_EVENTS_BIND`): the session's
            roster watcher runs there for as long as the pane lives.

A timing counts only on mains power with no sleep inside its window: each run
records the power source, the load, and whether the wall clock ran ahead of the
monotonic one (the Mac slept). Such a run is printed as "not judged".

    python3 idle_probe.py --zo target/release/zo --label main --state idle --runs 3

The benchmark script itself is not touched (it belongs to the output slice).
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import tui_bench as bench  # noqa: E402  (the harness this probe borrows its terminal from)


def power_source() -> str:
    try:
        first = subprocess.run(["pmset", "-g", "batt"], capture_output=True, text=True,
                               timeout=10).stdout.splitlines()[0]
    except (OSError, subprocess.TimeoutExpired, IndexError):
        return "unknown"
    return "AC" if "AC Power" in first else first.strip()


def clocks() -> tuple[float, float]:
    """(wall, monotonic): the monotonic clock stops while the Mac sleeps, the wall one does not."""
    return time.time(), time.monotonic()


def measure(binary: str, label: str, state: str, run: int, window: float, settle: float,
            seed_agents: int, base: Path, keep: bool) -> dict:
    dirs = bench.make_dirs(base, f"{label}-{state}", run)
    cli = bench.zo_cli(binary, label, seed_agents)
    url = "http://127.0.0.1:9"  # nothing answers, and an idle zo asks nothing
    cli.prepare(dirs, url)
    env = cli.env(dirs, url)
    if state == "events":
        env["ZO_EVENTS_BIND"] = "127.0.0.1:0"
    result: dict = {
        "label": label, "state": state, "run": run, "window_s": window,
        "power": power_source(), "load_before": os.getloadavg()[0],
    }
    term = bench.Terminal(cli.argv(dirs), env, dirs.project)
    try:
        if not term.wait_screen(cli.ready, 90):
            result["error"] = "never showed its composer"
            result["screen"] = term.screen_text()
            return result
        term.wait_quiet(1.0, 10)
        term.pump(settle)
        wall0, mono0 = clocks()
        t0 = bench.mono_ns()
        term.pump(window)
        t1 = bench.mono_ns()
        wall1, mono1 = clocks()
        measured = term.window(t0, t1)
        result.update({
            "wakeups_per_s": measured.get("wakeups_per_s"),
            "cpu_pct": measured.get("cli_cpu_pct"),
            "bytes": measured.get("bytes"),
            "processes": measured.get("processes"),
            "rss_mb": measured.get("cli_rss_mb"),
            "slept_s": round((wall1 - wall0) - (mono1 - mono0), 2),
            "power_after": power_source(),
            "load_after": os.getloadavg()[0],
        })
        # A window's wakeups from two samples a minute apart: keep the raw ends
        # so the rate can be recomputed by anyone reading the file.
        first, last = term.at(t0), term.at(t1)
        if first and last:
            result["wakeups_first"] = first.wakeups
            result["wakeups_last"] = last.wakeups
        if measured.get("bytes"):
            result["warning"] = "the screen changed while the window ran"
    finally:
        term.close([b"\x03", b"\x03", b"\x04"])
        if not keep:
            import shutil
            shutil.rmtree(dirs.root, ignore_errors=True)
    judged = (result.get("power") == "AC" and result.get("power_after") == "AC"
              and abs(result.get("slept_s", 0)) < 0.5)
    result["judged"] = judged
    if not judged:
        on_mains = result.get("power") == "AC" and result.get("power_after") == "AC"
        why = "slept" if on_mains else "battery"
        result["not_judged"] = why
    return result


def table(results: list[dict]) -> str:
    lines = ["| label | state | runs | wakeups/s (median) | min | max | CPU % | not judged |",
             "|---|---|---|---|---|---|---|---|"]
    groups: dict[tuple[str, str], list[dict]] = {}
    for one in results:
        groups.setdefault((one["label"], one["state"]), []).append(one)
    for (label, state), rows in groups.items():
        good = [row for row in rows if row.get("judged") and row.get("wakeups_per_s") is not None]
        rates = sorted(row["wakeups_per_s"] for row in good)
        cpus = [row["cpu_pct"] for row in good if row.get("cpu_pct") is not None]
        skipped = len(rows) - len(good)
        if rates:
            lines.append(f"| {label} | {state} | {len(good)} | {statistics.median(rates)} | {rates[0]} | "
                         f"{rates[-1]} | {round(statistics.median(cpus), 2) if cpus else '-'} | {skipped} |")
        else:
            lines.append(f"| {label} | {state} | 0 | - | - | - | - | {skipped} |")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--zo", required=True, help="the zo binary to measure")
    parser.add_argument("--label", default="zo", help="the row name of --zo")
    parser.add_argument("--state", action="append", choices=["idle", "events"],
                        help="a state to measure (repeat for more; default idle and events)")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--window", type=float, default=60.0, help="seconds counted per run")
    parser.add_argument("--settle", type=float, default=6.0, help="seconds after the composer before counting")
    parser.add_argument("--seed-agents", type=int, default=0,
                        help="finished helpers seeded into the store (the bench seeds 432)")
    parser.add_argument("--out", help="append one JSON line per run to this file")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    states = args.state or ["idle", "events"]
    base = Path(tempfile.mkdtemp(prefix="idle-probe-"))
    results: list[dict] = []
    try:
        for run in range(1, args.runs + 1):
            for state in states:
                one = measure(args.zo, args.label, state, run, args.window, args.settle,
                              args.seed_agents, base, args.keep)
                results.append(one)
                line = json.dumps(one, ensure_ascii=False)
                print(line, flush=True)
                if args.out:
                    with open(args.out, "a", encoding="utf-8") as handle:
                        handle.write(line + "\n")
    finally:
        if not args.keep:
            import shutil
            shutil.rmtree(base, ignore_errors=True)
    print()
    print(table(results))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
