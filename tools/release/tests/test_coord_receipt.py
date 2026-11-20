#!/usr/bin/env python3
"""The coordinator's gate receipt (`tools/release/coord_receipt.py`, t-47516, t-119757).

A receipt stands only for what its evidence still says: every `zo line` job green by its
record, its rc file, its RC lines and its verdict, its command the checkout's own gate
script, its log's owner and folder the ones its record names, all on one tree under one
owner, run, logs folder and checkout, its bytes as sealed, made after the last job ended —
and, as the lane and the door read it, every job one the build line ran (its record in the
line's jobs/) whose own word on owner and run is the coordinator's. A recipe counts only
where an entry ran exactly `just <recipe>` in that gate's folder from the top of the
checkout, the one place the gate script runs (a log without its head and tree is refused):
the checkout itself is never read, so one removed before the receipt was made or after
changes nothing. A receipt of the old build queue (version 1) is refused with its reason.

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
from receipt_fixture import HEAD, RUN, TREE, job_id, log_of, rc_of, rewrite

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

    def careless(self, jobs, **facts):
        """A receipt sealed over whatever JOBS' files hold now, read by no one first."""
        path = self.tmp / "careless.json"
        path.write_text(receipt_fixture.sealed(jobs, **facts))
        return path

    def refused(self, call):
        with self.assertRaises(coord_receipt.Refused) as caught:
            call()
        return str(caught.exception)

    def facts(self, receipt, folder=None):
        return coord_receipt.read(receipt, folder)

    def stands(self, jobs):
        """The same material, untouched, makes a receipt that reads back: a refusal below is its rule's."""
        self.assertTrue(self.facts(self.made(jobs)))

    def read_cli(self, receipt, *more, run=RUN):
        """The reader as the lane and the door ask it: held to the fixture's build line and RUN."""
        return subprocess.run([sys.executable, str(SCRIPT), "read", "--receipt", str(receipt),
                               "--line", str(receipt_fixture.line_of(self.tmp)), "--run", run, *more],
                              capture_output=True, text=True)

    def edit_log(self, path, change):
        log = log_of(path)
        log.write_text(change(log.read_text()))


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

    def test_a_job_run_below_its_checkouts_top_is_refused_and_from_the_top_a_bare_just_is_the_roots(self):
        # ZO's review of t-47516, P1-1: jobs that ran in zo-ide ran zo's `just test` and `just doc`,
        # never the root's. The gate script runs only at the top: anywhere else it stops before its
        # head and tree, so such a job's log is refused even with its record made green by hand — the
        # reader never looks at the checkout (the review of t-119757's fix, R1).
        gate = {"g1": [("test", "just test", 0), ("doc", "just doc", 0)],
                "g2": [("zo-fmt-check", "zsh -c 'cd zo-ide && just fmt-check'", 0)]}
        below = self.gate(gate, cwd=self.tmp / "coord-gate" / "zo-ide")
        for path in below:
            receipt_fixture.stopped_below_the_top(path)
        self.assertIn("does not open with its head and tree", self.refused(lambda: coord_receipt.make(below)))
        top = self.made(self.gate(gate))
        recipes = lambda folder: [line.split()[1] for line in self.facts(top, folder) if line.startswith("recipe ")]
        self.assertEqual(recipes("."), ["doc", "test"])
        self.assertEqual(recipes("zo-ide"), ["fmt-check"])

    def test_each_head_is_said_with_the_first_end_of_its_jobs(self):
        one = self.gate({"g1": [("pii-check", "just pii-check", 0)]}, head="3" * 40)
        two = receipt_fixture.write_gate(self.tmp, "l9", {"g2": [("test", "just test", 0)]}, head="4" * 40)
        receipt_fixture.ended(one[0], 1792610000)
        receipt_fixture.ended(two[0], 1792610500)
        heads = [line for line in self.facts(self.made(one + two)) if line.startswith("head ")]
        self.assertEqual(heads, [f"head {'3' * 40} 1792610000", f"head {'4' * 40} 1792610500"])

    def test_the_lines_own_words_above_the_script_and_after_its_verdict_change_nothing(self):
        # zo line writes its slot's build folder above the command's output, and a notice it
        # could not send under it, after the job ended: neither is the script's word.
        jobs = self.gate()
        self.edit_log(jobs[0], lambda text: "zo line: a new waiter took the job\n" + text
                      + "zo line: telling run:run-1001 how the job ended did not go: exit 1 — no run\n")
        self.assertIn(f"run {RUN}", self.facts(self.made(jobs)))

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
        rc_of(jobs[0]).write_text("1\n")
        again = self.read_cli(receipt)
        self.assertEqual(again.returncode, 1)
        self.assertEqual(len(again.stdout.splitlines()), 1, again.stdout)
        self.assertIn("ended rc=1", again.stdout)


