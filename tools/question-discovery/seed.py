#!/usr/bin/env python3
"""The candidate table of the question search (t-6349): which Jev seats
hold labeled rows on this machine, how many, where each seat's label comes
from, and whether the asking stage can rebuild the seat's state at the
judgment's own clock — the facts a search is planned on.

This script **only reads files and counts**. It asks nothing, grades
nothing and reads no word of a state: a ledger row's `agreed` mark is
counted as the ledger wrote it, a seed's rows are counted, and the words in
the table below are the plan's, not a rule. The rows a search asks about,
their labels and their baselines are the asking stage's
(`smart_router::question_discovery`), through the seats' shipped functions.

Which ledgers are which is `tools/label-audit/seed.py`'s reading (`ledgers`),
loaded from there rather than copied.

    tools/question-discovery/seed.py --out seed.json
    tools/question-discovery/seed.py --patch-seed /tmp/patch-review-replay/seed.json \\
        --notify-seed /tmp/notify-replay/seed.json --out seed.json
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
from pathlib import Path

_AUDIT_PATH = Path(__file__).resolve().parents[1] / "label-audit" / "seed.py"
_spec = importlib.util.spec_from_file_location("label_audit_seed", _AUDIT_PATH)
audit = importlib.util.module_from_spec(_spec)
sys.modules.setdefault(_spec.name, audit)
_spec.loader.exec_module(audit)

# Each seat's ledger file, as its row of `JEV_USES` names it
# (`crates/zerocode-core/src/jev.rs`, `ledger:`); the test holds each copy
# to the Rust source.
LEDGERS = {
    "recall": "rerank-shadow.jsonl",
    "placement": "worker-placement.jsonl",
    "patch_review": "patch-review.jsonl",
    "notify": "notify-call.jsonl",
    "summon": "summon-choice.jsonl",
    "routing": "route-outcomes.jsonl",
}
# Whose house each ledger sits in: the window's `jev/` or a zo project's
# state (`label-audit/seed.py` reads both).
HOUSE = {"recall": "projects", "placement": "window", "patch_review": "projects", "notify": "window", "summon": "window", "routing": "projects"}

# The plan's candidates (t-6349 spec; report §6-8), each with where its label
# comes from and whether the asking stage can rebuild its state.
CANDIDATES = [
    {"seat": "patch_review", "label": "hindsight: the patch's lines edited again inside the window, or a check green after it (runtime::patch_review::hindsight_of_turn)",
     "stateFrom": "zo transcripts through the seat's own ask_reading/state (tools/patch-review-replay seed)", "supported": True},
    {"seat": "notify", "label": "the person's hand on the pane inside a minute, while present (notify_call::agreed)",
     "stateFrom": "the ring's facts at its own clock (tools/notify-replay seed) through notify_call::ask", "supported": True},
    {"seat": "recall", "label": "comparison: the note the turn read or cited was the one ranked first (rerank_shadow::mark); most turns touch no note",
     "stateFrom": "not rebuildable: the ledger keeps fingerprints, and the vault's notes at that clock are gone", "supported": False},
    {"seat": "placement", "label": "comparison: the pane not moved inside five minutes while somebody saw it (worker_placement::mark)",
     "stateFrom": "not rebuildable: the ledger keeps counts of panes and brief chars, not the panes", "supported": False},
    {"seat": "summon", "label": "comparison: the agent the coordinator chose (summon_choice) — a proxy the plan retires (§6-1: a pinned model decides)",
     "stateFrom": "rebuildable from the ledger and the authority store (tools/summon-replay seed); not run here", "supported": False},
    {"seat": "routing", "label": "what the turn did, read as a level (route_label), and the chat probe's recorded answer",
     "stateFrom": "zo transcripts through routing_state (tools/routing-replay seed); not run here", "supported": False},
]


def marks_of(rows: list[dict]) -> dict:
    """How many rows the ledger holds, and how its `agreed` marks fall."""
    agreed = sum(1 for row in rows if row.get("agreed") is True)
    disagreed = sum(1 for row in rows if row.get("agreed") is False)
    return {"rows": len(rows), "agreed": agreed, "disagreed": disagreed, "unmarked": len(rows) - agreed - disagreed}


def count_source(path: Path | None, kind: str) -> dict | None:
    if path is None or not path.is_file():
        return None
    seed = json.loads(path.read_text(encoding="utf-8"))
    if kind == "patch_review":
        transcripts = seed.get("transcripts") or []
        return {"path": str(path), "transcripts": len(transcripts), "editResults": sum(int(row.get("edits") or 0) for row in transcripts)}
    if kind == "notify":
        return {"path": str(path), "rings": len(seed.get("rows") or [])}
    return {"path": str(path)}


def build(zo_home: Path, sources: dict[str, Path | None]) -> dict:
    found = audit.ledgers(zo_home)
    table = []
    for candidate in CANDIDATES:
        seat = candidate["seat"]
        rows = found[HOUSE[seat]].get(LEDGERS[seat], [])
        table.append(dict(candidate, ledger=LEDGERS[seat], marks=marks_of(rows), source=count_source(sources.get(seat), seat)))
    return {"zoHome": str(zo_home), "candidates": table}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--zo-home", type=Path, default=Path(os.environ.get("ZO_STATE_DIR", Path.home() / ".zo")))
    parser.add_argument("--patch-seed", type=Path, default=None, help="tools/patch-review-replay/seed.py's output")
    parser.add_argument("--notify-seed", type=Path, default=None, help="tools/notify-replay/seed.py's output")
    parser.add_argument("--out", type=Path, default=None)
    args = parser.parse_args(argv)
    seed = build(args.zo_home.expanduser(), {"patch_review": args.patch_seed, "notify": args.notify_seed})
    for row in seed["candidates"]:
        marks = row["marks"]
        print(f"{row['seat']:<13} {row['ledger']:<24} rows {marks['rows']:>6} agreed {marks['agreed']:>5} disagreed {marks['disagreed']:>5} supported {row['supported']}", file=sys.stderr)
    text = json.dumps(seed, indent=1, ensure_ascii=False) + "\n"
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text, encoding="utf-8")
        print(f"wrote {args.out}", file=sys.stderr)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
