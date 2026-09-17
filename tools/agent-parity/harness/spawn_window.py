#!/usr/bin/env python3
"""Files each product touches between the Agent tool_use flush and the child's
first provider request — the axis-A mechanism, read off mtimes."""
import json, os, sys
for run in sys.argv[1:]:
    d = os.path.join(os.environ.get("PARITY_ROOT", "/tmp/zo-agent-parity-20260907"), "runs", run)
    rows = [json.loads(l) for l in open(os.path.join(d, "requests.jsonl"))]
    spawn = [r for r in rows if r["kind"] == "parent" and not r["tool_uses"]][0]
    child = [r for r in rows if r["kind"] == "child"][0]
    t0, t1 = spawn["t_out"], child["t_in"]
    print("== %s   window %.1f ms" % (run, (t1 - t0) * 1000))
    hits = []
    for base, _, files in os.walk(os.path.join(d, "sandbox")):
        for f in files:
            p = os.path.join(base, f)
            try:
                st = os.stat(p)
            except OSError:
                continue
            if t0 - 0.05 <= st.st_mtime <= t1 + 0.05:
                hits.append((st.st_mtime, p, st.st_size))
    for m, p, sz in sorted(hits):
        print("   %+7.1f ms  %8d B  %s" % ((m - t0) * 1000, sz, p[len(d) + 1:]))
    print("   files: %d" % len(hits))
