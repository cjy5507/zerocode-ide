#!/usr/bin/env python3
"""The coordinator's gate receipt (`tools/release/coord_receipt.py`, t-47516).

A receipt stands only for what its evidence still says: every job green by its rc
file, its RC lines and its verdict, all on one tree under one owner, run and folder,
its bytes as sealed, made after the last job ended — and, as the lane and the door
read it, every job one the build line ran (in its done/, started and ended green in
its runner.log) whose own word on owner and run is the coordinator's. A recipe
counts only where an entry ran exactly `just <recipe>` in that gate's folder from the
top of the checkout.

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
from unittest import mock
from pathlib import Path

import receipt_fixture
from receipt_fixture import FIRST_JOB, HEAD, RUN, TREE

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

    def read_cli(self, receipt, *more, run=RUN):
        """The reader as the lane and the door ask it: held to the fixture's build line and RUN."""
        return subprocess.run([sys.executable, str(SCRIPT), "read", "--receipt", str(receipt),
                               "--line", str(self.tmp / "build-queue"), "--run", run, *more],
                              capture_output=True, text=True)


class Reading(ReceiptCase):
    def test_a_green_gate_reads_back_as_its_owner_run_tree_logs_and_head(self):
        jobs = self.gate()
        facts = self.facts(self.made(jobs))
        self.assertEqual(facts[:5], ["who coord-l9", f"run {RUN}", f"tree {TREE}", f"logs {self.tmp / 'gates' / 'l9'}",
                                     f"cwd {self.tmp / 'coord-gate'}"])
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

    def test_a_just_counts_only_from_the_top_of_the_checkout(self):
        # ZO's review of t-47516, P1-1: jobs that ran in zo-ide ran zo's `just test` and
        # `just doc`, never the root's — and a `cd` from there names no folder of the
        # checkout either. The same entries from the top cover the root's and zo's.
        gate = {"g1": [("test", "just test", 0), ("doc", "just doc", 0)],
                "g2": [("zo-fmt-check", "zsh -c 'cd zo-ide && just fmt-check'", 0)]}
        below = self.made(self.gate(gate, cwd=self.tmp / "coord-gate" / "zo-ide"))
        recipes = lambda receipt, folder: [line.split()[1] for line in self.facts(receipt, folder)
                                           if line.startswith("recipe ")]
        self.assertEqual(recipes(below, "."), [], "a bare just in zo-ide is not the root's recipe")
        self.assertEqual(recipes(below, "zo-ide"), [])
        top = self.made(self.gate(gate))
        self.assertEqual(recipes(top, "."), ["doc", "test"])
        self.assertEqual(recipes(top, "zo-ide"), ["fmt-check"])

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
        read = self.read_cli(receipt, "--dir", ".")
        self.assertEqual(read.returncode, 0, read.stdout + read.stderr)
        self.assertIn("recipe pii-check", read.stdout.splitlines())
        # The CLI never reads a receipt apart from a build line and a run to hold its jobs to.
        loose = subprocess.run([sys.executable, str(SCRIPT), "read", "--receipt", str(receipt)], capture_output=True, text=True)
        self.assertEqual(loose.returncode, 2, loose.stdout + loose.stderr)
        Path(str(jobs[0])[:-4] + ".rc").write_text("1\n")
        again = self.read_cli(receipt)
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

    def test_jobs_of_two_owners_runs_or_folders_are_refused(self):
        for key, value, why in (("who", "w-70906", "the jobs say w-70906, not a coordinator's coord-<label>"),
                                ("run", "run-1", "more than one run"),
                                ("cwd", self.tmp / "coord-gate" / "zo-ide", "more than one cwd")):
            with self.subTest(key=key):
                jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
                jobs += receipt_fixture.write_gate(self.tmp, "l9", {"g2": [("test", "just test", 0)]}, **{key: value})
                self.assertIn(why, self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_job_that_names_no_folder_is_refused(self):
        jobs = self.gate()
        job = Path(jobs[0])
        job.write_text("".join(line for line in job.read_text().splitlines(True) if not line.startswith("# cwd: ")))
        self.assertIn("names no owner, run or folder", self.refused(lambda: coord_receipt.make(jobs)))

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
        for key, value in (("tree", "6" * 40), ("who", "coord-l10"), ("run", "run-1"), ("cwd", "/elsewhere")):
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


SPLIT = {"g1-build": [("pii-check", "just pii-check", 0)],
         "g3a-zo-lint": [("zo-fmt-check", "zsh -c 'cd zo-ide && just fmt-check'", 0)]}


class SplitGate(ReceiptCase):
    """t-76396: a gate run as two lines (mkgate-split) has two owners, coord-<label>-root and
    coord-<label>-zo; the lane takes them as one receipt, and nothing looser."""

    def split(self, root="coord-l9-root", zo="coord-l9-zo", **kw):
        return self.gate(SPLIT, who={"g1-build": root, "g3a-zo-lint": zo}, **kw)

    def test_the_root_and_zo_lines_of_one_label_are_one_receipt(self):
        said = self.read_cli(self.made(self.split()), "--dir", ".")
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        lines = said.stdout.splitlines()
        self.assertIn("who coord-l9-root+coord-l9-zo", lines)
        self.assertIn(f"run {RUN}", lines)
        self.assertIn("recipe pii-check", lines)

    def test_each_line_must_have_ended_green_under_its_own_owner(self):
        receipt = self.made(self.split())
        runner = self.tmp / "build-queue" / "runner.log"
        name = f"0-{FIRST_JOB + 1}-coord-l9-g3a-zo-lint"
        runner.write_text(runner.read_text().replace(f"RUN {name} who=coord-l9-zo", f"RUN {name} who=coord-l9-root"))
        said = self.read_cli(receipt)
        self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
        self.assertIn(f"runner.log never ended job {name} green under coord-l9-zo", said.stdout)

    def test_owners_other_than_the_two_lines_of_one_label_are_refused(self):
        for case, root, zo in (("two labels", "coord-l9-root", "coord-l10-zo"),
                               ("a plain owner and a line", "coord-l9", "coord-l9-zo"),
                               ("the same line twice over two names", "coord-l9-zo", "coord-l9-zo+"),
                               ("a third kind of line", "coord-l9-root", "coord-l9-web"),
                               ("not a coordinator", "coord-l9-root", "w-70906"),
                               ("one line named twice with another label", "coord-l9-root", "coord-l9-root-zo")):
            with self.subTest(case=case):
                with self.assertRaises(coord_receipt.Refused):
                    coord_receipt.make(self.split(root, zo))
                shutil.rmtree(self.tmp / "build-queue")

    def test_a_single_line_owner_still_stands_alone_but_must_be_a_coordinator(self):
        said = self.read_cli(self.made(self.gate(SPLIT, who="coord-l9-zo")))
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)

    def test_make_refuses_owners_that_are_not_coordinators_even_when_every_job_names_the_same_one(self):
        """t-76396 r3: one non-coordinator owner skipped the pair check, so make sealed a receipt only read refused."""
        for case, who in (("every job one worker", {"g1-build": "w-70906", "g3a-zo-lint": "w-70906"}),
                          ("a plain worker for all", "w-70906")):
            with self.subTest(case=case):
                self.assertIn("coordinator", self.refused(lambda: coord_receipt.make(self.gate(SPLIT, who=who))))
                shutil.rmtree(self.tmp / "build-queue")

    def test_read_refuses_a_receipt_a_worker_owned_that_an_older_make_sealed(self):
        """t-76396 r4: make now refuses these, so the reader's own refusal is pinned on a receipt built by hand."""
        jobs = self.gate(SPLIT, who="w-70906")
        receipt = self.tmp / "receipt.json"
        receipt.write_text(receipt_fixture.sealed(jobs, who="w-70906"))
        said = self.read_cli(receipt)
        self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
        self.assertIn("the jobs say w-70906, not a coordinator's coord-<label>", said.stdout)

    def test_a_mixed_non_coordinator_owner_is_refused_before_any_pair_check(self):
        """t-76396 r4: each owner is a coordinator's name before the owners are counted or paired."""
        for case, root, zo in (("worker beside a line", "coord-l9-root", "w-70906"),
                               ("worker beside a plain coordinator", "coord-l9", "w-70906")):
            with self.subTest(case=case):
                with mock.patch.object(coord_receipt, "split_gate", side_effect=AssertionError("pair check reached")):
                    said = self.refused(lambda: coord_receipt.make(self.split(root, zo)))
                self.assertIn("the jobs say w-70906, not a coordinator's coord-<label>", said)
                shutil.rmtree(self.tmp / "build-queue")

    def test_a_plus_in_one_jobs_owner_is_refused_so_no_bundle_skips_the_pair_check(self):
        """t-76396 r2: an owner that is itself coord-a+coord-b made the joined name look like two owners."""
        for case, root, zo in (("a root-and-zo bundle on both", "coord-l9-root+coord-l10-zo", "coord-l9-root+coord-l10-zo"),
                               ("a three-label bundle on both", "coord-l9+coord-l10+coord-l11", "coord-l9+coord-l10+coord-l11"),
                               ("a bundle beside a plain line", "coord-l9-root+coord-l10-zo", "coord-l9-zo")):
            with self.subTest(case=case):
                self.assertIn("+", self.refused(lambda: coord_receipt.make(self.split(root, zo))))
                shutil.rmtree(self.tmp / "build-queue")
        for case, who in (("one job", "coord-l9-root+coord-l10-zo"), ("every job", "coord-l9+coord-l10+coord-l11")):
            with self.subTest(case=case):
                self.refused(lambda: coord_receipt.make(self.gate(SPLIT, who=who)))
                shutil.rmtree(self.tmp / "build-queue")

    def test_the_two_lines_of_one_label_and_one_plain_owner_still_make_a_receipt(self):
        self.assertEqual(coord_receipt.make(self.split())["who"], "coord-l9-root+coord-l9-zo")
        shutil.rmtree(self.tmp / "build-queue")
        self.assertEqual(coord_receipt.make(self.gate(SPLIT, who="coord-l9"))["who"], "coord-l9")

    def test_the_run_is_still_the_lanes_for_both_lines(self):
        said = self.read_cli(self.made(self.split(run="run-11955")))
        self.assertIn("the jobs say run run-11955, not run-4275", said.stdout)


