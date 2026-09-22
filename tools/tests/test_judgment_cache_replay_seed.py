#!/usr/bin/env python3
"""Contract for `tools/judgment-cache-replay/seed.py` — it counts the shapes
of the questions this machine's screen seats asked, carries no words, and
copies its constants from the Rust they belong to.

Run: python3 tools/tests/test_judgment_cache_replay_seed.py   (stdlib only)
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
    "judgment_cache_replay_seed", REPO / "tools" / "judgment-cache-replay" / "seed.py"
)
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)


def ask(flow: str, candidates: int, pressed_before: int, attempt: int, *, elapsed=250, cached=False, outcome="answered") -> dict:
    row = {"flow": flow, "errand": "goal", "mode": "auto", "rubricVersion": 4, "attempt": attempt,
           "outcome": outcome, "candidates": candidates, "showsLines": 0, "pressedBefore": pressed_before,
           "at": 1_790_000_000_000, "elapsedMs": elapsed, "requests": 0 if cached else 1, "redactedLines": 0}
    if cached:
        row["cached"] = True
    return row


def write(path: Path, rows: list) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(row) + "\n" for row in rows))


class Gathering(unittest.TestCase):
    def test_a_repeated_shape_is_counted_once_it_has_been_seen_and_a_judges_note_is_not_an_ask(self):
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw) / "zo"
            write(home / seed.REQUESTS_DIR / "browser-action.jsonl", [
                ask("SENTINEL-FLOW", 4, 0, 1),
                ask("SENTINEL-FLOW", 4, 1, 2),
                ask("SENTINEL-FLOW", 4, 0, 1, cached=True, elapsed=1),
                {"at": 1, "transition": "rise", "rows": 35},
                ask("other", 4, 0, 1, outcome="timeout"),
            ])
            sessions = Path(raw) / "sessions"
            write(sessions / "20260922-000000-1" / "desktop-action.jsonl", [ask("SENTINEL-FLOW", 2, 0, 1)])
            built = seed.build(home, sessions)
            browser = built["seats"]["browser"]
            self.assertEqual(browser["rows"], 4, "the judge's note is not an ask")
            self.assertEqual(browser["answered"], 3)
            self.assertEqual(browser["distinctShapes"], 3)
            self.assertEqual(browser["repeatedShape"], 1, "the cached row repeats the first row's shape")
            self.assertEqual(browser["cached"], 1)
            self.assertEqual(browser["requests"], 3)
            self.assertEqual(browser["wireCalls"], 2, "a cached row answered without a call")
            self.assertEqual(browser["p50Ms"], 250)
            self.assertAlmostEqual(browser["repeatedShare"], 0.25)
            desktop = built["seats"]["desktop"]
            self.assertEqual((desktop["rows"], desktop["repeatedShape"]), (1, 0), "a shape is per seat")

    def test_the_seed_carries_no_words(self):
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw) / "zo"
            write(home / seed.REQUESTS_DIR / "emulator-action.jsonl", [ask("SENTINEL-FLOW", 3, 0, 1)])
            out = Path(raw) / "seed.json"
            result = subprocess.run(
                [sys.executable, str(REPO / "tools/judgment-cache-replay/seed.py"),
                 "--config-home", str(home), "--no-sessions", "--out", str(out)],
                text=True, capture_output=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            text = out.read_text()
            self.assertNotIn("SENTINEL", text, "a flow's name is a digest")
            written = json.loads(text)
            self.assertEqual(written["seats"]["emulator"]["rows"], 1)
            self.assertEqual(written["memoRowsCap"], seed.MEMO_ROWS_CAP)
            self.assertEqual(seed.flow_digest({"flow": "SENTINEL-FLOW"}), seed.flow_digest({"flow": "SENTINEL-FLOW"}))
            self.assertEqual(len(seed.flow_digest({"flow": "x"})), 16)

    def test_a_missing_home_is_an_empty_seed(self):
        built = seed.build(Path("/nonexistent/zo"), Path("/nonexistent/sessions"))
        self.assertEqual(built["seats"], {})


class ConstantsMatchTheirSource(unittest.TestCase):
    """The numbers the seeder carries are copies of Rust constants, and a copy
    that drifts is a seed made for another seat."""

    def test_the_memo_cap_is_the_memos_own(self):
        text = (REPO / "crates/zerocode-core/src/jev/memo.rs").read_text()
        found = re.search(r"const\s+MEMO_ROWS_CAP\s*:[^=]*=\s*([0-9_]+)\s*;", text)
        self.assertIsNotNone(found, "MEMO_ROWS_CAP is no longer declared")
        self.assertEqual(seed.MEMO_ROWS_CAP, int(found.group(1).replace("_", "")))

    def test_the_agreement_floor_is_the_seats_own(self):
        text = (REPO / "crates/zerocode-core/src/jev.rs").read_text()
        found = re.search(r"const\s+JUDGMENT_CACHE_AGREEMENT_FLOOR_PERMILLE\s*:[^=]*=\s*([0-9_]+)\s*;", text)
        self.assertIsNotNone(found, "JUDGMENT_CACHE_AGREEMENT_FLOOR_PERMILLE is no longer declared")
        self.assertEqual(seed.AGREEMENT_FLOOR_PERMILLE, int(found.group(1).replace("_", "")))

    def test_the_ledgers_and_folders_are_the_tables_own(self):
        jev = (REPO / "crates/zerocode-core/src/jev.rs").read_text()
        for ledger in seed.SCREEN_LEDGERS:
            self.assertIn(f'ledger: "{ledger}"', jev)
        count = (REPO / "crates/zerocode-core/src/jev/count.rs").read_text()
        self.assertIn(f'pub const REQUESTS_DIR: &str = "{seed.REQUESTS_DIR}";', count)
        evidence = (REPO / "crates/zerocode-shell/src/computer_use/evidence.rs").read_text()
        self.assertIn(f'pub const SESSIONS_DIR: &str = "{seed.SESSIONS_DIR}";', evidence)


if __name__ == "__main__":
    unittest.main()
