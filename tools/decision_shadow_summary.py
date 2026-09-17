#!/usr/bin/env python3
"""Read the routing decision-shadow ledger without labels.

`zo decision-shadow eval` needs labels a person wrote. Before anyone has written
them, the only question is whether the shadow is worth labelling at all: how many
rows there are, how often the judgment answered, how long it took off the cache,
and how often it agreed with the probe it shadows. This prints exactly that, per
ledger and in total, from the JSON lines the shadow already writes. It sends
nothing and needs no key.

Rows carry a task fingerprint, never the task's words, so this reads nothing a
person typed.

The two Jev ledgers spell their fields differently — the routing shadow's rows
are camelCase (`elapsedMs`, `inputTokens`, `rubricVersion`), the rerank
shadow's snake_case — so every field is read through `field()`, which takes
either. Read with one spelling only, the routing ledger's latency came out as
0 ms.

    tools/decision_shadow_summary.py            # every project ledger under ~/.zo
    tools/decision_shadow_summary.py --ledger P # one file
    tools/decision_shadow_summary.py --json
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections import Counter
from pathlib import Path

ANSWERED = "answered"
# The design's floor before a shadow is read as evidence
# (docs/design/jev-decision-shadow-20260917.md §5).
EVIDENCE_FLOOR_ROWS = 200


def default_ledgers() -> list[Path]:
    home = Path(os.environ.get("ZO_STATE_DIR") or Path.home() / ".zo")
    return sorted(home.glob("projects/*/state/smart-router/decision-shadow.jsonl"))


def read_rows(path: Path) -> list[dict]:
    rows = []
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return rows
    for line in text.splitlines():
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


def field(row: dict, name: str, default=None):
    """`row[name]` in either spelling the ledgers write: snake_case or camelCase."""
    if name in row:
        return row[name]
    head, *rest = name.split("_")
    camel = head + "".join(part[:1].upper() + part[1:] for part in rest)
    return row.get(camel, default)


def sent_a_request(row: dict) -> bool:
    """Whether a row's judgment went over the wire. A row written since the Jev
    door says so in `requests` — none for a refusal (`not_consented`, `budget`,
    `off`, `no_key`) or a recall, one plus its retries when it left; a row from
    before says so the only way it can: not cached, and not `no_key`."""
    requests = field(row, "requests")
    if requests is not None:
        return int(requests) > 0
    return not row.get("cached") and row.get("outcome") != "no_key"


def percentile(values: list[int], share: float) -> int | None:
    if not values:
        return None
    ordered = sorted(values)
    # Nearest-rank, so a p95 is a value that was actually observed.
    rank = max(1, int(round(share * len(ordered))))
    return ordered[min(rank, len(ordered)) - 1]


def summarise(rows: list[dict]) -> dict:
    outcomes = Counter(str(row.get("outcome")) for row in rows)
    cached = sum(1 for row in rows if row.get("cached"))
    # A row that sent nothing — a recall, a missing key, the door's refusal —
    # would only flatter the latency, so it is left out of it: the same rule the
    # ledger's own `called()` applies.
    timed = [int(field(row, "elapsed_ms", 0)) for row in rows if sent_a_request(row)]
    answered = [row for row in rows if row.get("outcome") == ANSWERED]
    retried = sum(1 for row in answered if int(field(row, "retries", 0)) > 0)
    tokens = [int(field(row, "input_tokens")) for row in answered if field(row, "input_tokens") is not None]
    models = Counter(str(row.get("model")) for row in answered if row.get("model"))
    rubrics = Counter(int(field(row, "rubric_version", 0)) for row in rows)
    redacted = sum(int(field(row, "redacted_lines", 0) or 0) for row in rows)

    # Agreement is only defined where both readers answered the same task.
    both = [
        row
        for row in answered
        if isinstance(row.get("probe"), dict) and isinstance(row.get("jev"), dict)
    ]
    per_axis: dict[str, dict] = {}
    agree_all = 0
    for row in both:
        probe = row["probe"]
        jev = row["jev"]
        every_axis_agrees = True
        for axis, cell in jev.items():
            if not isinstance(cell, dict) or axis not in probe:
                continue
            slot = per_axis.setdefault(
                axis,
                {"rows": 0, "agree": 0, "confidence_sum": 0.0, "jev_choices": Counter(), "probe_choices": Counter()},
            )
            slot["rows"] += 1
            jev_choice = str(cell.get("choice"))
            probe_choice = str(probe[axis])
            slot["jev_choices"][jev_choice] += 1
            slot["probe_choices"][probe_choice] += 1
            if jev_choice == probe_choice:
                slot["agree"] += 1
            else:
                every_axis_agrees = False
            slot["confidence_sum"] += float(cell.get("confidence", 0.0))
        if every_axis_agrees:
            agree_all += 1

    axes = {}
    for axis, slot in sorted(per_axis.items()):
        n = slot["rows"]
        axes[axis] = {
            "rows": n,
            "agreement": round(slot["agree"] / n, 3) if n else None,
            "mean_confidence": round(slot["confidence_sum"] / n, 3) if n else None,
            "jev_choices": dict(slot["jev_choices"].most_common()),
            "probe_choices": dict(slot["probe_choices"].most_common()),
        }

    return {
        "rows": len(rows),
        "outcomes": dict(outcomes.most_common()),
        "cached": cached,
        "redacted_lines": redacted,
        "answered": len(answered),
        "answered_share": round(len(answered) / len(rows), 3) if rows else None,
        "retried": retried,
        "latency_ms": {
            "timed_rows": len(timed),
            "p50": percentile(timed, 0.50),
            "p95": percentile(timed, 0.95),
            "max": max(timed) if timed else None,
        },
        "input_tokens": {
            "rows": len(tokens),
            "total": sum(tokens),
            "mean": round(sum(tokens) / len(tokens)) if tokens else None,
        },
        "models": dict(models.most_common()),
        "rubric_versions": {str(k): v for k, v in sorted(rubrics.items())},
        "both_answered": len(both),
        "all_axes_agree_share": round(agree_all / len(both), 3) if both else None,
        "axes": axes,
        "evidence_floor": {
            "rows_needed": EVIDENCE_FLOOR_ROWS,
            "both_answered": len(both),
            "reached": len(both) >= EVIDENCE_FLOOR_ROWS,
        },
    }


def render(name: str, s: dict) -> str:
    lines = [f"{name}"]
    lines.append(
        f"  rows {s['rows']}  answered {s['answered']}"
        + (f" ({s['answered_share']:.0%})" if s["answered_share"] is not None else "")
        + f"  cached {s['cached']}  retried {s['retried']}"
    )
    if s["outcomes"]:
        lines.append("  outcomes " + ", ".join(f"{k} {v}" for k, v in s["outcomes"].items()))
    if s["redacted_lines"]:
        lines.append(f"  lines withheld at the Jev door {s['redacted_lines']}")
    lat = s["latency_ms"]
    if lat["timed_rows"]:
        lines.append(f"  latency (uncached, n={lat['timed_rows']}) p50 {lat['p50']} ms  p95 {lat['p95']} ms  max {lat['max']} ms")
    tok = s["input_tokens"]
    if tok["rows"]:
        lines.append(f"  input tokens total {tok['total']}  mean {tok['mean']}  (n={tok['rows']})")
    if s["models"]:
        lines.append("  models " + ", ".join(f"{k} ×{v}" for k, v in s["models"].items()))
    if s["both_answered"]:
        lines.append(
            f"  probe vs jev: both answered {s['both_answered']}  all axes agree {s['all_axes_agree_share']:.0%}"
        )
        for axis, a in s["axes"].items():
            lines.append(
                f"    {axis:<11} agree {a['agreement']:.0%}  mean confidence {a['mean_confidence']:.2f}"
                f"  jev {a['jev_choices']}  probe {a['probe_choices']}"
            )
    floor = s["evidence_floor"]
    lines.append(
        f"  evidence floor: {floor['both_answered']}/{floor['rows_needed']} rows where both answered"
        + ("  — reached; label a sample and run `zo decision-shadow eval`" if floor["reached"] else "  — not yet")
    )
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--ledger", type=Path, action="append", help="a decision-shadow.jsonl; repeatable")
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    args = parser.parse_args(argv)

    ledgers = args.ledger or default_ledgers()
    if not ledgers:
        print("no decision-shadow ledger found under ~/.zo/projects/*/state/smart-router/", file=sys.stderr)
        print("turn it on with smart.decisionShadow = \"shadow\" and run a few turns first", file=sys.stderr)
        return 1

    per_ledger = {}
    every_row: list[dict] = []
    for path in ledgers:
        rows = read_rows(path)
        every_row.extend(rows)
        per_ledger[str(path)] = summarise(rows)
    total = summarise(every_row)

    if args.json:
        print(json.dumps({"ledgers": per_ledger, "total": total}, ensure_ascii=False, indent=2))
        return 0

    for path, s in per_ledger.items():
        print(render(shorten(path), s))
        print()
    if len(per_ledger) > 1:
        print(render("total", total))
    return 0


def shorten(path: str) -> str:
    home = str(Path.home())
    return path.replace(home, "~", 1) if path.startswith(home) else path


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
