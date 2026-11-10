#!/usr/bin/env python3
"""Run the bench tasks against a zo binary and score each run.

One run = one task, one model, one harness arm:

1. copy the task's `repo/` to a fresh directory and commit it, so the model
   works in a real git tree;
2. run `zo --json` there with the task's `prompt.txt`;
3. let the task's `check.sh <workdir>` judge the result (it copies the tree and
   runs a verifier the model never saw). The verifier is also handed the
   model's last message, in a file kept outside the tree, so a task can ask a
   question and judge the answer without making the model write it down.

An arm is a catalog overlay handed to zo as the operator's export of
`ZO_MODEL_CONTEXT_WINDOWS` (`--arm name=<json>`), so an experiment changes one
harness field without a build. With no `--arm`, one arm runs: `shipped`.

Output: one JSON line per run (`--out`), and a table per (model, arm) on
stdout. Token columns are zo's session-cumulative `usage` frame; turning them
into money is left to the price table of whoever reads them.

`requests` is how many times the model was asked — the round trips of the
run. Every one re-reads the whole conversation, so on a long session it is
what a harness change mostly moves: a tool looked up before it is called, a
file read before it is edited, a rule searched for that could have been in
the prompt. Pass rate does not see any of that (every arm passes the easy
tasks), which is what tasks 27–29 are for: each has one answer and a road to
it that a harness can make shorter. Compare two binaries with two runs, each
with an `--out` file of its own.

Standard library only. Run it with `python3 -I`.
"""

import argparse
import json
import math
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

CATALOG_ENV = "ZO_MODEL_CONTEXT_WINDOWS"
# The window plants its vault into every pane; a bench run is not a pane's
# session and must not recall the developer's notes — nor write to them, which
# a session told of a vault does. The variable is set to zo's own "off" word
# and not merely removed: with it absent, a vault that settings declare is
# connected all the same.
VAULT_ENV = "ZEROCODE_SECOND_BRAIN"
VAULT_OFF = "off"
GIT_IDENTITY = ("-c", "user.name=bench", "-c", "user.email=bench@example.invalid")


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--zo", required=True, help="zo binary to run")
    parser.add_argument("--model", action="append", required=True, help="model id (repeatable)")
    parser.add_argument("--effort", default=None, help="effort for every run")
    parser.add_argument("--arm", action="append", default=[], help="name=<catalog overlay json>")
    parser.add_argument("--tasks", default=str(Path(__file__).with_name("tasks")))
    parser.add_argument("--only", default=None, help="comma-separated task ids")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--timeout", type=int, default=900, help="seconds per run")
    parser.add_argument("--max-turns", type=int, default=40)
    parser.add_argument("--out", required=True, help="JSONL results file (appended)")
    return parser.parse_args(argv)


def arms_of(specs):
    if not specs:
        return [("shipped", None)]
    arms = []
    for spec in specs:
        name, _, overlay = spec.partition("=")
        json.loads(overlay)  # refuse a malformed arm before any run is paid for
        arms.append((name, overlay))
    return arms


def tasks_in(root, only):
    wanted = set(only.split(",")) if only else None
    for task in sorted(Path(root).iterdir()):
        meta_path = task / "meta.json"
        if not (task / "prompt.txt").is_file() or not meta_path.is_file():
            continue
        meta = json.loads(meta_path.read_text())
        if wanted is None or meta.get("id") in wanted:
            yield task, meta


def prepare_workdir(task):
    work = Path(tempfile.mkdtemp(prefix=f"zo-bench-{task.name}-"))
    shutil.copytree(task / "repo", work, dirs_exist_ok=True)
    for args in (["init", "-q"], ["add", "-A"], ["commit", "-qm", "task"]):
        subprocess.run(["git", *GIT_IDENTITY, *args], cwd=work, check=True, capture_output=True)
    return work


def run_zo(args, model, overlay, work, prompt, answer):
    env = dict(os.environ)
    env[VAULT_ENV] = VAULT_OFF
    if overlay is None:
        env.pop(CATALOG_ENV, None)
    else:
        env[CATALOG_ENV] = overlay
    command = [
        args.zo, "--json", "--model", model, "--permission-mode", "workspace-write",
        "--max-turns", str(args.max_turns), "--cwd", str(work),
        "--last-message", str(answer),
    ]
    if args.effort:
        command += ["--effort", args.effort]
    command.append(prompt)
    started = time.monotonic()
    try:
        done = subprocess.run(
            command, cwd=work, env=env, capture_output=True, text=True,
            timeout=args.timeout, stdin=subprocess.DEVNULL,
        )
        return done.returncode, done.stdout, done.stderr, time.monotonic() - started, False
    except subprocess.TimeoutExpired as expired:
        stdout = expired.stdout.decode() if isinstance(expired.stdout, bytes) else expired.stdout or ""
        return None, stdout, "", time.monotonic() - started, True


