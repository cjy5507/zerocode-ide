#!/usr/bin/env python3
"""Pick what the routing seat's replay reads on this machine (t-6346): the zo
transcripts whose turns and spawns the second version is asked about, the
routing ledgers whose rows hold the chat probe's recorded answers, and the
route-outcome ledgers.

This script **only reads files and counts**. It builds no question, reads no
task and grades nothing: what a turn is asked (its words, the keyword tables'
reading, the probe's gate) is `smart_router::turn`, what a turn did is
`smart_router::route_label`, the questions and their reading are
`runtime::model_router::decision`, and the replay that uses this seed hands
each transcript to those functions as they ship
(`smart_router::routing_replay::the_routing_seat_replayed_on_this_machines_turns`).
A second copy of any of those rules in Python would be a second rule, and the
one under test would stop being the one that ships.

A seed row carries a path and counts — messages, the turns a person began,
the agents those turns started — and, for a ledger, how many rows it holds
and how many carry the probe's answer or an attempt key. No words: nothing a
person typed or a tool printed is read into the seed.

Which files are transcripts is compaction-replay's reading (`is_transcript`,
`transcripts`, `read_rows`), loaded from there rather than copied.

    tools/routing-replay/seed.py --out seed.json             # the last 7 days under ~/.zo
    tools/routing-replay/seed.py --days 30 --out seed.json
    tools/routing-replay/seed.py --project ~/.zo/projects --out seed.json
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import time
from pathlib import Path

_PICKER_PATH = Path(__file__).resolve().parents[1] / "compaction-replay" / "seed.py"
_spec = importlib.util.spec_from_file_location("compaction_replay_seed", _PICKER_PATH)
picker = importlib.util.module_from_spec(_spec)
sys.modules.setdefault(_spec.name, picker)
_spec.loader.exec_module(picker)

# `HARNESS_TAG_OPEN` in `zo-ide/crates/runtime/src/patch_review.rs`: a user
# message that opens with it continues the turn before it.
HARNESS_TAG_OPEN = "[zo:"

# `is_fan_out_tool` in `zo-ide/crates/runtime/src/conversation/tool.rs`: the
# calls that start agents.
SPAWN_TOOLS = ("Agent", "Task", "SpawnMultiAgent", "Workflow")

# `zerocode_core::jev::ROUTING.ledger` (zo's `DECISION_SHADOW_FILE`) and
# `OUTCOME_FILE` in `zo-ide/crates/runtime/src/model_router/outcome.rs`, both
# under a project's `state/` in `OUTCOME_DIR`.
ROUTING_LEDGER = "decision-shadow.jsonl"
ROUTE_OUTCOMES = "route-outcomes.jsonl"
STATE_DIR = ("state", "smart-router")


def person_turns(rows: list[dict]) -> int:
    """User messages that speak and do not continue a harness note — the
    turns a person began (`runtime::patch_review::persons_turns`)."""
    turns = 0
    for row in rows:
        if row.get("type") != "message":
            continue
        message = row.get("message") or {}
        if message.get("role") != "user":
            continue
        for block in message.get("blocks") or []:
            words = block.get("text") if block.get("type") == "text" else None
            if isinstance(words, str) and words.strip() and not words.lstrip().startswith(HARNESS_TAG_OPEN):
                turns += 1
                break
    return turns


def spawns(rows: list[dict]) -> int:
    """Calls that started agents."""
    count = 0
    for row in rows:
        if row.get("type") != "message":
            continue
        for block in (row.get("message") or {}).get("blocks") or []:
            if block.get("type") == "tool_use" and block.get("name") in SPAWN_TOOLS:
                count += 1
    return count


def describe(path: Path, since_ms: int) -> dict | None:
    rows = picker.read_rows(path)
    messages = [row for row in rows if row.get("type") == "message"]
    updated = max((row.get("updated_at_ms") or 0 for row in rows), default=0)
    if updated < since_ms or not messages:
        return None
    turns = person_turns(rows)
    if turns == 0:
        return None
    return {"path": str(path), "messages": len(messages), "turns": turns, "spawns": spawns(rows), "updatedAt": updated}


def ledger(path: Path) -> dict:
    """A ledger's size: its rows, the ones that carry the probe's answer as an
    object, and the ones that carry an attempt key."""
    rows = picker.read_rows(path)
    probed = sum(1 for row in rows if isinstance(row.get("probe"), dict))
    attempted = sum(1 for row in rows if row.get("attempt") or row.get("runId"))
    return {"path": str(path), "rows": len(rows), "probed": probed, "attempted": attempted}


def state_files(roots: list[Path], name: str) -> list[Path]:
    found = []
    for root in roots:
        for path in sorted(root.glob(f"*/{STATE_DIR[0]}/{STATE_DIR[1]}/{name}")):
            if path.is_file() and path.stat().st_size > 0:
                found.append(path)
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, action="append", default=None,
                        help="a projects directory to read (default: $ZO_STATE_DIR or ~/.zo/projects)")
    parser.add_argument("--days", type=int, default=7, help="transcripts updated within this many days")
    parser.add_argument("--out", type=Path, required=True, help="where to write the seed")
    args = parser.parse_args()

    roots = [Path(root).expanduser() for root in (args.project or [Path(os.environ.get("ZO_STATE_DIR", Path.home() / ".zo")) / "projects"])]
    since = int(time.time() * 1000) - args.days * 86_400_000
    held = []
    for path in picker.transcripts(roots):
        row = describe(path, since)
        if row is not None:
            held.append(row)
    held.sort(key=lambda row: (row["updatedAt"], row["path"]))
    seed = {
        "since": since,
        "days": args.days,
        "transcripts": held,
        "ledgers": [ledger(path) for path in state_files(roots, ROUTING_LEDGER)],
        "outcomes": [ledger(path) for path in state_files(roots, ROUTE_OUTCOMES)],
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(seed, indent=1) + "\n")
    print(
        f"{len(held)} transcripts ({sum(row['turns'] for row in held)} turns, {sum(row['spawns'] for row in held)} spawns) "
        f"· {len(seed['ledgers'])} routing ledgers ({sum(row['probed'] for row in seed['ledgers'])} probe answers) "
        f"· {len(seed['outcomes'])} route-outcome ledgers ({sum(row['rows'] for row in seed['outcomes'])} rows) → {args.out}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
