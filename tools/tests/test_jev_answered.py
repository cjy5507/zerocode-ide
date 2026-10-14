#!/usr/bin/env python3
"""Contract for `tools/jev-answered/answered.py` — the share of each Jev
seat's requests that came back answered, by provider folder and time window,
and the failure words of the rest (t-55423).

It must count only rows that carry an outcome, split them by the folder the
provider keeps them in and by the windows asked for, print no part of a row
but its outcome word, and leave the home it reads exactly as it was. The
keys, the answered word and the folder names are read from the Rust they
mirror.

Run: python3 tools/tests/test_jev_answered.py   (stdlib only)
"""

from __future__ import annotations

import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from datetime import datetime
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("jev_answered", REPO / "tools" / "jev-answered" / "answered.py")
answered = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = answered
_spec.loader.exec_module(answered)

PROVIDER = "judgment-" + "ab" * 32
OTHER = "judgment-" + "cd" * 32


def rust(path: str) -> str:
    return (REPO / path).read_text()


def ms(text: str) -> int:
    return int(datetime.fromisoformat(text).timestamp() * 1000)


def write(path: Path, rows: list) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join((row if isinstance(row, str) else json.dumps(row)) + "\n" for row in rows))


class TheNamesAreTheRusts(unittest.TestCase):
    def test_the_keys_and_the_answered_word_are_the_summarys(self) -> None:
        summary = rust("crates/zerocode-core/src/jev/summary.rs")
        self.assertIn(f'pub const ANSWERED: &str = "{answered.ANSWERED}";', summary)
        self.assertIn(f'pub const AT: LedgerKey = LedgerKey {{\n    canonical: "{answered.AT_KEY}",', summary)
        self.assertIn(f'pub const OUTCOME: LedgerKey = LedgerKey {{\n    canonical: "{answered.OUTCOME_KEY}",', summary)

    def test_the_provider_folder_is_the_one_the_evidence_goes_to(self) -> None:
        self.assertIn(
            f'base.join("{answered.replay.PROVIDER_LEDGERS}").join(self.identity())',
            rust("crates/judgment-provider/src/lib.rs"),
        )


