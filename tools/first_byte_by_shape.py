#!/usr/bin/env python3
"""Why a model's first byte takes as long as it does — from the ledger, by shape.

`zo scoreboard` already says what each model's first byte costs. It cannot say
WHY, and the difference matters: a wait that grows with the request is ours to
fix by sending less, a wait that only the first request of a process pays is a
handshake, and a wait that is there at five messages and at five hundred alike
is the road's own floor and no amount of trimming will move it.

This cuts the request-timings ledger three ways and prints all three side by
side, per model:

  BY SIZE      first byte by how many messages the request carried. A road
               whose smallest bucket already costs what its largest does is
               not slow because of what we send it.
  BY POSITION  the first request of a session against every later one. A
               per-process handshake (a `loadCodeAssist` round trip, a token
               refresh, a cold TLS session) shows up here and nowhere else.
  WHOLE        every row, so the two cuts can be read against the number the
               scoreboard shows.

    tools/first_byte_by_shape.py                         # every project ledger
    tools/first_byte_by_shape.py --model gemini-3.8-flash
    tools/first_byte_by_shape.py --ledger PATH --json

It reads local files and sends nothing. The ledger carries no words a person
typed — a row is a model, a count and four durations — so nothing it prints
could be a person's text.

WHAT IT ANSWERED ONCE (t-4701, 2026-09-18, 2,565 rows on this machine):
`gemini-3.8-flash` read 3,307 ms at the median against `claude-opus-5`'s 1,525
and `claude-haiku-4-5`'s 771 — the name says flash and the reading says
otherwise. All three cuts, and what each ruled out:

  BY SIZE      its 0-5-message bucket ALREADY cost 2,562 ms (opus in the same
               bucket: 799). Context is not the cause. It adds — 3,022 ms at
               101-200 messages, 4,848 ms past 400 — but it is not the floor.
  BY POSITION  first request of a session 2,814 ms against 3,315 ms for the
               rest. No per-process handshake is being paid, which is what the
               fix recorded in the vault predicted
               (`gemini-first-request-paid-loadcodeassist-every-process`).
  WHAT IS LEFT the rung. This column is the name ZO picked, and the Antigravity
               registry serves that name only as `-low`/`-medium`/`-high`/
               `-tiered`; discovery folds the four back into one selection id
               (`model_discovery::…` "one selection id per release, each
               carrying the wire map"), so one ledger row covers four models.
               Their published thinking budgets, from `fetchAvailableModels`
               on 2026-09-18: low 1,000 tokens, medium 4,000, high and tiered
               unbounded.

Measured the same day on the same road, same session, alternating, one line
asked of each (`tools/type_value_latency.py`, medians of 9-11 calls over three
readings): `gemini-3.5-flash-lite`, which the registry serves with no thinking
at all, 817-945 ms; `gemini-3.8-flash-low` 1,225-1,842 ms;
`gemini-3.8-flash-high` 1,749-2,332 ms with its answer cut off at 32 tokens,
answering 1 time in 31 because the rest of the budget went on narration.
`claude-haiku-4-5` on Anthropic's own road, in the same rounds: 617-671 ms.

So the wait is two things, and only one of them is ours. OURS: we ask a flash
to think, and there is no rung below it — `thinkingLevel: "none"` is refused
(400, invalid value), `"minimal"` is refused for this model by name, and
sending no thinkingConfig at all still read 2,659 ms because the budget is
baked into the served id. The only road to a Gemini that does not think is a
different id, which is a CATALOG choice. THEIRS: the ~900 ms that a Code Assist
row with no thinking still costs, against ~640 ms for a comparable one-liner on
Anthropic's road. That part no request of ours can move.
"""

from __future__ import annotations

import argparse
import collections
import glob
import json
import os
import sys

# Message counts a request is filed under. The first bucket is the one that
# matters: whatever a road costs THERE, it costs before anything we sent.
SIZE_BUCKETS = [
    (0, 5, "0-5"),
    (6, 20, "6-20"),
    (21, 50, "21-50"),
    (51, 100, "51-100"),
    (101, 200, "101-200"),
    (201, 400, "201-400"),
    (401, 10**9, "401+"),
]

# A model with fewer rows than this is noise, not a reading.
MIN_ROWS = 20


