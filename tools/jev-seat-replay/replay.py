#!/usr/bin/env python3
"""Every Jev seat's judgment before and after a change, on one frozen copy of
this machine's ledgers — the combined table and nothing else (t-9427).

Nothing here grades anything. The numbers are `zo jev summary --json`'s — the
binary under test, reading the copy through a home of its own — so a table
row is the product's own verdict, and two binaries on the same copy differ
only by what the change changed.

    replay.py snapshot --zo <zo> --project <dir> [--project <dir> ...] --out <dir>
    replay.py table --snapshot <dir> --project <dir> --zo before=<zo> --zo after=<zo>
                    [--seat <id> ...] [--json <file>]

`snapshot` copies, read-only, the seat ledgers the use table names — the
names are read from the binary (`zo jev summary --json` against an empty home
lists every seat's `ledger`), never spelled here — from `<zo home>/jev/` and
from each project's `projects/<slug>/state/smart-router/`, with the settings
file's `smart` block and nothing else of it. The person's files are read and
never written. `table` runs each binary with `ZO_CONFIG_HOME`, `ZO_HOME` and
`HOME` pointed away from the person's home and prints one markdown table.

The judged numbers (window, marks, verdict) are counts of rows and do not move
with the clock; the week's are read from the moment the command runs.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

# The window's seats' ledger folder under the zo home
# (`zerocode_core::jev::count::REQUESTS_DIR`).
WINDOW_LEDGERS = "jev"

# Where each project's zo seats keep theirs, under `<zo home>/projects/<slug>`
# (`runtime::jev_ledger_dir`).
PROJECT_LEDGERS = ("state", "smart-router")

# How many characters of a sanitized path a project slug keeps before its
# hash (`runtime::project_slug`), and the hash's own spelling.
SLUG_STEM_CHARS = 80
SLUG_HASH = re.compile(r"[0-9a-f]{16}")

# The settings block the seats' modes are read from (`SMART_SETTINGS_KEY`).
SMART_SETTINGS_KEY = "smart"


def slug_stem(project: Path) -> str:
    """The sanitized path a project's slug opens with, as zo spells it: every
    character but an ASCII letter, a digit, `-`, `_` or `.` turned to `-`,
    the last eighty kept, dashes trimmed from both ends."""
    sanitized = "".join(ch if (ch.isascii() and ch.isalnum()) or ch in "-_." else "-" for ch in str(project))
    return sanitized[-SLUG_STEM_CHARS:].strip("-")


def project_dir(zo_home: Path, project: Path) -> Path:
    """The one `projects/<stem>-<hash>` folder the project's rows live under.
    Refuses none and refuses two: a guess would copy another project's rows."""
    stem = slug_stem(project)
    root = zo_home / "projects"
    found = [
        entry
        for entry in (sorted(root.iterdir()) if root.is_dir() else [])
        if entry.name.startswith(stem + "-") and SLUG_HASH.fullmatch(entry.name[len(stem) + 1 :])
    ]
    if len(found) != 1:
        raise SystemExit(f"{project}: {len(found)} project folders under {root} open with {stem!r}")
    return found[0]


