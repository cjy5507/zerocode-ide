#!/usr/bin/env python3
"""Gather every question the orchestration ledger holds and what its asker
could have known about the receiver while waiting — the baseline for the
receiver-notice rule (t-6740, Traycer T4) — and, on a ledger written after
the rule, the notices the asker was actually sent.

This script **only reads the ledger and copies facts**. It decides nothing:
which receiver facts become a notice, and what that notice says, is the
`ReceiverNews` table in `crates/zerocode-core/src/orchestration.rs`, and the
ledger's own reading of "answered" is `Message::answers` there. A second
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
  * the BASELINE — what the ledger's own observations said about a worker
    receiver (`receiver_facts`): whether it had an open attempt when asked;
    the turn ends, stall notices, judged causes and quota walls the ledger
    recorded about it while the asker waited (each with the fact's own time
    and the time the ledger learned it); and whether its attempt ended
    before the wait did — and how, read off the ledger's record of THAT
    attempt's ending (`done`, `died`: the row filed under the attempt's own
    dispatch key; `stopped`, `abandoned`: the task's record while the
    attempt is the task's latest), or `unexplained` where the ledger kept
    no reason this replay can read. A row naming another attempt of the
    same worker, or no attempt at all, is never this attempt's ending. A run (its coordinator)
    or a bare pane has no baseline facts: the ledger wrote no turn facts
    about a coordinator's pane before the rule, and the row says so
    (`receiver_facts: unobservable`) rather than guessing;
  * the NOTICES — the rule's own lines (`notices`): each `status` line from
    `ledger` threaded on the question while the asker waited, with its
    reason word, whether it was final, the fact's time and the time the
    ledger wrote it. A ledger written before the rule holds none, for any
    receiver; after it, run and pane receivers have them too.

One ledger at a time: the authority store keys every row by `ledger_id`,
and ids are minted per ledger, so two ledgers in one store can both hold an
`m-1`. The one ledger the store holds is read; a store holding several is
refused until one is named.

Ids are hashed and bodies are never copied. Only words, counts and
durations leave.

    tools/ask-wait-replay/seed.py --out seed.json            # the window's ledger, read-only
    tools/ask-wait-replay/seed.py --db PATH --out seed.json
    tools/ask-wait-replay/seed.py --db PATH --ledger-id ID --out seed.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
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
# row of one of these can never be a question's answer. Of them only
# `worker_died` ends an attempt here: a `classifier_declined` row is news and
# never a settlement — a declined worker is ended by the outcome its
# coordinator or the handover walk writes into the task (`ENDING_OUTCOMES`) —
# a `model_deviated` row tells a switch the worker's CLI made, not an
# ending; and an `account_switched` row is the window's receipt that a walled
# worker went on under another Claude login with the same worker id, dispatch
# and conversation (t-7538) — news again, not an ending.
LEDGERS_OWN_KINDS = (
    "went_quiet",
    "deadlocked",
    "worker_died",
    "quota_walled",
    "handover",
    "resumed",
    "classifier_declined",
    "model_deviated",
    "account_switched",
)

# `STALL_JUDGED_REASON` there.
STALL_JUDGED_REASON = "judged"

# `Ending::as_str` there: the outcome an attempt's coordinator ended it with,
# as `end_attempt` writes it into the task's result.
ENDING_OUTCOMES = ("stopped", "abandoned")

# The address heads the ledger routes (`WORKER_ADDRESS_PREFIX`, `RUN_ADDRESS_PREFIX`,
# `PANE_ADDRESS_PREFIX`, `REMOTE_ADDRESS_PREFIX`).
ADDRESS_HEADS = ("worker", "run", "pane", "remote", "home")

# The tables a ledger's rows live in, every one keyed by `ledger_id`.
LEDGER_TABLES = ("ledger_messages", "ledger_dispatches", "ledger_workers", "ledger_tasks")


class LedgerScopeError(Exception):
    """The store does not name the one ledger this replay should read."""


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


def body_json(text: str | None) -> dict:
    try:
        held = json.loads(text or "")
    except (json.JSONDecodeError, TypeError):
        return {}
    return held if isinstance(held, dict) else {}


def percentile(values: list[int], share: float) -> int | None:
    if not values:
        return None
    ordered = sorted(values)
    at = min(len(ordered) - 1, max(0, round(share * (len(ordered) - 1))))
    return ordered[at]


def tables_of(connection: sqlite3.Connection) -> set[str]:
    return {row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type = 'table'")}


def ledger_ids(connection: sqlite3.Connection) -> list[str]:
    """Every ledger the store holds rows for."""
    present = tables_of(connection)
    held: set[str] = set()
    for table in LEDGER_TABLES:
        if table in present:
            held.update(row[0] for row in connection.execute(f"SELECT DISTINCT ledger_id FROM {table}"))
    return sorted(held)


def scoped_ledger(connection: sqlite3.Connection, asked: str | None) -> str:
    """The one ledger this replay reads: the one named, or the only one the
    store holds. Several and none named is refused, because rows from two
    ledgers under one id map would pin one ledger's answer on the other's
    question."""
    held = ledger_ids(connection)
    if asked is None:
        if len(held) != 1:
            raise LedgerScopeError(
                f"the store holds {len(held)} ledgers ({', '.join(held) or 'none'}) — name one with --ledger-id"
            )
        return held[0]
    if asked not in held:
        raise LedgerScopeError(f"the store holds no ledger {asked!r} ({', '.join(held) or 'none'})")
    return asked


class Ledger:
    """The tables a question's wait is read from, loaded once — for ONE
    ledger, every read scoped by its `ledger_id`."""

    def __init__(self, connection: sqlite3.Connection, ledger_id: str):
        scope = (ledger_id,)
        self.messages = [dict(row) for row in connection.execute(
            "SELECT run, id, sender, recipient, kind, body, thread, task, dispatch, created_ms "
            "FROM ledger_messages WHERE ledger_id = ? ORDER BY ordinal",
            scope,
        )]
        self.dispatches = [dict(row) for row in connection.execute(
            "SELECT run, id, task, worker, started_ms, ended_ms, succeeded FROM ledger_dispatches "
            "WHERE ledger_id = ? ORDER BY ordinal",
            scope,
        )]
        self.workers = {row["id"]: dict(row) for row in connection.execute(
            "SELECT run, id, agent, state, started_ms, dispatch, taken_over FROM ledger_workers "
            "WHERE ledger_id = ?",
            scope,
        )}
        # What each task's record says now. One field per task, written over
        # by every attempt that ends it — so it speaks for an attempt only
        # while that attempt is the task's latest (`latest_attempt`).
        self.task_results: dict[tuple[str, str], str] = {}
        if "ledger_tasks" in tables_of(connection):
            self.task_results = {
                (row["run"], row["id"]): row["result"]
                for row in connection.execute(
                    "SELECT run, id, result FROM ledger_tasks WHERE ledger_id = ?", scope
                )
            }
        latest: dict[tuple[str, str], tuple[int, str]] = {}
        for row in self.dispatches:
            key = (row["run"], row["task"])
            held = latest.get(key)
            if held is None or row["started_ms"] >= held[0]:
                latest[key] = (row["started_ms"], row["id"])
        self.latest_attempt = {key: held[1] for key, held in latest.items()}
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
            if row["sender"] != LEDGER_ITSELF or row["kind"] not in ("went_quiet", "quota_walled"):
                continue
            body = body_json(row["body"])
            worker = body.get("workerId")
            if isinstance(worker, str):
                self.facts_by_worker.setdefault(worker, []).append((row, body))
        # The rows that END an attempt, under that attempt's own key — the
        # ledger's `worker_died` and the worker's `worker_done`, each filed
        # with the dispatch it ended (`announce_a_death` and `send` in
        # `crates/zerocode-core/src/orchestration.rs`). A row without the key
        # is filed under no attempt: nothing but the key says whose it was.
        self.endings_by_attempt: dict[str, list[tuple[str, int]]] = {}
        for row in self.messages:
            ends = (row["kind"] == "worker_died" and row["sender"] == LEDGER_ITSELF) or (
                row["kind"] == "worker_done" and row["sender"].startswith("worker:")
            )
            if ends and row["dispatch"]:
                self.endings_by_attempt.setdefault(row["dispatch"], []).append((row["kind"], row["created_ms"]))

    def root_questions(self) -> list[dict]:
        return [row for row in self.messages if row["kind"] == "question" and not row["thread"]]

    def answer_of(self, question: dict) -> tuple[dict | None, dict | None]:
        """The first thread row from the asked seat that is not the ledger's
        voice — and, beside it, the first from that seat under the older,
        looser reading (any kind, any author), so a row the two disagree on
        is counted rather than silently taken."""
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

    def notices_on(self, question: dict) -> list[tuple[dict, dict]]:
        """The rule's lines: the ledger's own `status` rows threaded on the
        question, each with the facts it carried."""
        held = []
        for row in self.by_thread.get(question["id"], []):
            if row["sender"] != LEDGER_ITSELF or row["kind"] != "status":
                continue
            body = body_json(row["body"])
            if isinstance(body.get("reason"), str):
                held.append((row, body))
        return held

    def receiver_attempt(self, worker_id: str, at_ms: int) -> dict | None:
        """The receiver's dispatch that was open when the question was asked."""
        for row in self.dispatches:
            if row["worker"] != worker_id or row["started_ms"] > at_ms:
                continue
            if row["ended_ms"] is None or row["ended_ms"] > at_ms:
                return row
        return None

    def ending_of(self, attempt: dict) -> str:
        """How the receiver's attempt ended, by the ledger's own record of
        THAT attempt and nothing else: `died` (a `worker_died` row filed under
        its dispatch), `done` (its `worker_done`, under the same key) — each
        written by the time the attempt ended — the outcome the ledger wrote
        into the task for this attempt (`stopped`, `abandoned` — read only
        while the attempt is the task's latest, since the next one writes over
        it), or `unexplained` where the ledger kept no reason this replay can
        read. Never a guess: an attempt with no recorded reason is not counted
        as a stop, and a death or a report of the same worker's NEXT attempt —
        or one that names no attempt — is not this attempt's ending, however
        close in time."""
        ended = [kind for kind, at_ms in self.endings_by_attempt.get(attempt["id"], []) if at_ms <= attempt["ended_ms"]]
        if "worker_died" in ended:
            return "died"
        if "worker_done" in ended:
            return "done"
        task = (attempt["run"], attempt["task"])
        if self.latest_attempt.get(task) == attempt["id"]:
            outcome = body_json(self.task_results.get(task)).get("outcome")
            if outcome in ENDING_OUTCOMES:
                return outcome
        return "unexplained"


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
        # A row the older, looser reading would take as the answer that the
        # stricter one refuses — the ledger's own voice in the thread.
        "misread_answer": loose is not None and (strict is None or loose["id"] != strict["id"]),
    }
    body_key = (question["sender"], question["recipient"], hashlib.sha256(question["body"].encode()).hexdigest())
    repeats = earlier.get(body_key, [])
    row["repeat_of"] = next(
        (hashed(prior["id"]) for prior in repeats if prior["open_until_ms"] > asked_ms), None
    )
    earlier.setdefault(body_key, []).append({"id": question["id"], "open_until_ms": end_ms})
    # The rule's lines the asker was sent while it waited — words, flags and
    # clocks only; the advice sentence and every id stay behind.
    row["notices"] = [
        {
            "reason": body["reason"],
            "final": body.get("final") is True,
            "fact_ms": body.get("factMs") if isinstance(body.get("factMs"), int) else None,
            "learned_ms": held["created_ms"],
        }
        for held, body in ledger.notices_on(question)
        if asked_ms < held["created_ms"] <= end_ms
    ]

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
        row["receiver_ended"] = {
            "ending": ledger.ending_of(attempt),
            "fact_ms": ended_ms,
            "wait_after_ms": end_ms - ended_ms,
        }
    else:
        row["receiver_ended"] = None
    return row


