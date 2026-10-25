#!/usr/bin/env python3
"""Direct against selective delegation, task by task: the harness of t-73910 A17·A24 (W4-3).

The same scenario is run under two conditions with one agent, model and effort for both:

    direct     one worker does the whole task itself and summons nobody
    delegate   the same worker may summon workers through zerocode-orc where splitting
               helps, and does the work itself where it does not

Nothing here spends a request. `plan` writes the runs in a seeded random order, each with
its brief, and prints the zerocode-orc lines a coordinator types for them; the runs are the
coordinator's to start, after the person has set the model, effort and budget.

    eval.py plan --scenarios scenarios.json --reps 3 --seed 1 --agent claude \\
                 --model <id> --effort <level> --out DIR
    eval.py prepare --out DIR             a fresh git checkout of each run's seed
    eval.py accept --out DIR [--run R]    the scenario's acceptance command in the checkout,
                                          and the files it changed outside the allowed ones
    eval.py collect --out DIR --run R --task t-1 [--task t-2 …]
                                          `zerocode-orc dispatch-show` of each task the run
                                          made (a read), saved as dispatch-<task>.json
    eval.py summarize --out DIR [--json]  the table, per scenario and condition

What `summarize` reads, and what it says when it is not there:
  * accept.json — passed only with rc 0 and nothing changed outside `files`. A run with no
    accept.json is not passed: every planned run stays in the denominator.
  * dispatch-*.json — the wall clock from the first attempt's start to the latest
    verification (`afterReport.verifiedMs`), attempts (rework = attempts beyond the first)
    and the most attempts that ran at once. A task with no verification leaves the run's
    wall clock unknown.
  * cost-*.json — a task's TaskCost as the window serializes it. Tokens count only where the
    history is complete and every conversation is linked (`GenerationCost::measured_tokens`);
    dollars only where `usd` is set. Anything else, or no file, is unknown with its reason —
    never 0.
  * the person's interventions have no ledger column: always unknown.
"""

import argparse
import json
import os
import random
import shlex
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
CONDITIONS = {
    "direct": (
        "Do this whole task yourself, in the checkout named below. Do not summon, delegate to "
        "or start any other agent."
    ),
    "delegate": (
        "Do this task in the checkout named below. You may summon worker agents through "
        "zerocode-orc where splitting the work helps; do it yourself where it does not."
    ),
}
GIT_ENV = {"GIT_AUTHOR_NAME": "eval", "GIT_AUTHOR_EMAIL": "eval@example.invalid",
           "GIT_COMMITTER_NAME": "eval", "GIT_COMMITTER_EMAIL": "eval@example.invalid"}


def load(path):
    return json.loads(Path(path).read_text())


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=1, ensure_ascii=False) + "\n")


def plan(args):
    scenarios = load(args.scenarios)["scenarios"]
    out = Path(args.out).resolve()
    if (out / "plan.json").exists():
        sys.exit(f"{out}/plan.json exists: a plan is written once")
    runs = [(s, condition, rep) for s in scenarios for condition in CONDITIONS for rep in range(1, args.reps + 1)]
    random.Random(args.seed).shuffle(runs)
    planned = []
    for order, (scenario, condition, rep) in enumerate(runs, 1):
        run = f"r{order:02d}-{scenario['id']}-{condition}-{rep}"
        folder = out / run
        folder.mkdir(parents=True)
        checkout = folder / "checkout"
        brief = f"{CONDITIONS[condition]}\n\nCheckout: {checkout}\n\n{scenario['brief']}\n"
        (folder / "brief.md").write_text(brief)
        planned.append({"run": run, "order": order, "scenario": scenario["id"], "condition": condition,
                        "rep": rep, "checkout": str(checkout)})
    write(out / "plan.json", {
        "agent": args.agent, "model": args.model, "effort": args.effort, "seed": args.seed,
        "reps": args.reps, "scenarios": str(Path(args.scenarios).resolve()), "runs": planned,
    })
    for one in planned:
        brief = f'"$(cat {shlex.quote(str(out / one["run"] / "brief.md"))})"'
        print(f"# {one['run']}")
        print(shlex.join(["zerocode-orc", "task-create", "--retry-request", f"eval-{one['run']}-task",
                          "--title", f"eval {one['run']}", "--spec"]) + f" {brief}")
        print(shlex.join(["zerocode-orc", "worker-start", "--retry-request", f"eval-{one['run']}-start",
                          "--agent", args.agent, "--task", "<task id>", "--model", args.model,
                          "--effort", args.effort, "--prompt"]) + f" {brief}")
    return 0


