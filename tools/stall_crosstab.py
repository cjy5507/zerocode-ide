#!/usr/bin/env python3
"""The stall seat's crosstab: why a worker stopped against what was done next.

The stall seat asks Jev one closed question per silence the marker table could
not name, and a later beat labels that answer with what the coordinator did
afterwards (`zerocode_core::stall_cause`). Whether the seat is worth an apply
stage is a question about the JOINT distribution of those two, so this prints
it as a table: the seven causes down the side, the six things a coordinator
actually does across the top, counts in the cells.

Both axes are read out of `stall_cause.rs` rather than typed here, so a word
added to either enum shows up as a row or a column instead of as a silent gap.

The cells come from the seat's own ledger (`~/.zo/jev/stall-cause.jsonl`):
an answered row's `chosen` and the `followed` its label row carries. The COLUMN
totals do not — they are computed from the orchestration ledger for every
silence the seat would have been asked about, by the same two rules the product
holds:

  * what one episode is — `quiet_episode`: the `went_quiet` rows of one
    dispatch, closed by the worker's own next word;
  * what followed it — `stall_cause::followed`: the run coordinator's own mail,
    a continuation receipt, the worker's report, a dispatch closed without one,
    or the terminal's death, whichever the ledger stamped first inside
    `STALL_LABEL_WINDOW_MS`.

So the columns are measurable before the seat has answered anything, and the
gap between the column totals and the cells is exactly how much of the
population the seat has seen. When that gap is everything, the reason is
usually the Jev door rather than the seat: consent is read as a consented root
or a folder under one, and a worker's checkout is a worktree that may sit under
neither. `--consent` prints that split.

It reads three files and sends nothing.

    tools/stall_crosstab.py                 # the table
    tools/stall_crosstab.py --consent       # and what the door would refuse
    tools/stall_crosstab.py --json          # the rows, one object per episode
"""

from __future__ import annotations

import argparse
import collections
import json
import os
import pathlib
import re
import sqlite3
import sys

# The ledger the window writes and the settings the Jev door reads. Both are
# per-machine and neither is a product path, so both are overridable.
LEDGER = pathlib.Path(
    os.environ.get(
        "ZEROCODE_AUTHORITY_DB",
        os.path.expanduser(
            "~/Library/Application Support/dev.zerocode.app/authority/authority.sqlite"
        ),
    )
)
ZO_SETTINGS = pathlib.Path(
    os.environ.get("ZO_SETTINGS", os.path.expanduser("~/.zo/settings.json"))
)
STALL_ROWS = pathlib.Path(
    os.environ.get("ZO_STALL_ROWS", os.path.expanduser("~/.zo/jev/stall-cause.jsonl"))
)
# `stall_cause.rs`, relative to this file — the source both axes are read from.
CAUSE_SOURCE = (
    pathlib.Path(__file__).resolve().parent.parent
    / "crates"
    / "zerocode-core"
    / "src"
    / "stall_cause.rs"
)



def label_window_ms(source: str) -> int:
    """`stall_cause::STALL_LABEL_WINDOW_MS`, read off the source like the axes —
    a copy of the number here was two hours after the product's became four."""
    found = re.search(r"pub const STALL_LABEL_WINDOW_MS: i64 = ([0-9_ *]+);", source)
    if not found:
        raise SystemExit(f"{CAUSE_SOURCE} no longer spells STALL_LABEL_WINDOW_MS as a product of numbers")
    window = 1
    for factor in found.group(1).split("*"):
        window *= int(factor.strip().replace("_", ""))
    return window


# The two rules this file borrows, as the product spells them.
LABEL_WINDOW_MS = label_window_ms(CAUSE_SOURCE.read_text())
# `MessageKind::is_the_ledgers_own` — a kind the ledger writes about a worker
# is never the coordinator writing TO it.
LEDGERS_OWN = frozenset(
    {"went_quiet", "deadlocked", "worker_died", "quota_walled", "handover", "resumed"}
)


def axes(source: str) -> tuple[list[str], list[str]]:
    """The cause words and the followed words, in the order the source lists them."""

    def words(opens: str, closes: str) -> list[str]:
        at = source.index(opens)
        body = source[at : source.index(closes, at)]
        return re.findall(r'=> "([a-z_]+)"', body)

    causes = words("pub const fn word(self) -> &'static str {", "\n    }")
    followed = words("impl Followed {", "\n}")
    if not causes or not followed:
        raise SystemExit(f"{CAUSE_SOURCE} no longer spells its answer words the same way")
    return causes, followed


