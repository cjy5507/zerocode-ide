#!/usr/bin/env python3
"""The label audit's seed copies what the ledgers and the black box say and
nothing else (t-6342)."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("label_audit_seed", REPO / "tools/label-audit/seed.py")
seed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(seed)


class LabelAuditSeed(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def write(self, relative: str, lines: list[str]) -> Path:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return path

    def test_the_windows_seats_and_every_projects_seats_are_one_ledger_each(self):
        home = self.root / "zo"
        self.write("zo/jev/stall-cause.jsonl", ['{"at": 1, "stall": "dp-1@1", "outcome": "answered"}', '{"torn": '])
        self.write("zo/projects/b-project/state/smart-router/rerank-shadow.jsonl", ['{"at": 3, "label": "q:n", "rank": 0}'])
        self.write("zo/projects/a-project/state/smart-router/rerank-shadow.jsonl", ['{"at": 2, "label": "q:n"}'])
        found = seed.ledgers(home)
        window, projects = found["window"], found["projects"]
        self.assertEqual([row["stall"] for row in window["stall-cause.jsonl"]], ["dp-1@1"], "a torn line is skipped")
        recall = projects["rerank-shadow.jsonl"]
        self.assertEqual([row["at"] for row in recall], [2, 3], "projects in name order, rows in file order")
        self.assertEqual([row["_project"] for row in recall], [0, 1], "each row names its project")
        self.assertNotIn("_project", window["stall-cause.jsonl"][0], "the window's seats belong to no project")

    def test_a_ledger_name_both_homes_hold_stays_two_ledgers(self):
        # zo's governor wrote `step-effort.jsonl` before it had a ledger of its
        # own; the window's effort seat writes the same name under `jev/`.
        home = self.root / "zo"
        self.write("zo/projects/a-project/state/smart-router/step-effort.jsonl", ['{"at": 1, "kind": "step"}'])
        found = seed.ledgers(home)
        self.assertNotIn("step-effort.jsonl", found["window"], "the window's seat wrote nothing")
        self.assertEqual(len(found["projects"]["step-effort.jsonl"]), 1)

    def test_the_black_box_is_reduced_to_stage_declarations_and_worker_terminals(self):
        log = self.write(
            "window-errors.log",
            [
                "1790048289658 watch main: [12, 3]",
                "1790048295546 watch main: []",
                "1790048295600 watch board-popout: [9]",
                "1790140369240 orchestration: mail waiting for worker:w-6351 in run-4275 is parked for terminal 17's own turn-end hook",
                "not a stamped line worker:w-1 terminal 2",
            ],
        )
        watch, terms = seed.black_box([log, self.root / "missing.log"])
        self.assertEqual(watch, [[1790048289658, [3, 12]], [1790048295546, []]], "the main window's stage only")
        self.assertEqual(terms, {"w-6351": [[1790140369240, 17]]})

    def test_the_seed_file_holds_the_three_parts(self):
        home = self.root / "zo"
        self.write("zo/jev/worker-placement.jsonl", ['{"at": 1, "placement": "w-1", "outcome": "answered"}'])
        log = self.write("window-errors.log", ["5 watch main: [1]"])
        out = self.root / "seed.json"
        self.assertEqual(seed.main(["--zo-home", str(home), "--black-box", str(log), "--out", str(out)]), 0)
        written = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(sorted(written), ["ledgers", "watch", "workerTerms"])
        self.assertEqual(sorted(written["ledgers"]), ["projects", "window"])
        self.assertEqual(written["watch"], [[5, [1]]])
        self.assertEqual(len(written["ledgers"]["window"]["worker-placement.jsonl"]), 1)


if __name__ == "__main__":
    unittest.main()