def scenario_of(plan_doc, run):
    scenarios = {s["id"]: s for s in load(plan_doc["scenarios"])["scenarios"]}
    return scenarios[run["scenario"]]


def runs_of(out, only=None):
    doc = load(Path(out) / "plan.json")
    runs = [r for r in doc["runs"] if only in (None, r["run"])]
    if only and not runs:
        sys.exit(f"no run {only} in the plan")
    return doc, runs


def git(cwd, *args):
    return subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True, check=True,
                          env={**os.environ, **GIT_ENV}).stdout


def prepare(args):
    doc, runs = runs_of(args.out)
    scenarios_dir = Path(doc["scenarios"]).parent
    for run in runs:
        checkout = Path(run["checkout"])
        if checkout.exists():
            print(f"{run['run']}: kept, already prepared")
            continue
        shutil.copytree(scenarios_dir / scenario_of(doc, run)["seed"], checkout)
        git(checkout, "init", "-q", "-b", "main")
        git(checkout, "add", "-A")
        git(checkout, "commit", "-q", "-m", "seed")
        print(f"{run['run']}: {checkout} at {git(checkout, 'rev-parse', 'HEAD').strip()}")
    return 0


def accept(args):
    doc, runs = runs_of(args.out, args.run)
    for run in runs:
        checkout = Path(run["checkout"])
        if not checkout.exists():
            print(f"{run['run']}: not prepared")
            continue
        scenario = scenario_of(doc, run)
        began = time.monotonic()
        said = subprocess.run(scenario["accept"], cwd=checkout, capture_output=True, text=True)
        # What the run changed since the seed, committed or not, and the files it added.
        seed = git(checkout, "rev-list", "--max-parents=0", "HEAD").split()[0]
        changed = set(git(checkout, "diff", "--name-only", seed).split())
        changed |= set(git(checkout, "ls-files", "--others", "--exclude-standard").split())
        outside = sorted(p for p in changed if p not in scenario["files"] and not p.endswith(".pyc"))
        result = {"rc": said.returncode, "outside": outside, "tookS": round(time.monotonic() - began, 2),
                  "passed": said.returncode == 0 and not outside}
        write(Path(args.out) / run["run"] / "accept.json", result)
        print(f"{run['run']}: rc={result['rc']} outside={len(outside)} passed={result['passed']}")
    return 0


def collect(args):
    _, runs = runs_of(args.out, args.run)
    folder = Path(args.out) / runs[0]["run"]
    for task in args.task:
        said = subprocess.run(["zerocode-orc", "dispatch-show", "--task", task], capture_output=True, text=True)
        if said.returncode != 0:
            print(f"{task}: dispatch-show rc={said.returncode}: {said.stderr.strip()[:200]}")
            continue
        (folder / f"dispatch-{task}.json").write_text(said.stdout)
        print(f"{task}: saved")
    return 0


def run_facts(folder):
    """One run's numbers, each None where its source is missing, with why."""
    facts = {"passed": False, "outcome": "unknown: no accept.json", "wallMs": None, "attempts": None,
             "rework": None, "concurrent": None, "tokens": None, "usd": None, "why": []}
    accepted = folder / "accept.json"
    if accepted.exists():
        verdict = load(accepted)
        facts["passed"] = bool(verdict.get("passed"))
        facts["outcome"] = "passed" if facts["passed"] else f"failed: rc={verdict.get('rc')} outside={verdict.get('outside')}"
    dispatches = [load(p) for p in sorted(folder.glob("dispatch-*.json"))]
    if not dispatches:
        facts["why"].append("no dispatch-show saved")
    else:
        tasks = [d.get("time", {}).get("task", {}) for d in dispatches]
        starts = [t.get("firstStartedMs") for t in tasks]
        verified = [(t.get("afterReport") or {}).get("verifiedMs") for t in tasks]
        if None in starts or None in verified:
            facts["why"].append("a task has no start or no verification")
        else:
            facts["wallMs"] = max(verified) - min(starts)
        attempts = [t.get("attempts") for t in tasks]
        if None not in attempts:
            facts["attempts"] = sum(attempts)
            facts["rework"] = sum(max(a - 1, 0) for a in attempts)
        spans = [(d.get("startedMs"), d.get("endedMs")) for d in dispatches]
        if all(s is not None and e for s, e in spans):
            facts["concurrent"] = max(sum(1 for s2, e2 in spans if s2 <= s < e2) for s, _ in spans)
    costs = [load(p) for p in sorted(folder.glob("cost-*.json"))]
    if not dispatches or len(costs) != len(dispatches):
        facts["why"].append(f"cost read for {len(costs)} of {len(dispatches)} tasks")
    else:
        tokens, usd = 0, 0.0
        for cost in costs:
            generation = cost.get("generation") or {}
            known, linked = generation.get("sessionsKnown", 0), generation.get("sessionsLinked", 0)
            if tokens is not None and generation.get("historyComplete") and linked and linked == known:
                tokens += sum(generation.get(k, 0) for k in ("inputTokens", "outputTokens", "cacheReadTokens", "cacheWriteTokens"))
            else:
                tokens = None
            if usd is not None and generation.get("usd") is not None:
                usd += generation["usd"]
            else:
                if usd is not None:
                    facts["why"].append(f"dollars: {generation.get('usdReason') or 'not read'}")
                usd = None
        facts["tokens"], facts["usd"] = tokens, usd
    return facts