def consented_roots() -> list[pathlib.PurePosixPath]:
    """The workspaces the person's own settings consent to sending words from."""
    try:
        held = json.loads(ZO_SETTINGS.read_text())
    except (OSError, ValueError):
        return []
    roots = held.get("smart", {}).get("jev", {}).get("workspaces", [])
    return [pathlib.PurePosixPath(root) for root in roots if isinstance(root, str)]


def consents(roots: list[pathlib.PurePosixPath], path: str | None) -> bool:
    """`JevSettings::consents` — a consented root, or a folder under one."""
    if not path:
        return False
    held = pathlib.PurePosixPath(path)
    return any(held == root or root in held.parents for root in roots)


def episodes(by_run, dispatches):
    """One row per silence, by `quiet_episode`'s own rule.

    An episode opens on the first `went_quiet` of a dispatch that carries a
    turn or a stall sample, counts the rest, and closes when that worker says
    anything of its own — which is what makes the next silence a new one.
    """
    found = []
    for run, rows in by_run.items():
        open_now: dict[str, list] = {}
        for message in rows:
            sender = message["sender"] or ""
            if sender.startswith("worker:"):
                said = sender[len("worker:") :]
                for dispatch in [
                    held for held, one in open_now.items() if one[2] == said
                ]:
                    first, notices, who = open_now.pop(dispatch)
                    found.append((run, who, dispatch, first, notices))
            if message["kind"] != "went_quiet" or not message["dispatch"]:
                continue
            try:
                body = json.loads(message["body"])
            except ValueError:
                continue
            observed = body.get("turnEndedMs") or body.get("stalledSinceMs")
            if observed is None:
                # An episode-closing summary is an observation, but neither a
                # turn nor a stall sample; only those two define an episode.
                continue
            who = dispatches.get((run, message["dispatch"]), {}).get("worker")
            if who is None:
                continue
            if message["dispatch"] in open_now:
                open_now[message["dispatch"]][1] += 1
            else:
                open_now[message["dispatch"]] = [observed, 1, who]
        for dispatch, (first, notices, who) in open_now.items():
            found.append((run, who, dispatch, first, notices))
    return found


def followed(by_run, dispatches, run, worker, dispatch, asked_ms, now_ms):
    """`stall_cause::followed`, plus the second event for the ambiguity it leaves."""
    until = asked_ms + LABEL_WINDOW_MS
    reporter, coordinator = f"worker:{worker}", f"run:{run}"
    seen, died = [], False
    for message in by_run[run]:
        if message["kind"] == "worker_died" and message["dispatch"] == dispatch:
            died = True
        at = message["created_ms"]
        if not asked_ms < at <= until:
            continue
        kind = message["kind"]
        if kind == "worker_done" and message["sender"] == reporter:
            what = "worker_done"
        elif kind == "resumed" and message["dispatch"] == dispatch:
            what = "resumed"
        elif kind == "worker_died" and message["dispatch"] == dispatch:
            what = "worker_died"
        elif (
            message["recipient"] == reporter
            and message["sender"] == coordinator
            and kind not in LEDGERS_OWN
        ):
            what = "mail"
        else:
            continue
        seen.append((at, what))
    held = dispatches.get((run, dispatch))
    if held and held["ended_ms"] is not None:
        at = held["ended_ms"]
        if (
            asked_ms < at <= until
            and held["succeeded"] != 1
            and not died
        ):
            seen.append((at, "worker_stop"))
    seen.sort()
    if not seen:
        return ("none", until, None) if now_ms > until else (None, None, None)
    return seen[0][1], seen[0][0], seen[1][1] if len(seen) > 1 else None


def read_ledger():
    if not LEDGER.exists():
        raise SystemExit(f"no orchestration ledger at {LEDGER}")
    held = sqlite3.connect(f"file:{LEDGER}?mode=ro", uri=True)
    held.row_factory = sqlite3.Row
    messages = [
        dict(row)
        for row in held.execute(
            "SELECT run, sender, recipient, kind, body, dispatch, created_ms "
            "FROM ledger_messages ORDER BY run, ordinal"
        )
    ]
    dispatches = {
        (row["run"], row["id"]): dict(row)
        for row in held.execute(
            "SELECT run, id, worker, ended_ms, succeeded FROM ledger_dispatches"
        )
    }
    workers = {
        (row["run"], row["id"]): dict(row)
        for row in held.execute("SELECT run, id, agent, checkout FROM ledger_workers")
    }
    by_run = collections.defaultdict(list)
    for message in messages:
        by_run[message["run"]].append(message)
    return messages, dispatches, workers, by_run


