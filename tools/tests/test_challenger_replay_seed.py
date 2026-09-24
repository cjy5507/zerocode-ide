#!/usr/bin/env python3
"""Contract for `tools/challenger-replay/seed.py` — it picks the challenger
arm's ledger rows as each ledger knew them at one clock, and nothing else.

The seeder must find the seat's ledgers where zo writes them, keep only the
rows written at or before `--until` (a replay row sees only what the ledger
knew at its own clock), skip a line that does not parse, and carry the rows
as they are: the draw, the blind and the standing are re-derived by the Rust
replay through the product's own functions.

Run: python3 tools/tests/test_challenger_replay_seed.py   (stdlib only)
"""

from __future__ import annotations

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
    "challenger_replay_seed", REPO / "tools" / "challenger-replay" / "seed.py"
)
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)


def ledger(root: Path, project: str, rows: list) -> Path:
    path = root / project / "state" / seed.LEDGER_DIR / seed.LEDGER_FILE
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join((row if isinstance(row, str) else json.dumps(row)) + "\n" for row in rows))
    return path


class Picking(unittest.TestCase):
    def test_the_ledgers_are_found_where_zo_writes_them(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            one = ledger(root, "proj-a", [{"at": 1}])
            ledger(root, "proj-b", [])
            (root / "proj-c" / "state" / seed.LEDGER_DIR).mkdir(parents=True)
            (root / "proj-c" / "state" / seed.LEDGER_DIR / "patch-review.jsonl").write_text('{"at":1}\n')
            found = seed.ledgers([root])
            self.assertEqual(found[0], one)
            self.assertEqual(len(found), 2, "another seat's ledger is not the challenger's")

    def test_a_row_after_the_clock_is_not_read(self):
        with tempfile.TemporaryDirectory() as raw:
            path = ledger(
                Path(raw),
                "proj",
                [
                    {"at": 10, "outcome": "answered", "attempt": "a#1"},
                    {"at": 20, "label": "a#1", "won": True},
                    "{torn",
                    {"outcome": "answered"},
                ],
            )
            self.assertEqual([row["at"] for row in seed.rows_until(path, 15)], [10], "the label came after the clock")
            self.assertEqual([row["at"] for row in seed.rows_until(path, 20)], [10, 20])

    def test_the_command_writes_the_clock_and_the_rows_as_they_are(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            row = {"at": 5, "outcome": "answered", "attempt": "a#1", "costMicros": 12, "incumbentDesign": "0123456789abcdef"}
            ledger(tmp, "proj", [row, {"at": 99}])
            out = tmp / "seed.json"
            ran = subprocess.run(
                [sys.executable, str(REPO / "tools/challenger-replay/seed.py"), "--project", str(tmp), "--until", "50", "--out", str(out)],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(ran.returncode, 0, ran.stderr)
            written = json.loads(out.read_text())
            self.assertEqual(written["until"], 50)
            self.assertEqual(len(written["ledgers"]), 1)
            self.assertEqual(written["ledgers"][0]["rows"], [row])


class ConstantsMatchTheirSource(unittest.TestCase):
    """The names the seeder carries are copies of Rust constants, and a copy
    that drifts reads a folder nothing writes to."""

    def test_the_ledger_file_is_the_seats_own(self):
        source = (REPO / "crates/zerocode-core/src/jev.rs").read_text()
        block = source[source.index("pub const CHALLENGER: JevUse") :]
        found = re.search(r'ledger:\s*"([^"]+)"', block)
        self.assertIsNotNone(found)
        self.assertEqual(seed.LEDGER_FILE, found.group(1))

    def test_the_ledger_folder_is_the_runtimes_own(self):
        source = (REPO / "zo-ide/crates/runtime/src/config/mod.rs").read_text()
        found = re.search(r'pub const JEV_LEDGER_DIR:\s*&str\s*=\s*"([^"]+)"', source)
        self.assertIsNotNone(found)
        self.assertEqual(seed.LEDGER_DIR, found.group(1))

    def test_the_clock_key_is_the_ledgers_own(self):
        source = (REPO / "crates/zerocode-core/src/jev/summary.rs").read_text()
        found = re.search(r'pub const AT: LedgerKey = LedgerKey \{\s*canonical:\s*"([^"]+)"', source)
        self.assertIsNotNone(found)
        self.assertEqual(seed.AT_KEY, found.group(1))


if __name__ == "__main__":
    unittest.main()
