#!/usr/bin/env python3
"""tools/computer-bench/eye.py — what one look at the desktop costs the
Computer Use helper (docs/design/computer-use-bench.md, eye). Run inside a
ZeroCode pane. Looks only: no input, no pointer, nothing opened.

`cost`: N desktop `screenshot`s and N desktop `observe --diff`s, each timed
end to end and weighed in the helper's own CPU time (its process's `ps`
time before and after, divided by the looks); one OCR `wait-for` for a word
that is not on the screen, bounded by --wait-ms: how many looks it took, how
many of them read the screen, and the helper CPU they cost; --reads desktop
`read --ocr`s in a row, the first and the rest weighed apart (the rest read
again only what repainted); then --idle-s seconds of nothing, and the helper
CPU they cost — the continuous eye's own price while it stays open after
the looks. A helper that cannot be found (another machine, a test's
scripted shim) is reported as such, never as zero.
"""
import argparse
import json
import re
import statistics
import subprocess
import sys
import time
import uuid

SHIM = "zerocode-computer"
# The helper's executable inside the app bundle (macos.rs HELPER_EXECUTABLE).
HELPER_EXECUTABLE = "zerocode-computer-use-macos"
LOOKS_PATTERN = re.compile(r"\((\d+) looks, \d+ matches(?:, (\d+) reads)?")


def call(argv):
    started = time.perf_counter()
    out = subprocess.run([SHIM, *argv, "--json"], capture_output=True, text=True)
    elapsed = (time.perf_counter() - started) * 1000
    line = next((line for line in (out.stdout + out.stderr).splitlines() if line.strip().startswith("{")), "{}")
    answer = json.loads(line)
    if out.returncode != 0:
        answer["ok"] = False
    return elapsed, answer


def helper_pid():
    out = subprocess.run(["pgrep", "-f", f"Contents/MacOS/{HELPER_EXECUTABLE}"], capture_output=True, text=True)
    pids = [int(word) for word in out.stdout.split()]
    return pids[0] if len(pids) == 1 else None


def cpu_ms(pid):
    """A process's CPU time so far, in ms (`ps` time: [[h:]m:]s.cc)."""
    if pid is None:
        return None
    out = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True)
    text = out.stdout.strip()
    if not text:
        return None
    seconds = 0.0
    for part in text.split(":"):
        seconds = seconds * 60 + float(part)
    return seconds * 1000


def percentile(values, share):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(round(share * (len(ordered) - 1))))]


def looks(argv, n, pid):
    walls, failed = [], 0
    before = cpu_ms(pid)
    for _ in range(n):
        ms, answer = call(argv)
        if answer.get("ok"):
            walls.append(ms)
        else:
            failed += 1
    after = cpu_ms(pid)
    report = {"look": " ".join(argv), "n": n, "failed": failed}
    if walls:
        report.update(wall_p50_ms=round(statistics.median(walls), 1), wall_p95_ms=round(percentile(walls, 0.95), 1))
    report["helper_cpu_ms_per_look"] = round((after - before) / n, 1) if before is not None and after is not None else None
    return report


def ocr_wait(wait_ms, pid):
    absent = f"zc-eye-{uuid.uuid4().hex[:12]}"
    before = cpu_ms(pid)
    ms, answer = call(["wait-for", "--text", absent, "--ocr", "--timeout-ms", str(wait_ms)])
    after = cpu_ms(pid)
    found = LOOKS_PATTERN.search(json.dumps(answer))
    looks_taken = int(found.group(1)) if found else None
    # Before the eye every look read the screen; the message says reads once it gates them.
    reads = int(found.group(2)) if found and found.group(2) else looks_taken
    return {
        "wait": f"wait-for --text <absent> --ocr --timeout-ms {wait_ms}",
        "code": (answer.get("error") or {}).get("code") if not answer.get("ok") else "found",
        "wall_ms": round(ms, 1),
        "looks": looks_taken,
        "reads": reads,
        "helper_cpu_ms": round(after - before, 1) if before is not None and after is not None else None,
    }


def reads(n, pid):
    """Desktop OCR reads in a row: the first, and the median of the rest."""
    walls, cpus, lines = [], [], []
    for _ in range(n):
        before = cpu_ms(pid)
        ms, answer = call(["read", "--ocr"])
        after = cpu_ms(pid)
        if answer.get("ok"):
            walls.append(ms)
            cpus.append(after - before if before is not None and after is not None else None)
            lines.append(len((answer.get("result") or {}).get("lines") or []))
    rest = [cpu for cpu in cpus[1:] if cpu is not None]
    return {
        "read": "read --ocr", "n": n, "answered": len(walls),
        "first_wall_ms": round(walls[0], 1) if walls else None,
        "first_helper_cpu_ms": round(cpus[0], 1) if cpus and cpus[0] is not None else None,
        "rest_wall_p50_ms": round(statistics.median(walls[1:]), 1) if len(walls) > 1 else None,
        "rest_helper_cpu_p50_ms": round(statistics.median(rest), 1) if rest else None,
        "lines": lines,
    }


def idle(seconds, pid):
    """The helper's CPU while nothing is asked of it, per second."""
    before = cpu_ms(pid)
    time.sleep(seconds)
    after = cpu_ms(pid)
    per_second = round((after - before) / seconds, 1) if before is not None and after is not None and seconds else None
    return {"idle_s": seconds, "helper_cpu_ms_per_s": per_second}


def cost(args):
    # A cold window starts its helper on the first look. Discovering the PID
    # before that look leaves every later CPU sample unknown. Keep startup
    # separate from the warm samples, and do not manufacture a zero CPU cost.
    warm_ms, warm_answer = call(["screenshot"])
    pid = helper_pid()
    report = {
        "warmup": {"look": "screenshot", "wall_ms": round(warm_ms, 1),
                   "ok": warm_answer.get("ok") is True},
        "helper": pid if pid is not None else f"no single {HELPER_EXECUTABLE} process: CPU not measured",
        "looks": [looks(["screenshot"], args.n, pid), looks(["observe", "--diff"], args.n, pid)],
        "ocr_wait": ocr_wait(args.wait_ms, pid),
        "ocr_reads": reads(args.reads, pid),
        "idle": idle(args.idle_s, pid),
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["warmup"]["ok"] and not any(row["failed"] for row in report["looks"]) else 1


def main(argv):
    parser = argparse.ArgumentParser(prog="eye.py", description=__doc__.split("\n\n")[0])
    parser.add_argument("mode", choices=["cost"])
    parser.add_argument("--n", type=int, default=20)
    parser.add_argument("--wait-ms", type=int, default=5000)
    parser.add_argument("--idle-s", type=float, default=10.0)
    parser.add_argument("--reads", type=int, default=6)
    args = parser.parse_args(argv[1:])
    return cost(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
