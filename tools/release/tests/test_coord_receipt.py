#!/usr/bin/env python3
"""The coordinator's gate receipt (`tools/release/coord_receipt.py`, t-47516).

A receipt stands only for what its evidence still says: every job green by its rc
file, its RC lines and its verdict, all on one tree under one owner and run, its
bytes as sealed, made after the last job ended. A recipe counts only where an entry
ran exactly `just <recipe>` in that gate's folder.

Run: python3 tools/release/tests/test_coord_receipt.py   (stdlib only)
"""

import importlib.util
import json
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

import receipt_fixture
from receipt_fixture import HEAD, RUN, TREE

SCRIPT = Path(__file__).resolve().parents[1] / "coord_receipt.py"
SPEC = importlib.util.spec_from_file_location("coord_receipt", SCRIPT)
coord_receipt = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(coord_receipt)

ZO_TEST_RUNTIME = "zsh -c 'cd zo-ide && ZO_DISABLE_KEYCHAIN=1 cargo test -p runtime --lib --bins --tests --no-fail-fast'"
# The shape of the coordinator's gate on 10-22 (l64): the Windows check as the
# development recipe, zo's tests split by package, zo's lints through just.
GATE = {
    "g1-build": [("pii-check", "just pii-check", 0), ("win-check", "just win-check-if-available", 0)],
    "g2-tests": [("test", "just test", 0), ("tools-test", "just tools-test", 0)],
    "g3a-zo-lint": [("zo-fmt-check", "zsh -c 'cd zo-ide && just fmt-check'", 0),
                    ("zo-clippy", "zsh -c 'cd zo-ide && just clippy'", 0)],
    "g3b1-zo-test-runtime": [("zo-test-runtime", ZO_TEST_RUNTIME, 0)],
}


class ReceiptCase(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="receipt-"))

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def gate(self, jobs=None, **kw):
        return receipt_fixture.write_gate(self.tmp, "l9", jobs or GATE, **kw)

    def made(self, jobs):
        path = self.tmp / "receipt.json"
        path.write_text(json.dumps(coord_receipt.make(jobs)))
        return path

    def refused(self, call):
        with self.assertRaises(coord_receipt.Refused) as caught:
            call()
        return str(caught.exception)

    def facts(self, receipt, folder=None):
        return coord_receipt.read(receipt, folder)


class Reading(ReceiptCase):
    def test_a_green_gate_reads_back_as_its_owner_run_tree_logs_and_head(self):
        jobs = self.gate()
        facts = self.facts(self.made(jobs))
        self.assertEqual(facts[:4], ["who coord-l9", f"run {RUN}", f"tree {TREE}", f"logs {self.tmp / 'gates' / 'l9'}"])
        heads = [line.split() for line in facts if line.startswith("head ")]
        self.assertEqual([head[1] for head in heads], [HEAD])
        self.assertLessEqual(int(heads[0][2]), int(time.time()))
        self.assertFalse([line for line in facts if line.startswith("recipe ")], "recipes only for a folder asked about")

    def test_a_recipe_counts_only_where_an_entry_ran_exactly_just_it_in_that_folder(self):
        receipt = self.made(self.gate())
        recipes = lambda folder: [line.split()[1] for line in self.facts(receipt, folder) if line.startswith("recipe ")]
        # The development Windows check is its own recipe, not the lane's strict one,
        # and zo's tests run by package are no `just test` in zo-ide.
        self.assertEqual(recipes("."), ["pii-check", "test", "tools-test", "win-check-if-available"])
        self.assertEqual(recipes("zo-ide"), ["clippy", "fmt-check"])
        self.assertEqual(recipes("zo-ide/"), ["clippy", "fmt-check"])

    def test_a_command_that_is_not_the_recipe_alone_counts_for_nothing(self):
        for command in ("just test --nocapture", "just -f other test", "zsh -c 'cd zo-ide && FOO=1 just test'",
                        "zsh -c 'cd zo-ide && just test && true'", "cargo test --workspace", "just"):
            with self.subTest(command=command):
                words = shlex.split(command)
                self.assertIsNone(coord_receipt.recipe_of(words, "."))
                self.assertIsNone(coord_receipt.recipe_of(words, "zo-ide"))
        self.assertEqual(coord_receipt.recipe_of(["bash", "-c", "cd zo-ide && just e2e"], "zo-ide"), "e2e")
        self.assertIsNone(coord_receipt.recipe_of(["bash", "-c", "cd zo-ide && just e2e"], "."))

    def test_each_head_is_said_with_the_first_end_of_its_jobs(self):
        one = self.gate({"g1": [("pii-check", "just pii-check", 0)]}, head="3" * 40)
        two = receipt_fixture.write_gate(self.tmp, "l9", {"g2": [("test", "just test", 0)]}, head="4" * 40)
        receipt_fixture.ended(one[0], 1792610000)
        receipt_fixture.ended(two[0], 1792610500)
        heads = [line for line in self.facts(self.made(one + two)) if line.startswith("head ")]
        self.assertEqual(heads, [f"head {'3' * 40} 1792610000", f"head {'4' * 40} 1792610500"])

    def test_the_cli_prints_the_facts_and_says_a_refusal_on_one_line(self):
        jobs = self.gate()
        made = subprocess.run([sys.executable, str(SCRIPT), "make", *map(str, jobs)], capture_output=True, text=True)
        self.assertEqual(made.returncode, 0, made.stdout + made.stderr)
        receipt = self.tmp / "cli.json"
        receipt.write_text(made.stdout)
        read = subprocess.run([sys.executable, str(SCRIPT), "read", "--receipt", str(receipt), "--dir", "."],
                              capture_output=True, text=True)
        self.assertEqual(read.returncode, 0, read.stdout + read.stderr)
        self.assertIn("recipe pii-check", read.stdout.splitlines())
        Path(str(jobs[0])[:-4] + ".rc").write_text("1\n")
        again = subprocess.run([sys.executable, str(SCRIPT), "read", "--receipt", str(receipt)], capture_output=True, text=True)
        self.assertEqual(again.returncode, 1)
        self.assertEqual(len(again.stdout.splitlines()), 1, again.stdout)
        self.assertIn("ended rc=1", again.stdout)


