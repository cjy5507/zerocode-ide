#!/usr/bin/env python3
"""Gather every question the orchestration ledger holds and what its asker
could have known about the receiver while waiting — the baseline for the
receiver-notice rule (t-6740, Traycer T4).

This script **only reads the ledger and copies facts**. It decides nothing:
which receiver facts become a notice, and what that notice says, is the
`RECEIVER_NOTICE` table in `crates/zerocode-core/src/orchestration.rs`, and
the ledger's own reading of "answered" is `Message::answers` there. A second
copy of either in Python would be a second rule.

What a seed row is: one root question (`kind = question`, no thread) with
the facts the ledger held ABOUT ITS RECEIVER between the question and the
moment the asker stopped waiting — and only facts the ledger wrote at the
time, keyed by when it wrote them, never a later fact moved earlier:

  * who asked whom (address heads only: `worker`, `run`, `pane`);
  * when the wait ended and why: the answer landed (`answered`), the
    asker's own dispatch ended first (`closed`), or the ledger's last write
    came with the question still open (`censored` — right-censored, and
    counted apart from the other two on purpose);
  * for a receiver that is a worker: whether it had an open attempt when
    asked; the turn ends, stall notices, judged causes and quota walls the
    ledger recorded about it while the asker waited (each with the fact's
    own time and the time the ledger learned it); and whether its attempt
    ended before the wait did — and how (`done`, `died`, `stopped`, which
    is the ledger's word for an attempt closed by a stop or an abandon);
  * for a receiver that is a run (its coordinator) or a bare pane: nothing
    — the ledger records no turn facts about a coordinator's pane, and the
    row says so (`receiver_facts: unobservable`) rather than guessing.

Ids are hashed and bodies are never copied. Only counts and durations leave.

    tools/ask-wait-replay/seed.py --out seed.json            # the window's ledger, read-only
    tools/ask-wait-replay/seed.py --db PATH --out seed.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sqlite3
import statistics
import sys
from pathlib import Path

# Where the window keeps its authority store (`crates/zerocode-shell/src/orchestration.rs`).
DEFAULT_DB = (
    Path.home()
    / "Library"
    / "Application Support"
    / "dev.zerocode.app"
    / "authority"
    / "authority.sqlite"
)

# `LEDGER_ITSELF` in `crates/zerocode-core/src/orchestration.rs`: the sender
# of every observation the ledger writes about a worker.
LEDGER_ITSELF = "ledger"

# `MessageKind::is_the_ledgers_own` there: kinds only the ledger writes. A
# row of one of these can never be a question's answer.
LEDGERS_OWN_KINDS = ("went_quiet", "deadlocked", "worker_died", "quota_walled", "handover", "resumed")

# `STALL_JUDGED_REASON` there.
STALL_JUDGED_REASON = "judged"

# The address heads the ledger routes (`WORKER_ADDRESS_PREFIX`, `RUN_ADDRESS_PREFIX`,
# `PANE_ADDRESS_PREFIX`, `REMOTE_ADDRESS_PREFIX`).
ADDRESS_HEADS = ("worker", "run", "pane", "remote", "home")


def address_head(address: str) -> str:
    head = address.split(":", 1)[0]
    return head if head in ADDRESS_HEADS else "other"


def hashed(value: str | None) -> str | None:
    if value is None:
        return None
    return hashlib.sha256(value.encode()).hexdigest()[:12]


def open_read_only(path: Path) -> sqlite3.Connection:
    """The store, read-only. `mode=ro` refuses every write at the door, so a
    bug here cannot move the person's ledger."""
    uri = f"file:{path}?mode=ro"
    connection = sqlite3.connect(uri, uri=True)
    connection.row_factory = sqlite3.Row
    return connection


def body_json(text: str) -> dict:
    try:
        held = json.loads(text)
    except (json.JSONDecodeError, TypeError):
        return {}
    return held if isinstance(held, dict) else {}


def percentile(values: list[int], share: float) -> int | None:
    if not values:
        return None
    ordered = sorted(values)
    at = min(len(ordered) - 1, max(0, round(share * (len(ordered) - 1))))
    return ordered[at]