def gather(db: Path, ledger_id: str | None = None) -> dict:
    with open_read_only(db) as connection:
        scoped = scoped_ledger(connection, ledger_id)
        ledger = Ledger(connection, scoped)
    earlier: dict[tuple[str, str, str], list[dict]] = {}
    rows = [question_row(ledger, question, earlier) for question in ledger.root_questions()]
    return {
        "db": hashed(str(db)),
        "ledger": hashed(scoped),
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
    lines.append(f"misread answers under the older reading: {sum(1 for row in rows if row['misread_answer'])}")
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
    lines.append("baseline — what the ledger's own observations held about the receiver:")
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
    for ending in ("done", "died", *ENDING_OUTCOMES, "unexplained"):
        part = [row["receiver_ended"]["wait_after_ms"] for row in ended if row["receiver_ended"]["ending"] == ending]
        if part:
            lines.append(f"    {ending}: n {len(part)}, wait after the ending total {sum(part) / 1000:.0f} s, "
                         f"mean {statistics.mean(part) / 1000:.0f} s, max {max(part) / 1000:.0f} s")
    lines.append("")
    notified = [row for row in rows if row["notices"]]
    lines.append(f"notices — the rule's lines the asker was sent while waiting: {len(notified)} of {len(rows)} questions")
    lags: dict[str, list[int]] = {}
    for row in notified:
        for one in row["notices"]:
            if one["fact_ms"] is not None:
                lags.setdefault(one["reason"], []).append(one["learned_ms"] - one["fact_ms"])
    for reason, held in sorted(lags.items()):
        lines.append(f"  {reason}: n {len(held)}, fact → row p50 {percentile(held, 0.5)} ms, p95 {percentile(held, 0.95)} ms")
    by_receiver: dict[str, int] = {}
    for row in notified:
        by_receiver[row["receiver"]] = by_receiver.get(row["receiver"], 0) + 1
    if by_receiver:
        lines.append("  by receiver: " + ", ".join(f"{k} {v}" for k, v in sorted(by_receiver.items())))
    finals = [row for row in notified if any(one["final"] for one in row["notices"])]
    lines.append(f"  with a final word while waiting: {len(finals)}")
    return lines


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--db", type=Path, default=DEFAULT_DB, help="the authority store (opened read-only)")
    parser.add_argument("--ledger-id", help="which ledger in the store (default: the only one it holds)")
    parser.add_argument("--out", type=Path, help="where to write the seed JSON")
    args = parser.parse_args()
    if not args.db.is_file():
        print(f"no ledger at {args.db}", file=sys.stderr)
        return 2
    try:
        seed = gather(args.db, args.ledger_id)
    except LedgerScopeError as why:
        print(why, file=sys.stderr)
        return 2
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(seed, indent=1) + "\n")
    print("\n".join(summarize(seed)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
