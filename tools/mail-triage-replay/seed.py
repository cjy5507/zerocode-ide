#!/usr/bin/env python3
"""Gather one ledger's coordinator mail and the receipts its coordinators'
acts were filed under — the seed for the mail triage seat's replay (t-9471).

This script **only reads the ledger and copies rows**. It labels nothing,
asks nothing and scores nothing: which receipts are a coordinator's acts,
which of them handles a letter, when a letter was handed over and what its
label is are `zerocode_core::mail_triage` (`Filed::of_parts`, `Mailroom`),
and the replay that reads this seed hands each run to those as they ship
(`mail_triage::tests::the_mail_this_machine_would_have_labeled`). A second
copy of any of that in Python would be a second rule.

What a seed holds, for the one ledger it reads:

  * every run: its own address (`run:<id>`, `Run::address`), its seat's
    session — hashed — and every message it holds in the core's own shape,
    **with no body, subject or payload**, and an address that is not a run's,
    a worker's or the ledger's cut to its head; its attempts; the batch its
    coordinator holds open, the ids still pending in its inbox, and every
    batch that inbox acknowledged, oldest first;
  * every receipt: who filed it (hashed, alike wherever the same session
    appears), its verb, when, the inbox a `check` looked in with what it
    handed over, and its printed answer cut down to the ledger's own ids —
    a text shaped like a minted id — and a look's own page word (`mode`), the
    one thing that says a receipt an older window filed with no verb was the
    inbox's. Never the retry name, the fingerprint, a title or an answer's
    prose.

One ledger at a time: the authority store keys every row by `ledger_id`, and
ids are minted per ledger, so two ledgers in one store can both hold an
`m-1`. The one ledger the store holds is read; a store holding several is
refused until one is named.

    tools/mail-triage-replay/seed.py --out seed.json                 # today's mail, the window's ledger
    tools/mail-triage-replay/seed.py --days 7 --out week.json
    tools/mail-triage-replay/seed.py --db PATH --ledger-id ID --from-ms MS --out seed.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sqlite3
import sys
import time
from datetime import datetime
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

# `SEED_SCHEMA` in `crates/zerocode-core/src/mail_triage/tests.rs`: the shape
# the replay reads.
SEED_SCHEMA = 1

# `RUN_ADDRESS_PREFIX` in `crates/zerocode-core/src/orchestration.rs`: a run's
# own address — its coordinator's inbox — is this and its id (`Run::address`).
RUN_ADDRESS_PREFIX = "run:"

# `WORKER_ADDRESS_PREFIX` and `LEDGER_ITSELF` there: a worker's address, and
# the sender of every notice the ledger writes.
WORKER_ADDRESS_PREFIX = "worker:"
LEDGER_ITSELF = "ledger"

# `LOOK_MODE_KEY` there: the key a look that hands nothing over names its page
# under. Its word is the one non-id a receipt's answer keeps.
LOOK_MODE_KEY = "mode"

# The shape of an id the ledger mints (`Ledger::mint`: `m-`, `t-`, `w-`,
# `dp-`, `d-`, `run-`, `gate-` and a counter). A receipt's answer keeps a text
# of this shape and nothing else.
ID_SHAPE = re.compile(r"[a-z]+-[0-9]+")

# The tables a ledger's rows live in, every one keyed by `ledger_id`.
LEDGER_TABLES = ("ledger_runs", "ledger_messages", "ledger_served")


class LedgerScopeError(Exception):
    """The store does not name the one ledger this replay should read."""


def hashed(value: str | None) -> str | None:
    """A session, as the seed may carry it: alike wherever it appears, and
    never itself."""
    if value is None:
        return None
    return hashlib.sha256(value.encode()).hexdigest()[:16]


def open_read_only(path: Path) -> sqlite3.Connection:
    """The store, read-only. `mode=ro` refuses every write at the door, so a
    bug here cannot move the person's ledger."""
    connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    return connection