class OnTheLine(ReceiptCase):
    """ZO's review of t-47516, P2-1: the receipt's evidence is only words the jobs wrote, so
    the reader holds every job to the build line that ran it and to the coordinator's run.
    None of it stops one who writes the line's files too; it stops a job copied by hand,
    one the runner refused or never ran, and one of another run or owner."""

    def test_the_lines_own_jobs_stand(self):
        said = self.read_cli(self.made(self.gate()))
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        self.assertIn("who coord-l9", said.stdout.splitlines())

    def test_jobs_outside_the_lines_done_folder_are_refused(self):
        jobs = self.gate()
        elsewhere = self.tmp / "elsewhere"
        elsewhere.mkdir()
        for path in jobs:
            for kind in ("job", "rc", "log"):
                shutil.copy2(str(path)[:-len(".job")] + "." + kind, elsewhere)
        said = self.read_cli(self.made([elsewhere / path.name for path in jobs]))
        self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
        self.assertIn(f"is not in {(self.tmp / 'build-queue' / 'done').resolve()}", said.stdout)

    def test_a_job_the_runner_did_not_end_green_under_its_owner_is_refused(self):
        receipt = self.made(self.gate())
        runner = self.tmp / "build-queue" / "runner.log"
        lines = runner.read_text().splitlines(True)
        name = f"0-{FIRST_JOB + 1}-coord-l9-g2-tests"
        for case, kept in (
                ("never ran", [line for line in lines if f" {name} " not in line]),
                ("ran red last", lines + [f"RUN {name} who=coord-l9 lane=2 cap=1800s 05:00:00\n",
                                          f"DONE {name} rc=1 9s lane=2 05:00:09 load=4\n"]),
                ("ran under another owner", [line.replace("who=coord-l9", "who=w-70906") if line.startswith(f"RUN {name} ")
                                             else line for line in lines])):
            with self.subTest(case=case):
                runner.write_text("".join(kept))
                said = self.read_cli(receipt)
                self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
                self.assertIn(f"runner.log never ended job {name} green under coord-l9", said.stdout)
        runner.unlink()
        self.assertIn("runner.log is missing", self.read_cli(receipt).stdout)

    def test_jobs_that_say_another_run_or_no_coordinator_are_refused(self):
        said = self.read_cli(self.made(self.gate(run="run-11955")))
        self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
        self.assertIn("the jobs say run run-11955, not run-4275", said.stdout)
        self.assertIn("not a coordinator's coord-<label>", self.refused(lambda: coord_receipt.make(self.gate(who="w-70906"))))
        shutil.rmtree(self.tmp / "build-queue")
        said = self.read_cli(self.made(self.gate()), run="run-1")
        self.assertIn("the jobs say run run-4275, not run-1", said.stdout, "the run is the lane's, not the receipt's")


if __name__ == "__main__":
    unittest.main(verbosity=1)
