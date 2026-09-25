#!/usr/bin/env python3
"""Contract for `tools/jev-seat-replay/replay.py` — it copies the seat ledgers
read-only and tabulates what a binary said, and grades nothing itself.

The copy must take only the ledgers it was named, only the settings' `smart`
block, leave the person's files as they were and itself unwritable; the
project folder must be found the way zo spells its slug, or refused; and the
table must carry each binary's verdict as that binary's summary gave it. The
folder names and the slug's stem width are read from the Rust they mirror.

Run: python3 tools/tests/test_jev_seat_replay.py   (stdlib only)
"""

from __future__ import annotations

import importlib.util
import json
import os
import re
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("jev_seat_replay", REPO / "tools" / "jev-seat-replay" / "replay.py")
replay = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = replay
_spec.loader.exec_module(replay)


def rust(path: str) -> str:
    return (REPO / path).read_text()


def seat(id: str, found: bool, verdict: str, line: str | None, compared: int, agreed: int) -> dict:
    return {
        "id": id,
        "found": "/copy/ledger.jsonl" if found else None,
        "stand": "recording",
        "rowsToNextJudgment": 3,
        "week": {"answered": 9},
        "judged": {
            "window": {"rows": 25, "called": 25, "p50Ms": 300, "p95Ms": 640},
            "windowWanted": 25,
            "agreement": {
                "compared": compared,
                "agreed": agreed,
                "notCompared": 4,
                "notComparedBy": {"unseen": 1, "not_carried": 3},
                "lowerBound": 0.5,
                "baselineAgreed": 1,
                "baselineCompared": 2,
            },
        },
        "verdict": {"verdict": verdict, "line": line},
    }


class TheNamesAreTheRusts(unittest.TestCase):
    def test_the_folders_are_the_ones_the_seats_write(self) -> None:
        self.assertIn(f'pub const REQUESTS_DIR: &str = "{replay.WINDOW_LEDGERS}";', rust("crates/zerocode-core/src/jev/count.rs"))
        self.assertIn(
            f'pub const JEV_LEDGER_DIR: &str = "{replay.PROJECT_LEDGERS[1]}";',
            rust("zo-ide/crates/runtime/src/config/mod.rs"),
        )
        self.assertIn('.join("state")', rust("zo-ide/crates/runtime/src/config/mod.rs"))

    def test_the_slug_keeps_the_stem_zo_keeps_and_a_hash_of_its_width(self) -> None:
        config = rust("zo-ide/crates/runtime/src/config/mod.rs")
        self.assertIn(f"sanitized.len().saturating_sub({replay.SLUG_STEM_CHARS})", config)
        self.assertIn('format!("{:016x}", hasher.finish())', rust("zo-ide/crates/runtime/src/sandbox.rs"))
        self.assertEqual(replay.slug_stem(Path("/Users/someone/2026/zerocode")), "Users-someone-2026-zerocode")
        # One dash per character, as Rust's `chars()` counts them: the slash,
        # then each of the two syllables.
        self.assertEqual(replay.slug_stem(Path("/tmp/a b/한글.d")), "tmp-a-b---.d")
        long = Path("/" + "x" * 100)
        self.assertEqual(len(replay.slug_stem(long)), replay.SLUG_STEM_CHARS)


