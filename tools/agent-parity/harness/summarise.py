#!/usr/bin/env python3
"""min / median over a set of runs of one axis, per metric key."""
import json, os, statistics, subprocess, sys
ROOT = os.environ.get("PARITY_ROOT", "/tmp/zo-agent-parity-20260907")
sys.path.insert(0, os.path.join(ROOT, "harness"))
import analyze  # noqa: E402

keys = sys.argv[1].split(",")
runs = sys.argv[2:]
cols = {k: [] for k in keys}
for run in runs:
    d = run if os.path.isabs(run) else os.path.join(ROOT, "runs", run)
    _, _, _, m = analyze.analyse(d)
    for k in keys:
        v = m.get(k)
        if isinstance(v, (int, float)):
            cols[k].append(v)
for k, vals in cols.items():
    if not vals:
        print("%-34s no samples" % k)
        continue
    print("%-34s n=%d  min=%.1f  median=%.1f  max=%.1f   %s"
          % (k, len(vals), min(vals), statistics.median(vals), max(vals),
             [round(v, 1) for v in vals]))
