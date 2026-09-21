#!/usr/bin/env python3
"""r40 — how zo actually uses its tools, counted from its own transcripts.

Run it from `zo-ide/`:

    python3 tools/tool-habits-r40.py                # last 24h
    python3 tools/tool-habits-r40.py --hours 72
    python3 tools/tool-habits-r40.py --json /tmp/habits.json

Every number in `docs/analysis/tool-habits-r40.md` comes from this script, and
nothing here writes anything.

The session reader is r24a's (`new-content-bytes.py`), imported rather than
copied: which files make a session, how rotated segments and the vault fold
into one ordered list, and which record is a message are decided in one place.
What is new here is the question — not how many bytes a tool costs, but how
the model *reaches* for tools:

* **batch width** — how many `tool_use` blocks one assistant message carries.
  The prompt asks for independent reads in one message; this says whether it
  happens.
* **shell reads** — `bash` calls whose command is a plain `cat`/`head`/`tail`/
  `sed -n`/`grep`/`rg`/`find`, i.e. work a dedicated tool already does
  cheaper and permission-scoped.  The classification here is by first word
  only, deliberately looser than the harness's `bash_redirect::dedicated_read_for`
  (which refuses pipes, globs and redirections): this is the ceiling, the
  harness redirects a subset of it.
* **waits** — `bash` calls that block the turn on other agents
  (`zerocode-orc check --wait`, `sleep N` with N ≥ 60).
* **delegation** — `Agent`/`SpawnMultiAgent`/`Workflow` calls, and what they
  asked for.
* **harness marks** — how often the three r40 mechanisms fired: the redirect
  note on a `bash` result, the auto-background note on a wait, and the
  serial-read nudge on a tool result.  Zero before the r40 binary, and the
  numbers to watch after it.
"""

from __future__ import annotations  # `str | None` below must load on the
# system python3 (3.9) too: the gate calls bare `python3`, so which one
# answers depends on the pane's PATH.

import argparse
import importlib.util
import json
import os
import re
import shlex
import sys
import time
from collections import Counter, defaultdict
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))

_spec = importlib.util.spec_from_file_location(
    "new_content_bytes", os.path.join(HERE, "new-content-bytes.py"))
r24 = importlib.util.module_from_spec(_spec)
# Registered before exec: the reader's `@dataclass` looks its module up in
# `sys.modules` (Python 3.14), and an unregistered module has no entry.
sys.modules[_spec.name] = r24
_spec.loader.exec_module(r24)

READ_PROGRAMS = ("cat", "head", "tail", "sed", "grep", "rg", "find", "ls", "wc")
DELEGATION_TOOLS = ("Agent", "SpawnMultiAgent", "Workflow")
# The three r40 marks, spelled as the harness spells them
# (`bash_redirect::redirect_note`, `bash_redirect::auto_background_note`,
# `repetition::SERIAL_READS_NUDGE`).
REDIRECT_MARK = "the shell is for what the dedicated tools cannot do"
BACKGROUND_MARK = "waits on other agents, so it was started in the background"
NUDGE_MARK = "The last three tool batches each carried a single read-only call"
DIGEST_MARK = "[digest] "
WAIT_SLEEP_SECS = 60


def first_program(command: str) -> str | None:
    try:
        words = shlex.split(command, posix=True)
    except ValueError:
        words = command.split()
    for word in words:
        if "=" in word and not word.startswith("-"):
            continue  # VAR=value prefix
        if word in ("export", "cd", "&&", ";"):
            return None
        return os.path.basename(word)
    return None


def is_wait(command: str) -> bool:
    if re.search(r"zerocode-orc\s+check\b.*--wait", command):
        return True
    match = re.match(r"\s*sleep\s+(\d+)", command)
    return bool(match) and int(match.group(1)) >= WAIT_SLEEP_SECS