class TheCopy(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        root = Path(self.dir.name)
        self.home = root / "home" / ".zo"
        (self.home / "jev").mkdir(parents=True)
        (self.home / "jev" / "notify-call.jsonl").write_text('{"at": 1}\n')
        (self.home / "jev" / "requests-2026-09-26.count").write_text("7\n")
        self.project = Path("/Users/someone/work")
        slug = self.home / "projects" / f"{replay.slug_stem(self.project)}-0123456789abcdef" / "state" / "smart-router"
        slug.mkdir(parents=True)
        (slug / "rerank-shadow.jsonl").write_text('{"at": 2}\n')
        (slug / "decision-labels.draft.jsonl").write_text('{"words": "a person\'s prompt"}\n')
        (self.home / "settings.json").write_text(json.dumps({"smart": {"jev": {"enabled": True}}, "credentials": "secret"}))
        self.out = root / "copy"

    def tearDown(self) -> None:
        for dirpath, dirnames, filenames in os.walk(self.dir.name):
            for name in dirnames + filenames:
                os.chmod(os.path.join(dirpath, name), 0o700)
        self.dir.cleanup()

    def test_it_takes_only_the_named_ledgers_and_the_smart_block_and_cannot_be_written(self) -> None:
        before = {path: path.read_bytes() for path in self.home.rglob("*") if path.is_file()}
        copies = replay.snapshot(self.home, [self.project], ["notify-call.jsonl", "rerank-shadow.jsonl"], self.out)
        self.assertEqual(sorted(path.name for path in copies), ["notify-call.jsonl", "rerank-shadow.jsonl"])
        copied = sorted(str(path.relative_to(self.out)) for path in self.out.rglob("*") if path.is_file())
        self.assertNotIn("decision-labels.draft.jsonl", " ".join(copied), "a file no seat names was copied")
        self.assertNotIn("requests-2026-09-26.count", " ".join(copied))
        self.assertEqual(json.loads((self.out / ".zo" / "settings.json").read_text()), {"smart": {"jev": {"enabled": True}}})
        after = {path: path.read_bytes() for path in self.home.rglob("*") if path.is_file()}
        self.assertEqual(before, after, "the person's files changed")
        with self.assertRaises(PermissionError):
            (self.out / ".zo" / "jev" / "notify-call.jsonl").write_text("written\n")
        with self.assertRaises(PermissionError):
            (self.out / ".zo" / "jev" / "new.jsonl").write_text("written\n")

    def test_a_project_folder_is_one_or_refused(self) -> None:
        self.assertTrue(replay.project_dir(self.home, self.project).name.endswith("-0123456789abcdef"))
        with self.assertRaises(SystemExit):
            replay.project_dir(self.home, Path("/Users/someone/elsewhere"))
        twin = self.home / "projects" / f"{replay.slug_stem(self.project)}-fedcba9876543210"
        twin.mkdir()
        with self.assertRaises(SystemExit):
            replay.project_dir(self.home, self.project)


class TheTable(unittest.TestCase):
    def test_each_binary_says_its_own_verdict_seat_by_seat(self) -> None:
        before = {"seats": [seat("routing", False, "hold", "too_few_rows", 0, 0), seat("recall", True, "fall", "latency", 0, 0)]}
        after = {"seats": [seat("routing", False, "hold", "too_few_rows", 0, 0), seat("recall", True, "keep", None, 0, 0)]}
        rows = replay.rows_of([("before", before), ("after", after)], [])
        self.assertEqual([(row["seat"], row["binary"]) for row in rows], [("recall", "before"), ("recall", "after")])
        text = replay.render(rows)
        self.assertIn(
            "| recall | before | recording | fall (latency) | 25/25 | 25 | 300/640 | 0/0 (0.500) | 4 (not_carried 3 · unseen 1) | 1/2 | 9 | 3 |",
            text,
        )
        self.assertIn("| recall | after | recording | keep | 25/25 |", text)
        named = replay.rows_of([("before", before)], ["routing"])
        self.assertEqual([row["seat"] for row in named], ["routing"], "a named seat is tabled though nothing was found")
        self.assertTrue(re.match(r"^\| seat \|", text))

    def test_why_the_rows_compare_nothing_rides_word_by_word(self) -> None:
        # t-9556: the reasons the binary counted, most first, and nothing
        # where a binary counted none or predates the words.
        row = replay.seat_row(seat("placement", True, "hold", "agreement", 1, 1), "after")
        self.assertEqual(row["notComparedBy"], {"unseen": 1, "not_carried": 3})
        older = seat("placement", True, "hold", "agreement", 1, 1)
        del older["judged"]["agreement"]["notComparedBy"]
        self.assertIn("| 4 |", replay.render([replay.seat_row(older, "before")]))

    def test_the_act_line_rides_before_and_after(self) -> None:
        # t-9468: the answers at the line the seat's bands fix, and at the
        # line its labels draw — or why they draw none.
        at = lambda line, share, wrong, baseline, left: {  # noqa: E731
            "fromPermille": line, "applyShare": share, "errorPermille": wrong,
            "baselineErrorPermille": baseline, "underErrorPermille": left,
        }
        drew = seat("notify", True, "rise", None, 50, 47)
        drew["calibration"] = {
            "fixed": at(850, 0.41, 60, 1000, 480), "actFromPermille": 300, "reason": None,
            "drawn": at(300, 0.5, 60, 1000, 500), "tableLine": 300,
        }
        drew["verdict"]["actLine"] = 300
        none = seat("stall", True, "hold", "too_few_rows", 0, 0)
        none["calibration"] = {"fixed": at(850, 0.9, 70, 400, 0), "actFromPermille": None, "reason": "whole"}
        rows = [replay.seat_row(drew, "after"), replay.seat_row(none, "after")]
        self.assertEqual((rows[0]["judgedAt"], rows[0]["tableLine"]), (300, 300))
        text = replay.render_lines(rows)
        self.assertIn("| notify | after | 850‰ | 41% | 60‰ | 1000‰ | 480‰ | 300‰ | 50% | 60‰ | 1000‰ | 500‰ | 300‰ |", text)
        self.assertIn("| stall | after | 850‰ | 90% | 70‰ | 400‰ | 0‰ | none (whole) | — | — | — | — | — |", text)
        older = replay.seat_row(seat("notify", True, "hold", None, 0, 0), "before")
        self.assertEqual((older["fixedLine"], older["actLine"], older["reason"]), (None, None, None), "a binary older than the line")

    def test_a_binary_is_named_by_its_label(self) -> None:
        self.assertEqual(replay.binaries(["before=/a/zo"]), [("before", Path("/a/zo"))])
        with self.assertRaises(SystemExit):
            replay.binaries(["/a/zo"])


class TheTableKept(unittest.TestCase):
    """t-9468: the rows a binary drew are kept beside the ledger each was
    read off, through a file renamed into place, and never outside the home
    the command was pointed at — a snapshot's folder is opened for the write
    and closed again."""

    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        self.home = Path(self.dir.name) / ".zo"
        self.window = self.home / "jev"
        self.window.mkdir(parents=True)

    def tearDown(self) -> None:
        for dirpath, dirnames, filenames in os.walk(self.dir.name):
            for name in dirnames + filenames:
                os.chmod(os.path.join(dirpath, name), 0o700)
        self.dir.cleanup()

    def answer(self, found: Path) -> dict:
        row = {"seat": "notify", "rubricVersion": 1, "computedAtMs": 5, "actFromPermille": 300}
        return {
            "thresholdsFile": "thresholds.json",
            "seats": [
                {"id": "notify", "found": str(found), "calibration": {"row": row}},
                {"id": "recall", "found": str(found), "calibration": {"row": None}},
                {"id": "stall", "found": None, "calibration": {"row": dict(row, seat="stall")}},
            ],
        }

    def test_rows_are_kept_beside_their_ledger_and_only_under_the_home(self) -> None:
        kept = replay.threshold_rows(self.answer(self.window / "notify-call.jsonl"), self.home)
        self.assertEqual(list(kept), [self.window])
        self.assertEqual([row["seat"] for row in kept[self.window]], ["notify"], "only a seat whose stage reads a line")
        with self.assertRaises(SystemExit):
            replay.threshold_rows(self.answer(Path(self.dir.name) / "elsewhere" / "notify-call.jsonl"), self.home)

    def test_a_read_only_folder_is_opened_for_the_rename_and_closed_again(self) -> None:
        (self.window / "thresholds.json").write_text("[]\n")
        replay.read_only(self.home)
        rows = [{"seat": "notify", "actFromPermille": 300}]
        path = replay.write_table(self.window, "thresholds.json", rows)
        self.assertEqual(json.loads(path.read_text()), rows)
        self.assertEqual([p.name for p in self.window.iterdir()], ["thresholds.json"], "no temporary file left behind")
        with self.assertRaises(PermissionError):
            (self.window / "new.jsonl").write_text("written\n")
        with self.assertRaises(PermissionError):
            path.write_text("[]\n")

    def test_the_snapshot_takes_the_table_the_binary_names(self) -> None:
        source = Path(self.dir.name) / "person" / ".zo"
        (source / "jev").mkdir(parents=True)
        (source / "jev" / "notify-call.jsonl").write_text('{"at": 1}\n')
        (source / "jev" / "thresholds.json").write_text('[{"seat": "notify"}]\n')
        out = Path(self.dir.name) / "copy"
        copies = replay.snapshot(source, [], ["notify-call.jsonl", "thresholds.json"], out)
        self.assertEqual(sorted(path.name for path in copies), ["notify-call.jsonl", "thresholds.json"])


if __name__ == "__main__":
    unittest.main()
