#!/usr/bin/env python3
"""Gather the phone steps this window's walks have judged — the emulator
seat's answered rows, with the probabilities it spread over the numbered
controls and how the press it chose fared — from the evidence folders on
this machine: the seed for the branching seat's replay (t-6044).

This script **only reads files and copies facts**. It judges nothing, asks
nothing and counts nothing: which of a row's controls are a fork's
candidates is `zerocode_core::branching::top_k`, the question is
`branching::ask`, the mark is `branching::agreed`, and the replay that uses
this seed hands each row to those functions as they ship
(`computer_use::errand::branch::tests::the_forks_this_desk_would_take`). A
second copy of any of that in Python would be a second rule.

What a seed row is: one press a phone walk's judgment made — an
`emulator-action.jsonl` row whose `outcome` is `answered` — with what the
walk knew AT THAT MOMENT and nothing later: the goal (`flow`), the errand,
every option's probability as the seat answered it, the number it chose, and
from the steps log beside it the platform, the device and how long that
step's look and press took. And the label, read from AFTER the press: the
seat's own `agreed` mark (the walk reached its goal, or the stop was
cleared), when the walk wrote one.

The rows carry no legend lines and no result screens — the evidence keeps a
step's words, not the controls it saw — so the replay reads them for how
often a fork WOULD have been offered and how the single press fared, and
asks the endpoint over the fake desk's scenarios instead (the README says
which numbers come from which).

    tools/branching-replay/seed.py --out seed.json      # this window's evidence folders
    tools/branching-replay/seed.py --sessions DIR --ledger FILE
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

# `BRANCHING_K` in `crates/zerocode-core/src/jev.rs`: how many candidates a
# forked step tries. Carried so a seed made under another table is refused.
K = 2

# `BRANCHING_K_CAP` there: the most candidates the question offers.
K_CAP = 3

# `BRANCHING_APPLY_DEADLINE_MS` there: the wall the comparison is waited for.
APPLY_DEADLINE_MS = 1_500

# `SCREEN_CANDIDATE_CAP` there: the controls one result carries.
CONTROLS_CAP = 12

# The seat whose rows a fork reads: `zerocode_core::jev::EMULATOR`'s ledger.
EMULATOR_LEDGER = "emulator-action.jsonl"

# The step log every walk leaves beside its rows (`run_evidence`).
STEPS_LOG = "steps.jsonl"

# The row words the emulator seat writes (`computer_use::errand`).
ANSWERED = "answered"
MARK_PREFIX = "mark:"

# The tool the phone's steps are logged under (`RecipeTool::Emulator`).
EMULATOR_TOOL = "emulator"


def default_sessions() -> Path:
    return Path.home() / "Library" / "Application Support" / "dev.zerocode.app" / "computer-use" / "sessions"


def default_ledger() -> Path:
    home = os.environ.get("ZO_CONFIG_HOME") or os.environ.get("ZO_HOME") or str(Path.home() / ".zo")
    return Path(home) / "jev" / EMULATOR_LEDGER


def read_rows(path: Path) -> list[dict]:
    """The file's JSON lines. A torn trailing line is skipped, never fatal."""
    rows: list[dict] = []
    try:
        text = path.read_text(errors="replace")
    except OSError:
        return rows
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(row, dict):
            rows.append(row)
    return rows


def flag_value(argv: list, flag: str) -> str | None:
    """The word after `flag` in a step's argv, when it carries one."""
    for at, word in enumerate(argv[:-1]):
        if word == flag:
            return str(argv[at + 1])
    return None


def phone_steps(steps: list[dict]) -> list[dict]:
    """The emulator steps of one walk, in order: verb, platform, device and
    the milliseconds the observation recorded."""
    found = []
    for step in steps:
        if step.get("tool") != EMULATOR_TOOL:
            continue
        argv = step.get("argv") or []
        observation = step.get("observation") or {}
        found.append({
            "n": step.get("n"),
            "verb": step.get("verb"),
            "platform": flag_value(argv, "--platform"),
            "device": flag_value(argv, "--device"),
            "ok": bool(step.get("ok")),
            "lookMs": observation.get("look_ms"),
            "actMs": observation.get("act_ms"),
            "elapsedMs": observation.get("elapsed_ms"),
        })
    return found


