#!/usr/bin/env python3
"""How each Jev seat's judgments ended — by the provider folder its rows were
kept in and the time window they fell in — counted straight from the
ledgers' raw rows (t-55423). The same command on rows one running version
wrote, before and after a change, is that change's before/after table; rows
two versions wrote were counted by two formulas and are not compared.

    answered.py [--home <zo home>] [--project <dir> ...]
                [--window <label>=<from>..<to> ...]
                [--app <ZeroCode.app>] [--zo <zo>] [--json <file>]

A time is local `YYYY-MM-DDTHH:MM[:SS]`, the same with `Z` or an offset, or
epoch milliseconds; an end left empty is open, and a window holds its start
and not its end. Windows are reported in UTC. Without `--window` every row
falls in one window, `all`. Without `--project` every project's folder is
read and summed; with it, only the named ones, found by zo's slug rule.
`--app` and `--zo` name the versions installed when the count was taken and
when each one's file last changed; a row does not say which version wrote
it, so a window a version file changed inside is marked.

What a row counts as, from its own fields:
- a row with an outcome is one judgment; a row without one (a label, a
  note) is none;
- sent: its `requests` is 1 or more, and `requests` summed is the attempts
  sent; more than one is a retried judgment;
- not sent: `requests` 0 — a memo hit (`cached`), a question that rode on
  another's request, or an outcome decided before anything went out —
  counted apart, by `cached` or by the outcome;
- answered of sent, failures of sent by outcome word, and applied (its
  `applied` is true);
- probed: its `probe` asked a second question (any value but `not_asked`);
- who it names: an answered row's `model` opens with `<provider identity>:`,
  and a row naming another folder's provider is counted; a window row names
  the route it was sent on (`clientRoute`, written from v1.1.67 on; zo's
  rows name none), counted by route; a sent judgment naming neither is
  unattributed;
- per file: a line that is not a JSON object is corrupt, and a judgment
  without a time is untimed.

Below the seats, each provider folder's judgments in each window are summed
over every seat and place.

Read-only: every ledger is read once and its sha256 taken over the very
bytes counted; the one file written is the one `--json` names. Nothing of a
row leaves this script but its seat, folder, time and the words above.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import plistlib
import re
import subprocess
import sys
from collections import Counter
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

_spec = importlib.util.spec_from_file_location(
    "jev_seat_replay", Path(__file__).resolve().parents[1] / "jev-seat-replay" / "replay.py"
)
replay = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(replay)

# The row keys a judgment is counted by, canonical spelling first
# (`zerocode_core::jev::summary::{AT, OUTCOME, REQUESTS, CACHED, APPLIED,
# MODEL, CLIENT_ROUTE, RUBRIC_VERSION}`), and the one outcome that answered.
AT_KEY = "at"
OUTCOME_KEY = "outcome"
REQUESTS_KEY = "requests"
CACHED_KEY = "cached"
APPLIED_KEY = "applied"
MODEL_KEY = "model"
CLIENT_ROUTE_KEY = "clientRoute"
RUBRIC_KEYS = ("rubricVersion", "rubric_version")
ANSWERED = "answered"

# A decision row's second question, and its word for one never asked
# (`smart_router::decision_shadow::PROBE_NOT_ASKED`).
PROBE_KEY = "probe"
PROBE_NOT_ASKED = "not_asked"

# An answered row's `model` opens with its provider's identity and this
# mark (the window reads it so: `typesafe_settings`).
PROVIDER_MARK = ":"

# A route as `Provider::route_fingerprint` spells it; any other value under
# the key is no route.
ROUTE = re.compile(r"route-[0-9a-f]{1,64}")

# A seat's ledger: one JSON row a line.
LEDGER_SUFFIX = ".jsonl"

# The two places seats keep ledgers, as the table names them: the window's
# folder under the zo home, and the projects' folders summed.
WINDOW_PLACE = "window"
PROJECTS_PLACE = "projects"

# Jev's own rows sit in the place's folder itself rather than under
# `providers/` (`Provider::evidence_dir`); the folder column names it so.
JEV_FOLDER = "(jev)"

# How many hex digits of a provider folder's or a route's hash, and of a
# file's sha256, the markdown shows; the JSON keeps them whole.
FOLDER_DIGITS = 8
SHA_DIGITS = 16

# An outcome printed as itself must look like a word; any other value is
# counted under one word, so no row's text can reach the table.
WORD = re.compile(r"[A-Za-z0-9_.:-]{1,64}")
NOT_A_WORD = "outcome_not_a_word"

# The window every row falls in when none is asked for.
ALL_WINDOW = "all"

# Where a macOS app keeps its version, and how long `zo --version` may take.
APP_INFO = ("Contents", "Info.plist")
APP_VERSION_KEY = "CFBundleShortVersionString"
VERSION_TIMEOUT_S = 10


@dataclass(frozen=True)
class Window:
    label: str
    start: Optional[int]
    end: Optional[int]

    def holds(self, at: int) -> bool:
        return (self.start is None or at >= self.start) and (self.end is None or at < self.end)


@dataclass
class Tally:
    judgments: int = 0
    sent: int = 0
    attempts: int = 0
    answered: int = 0
    applied: int = 0
    retried: int = 0
    probed: int = 0
    other_provider: int = 0
    unattributed: int = 0
    no_requests: int = 0
    failures: Counter = field(default_factory=Counter)
    unsent: Counter = field(default_factory=Counter)
    routes: Counter = field(default_factory=Counter)
    rubrics: Counter = field(default_factory=Counter)


@dataclass
class Read:
    path: str
    bytes: int
    sha256: str
    lines: int = 0
    corrupt: int = 0
    untimed: int = 0


def moment(text: str) -> Optional[int]:
    """Epoch milliseconds from a local or zoned ISO time or from digits;
    `None` for an empty end."""
    text = text.strip()
    if not text:
        return None
    if text.isdigit():
        return int(text)
    try:
        return int(datetime.fromisoformat(text[:-1] + "+00:00" if text.endswith("Z") else text).timestamp() * 1000)
    except ValueError:
        raise SystemExit(f"{text!r} is neither an ISO time nor epoch milliseconds") from None


def utc(at: Optional[int]) -> Optional[str]:
    return None if at is None else datetime.fromtimestamp(at / 1000, tz=timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


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


def word(value) -> str:
    return value if isinstance(value, str) and WORD.fullmatch(value) else NOT_A_WORD


def whole(value) -> Optional[int]:
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def provider_named(model, folder: str) -> Optional[bool]:
    """Whether a row's own `model` names this folder's provider (`True`),
    another's (`False`) or none (`None`). In Jev's own folder any model
    names Jev, whose identity is the model itself."""
    if not isinstance(model, str) or not model:
        return None
    if folder == JEV_FOLDER:
        return True
    named, mark, _ = model.partition(PROVIDER_MARK)
    if not mark:
        return None
    return named == folder


def judge(row: dict, folder: str, tally: Tally) -> None:
    """Count one judgment by its own fields."""
    tally.judgments += 1
    outcome = word(row[OUTCOME_KEY])
    requests = whole(row.get(REQUESTS_KEY))
    sent = requests is not None and requests > 0
    if requests is None or requests < 0:
        tally.no_requests += 1
    elif not sent:
        tally.unsent[CACHED_KEY if row.get(CACHED_KEY) is True else outcome] += 1
    else:
        tally.sent += 1
        tally.attempts += requests
        tally.retried += requests > 1
        if outcome == ANSWERED:
            tally.answered += 1
        else:
            tally.failures[outcome] += 1
    tally.applied += row.get(APPLIED_KEY) is True
    tally.probed += PROBE_KEY in row and row[PROBE_KEY] != PROBE_NOT_ASKED
    route = row.get(CLIENT_ROUTE_KEY)
    route = route if isinstance(route, str) and ROUTE.fullmatch(route) else None
    if route is not None:
        tally.routes[route] += 1
    named = provider_named(row.get(MODEL_KEY), folder)
    tally.other_provider += named is False
    tally.unattributed += sent and route is None and named is None
    rubric = next((row[key] for key in RUBRIC_KEYS if key in row), None)
    if whole(rubric) is not None:
        tally.rubrics[rubric] += 1


def count(home: Path, windows: list[Window], projects: list[Path] = ()) -> tuple[dict[tuple[str, str, str, str], Tally], list[Read]]:
    """Each judgment by (place, seat ledger, provider folder, window), and
    what was read of each ledger: its bytes, their sha256, and the lines
    that were corrupt or untimed."""
    tallies: dict[tuple[str, str, str, str], Tally] = {}
    reads: list[Read] = []
    for place, base in places(home, list(projects)):
        for folder, path in provider_folders(base):
            for ledger in sorted(path.glob("*" + LEDGER_SUFFIX)):
                seat = ledger.name[: -len(LEDGER_SUFFIX)]
                data = ledger.read_bytes()
                read = Read(str(ledger.relative_to(home)), len(data), hashlib.sha256(data).hexdigest())
                for line in data.splitlines():
                    if not line.strip():
                        continue
                    read.lines += 1
                    try:
                        row = json.loads(line)
                    except ValueError:
                        row = None
                    if not isinstance(row, dict):
                        read.corrupt += 1
                        continue
                    if OUTCOME_KEY not in row:
                        continue
                    at = whole(row.get(AT_KEY))
                    if at is None:
                        read.untimed += 1
                        continue
                    for one in windows:
                        if one.holds(at):
                            judge(row, folder, tallies.setdefault((place, seat, folder, one.label), Tally()))
                reads.append(read)
    return tallies, reads


def most_first(words: Counter) -> dict:
    return dict(sorted(words.items(), key=lambda one: (-one[1], str(one[0]))))


def share(answered: int, sent: int) -> Optional[float]:
    return answered / sent if sent else None


def rows_of(tallies: dict[tuple[str, str, str, str], Tally], windows: list[Window]) -> list[dict]:
    """One row per place, seat, folder and window that held a judgment —
    the window's rows in the order the windows were given."""
    order = {one.label: index for index, one in enumerate(windows)}
    keys = sorted(
        tallies,
        key=lambda key: (key[0] != WINDOW_PLACE, key[0], key[1], key[2] != JEV_FOLDER, key[2], order[key[3]]),
    )
    rows = []
    for place, seat, folder, label in keys:
        tally = tallies[(place, seat, folder, label)]
        rows.append(
            {
                "place": place,
                "seat": seat,
                "folder": folder,
                "window": label,
                "rubrics": sorted(tally.rubrics),
                "judgments": tally.judgments,
                "sent": tally.sent,
                "attempts": tally.attempts,
                "answered": tally.answered,
                "share": share(tally.answered, tally.sent),
                "applied": tally.applied,
                "unsent": most_first(tally.unsent),
                "noRequests": tally.no_requests,
                "retried": tally.retried,
                "probed": tally.probed,
                "otherProvider": tally.other_provider,
                "unattributed": tally.unattributed,
                "routes": most_first(tally.routes),
                "failures": most_first(tally.failures),
            }
        )
    return rows