class Ledger:
    """The tables a question's wait is read from, loaded once."""

    def __init__(self, connection: sqlite3.Connection):
        self.messages = [dict(row) for row in connection.execute(
            "SELECT run, id, sender, recipient, kind, body, thread, task, dispatch, created_ms "
            "FROM ledger_messages ORDER BY ordinal"
        )]
        self.dispatches = [dict(row) for row in connection.execute(
            "SELECT run, id, task, worker, started_ms, ended_ms, succeeded FROM ledger_dispatches"
        )]
        self.workers = {row["id"]: dict(row) for row in connection.execute(
            "SELECT run, id, agent, state, started_ms, dispatch, taken_over FROM ledger_workers"
        )}
        self.observed_until_ms = max((row["created_ms"] for row in self.messages), default=0)
        self.observed_from_ms = min((row["created_ms"] for row in self.messages), default=0)
        self.by_thread: dict[str, list[dict]] = {}
        for row in self.messages:
            if row["thread"]:
                self.by_thread.setdefault(row["thread"], []).append(row)
        self.dispatch_by_id = {row["id"]: row for row in self.dispatches}
        # The ledger's observations about each worker, in the order written.
        self.facts_by_worker: dict[str, list[tuple[dict, dict]]] = {}
        for row in self.messages:
            if row["sender"] != LEDGER_ITSELF or row["kind"] not in ("went_quiet", "worker_died", "quota_walled"):
                continue
            body = body_json(row["body"])
            worker = body.get("workerId")
            if isinstance(worker, str):
                self.facts_by_worker.setdefault(worker, []).append((row, body))
        self.done_by_worker: dict[str, list[int]] = {}
        for row in self.messages:
            if row["kind"] == "worker_done" and row["sender"].startswith("worker:"):
                self.done_by_worker.setdefault(row["sender"][len("worker:"):], []).append(row["created_ms"])

    def root_questions(self) -> list[dict]:
        return [row for row in self.messages if row["kind"] == "question" and not row["thread"]]

    def answer_of(self, question: dict) -> tuple[dict | None, dict | None]:
        """The first thread row from the asked seat that is not the ledger's
        voice — and, beside it, the first from that seat under today's looser
        reading (any kind, any author), so a row the two disagree on is
        counted rather than silently taken."""
        thread = self.by_thread.get(question["id"], [])
        loose = next((row for row in thread if row["sender"] == question["recipient"]), None)
        strict = next(
            (
                row
                for row in thread
                if row["sender"] == question["recipient"]
                and row["sender"] != LEDGER_ITSELF
                and row["kind"] not in LEDGERS_OWN_KINDS
            ),
            None,
        )
        return strict, loose

    def receiver_attempt(self, worker_id: str, at_ms: int) -> dict | None:
        """The receiver's dispatch that was open when the question was asked."""
        for row in self.dispatches:
            if row["worker"] != worker_id or row["started_ms"] > at_ms:
                continue
            if row["ended_ms"] is None or row["ended_ms"] > at_ms:
                return row
        return None