def is_answered_press(row: dict) -> bool:
    """A row the branching seat could have read: the emulator seat answered,
    and its answer spread probabilities over numbered controls."""
    if row.get("outcome") != ANSWERED:
        return False
    probabilities = row.get("probabilities")
    if not isinstance(probabilities, dict):
        return False
    return any(str(option).startswith(MARK_PREFIX) for option in probabilities)


def seed_row(row: dict, steps: list[dict], source: str) -> dict:
    """One answered press, with the facts the walk had and its label."""
    clicks = [step for step in steps if step["verb"] == "click"]
    looks = [step for step in steps if step["verb"] == "marks"]
    attempt = row.get("attempt")
    click = None
    if isinstance(attempt, int) and 1 <= attempt <= len(clicks):
        click = clicks[attempt - 1]
    look = None
    if isinstance(attempt, int) and 1 <= attempt <= len(looks):
        look = looks[attempt - 1]
    where = click or look or (steps[0] if steps else {})
    seeded = {
        "source": source,
        "flow": row.get("flow"),
        "errand": row.get("errand"),
        "attempt": attempt,
        "mode": row.get("mode"),
        "platform": where.get("platform"),
        "device": where.get("device"),
        "probabilities": {
            str(option): value
            for option, value in row["probabilities"].items()
            if isinstance(value, (int, float))
        },
        "confidence": row.get("confidence"),
        "chosen": row.get("chosen"),
        "pressed": row.get("pressed"),
        "lookMs": (click or {}).get("lookMs") if click else (look or {}).get("elapsedMs"),
        "actMs": (click or {}).get("actMs"),
        # The label, read from after the press: the seat's own mark when the
        # walk wrote one; a row without it left no label.
        "label": {"agreed": row.get("agreed")} if isinstance(row.get("agreed"), bool) else None,
    }
    return seeded


def gather(sessions: Path, ledger: Path | None) -> tuple[list[dict], dict]:
    rows: list[dict] = []
    counted = {"sessions": 0, "emulatorRows": 0, "answered": 0, "ledgerRows": 0}
    if sessions.is_dir():
        for folder in sorted(sessions.iterdir()):
            seat_rows = read_rows(folder / EMULATOR_LEDGER)
            if not seat_rows:
                continue
            counted["sessions"] += 1
            counted["emulatorRows"] += len(seat_rows)
            steps = phone_steps(read_rows(folder / STEPS_LOG))
            for row in seat_rows:
                if is_answered_press(row):
                    counted["answered"] += 1
                    rows.append(seed_row(row, steps, folder.name))
    if ledger is not None and ledger.is_file():
        ledger_rows = read_rows(ledger)
        counted["ledgerRows"] = len(ledger_rows)
        for row in ledger_rows:
            if is_answered_press(row):
                counted["answered"] += 1
                rows.append(seed_row(row, [], ledger.name))
    return rows, counted


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--sessions", type=Path, default=default_sessions(), help="the evidence folders")
    parser.add_argument("--ledger", type=Path, default=None, help="the emulator seat's one ledger (default: the zo home's)")
    parser.add_argument("--no-ledger", action="store_true", help="read the evidence folders alone")
    parser.add_argument("--out", type=Path, default=None, help="where the seed goes (default: stdout)")
    args = parser.parse_args(argv)
    ledger = None if args.no_ledger else (args.ledger or default_ledger())
    rows, counted = gather(args.sessions, ledger)
    seed = {
        "k": K,
        "kCap": K_CAP,
        "applyDeadlineMs": APPLY_DEADLINE_MS,
        "controlsCap": CONTROLS_CAP,
        "counted": counted,
        "rows": rows,
    }
    text = json.dumps(seed, ensure_ascii=False, indent=1)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text)
    else:
        sys.stdout.write(text + "\n")
    sys.stderr.write(
        f"sessions={counted['sessions']} emulatorRows={counted['emulatorRows']} "
        f"ledgerRows={counted['ledgerRows']} answered={counted['answered']} rows={len(rows)}\n"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