def totals_of(rows: list[dict], windows: list[Window]) -> list[dict]:
    """Each provider folder's judgments in each window, every seat and place
    summed."""
    order = {one.label: index for index, one in enumerate(windows)}
    sums: dict[tuple[str, str], dict] = {}
    for row in rows:
        total = sums.setdefault(
            (row["folder"], row["window"]),
            {"folder": row["folder"], "window": row["window"], "judgments": 0, "sent": 0, "attempts": 0,
             "answered": 0, "noRequests": 0, "unsent": Counter(), "failures": Counter()},
        )
        for name in ("judgments", "sent", "attempts", "answered", "noRequests"):
            total[name] += row[name]
        total["unsent"].update(row["unsent"])
        total["failures"].update(row["failures"])
    totals = [sums[key] for key in sorted(sums, key=lambda key: (key[0] != JEV_FOLDER, key[0], order[key[1]]))]
    for total in totals:
        total["share"] = share(total["answered"], total["sent"])
        total["unsent"] = most_first(total["unsent"])
        total["failures"] = most_first(total["failures"])
    return totals


def short(name: str) -> str:
    """A provider folder or a route by its kind and the first digits of its
    hash."""
    stem, dash, digest = name.rpartition("-")
    return f"{stem}-{digest[:FOLDER_DIGITS]}" if dash and stem else name


