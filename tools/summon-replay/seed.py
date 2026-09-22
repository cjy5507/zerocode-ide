#!/usr/bin/env python3
"""Build one replay seed for the summon seat's agreement measurement (t-5873).

The seed is an EXTRACTION and never a calculation: every number it carries is
copied out of a row, and the arithmetic that turns summonses into an agent's
record lives in one place only
(`zerocode_core::summon_choice::records`), which the harness calls on the
rows this file hands it.

Three sources, all read-only:

  * `~/.zo/jev/summon-choice.jsonl` — the rows to replay. Each carries the
    option set the question offered and the agent the summons landed on,
    which is the label.
  * the authority store (`authority.sqlite`) — every summons this ledger
    holds, with the dispatch that owned it, for `records`; and every task's
    own words, because a row records how long its brief was and not what it
    said.
  * `zerocode-orc agent-list` — each agent's quota gauge as this window last
    read it.

Usage:

    python3 tools/summon-replay/seed.py \
        --ledger ~/.zo/jev/summon-choice.jsonl \
        --store "~/Library/Application Support/dev.zerocode.app/authority/authority.sqlite" \
        --out /tmp/summon-replay/seed.json
"""

import argparse
from contextlib import closing
import json
import os
import pathlib
import sqlite3
import subprocess
import sys

#: The row key that names the agent a summons landed on — the label.
LABEL_KEY = "agent"
#: The row key that names the set the question offered.
OPTIONS_KEY = "options"
#: The row key that carries the `agreed` mark production wrote.
AGREED_KEY = "agreed"


def rows(path: pathlib.Path) -> list[dict]:
    held = []
    for line in path.read_text().splitlines():
        line = line.strip()
        if line:
            held.append(json.loads(line))
    return held


def option_ids(row: dict) -> list[str]:
    """The ids one row offered, whichever shape it wrote them in.

    Rows written before t-5873 name their options as bare ids; rows written
    after name each option's own numbers as well.
    """
    offered = row.get(OPTIONS_KEY) or []
    return [held if isinstance(held, str) else held["id"] for held in offered]


def snapshot(store: pathlib.Path) -> sqlite3.Connection:
    """Read a consistent SQLite snapshot, including committed WAL pages."""
    db = sqlite3.connect(":memory:")
    with closing(sqlite3.connect(store.resolve().as_uri() + "?mode=ro", uri=True)) as source:
        source.backup(db)
    db.row_factory = sqlite3.Row
    return db


