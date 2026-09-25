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
    replay.py thresholds --zo <zo> --home <zo home> --project <dir> [--write]

`snapshot` copies, read-only, the seat ledgers the use table names — the
names are read from the binary (`zo jev summary --json` against an empty home
lists every seat's `ledger`), never spelled here — from `<zo home>/jev/` and
from each project's `projects/<slug>/state/smart-router/`, with the settings
file's `smart` block and nothing else of it. The person's files are read and
never written. `table` runs each binary with `ZO_CONFIG_HOME`, `ZO_HOME` and
`HOME` pointed away from the person's home and prints one markdown table,
and beside it the act-line table (t-9468): each seat's answers at the line its
bands fix and at the line its labels draw.

`thresholds` asks one binary what each seat's labels draw and keeps it where
the product reads it — the thresholds file beside each seat's ledger, named by
the binary — only on `--write`, through a file renamed into place, and never
outside the home it was pointed at: run on a snapshot it writes the snapshot.

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


def summary(zo: Path, zo_home: Path, project: Path, *more: str) -> dict:
    """`zo jev summary --json --cwd <project>`, run by `zo` with every home it
    could read or write pointed at `zo_home` and a scratch `HOME` — with
    `more` of the verb's own flags (`--act-lines`, t-9468) when asked."""
    with tempfile.TemporaryDirectory(prefix="jev-seat-replay-home-") as home:
        env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": home,
            "ZO_CONFIG_HOME": str(zo_home),
            "ZO_HOME": str(zo_home),
            "ZO_DISABLE_KEYCHAIN": "1",
        }
        done = subprocess.run(
            [str(zo), "jev", "summary", "--json", "--cwd", str(project), *more],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
    if done.returncode != 0:
        raise SystemExit(f"{zo}: jev summary exited {done.returncode}: {done.stderr.strip()}")
    return json.loads(done.stdout)


def ledger_names(zo: Path, project: Path) -> list[str]:
    """Every seat's ledger file, as the binary's own table names it, and the
    file the seats' act lines are kept in beside them (t-9468) when the
    binary names one."""
    with tempfile.TemporaryDirectory(prefix="jev-seat-replay-empty-") as empty:
        answer = summary(zo, Path(empty), project)
    names = {seat["ledger"] for seat in answer["seats"]}
    if answer.get("thresholdsFile"):
        names.add(answer["thresholdsFile"])
    return sorted(names)


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
        # Why, word by word (t-9556) — absent from a binary older than it.
        "notComparedBy": agreement.get("notComparedBy"),
        "lowerBound": agreement.get("lowerBound"),
        "baselineAgreed": agreement.get("baselineAgreed"),
        "baselineCompared": agreement.get("baselineCompared"),
        "weekAnswered": (seat.get("week") or {}).get("answered"),
        "toNext": seat.get("rowsToNextJudgment"),
        # The act line (t-9468): the answers at the line the seat's bands fix
        # and at the line its labels draw — or why they draw none — and the
        # line the product read, from the table beside the ledger.
        **line_numbers("fixed", calibration(seat).get("fixed")),
        "actLine": calibration(seat).get("actFromPermille"),
        "reason": calibration(seat).get("reason"),
        **line_numbers("drawn", calibration(seat).get("drawn")),
        "tableLine": calibration(seat).get("tableLine"),
        "judgedAt": verdict.get("actLine"),
    }


def calibration(seat: dict) -> dict:
    """What a binary's summary said of a seat's act line; nothing from a
    binary older than it."""
    return seat.get("calibration") or {}


def line_numbers(side: str, at: dict | None) -> dict:
    """The answers at one line, as the binary counted them: the line, the
    share of answered requests it acts on, and how often the marks of what
    it acts on, the baseline on the same marks, and the marks of what it
    leaves alone said wrong, per thousand."""
    at = at or {}
    return {
        f"{side}Line": at.get("fromPermille"),
        f"{side}Share": at.get("applyShare"),
        f"{side}Error": at.get("errorPermille"),
        f"{side}BaselineError": at.get("baselineErrorPermille"),
        f"{side}UnderError": at.get("underErrorPermille"),
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


def withheld(row: dict) -> str:
    """How many rows compared nothing, and why, most first: `4 (not_carried
    3 · unseen 1)`; the count alone where the binary said no words."""
    words = row.get("notComparedBy") or {}
    said = " · ".join(f"{word} {count}" for word, count in sorted(words.items(), key=lambda one: (-one[1], one[0])))
    return cell(row["notCompared"]) + (f" ({said})" if said else "")


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
                    withheld(row),
                    f"{cell(row['baselineAgreed'])}/{cell(row['baselineCompared'])}",
                    cell(row["weekAnswered"]),
                    cell(row["toNext"]),
                ]
            )
            + " |"
        )
    return "\n".join(lines)


def share(value) -> str:
    return "—" if value is None else f"{value * 100:.0f}%"


