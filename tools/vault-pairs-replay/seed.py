#!/usr/bin/env python3
"""Join vault pair judgments to later explicit weekly-review labels.

The seed contains hashed pair ids and numeric answers only. Labels never
become request state; a mark made before the judgment is ignored.
"""

import argparse
import hashlib
import json
from pathlib import Path


def rows(path: Path):
    for line in path.read_text().splitlines():
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(item, dict):
            yield item


def key(row):
    return (
        row.get("left"), row.get("right"),
        row.get("leftModifiedMs"), row.get("rightModifiedMs"),
    )


def seed(path: Path):
    found = list(rows(path))
    labels = {}
    for row in found:
        if row.get("kind") == "label":
            labels.setdefault(key(row), []).append(row)
    samples = []
    for row in found:
        if row.get("outcome") != "answered" or row.get("kind") == "label":
            continue
        after = [label for label in labels.get(key(row), [])
                 if label.get("at", -1) >= row.get("at", 0)]
        actual = min(after, key=lambda label: label["at"]).get("actual") if after else None
        pair = hashlib.sha256((str(row.get("left")) + "\0" + str(row.get("right"))).encode()).hexdigest()[:16]
        samples.append({
            "pair": pair,
            "proposed": row.get("proposal"),
            "actual": actual,
            "link_state": row.get("linkState"),
            "same_claim": row.get("sameClaim"),
            "opposite_claim": row.get("oppositeClaim"),
            "replaces": row.get("replaces"),
            "input_tokens": row.get("inputTokens", 0),
            "requests": row.get("requests", 0),
        })
    return {"samples": samples}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ledger", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    result = seed(args.ledger)
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    labeled = sum(row["actual"] is not None for row in result["samples"])
    print(f"answered={len(result['samples'])} labeled={labeled}")


if __name__ == "__main__":
    main()
