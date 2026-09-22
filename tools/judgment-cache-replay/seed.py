#!/usr/bin/env python3
"""Gather, from this machine's screen-seat ledgers, how often a walk asked a
question a memo could have answered — and nothing else.

The judgment cache (`zerocode_core::jev::JUDGMENT_CACHE`, t-6132) keys an
answer by the bytes the door cleared. Those bytes are not in any ledger —
a row keeps facts about the question, never the question — so this script
does not rebuild keys and asks nothing. What it can read is the shape of
each question a screen seat asked: the seat, the errand, the flow it walked
for (as a digest, never the name), how many controls were offered, how many
presses came before, and the attempt. Two rows of one shape are one screen
asked twice under one goal at the same point of a walk — the hit a memo
would have served — and the share of rows that repeat an earlier shape is
the most a memo could have saved this machine's walks.

It reads:

- `<config home>/jev/{browser,desktop,emulator}-action.jsonl` — the three
  screen seats' one ledger each (`systemone::ledger_of`);
- `<data root>/computer-use/sessions/*/{browser,desktop,emulator}-action.jsonl`
  — the same rows beside each walk's evidence (`errand::write_rows`).

A row is read only where it is an ask (`outcome`, not a judge's transition),
and the seed carries numbers: rows, distinct shapes, rows that repeat a shape,
the judgment latency population (`elapsedMs` of answered rows that went over
the wire — a `cached` row answered without a call and is left out, as
`summary::summarize_rows` leaves it out) and the requests counted. No word a
person wrote is in it: a flow's name is carried as a digest.

    tools/judgment-cache-replay/seed.py                       # this machine
    tools/judgment-cache-replay/seed.py --out seed.json
    tools/judgment-cache-replay/seed.py --config-home ~/.zo --sessions <root>
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from collections import Counter
from pathlib import Path

# `JevUse.ledger` of the three screen seats in `crates/zerocode-core/src/jev.rs`.
SCREEN_LEDGERS = ("browser-action.jsonl", "desktop-action.jsonl", "emulator-action.jsonl")
# `count::REQUESTS_DIR` in `crates/zerocode-core/src/jev/count.rs`.
REQUESTS_DIR = "jev"
# `evidence::SESSIONS_DIR` in `crates/zerocode-shell/src/computer_use/evidence.rs`.
SESSIONS_DIR = "computer-use/sessions"
# `memo::MEMO_ROWS_CAP` in `crates/zerocode-core/src/jev/memo.rs`: the most
# distinct shapes one memo holds before it drops its oldest half.
MEMO_ROWS_CAP = 2_000
# `JUDGMENT_CACHE_AGREEMENT_FLOOR_PERMILLE` in `crates/zerocode-core/src/jev.rs`.
AGREEMENT_FLOOR_PERMILLE = 900

# The facts of a question's shape, by the row keys `errand::row` writes.
SHAPE_KEYS = ("errand", "candidates", "pressedBefore", "attempt", "showsLines")


def read_rows(path: Path):
    """Every JSON object in a ledger, torn lines skipped."""
    try:
        with path.open(errors="replace") as handle:
            for line in handle:
                line = line.strip()
                if not line:
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if isinstance(row, dict):
                    yield row
    except OSError:
        return


def is_ask(row: dict) -> bool:
    """A row that asked something: has an `outcome`, is not a judge's note."""
    return "transition" not in row and isinstance(row.get("outcome"), str) and row["outcome"] != "control"


def flow_digest(row: dict) -> str:
    """The flow a walk was for, as sixteen hex digits — never its words."""
    name = row.get("flow")
    if not isinstance(name, str):
        return ""
    return hashlib.sha256(name.encode("utf-8")).hexdigest()[:16]


def shape_of(seat: str, row: dict) -> tuple:
    return (seat, flow_digest(row)) + tuple(row.get(key) for key in SHAPE_KEYS)


def ledgers_under(config_home: Path | None, sessions: Path | None) -> list[tuple[str, Path]]:
    found: list[tuple[str, Path]] = []
    if config_home is not None:
        for ledger in SCREEN_LEDGERS:
            path = config_home / REQUESTS_DIR / ledger
            if path.is_file():
                found.append((ledger.split("-", 1)[0], path))
    if sessions is not None and sessions.is_dir():
        for session in sorted(sessions.iterdir()):
            for ledger in SCREEN_LEDGERS:
                path = session / ledger
                if path.is_file():
                    found.append((ledger.split("-", 1)[0], path))
    return found


def percentile(sorted_values: list[int], share: float) -> int | None:
    if not sorted_values:
        return None
    return sorted_values[min(len(sorted_values) - 1, int(share * len(sorted_values)))]


def build(config_home: Path | None, sessions: Path | None) -> dict:
    """The seed: per seat, how many asks repeat a shape an earlier ask had."""
    per_seat: dict[str, dict] = {}
    seen: dict[str, Counter] = {}
    latency: dict[str, list[int]] = {}
    for seat, path in ledgers_under(config_home, sessions):
        tally = per_seat.setdefault(seat, {"rows": 0, "answered": 0, "requests": 0, "cached": 0, "repeatedShape": 0})
        shapes = seen.setdefault(seat, Counter())
        waits = latency.setdefault(seat, [])
        for row in read_rows(path):
            if not is_ask(row):
                continue
            tally["rows"] += 1
            requests = row.get("requests")
            if isinstance(requests, int):
                tally["requests"] += requests
            if row.get("cached") is True:
                tally["cached"] += 1
            shape = shape_of(seat, row)
            if shapes[shape] > 0:
                tally["repeatedShape"] += 1
            shapes[shape] += 1
            if row.get("outcome") == "answered":
                tally["answered"] += 1
                elapsed = row.get("elapsedMs")
                if isinstance(elapsed, int) and row.get("cached") is not True:
                    waits.append(elapsed)
    for seat, tally in per_seat.items():
        tally["distinctShapes"] = len(seen[seat])
        waits = sorted(latency[seat])
        tally["wireCalls"] = len(waits)
        tally["p50Ms"] = percentile(waits, 0.50)
        tally["p95Ms"] = percentile(waits, 0.95)
        tally["repeatedShare"] = (tally["repeatedShape"] / tally["rows"]) if tally["rows"] else None
    return {
        "memoRowsCap": MEMO_ROWS_CAP,
        "agreementFloorPermille": AGREEMENT_FLOOR_PERMILLE,
        "shapeKeys": list(SHAPE_KEYS),
        "seats": per_seat,
    }


def default_config_home() -> Path:
    for var in ("ZO_CONFIG_HOME", "ZO_HOME"):
        value = os.environ.get(var)
        if value:
            return Path(value)
    return Path.home() / ".zo"


def default_sessions() -> Path:
    return Path.home() / "Library" / "Application Support" / "dev.zerocode.app" / SESSIONS_DIR


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--config-home", type=Path, default=None, help="zo's config home (default: $ZO_CONFIG_HOME, $ZO_HOME, ~/.zo)")
    parser.add_argument("--sessions", type=Path, default=None, help=f"the window's {SESSIONS_DIR} folder")
    parser.add_argument("--no-sessions", action="store_true", help="read the config home's ledgers only")
    parser.add_argument("--out", type=Path, default=None, help="write the seed here (default: stdout)")
    args = parser.parse_args()
    config_home = args.config_home or default_config_home()
    sessions = None if args.no_sessions else (args.sessions or default_sessions())
    seed = build(config_home, sessions)
    text = json.dumps(seed, indent=1, ensure_ascii=False) + "\n"
    if args.out is None:
        sys.stdout.write(text)
    else:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
