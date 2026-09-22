#!/usr/bin/env python3
"""Build one replay seed for the browser-read seat's agreement (t-6041).

The seed is an EXTRACTION and never a calculation: every number it carries
is copied out of a row of `browser-read.jsonl`, and the rule that turns a
row's answer into a fold lives in one place only
(`zerocode_core::browser_read`), which the harness calls on what the pages
said (`tools/browser-read-replay/gather.mjs` gathers those).

What this file does is join each read's row to the label its next press
wrote, seeing only what the ledger knew at the read's own clock:

  * a read row is one the window wrote (`read` key, an `outcome`);
  * its label is the row whose `label` names that read — written AFTER the
    read, by the press that followed it; a label older than its read is a
    row the ledger could not have written and is refused;
  * a read with no label is in the seed with `agreed: null` — not counted,
    not dropped.

The numbers a reader may compute from the seed are the pooled raw rates and
one Wilson lower bound per pass; the seed carries none of them.

Usage:

    python3 tools/browser-read-replay/seed.py \
        --ledger ~/.zo/jev/browser-read.jsonl \
        --out /tmp/browser-read/seed.json
"""

import argparse
import json
import pathlib
import sys

#: The row key a read is named by, and the key its label points back with.
READ_KEY = "read"
LABEL_KEY = "label"
#: The row key that carries the `agreed` mark the press wrote.
AGREED_KEY = "agreed"
#: The row key that says whether the read handed back the folded page.
APPLIED_KEY = "applied"

#: Copies of the seat's own numbers, held to their Rust source by
#: `tools/tests/test_browser_read_replay_seed.py` (ConstantsMatchTheirSource):
#: the wall every shard is asked under, the fewest permille a chrome answer
#: must carry before the fold drops it, and the most blocks one read asks
#: about.
WALL_MS = 1_500
FOLD_FLOOR_PERMILLE = 700
BLOCK_CAP = 48


def rows(path: pathlib.Path) -> list[dict]:
    held = []
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(row, dict):
            held.append(row)
    return held


def is_read(row: dict) -> bool:
    return READ_KEY in row and "outcome" in row and "transition" not in row


def is_label(row: dict) -> bool:
    return LABEL_KEY in row and AGREED_KEY in row


def join(held: list[dict]) -> list[dict]:
    """Each read row with the label that followed it, or `agreed: null`."""
    labels = {}
    for row in held:
        if not is_label(row):
            continue
        # One label per read: the first written wins, later ones are noise.
        labels.setdefault(row[LABEL_KEY], row)
    replays = []
    for row in held:
        if not is_read(row):
            continue
        label = labels.get(row[READ_KEY])
        if label is not None and label.get("at", 0) < row.get("at", 0):
            raise ValueError(f"label {row[READ_KEY]} is older than its read")
        replays.append({
            "read": row[READ_KEY],
            "at": row.get("at"),
            "host": row.get("host"),
            "pathFingerprint": row.get("pathFingerprint"),
            "mode": row.get("mode"),
            "outcome": row.get("outcome"),
            "elapsedMs": row.get("elapsedMs"),
            "blocks": row.get("blocks"),
            "asked": row.get("asked"),
            "shards": row.get("shards"),
            "chrome": row.get("chrome"),
            "droppable": row.get("droppable"),
            "folded": row.get("folded"),
            "charsBefore": row.get("charsBefore"),
            "charsAfter": row.get("charsAfter"),
            "applied": row.get(APPLIED_KEY),
            "agreed": None if label is None else bool(label[AGREED_KEY]),
            "labelVerb": None if label is None else label.get("verb"),
            "labelBlockPath": None if label is None else label.get("blockPath"),
        })
    return replays


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ledger", required=True, type=pathlib.Path)
    parser.add_argument("--out", required=True, type=pathlib.Path)
    args = parser.parse_args()
    if not args.ledger.exists():
        print(f"seed: no ledger at {args.ledger}", file=sys.stderr)
        return 1
    try:
        replays = join(rows(args.ledger))
    except ValueError as why:
        print(f"seed: {why}", file=sys.stderr)
        return 1
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps({
        "seat": "browser_read",
        "wallMs": WALL_MS,
        "foldFloorPermille": FOLD_FLOOR_PERMILLE,
        "blockCap": BLOCK_CAP,
        "replays": replays,
    }, indent=1, ensure_ascii=False) + "\n")
    labelled = sum(1 for row in replays if row["agreed"] is not None)
    print(f"{len(replays)} reads, {labelled} labelled → {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
