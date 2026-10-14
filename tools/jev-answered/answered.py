#!/usr/bin/env python3
"""How many of each Jev seat's requests came back answered — by the provider
folder its rows were kept in and the time window they fell in — and the
failure words of the rest (t-55423). The same command before and after a
change is that change's before/after table.

    answered.py [--home <zo home>] [--project <dir> ...] [--window <label>=<from>..<to> ...] [--json <file>]

A time is local `YYYY-MM-DDTHH:MM` (seconds optional) or epoch milliseconds,
and an end left empty is open; a window holds its start and not its end.
Without `--window` every row falls in one window, `all`. Without `--project`
every project's folder is read and summed; with it, only the named ones,
found by zo's slug rule.

Read-only: the ledgers are opened for reading and the one file written is the
one `--json` names. A row is a request when it carries an outcome; a row
without one (a label, a note added after the answer) is not a request. Nothing
of a row leaves this script but its seat, folder, time and outcome word.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import sys
from collections import Counter
from dataclasses import asdict, dataclass
from datetime import datetime
from pathlib import Path
from typing import Iterator, Optional

_spec = importlib.util.spec_from_file_location(
    "jev_seat_replay", Path(__file__).resolve().parents[1] / "jev-seat-replay" / "replay.py"
)
replay = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(replay)

# The ledger row's keys and the one outcome a request answered with
# (`zerocode_core::jev::summary::{AT, OUTCOME, ANSWERED}`).
AT_KEY = "at"
OUTCOME_KEY = "outcome"
ANSWERED = "answered"

# A seat's ledger: one JSON row a line.
LEDGER_SUFFIX = ".jsonl"

# The two places seats keep ledgers, as the table names them: the window's
# folder under the zo home, and every project's folder summed.
WINDOW_PLACE = "window"
PROJECTS_PLACE = "projects"

# Jev's own rows sit in the place's folder itself rather than under
# `providers/` (`Provider::evidence_dir`); the folder column names it so.
JEV_FOLDER = "(jev)"

# How many hex digits of a provider folder's hash the table shows; the JSON
# keeps the whole name.
FOLDER_DIGITS = 8

# An outcome printed as itself must look like a word; any other value is
# counted under one word, so no row's text can reach the table.
WORD = re.compile(r"[A-Za-z0-9_.:-]{1,64}")
NOT_A_WORD = "outcome_not_a_word"

# The window every row falls in when none is asked for.
ALL_WINDOW = "all"


@dataclass(frozen=True)
class Window:
    label: str
    start: Optional[int]
    end: Optional[int]

    def holds(self, at: int) -> bool:
        return (self.start is None or at >= self.start) and (self.end is None or at < self.end)


def moment(text: str) -> Optional[int]:
    """Epoch milliseconds from local `YYYY-MM-DDTHH:MM[:SS]` or from digits;
    `None` for an empty end."""
    text = text.strip()
    if not text:
        return None
    if text.isdigit():
        return int(text)
    try:
        return int(datetime.fromisoformat(text).timestamp() * 1000)
    except ValueError:
        raise SystemExit(f"{text!r} is neither a local YYYY-MM-DDTHH:MM time nor epoch milliseconds") from None


def window(spec: str) -> Window:
    label, equals, span = spec.partition("=")
    start, dots, end = span.partition("..")
    if not label or not equals or not dots:
        raise SystemExit(f"--window wants label=from..to, not {spec!r}")
    return Window(label, moment(start), moment(end))


def places(home: Path, projects: list[Path]) -> list[tuple[str, Path]]:
    """Every folder a seat's base ledgers sit in: the window's, then each
    project's — the named ones, or all — as zo names them under the home."""
    found = [(WINDOW_PLACE, home / replay.WINDOW_LEDGERS)]
    if projects:
        folders = [replay.project_dir(home, project) for project in projects]
    else:
        root = home / "projects"
        folders = sorted(root.iterdir()) if root.is_dir() else []
    found += [(PROJECTS_PLACE, folder.joinpath(*replay.PROJECT_LEDGERS)) for folder in folders]
    return [(place, folder) for place, folder in found if folder.is_dir()]


def provider_folders(base: Path) -> list[tuple[str, Path]]:
    """Jev's rows in `base` itself, and each other provider's folder under
    it, by name."""
    found = [(JEV_FOLDER, base)]
    providers = base / replay.PROVIDER_LEDGERS
    if providers.is_dir():
        found += [(entry.name, entry) for entry in sorted(providers.iterdir()) if entry.is_dir()]
    return found


