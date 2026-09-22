#!/usr/bin/env python3
"""Contract for `tools/browser-read-replay/seed.py` — a replay row sees only
what the ledger knew at its own clock, and the seeder's copies of the seat's
numbers are the Rust table's.

Run: python3 tools/tests/test_browser_read_replay_seed.py   (stdlib only)
"""

import importlib.util
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location(
    "browser_read_replay_seed", REPO / "tools" / "browser-read-replay" / "seed.py"
)
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)


def read_row(**over) -> dict:
    row = {
        "at": 1790050000000, "read": "browser-3@1790050000000", "pane": "browser-3",
        "host": "docs.example.com", "pathFingerprint": "0123456789abcdef", "mode": "shadow", "outcome": "answered",
        "elapsedMs": 310, "blocks": 6, "asked": 6, "shards": 1, "chrome": 3, "droppable": 2,
        "folded": 0, "charsBefore": 900, "charsAfter": 900, "applied": False, "requests": 1,
        "redactedLines": 0,
    }
    row.update(over)
    return row


def label_row(**over) -> dict:
    row = {"at": 1790050004000, "label": "browser-3@1790050000000", "agreed": True, "verb": "click",
           "pane": "browser-3", "applied": False, "blockPath": "body>main>article"}
    row.update(over)
    return row


class JoinsTheLabelThatFollowed(unittest.TestCase):
    def test_a_read_carries_its_labels_mark_and_an_unlabelled_read_carries_none(self):
        replays = seed.join([read_row(), label_row(agreed=False), read_row(at=2, read="browser-3@2")])
        self.assertEqual([row["agreed"] for row in replays], [False, None])
        self.assertEqual(replays[0]["labelVerb"], "click")
        self.assertEqual(replays[0]["labelBlockPath"], "body>main>article")
        self.assertEqual(replays[1]["labelVerb"], None)

    def test_a_transition_and_a_label_are_not_reads(self):
        replays = seed.join([
            {"at": 5, "transition": "rise", "read": "x", "outcome": "answered"},
            label_row(),
            read_row(),
        ])
        self.assertEqual([row["read"] for row in replays], ["browser-3@1790050000000"])

    def test_the_first_label_wins_and_later_ones_are_noise(self):
        replays = seed.join([read_row(), label_row(agreed=False), label_row(at=1790050009000, agreed=True)])
        self.assertEqual(replays[0]["agreed"], False)

    def test_a_label_older_than_its_read_is_refused(self):
        with self.assertRaises(ValueError):
            seed.join([read_row(), label_row(at=1790049000000)])

    def test_every_number_is_copied_and_none_is_computed(self):
        replays = seed.join([read_row(charsBefore=1000, charsAfter=400, folded=3)])
        row = replays[0]
        self.assertEqual((row["charsBefore"], row["charsAfter"], row["folded"]), (1000, 400, 3))
        self.assertNotIn("savedShare", row)


class TheCommandLine(unittest.TestCase):
    def test_a_ledger_seeds_a_file_and_a_bad_ledger_exits_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            ledger = Path(tmp) / "browser-read.jsonl"
            ledger.write_text("\n".join(json.dumps(row) for row in [read_row(), label_row(), {"not": "a row"}]) + "\n")
            out = Path(tmp) / "seed.json"
            done = subprocess.run(
                [sys.executable, str(REPO / "tools/browser-read-replay/seed.py"), "--ledger", str(ledger), "--out", str(out)],
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(done.returncode, 0, done.stderr)
            written = json.loads(out.read_text())
            self.assertEqual(written["seat"], "browser_read")
            self.assertEqual(len(written["replays"]), 1)
            self.assertEqual(written["replays"][0]["agreed"], True)
            ledger.write_text(json.dumps(read_row()) + "\n" + json.dumps(label_row(at=1)) + "\n")
            done = subprocess.run(
                [sys.executable, str(REPO / "tools/browser-read-replay/seed.py"), "--ledger", str(ledger), "--out", str(out)],
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(done.returncode, 1)
            self.assertIn("older than its read", done.stderr)


class ConstantsMatchTheirSource(unittest.TestCase):
    """The numbers the seeder carries are copies of Rust constants, and a
    copy that drifts is a seed read against a wall or a line the window
    never used."""

    def named(self, path: str, name: str) -> int:
        text = (REPO / path).read_text()
        found = re.search(rf"const\s+{name}\s*:[^=]*=\s*([0-9_]+)\s*;", text)
        self.assertIsNotNone(found, f"{name} is no longer declared in {path}")
        return int(found.group(1).replace("_", ""))

    def test_the_wall_is_the_seats_own_apply_deadline(self):
        self.assertEqual(seed.WALL_MS, self.named("crates/zerocode-core/src/jev.rs", "BROWSER_READ_APPLY_DEADLINE_MS"))

    def test_the_fold_line_is_the_seats_own(self):
        self.assertEqual(
            seed.FOLD_FLOOR_PERMILLE,
            self.named("crates/zerocode-core/src/jev.rs", "BROWSER_READ_FOLD_FLOOR_PERMILLE"),
        )

    def test_the_block_cap_is_the_tables_own(self):
        self.assertEqual(seed.BLOCK_CAP, self.named("crates/zerocode-core/src/jev.rs", "BROWSER_READ_BLOCK_CAP"))

    def test_the_gatherer_reads_the_cutter_and_the_landmarks_from_their_source(self):
        gather = (REPO / "tools/browser-read-replay/gather.mjs").read_text()
        self.assertIn("BROWSER_READ_BLOCKS_BODY", gather)
        self.assertIn("BROWSER_READ_BLOCK_ROOTS", gather)
        self.assertNotIn('"[role=navigation]"', gather, "the landmark list is not copied")


if __name__ == "__main__":
    unittest.main()
