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
import json
import os
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import tempfile

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


def store_facts(store: pathlib.Path) -> tuple[list[dict], dict[str, dict]]:
    """Every summons this ledger holds, and every task's own words.

    The store is copied first: a live window holds it open, and a reader that
    opens the file in place can be handed a page a writer is mid-way through.
    """
    with tempfile.TemporaryDirectory() as scratch:
        copy = pathlib.Path(scratch) / "authority.sqlite"
        shutil.copy(store, copy)
        db = sqlite3.connect(f"file:{copy}?mode=ro", uri=True)
        db.row_factory = sqlite3.Row
        carried = [
            {
                "agent": held["agent"],
                "startedMs": held["started_ms"],
                "endedMs": held["ended_ms"],
                "succeeded": None if held["succeeded"] is None else bool(held["succeeded"]),
                "title": held["title"] or None,
            }
            for held in db.execute(
                """
                SELECT w.agent            AS agent,
                       w.started_ms       AS started_ms,
                       max(d.started_ms)  AS newest_attempt,
                       d.ended_ms         AS ended_ms,
                       d.succeeded        AS succeeded,
                       trim(t.title)      AS title
                  FROM ledger_workers w
                  -- The link lives on the DISPATCH. A worker's own `dispatch`
                  -- names the attempt only while it carries one and is
                  -- cleared when that attempt ends: 4 of 556 worker rows on
                  -- this machine still hold one, against 549 dispatches that
                  -- name their worker.
                  LEFT JOIN ledger_dispatches d
                         ON d.ledger_id = w.ledger_id
                        AND d.run = w.run
                        AND d.worker = w.id
                  LEFT JOIN ledger_tasks t
                         ON t.ledger_id = d.ledger_id
                        AND t.run = d.run
                        AND t.id = d.task
                 -- One row per worker: the newest attempt it was given, the
                 -- same one the fold reads. SQLite hands the bare columns
                 -- from the row `max()` picked, so a worker with two
                 -- attempts is still one summons.
                 GROUP BY w.ledger_id, w.run, w.id
                HAVING d.started_ms IS NULL OR d.started_ms = max(d.started_ms)
                """
            )
        ]
        tasks = {
            held["id"]: {"title": held["title"] or "", "spec": held["spec"] or "", "failures": held["failures"]}
            for held in db.execute(
                "SELECT id, title, spec, failures FROM ledger_tasks"
            )
        }
        attempts: dict[str, int] = {}
        for held in db.execute("SELECT task, count(*) AS n FROM ledger_dispatches GROUP BY task"):
            attempts[held["task"]] = held["n"]
        for task, held in tasks.items():
            held["attempts"] = attempts.get(task, 0)
        db.close()
    return carried, tasks


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

    carried, tasks = store_facts(store)
    gauge = gauges()

    replays = []
    for row in rows(ledger):
        if not isinstance(row.get(AGREED_KEY), bool):
            # No mark: an `--agent auto` summons, a set that never offered the
            # agent, a question nobody answered. Nothing to agree about, so
            # nothing to replay.
            continue
        task = tasks.get(row.get("task"))
        if task is None:
            continue
        replays.append(
            {
                "at": row["at"],
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
            {"carried": carried, "gauges": gauge, "replays": replays},
            ensure_ascii=False,
            indent=1,
        )
    )
    print(
        f"{out}: {len(replays)} rows to replay, {len(carried)} summonses in the ledger, "
        f"{len(gauge)} agents",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