def percentile(values: list[float], share: float) -> float:
    ordered = sorted(values)
    if not ordered:
        return float("nan")
    spot = (len(ordered) - 1) * share
    low, high = int(spot), min(int(spot) + 1, len(ordered) - 1)
    return ordered[low] + (ordered[high] - ordered[low]) * (spot - low)


def ledgers(named: str | None) -> list[str]:
    if named:
        return [named]
    home = os.path.expanduser("~/.zo/projects")
    return sorted(glob.glob(os.path.join(home, "*", "state", "request-timings", "timings.jsonl")))


def rows(paths: list[str]) -> list[dict]:
    found = []
    for path in paths:
        try:
            handle = open(path, encoding="utf-8")
        except OSError:
            continue
        with handle:
            for line in handle:
                line = line.strip()
                if not line:
                    continue
                try:
                    row = json.loads(line)
                except ValueError:
                    continue
                if row.get("ttfb_ms") is not None and row.get("model"):
                    found.append(row)
    return found


def cut(label: str, waits: list[float]) -> dict:
    return {
        "cut": label,
        "n": len(waits),
        "p10Ms": round(percentile(waits, 0.1)),
        "p50Ms": round(percentile(waits, 0.5)),
        "p90Ms": round(percentile(waits, 0.9)),
    }


def by_size(model_rows: list[dict]) -> list[dict]:
    out = []
    for low, high, label in SIZE_BUCKETS:
        waits = [
            row["ttfb_ms"] for row in model_rows if low <= row.get("request_messages", 0) <= high
        ]
        if waits:
            out.append(cut(label, waits))
    return out


def by_position(model_rows: list[dict]) -> list[dict]:
    """First request of a session against every later one.

    A session is a process, so `iteration` alone will not do — a turn's second
    iteration is still that process's second request. The rows are ordered
    within a session and the earliest one is the one that pays a handshake.
    """
    sessions = collections.defaultdict(list)
    for row in model_rows:
        sessions[row.get("session_id", "")].append(row)
    first, rest = [], []
    for taken in sessions.values():
        taken.sort(key=lambda row: (row.get("recorded_at", 0), row.get("iteration", 0)))
        first.append(taken[0]["ttfb_ms"])
        rest.extend(row["ttfb_ms"] for row in taken[1:])
    out = []
    if first:
        out.append(cut("first in session", first))
    if rest:
        out.append(cut("later", rest))
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ledger", help="one timings.jsonl instead of every project's")
    parser.add_argument("--model", default="", help="only models whose id contains this")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--min-rows", type=int, default=MIN_ROWS)
    args = parser.parse_args()

    found = rows(ledgers(args.ledger))
    if not found:
        print("no timing rows found", file=sys.stderr)
        return 2
    by_model = collections.defaultdict(list)
    for row in found:
        by_model[row["model"]].append(row)

    report = []
    for model, model_rows in sorted(by_model.items(), key=lambda pair: -len(pair[1])):
        if len(model_rows) < args.min_rows or (args.model and args.model not in model):
            continue
        report.append(
            {
                "model": model,
                "whole": cut("whole", [row["ttfb_ms"] for row in model_rows]),
                "bySize": by_size(model_rows),
                "byPosition": by_position(model_rows),
            }
        )
    if args.json:
        print(json.dumps(report, indent=2))
        return 0

    print(f"{len(found)} rows, {len(report)} models with at least {args.min_rows}\n")
    for entry in report:
        whole = entry["whole"]
        print(f"{entry['model']}  —  n={whole['n']}  p10 {whole['p10Ms']}  "
              f"p50 {whole['p50Ms']}  p90 {whole['p90Ms']} ms")
        print(f"    {'by size':>16}{'n':>6}{'p10':>8}{'p50':>8}{'p90':>8}")
        for line in entry["bySize"]:
            print(f"    {line['cut']:>16}{line['n']:>6}{line['p10Ms']:>8}{line['p50Ms']:>8}{line['p90Ms']:>8}")
        print(f"    {'by position':>16}{'n':>6}{'p10':>8}{'p50':>8}{'p90':>8}")
        for line in entry["byPosition"]:
            print(f"    {line['cut']:>16}{line['n']:>6}{line['p10Ms']:>8}{line['p50Ms']:>8}{line['p90Ms']:>8}")
        print()
    print("Read the first size bucket first: what a road costs there, it costs before")
    print("anything we sent it. A 'first in session' that is not slower than 'later'")
    print("means no per-process handshake is being paid.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
