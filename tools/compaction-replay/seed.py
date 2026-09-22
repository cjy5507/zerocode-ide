#!/usr/bin/env python3
"""Pick the transcripts on this machine whose compaction points the relevance
seat can be replayed at, and say where each one's recorded cuts fell.

This script **only reads files and counts**. It cuts nothing, estimates no
token, builds no question and scores no drop: the boundary rule is
`runtime::prepare_compaction` and the tail rule `preserved_tail_len_for_budget`,
the candidates and the state are `runtime::compaction_relevance`, and the
replay that uses this seed hands each transcript to those functions as they
ship (`smart_router::compaction_seat::tests::the_compactions_this_machine_would_have_made`).
A second copy of any of that arithmetic in Python would be a second rule, and
the one under test would stop being the one that ships.

What a seed row carries is a path, a message count, the model the transcript
ran on, and the absolute index of every compaction the transcript recorded
(`first_kept_message_index`, the seq of the first message a round kept). No
words: nothing a person typed or a tool printed is read into the seed.

The replay reads every judgment's input from before its cut — the session is
rebuilt from the messages up to it — and the turns after only as the label
(a dropped block read again inside `COMPACTION_REGRET_TURNS` turns is regret).
The seed carries that window so a replay made for another one is refused.

    tools/compaction-replay/seed.py                      # every project under ~/.zo
    tools/compaction-replay/seed.py --out seed.json
    tools/compaction-replay/seed.py --project ~/.zo/projects
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

# `MIN_COMPACTABLE_MESSAGES` in `zo-ide/crates/runtime/src/compact/mod.rs`:
# at least this many messages must remain summarizable after the tail, so a
# transcript shorter than it plus the smallest tail has no boundary at all.
MIN_COMPACTABLE_MESSAGES = 8

# `COMPACTION_REGRET_TURNS` in `crates/zerocode-core/src/jev.rs` — the
# window the label reads after a cut.
REGRET_TURNS = 5

# The smallest preserved tail (`CompactionConfig::default().preserve_recent_messages`).
SMALLEST_TAIL = 4

# Sidecars that share the transcript's extension and are not transcripts.
SIDECAR_MARKS = (".vault.jsonl", ".rot-", ".todos.json", ".prefs.json")


def is_transcript(path: Path) -> bool:
    name = path.name
    return name.startswith("session-") and name.endswith(".jsonl") and not any(mark in name for mark in SIDECAR_MARKS)


def transcripts(roots: list[Path]) -> list[Path]:
    found = []
    seen = set()
    for root in roots:
        for path in sorted(root.glob("*/sessions/session-*.jsonl")):
            if is_transcript(path) and path.resolve() not in seen:
                seen.add(path.resolve())
                found.append(path)
    return found


def read_rows(path: Path) -> list[dict]:
    """The transcript's records. A torn trailing line is skipped, never fatal."""
    rows = []
    for line in path.read_text(errors="replace").splitlines():
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


def user_turns(rows: list[dict]) -> int:
    """How many turns a person began — a user message that spoke, not a tool result."""
    turns = 0
    for row in rows:
        if row.get("type") != "message":
            continue
        message = row.get("message") or {}
        if message.get("role") != "user":
            continue
        if any(block.get("type") == "text" for block in message.get("blocks") or []):
            turns += 1
    return turns


def describe(path: Path) -> dict | None:
    rows = read_rows(path)
    messages = sum(1 for row in rows if row.get("type") == "message")
    if messages < MIN_COMPACTABLE_MESSAGES + SMALLEST_TAIL:
        return None
    model = None
    for row in rows:
        if row.get("type") == "message":
            model = (row.get("message") or {}).get("model")
            if model:
                break
    recorded = []
    for row in rows:
        if row.get("type") == "compaction" and isinstance(row.get("first_kept_message_index"), int):
            recorded.append(row["first_kept_message_index"])
    return {
        "path": str(path),
        "messages": messages,
        "model": model,
        "userTurns": user_turns(rows),
        "recordedCuts": recorded,
        "vault": path.with_name(path.name[: -len(".jsonl")] + ".vault.jsonl").is_file(),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--project",
        type=Path,
        action="append",
        default=None,
        help="a projects directory to read (default: $ZO_STATE_DIR or ~/.zo/projects)",
    )
    parser.add_argument("--out", type=Path, default=None, help="where to write the seed (default: stdout)")
    args = parser.parse_args()

    roots = args.project or [Path(os.environ.get("ZO_STATE_DIR", Path.home() / ".zo")) / "projects"]
    held = []
    for path in transcripts([Path(root).expanduser() for root in roots]):
        row = describe(path)
        if row is not None:
            held.append(row)
    held.sort(key=lambda row: (-row["messages"], row["path"]))
    recorded = sum(len(row["recordedCuts"]) for row in held)
    print(
        f"{len(held)} transcripts with a boundary from {len(roots)} root(s); "
        f"{recorded} recorded compactions, {sum(1 for row in held if row['vault'])} with a vault",
        file=sys.stderr,
    )
    seed = {"regretTurns": REGRET_TURNS, "minCompactableMessages": MIN_COMPACTABLE_MESSAGES, "transcripts": held}
    text = json.dumps(seed, indent=1) + "\n"
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text)
        print(f"wrote {args.out}", file=sys.stderr)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