def per_thousand(value) -> str:
    return "—" if value is None else f"{value}‰"


def render_lines(rows: list[dict]) -> str:
    """The act-line table: before — the line the seat's bands fix — and
    after — the line its labels draw, or why none — each with the share it
    acts on, how often what it acts on is wrong, the baseline on the same
    marks, and what it leaves alone."""
    head = (
        "| seat | binary | fixed line | acts on | wrong | baseline wrong | left wrong "
        "| drawn line | acts on | wrong | baseline wrong | left wrong | table line |"
    )
    lines = [head, "|" + "|".join(["---"] * (head.count("|") - 1)) + "|"]
    for row in rows:
        drawn = per_thousand(row["actLine"]) if row["actLine"] is not None else f"none ({cell(row['reason'])})"
        lines.append(
            "| "
            + " | ".join(
                [
                    row["seat"],
                    row["binary"],
                    per_thousand(row["fixedLine"]),
                    share(row["fixedShare"]),
                    per_thousand(row["fixedError"]),
                    per_thousand(row["fixedBaselineError"]),
                    per_thousand(row["fixedUnderError"]),
                    drawn,
                    share(row["drawnShare"]),
                    per_thousand(row["drawnError"]),
                    per_thousand(row["drawnBaselineError"]),
                    per_thousand(row["drawnUnderError"]),
                    per_thousand(row["tableLine"]),
                ]
            )
            + " |"
        )
    return "\n".join(lines)


def threshold_rows(answer: dict, home: Path) -> dict[Path, list[dict]]:
    """The rows one binary's summary drew for the seats whose stage reads an
    act line, grouped by the folder each seat's ledger was found in — every
    folder under `home`, or the command refuses: a row is kept beside the
    ledger it was read off, and nowhere else."""
    folders: dict[Path, list[dict]] = {}
    for seat in answer["seats"]:
        row = calibration(seat).get("row")
        if not row or not seat.get("found"):
            continue
        folder = Path(seat["found"]).parent
        if not folder.resolve().is_relative_to(home.resolve()):
            raise SystemExit(f"{seat['id']}: its ledger is at {folder}, outside {home}")
        folders.setdefault(folder, []).append(row)
    return folders


def write_table(folder: Path, name: str, rows: list[dict]) -> Path:
    """Keep `rows` as `folder/name`: written beside it and renamed over it,
    so a reader sees the old table or the new one and never half of either.
    A folder a snapshot left read-only is opened for the rename and closed
    again, and the file keeps the folder's word."""
    mode = stat.S_IMODE(folder.stat().st_mode)
    closed = not mode & stat.S_IWUSR
    if closed:
        folder.chmod(mode | stat.S_IWUSR)
    try:
        handle, temporary = tempfile.mkstemp(dir=folder, prefix=f".{name}.", suffix=".tmp")
        with os.fdopen(handle, "w") as out:
            out.write(json.dumps(rows, indent=1) + "\n")
        os.chmod(temporary, 0o444 if closed else 0o644)
        os.replace(temporary, folder / name)
    finally:
        if closed:
            folder.chmod(mode)
    return folder / name


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
    keep = verbs.add_parser("thresholds", help="what each seat's labels draw, kept beside its ledger on --write")
    keep.add_argument("--zo", type=Path, required=True)
    keep.add_argument("--home", type=Path, required=True, help="the zo home to read — and, on --write, to keep the table in")
    keep.add_argument("--project", type=Path, required=True)
    keep.add_argument("--write", action="store_true", help="keep the rows; without it, only say what would be kept")
    args = parser.parse_args(argv)
    if args.verb == "snapshot":
        names = ledger_names(args.zo, args.project[0] if args.project else Path.cwd())
        copies = snapshot(args.zo_home, args.project, names, args.out)
        print(f"{len(copies)} ledgers copied read-only under {args.out / '.zo'}")
        return 0
    if args.verb == "thresholds":
        # Every line of the grid and the row to keep ride only when asked.
        answer = summary(args.zo, args.home, args.project, "--act-lines")
        name = answer.get("thresholdsFile")
        if not name:
            raise SystemExit(f"{args.zo}: this binary names no thresholds file")
        for folder, kept in threshold_rows(answer, args.home).items():
            said = ", ".join(f"{row['seat']} {row.get('actFromPermille') or 'none (' + str(row.get('reason')) + ')'}" for row in kept)
            if args.write:
                print(f"kept {write_table(folder, name, kept)}: {said}")
            else:
                print(f"would keep {folder / name}: {said}")
        return 0
    home = args.snapshot / ".zo"
    summaries = [(label, summary(zo, home, args.project)) for label, zo in binaries(args.zo)]
    rows = rows_of(summaries, args.seat)
    print(render(rows))
    if any(row["fixedLine"] is not None or row["reason"] is not None or row["actLine"] is not None for row in rows):
        print()
        print(render_lines(rows))
    if args.json:
        args.json.write_text(json.dumps(rows, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