def census(root: Path, since_ms: int) -> dict:
    tools = Counter()
    batch_width = Counter()
    shell = Counter()
    shell_reads = Counter()
    waits = 0
    backgrounded_waits = 0
    delegations = Counter()
    delegation_asks: list[str] = []
    marks = Counter()
    messages = 0
    sessions = 0
    for _key, ordered, _compactions, span in r24.zo_sessions(root, use_vault=False):
        end = span[1]
        if end is None or end < since_ms:
            continue
        sessions += 1
        for _index, message, _ts in ordered:
            blocks = message.get("blocks") or []
            if message.get("role") == "assistant":
                uses = [b for b in blocks if isinstance(b, dict) and b.get("type") == "tool_use"]
                if not uses:
                    continue
                messages += 1
                batch_width[len(uses)] += 1
                for use in uses:
                    name = use.get("name") or "?"
                    tools[name] += 1
                    try:
                        inp = json.loads(use.get("input") or "{}")
                    except (TypeError, json.JSONDecodeError):
                        inp = {}
                    if not isinstance(inp, dict):
                        continue
                    if name == "bash":
                        command = str(inp.get("command") or "")
                        program = first_program(command)
                        if program:
                            shell[program] += 1
                            if program in READ_PROGRAMS:
                                shell_reads[program] += 1
                        if is_wait(command):
                            waits += 1
                            if inp.get("run_in_background") is True:
                                backgrounded_waits += 1
                    elif name in DELEGATION_TOOLS:
                        delegations[name] += 1
                        ask = inp.get("description") or inp.get("prompt") or inp.get("task") or ""
                        delegation_asks.append(f"{name}: {str(ask)[:90]}")
            elif message.get("role") == "tool":
                for block in blocks:
                    if not isinstance(block, dict) or block.get("type") not in ("tool_result", "text"):
                        continue
                    body = block.get("output") or block.get("text") or ""
                    if REDIRECT_MARK in body:
                        marks["redirect"] += 1
                    if BACKGROUND_MARK in body:
                        marks["auto_background"] += 1
                    if NUDGE_MARK in body:
                        marks["nudge"] += 1
                    if DIGEST_MARK in body:
                        marks["digest"] += 1
    return {
        "sessions": sessions,
        "tool_messages": messages,
        "tools": dict(tools.most_common()),
        "batch_width": dict(sorted(batch_width.items())),
        "shell_programs": dict(shell.most_common(20)),
        "shell_reads": dict(shell_reads.most_common()),
        "waits": waits,
        "backgrounded_waits": backgrounded_waits,
        "delegations": dict(delegations),
        "delegation_asks": delegation_asks[:20],
        "marks": dict(marks),
    }


def print_report(report: dict) -> None:
    total = sum(report["tools"].values())
    single = report["batch_width"].get(1, 0)
    msgs = report["tool_messages"]
    print(f"sessions {report['sessions']}  tool-carrying messages {msgs}  tool calls {total}")
    print("\n== tools")
    for name, count in report["tools"].items():
        print(f"  {count:6d}  {name}")
    print("\n== batch width (tool_use blocks per assistant message)")
    for width, count in report["batch_width"].items():
        print(f"  {width:3d} × {count}")
    if msgs:
        print(f"  single-call messages: {single}/{msgs} = {100 * single / msgs:.0f}%")
    reads = sum(report["shell_reads"].values())
    bash = report["tools"].get("bash", 0)
    print(f"\n== bash first words (read-ish {reads}/{bash})")
    for name, count in report["shell_programs"].items():
        flag = "  <- read" if name in READ_PROGRAMS else ""
        print(f"  {count:6d}  {name}{flag}")
    print(f"\n== waits on other agents: {report['waits']} "
          f"(already run_in_background: {report['backgrounded_waits']})")
    print(f"\n== delegations: {report['delegations'] or 'none'}")
    for ask in report["delegation_asks"]:
        print(f"  {ask}")
    print(f"\n== r40 harness marks: {report['marks'] or 'none yet'}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", default=os.path.expanduser("~/.zo/projects"))
    parser.add_argument("--hours", type=float, default=24.0)
    parser.add_argument("--json")
    args = parser.parse_args()
    since_ms = int((time.time() - args.hours * 3600) * 1000)
    report = census(Path(args.root), since_ms)
    print_report(report)
    if args.json:
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump(report, handle, ensure_ascii=False, indent=2)
    return 0


if __name__ == "__main__":
    sys.exit(main())
