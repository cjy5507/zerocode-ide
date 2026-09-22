#!/usr/bin/env python3
"""Pick every hedge this machine has fired, and the sample the rule read at it.

This script **only reads rows**. It names no delay, scores no win and tunes
nothing: the hedge rule is `zerocode_core::jev::hedge::plan` and lives in one
place, and the replay that uses this seed hands these samples straight to it
(`jev::hedge::tests::the_firings_that_already_happened`). That is the same
split `tools/summon-replay` keeps, for the same reason — a second copy of the
arithmetic in Python would be a second rule, and the one under test would stop
being the one that ships.

The one number it does compute is a check on its own reading, not on the rule:
the delay the ledger recorded for each firing must be what the rule as it
stood — `min(p75, wall - p50)` — names from the sample reconstructed here. A
seed whose reconstruction does not reproduce the recorded delays is a seed
that read the wrong rows, and the script says so rather than handing them on.

A row carries latencies, an outcome and counts. It carries no task text and no
query: nothing a person typed is read or written here.

    tools/hedge-replay/seed.py                       # every ledger under ~/.zo
    tools/hedge-replay/seed.py --out seed.json
    tools/hedge-replay/seed.py --project ~/.zo/projects/<slug>
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
from pathlib import Path

# The wall a judgment is used inside, both seats: `ROUTING_APPLY_DEADLINE_MS`
# in `crates/zerocode-core/src/jev.rs`, which `RERANK_APPLY_DEADLINE` takes as
# its own ("the same question asked twice").
WALL_MS = 1500

# `HEDGE_SAMPLE_ROWS` in `zo-ide/.../smart_router/jev_gate.rs`: the tail of a
# use's own ledger the rule reads its sample from.
SAMPLE_ROWS = 256

# `MIN_SAMPLES` in `crates/zerocode-core/src/jev/hedge.rs` — below it the rule
# names no rank, so a firing under it could not have happened and a seed row
# for it would be noise.
MIN_SAMPLES = 8

# `door::ANSWERED_OUTCOME`.
ANSWERED = "answered"

# The two ledgers a hedge can fire on, and the seat each one is.
LEDGERS = {"decision-shadow.jsonl": "routing", "rerank-shadow.jsonl": "recall"}


def field(row: dict, *names: str):
    """A column under either spelling — the routing ledger writes camelCase
    and the recall ledger snake_case, and reading one of them only is how a
    whole seat silently comes out as no rows at all."""
    for name in names:
        if name in row:
            return row[name]
    return None


def one_requests_own_latency(row: dict) -> int | None:
    """The sample this row is, or `None` when it is not one — `Timed::sample`
    in `jev_gate.rs`, which this mirrors and the seed's self-check proves.

    `elapsedMs` is a wire latency on fewer rows than it looks: on a timeout it
    is the wall, on a memo answer zero, on a retried row an attempt plus its
    backoffs, and on a hedged row the faster of two copies."""
    if row.get("outcome") != ANSWERED:
        return None
    if row.get("cached") or (field(row, "retries") or 0) != 0:
        return None
    if field(row, "hedgeFired", "hedge_fired"):
        return None
    elapsed = field(row, "elapsedMs", "elapsed_ms")
    return elapsed if isinstance(elapsed, int) else None


def nearest_rank(sorted_samples: list[int], q: float) -> int:
    """`hedge::percentile` — a value the sample actually holds. Here only to
    check the reconstruction against the recorded delays."""
    rank = max(1, math.ceil(q * len(sorted_samples)))
    return sorted_samples[min(rank, len(sorted_samples)) - 1]


def ledgers(roots: list[Path]) -> list[tuple[str, Path]]:
    found = []
    seen = set()
    for root in roots:
        for project in sorted(root.glob("*/state/smart-router")):
            for name, seat in LEDGERS.items():
                path = project / name
                if path.is_file() and path.resolve() not in seen:
                    seen.add(path.resolve())
                    found.append((seat, path))
    return found


def rows_of(path: Path) -> list[dict]:
    """The ledger's rows. A ledger pruned to its newer half can open on half a
    line, so an unreadable line is skipped rather than fatal."""
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


def firings_in(seat: str, path: Path) -> list[dict]:
    rows = rows_of(path)
    out = []
    for index, row in enumerate(rows):
        if not field(row, "hedgeFired", "hedge_fired"):
            continue
        window = rows[max(0, index - SAMPLE_ROWS) : index]
        samples = [s for s in (one_requests_own_latency(r) for r in window) if s is not None]
        out.append(
            {
                "seat": seat,
                "at": row.get("at"),
                "samples": samples,
                "recordedDelayMs": field(row, "hedgeDelayMs", "hedge_delay_ms"),
                "outcome": row.get("outcome"),
                "won": bool(field(row, "hedgeWon", "hedge_won")),
                "loserMs": field(row, "loserMs", "loser_ms"),
                "elapsedMs": field(row, "elapsedMs", "elapsed_ms"),
            }
        )
    return out


def reconstruction_holds(firings: list[dict]) -> tuple[int, int]:
    """How many firings' recorded delays the reconstructed samples reproduce,
    under the rule as it stood when they were written."""
    held = 0
    for firing in firings:
        recorded = firing["recordedDelayMs"]
        if type(recorded) is not int or len(firing["samples"]) < MIN_SAMPLES:
            continue
        sorted_samples = sorted(firing["samples"])
        by_load = nearest_rank(sorted_samples, 0.75)
        median = nearest_rank(sorted_samples, 0.5)
        held += int(min(by_load, WALL_MS - median) == recorded)
    # Every firing is a claim, including one whose history or delay is missing.
    # Dropping those claims made an unreadable ledger verify as 0/0.
    return held, len(firings)


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
    firings: list[dict] = []
    for seat, path in ledgers([Path(root).expanduser() for root in roots]):
        firings.extend(firings_in(seat, path))
    firings.sort(key=lambda f: (f["seat"], f["at"] or 0))

    held, recorded = reconstruction_holds(firings)
    print(
        f"{len(firings)} firings from {len(roots)} root(s); "
        f"{held}/{recorded} recorded delays reproduced from the reconstructed samples",
        file=sys.stderr,
    )
    if recorded and held != recorded:
        print(
            "the seed did not reproduce every recorded delay — it read the wrong rows, "
            "and replaying it would compare the rule against a sample the wire never saw",
            file=sys.stderr,
        )
        return 1

    seed = {"wallMs": WALL_MS, "sampleRows": SAMPLE_ROWS, "firings": firings}
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