def read_events(stdout):
    """What the run did, from zo's `--json` lines: `usage` carries the
    session's running `total_tokens`, `tool_call`/`tool_result` share an
    `id`, `wire_model` names each model a request went to, `stream_phase`
    says when a request is sent, and `result` closes the run.

    A request is counted at its first attempt: a retry after a dropped
    connection sends the same request again and is not a round trip the model
    asked for."""
    total_tokens = 0
    requests = 0
    calls = {}
    tool_errors = 0
    models = []
    status = None
    error = None
    for line in stdout.splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        kind = event.get("type")
        if kind == "usage":
            total_tokens = max(total_tokens, event.get("total_tokens") or 0)
        elif kind == "tool_call":
            calls[event.get("id")] = event.get("name")
        elif kind == "tool_result" and event.get("is_error"):
            tool_errors += 1
        elif kind == "stream_phase":
            if event.get("phase") == "request_sent" and event.get("attempt", 1) == 1:
                requests += 1
        elif kind == "wire_model" and event.get("model") not in models:
            models.append(event.get("model"))
        elif kind == "result":
            status, error = event.get("status"), event.get("error")
    by_tool = {}
    for name in calls.values():
        by_tool[name] = by_tool.get(name, 0) + 1
    return {
        "total_tokens": total_tokens,
        "requests": requests,
        "tool_calls": len(calls),
        "tool_errors": tool_errors,
        "by_tool": by_tool,
        "models": models,
        "status": status,
        "error": error,
    }


def judge(task, work, answer):
    """The task's own `check.sh` when it has one; otherwise its hidden
    verifier on a copy of the tree, so a model cannot pass by editing what
    the verifier reads. `answer` is the file the model's last message was
    written to — the last argument either script is given, and one a task
    that judges the tree alone never reads."""
    check = task / "check.sh"
    if check.is_file():
        done = subprocess.run(["sh", str(check), str(work), str(answer)], capture_output=True, text=True)
    else:
        scratch = Path(tempfile.mkdtemp(prefix=f"zo-judge-{task.name}-"))
        try:
            shutil.copytree(work, scratch, dirs_exist_ok=True)
            done = subprocess.run(
                ["sh", str(task / "hidden" / "verify.sh"), str(scratch), str(task / "repo"), str(answer)],
                capture_output=True, text=True,
            )
        finally:
            shutil.rmtree(scratch, ignore_errors=True)
    return done.returncode == 0, (done.stdout + done.stderr).strip()[-600:]


def wilson_lower(passed, total, z=1.96):
    if total == 0:
        return 0.0
    rate = passed / total
    centre = rate + z * z / (2 * total)
    spread = z * math.sqrt(rate * (1 - rate) / total + z * z / (4 * total * total))
    return (centre - spread) / (1 + z * z / total)


def summarize(rows):
    groups = {}
    for row in rows:
        groups.setdefault((row["model"], row["arm"]), []).append(row)
    print(f"{'model':24} {'arm':14} {'pass':>7} {'wilson95':>9} {'wall_s':>7} {'tokens':>8} {'req':>5} {'calls':>6} {'tool_err':>8}")
    for (model, arm), group in sorted(groups.items()):
        passed = sum(row["passed"] for row in group)
        total = len(group)
        print(
            f"{model:24} {arm:14} {passed:>3}/{total:<3} {wilson_lower(passed, total):>9.2f} "
            f"{statistics.median(row['wall_s'] for row in group):>7.0f} "
            f"{statistics.median(row['total_tokens'] for row in group):>8.0f} "
            f"{statistics.mean(row.get('requests', 0) for row in group):>5.1f} "
            f"{statistics.mean(row['tool_calls'] for row in group):>6.1f} "
            f"{statistics.mean(row['tool_errors'] for row in group):>8.1f}"
        )


def main(argv):
    args = parse_args(argv)
    arms = arms_of(args.arm)
    rows = []
    with open(args.out, "a", encoding="utf-8") as out:
        for task, meta in tasks_in(args.tasks, args.only):
            prompt = (task / "prompt.txt").read_text().strip()
            for model in args.model:
                for arm, overlay in arms:
                    for attempt in range(args.repeat):
                        work = prepare_workdir(task)
                        # Beside the tree, not in it: the model neither sees
                        # the file nor finds it in `git status`.
                        said = Path(tempfile.mkdtemp(prefix=f"zo-bench-said-{task.name}-"))
                        answer = said / "last-message.txt"
                        code, stdout, stderr, wall, timed_out = run_zo(args, model, overlay, work, prompt, answer)
                        passed, verdict = judge(task, work, answer)
                        row = {
                            "task": meta.get("id"), "slug": meta.get("slug"), "model": model,
                            "arm": arm, "attempt": attempt, "passed": passed,
                            "wall_s": round(wall, 1), "exit": code, "timed_out": timed_out,
                            "verdict": verdict, "stderr_tail": stderr.strip()[-300:],
                            **read_events(stdout),
                        }
                        rows.append(row)
                        out.write(json.dumps(row) + "\n")
                        out.flush()
                        print(f"{meta.get('id'):>4} {model:24} {arm:14} {'PASS' if passed else 'FAIL'} {wall:6.0f}s", flush=True)
                        shutil.rmtree(work, ignore_errors=True)
                        shutil.rmtree(said, ignore_errors=True)
    summarize(rows)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
