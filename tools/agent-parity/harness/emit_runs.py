#!/usr/bin/env python3
"""One runs.jsonl row per run, fields per agent-workflow-quality-baseline.md §2.

`task` carries the axis id (A–J) and `surface` the subject, since this suite
measures agent axes rather than the Q1–Q6 fixtures; every other field name and
shape is the baseline's.
"""
import json, os, platform, subprocess, sys, time

ROOT = os.environ.get("PARITY_ROOT", "/tmp/zo-agent-parity-20260907")
sys.path.insert(0, os.path.join(ROOT, "harness"))
import analyze  # noqa: E402

# The checkout whose sha the rows carry: the one it is run from unless
# `PARITY_GIT` names another.
GIT = os.environ.get("PARITY_GIT", os.getcwd())


def sh(*args):
    try:
        return subprocess.run(args, capture_output=True, text=True,
                              cwd=GIT).stdout.strip()
    except Exception:
        return ""


def host():
    mem = None
    try:
        mem = int(subprocess.run(["sysctl", "-n", "hw.memsize"],
                                 capture_output=True, text=True).stdout.strip())
    except Exception:
        pass
    return {"os": sys.platform, "cpu": platform.machine(), "mem": mem}


def build_of(subject):
    if subject == "zo":
        return {"git_sha": sh("git", "rev-parse", "--short", "HEAD"),
                "dirty": bool(sh("git", "status", "--porcelain")), "profile": "release"}
    return {"git_sha": None, "dirty": None, "profile": "claude-cli-" + os.environ.get("PARITY_CLAUDE_VERSION", "unknown")}


def row(run_dir):
    axis, drv, reqs, metrics = analyze.analyse(run_dir)
    subject = drv.get("subject")
    if drv.get("panes"):
        subject = subject + "-pane"
    parents = [r for r in reqs if r.get("kind") == "parent"]
    first_visible = None
    if parents:
        first_visible = round((parents[0]["t_out"] - parents[0]["t_in"]) * 1000, 3)
    tokens_in = sum(r.get("raw_len", 0) for r in reqs) // 4
    verdict = metrics.get("pass")
    if verdict is None:
        verdict = drv.get("outcome") == "done"
    disk = 0.0
    sb = os.path.join(run_dir, "sandbox")
    if os.path.isdir(sb):
        total = 0
        for base, _, files in os.walk(sb):
            for f in files:
                try:
                    total += os.path.getsize(os.path.join(base, f))
                except OSError:
                    pass
        disk = round(total / 1_000_000, 3)
    return {
        "task": axis, "surface": subject,
        "build": build_of(drv.get("subject")),
        "protocol": {"events": None,
                     "capabilities": sorted({t for r in reqs
                                             for t in (r.get("tools") or [])})},
        "model": {"requested": "scripted", "effective": "scripted-anthropic",
                  "effort": None},
        "account": {"provider": "scripted-anthropic", "label": "hermetic"},
        "host": host(),
        "verified": bool(verdict), "interventions": 0,
        "elapsed_ms": int(round(drv.get("elapsed_s", 0) * 1000)),
        "first_visible_ms": first_visible,
        "stream": {"queue_max": None, "paint_p95_ms": None},
        "tokens": {"in": tokens_in, "out": 0, "cached": 0},
        "cost_usd": 0.0, "rss_peak_mb": None, "disk_delta_mb": disk,
        "lane": "deterministic", "seed": 20260907,
        "repeat": 1, "reason": None,
        "axis_metrics": metrics,
        "evidence": {"requests": os.path.join(run_dir, "requests.jsonl"),
                     "driver": os.path.join(run_dir, "driver.json"),
                     "capture": os.path.join(run_dir, "%s.capture" % subject)},
    }


def main():
    out = os.path.join(ROOT, "fixtures", "runs.jsonl")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    rows = []
    for name in sorted(sys.argv[1:]):
        d = name if os.path.isabs(name) else os.path.join(ROOT, "runs", name)
        if not os.path.isfile(os.path.join(d, "driver.json")):
            print("skip (no driver.json):", name, file=sys.stderr)
            continue
        rows.append(row(d))
    with open(out, "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    print("wrote %d rows to %s" % (len(rows), out))


if __name__ == "__main__":
    main()