def facts_at(db: sqlite3.Connection, row: dict) -> tuple[list[dict], dict] | None:
    """Extract facts known before this row; the Rust fold still does all scoring.

    A worker's latest dispatch today may have started after the replayed row.
    Select its latest dispatch THEN, and mask an outcome that had not landed.
    Task wording has no version history in this store and remains a reconstruction.
    """
    at = row["at"]
    tasks = list(db.execute(
        "SELECT * FROM ledger_tasks WHERE run = ? AND id = ? AND created_ms < ?",
        (row.get("run"), row.get("task"), at),
    ))
    if not tasks:
        return None
    if len(tasks) != 1:
        raise ValueError("the row does not identify a unique ledger task")
    task = tasks[0]
    ledger = task["ledger_id"]
    carried = [
        {
            "agent": held["agent"], "startedMs": held["started_ms"],
            "endedMs": held["ended_ms"] if held["ended_ms"] is not None and held["ended_ms"] <= at else None,
            "succeeded": bool(held["succeeded"]) if held["ended_ms"] is not None and held["ended_ms"] <= at and held["succeeded"] is not None else None,
            "title": held["title"] or None,
        }
        for held in db.execute(
            """
            WITH attempts AS (
                SELECT w.agent, w.started_ms, d.ended_ms, d.succeeded, trim(t.title) AS title,
                       row_number() OVER (
                           PARTITION BY w.ledger_id, w.run, w.id
                           ORDER BY d.started_ms DESC, d.id DESC
                       ) AS newest
                  FROM ledger_workers w
                  LEFT JOIN ledger_dispatches d
                    ON d.ledger_id = w.ledger_id AND d.run = w.run AND d.worker = w.id
                   AND d.started_ms < ?
                  LEFT JOIN ledger_tasks t
                    ON t.ledger_id = d.ledger_id AND t.run = d.run AND t.id = d.task
                 WHERE w.ledger_id = ? AND w.started_ms < ?
            ) SELECT * FROM attempts WHERE newest = 1
            """, (at, ledger, at),
        )
    ]
    prior = list(db.execute(
        "SELECT ended_ms, succeeded FROM ledger_dispatches "
        "WHERE ledger_id = ? AND run = ? AND task = ? AND started_ms < ? "
        "ORDER BY ended_ms DESC, started_ms DESC, id DESC",
        (ledger, task["run"], task["id"], at),
    ))
    # The task's current failure streak includes later attempts. Only closed
    # attempts known then count, stopping at the latest successful one.
    failures = 0
    for held in prior:
        if held["ended_ms"] is None or held["ended_ms"] > at or held["succeeded"] is None:
            continue
        if held["succeeded"]:
            break
        failures += 1
    return carried, {
        "title": task["title"] or "", "spec": task["spec"] or "",
        "attempts": row.get("attempts", len(prior)),
        "failures": row.get("failures", failures),
    }


def gauges() -> dict[str, dict]:
    """Each agent's quota gauge, as `agent-list` reports this window's cache."""
    said = subprocess.run(
        ["zerocode-orc", "agent-list"], capture_output=True, text=True, check=True
    ).stdout
    held = {}
    for agent in json.loads(said)["agents"]:
        headroom = agent.get("headroom")
        held[agent["id"]] = {
            "spentPercent": None if headroom is None else headroom.get("usedPercent"),
            # The window the gauge describes. `agent-list` reports the number
            # and not the word, and the option sentence needs the word; every
            # gauge this window keeps for an agent names one window at a time,
            # so the harness is told which and says so.
            "window": None if headroom is None else "weekly",
        }
    return held


def main() -> int:
    ask = argparse.ArgumentParser(description=__doc__)
    ask.add_argument("--ledger", required=True)
    ask.add_argument("--store", required=True)
    ask.add_argument("--out", required=True)
    said = ask.parse_args()

    ledger = pathlib.Path(os.path.expanduser(said.ledger))
    store = pathlib.Path(os.path.expanduser(said.store))
    out = pathlib.Path(os.path.expanduser(said.out))

    db = snapshot(store)
    gauge = gauges()

    replays = []
    for row in rows(ledger):
        if not isinstance(row.get(AGREED_KEY), bool):
            # No mark: an `--agent auto` summons, a set that never offered the
            # agent, a question nobody answered. Nothing to agree about, so
            # nothing to replay.
            continue
        facts = facts_at(db, row)
        if facts is None:
            continue
        carried, task = facts
        replays.append(
            {
                "at": row["at"],
                "carried": carried,
                "task": row["task"],
                "rubricVersion": row.get("rubricVersion"),
                "label": row[LABEL_KEY],
                "agreedInProduction": row[AGREED_KEY],
                "options": option_ids(row),
                "worktree": bool(row.get("worktree")),
                "replaces": bool(row.get("replaces")),
                "carriesATask": bool(row.get("carriesATask")),
                "attempts": task["attempts"],
                "failures": task["failures"],
                "title": task["title"],
                "spec": task["spec"],
            }
        )

    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(
        json.dumps(
            {"gauges": gauge, "replays": replays},
            ensure_ascii=False,
            indent=1,
        )
    )
    print(
        f"{out}: {len(replays)} rows with historical dispatch snapshots, "
        f"{len(gauge)} agents",
        file=sys.stderr,
    )
    db.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
