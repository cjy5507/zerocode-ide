#!/usr/bin/env python3
"""Step effort governor A/B (t-5633 §6): six tasks in ONE resumed zo session,
arms crossed (A B B A A B), the governor's word toggled in the project's
`.zo/settings.json` between turns. A = no word (table in shadow, nothing
applied), B = `on`.

    run.py run --zo <bin> --model <id> --out <dir>
    run.py analyze --out <dir>

The child zo inherits this environment. It is launched without `CODEX_HOME`,
and since t-5777 that is fine: zo follows the account the ZeroCode window is
signed in to. Export `CODEX_HOME=<a codex home>` here only to measure a
DIFFERENT OpenAI account than the window's.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
SEED = HERE / "seed"

TASKS = [
    "Add a `median(values)` function to calc/core.py (raise ValueError on an empty list) and a unittest for it in tests/test_core.py. Run `python3 -m unittest -q` and make sure everything passes.",
    "`parse_duration` in calc/core.py returns the wrong number of seconds for hours (`1h30m` should be 5400). Fix it, add a regression test in tests/test_core.py, and run `python3 -m unittest -q`.",
    "Add a `Stack` class to calc/core.py with push/pop/peek/__len__; pop and peek raise IndexError on an empty stack. Add tests and run `python3 -m unittest -q`.",
    "Change `word_count` in calc/core.py to ignore case and surrounding punctuation ('Hello, hello!' counts as {'hello': 2}). Update the tests and run `python3 -m unittest -q`.",
    "Add calc/__main__.py so `python3 -m calc 3 1 2` prints the median (2) of its arguments. Add a test that runs it with subprocess and run `python3 -m unittest -q`.",
    "`paginate` in calc/core.py is documented as 1-based but behaves 0-based (page 1 skips the first page). Fix the code so page 1 is the first page, fix the existing test accordingly, add one for page 2, and run `python3 -m unittest -q`.",
]

# Crossed order: the arm of each task, and the reverse for a second model.
ARMS = ["A", "B", "B", "A", "A", "B"]


def settings_for(arm: str) -> dict:
    return {"smart": {"zoStepEffort": "on"}} if arm == "B" else {}


def newest_session(cache_root: Path, since: float) -> str | None:
    picks = [p for p in cache_root.glob("session-*") if p.stat().st_mtime >= since]
    picks.sort(key=lambda p: p.stat().st_mtime)
    return picks[-1].name if picks else None


def run(args: argparse.Namespace) -> None:
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    proj = out / "proj"
    if proj.exists():
        shutil.rmtree(proj)
    shutil.copytree(SEED, proj)
    (proj / ".zo").mkdir(exist_ok=True)
    subprocess.run(["git", "init", "-q"], cwd=proj, check=True)
    subprocess.run(["git", "add", "-A"], cwd=proj, check=True)
    subprocess.run(["git", "-c", "user.email=ab@x", "-c", "user.name=ab", "commit", "-qm", "seed"], cwd=proj, check=True)
    home = Path(os.environ.get("ZO_CONFIG_HOME") or Path.home() / ".zo")
    cache_root = home / "cache" / "prompt-cache"
    arms = list(reversed(ARMS)) if args.reverse else ARMS
    session = None
    log = []
    for index, (task, arm) in enumerate(zip(TASKS, arms), start=1):
        (proj / ".zo" / "settings.json").write_text(json.dumps(settings_for(arm)) + "\n")
        cmd = [
            args.zo, "-p", "--no-spawn", "--effort", "smart", "--model", args.model,
            "--permission-mode", "danger-full-access", "--cwd", str(proj),
            "--last-message", str(out / f"turn-{index}.last.txt"),
        ]
        if session:
            cmd += ["--resume", session]
        started = time.time()
        proc = subprocess.run(cmd, input=task + "\n", text=True, capture_output=True, cwd=proj, timeout=1800)
        ended = time.time()
        elapsed = ended - started
        (out / f"turn-{index}.stdout.txt").write_text(proc.stdout)
        (out / f"turn-{index}.stderr.txt").write_text(proc.stderr)
        if session is None:
            session = newest_session(cache_root, started - 5)
        row = {"turn": index, "arm": arm, "elapsed_s": round(elapsed, 1), "rc": proc.returncode, "session": session,
               "started_ms": int(started * 1000), "ended_ms": int(ended * 1000)}
        print(json.dumps(row), flush=True)
        log.append(row)
        tests = subprocess.run(["python3", "-m", "unittest", "-q"], cwd=proj, capture_output=True, text=True)
        row["tests_green"] = tests.returncode == 0
        subprocess.run(["git", "add", "-A"], cwd=proj, check=True)
        subprocess.run(["git", "-c", "user.email=ab@x", "-c", "user.name=ab", "commit", "-qm", f"turn {index}"], cwd=proj)
    (out / "turns.json").write_text(json.dumps({"model": args.model, "session": session, "turns": log}, indent=1))


def read_jsonl(path: Path) -> list[dict]:
    if not path.exists():
        return []
    rows = []
    for line in path.read_text().splitlines():
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return rows


def find_state_files(name: str, since: float) -> list[Path]:
    home = Path(os.environ.get("ZO_CONFIG_HOME") or Path.home() / ".zo")
    found = []
    for p in (home / "projects").rglob(name):
        if p.stat().st_mtime >= since:
            found.append(p)
    return found


def analyze(args: argparse.Namespace) -> None:
    out = Path(args.out)
    meta = json.loads((out / "turns.json").read_text())
    session = meta["session"]
    home = Path(os.environ.get("ZO_CONFIG_HOME") or Path.home() / ".zo")
    requests = read_jsonl(home / "cache" / "prompt-cache" / session / "requests.jsonl")
    since = (out / "turns.json").stat().st_mtime - 6 * 3600
    timings = []
    for p in find_state_files("timings.jsonl", since):
        timings += [r for r in read_jsonl(p) if r.get("session_id") == session]
    steps = []
    for p in find_state_files("step-effort-zo.jsonl", since):
        steps += read_jsonl(p)
    print(f"model={meta['model']} session={session} requests={len(requests)} timings={len(timings)} step_rows={len(steps)}")
    header = ("turn", "arm", "req", "input", "cache_rd", "cache_wr", "rewrites", "output", "rd%", "ttfb_ms", "wall_s", "efforts", "deltas", "green")
    print(" | ".join(header))
    totals: dict[str, dict] = {"A": {}, "B": {}}
    for turn in meta["turns"]:
        lo, hi = turn["started_ms"], turn["ended_ms"] + 2000
        rows = [r for r in requests if lo <= r.get("ts_unix_ms", 0) <= hi]
        attempts = {r.get("attempt") for r in rows}
        inp = sum(r.get("input_uncached", 0) for r in rows)
        rd = sum(r.get("cache_read", 0) for r in rows)
        wr = sum(r.get("cache_creation", 0) for r in rows)
        outp = sum(r.get("output", 0) for r in rows)
        denom = inp + rd + wr
        share = (100.0 * rd / denom) if denom else 0.0
        tt = [t.get("ttfb_ms") for t in timings if t.get("attempt") in attempts and t.get("ttfb_ms") is not None]
        ttfb = sorted(tt)[len(tt) // 2] if tt else None
        efforts = ",".join(sorted({r.get("effort", "") for r in rows}))
        srows = [s for s in steps if s.get("kind") == "step" and lo <= s.get("at", 0) <= hi]
        deltas = "".join({-1: "-", 0: ".", 1: "+", 2: "^"}.get(s.get("delta"), "?") for s in srows)
        rewrites = sum(1 for r in rows if r.get("cache_creation", 0) > 10_000)
        print(" | ".join(str(x) for x in (
            turn["turn"], turn["arm"], len(rows), inp, rd, wr, rewrites, outp, f"{share:.0f}", ttfb, turn["elapsed_s"], efforts, deltas, turn.get("tests_green"),
        )))
        t = totals[turn["arm"]]
        for k, v in (("req", len(rows)), ("input", inp), ("rd", rd), ("wr", wr), ("rewrites", rewrites), ("out", outp), ("wall", turn["elapsed_s"]), ("turns", 1)):
            t[k] = t.get(k, 0) + v
        t.setdefault("ttfb", []).extend(tt)
    for arm, t in totals.items():
        denom = t.get("input", 0) + t.get("rd", 0) + t.get("wr", 0)
        share = (100.0 * t.get("rd", 0) / denom) if denom else 0.0
        tt = sorted(t.get("ttfb", []))
        med = tt[len(tt) // 2] if tt else None
        print(f"arm {arm}: turns={t.get('turns', 0)} req={t.get('req', 0)} input={t.get('input', 0)} cache_rd={t.get('rd', 0)} cache_wr={t.get('wr', 0)} rewrites={t.get('rewrites', 0)} output={t.get('out', 0)} rd%={share:.1f} ttfb_med={med} wall={t.get('wall', 0):.0f}s")
    applied = [s for s in steps if s.get("kind") == "step" and s.get("applied")]
    moves = [s for s in steps if s.get("kind") == "step" and s.get("move")]
    reasons: dict[str, int] = {}
    for s in steps:
        if s.get("kind") == "step":
            reasons[s.get("reason")] = reasons.get(s.get("reason"), 0) + 1
    print(f"step rows: {len([s for s in steps if s.get('kind') == 'step'])} applied={len(applied)} moves={len(moves)} reasons={reasons}")
    judgments = [s for s in steps if s.get("kind") == "judgment"]
    print(f"judgment rows: {len(judgments)} outcomes={ {j.get('outcome'): 1 for j in judgments} }")


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--zo", required=True)
    r.add_argument("--model", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--reverse", action="store_true")
    a = sub.add_parser("analyze")
    a.add_argument("--out", required=True)
    args = parser.parse_args()
    if args.cmd == "run":
        run(args)
    else:
        analyze(args)


if __name__ == "__main__":
    main()