def seat_rows():
    """The seat's own rows, and the answers a label row has landed on."""
    if not STALL_ROWS.exists():
        return [], {}
    held = []
    for line in STALL_ROWS.read_text().splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            held.append(json.loads(line))
        except ValueError:
            # A torn last line is what a crash leaves; the rest still counts.
            continue
    labels = {
        row["label"]: row.get("followed")
        for row in held
        if isinstance(row.get("label"), str)
    }
    return held, labels


def main() -> int:
    parsed = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parsed.add_argument("--consent", action="store_true", help="what the door would refuse")
    parsed.add_argument("--json", action="store_true", help="one object per episode")
    args = parsed.parse_args()

    causes, columns = axes(CAUSE_SOURCE.read_text())
    messages, dispatches, workers, by_run = read_ledger()
    roots = consented_roots()
    now_ms = max((one["created_ms"] for one in messages), default=0)

    rows = []
    for run, worker, dispatch, asked_ms, notices in episodes(by_run, dispatches):
        what, at, then = followed(
            by_run, dispatches, run, worker, dispatch, asked_ms, now_ms
        )
        held = workers.get((run, worker), {})
        rows.append(
            {
                "run": run,
                "worker": worker,
                "dispatch": dispatch,
                "askedMs": asked_ms,
                "notices": notices,
                "followed": what,
                "then": then,
                "latencyMs": None if at is None or what == "none" else at - asked_ms,
                "agent": held.get("agent"),
                "checkout": held.get("checkout"),
                "consented": consents(roots, held.get("checkout")),
            }
        )
    if args.json:
        json.dump(rows, sys.stdout, indent=1)
        print()
        return 0

    seat, labels = seat_rows()
    answered = [one for one in seat if one.get("outcome") == "answered"]
    cells = collections.Counter(
        (one.get("chosen"), labels.get(one.get("stall")))
        for one in answered
        if labels.get(one.get("stall"))
    )
    closed = [one for one in rows if one["followed"]]
    totals = collections.Counter(one["followed"] for one in closed)

    said = sum(1 for one in messages if one["kind"] == "went_quiet")
    print(f"ledger {LEDGER}")
    print(
        f"messages {len(messages)} · went_quiet {said} · "
        f"episodes {len(rows)} (label window closed for {len(closed)})"
    )
    print()
    head = f"{'cause':<28}" + "".join(f"{one[:11]:>13}" for one in columns) + f"{'sum':>8}"
    print(head)
    print("-" * len(head))
    for cause in causes:
        line = f"{cause:<28}" + "".join(
            f"{cells.get((cause, one), 0):>13}" for one in columns
        )
        print(line + f"{sum(cells.get((cause, one), 0) for one in columns):>8}")
    print("-" * len(head))
    print(
        f"{'ledger totals':<28}"
        + "".join(f"{totals.get(one, 0):>13}" for one in columns)
        + f"{sum(totals.values()):>8}"
    )
    print()
    print(f"seat rows {len(seat)}, answered {len(answered)}, labelled {len(labels)}")
    outcomes = collections.Counter(one.get("outcome") for one in seat)
    if outcomes:
        print("  outcomes " + ", ".join(f"{k}×{v}" for k, v in outcomes.most_common()))
    print()
    print("what followed, measured — the column the seat has not reached yet")
    print(f"{'followed':<14}{'n':>6}{'share':>9}{'p50':>11}{'p90':>11}   then")
    for one in columns:
        count = totals.get(one, 0)
        if count == 0:
            print(f"{one:<14}{0:>6}{'—':>9}{'—':>11}{'—':>11}")
            continue
        waits = sorted(
            held["latencyMs"]
            for held in closed
            if held["followed"] == one and held["latencyMs"] is not None
        )
        def minutes(at: int) -> str:
            return f"{waits[at] / 60000:.1f}m"
        p50 = minutes(len(waits) // 2) if waits else "—"
        p90 = minutes(min(len(waits) - 1, int(0.9 * (len(waits) - 1)))) if waits else "—"
        after = collections.Counter(
            held["then"] for held in closed if held["followed"] == one
        )
        print(
            f"{one:<14}{count:>6}{count / len(closed) * 100:>8.1f}%{p50:>11}{p90:>11}   "
            + ", ".join(f"{k or 'nothing'}×{v}" for k, v in after.most_common(3))
        )
    if args.consent:
        print()
        print(f"consented roots {[str(one) for one in roots] or 'none'}")
        seats = sum(1 for one in workers.values() if consents(roots, one.get("checkout")))
        ok = sum(1 for one in rows if one["consented"])
        print(
            f"  worker rows {seats}/{len(workers)} consented · "
            f"episodes {ok}/{len(rows)} consented — the door refuses the rest"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
