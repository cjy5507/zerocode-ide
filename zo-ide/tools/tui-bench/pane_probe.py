#!/usr/bin/env python3
"""What three helpers working in panes cost their parent, and how soon their
answers are heard (t-17057).

The parent's wait for a pane child (`runtime::subagent_panes::
wait_for_turn_result_on`) looked every 250 ms: two result files opened and a
`tmux list-panes` spawned, four process spawns a second for each child, for as
long as it worked (t-11961 cause 5). This probe runs the wait itself, as the
`pane_probe_three_helpers_at_work` test of the runtime crate — three children
in three directories, a fake `tmux` that logs each call it is run with — and
reads the kernel's account of that process from the outside:

  spawns per second   the fake tmux's own log: one line is one process spawn.
                      A seam that counts `Tmux` calls, not a sample of the
                      process tree (a spawn lasts a few milliseconds and a tree
                      sampled every 100 ms sees a handful of them).
  CPU                 `proc_pid_rusage` of the test process between the markers
                      it prints, its own time plus the time of the children it
                      reaped (the fake tmux shells), as a share of one core.
  wakeups             the same call's `ri_pkg_idle_wkups + ri_interrupt_wkups`.
  __posix_spawn       `--sample`: `/usr/bin/sample` over three seconds of the
                      window, counting the samples that stand in `posix_spawn`,
                      which is the evidence t-11961 quoted (12 in 3 s).
  result latency      from an answer landing to the wait returning, over
                      `--rounds` rounds at different phases of a quarter second.

Run it on the runtime crate's newest test binary, before and after a change:

    python3 pane_probe.py --binary target/debug/deps/runtime-<hash> --label main

A timing counts only on mains power with no sleep inside its window: a run
records the power source and whether the wall clock ran ahead of the monotonic
one (the Mac slept), and is printed as "not judged" when either fails.

The benchmark script itself is not touched (it belongs to the output slice).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import tui_bench as bench  # noqa: E402  (its `proc_pid_rusage` reader)

TEST = "subagent_panes::tests::pane_probe_three_helpers_at_work"


def power_source() -> str:
    try:
        first = subprocess.run(["pmset", "-g", "batt"], capture_output=True, text=True,
                               timeout=10).stdout.splitlines()[0]
    except (OSError, subprocess.TimeoutExpired, IndexError):
        return "unknown"
    return "AC" if "AC Power" in first else first.strip()


def account(pid: int) -> dict | None:
    info = bench.rusage(pid)
    if info is None:
        return None
    return {
        "cpu_ms": bench.ticks_to_ms(info.ri_user_time + info.ri_system_time),
        "child_cpu_ms": bench.ticks_to_ms(info.ri_child_user_time + info.ri_child_system_time),
        "wakeups": info.ri_pkg_idle_wkups + info.ri_interrupt_wkups,
        "wall": time.time(),
        "mono": time.monotonic(),
    }


def sample_spawns(pid: int, seconds: int, out: Path) -> int | None:
    """Samples of the process that stand in posix_spawn (or its wrappers)."""
    path = out / "spawn.sample.txt"
    try:
        subprocess.run(["/usr/bin/sample", str(pid), str(seconds), "-file", str(path)],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=seconds + 30)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if not path.exists():
        return None
    count = 0
    for line in path.read_text(errors="replace").splitlines():
        if "posix_spawn" in line or "__posix_spawn" in line:
            digits = line.strip().split(" ", 1)[0]
            if digits.isdigit():
                count += int(digits)
    return count


def run(binary: str, label: str, seconds: int, rounds: int, channels: bool, sample: bool,
        base: Path) -> dict:
    stat = os.stat(binary)
    directory = Path(tempfile.mkdtemp(prefix=f"{label}-", dir=base))
    env = {
        "PATH": bench.BENCH_PATH,
        "HOME": str(base),
        "TMPDIR": str(base) + "/",
        "PANE_PROBE_DIR": str(directory),
        "PANE_PROBE_SECONDS": str(seconds),
        "PANE_PROBE_ROUNDS": str(rounds),
        "PANE_PROBE_CHANNELS": "1" if channels else "0",
    }
    result: dict = {
        "label": label, "seconds": seconds, "channels": channels, "power": power_source(),
        "load_before": os.getloadavg()[0],
        "binary": binary, "binary_ls": time.strftime("%b %d %H:%M", time.localtime(stat.st_mtime))
        + f" {stat.st_size}",
    }
    process = subprocess.Popen(
        [binary, "--exact", TEST, "--ignored", "--nocapture", "--test-threads=1"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env, text=True, bufsize=1)
    marks: dict[str, dict] = {}
    fields: dict[str, dict] = {}
    output: list[str] = []
    sampled: list[int | None] = []

    def reader() -> None:
        assert process.stdout is not None
        for line in process.stdout:
            output.append(line)
            if not line.startswith("PANE_PROBE "):
                continue
            _, tag, payload = line.split(" ", 2)
            fields[tag] = json.loads(payload)
            if tag in ("work_start", "work_end"):
                marks[tag] = account(process.pid) or {}
            if tag == "work_start" and sample:
                # Start after the parent has connected to a channel (`hold_after`).
                threading.Timer(min(4, seconds / 3), lambda: sampled.append(
                    sample_spawns(process.pid, 3, directory))).start()

    thread = threading.Thread(target=reader, daemon=True)
    thread.start()
    process.wait(timeout=seconds + rounds * 3 + 120)
    thread.join(timeout=5)
    time.sleep(0.2)
    (directory / "probe.out").write_text("".join(output))
    result["power_after"] = power_source()
    start, end = marks.get("work_start"), marks.get("work_end")
    if not (start and end and "cpu_ms" in start and "cpu_ms" in end):
        result["error"] = "the probe printed no work window"
        result["output_tail"] = "".join(output[-15:])
        return result
    wall = end["mono"] - start["mono"]
    spent = (end["cpu_ms"] - start["cpu_ms"]) + (end["child_cpu_ms"] - start["child_cpu_ms"])
    result.update({
        "window_s": round(wall, 2),
        "tmux_asks": fields["work_end"]["tmux_asks"],
        "spawns_per_s": round(fields["work_end"]["tmux_asks"] / wall, 2),
        "cpu_pct_of_one_core": round(spent / (wall * 10), 2),
        "self_cpu_ms": round(end["cpu_ms"] - start["cpu_ms"], 1),
        "child_cpu_ms": round(end["child_cpu_ms"] - start["child_cpu_ms"], 1),
        "wakeups_per_s": round((end["wakeups"] - start["wakeups"]) / wall, 1),
        "slept_s": round((end["wall"] - start["wall"]) - wall, 2),
        "latency": fields.get("latency"),
    })
    if sample:
        result["posix_spawn_samples_in_3s"] = sampled[0] if sampled else None
    judged = (result["power"] == "AC" and result["power_after"] == "AC"
              and abs(result["slept_s"]) < 0.5)
    result["judged"] = judged
    if not judged:
        result["not_judged"] = "battery" if "AC" not in (result["power"], result["power_after"]) else "slept"
    return result


def table(results: list[dict]) -> str:
    lines = ["| label | channels | tmux spawns/s | CPU % of a core | wakeups/s | posix_spawn samples/3 s | "
             "answer heard after (p50 / max ms) | not judged |",
             "|---|---|---|---|---|---|---|---|"]
    for one in results:
        if "error" in one:
            lines.append(f"| {one['label']} | {one['channels']} | error: {one['error']} | | | | | |")
            continue
        latency = one.get("latency") or {}
        lines.append(
            f"| {one['label']} | {'yes' if one['channels'] else 'no'} | {one['spawns_per_s']} | "
            f"{one['cpu_pct_of_one_core']} | {one['wakeups_per_s']} | "
            f"{one.get('posix_spawn_samples_in_3s', '-')} | "
            f"{round(latency.get('p50_ms', 0), 1)} / {round(latency.get('max_ms', 0), 1)} | "
            f"{one.get('not_judged', '')} |")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", required=True, help="the runtime crate's test binary")
    parser.add_argument("--label", default="main")
    parser.add_argument("--seconds", type=int, default=20, help="how long the three children work")
    parser.add_argument("--rounds", type=int, default=8, help="answer-latency rounds (three answers each)")
    parser.add_argument("--channels", choices=["yes", "no", "both"], default="both",
                        help="whether each child has a channel, as a child of this zo has")
    parser.add_argument("--runs", type=int, default=1)
    parser.add_argument("--sample", action="store_true", help="count posix_spawn samples with `sample`")
    parser.add_argument("--out", help="append one JSON line per run to this file")
    args = parser.parse_args()
    base = Path(tempfile.mkdtemp(prefix="pane-probe-"))
    modes = {"yes": [True], "no": [False], "both": [False, True]}[args.channels]
    results: list[dict] = []
    try:
        for _ in range(args.runs):
            for channels in modes:
                one = run(args.binary, args.label, args.seconds, args.rounds, channels, args.sample, base)
                results.append(one)
                line = json.dumps(one, ensure_ascii=False)
                print(line, flush=True)
                if args.out:
                    with open(args.out, "a", encoding="utf-8") as handle:
                        handle.write(line + "\n")
    finally:
        import shutil
        shutil.rmtree(base, ignore_errors=True)
    print()
    print(table(results))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