def summary(zo: Path, zo_home: Path, project: Path) -> dict:
    """`zo jev summary --json --cwd <project>`, run by `zo` with every home it
    could read or write pointed at `zo_home` and a scratch `HOME`."""
    with tempfile.TemporaryDirectory(prefix="jev-seat-replay-home-") as home:
        env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": home,
            "ZO_CONFIG_HOME": str(zo_home),
            "ZO_HOME": str(zo_home),
            "ZO_DISABLE_KEYCHAIN": "1",
        }
        done = subprocess.run(
            [str(zo), "jev", "summary", "--json", "--cwd", str(project)],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
    if done.returncode != 0:
        raise SystemExit(f"{zo}: jev summary exited {done.returncode}: {done.stderr.strip()}")
    return json.loads(done.stdout)


def ledger_names(zo: Path, project: Path) -> list[str]:
    """Every seat's ledger file, as the binary's own table names it."""
    with tempfile.TemporaryDirectory(prefix="jev-seat-replay-empty-") as empty:
        seats = summary(zo, Path(empty), project)["seats"]
    return sorted({seat["ledger"] for seat in seats})


def read_only(path: Path) -> None:
    """Take write permission off `path` and everything under it."""
    for root, dirs, files in os.walk(path, topdown=False):
        for name in files + dirs:
            one = Path(root) / name
            one.chmod(one.stat().st_mode & ~(stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH))
    path.chmod(path.stat().st_mode & ~(stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH))


def snapshot(zo_home: Path, projects: list[Path], ledgers: list[str], out: Path) -> list[Path]:
    """Copy the named ledgers and the settings' `smart` block into
    `out/.zo`, read-only; answer the copies made. Reads `zo_home`, writes
    only under `out`."""
    home = out / ".zo"
    copies: list[Path] = []
    sources = [(zo_home / WINDOW_LEDGERS, home / WINDOW_LEDGERS)]
    for project in projects:
        found = project_dir(zo_home, project)
        sources.append((found.joinpath(*PROJECT_LEDGERS), home / "projects" / found.name / Path(*PROJECT_LEDGERS)))
    for source, target in sources:
        target.mkdir(parents=True, exist_ok=True)
        for name in ledgers:
            if (source / name).is_file():
                shutil.copy2(source / name, target / name)
                copies.append(target / name)
    settings = zo_home / "settings.json"
    smart = json.loads(settings.read_text()).get(SMART_SETTINGS_KEY, {}) if settings.is_file() else {}
    (home / "settings.json").write_text(json.dumps({SMART_SETTINGS_KEY: smart}, indent=1) + "\n")
    read_only(home)
    return copies


def seat_row(seat: dict, binary: str) -> dict:
    """One seat's judged numbers, as one binary's summary gave them."""
    judged = seat.get("judged") or {}
    window = judged.get("window") or {}
    agreement = judged.get("agreement") or {}
    verdict = seat.get("verdict") or {}
    return {
        "seat": seat["id"],
        "binary": binary,
        "stand": seat.get("stand"),
        "verdict": verdict.get("verdict"),
        "line": verdict.get("line"),
        "window": window.get("rows"),
        "wanted": judged.get("windowWanted"),
        "called": window.get("called"),
        "p50": window.get("p50Ms"),
        "p95": window.get("p95Ms"),
        "compared": agreement.get("compared"),
        "agreed": agreement.get("agreed"),
        "notCompared": agreement.get("notCompared"),
        "lowerBound": agreement.get("lowerBound"),
        "baselineAgreed": agreement.get("baselineAgreed"),
        "baselineCompared": agreement.get("baselineCompared"),
        "weekAnswered": (seat.get("week") or {}).get("answered"),
        "toNext": seat.get("rowsToNextJudgment"),
    }


def rows_of(summaries: list[tuple[str, dict]], seats: list[str]) -> list[dict]:
    """The table's rows, seat by seat and binary by binary in the order
    given: the named seats, or every seat some binary found a ledger for."""
    found = {seat["id"] for _, one in summaries for seat in one["seats"] if seat.get("found")}
    wanted = seats or [seat["id"] for seat in summaries[0][1]["seats"] if seat["id"] in found]
    rows = []
    for id in wanted:
        for binary, one in summaries:
            seat = next((seat for seat in one["seats"] if seat["id"] == id), None)
            if seat is not None:
                rows.append(seat_row(seat, binary))
    return rows


def cell(value) -> str:
    return "—" if value is None else str(value)


def render(rows: list[dict]) -> str:
    """The rows as one markdown table."""
    head = "| seat | binary | stand | verdict (line) | window | calls | p50/p95 ms | agreed/compared (lower) | not compared | baseline | week answered | to next |"
    lines = [head, "|" + "|".join(["---"] * (head.count("|") - 1)) + "|"]
    for row in rows:
        verdict = cell(row["verdict"]) + (f" ({row['line']})" if row["line"] else "")
        bound = "—" if row["lowerBound"] is None else f"{row['lowerBound']:.3f}"
        lines.append(
            "| "
            + " | ".join(
                [
                    row["seat"],
                    row["binary"],
                    cell(row["stand"]),
                    verdict,
                    f"{cell(row['window'])}/{cell(row['wanted'])}",
                    cell(row["called"]),
                    f"{cell(row['p50'])}/{cell(row['p95'])}",
                    f"{cell(row['agreed'])}/{cell(row['compared'])} ({bound})",
                    cell(row["notCompared"]),
                    f"{cell(row['baselineAgreed'])}/{cell(row['baselineCompared'])}",
                    cell(row["weekAnswered"]),
                    cell(row["toNext"]),
                ]
            )
            + " |"
        )
    return "\n".join(lines)


def binaries(pairs: list[str]) -> list[tuple[str, Path]]:
    named = []
    for pair in pairs:
        label, _, path = pair.partition("=")
        if not label or not path:
            raise SystemExit(f"--zo wants label=path, not {pair!r}")
        named.append((label, Path(path)))
    return named


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    verbs = parser.add_subparsers(dest="verb", required=True)
    take = verbs.add_parser("snapshot", help="copy the seat ledgers, read-only")
    take.add_argument("--zo", type=Path, required=True, help="a zo binary, asked for the seat table's ledger names")
    take.add_argument("--zo-home", type=Path, default=Path.home() / ".zo")
    take.add_argument("--project", type=Path, action="append", default=[])
    take.add_argument("--out", type=Path, required=True)
    table = verbs.add_parser("table", help="each binary's verdicts on the copy, as one table")
    table.add_argument("--snapshot", type=Path, required=True, help="the folder `snapshot --out` named")
    table.add_argument("--project", type=Path, required=True)
    table.add_argument("--zo", action="append", required=True, help="label=path, one per binary")
    table.add_argument("--seat", action="append", default=[])
    table.add_argument("--json", type=Path)
    args = parser.parse_args(argv)
    if args.verb == "snapshot":
        names = ledger_names(args.zo, args.project[0] if args.project else Path.cwd())
        copies = snapshot(args.zo_home, args.project, names, args.out)
        print(f"{len(copies)} ledgers copied read-only under {args.out / '.zo'}")
        return 0
    home = args.snapshot / ".zo"
    summaries = [(label, summary(zo, home, args.project)) for label, zo in binaries(args.zo)]
    rows = rows_of(summaries, args.seat)
    print(render(rows))
    if args.json:
        args.json.write_text(json.dumps(rows, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
