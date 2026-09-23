#!/usr/bin/env python3
"""Contract for `tools/branching-replay/seed.py` — it gathers the phone
presses a walk's judgment made and the facts the walk had at each, reads
nothing from after a press except its label, and carries the table's numbers
as the Rust source spells them.

Run: python3 tools/tests/test_branching_replay_seed.py   (stdlib only)
"""

import importlib.util
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("branching_replay_seed", REPO / "tools" / "branching-replay" / "seed.py")
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)


def write(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(row) + "\n" for row in rows))


def answered(attempt: int, chosen: str, spread: dict, **more) -> dict:
    row = {"flow": "Wi-Fi 설정을 열어라", "errand": "goal", "mode": "on", "rubricVersion": 4,
           "attempt": attempt, "outcome": "answered", "candidates": len(spread), "confidence": 0.7,
           "probabilities": {"give_up": 0.0, "done": 0.0, **spread}, "chosen": chosen, "pressed": True}
    row.update(more)
    return row


def step(n: int, verb: str, ok: bool = True, **observation) -> dict:
    argv = [verb, "--platform", "android", "--device", "Pixel_6", "--json"]
    if verb == "click":
        argv += ["--mark", "2", "--look", "look-1"]
    return {"n": n, "tool": "emulator", "verb": verb, "argv": argv, "ok": ok, "observation": observation}


class Gathering(unittest.TestCase):
    def test_an_answered_press_is_a_row_with_the_walks_facts_and_its_label(self):
        with tempfile.TemporaryDirectory() as raw:
            folder = Path(raw) / "sessions" / "20260920-1"
            write(folder / seed.EMULATOR_LEDGER, [
                answered(1, "mark:2", {"mark:1": 0.4, "mark:2": 0.6}, agreed=True),
                {"flow": "x", "errand": "goal", "mode": "on", "attempt": 2, "outcome": "no_look"},
            ])
            write(folder / seed.STEPS_LOG, [
                {"n": 1, "tool": "browser", "verb": "goto", "argv": ["goto", "a"], "ok": True},
                step(2, "marks", elapsed_ms=1386),
                step(3, "click", look_ms=1386, act_ms=1420),
            ])
            rows, counted = seed.gather(Path(raw) / "sessions", None)
            self.assertEqual(counted, {"sessions": 1, "emulatorRows": 2, "answered": 1, "ledgerRows": 0})
            self.assertEqual(len(rows), 1, "a row that was never answered is not a press")
            row = rows[0]
            self.assertEqual(row["flow"], "Wi-Fi 설정을 열어라")
            self.assertEqual(row["platform"], "android")
            self.assertEqual(row["device"], "Pixel_6")
            self.assertEqual(row["chosen"], "mark:2")
            self.assertEqual(row["probabilities"], {"give_up": 0.0, "done": 0.0, "mark:1": 0.4, "mark:2": 0.6})
            self.assertEqual(row["lookMs"], 1386)
            self.assertEqual(row["actMs"], 1420)
            self.assertEqual(row["label"], {"agreed": True})

    def test_a_press_the_walk_never_graded_has_no_label_and_the_ledger_is_read_too(self):
        with tempfile.TemporaryDirectory() as raw:
            folder = Path(raw) / "sessions" / "20260920-2"
            write(folder / seed.EMULATOR_LEDGER, [answered(1, "mark:10", {"mark:10": 1.0, "mark:1": 0.0})])
            ledger = Path(raw) / "zo" / "jev" / seed.EMULATOR_LEDGER
            write(ledger, [answered(1, "mark:3", {"mark:3": 0.5, "mark:4": 0.5}, agreed=False),
                           {"outcome": "timeout", "attempt": 1}])
            rows, counted = seed.gather(Path(raw) / "sessions", ledger)
            self.assertEqual(counted["answered"], 2)
            self.assertEqual(counted["ledgerRows"], 2)
            self.assertIsNone(rows[0]["label"], "no mark, no label")
            self.assertIsNone(rows[0]["platform"], "no step log, no device — never guessed")
            self.assertEqual(rows[1]["label"], {"agreed": False})
            self.assertEqual(rows[1]["source"], seed.EMULATOR_LEDGER)

    def test_the_seed_carries_no_rule(self):
        """The seed copies every probability as answered: which of them make a
        fork's candidate is the Rust table's rule (`branching::top_k`)."""
        with tempfile.TemporaryDirectory() as raw:
            folder = Path(raw) / "sessions" / "20260920-3"
            write(folder / seed.EMULATOR_LEDGER, [answered(1, "mark:2", {"mark:1": 0.0, "mark:2": 1.0, "mark:3": 0.0})])
            rows, _ = seed.gather(Path(raw) / "sessions", None)
            self.assertEqual(sorted(rows[0]["probabilities"]), ["done", "give_up", "mark:1", "mark:2", "mark:3"])
            self.assertNotIn("candidates", rows[0], "the seed does not count candidates")
            self.assertNotIn("forked", rows[0], "the seed does not decide forks")


class ConstantsMatchTheirSource(unittest.TestCase):
    """The numbers the seeder carries are copies of Rust constants, and a copy
    that drifts is a seed made for another window."""

    def value_of(self, text: str, name: str, depth: int = 0) -> int:
        self.assertLess(depth, 6, f"{name} chains too deep")
        found = re.search(rf"const\s+{name}\s*:[^=]*=\s*([^;]+);", text)
        self.assertIsNotNone(found, f"{name} is no longer declared")
        expression = found.group(1).strip()
        if re.fullmatch(r"[0-9_*\s]+", expression):
            return eval(expression.replace("_", ""))  # digits and `*` only
        return self.value_of(text, expression, depth + 1)

    def setUp(self):
        self.jev = (REPO / "crates/zerocode-core/src/jev.rs").read_text()

    def test_k(self):
        self.assertEqual(seed.K, self.value_of(self.jev, "BRANCHING_K"))

    def test_k_cap(self):
        self.assertEqual(seed.K_CAP, self.value_of(self.jev, "BRANCHING_K_CAP"))

    def test_the_wall(self):
        self.assertEqual(seed.APPLY_DEADLINE_MS, self.value_of(self.jev, "BRANCHING_APPLY_DEADLINE_MS"))

    def test_the_controls_cap(self):
        self.assertEqual(seed.CONTROLS_CAP, self.value_of(self.jev, "SCREEN_CANDIDATE_CAP"))

    def test_the_ledger_is_the_emulator_seats(self):
        # The row by its declaration, not by the field that happens to end it:
        # a column added to every row must not read as the row moving.
        row = re.search(r'pub const EMULATOR: JevUse = JevUse \{\n(.*?)\n\};', self.jev, re.S)
        self.assertIsNotNone(row, "the emulator row moved")
        found = re.search(r'^\s*ledger:\s*"([^"]+)",', row.group(1), re.M)
        self.assertIsNotNone(found, "the emulator row names no ledger")
        self.assertEqual(seed.EMULATOR_LEDGER, found.group(1))


if __name__ == "__main__":
    unittest.main()