class TheCount(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        self.home = Path(self.dir.name) / ".zo"
        window = self.home / answered.replay.WINDOW_LEDGERS
        before, after = ms("2026-10-15T07:00"), ms("2026-10-15T07:30")
        write(
            window / answered.replay.PROVIDER_LEDGERS / PROVIDER / "command-guard.jsonl",
            [
                {"at": before, "outcome": "answered", "command": "secret words"},
                {"at": before + 1, "outcome": "http_503"},
                {"at": before + 2, "outcome": "http_503"},
                {"at": before + 3, "outcome": "timeout"},
                {"at": before + 4, "label": "allow", "agreed": True},
                {"at": after, "outcome": "answered"},
                "{not json",
                {"outcome": "answered"},
            ],
        )
        write(window / answered.replay.PROVIDER_LEDGERS / OTHER / "command-guard.jsonl", [{"at": after, "outcome": "http_502"}])
        write(window / "command-guard.jsonl", [{"at": before, "outcome": "answered"}])
        for slug, outcome in [("Users-dev-a-0123456789abcdef", "answered"), ("Users-dev-b-fedcba9876543210", "http_502")]:
            write(
                self.home / "projects" / slug / Path(*answered.replay.PROJECT_LEDGERS) / answered.replay.PROVIDER_LEDGERS / PROVIDER / "rerank-shadow.jsonl",
                [{"at": before, "outcome": outcome}],
            )
        write(window / "decision-labels.draft.jsonl", [{"words": "a person's prompt"}])
        self.windows = [answered.window("before=2026-10-14T19:10..2026-10-15T07:10"), answered.window("after=2026-10-15T07:12..")]

    def tearDown(self) -> None:
        self.dir.cleanup()

    def table(self) -> list[dict]:
        tally, self.unplaced = answered.count(self.home, self.windows)
        return answered.rows_of(tally, self.windows)

    def test_each_seat_folder_and_window_gets_its_own_share_and_failure_words(self) -> None:
        rows = {(row["place"], row["seat"], row["folder"], row["window"]): row for row in self.table()}
        held = rows[("window", "command-guard", PROVIDER, "before")]
        self.assertEqual((held["requests"], held["answered"]), (4, 1), "the label row is no request")
        self.assertEqual(held["share"], 0.25)
        self.assertEqual(list(held["failures"].items()), [("http_503", 2), ("timeout", 1)], "most first")
        self.assertEqual(rows[("window", "command-guard", PROVIDER, "after")]["requests"], 1)
        self.assertEqual(rows[("window", "command-guard", OTHER, "after")]["failures"], {"http_502": 1})
        self.assertEqual(rows[("window", "command-guard", answered.JEV_FOLDER, "before")]["answered"], 1)
        projects = rows[("projects", "rerank-shadow", PROVIDER, "before")]
        self.assertEqual((projects["requests"], projects["answered"]), (2, 1), "every project's folder is summed")
        self.assertNotIn("decision-labels.draft", {key[1] for key in rows}, "a file with no request is no seat")
        self.assertEqual(self.unplaced, {"command-guard": 2}, "a line that does not parse and a request without a time")

    def test_the_table_says_counts_and_words_and_nothing_else_of_a_row(self) -> None:
        write(
            self.home / answered.replay.WINDOW_LEDGERS / "mail-triage.jsonl",
            [{"at": ms("2026-10-15T07:00"), "outcome": "refused: the person's words here"}],
        )
        text = answered.render(self.table(), self.unplaced)
        self.assertIn(f"| command-guard | window | judgment-{'ab' * 4} | before | 4 | 1 | 25.0% | http_503 2 · timeout 1 |", text)
        self.assertIn(f"| mail-triage | window | {answered.JEV_FOLDER} | before | 1 | 0 | 0.0% | {answered.NOT_A_WORD} 1 |", text)
        self.assertNotIn("secret", text)
        self.assertNotIn("person's", text)
        self.assertIn("in no window: command-guard 2.", text)
        lines = text.splitlines()
        self.assertLess(lines.index(next(line for line in lines if "| window |" in line)), lines.index(next(line for line in lines if "| projects |" in line)))

    def test_a_named_project_is_read_alone_by_zos_slug_and_an_unknown_one_is_refused(self) -> None:
        tally, _ = answered.count(self.home, self.windows, [Path("/Users/dev/a")])
        named = answered.rows_of(tally, self.windows)
        projects = [row for row in named if row["place"] == answered.PROJECTS_PLACE]
        self.assertEqual([(row["seat"], row["requests"], row["answered"]) for row in projects], [("rerank-shadow", 1, 1)])
        self.assertTrue(any(row["place"] == answered.WINDOW_PLACE for row in named), "the window's folder is read still")
        with self.assertRaises(SystemExit):
            answered.count(self.home, self.windows, [Path("/Users/dev/elsewhere")])

    def test_a_window_holds_its_start_and_not_its_end(self) -> None:
        one = answered.window("w=2026-10-15T07:00..2026-10-15T08:00")
        self.assertTrue(one.holds(ms("2026-10-15T07:00")))
        self.assertFalse(one.holds(ms("2026-10-15T08:00")))
        self.assertEqual(answered.window("w=1000..").end, None)
        self.assertEqual(answered.window("w=..1000"), answered.Window("w", None, 1000))
        for bad in ["no-equals", "w=2026-10-15", "w=yesterday..today"]:
            with self.assertRaises(SystemExit):
                answered.window(bad)


class TheHomeIsOnlyRead(unittest.TestCase):
    def test_nothing_under_the_home_changes_and_only_the_named_file_is_written(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            home = Path(root) / ".zo"
            write(home / answered.replay.WINDOW_LEDGERS / answered.replay.PROVIDER_LEDGERS / PROVIDER / "notify-call.jsonl", [{"at": 5, "outcome": "answered"}])
            before = {path: (path.read_bytes(), path.stat().st_mtime_ns) for path in home.rglob("*") if path.is_file()}
            out = Path(root) / "out"
            out.mkdir()
            with redirect_stdout(io.StringIO()) as printed:
                self.assertEqual(answered.main(["--home", str(home), "--json", str(out / "rows.json")]), 0)
            after = {path: (path.read_bytes(), path.stat().st_mtime_ns) for path in home.rglob("*") if path.is_file()}
            self.assertEqual(before, after, "the home changed")
            self.assertEqual(os.listdir(out), ["rows.json"])
            kept = json.loads((out / "rows.json").read_text())
            self.assertEqual([(row["seat"], row["window"], row["share"]) for row in kept["rows"]], [("notify-call", answered.ALL_WINDOW, 1.0)])
            self.assertIn("| notify-call | window |", printed.getvalue())


if __name__ == "__main__":
    unittest.main()