class Refusing(ReceiptCase):
    def test_a_red_job_is_refused(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0), ("test", "just test", 101)]})
        self.assertIn("ended rc=1", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_job_the_line_capped_is_refused(self):
        jobs = self.gate(rc={"g2-tests": "capped(1800s)"})
        self.assertIn("ended rc=capped(1800s)", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_red_entry_is_refused_under_a_green_rc_file(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0), ("test", "just test", 101)]}, rc={"g1": "0"})
        self.assertIn("entry test rc=101", self.refused(lambda: coord_receipt.make(jobs)))

    def test_an_entry_that_never_closed_is_refused(self):
        jobs = self.gate()
        log = Path(str(jobs[1])[:-4] + ".log")
        log.write_text("".join(line for line in log.read_text().splitlines(True) if not line.startswith("RC test=")))
        self.assertIn("entries and RC lines differ at test", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_verdict_that_counted_red_is_refused(self):
        jobs = self.gate()
        log = Path(str(jobs[0])[:-4] + ".log")
        log.write_text(log.read_text().replace("verdict: bad=0", "verdict: bad=1"))
        self.assertIn("verdict is not bad=0", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_log_that_does_not_open_with_its_head_and_tree_is_refused(self):
        jobs = self.gate()
        log = Path(str(jobs[2])[:-4] + ".log")
        log.write_text("STOP tree\n" + log.read_text())
        self.assertIn("does not open with its head and tree", self.refused(lambda: coord_receipt.make(jobs)))

    def test_jobs_on_two_trees_are_refused(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
        jobs += receipt_fixture.write_gate(self.tmp, "l9", {"g2": [("test", "just test", 0)]}, tree="5" * 40)
        self.assertIn("more than one tree", self.refused(lambda: coord_receipt.make(jobs)))

    def test_jobs_of_two_owners_or_runs_are_refused(self):
        for key, value in (("who", "w-70906"), ("run", "run-1")):
            with self.subTest(key=key):
                jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
                jobs += receipt_fixture.write_gate(self.tmp, "l9", {"g2": [("test", "just test", 0)]}, **{key: value})
                self.assertIn(f"more than one {key}", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_missing_entry_log_or_job_file_is_refused(self):
        jobs = self.gate()
        (self.tmp / "gates" / "l9" / "tools-test.log").unlink()
        self.assertIn("tools-test.log is missing", self.refused(lambda: coord_receipt.make(jobs)))
        self.assertIn("is missing", self.refused(lambda: coord_receipt.make([self.tmp / "no-such.job"])))

    def test_a_linked_file_is_refused(self):
        jobs = self.gate()
        rc = Path(str(jobs[0])[:-4] + ".rc")
        (self.tmp / "zero").write_text("0\n")
        rc.unlink()
        rc.symlink_to(self.tmp / "zero")
        self.assertIn("is not a plain file", self.refused(lambda: coord_receipt.make(jobs)))

    def test_evidence_changed_after_the_receipt_was_made_is_refused(self):
        jobs = self.gate()
        receipt = self.made(jobs)
        entry = self.tmp / "gates" / "l9" / "test.log"
        entry.write_text(entry.read_text() + "one more line\n")
        self.assertIn(f"{entry} is not what the receipt sealed", self.refused(lambda: self.facts(receipt)))

    def test_a_receipt_made_before_its_last_job_ended_is_refused(self):
        jobs = self.gate()
        receipt = self.made(jobs)
        receipt_fixture.ended(jobs[-1], time.time() + 3600)
        self.assertIn("made before its last job ended", self.refused(lambda: self.facts(receipt)))

    def test_a_receipt_whose_facts_are_not_its_jobs_is_refused(self):
        jobs = self.gate()
        for key, value in (("tree", "6" * 40), ("who", "coord-l10"), ("run", "run-1")):
            with self.subTest(key=key):
                receipt = self.tmp / f"{key}.json"
                receipt.write_text(receipt_fixture.sealed(jobs, **{key: value}))
                self.assertIn(f"the receipt's {key} is not its jobs'", self.refused(lambda: self.facts(receipt)))

    def test_a_red_gate_sealed_anyway_is_refused_on_reading(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0), ("test", "just test", 101)]})
        receipt = self.tmp / "red.json"
        receipt.write_text(receipt_fixture.sealed(jobs))
        self.assertIn("ended rc=1", self.refused(lambda: self.facts(receipt)))

    def test_what_is_not_a_receipt_is_refused(self):
        for name, text in (("text", "not json"), ("list", "[]"), ("v2", json.dumps({"receipt": 2})),
                           ("bare", json.dumps({"receipt": 1}))):
            with self.subTest(name=name):
                receipt = self.tmp / f"{name}.json"
                receipt.write_text(text)
                self.refused(lambda: self.facts(receipt))


if __name__ == "__main__":
    unittest.main(verbosity=1)
