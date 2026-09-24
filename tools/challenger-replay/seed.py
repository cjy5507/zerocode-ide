#!/usr/bin/env python3
"""Pick the challenger arm's ledger rows on this machine, as each ledger knew
them at one clock, for the replay that reads them.

This script **only reads files and picks rows**. It re-derives no draw and no
blind, and counts no standing: whether an attempt drew and which design the
judge saw first are `zerocode_core::jev::challenger::draws` and `Blind::over`,
and a pair's record is `challenger::standing` — the replay that reads this
seed hands every row to those functions as they ship
(`jev::challenger::tests::the_challenger_rows_this_machine_wrote_replayed`).
A second copy of any of them in Python would be a second rule, and the one
under test would stop being the one that ships.

A replay row sees only what the ledger knew at its own clock: `--until`
keeps the rows written at or before that instant and drops every later one,
so a label written after the cut does not grade a comparison replayed before
it. The rows carry no words — the arm writes fingerprints and numbers, never
the task or a design — and the seed carries the rows as they are.

    tools/challenger-replay/seed.py                          # every project under ~/.zo
    tools/challenger-replay/seed.py --out seed.json
    tools/challenger-replay/seed.py --until 1790300000000
    tools/challenger-replay/seed.py --ledger path/to/challenger.jsonl
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

# `CHALLENGER.ledger` in `crates/zerocode-core/src/jev.rs`: the seat's file.
LEDGER_FILE = "challenger.jsonl"

# `JEV_LEDGER_DIR` in `zo-ide/crates/runtime/src/config/mod.rs`: the folder
# under a project's state every zo Jev seat appends its ledger in.
LEDGER_DIR = "smart-router"

# `AT` in `crates/zerocode-core/src/jev/summary.rs`: the key every row's
# clock is written under.
AT_KEY = "at"


def ledgers(roots: list[Path]) -> list[Path]:
    """Every challenger ledger under `<root>/*/state/<LEDGER_DIR>/`."""
    found = []
    for root in roots:
        if not root.is_dir():
            continue
        for project in sorted(root.iterdir()):
            path = project / "state" / LEDGER_DIR / LEDGER_FILE
            if path.is_file():
                found.append(path)
    return found


def rows_until(path: Path, until_ms: int) -> list[dict]:
    """The ledger's rows written at or before `until_ms`, oldest first; a line
    that does not parse, or names no clock, is skipped."""
    kept = []
    with path.open(encoding="utf-8") as text:
        for line in text:
            line = line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            at = row.get(AT_KEY) if isinstance(row, dict) else None
            if isinstance(at, int) and at <= until_ms:
                kept.append(row)
    return kept


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--project",
        type=Path,
        action="append",
        default=None,
        help="a projects directory to read (default: $ZO_STATE_DIR/projects or ~/.zo/projects)",
    )
    parser.add_argument("--ledger", type=Path, action="append", default=None, help="one ledger file to read instead")
    parser.add_argument("--until", type=int, default=None, help="the clock the replay stands at, Unix ms (default: now)")
    parser.add_argument("--out", type=Path, default=None, help="where to write the seed (default: stdout)")
    args = parser.parse_args()

    until_ms = args.until if args.until is not None else int(time.time() * 1000)
    if args.ledger:
        paths = [path.expanduser() for path in args.ledger]
    else:
        roots = args.project or [Path(os.environ.get("ZO_STATE_DIR", Path.home() / ".zo")) / "projects"]
        paths = ledgers([Path(root).expanduser() for root in roots])
    picked = [{"path": str(path), "rows": rows_until(path, until_ms)} for path in paths]
    print(
        f"{len(picked)} challenger ledger(s); {sum(len(one['rows']) for one in picked)} rows at or before {until_ms}",
        file=sys.stderr,
    )
    seed = {"until": until_ms, "ledgers": picked}
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
