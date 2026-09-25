#!/usr/bin/env python3
"""Extract seven-day skill-turn counts without keeping requests or skill names.

A verified gold file may later pair each hash id with a skill hash or null.
An assisted run file may pair it with the agent's first Skill load. Merely
comparing Jev's suggestion to an agent's old load is not accuracy evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import time
from pathlib import Path


def digest(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()[:20]


def read_jsonl(path: Path):
    with path.open(encoding="utf-8", errors="replace") as source:
        for line in source:
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(row, dict):
                yield row


def tool_skill(block: dict) -> str | None:
    if block.get("type") != "tool_use" or block.get("name") not in {"Skill", "skill_load"}:
        return None
    raw = block.get("input")
    if isinstance(raw, str):
        try:
            raw = json.loads(raw)
        except json.JSONDecodeError:
            return None
    if not isinstance(raw, dict):
        return None
    name = raw.get("skill") or raw.get("name")
    return name if isinstance(name, str) and name else None


def extract(path: Path, since_ms: int) -> list[dict]:
    turns: list[dict] = []
    current: dict | None = None
    for row in read_jsonl(path):
        if row.get("type") != "message" or (row.get("updated_at_ms") or 0) < since_ms:
            continue
        message = row.get("message")
        if not isinstance(message, dict):
            continue
        if message.get("role") == "user":
            index = row.get("turn_index")
            if not isinstance(index, int):
                continue
            current = {"id": digest(f"{path}:{index}"), "firstLoad": None, "skillCalls": 0}
            turns.append(current)
        if current is None:
            continue
        for block in message.get("blocks", []):
            if not isinstance(block, dict):
                continue
            name = tool_skill(block)
            if name is not None:
                current["skillCalls"] += 1
                if current["firstLoad"] is None:
                    current["firstLoad"] = digest(name)
    return turns


def wilson_lower(hits: int, total: int) -> float | None:
    if total == 0:
        return None
    z = 1.96
    p = hits / total
    return (p + z * z / (2 * total) - z * ((p * (1 - p) / total + z * z / (4 * total * total)) ** 0.5)) / (1 + z * z / total)


def evaluate(seed: list[dict], gold: dict[str, dict], assisted: dict[str, dict]) -> dict:
    covered = [row for row in seed if row["id"] in gold and gold[row["id"]].get("skill") is not None]
    uncovered = [row for row in seed if row["id"] in gold and gold[row["id"]].get("skill") is None]
    covered_ids = {row["id"] for row in covered}
    uncovered_ids = {row["id"] for row in uncovered}
    results: dict = {"covered": len(covered), "uncovered": len(uncovered)}
    for arm, get_load in [
        ("baseline", lambda row: row["firstLoad"]),
        ("assisted", lambda row: assisted[row["id"]].get("firstLoad")),
    ]:
        rows = [row for row in covered + uncovered if arm == "baseline" or row["id"] in assisted]
        good_covered = sum(get_load(row) == gold[row["id"]]["skill"] for row in rows if row["id"] in covered_ids)
        good_uncovered = sum(get_load(row) is None for row in rows if row["id"] in uncovered_ids)
        compared_covered = sum(row["id"] in covered_ids for row in rows)
        compared_uncovered = sum(row["id"] in uncovered_ids for row in rows)
        results[arm] = {
            "comparedCovered": compared_covered,
            "wrongLoad": compared_covered - good_covered,
            "accuracyLower95": wilson_lower(good_covered, compared_covered),
            "comparedUncovered": compared_uncovered,
            "needlessLoad": compared_uncovered - good_uncovered,
        }
    paired = [row for row in covered + uncovered if row["id"] in assisted]
    paired_covered = [row for row in paired if row["id"] in covered_ids]
    paired_uncovered = [row for row in paired if row["id"] in uncovered_ids]
    fixed = sum(
        row["firstLoad"] != gold[row["id"]]["skill"]
        and assisted[row["id"]].get("firstLoad") == gold[row["id"]]["skill"]
        for row in paired
    )
    harmed = sum(
        row["firstLoad"] == gold[row["id"]]["skill"]
        and assisted[row["id"]].get("firstLoad") != gold[row["id"]]["skill"]
        for row in paired
    )
    results["paired"] = len(paired)
    results["fixed"] = fixed
    results["harmed"] = harmed
    base_wrong_paired = sum(row["firstLoad"] != gold[row["id"]]["skill"] for row in paired_covered)
    base_needless_paired = sum(row["firstLoad"] is not None for row in paired_uncovered)
    results["completionMet"] = (
        bool(paired_covered and paired_uncovered)
        and results["assisted"]["wrongLoad"] < base_wrong_paired
        and results["assisted"]["needlessLoad"] < base_needless_paired
        and harmed * 3 < fixed
    )
    return results


# The ledgers the turn-start suggestion's labels are in: its own since it
# became a seat of its own (t-6877; `zerocode_core::jev::SKILL_SUGGESTION`),
# and the search's, which it shared before.
SUGGESTION_LEDGERS = ("skill-suggestion.jsonl", "skill-search.jsonl")


def observed_labels(root: Path, since_ms: int) -> dict[str, int]:
    rows = (
        row
        for ledger in SUGGESTION_LEDGERS
        for path in root.glob(f"*/state/smart-router/{ledger}")
        for row in read_jsonl(path)
        if (row.get("at") or 0) >= since_ms and "agreed" in row
    )
    counts = {"compared": 0, "agreed": 0, "disagreed": 0,
              "unusedObserved": 0, "unusedLoad": 0, "baselineUnusedObserved": 0,
              "baselineUnusedLoad": 0}
    for row in rows:
        counts["compared"] += 1
        counts["agreed" if row["agreed"] else "disagreed"] += 1
        if isinstance(row.get("unusedLoad"), bool):
            counts["unusedObserved"] += 1
            counts["unusedLoad"] += int(row["unusedLoad"])
        if isinstance(row.get("baselineUnusedLoad"), bool):
            counts["baselineUnusedObserved"] += 1
            counts["baselineUnusedLoad"] += int(row["baselineUnusedLoad"])
    return counts


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sessions", type=Path, default=Path.home() / ".zo/projects")
    parser.add_argument("--days", type=int, default=7)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--gold", type=Path)
    parser.add_argument("--assisted", type=Path)
    parser.add_argument("--daily-turns", type=int, default=300)
    parser.add_argument("--usd-per-million-input", type=float, default=0.042)
    args = parser.parse_args()
    since_ms = int(time.time() * 1000) - args.days * 86_400_000
    seed = [
        row
        for path in args.sessions.glob("*/sessions/*.jsonl")
        if path.stat().st_mtime * 1_000 >= since_ms
        for row in extract(path, since_ms)
    ]
    seed.sort(key=lambda row: row["id"])
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text("".join(json.dumps(row, sort_keys=True) + "\n" for row in seed))
    summary = {
        "turns": len(seed),
        "skillCalls": sum(row["skillCalls"] for row in seed),
        "turnsWithSkill": sum(row["firstLoad"] is not None for row in seed),
        "turnsWithoutSkill": sum(row["firstLoad"] is None for row in seed),
        "labels": observed_labels(args.sessions, since_ms),
    }
    if args.gold:
        gold = {row["id"]: row for row in read_jsonl(args.gold)}
        assisted = {row["id"]: row for row in read_jsonl(args.assisted)} if args.assisted else {}
        summary["evaluation"] = evaluate(seed, gold, assisted)
    if args.assisted:
        assisted = list(read_jsonl(args.assisted))
        billed = sum(row.get("inputTokens", 0) for row in assisted)
        summary["billing"] = {
            "measuredTurns": len(assisted),
            "inputTokens": billed,
            "requests": sum(row.get("requests", 0) for row in assisted),
            "measuredUsd": round(billed * args.usd_per_million_input / 1_000_000, 6),
            "projectedDailyUsd": round(
                billed / len(assisted) * args.daily_turns * args.usd_per_million_input / 1_000_000, 6
            ) if assisted else None,
        }
    print(json.dumps(summary, sort_keys=True))


if __name__ == "__main__":
    main()