def said(words: dict, spell=str) -> str:
    return " · ".join(f"{spell(one)} {n:,}" for one, n in words.items()) or "—"


def percent(value: Optional[float]) -> str:
    return "—" if value is None else f"{value * 100:.1f}%"


def unsent(row: dict) -> str:
    return said(row["unsent"]) + (f" · no requests count {row['noRequests']:,}" if row["noRequests"] else "")


def table(head: list[str], lines: list[list[str]]) -> list[str]:
    return ["| " + " | ".join(head) + " |", "|" + "|".join(["---"] * len(head)) + "|"] + ["| " + " | ".join(line) + " |" for line in lines]


def changed_inside(one: Window, versions: dict) -> list[str]:
    """The installed things whose file changed inside a window."""
    return [name for name, seen in versions.items() if seen and one.holds(moment(seen["modified"]))]


def render(rows: list[dict], totals: list[dict], reads: list[Read], windows: list[Window], counted: dict) -> str:
    def version(name: str) -> str:
        seen = counted["versions"][name]
        return f"{name} not named" if seen is None else f"{name} {seen['version'] or 'unknown'} (file changed {seen['modified']})"

    def spans() -> str:
        said_windows = []
        for one in windows:
            inside = changed_inside(one, counted["versions"])
            mark = f" [{' and '.join(inside)} file changed inside]" if inside else ""
            said_windows.append(f"{one.label} {utc(one.start) or 'open'}..{utc(one.end) or 'open'}{mark}")
        return " · ".join(said_windows)

    out = [
        f"Counted {counted['countedAt']} from {counted['home']} · {version('app')} · {version('zo')}",
        f"Windows (UTC): {spans()}",
        "",
    ]
    out += table(
        ["seat ledger", "place", "provider folder", "window", "rubric", "judgments", "sent", "attempts",
         "answered of sent", "share", "applied", "not sent", "retried", "probed", "other provider",
         "unattributed", "routes", "failures of sent (most first)"],
        [
            [
                row["seat"],
                row["place"],
                short(row["folder"]),
                row["window"],
                ",".join(str(one) for one in row["rubrics"]) or "—",
                f"{row['judgments']:,}",
                f"{row['sent']:,}",
                f"{row['attempts']:,}",
                f"{row['answered']:,}",
                percent(row["share"]),
                f"{row['applied']:,}",
                unsent(row),
                f"{row['retried']:,}",
                f"{row['probed']:,}",
                f"{row['otherProvider']:,}",
                f"{row['unattributed']:,}",
                said(row["routes"], short),
                said(row["failures"]),
            ]
            for row in rows
        ],
    )
    out += ["", "Every seat and place summed, by provider folder and window:", ""]
    out += table(
        ["provider folder", "window", "judgments", "sent", "attempts", "answered of sent", "share", "not sent", "failures of sent (most first)"],
        [
            [
                short(total["folder"]),
                total["window"],
                f"{total['judgments']:,}",
                f"{total['sent']:,}",
                f"{total['attempts']:,}",
                f"{total['answered']:,}",
                percent(total["share"]),
                unsent(total),
                said(total["failures"]),
            ]
            for total in totals
        ],
    )
    out += ["", "Files read (sha256 of the bytes counted):", ""]
    out += table(
        ["file", "bytes", "lines", "corrupt", "untimed", "sha256"],
        [[one.path, f"{one.bytes:,}", f"{one.lines:,}", f"{one.corrupt:,}", f"{one.untimed:,}", one.sha256[:SHA_DIGITS]] for one in reads],
    )
    return "\n".join(out)