def middle(values):
    known = [v for v in values if v is not None]
    if len(known) != len(values) or not known:
        return None
    return statistics.median(known)


def summarize(args):
    doc, runs = runs_of(args.out)
    rows = {}
    for run in runs:
        rows.setdefault((run["scenario"], run["condition"]), []).append(run_facts(Path(args.out) / run["run"]))
    table = []
    for (scenario, condition), facts in sorted(rows.items()):
        walls = [f["wallMs"] for f in facts]
        table.append({
            "scenario": scenario, "condition": condition, "runs": len(facts),
            "passed": sum(f["passed"] for f in facts),
            "wallMedianMs": middle(walls),
            "wallSlowestMs": max(walls) if None not in walls else None,
            "tokensMedian": middle([f["tokens"] for f in facts]),
            "usdMedian": middle([f["usd"] for f in facts]),
            "rework": sum(f["rework"] for f in facts) if None not in [f["rework"] for f in facts] else None,
            "mostAtOnce": max(f["concurrent"] for f in facts) if None not in [f["concurrent"] for f in facts] else None,
            "interventions": None,
            "why": sorted({w for f in facts for w in f["why"]}),
        })
    if args.json:
        print(json.dumps({"model": doc["model"], "effort": doc["effort"], "rows": table}, indent=1))
        return 0
    show = lambda v: "unknown" if v is None else (f"{v:.4g}" if isinstance(v, float) else str(v))
    print(f"agent {doc['agent']} · model {doc['model']} · effort {doc['effort']} · seed {doc['seed']}")
    print("| scenario | condition | passed | wall median ms | wall slowest ms | tokens median | usd median | rework | most at once | interventions |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for row in table:
        print(f"| {row['scenario']} | {row['condition']} | {row['passed']}/{row['runs']} | {show(row['wallMedianMs'])} | "
              f"{show(row['wallSlowestMs'])} | {show(row['tokensMedian'])} | {show(row['usdMedian'])} | "
              f"{show(row['rework'])} | {show(row['mostAtOnce'])} | unknown |")
    for row in table:
        for why in row["why"]:
            print(f"unknown in {row['scenario']}/{row['condition']}: {why}")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    verbs = parser.add_subparsers(dest="verb", required=True)
    p = verbs.add_parser("plan")
    p.add_argument("--scenarios", default=str(HERE / "scenarios.json"))
    p.add_argument("--reps", type=int, required=True)
    p.add_argument("--seed", type=int, required=True)
    p.add_argument("--agent", required=True)
    p.add_argument("--model", required=True, help="the person's choice; the same for both conditions")
    p.add_argument("--effort", required=True, help="the person's choice; the same for both conditions")
    p.add_argument("--out", required=True)
    for name in ("prepare", "accept", "collect", "summarize"):
        q = verbs.add_parser(name)
        q.add_argument("--out", required=True)
        if name in ("accept", "collect"):
            q.add_argument("--run", required=(name == "collect"))
        if name == "collect":
            q.add_argument("--task", action="append", required=True)
        if name == "summarize":
            q.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    return {"plan": plan, "prepare": prepare, "accept": accept, "collect": collect, "summarize": summarize}[args.verb](args)


if __name__ == "__main__":
    sys.exit(main())
