#!/usr/bin/env python3
"""Gather every Jev seat's ledger rows on this machine, and what the window's
black box says the stage showed, into one seed for the label audit (t-6342).

This script **only reads files and copies facts**. It grades nothing: the old
marks are the `agreed` values the ledgers already hold, and the new marks, the
cheapest baselines and the confidence bands are the shipped functions the
audit hands each row to (`label_audit_tests.rs` beside `jev_summary.rs` in zo's
tools crate, an `#[ignore]` test). A second copy of any of those rules here
would be a second rule.

What a seed holds:

  * `ledgers`: `window` — the window's seats' ledgers under `<zo home>/jev/`
    — and `projects` — zo's, from every project's `state/smart-router/`,
    merged by file name, each row tagged with its project's index under
    `_project` so a join never crosses two projects. Two maps, because a
    name alone is not a seat: zo's step governor wrote `step-effort.jsonl`
    before it had a ledger of its own, and that is the window's effort
    seat's name.
  * `watch`: the stage declarations the window's black box recorded
    (`cmd::board::set_watched_terms`), `[at, [terms]]`, oldest first — what a
    placed worker's pane needed to be on to have been seen.
  * `workerTerms`: worker id → `[[at, terminal]]`, the terminal the black
    box's orchestration lines named a worker's pane by.

Ledger rows carry ids, counts, fingerprints and answers, never a person's
words, and the black box lines are reduced to numbers.

    tools/label-audit/seed.py --out seed.json
    tools/label-audit/seed.py --zo-home ~/.zo --black-box window-errors.log --out seed.json
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# The window's seats' ledger folder under the zo home
# (`zerocode_core::jev::count::REQUESTS_DIR`).
WINDOW_LEDGERS = "jev"

# Where each project's zo seats keep theirs, under `<zo home>/projects/<slug>`.
PROJECT_LEDGERS = ("state", "smart-router")

# The black box's line for the main window's stage declaration.
WATCH_LINE = re.compile(r"^(\d+) watch main: \[([\d, ]*)\]")

# An orchestration line that names a worker's pane by its terminal.
WORKER_TERM = re.compile(r"worker:(w-\d+)\b.*?terminal (\d+)")

# A black box line's own clock: the epoch milliseconds it opens with.
STAMP = re.compile(r"^(\d+) ")


def read_rows(path: Path) -> list[dict]:
    """A ledger's rows, oldest first, skipping a line that does not parse — a
    half-written last line is a crash's leftover, not a reason to refuse the
    rest."""
    rows = []
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return rows
    for line in text.splitlines():
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(row, dict):
            rows.append(row)
    return rows


def ledgers(zo_home: Path) -> dict[str, dict[str, list[dict]]]:
    """Every seat ledger under the zo home, by file name: the window's under
    `window`, every project's zo seats under `projects`."""
    window = {path.name: read_rows(path) for path in sorted((zo_home / WINDOW_LEDGERS).glob("*.jsonl"))}
    found: dict[str, list[dict]] = {}
    projects = zo_home / "projects"
    folders = sorted(projects.iterdir()) if projects.is_dir() else []
    for index, project in enumerate(folders):
        for path in sorted(project.joinpath(*PROJECT_LEDGERS).glob("*.jsonl")):
            rows = read_rows(path)
            for row in rows:
                row["_project"] = index
            found.setdefault(path.name, []).extend(rows)
    return {"window": window, "projects": found}


def black_box(paths: list[Path]) -> tuple[list, dict[str, list]]:
    """The stage declarations and the worker terminals the black box names."""
    watch: list = []
    worker_terms: dict[str, list] = {}
    for path in paths:
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for line in text.splitlines():
            declared = WATCH_LINE.match(line)
            if declared:
                terms = sorted(int(term) for term in declared.group(2).split(",") if term.strip())
                watch.append([int(declared.group(1)), terms])
                continue
            stamp = STAMP.match(line)
            if not stamp:
                continue
            for worker, terminal in WORKER_TERM.findall(line):
                worker_terms.setdefault(worker, []).append([int(stamp.group(1)), int(terminal)])
    watch.sort(key=lambda declared: declared[0])
    for seen in worker_terms.values():
        seen.sort()
    return watch, worker_terms


def default_black_box() -> list[Path]:
    """The window's black box, older half first."""
    root = Path.home() / "Library" / "Application Support" / "dev.zerocode.app"
    return [root / "window-errors.log.1", root / "window-errors.log"]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--zo-home", type=Path, default=Path.home() / ".zo")
    parser.add_argument("--black-box", type=Path, action="append", default=None)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    watch, worker_terms = black_box(args.black_box or default_black_box())
    seed = {
        "ledgers": ledgers(args.zo_home.expanduser()),
        "watch": watch,
        "workerTerms": worker_terms,
    }
    args.out.write_text(json.dumps(seed), encoding="utf-8")
    held = [rows for home in seed["ledgers"].values() for rows in home.values()]
    rows = sum(len(each) for each in held)
    print(
        f"{rows} rows from {len(held)} ledgers, "
        f"{len(watch)} stage declarations, {len(worker_terms)} workers named → {args.out}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