def changed(path: Path) -> str:
    return utc(int(path.stat().st_mtime * 1000))


def app_installed(app: Optional[Path]) -> Optional[dict]:
    if app is None:
        return None
    info = app.joinpath(*APP_INFO)
    with info.open("rb") as handle:
        version = plistlib.load(handle).get(APP_VERSION_KEY)
    return {"version": version, "modified": changed(info)}


def zo_installed(zo: Optional[Path]) -> Optional[dict]:
    if zo is None:
        return None
    done = subprocess.run([str(zo), "--version"], capture_output=True, text=True, check=False, timeout=VERSION_TIMEOUT_S)
    return {"version": done.stdout.strip() or None, "modified": changed(zo.resolve())}


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--home", type=Path, default=Path.home() / ".zo", help="the zo home to read")
    parser.add_argument("--project", type=Path, action="append", default=[], help="a project whose folder to read (default: every one)")
    parser.add_argument("--window", action="append", default=[], help="label=from..to, one per window")
    parser.add_argument("--app", type=Path, help="the installed app, to name its version")
    parser.add_argument("--zo", type=Path, help="the installed zo, to name its version")
    parser.add_argument("--json", type=Path, help="the rows and the files read, as JSON too")
    args = parser.parse_args(argv)
    windows = [window(spec) for spec in args.window] or [Window(ALL_WINDOW, None, None)]
    if len({one.label for one in windows}) != len(windows):
        raise SystemExit("each --window wants its own label")
    counted = {
        "countedAt": utc(int(datetime.now(timezone.utc).timestamp() * 1000)),
        "home": str(args.home),
        "projects": [str(one) for one in args.project],
        "versions": {"app": app_installed(args.app), "zo": zo_installed(args.zo)},
    }
    tallies, reads = count(args.home, windows, args.project)
    rows = rows_of(tallies, windows)
    totals = totals_of(rows, windows)
    print(render(rows, totals, reads, windows, counted))
    if args.json:
        kept = dict(
            counted,
            windows=[
                dict(asdict(one), startUtc=utc(one.start), endUtc=utc(one.end), changedInside=changed_inside(one, counted["versions"]))
                for one in windows
            ],
            rows=rows,
            totals=totals,
            files=[asdict(one) for one in reads],
        )
        args.json.write_text(json.dumps(kept, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