def question_row(ledger: Ledger, question: dict, earlier: dict[tuple[str, str, str], list[dict]]) -> dict:
    asked_ms = question["created_ms"]
    strict, loose = ledger.answer_of(question)
    closed_ms = None
    if question["dispatch"]:
        dispatch = ledger.dispatch_by_id.get(question["dispatch"])
        if dispatch and dispatch["ended_ms"] is not None and dispatch["ended_ms"] >= asked_ms:
            closed_ms = dispatch["ended_ms"]
    if strict is not None and (closed_ms is None or strict["created_ms"] <= closed_ms):
        outcome, end_ms = "answered", strict["created_ms"]
    elif closed_ms is not None:
        outcome, end_ms = "closed", closed_ms
    else:
        outcome, end_ms = "censored", ledger.observed_until_ms
    row = {
        "question": hashed(question["id"]),
        "run": hashed(question["run"]),
        "asker": address_head(question["sender"]),
        "receiver": address_head(question["recipient"]),
        "asked_ms": asked_ms,
        "outcome": outcome,
        "wait_ms": max(0, end_ms - asked_ms),
        # A row today's reading would take as the answer that the stricter
        # one refuses — the ledger's own voice in the thread.
        "misread_answer": loose is not None and (strict is None or loose["id"] != strict["id"]),
    }
    body_key = (question["sender"], question["recipient"], hashlib.sha256(question["body"].encode()).hexdigest())
    repeats = earlier.get(body_key, [])
    row["repeat_of"] = next(
        (hashed(prior["id"]) for prior in repeats if prior["open_until_ms"] > asked_ms), None
    )
    earlier.setdefault(body_key, []).append({"id": question["id"], "open_until_ms": end_ms})

    if row["receiver"] != "worker":
        row["receiver_facts"] = "unobservable"
        return row
    worker_id = question["recipient"][len("worker:"):]
    attempt = ledger.receiver_attempt(worker_id, asked_ms)
    if attempt is None:
        known = worker_id in ledger.workers
        row["receiver_facts"] = "no_open_attempt" if known else "unknown_worker"
        return row
    row["receiver_facts"] = "observed"
    facts = ledger.facts_by_worker.get(worker_id, [])
    turn_ends = []
    stalls = []
    judged = []
    walls = []
    for held, body in facts:
        learned_ms = held["created_ms"]
        if learned_ms <= asked_ms or learned_ms > end_ms:
            continue
        if held["kind"] == "went_quiet" and body.get("dispatchId") == attempt["id"]:
            turn_ms = body.get("turnEndedMs")
            if isinstance(turn_ms, int) and turn_ms >= asked_ms:
                turn_ends.append({"fact_ms": turn_ms, "learned_ms": learned_ms})
            reason = body.get("reason")
            if reason == "stalled" and isinstance(body.get("stalledSinceMs"), int):
                stalls.append({"fact_ms": body["stalledSinceMs"], "learned_ms": learned_ms})
            elif reason == STALL_JUDGED_REASON and isinstance(body.get("cause"), str):
                judged.append({"fact_ms": body.get("stalledSinceMs"), "learned_ms": learned_ms, "cause": body["cause"]})
        elif held["kind"] == "quota_walled":
            walls.append({"learned_ms": learned_ms})
    row["turn_ends"] = turn_ends
    row["stalls"] = stalls
    row["judged"] = judged
    row["quota_walls"] = walls
    first_turn = min((one["fact_ms"] for one in turn_ends), default=None)
    row["wait_after_first_turn_end_ms"] = None if first_turn is None or first_turn >= end_ms else end_ms - first_turn
    ended_ms = attempt["ended_ms"]
    if ended_ms is not None and asked_ms < ended_ms < end_ms:
        if any(held["kind"] == "worker_died" and asked_ms < held["created_ms"] <= end_ms for held, _ in facts):
            ending = "died"
        elif any(asked_ms < done_ms <= ended_ms for done_ms in ledger.done_by_worker.get(worker_id, [])):
            ending = "done"
        else:
            ending = "stopped"
        row["receiver_ended"] = {"ending": ending, "fact_ms": ended_ms, "wait_after_ms": end_ms - ended_ms}
    else:
        row["receiver_ended"] = None
    return row


def gather(db: Path) -> dict:
    with open_read_only(db) as connection:
        ledger = Ledger(connection)
    earlier: dict[tuple[str, str, str], list[dict]] = {}
    rows = [question_row(ledger, question, earlier) for question in ledger.root_questions()]
    return {
        "db": hashed(str(db)),
        "observed_from_ms": ledger.observed_from_ms,
        "observed_until_ms": ledger.observed_until_ms,
        "runs": len({row["run"] for row in ledger.messages}),
        "rows": rows,
    }


