#!/usr/bin/env python3
"""Select recent zo transcripts for the claim seat's Rust replay.

Only paths and counts enter the seed. The shipped Rust scanner selects claims,
tool evidence and hindsight at each turn's own clock; Python never copies the
rubric or records a person's words.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import time
from pathlib import Path

PICKER = Path(__file__).resolve().parents[1] / "compaction-replay" / "seed.py"
spec = importlib.util.spec_from_file_location("compaction_replay_seed", PICKER)
picker = importlib.util.module_from_spec(spec)
sys.modules.setdefault(spec.name, picker)
spec.loader.exec_module(picker)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, action="append")
    parser.add_argument("--days", type=int, default=7)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    roots = args.project or [Path(os.environ.get("ZO_STATE_DIR", Path.home() / ".zo")) / "projects"]
    since = int(time.time() * 1000) - args.days * 86_400_000
    held = []
    for path in picker.transcripts([root.expanduser() for root in roots]):
        rows = picker.read_rows(path)
        messages = [row for row in rows if row.get("type") == "message"]
        updated = max((row.get("updated_at_ms") or 0 for row in messages), default=0)
        if updated < since:
            continue
        held.append({"path": str(path), "messages": len(messages), "updatedAt": updated})
    held.sort(key=lambda row: (row["updatedAt"], row["path"]))
    seed = {"since": since, "transcripts": held}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(seed) + "\n")
    print(f"{len(held)} transcripts from {len(roots)} root(s) → {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