def ledger_ids(connection: sqlite3.Connection) -> list[str]:
    """Every ledger the store holds rows for."""
    present = {row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type = 'table'")}
    held: set[str] = set()
    for table in LEDGER_TABLES:
        if table in present:
            held.update(row[0] for row in connection.execute(f"SELECT DISTINCT ledger_id FROM {table}"))
    return sorted(held)


def scoped_ledger(connection: sqlite3.Connection, asked: str | None) -> str:
    """The one ledger this replay reads: the one named, or the only one the
    store holds."""
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


def address_kept(address: str) -> str:
    """An address as the seed may carry it: whole when it is a run's, a
    worker's or the ledger's own — the three the label reads — and otherwise
    its head alone (`pane:`, `@worktree:`), since a crowd's address can name a
    folder on this machine and the replay reads nothing past the head."""
    for prefix in (RUN_ADDRESS_PREFIX, WORKER_ADDRESS_PREFIX):
        rest = address[len(prefix):] if address.startswith(prefix) else None
        if rest is not None and ID_SHAPE.fullmatch(rest):
            return address
    if address == LEDGER_ITSELF:
        return address
    return address.split(":", 1)[0] + ":"


def answer_kept(inline: str | None) -> dict:
    """A receipt's printed answer, cut to what the replay may read: every
    top-level text shaped like a minted id, and a look's page word. A tombstone,
    a text that is not an object and everything else read as nothing."""
    if not inline:
        return {}
    try:
        printed = json.loads(inline)
    except json.JSONDecodeError:
        return {}
    if not isinstance(printed, dict):
        return {}
    kept = {}
    for key, value in printed.items():
        if not isinstance(value, str):
            continue
        if ID_SHAPE.fullmatch(value) or key == LOOK_MODE_KEY:
            kept[key] = value
    return kept


def message_row(row: sqlite3.Row) -> dict:
    """One message in the core's own shape (`orchestration::Message`), its
    words left behind."""
    message = {
        "id": row["id"],
        "from": address_kept(row["sender"]),
        "to": address_kept(row["recipient"]),
        "kind": row["kind"],
        "body": "",
        "thread": row["thread"],
        "task": row["task"],
        "dispatch": row["dispatch"],
        "created_ms": row["created_ms"],
    }
    if row["priority"] and row["priority"] != "normal":
        message["priority"] = row["priority"]
    return message


def dispatch_row(row: sqlite3.Row) -> dict:
    """One attempt in the core's own shape (`orchestration::Dispatch`)."""
    succeeded = row["succeeded"]
    attempt = {
        "id": row["id"],
        "task": row["task"],
        "worker": row["worker"],
        "started_ms": row["started_ms"],
        "ended_ms": row["ended_ms"],
        "succeeded": None if succeeded is None else bool(succeeded),
    }
    if row["retry_of"]:
        attempt["retry_of"] = row["retry_of"]
    return attempt


def gather(db: Path, ledger_id: str | None = None, from_ms: int | None = None) -> dict:
    """The seed of one ledger: its runs' mail and inboxes, and every receipt."""
    connection = open_read_only(db)
    try:
        ledger = scoped_ledger(connection, ledger_id)
        scope = (ledger,)
        messages: dict[str, list[dict]] = {}
        newest = 0
        for row in connection.execute(
            "SELECT run, id, sender, recipient, kind, priority, thread, task, dispatch, created_ms "
            "FROM ledger_messages WHERE ledger_id = ? ORDER BY ordinal",
            scope,
        ):
            messages.setdefault(row["run"], []).append(message_row(row))
            newest = max(newest, row["created_ms"] or 0)
        attempts: dict[str, list[dict]] = {}
        for row in connection.execute(
            "SELECT run, id, task, worker, started_ms, ended_ms, succeeded, retry_of "
            "FROM ledger_dispatches WHERE ledger_id = ? ORDER BY ordinal",
            scope,
        ):
            attempts.setdefault(row["run"], []).append(dispatch_row(row))
        inboxes: dict[tuple[str, str], dict] = {}
        for row in connection.execute(
            "SELECT ordinal, run, address, open_delivery, open_opened_ms FROM ledger_inboxes WHERE ledger_id = ?",
            scope,
        ):
            inboxes[(row["run"], row["address"])] = {
                "ordinal": row["ordinal"],
                "open": row["open_delivery"],
                "opened_ms": row["open_opened_ms"],
            }
        pending: dict[int, list[str]] = {}
        for row in connection.execute(
            "SELECT inbox_ordinal, message FROM ledger_inbox_pending WHERE ledger_id = ? "
            "ORDER BY inbox_ordinal, ordinal",
            scope,
        ):
            pending.setdefault(row["inbox_ordinal"], []).append(row["message"])
        opened: dict[int, list[str]] = {}
        for row in connection.execute(
            "SELECT inbox_ordinal, message FROM ledger_inbox_open_messages WHERE ledger_id = ? "
            "ORDER BY inbox_ordinal, ordinal",
            scope,
        ):
            opened.setdefault(row["inbox_ordinal"], []).append(row["message"])
        acked_messages: dict[int, list[str]] = {}
        for row in connection.execute(
            "SELECT acked_ordinal, message FROM ledger_acked_messages WHERE ledger_id = ? "
            "ORDER BY acked_ordinal, ordinal",
            scope,
        ):
            acked_messages.setdefault(row["acked_ordinal"], []).append(row["message"])
        acked: dict[tuple[str, str], list[dict]] = {}
        for row in connection.execute(
            "SELECT ordinal, run, address, delivery FROM ledger_acked WHERE ledger_id = ? ORDER BY seq",
            scope,
        ):
            acked.setdefault((row["run"], row["address"]), []).append(
                {"delivery": row["delivery"], "messages": acked_messages.get(row["ordinal"], [])}
            )
        runs = []
        for row in connection.execute(
            "SELECT id, coordinator FROM ledger_runs WHERE ledger_id = ? ORDER BY ordinal", scope
        ):
            address = RUN_ADDRESS_PREFIX + row["id"]
            seat = json.loads(row["coordinator"]) if row["coordinator"] else {}
            inbox = inboxes.get((row["id"], address))
            open_batch = None
            if inbox and inbox["open"]:
                open_batch = {"id": inbox["open"], "messages": opened.get(inbox["ordinal"], [])}
                if inbox["opened_ms"] is not None:
                    open_batch["opened_ms"] = inbox["opened_ms"]
            runs.append(
                {
                    "id": row["id"],
                    "address": address,
                    "seatActor": hashed(seat.get("actor")),
                    "messages": messages.get(row["id"], []),
                    "dispatches": attempts.get(row["id"], []),
                    "open": open_batch,
                    "pending": pending.get(inbox["ordinal"], []) if inbox else [],
                    "acked": acked.get((row["id"], address), []),
                }
            )
        handed: dict[int, list[str]] = {}
        for row in connection.execute(
            "SELECT served_ordinal, message FROM ledger_served_messages WHERE ledger_id = ? "
            "ORDER BY served_ordinal, ordinal",
            scope,
        ):
            handed.setdefault(row["served_ordinal"], []).append(row["message"])
        receipts = []
        for row in connection.execute(
            "SELECT ordinal, caller, verb, filed_ms, renderer, inline, check_address, check_delivery "
            "FROM ledger_served WHERE ledger_id = ? ORDER BY ordinal",
            scope,
        ):
            check = None
            if row["renderer"] is not None:
                check = {
                    "address": row["check_address"],
                    "delivery": row["check_delivery"],
                    "messages": handed.get(row["ordinal"], []),
                }
            receipts.append(
                {
                    "caller": hashed(row["caller"]),
                    "verb": row["verb"],
                    "filedMs": row["filed_ms"],
                    "answer": answer_kept(row["inline"]),
                    "check": check,
                }
            )
            newest = max(newest, row["filed_ms"] or 0)
    finally:
        connection.close()
    return {
        "schema": SEED_SCHEMA,
        "ledgerId": ledger,
        "readAtMs": newest,
        "fromMs": from_ms if from_ms is not None else start_of_today_ms(),
        "runs": runs,
        "receipts": receipts,
    }


def start_of_today_ms() -> int:
    """This machine's local midnight, in epoch milliseconds."""
    now = datetime.now().astimezone()
    return int(now.replace(hour=0, minute=0, second=0, microsecond=0).timestamp() * 1000)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--db", type=Path, default=DEFAULT_DB)
    parser.add_argument("--ledger-id")
    parser.add_argument("--out", type=Path, required=True)
    window = parser.add_mutually_exclusive_group()
    window.add_argument("--from-ms", type=int, help="label the letters written at or after this instant")
    window.add_argument("--days", type=float, help="label the letters of the last N days")
    args = parser.parse_args(argv)
    from_ms = args.from_ms
    if args.days is not None:
        from_ms = int(time.time() * 1000 - args.days * 86_400_000)
    try:
        gathered = gather(args.db, args.ledger_id, from_ms)
    except LedgerScopeError as refused:
        print(f"seed: {refused}", file=sys.stderr)
        return 2
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(gathered))
    # Counts of what was copied, and nothing judged: which of the messages
    # are letters is the replay's to say.
    print(
        json.dumps(
            {
                "ledgerId": gathered["ledgerId"],
                "runs": len(gathered["runs"]),
                "messages": sum(len(run["messages"]) for run in gathered["runs"]),
                "receipts": len(gathered["receipts"]),
                "fromMs": gathered["fromMs"],
                "readAtMs": gathered["readAtMs"],
            }
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