def summarize(seed: dict) -> list[str]:
    rows = seed["rows"]
    lines = []
    span_h = (seed["observed_until_ms"] - seed["observed_from_ms"]) / 3_600_000
    lines.append(f"questions {len(rows)} over {span_h:.1f} h of ledger ({seed['runs']} runs)")
    pairs: dict[tuple[str, str], int] = {}
    for row in rows:
        pairs[(row["asker"], row["receiver"])] = pairs.get((row["asker"], row["receiver"]), 0) + 1
    lines.append("asker→receiver: " + ", ".join(f"{a}→{r} {n}" for (a, r), n in sorted(pairs.items(), key=lambda kv: -kv[1])))
    lines.append(f"repeats (same words re-asked while open): {sum(1 for row in rows if row['repeat_of'])}")
    lines.append(f"misread answers under today's reading: {sum(1 for row in rows if row['misread_answer'])}")
    lines.append("")
    lines.append("| outcome | n | total wait | mean | p50 | p95 |")
    lines.append("|---|---|---|---|---|---|")
    for outcome in ("answered", "closed", "censored"):
        waits = [row["wait_ms"] for row in rows if row["outcome"] == outcome]
        if not waits:
            lines.append(f"| {outcome} | 0 | — | — | — | — |")
            continue
        lines.append(
            f"| {outcome} | {len(waits)} | {sum(waits) / 1000:.0f} s | {statistics.mean(waits) / 1000:.0f} s "
            f"| {percentile(waits, 0.5) / 1000:.0f} s | {percentile(waits, 0.95) / 1000:.0f} s |"
        )
    unanswered = [row["wait_ms"] for row in rows if row["outcome"] != "answered"]
    lines.append(f"unanswered wait (closed + censored): n {len(unanswered)}, total {sum(unanswered) / 1000:.0f} s"
                 + (f", mean {statistics.mean(unanswered) / 1000:.0f} s" if unanswered else ""))
    lines.append("")
    workers = [row for row in rows if row["receiver"] == "worker"]
    facts = {}
    for row in workers:
        facts[row["receiver_facts"]] = facts.get(row["receiver_facts"], 0) + 1
    lines.append(f"worker receivers {len(workers)}: " + ", ".join(f"{k} {v}" for k, v in sorted(facts.items())))
    observed = [row for row in workers if row["receiver_facts"] == "observed"]
    with_turns = [row for row in observed if row["turn_ends"]]
    lines.append(f"  with a turn end recorded while waiting: {len(with_turns)} / {len(observed)}"
                 f" (no turn fact at all: {len(observed) - len(with_turns)})")
    after_turn = [row["wait_after_first_turn_end_ms"] for row in with_turns if row["wait_after_first_turn_end_ms"] is not None]
    if after_turn:
        lines.append(f"  wait after the receiver's first turn end: total {sum(after_turn) / 1000:.0f} s, mean "
                     f"{statistics.mean(after_turn) / 1000:.0f} s, p50 {percentile(after_turn, 0.5) / 1000:.0f} s, "
                     f"p95 {percentile(after_turn, 0.95) / 1000:.0f} s")
        for outcome in ("answered", "closed", "censored"):
            part = [row["wait_after_first_turn_end_ms"] for row in with_turns
                    if row["outcome"] == outcome and row["wait_after_first_turn_end_ms"] is not None]
            if part:
                lines.append(f"    {outcome}: n {len(part)}, total {sum(part) / 1000:.0f} s, mean {statistics.mean(part) / 1000:.0f} s")
    latencies = [one["learned_ms"] - one["fact_ms"] for row in with_turns for one in row["turn_ends"]]
    if latencies:
        lines.append(f"  turn end → ledger row (existing hook road): n {len(latencies)}, p50 {percentile(latencies, 0.5)} ms, "
                     f"p95 {percentile(latencies, 0.95)} ms")
    stalled = [row for row in observed if row["stalls"] or row["judged"]]
    lines.append(f"  with a stall notice or judged cause while waiting: {len(stalled)}"
                 + (" (" + ", ".join(sorted({one['cause'] for row in stalled for one in row['judged']})) + ")" if any(row["judged"] for row in stalled) else ""))
    ended = [row for row in observed if row["receiver_ended"]]
    lines.append(f"  receiver's attempt ended before the wait did: {len(ended)}")
    for ending in ("done", "died", "stopped"):
        part = [row["receiver_ended"]["wait_after_ms"] for row in ended if row["receiver_ended"]["ending"] == ending]
        if part:
            lines.append(f"    {ending}: n {len(part)}, wait after the ending total {sum(part) / 1000:.0f} s, "
                         f"mean {statistics.mean(part) / 1000:.0f} s, max {max(part) / 1000:.0f} s")
    return lines


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--db", type=Path, default=DEFAULT_DB, help="the authority store (opened read-only)")
    parser.add_argument("--out", type=Path, help="where to write the seed JSON")
    args = parser.parse_args()
    if not args.db.is_file():
        print(f"no ledger at {args.db}", file=sys.stderr)
        return 2
    seed = gather(args.db)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(seed, indent=1) + "\n")
    print("\n".join(summarize(seed)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