class Refusing(ReceiptCase):
    def test_a_red_job_is_refused(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0), ("test", "just test", 101)]})
        self.assertIn(f"job {job_id('l9', 'g1')} ended failed", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_job_the_line_capped_stopped_or_lost_is_refused(self):
        for state in ("capped", "stopped", "lost", "withdrawn", "running", "waiting"):
            with self.subTest(state=state):
                jobs = self.gate(state={"g2-tests": state})
                self.assertIn(f"job {job_id('l9', 'g2-tests', 1)} ended {state}", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_record_or_rc_file_that_says_another_code_is_refused(self):
        jobs = self.gate()
        self.stands(jobs)
        rewrite(jobs[1], rc=1)
        self.assertIn(f"job {job_id('l9', 'g2-tests', 1)} ended rc=1", self.refused(lambda: coord_receipt.make(jobs)))
        rewrite(jobs[1], rc=False)   # JSON's false is no 0
        self.assertIn("ended rc=False", self.refused(lambda: coord_receipt.make(jobs)))
        rewrite(jobs[1], rc=0)
        for text, said in (("1\n", "rc=1"), ("", "rc=nothing")):
            rc_of(jobs[1]).write_text(text)
            self.assertIn(f"job {job_id('l9', 'g2-tests', 1)} ended {said}", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_red_entry_is_refused_under_a_green_record_and_rc_file(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0), ("test", "just test", 101)]}, rc={"g1": 0})
        self.assertIn("entry test rc=101", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_record_that_is_no_zo_line_jobs_is_refused(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
        self.stands(jobs)
        record = jobs[0]
        for name, change in (("another id", lambda: rewrite(record, id="j-1-other")),
                             ("not json", lambda: record.write_text("not json\n")),
                             ("a list", lambda: record.write_text("[]\n"))):
            with self.subTest(change=name):
                saved = record.read_text()
                change()
                self.assertIn(f"{record} is not a zo line job's record", self.refused(lambda: coord_receipt.make(jobs)))
                record.write_text(saved)
        odd = record.with_name("j-1 2.json")
        shutil.copy2(record, odd)
        self.assertIn("is not a zo line job's record", self.refused(lambda: coord_receipt.make([odd])))
        self.assertIn("is not a zo line job's record", self.refused(lambda: coord_receipt.make([log_of(record)])))

    def test_a_record_without_an_owner_checkout_start_or_end_is_refused(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
        for fields, said in (({"owner": ""}, "names no owner or checkout"), ({"checkout": None}, "names no owner or checkout"),
                             ({"started_ms": None}, "has no start and end"), ({"ended_ms": "now"}, "has no start and end"),
                             ({"ended_ms": 1}, "has no start and end")):
            with self.subTest(fields=fields):
                saved = jobs[0].read_text()
                rewrite(jobs[0], **fields)
                self.assertIn(said, self.refused(lambda: coord_receipt.make(jobs)))
                jobs[0].write_text(saved)

    def test_a_job_that_ran_anything_but_the_checkouts_gate_script_is_refused(self):
        # The entries are the gate script's own lines: the tree's coord-gate.sh, run from the
        # checkout it gates, is what prints exactly what it ran (t-119757).
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
        for command in (["bash", "/elsewhere/combined-168.sh"], ["env", "X=1", "bash", "tools/release/coord-gate.sh"],
                        ["bash", "./tools/release/coord-gate.sh"], ["bash -c 'tools/release/coord-gate.sh'"], "bash"):
            with self.subTest(command=command):
                rewrite(jobs[0], command=command)
                said = self.refused(lambda: coord_receipt.make(jobs))
                self.assertIn(f"job {job_id('l9', 'g1')} ran ", said)
                self.assertIn(", not bash tools/release/coord-gate.sh", said)

    def test_a_job_put_in_as_the_gate_scripts_inner_step_is_refused(self):
        # ZO's review of t-119757, F2: `--in-lane-env` skips lane-env.sh and run-current, and takes who, run and
        # logs from whoever called it — its recipes ran in a pane's environment, not the lane's.
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
        self.stands(jobs)
        logs = str(self.tmp / "gates" / "l9")
        rewrite(jobs[0], command=["bash", "tools/release/coord-gate.sh", "--in-lane-env", "coord-l9", RUN, logs, "-", "root"])
        self.assertIn(f"job {job_id('l9', 'g1')} ran coord-gate.sh --in-lane-env, its inner step past lane-env.sh and run-current",
                      self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_job_whose_command_names_another_owner_is_refused(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
        logs = str(self.tmp / "gates" / "l9")
        for command, said in ((["--who", "coord-l10", "--logs", logs, "root"], "--who coord-l10, its record's owner coord-l9"),
                              (["--logs", logs, "root"], "--who None, its record's owner coord-l9")):
            with self.subTest(command=command):
                rewrite(jobs[0], command=["bash", "tools/release/coord-gate.sh", *command])
                self.assertIn(f"job {job_id('l9', 'g1')} ran coord-gate.sh {said}", self.refused(lambda: coord_receipt.make(jobs)))
        rewrite(jobs[0], command=["bash", "tools/release/coord-gate.sh", "--who", "coord-l9", "--who", "coord-l9", "--logs", logs, "root"])
        self.assertIn("command gives --who twice or no value", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_job_whose_command_names_another_logs_folder_is_refused(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
        logs = self.tmp / "gates" / "l9"
        rewrite(jobs[0], command=["bash", "tools/release/coord-gate.sh", "--who", "coord-l9", "--logs", "/elsewhere", "root"])
        self.assertIn(f"job {job_id('l9', 'g1')} ran coord-gate.sh --logs /elsewhere, its log says # logs: {logs}",
                      self.refused(lambda: coord_receipt.make(jobs)))
        rewrite(jobs[0], command=["bash", "tools/release/coord-gate.sh", "--who", "coord-l9", "root"])
        self.assertIn("ran coord-gate.sh --logs None", self.refused(lambda: coord_receipt.make(jobs)))
        try:
            (self.tmp / "logs-link").symlink_to(logs, target_is_directory=True)
        except OSError:
            self.skipTest("this system makes no folder link here")
        rewrite(jobs[0], command=["bash", "tools/release/coord-gate.sh", "--who", "coord-l9", "--logs", str(self.tmp / "logs-link"), "root"])
        self.assertTrue(self.facts(self.made(jobs)), "the same folder named through a link")

    def test_a_log_whose_owner_or_folder_is_not_its_records_is_refused(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
        self.stands(jobs)
        self.edit_log(jobs[0], lambda text: text.replace("# who: coord-l9\n", "# who: coord-l10\n"))
        self.assertIn(f"job {job_id('l9', 'g1')}'s log says who coord-l10, its record coord-l9",
                      self.refused(lambda: coord_receipt.make(jobs)))
        self.edit_log(jobs[0], lambda text: text.replace("# who: coord-l10\n", "# who: coord-l9\n"))
        (self.tmp / "stack").mkdir()
        rewrite(jobs[0], checkout=str(self.tmp / "stack"))   # 168's job: put in from main, the gate run in another tree
        self.assertIn(f"job {job_id('l9', 'g1')}'s log says cwd {self.tmp / 'coord-gate'}, its record ran it in {self.tmp / 'stack'}",
                      self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_receipt_still_counts_its_recipes_after_its_checkout_is_removed(self):
        # ZO's review of t-119757, F1: the stack is cleaned after landing, maybe before the lane reads the receipt.
        receipt = self.made(self.gate())
        recipes = lambda: [line.split()[1] for line in self.facts(receipt, ".") if line.startswith("recipe ")]
        self.assertEqual(recipes(), ["pii-check", "test", "tools-test", "win-check-if-available"])
        shutil.rmtree(self.tmp / "coord-gate")
        self.assertEqual(recipes(), ["pii-check", "test", "tools-test", "win-check-if-available"], "the jobs' logs say they ran at the top")
        said = self.read_cli(receipt, "--dir", ".")
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        self.assertIn("recipe pii-check", said.stdout.splitlines())

    def test_a_receipt_made_after_its_checkout_was_removed_still_counts_its_recipes(self):
        # The review of t-119757's fix, R1: the stack is cleaned as soon as it lands — before the release
        # commit, so before make. Its jobs ran at the top by their own logs; the checkout is not read again.
        jobs = self.gate()
        shutil.rmtree(self.tmp / "coord-gate")
        receipt = self.made(jobs)
        self.assertNotIn("top", json.loads(receipt.read_text()), "the receipt says nothing of the checkout")
        self.assertEqual([line.split()[1] for line in self.facts(receipt, ".") if line.startswith("recipe ")],
                         ["pii-check", "test", "tools-test", "win-check-if-available"])
        said = self.read_cli(receipt, "--dir", "zo-ide")
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        self.assertEqual([line for line in said.stdout.splitlines() if line.startswith("recipe ")], ["recipe clippy", "recipe fmt-check"])

    def test_neither_the_checkout_nor_a_top_written_into_the_receipt_is_read(self):
        # The review of t-119757's fix, I-A: a receipt's own word on where its jobs ran is no evidence, nor is
        # what stands at the checkout's path now — the jobs' logs say where they ran.
        jobs = self.gate()
        every = ["pii-check", "test", "tools-test", "win-check-if-available"]
        recipes = lambda receipt: [line.split()[1] for line in self.facts(receipt, ".") if line.startswith("recipe ")]
        for top in (False, True, None, "yes"):
            with self.subTest(top=top):
                self.assertEqual(recipes(self.careless(jobs, top=top)), every)
        (self.tmp / "coord-gate" / ".git").rmdir()   # the path holds no checkout's top any more
        self.assertEqual(recipes(self.made(jobs)), every)

    def test_a_folder_named_through_a_link_is_the_same_folder(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0)]})
        try:
            (self.tmp / "linked").symlink_to(self.tmp / "coord-gate", target_is_directory=True)
        except OSError:
            self.skipTest("this system makes no folder link here")
        rewrite(jobs[0], checkout=str(self.tmp / "linked"))
        self.assertIn(f"cwd {self.tmp / 'linked'}", self.facts(self.made(jobs)))

    def test_an_entry_that_never_closed_is_refused(self):
        jobs = self.gate()
        self.edit_log(jobs[1], lambda text: "".join(line for line in text.splitlines(True) if not line.startswith("RC test=")))
        self.assertIn("entries and RC lines differ at test", self.refused(lambda: coord_receipt.make(jobs)))

    def test_an_entry_closed_twice_or_named_twice_is_refused(self):
        jobs = self.gate({"g1": [("test", "just test", 0)]})
        self.stands(jobs)
        self.edit_log(jobs[0], lambda text: text.replace("RC test=0 (3s)\n", "RC test=0 (3s)\nRC test=0 (3s)\n"))
        self.assertIn("closed entry test twice", self.refused(lambda: coord_receipt.make(jobs)))
        self.edit_log(jobs[0], lambda text: text.replace("RC test=0 (3s)\nRC test=0 (3s)\n", "RC test=0 (3s)\n")
                      .replace("entry test just test\n", "entry test just test\nentry test just doc\n"))
        self.assertIn("has an entry without a command, or one key twice", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_log_with_no_entry_is_refused(self):
        jobs = self.gate({"g1": [("test", "just test", 0)]})
        self.edit_log(jobs[0], lambda text: text.replace("entry test just test\n", "").replace("RC test=0 (3s)\n", ""))
        self.assertIn(f"job {job_id('l9', 'g1')} ran no entry", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_verdict_that_counted_red_or_is_not_one_is_refused(self):
        jobs = self.gate()
        self.edit_log(jobs[0], lambda text: text.replace("verdict: bad=0", "verdict: bad=1"))
        self.assertIn("verdict is not bad=0", self.refused(lambda: coord_receipt.make(jobs)))
        self.edit_log(jobs[0], lambda text: text.replace("verdict: bad=1", "verdict: bad=0") + "verdict: bad=0 (again)\n")
        self.assertIn("verdict is not bad=0", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_log_that_does_not_open_with_its_head_and_tree_is_refused(self):
        jobs = self.gate()
        self.stands(jobs)
        # The script said who it was and stopped on the tree: the line under its head lines is no head.
        self.edit_log(jobs[2], lambda text: text.replace(f"head {HEAD}", f"STOP tree\nhead {HEAD}"))
        self.assertIn("does not open with its head and tree", self.refused(lambda: coord_receipt.make(jobs)))
        # …and a line of the line's own below the script's first one is the script's text, not the line's.
        self.edit_log(jobs[2], lambda text: text.replace("STOP tree\n", "zo line: cargo builds in a slot\n"))
        self.assertIn("does not open with its head and tree", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_log_that_does_not_open_with_the_scripts_head_lines_is_refused(self):
        jobs = self.gate()
        self.edit_log(jobs[2], lambda text: "STOP tree\n" + text)
        self.assertIn("names no owner, run, folder or logs folder", self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_head_line_said_twice_is_refused(self):
        jobs = self.gate()
        self.edit_log(jobs[0], lambda text: text.replace(f"# run: {RUN}\n", f"# run: {RUN}\n# run: run-1\n"))
        self.assertIn("log says # run: twice", self.refused(lambda: coord_receipt.make(jobs)))

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

    def test_two_jobs_that_ran_one_entry_are_refused(self):
        # One logs folder for every job: the second job's test.log stands where the first one's was.
        jobs = self.gate({"g1": [("test", "just test", 0)], "g2": [("test", "just test", 0)]})
        self.assertIn(f"jobs {job_id('l9', 'g1')} and {job_id('l9', 'g2', 1)} both ran entry test",
                      self.refused(lambda: coord_receipt.make(jobs)))

    def test_a_job_that_names_no_folder_is_refused(self):
        jobs = self.gate()
        for head in ("# cwd: ", "# logs: ", "# run: ", "# who: "):
            with self.subTest(head=head):
                saved = log_of(jobs[0]).read_text()
                self.edit_log(jobs[0], lambda text: "".join(line for line in text.splitlines(True) if not line.startswith(head)))
                self.assertIn("names no owner, run, folder or logs folder", self.refused(lambda: coord_receipt.make(jobs)))
                log_of(jobs[0]).write_text(saved)

    def test_a_missing_entry_log_or_job_file_is_refused(self):
        jobs = self.gate()
        (self.tmp / "gates" / "l9" / "tools-test.log").unlink()
        self.assertIn("tools-test.log is missing", self.refused(lambda: coord_receipt.make(jobs)))
        self.assertIn("is missing", self.refused(lambda: coord_receipt.make([self.tmp / "line" / "jobs" / "j-1-none.json"])))
        log_of(jobs[0]).unlink()
        self.assertIn(f"{log_of(jobs[0])} is missing", self.refused(lambda: coord_receipt.make(jobs[:1])))

    def test_a_linked_file_is_refused(self):
        jobs = self.gate()
        rc = rc_of(jobs[0])
        (self.tmp / "zero").write_text("0\n")
        rc.unlink()
        rc.symlink_to(self.tmp / "zero")
        self.assertIn("is not a plain file", self.refused(lambda: coord_receipt.make(jobs)))

    def test_evidence_changed_after_the_receipt_was_made_is_refused(self):
        for name, path in (("an entry's log", lambda jobs: self.tmp / "gates" / "l9" / "test.log"),
                           ("the job's log", lambda jobs: log_of(jobs[1])), ("its record", lambda jobs: jobs[1])):
            with self.subTest(changed=name):
                jobs = self.gate()
                receipt = self.made(jobs)
                changed = path(jobs)
                changed.write_text(changed.read_text() + "\n")
                self.assertIn(f"{changed} is not what the receipt sealed", self.refused(lambda: self.facts(receipt)))
                shutil.rmtree(self.tmp / "line")

    def test_a_notice_the_line_added_after_the_receipt_was_made_is_refused(self):
        # A notice zo line could not send lands at the log's end after the job's end: made before it, the seal breaks.
        jobs = self.gate()
        receipt = self.made(jobs)
        self.edit_log(jobs[0], lambda text: text + "zo line: telling run:run-1001 how the job ended did not go: exit 1\n")
        self.assertIn(f"{log_of(jobs[0])} is not what the receipt sealed", self.refused(lambda: self.facts(receipt)))

    def test_a_receipt_made_before_its_last_job_ended_is_refused(self):
        jobs = self.gate()
        receipt_fixture.ended(jobs[-1], time.time() + 3600)
        self.assertIn("made before its last job ended", self.refused(lambda: self.facts(self.careless(jobs))))

    def test_a_receipt_whose_facts_are_not_its_jobs_is_refused(self):
        jobs = self.gate()
        for key, value in (("tree", "6" * 40), ("who", "coord-l10"), ("run", "run-1"), ("cwd", "/elsewhere"), ("logs", "/elsewhere")):
            with self.subTest(key=key):
                receipt = self.careless(jobs, **{key: value})
                self.assertIn(f"the receipt's {key} is not its jobs'", self.refused(lambda: self.facts(receipt)))

    def test_a_red_gate_sealed_anyway_is_refused_on_reading(self):
        jobs = self.gate({"g1": [("pii-check", "just pii-check", 0), ("test", "just test", 101)]})
        self.assertIn(f"job {job_id('l9', 'g1')} ended failed", self.refused(lambda: self.facts(self.careless(jobs))))

    def test_what_is_not_a_receipt_is_refused(self):
        for name, text, said in (("text", "not json", "the receipt is not JSON"),
                                 ("list", "[]", "the receipt is not a version 2 receipt"),
                                 ("v3", json.dumps({"receipt": 3}), "the receipt is not a version 2 receipt"),
                                 ("bare", json.dumps({"receipt": 2}), "the receipt has no jobs, seal or time"),
                                 ("a time that is no number", json.dumps({"receipt": 2, "jobs": [], "sealed": {}, "made": True}),
                                  "the receipt has no jobs, seal or time"),
                                 ("no job", json.dumps({"receipt": 2, "jobs": [], "sealed": {}, "made": 1}),
                                  "the receipt names no job")):
            with self.subTest(name=name):
                receipt = self.tmp / f"{name}.json"
                receipt.write_text(text)
                self.assertIn(said, self.refused(lambda: self.facts(receipt)))

    def test_a_receipt_of_the_old_build_queue_is_refused_with_its_reason(self):
        receipt = self.tmp / "v1.json"
        receipt.write_text(json.dumps({"receipt": 1, "who": "coord-l64", "run": "run-1001", "tree": TREE,
                                       "jobs": ["/build-queue/done/0-1792610090-coord-l64-g1"], "sealed": {}, "made": 1}))
        said = self.read_cli(receipt)
        self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
        self.assertIn("the receipt is a version 1 receipt of the old build queue, which has run nothing since 2026-11-09",
                      said.stdout)


SPLIT = {"g1-build": [("pii-check", "just pii-check", 0)],
         "g3a-zo-lint": [("zo-fmt-check", "zsh -c 'cd zo-ide && just fmt-check'", 0)]}


class SplitGate(ReceiptCase):
    """t-76396: a gate run as two jobs has two owners, coord-<label>-root and coord-<label>-zo;
    the lane takes them as one receipt, and nothing looser — on the zo line as on the old queue."""

    def split(self, root="coord-l9-root", zo="coord-l9-zo", **kw):
        return self.gate(SPLIT, who={"g1-build": root, "g3a-zo-lint": zo}, **kw)

    def drop_line(self):
        shutil.rmtree(receipt_fixture.line_of(self.tmp))

    def test_the_root_and_zo_jobs_of_one_label_are_one_receipt(self):
        said = self.read_cli(self.made(self.split()), "--dir", ".")
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        lines = said.stdout.splitlines()
        self.assertIn("who coord-l9-root+coord-l9-zo", lines)
        self.assertIn(f"run {RUN}", lines)
        self.assertIn("recipe pii-check", lines)

    def test_each_job_must_say_the_owner_its_record_names(self):
        jobs = self.split()
        rewrite(jobs[1], owner="coord-l9-root")   # put in under the root's name, its script saying zo's
        said = self.read_cli(self.careless(jobs, who="coord-l9-root"))
        self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
        self.assertIn(f"job {job_id('l9', 'g3a-zo-lint', 1)} ran coord-gate.sh --who coord-l9-zo, its record's owner coord-l9-root",
                      said.stdout)

    def test_owners_other_than_the_two_jobs_of_one_label_are_refused(self):
        for case, root, zo, why in (("two labels", "coord-l9-root", "coord-l10-zo", "more than one who"),
                                    ("a plain owner and a line", "coord-l9", "coord-l9-zo", "more than one who"),
                                    ("the same line twice over two names", "coord-l9-zo", "coord-l9-zo+", "joins owners with +"),
                                    ("a third kind of line", "coord-l9-root", "coord-l9-web", "not a coordinator's"),
                                    ("not a coordinator", "coord-l9-root", "w-70906", "not a coordinator's"),
                                    ("one line named twice with another label", "coord-l9-root", "coord-l9-root-zo",
                                     "not a coordinator's")):
            with self.subTest(case=case):
                self.assertIn(why, self.refused(lambda: coord_receipt.make(self.split(root, zo))))
                self.drop_line()

    def test_a_single_job_owner_still_stands_alone_but_must_be_a_coordinator(self):
        said = self.read_cli(self.made(self.gate(SPLIT, who="coord-l9-zo")))
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)

    def test_make_refuses_owners_that_are_not_coordinators_even_when_every_job_names_the_same_one(self):
        """t-76396 r3: one non-coordinator owner skipped the pair check, so make sealed a receipt only read refused."""
        for case, who in (("every job one worker", {"g1-build": "w-70906", "g3a-zo-lint": "w-70906"}),
                          ("a plain worker for all", "w-70906"), ("168's own owner", "release-1168")):
            with self.subTest(case=case):
                self.assertIn("coordinator", self.refused(lambda: coord_receipt.make(self.gate(SPLIT, who=who))))
                self.drop_line()

    def test_read_refuses_a_receipt_a_worker_owned_that_an_older_make_sealed(self):
        """t-76396 r4: make refuses these, so the reader's own refusal is pinned on a receipt built by hand."""
        jobs = self.gate(SPLIT, who="w-70906")
        said = self.read_cli(self.careless(jobs, who="w-70906"))
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
                self.drop_line()

    def test_a_plus_in_one_jobs_owner_is_refused_so_no_bundle_skips_the_pair_check(self):
        """t-76396 r2: an owner that is itself coord-a+coord-b made the joined name look like two owners."""
        for case, root, zo in (("a root-and-zo bundle on both", "coord-l9-root+coord-l10-zo", "coord-l9-root+coord-l10-zo"),
                               ("a three-label bundle on both", "coord-l9+coord-l10+coord-l11", "coord-l9+coord-l10+coord-l11"),
                               ("a bundle beside a plain line", "coord-l9-root+coord-l10-zo", "coord-l9-zo")):
            with self.subTest(case=case):
                self.assertIn("+", self.refused(lambda: coord_receipt.make(self.split(root, zo))))
                self.drop_line()
        for case, who in (("one job", "coord-l9-root+coord-l10-zo"), ("every job", "coord-l9+coord-l10+coord-l11")):
            with self.subTest(case=case):
                self.assertIn("joins owners with +", self.refused(lambda: coord_receipt.make(self.gate(SPLIT, who=who))))
                self.drop_line()

    def test_the_two_jobs_of_one_label_and_one_plain_owner_still_make_a_receipt(self):
        self.assertEqual(coord_receipt.make(self.split())["who"], "coord-l9-root+coord-l9-zo")
        self.drop_line()
        self.assertEqual(coord_receipt.make(self.gate(SPLIT, who="coord-l9"))["who"], "coord-l9")

    def test_the_run_is_still_the_lanes_for_both_jobs(self):
        said = self.read_cli(self.made(self.split(run="run-2002")))
        self.assertIn("the jobs say run run-2002, not run-1001", said.stdout)


class OnTheLine(ReceiptCase):
    """ZO's review of t-47516, P2-1: the receipt's evidence is only words the jobs wrote, so
    the reader holds every job to the build line that ran it and to the coordinator's run.
    None of it stops one who writes the line's files too; it stops a job copied by hand, one
    the line did not end green, and one of another run or owner."""

    def test_the_lines_own_jobs_stand(self):
        said = self.read_cli(self.made(self.gate()))
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        self.assertIn("who coord-l9", said.stdout.splitlines())

    def test_jobs_outside_the_lines_folder_are_refused(self):
        jobs = self.gate()
        elsewhere = self.tmp / "elsewhere"
        for folder in ("jobs", "logs"):
            (elsewhere / folder).mkdir(parents=True)
        for path in jobs:
            shutil.copy2(path, elsewhere / "jobs")
            for kind in (log_of(path), rc_of(path)):
                shutil.copy2(kind, elsewhere / "logs")
        said = self.read_cli(self.made([elsewhere / "jobs" / path.name for path in jobs]))
        self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
        self.assertIn(f"job {job_id('l9', 'g1-build')} is not in {(receipt_fixture.line_of(self.tmp) / 'jobs').resolve()}",
                      said.stdout)

    def test_a_job_the_line_did_not_end_green_is_refused(self):
        # What runner.log said of a job on the old queue, the zo line's record says itself.
        for case, fields, said in (("never ran", {"state": "waiting", "started_ms": None, "ended_ms": None}, "ended waiting"),
                                   ("ran red", {"state": "failed", "rc": 1}, "ended failed"),
                                   ("ran past its cap", {"state": "capped", "rc": 1}, "ended capped")):
            with self.subTest(case=case):
                jobs = self.gate()
                rewrite(jobs[1], **fields)
                answer = self.read_cli(self.careless(jobs))
                self.assertEqual(answer.returncode, 1, answer.stdout + answer.stderr)
                self.assertIn(f"job {job_id('l9', 'g2-tests', 1)} {said}", answer.stdout)
                shutil.rmtree(self.tmp / "line")

    def test_jobs_that_say_another_run_or_no_coordinator_are_refused(self):
        said = self.read_cli(self.made(self.gate(run="run-2002")))
        self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
        self.assertIn("the jobs say run run-2002, not run-1001", said.stdout)
        self.assertIn("not a coordinator's coord-<label>", self.refused(lambda: coord_receipt.make(self.gate(who="w-70906"))))
        shutil.rmtree(self.tmp / "line")
        said = self.read_cli(self.made(self.gate()), run="run-1")
        self.assertIn("the jobs say run run-1001, not run-1", said.stdout, "the run is the lane's, not the receipt's")

    def test_a_lane_given_no_run_takes_no_receipt(self):
        said = self.read_cli(self.made(self.gate()), run="")
        self.assertEqual(said.returncode, 1, said.stdout + said.stderr)
        self.assertIn("the lane was given no coordinator run (RELEASE_COORD_RUN)", said.stdout)


RELEASE = SCRIPT.parent
GATE_TOOLS = ("coord-gate.sh", "lane-env.sh", "lane.sh", "verify-every-recipe.sh")
FAKE_JUST = """#!/bin/sh
# just, as the gate script calls it: each call said, red for a recipe red.txt names.
echo "$(pwd -P) $*" >> "$HOME/just-calls.txt"
echo "recipe $1 ran"
if grep -qx "$1" "$HOME/stray.txt" 2>/dev/null; then echo stray > stray-file; fi
if grep -qx "$1" "$HOME/edit.txt" 2>/dev/null; then echo edited >> notes.txt; fi
if grep -qx "$1" "$HOME/red.txt" 2>/dev/null; then exit 1; fi
exit 0
"""


@unittest.skipUnless(sys.platform == "darwin", "the gate script runs on the coordinator's Mac, as the lane does")
class GateScript(ReceiptCase):
    """tools/release/coord-gate.sh, run for real in a checkout of its own (a fake `just` and
    `zerocode-orc` stand in): what it prints is what coord_receipt.py reads — the gate's every
    recipe as exactly `just <recipe>`, the strict win-check included (t-119757)."""

    def setUp(self):
        super().setUp()
        self.stack = self.tmp / "stack"
        tools = self.stack / "tools" / "release"
        tools.mkdir(parents=True)
        for name in GATE_TOOLS:
            shutil.copy2(RELEASE / name, tools / name)
        (self.stack / "justfile").write_text("verify: alpha beta win-check-if-available\n")
        (self.stack / "notes.txt").write_text("a tracked file\n")
        (self.stack / "zo-ide").mkdir()
        (self.stack / "zo-ide" / "justfile").write_text("verify: one win-check-if-available\n")
        self.git("init", "-q", "-b", "main")
        self.git("add", "-A")
        self.git("-c", "user.name=t", "-c", "user.email=t@x", "commit", "-q", "-m", "the stack")
        (self.stack / "node_modules").symlink_to(self.tmp)   # the stack's link, as the lane's scratch has one
        # The lane's PATH starts at $HOME/.cargo/bin (lane.sh LAUNCHD_PATH): the fake just goes there.
        bin_ = self.tmp / ".cargo" / "bin"
        bin_.mkdir(parents=True)
        (bin_ / "just").write_text(FAKE_JUST)
        (bin_ / "just").chmod(0o755)
        self.pane = self.tmp / "pane-bin"
        self.pane.mkdir()
        orc = self.pane / "zerocode-orc"
        orc.write_text('#!/bin/sh\n[ "$1" = run-current ] && echo \'{"runId":"run-1001","seated":true}\'\n')
        orc.chmod(0o755)
        self.logs = self.tmp / "gates" / "l9"

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.stack), *args], capture_output=True, text=True, check=True).stdout

    def gate(self, *args, cwd=None, pane=True):
        env = {"HOME": str(self.tmp), "PATH": (f"{self.pane}:" if pane else "") + "/usr/bin:/bin:/usr/sbin:/sbin"}
        return subprocess.run(["bash", "tools/release/coord-gate.sh", *args] if cwd is None
                              else ["bash", str(self.stack / "tools/release/coord-gate.sh"), *args],
                              cwd=cwd or self.stack, env=env, capture_output=True, text=True, timeout=60)

    def on_the_line(self, said, who="coord-l9", rc=0):
        """The job the line would keep of SAID: record, log (its own line above the script's) and rc file."""
        line = receipt_fixture.line_of(self.tmp)
        for folder in ("jobs", "logs"):
            (line / folder).mkdir(parents=True, exist_ok=True)
        ident = "j-1795140000000-c0de"
        now = int(time.time() * 1000)
        record = {"id": ident, "project": str(self.stack / ".git"), "checkout": str(self.stack.resolve()), "owner": who,
                  "class": "first", "quiet": True, "cap_secs": 7200,
                  "command": ["bash", "tools/release/coord-gate.sh", "--who", who, "--logs", str(self.logs), "root", "zo"],
                  "state": "passed" if rc == 0 else "failed", "queued_ms": now - 5000, "started_ms": now - 4000,
                  "ended_ms": now - 1000, "rc": rc, "slot": 0, "pid": 4242}
        (line / "jobs" / f"{ident}.json").write_text(json.dumps(record, indent=2) + "\n")
        (line / "logs" / f"{ident}.log").write_text(f"zo line: cargo builds in {self.tmp / 'slots' / '0'}\n" + said)
        (line / "logs" / f"{ident}.rc").write_text(f"{rc}\n")
        return line / "jobs" / f"{ident}.json"

    def test_its_log_reads_back_as_a_receipt_of_every_recipe_of_both_lists(self):
        ran = self.gate("--who", "coord-l9", "--logs", str(self.logs), "--calm-wait", "0", "root", "zo")
        self.assertEqual(ran.returncode, 0, ran.stdout + ran.stderr)
        top = self.stack.resolve()
        self.assertEqual((self.tmp / "just-calls.txt").read_text().splitlines(),
                         [f"{top} alpha", f"{top} beta", f"{top} win-check", f"{top}/zo-ide one", f"{top}/zo-ide win-check"],
                         "every recipe once, where its list lives, the strict win-check for the development one")
        lines = ran.stdout.splitlines()
        head = self.git("rev-parse", "HEAD").strip()
        self.assertEqual(lines[:5], ["# who: coord-l9", "# run: run-1001", f"# cwd: {top}", f"# logs: {self.logs.resolve()}",
                                     f"head {head} tree {self.git('rev-parse', 'HEAD^{tree}').strip()}"])
        self.assertIn("entry zo-win-check bash -c 'cd zo-ide && just win-check'", lines)
        self.assertEqual(lines[-1], f"verdict: bad=0 (root zo on {head[:9]})")
        self.assertEqual((self.logs.resolve() / "beta.log").read_text(), "recipe beta ran\n", "each entry's output in its own log")
        receipt = self.made([self.on_the_line(ran.stdout)])
        recipes = lambda folder: [l.split()[1] for l in self.facts(receipt, folder) if l.startswith("recipe ")]
        self.assertEqual(recipes("."), ["alpha", "beta", "win-check"])
        self.assertEqual(recipes("zo-ide"), ["one", "win-check"])
        said = self.read_cli(receipt)
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        self.assertIn(f"head {head}", " ".join(said.stdout.splitlines()))

    def test_a_red_recipe_runs_the_rest_and_ends_the_job_red(self):
        (self.tmp / "red.txt").write_text("alpha\n")
        ran = self.gate("--who", "coord-l9-root", "--logs", str(self.logs), "--calm-wait", "0", "root")
        self.assertEqual(ran.returncode, 1, ran.stdout + ran.stderr)
        closed = [line.split(" (")[0] for line in ran.stdout.splitlines() if line.startswith("RC ")]
        self.assertEqual(closed, ["RC alpha=1", "RC beta=0", "RC win-check=0"], "the recipes after a red one still run")
        self.assertTrue(ran.stdout.splitlines()[-1].startswith("verdict: bad=1 (root on "))
        self.assertIn("ended failed", self.refused(lambda: coord_receipt.make([self.on_the_line(ran.stdout, "coord-l9-root", 1)])))

    def test_a_checkout_changed_while_it_ran_ends_the_job_red(self):
        (self.tmp / "edit.txt").write_text("beta\n")   # a recipe that edits a tracked file of the tree it gates
        ran = self.gate("--who", "coord-l9-root", "--logs", str(self.logs), "--calm-wait", "0", "root")
        self.assertEqual(ran.returncode, 1, ran.stdout + ran.stderr)
        self.assertIn("STOP the checkout changed while the gate ran:", ran.stdout)
        self.assertIn("notes.txt", ran.stdout)
        self.assertTrue(ran.stdout.splitlines()[-1].startswith("verdict: bad=1 (root on "), "every entry green, the tree not")

    def test_a_file_a_recipe_leaves_untracked_is_no_change_to_the_tree(self):
        # The gated tree is what git tracks; a recipe's leftover is left in the lane's scratch the same way.
        (self.tmp / "stray.txt").write_text("beta\n")
        ran = self.gate("--who", "coord-l9-root", "--logs", str(self.logs), "--calm-wait", "0", "root")
        self.assertEqual(ran.returncode, 0, ran.stdout + ran.stderr)
        self.assertTrue((self.stack / "stray-file").exists())
        self.assertTrue(ran.stdout.splitlines()[-1].startswith("verdict: bad=0 (root on "))

    def test_it_will_not_start_where_its_receipt_could_not_stand(self):
        cases = (("an owner that is no coordinator's", ("--who", "t-119757", "--logs", str(self.logs), "root"), {},
                  "t-119757 is not a coordinator's coord-<label>"),
                 ("a logs folder inside the checkout", ("--who", "coord-l9", "--logs", str(self.stack / "gate-logs"), "root"), {},
                  "is inside the checkout it gates"),
                 ("a logs folder named from here", ("--who", "coord-l9", "--logs", "gate-logs", "root"), {}, "is no absolute path"),
                 ("below the checkout's top", ("--who", "coord-l9", "--logs", str(self.logs), "root"), {"cwd": self.stack / "zo-ide"},
                  "not the top of its checkout"),
                 ("no pane's run", ("--who", "coord-l9", "--logs", str(self.logs), "root"), {"pane": False}, "no run:"))
        for case, args, kw, said in cases:
            with self.subTest(case=case):
                ran = self.gate(*args, **kw)
                self.assertEqual(ran.returncode, 2, ran.stdout + ran.stderr)
                self.assertIn(f"STOP ", ran.stdout)
                self.assertIn(said, ran.stdout)
                self.assertFalse((self.tmp / "just-calls.txt").exists(), "no recipe ran")
                # Its head and tree only past every check: the reader takes a log that has them as run at the top.
                self.assertFalse([line for line in ran.stdout.splitlines() if line.startswith("head ")], ran.stdout)
        (self.stack / "justfile").write_text("verify: alpha beta gamma\n")   # a change nobody committed
        dirty = self.gate("--who", "coord-l9", "--logs", str(self.logs), "root")
        self.assertEqual(dirty.returncode, 2, dirty.stdout + dirty.stderr)
        self.assertIn("STOP the checkout is not clean:", dirty.stdout)
        self.assertFalse((self.tmp / "just-calls.txt").exists())
        for args in ((), ("--who", "coord-l9", "root"), ("--logs", str(self.logs), "root"), ("--who", "coord-l9", "--logs", "x"),
                     ("--who", "coord-l9", "--logs", str(self.logs), "root", "root"), ("--who", "coord-l9", "--logs", str(self.logs), "web")):
            with self.subTest(args=args):
                self.assertEqual(self.gate(*args).returncode, 2)


if __name__ == "__main__":
    unittest.main(verbosity=1)