def outcomes(ledger: Path) -> Iterator[tuple[Optional[int], Optional[str]]]:
    """Each request's time and outcome word, read line by line; a line that
    does not parse, or a request without a time, answers `None` for the
    part it lacks. Rows without an outcome are not requests and are skipped."""
    with ledger.open("rb") as handle:
        for line in handle:
            try:
                row = json.loads(line)
            except ValueError:
                yield None, None
                continue
            if not isinstance(row, dict):
                yield None, None
                continue
            if OUTCOME_KEY not in row:
                continue
            at = row.get(AT_KEY)
            outcome = row[OUTCOME_KEY]
            word = outcome if isinstance(outcome, str) and WORD.fullmatch(outcome) else NOT_A_WORD
            yield (at if isinstance(at, int) and not isinstance(at, bool) else None), word


def count(home: Path, windows: list[Window], projects: list[Path] = ()) -> tuple[dict[tuple[str, str, str, str], Counter], Counter]:
    """Outcome words by (place, seat ledger, provider folder, window), and
    by seat ledger how many lines could be placed in no window."""
    tally: dict[tuple[str, str, str, str], Counter] = {}
    unplaced: Counter = Counter()
    for place, base in places(home, list(projects)):
        for folder, path in provider_folders(base):
            for ledger in sorted(path.glob("*" + LEDGER_SUFFIX)):
                seat = ledger.name[: -len(LEDGER_SUFFIX)]
                for at, word in outcomes(ledger):
                    if at is None or word is None:
                        unplaced[seat] += 1
                        continue
                    for one in windows:
                        if one.holds(at):
                            tally.setdefault((place, seat, folder, one.label), Counter())[word] += 1
    return tally, unplaced


def rows_of(tally: dict[tuple[str, str, str, str], Counter], windows: list[Window]) -> list[dict]:
    """One row per place, seat, folder and window that held a request — the
    window's rows in the order the windows were given."""
    order = {one.label: index for index, one in enumerate(windows)}
    keys = sorted(
        tally,
        key=lambda key: (key[0] != WINDOW_PLACE, key[0], key[1], key[2] != JEV_FOLDER, key[2], order[key[3]]),
    )
    rows = []
    for place, seat, folder, label in keys:
        words = tally[(place, seat, folder, label)]
        requests = sum(words.values())
        answered = words.get(ANSWERED, 0)
        rows.append(
            {
                "place": place,
                "seat": seat,
                "folder": folder,
                "window": label,
                "requests": requests,
                "answered": answered,
                "share": answered / requests,
                "failures": dict(sorted(((word, n) for word, n in words.items() if word != ANSWERED), key=lambda one: (-one[1], one[0]))),
            }
        )
    return rows


def short(folder: str) -> str:
    """A provider folder by its kind and the first digits of its hash."""
    stem, dash, digest = folder.rpartition("-")
    return f"{stem}-{digest[:FOLDER_DIGITS]}" if dash and stem else folder


def render(rows: list[dict], unplaced: Counter) -> str:
    head = "| seat ledger | place | provider folder | window | requests | answered | share | failures (most first) |"
    lines = [head, "|" + "|".join(["---"] * (head.count("|") - 1)) + "|"]
    for row in rows:
        failures = " · ".join(f"{word} {n:,}" for word, n in row["failures"].items()) or "—"
        lines.append(
            "| "
            + " | ".join(
                [
                    row["seat"],
                    row["place"],
                    short(row["folder"]),
                    row["window"],
                    f"{row['requests']:,}",
                    f"{row['answered']:,}",
                    f"{row['share'] * 100:.1f}%",
                    failures,
                ]
            )
            + " |"
        )
    if unplaced:
        said = " · ".join(f"{seat} {n:,}" for seat, n in sorted(unplaced.items(), key=lambda one: (-one[1], one[0])))
        lines.append("")
        lines.append(f"Lines with no readable row or no `{AT_KEY}` time, in no window: {said}.")
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--home", type=Path, default=Path.home() / ".zo", help="the zo home to read")
    parser.add_argument("--project", type=Path, action="append", default=[], help="a project whose folder to read (default: every one)")
    parser.add_argument("--window", action="append", default=[], help="label=from..to, one per window")
    parser.add_argument("--json", type=Path, help="the rows as JSON, too")
    args = parser.parse_args(argv)
    windows = [window(spec) for spec in args.window] or [Window(ALL_WINDOW, None, None)]
    if len({one.label for one in windows}) != len(windows):
        raise SystemExit("each --window wants its own label")
    tally, unplaced = count(args.home, windows, args.project)
    rows = rows_of(tally, windows)
    print(render(rows, unplaced))
    if args.json:
        kept = {"windows": [asdict(one) for one in windows], "projects": [str(one) for one in args.project], "unplaced": dict(unplaced), "rows": rows}
        args.json.write_text(json.dumps(kept, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
