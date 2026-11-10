#!/usr/bin/env python3
"""Contract for `zo-ide/bench/run_tasks.py`: what it counts out of a run's
`--json` lines, and what the verifiers of the round-trip tasks (27–29) pass.

The bench is read for its `requests` column — the round trips of a run — and a
column that counted a retry as a round trip, or missed one, would send a
harness change the wrong way. The three tasks have one answer each — two of
them in the model's last message, which the runner hands the verifier — and a
judge that passed a wrong answer would make every arm look alike again.

Run: python3 tools/tests/test_zo_bench_run_tasks.py   (stdlib only)
"""

import importlib.util
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
BENCH = REPO / "zo-ide" / "bench"
_spec = importlib.util.spec_from_file_location("run_tasks", BENCH / "run_tasks.py")
run_tasks = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = run_tasks
_spec.loader.exec_module(run_tasks)


def lines(*events) -> str:
    return "\n".join(json.dumps(event) for event in events)


class ReadEvents(unittest.TestCase):
    def test_a_request_is_counted_once_however_often_it_was_resent(self):
        got = run_tasks.read_events(lines(
            {"type": "user", "text": "do it"},
            {"type": "stream_phase", "phase": "request_sent", "attempt": 1},
            {"type": "stream_phase", "phase": "request_sent", "attempt": 2},
            {"type": "tool_call", "id": "a", "name": "Read", "status": "pending"},
            {"type": "tool_call", "id": "a", "name": "Read", "status": "running"},
            {"type": "usage", "total_tokens": 7000},
            {"type": "tool_result", "id": "a", "is_error": True},
            {"type": "wire_model", "model": "m-1", "source": "session"},
            {"type": "stream_phase", "phase": "request_sent", "attempt": 1},
            {"type": "stream_phase", "phase": "first_byte", "attempt": 1},
            {"type": "usage", "total_tokens": 15000},
            {"type": "result", "status": "succeeded", "error": None},
        ))
        self.assertEqual(got["requests"], 2)
        self.assertEqual((got["tool_calls"], got["tool_errors"], got["by_tool"]), (1, 1, {"Read": 1}))
        self.assertEqual((got["total_tokens"], got["models"], got["status"]), (15000, ["m-1"], "succeeded"))

    def test_lines_that_are_not_events_are_passed_over(self):
        got = run_tasks.read_events("not json\n\n" + lines({"type": "stream_phase", "phase": "request_sent"}))
        self.assertEqual(got["requests"], 1, "a line with no attempt is a first attempt")
        self.assertEqual(got["status"], None)


class RunsKeepOutOfTheVault(unittest.TestCase):
    def test_the_vault_is_switched_off_and_not_only_left_out(self):
        self.assertEqual((run_tasks.VAULT_ENV, run_tasks.VAULT_OFF), ("ZEROCODE_SECOND_BRAIN", "off"))


class RoundTripTasks(unittest.TestCase):
    """Each verifier passes the one right outcome and fails the nearest wrong
    ones — a run that did nothing among them."""

    def judged(self, task: str, said: str = "", change=None) -> bool:
        folder = BENCH / "tasks" / task
        work = run_tasks.prepare_workdir(folder)
        beside = Path(tempfile.mkdtemp(prefix="zo-bench-test-"))
        try:
            answer = beside / "last-message.txt"
            answer.write_text(said)
            if change:
                change(work)
            return run_tasks.judge(folder, work, answer)[0]
        finally:
            shutil.rmtree(work, ignore_errors=True)
            shutil.rmtree(beside, ignore_errors=True)

    def test_every_task_has_what_the_runner_reads(self):
        ids = [meta["id"] for _, meta in run_tasks.tasks_in(BENCH / "tasks", "27,28,29")]
        self.assertEqual(ids, ["27", "28", "29"])

    def test_the_rule_task_wants_the_command_from_agents_md(self):
        task = "27-rule-in-agents-md"
        self.assertTrue(self.judged(task, "make zebra-check\n"))
        self.assertTrue(self.judged(task, "`make zebra-check`"))
        self.assertFalse(self.judged(task, "make test\n"))
        self.assertFalse(self.judged(task, ""), "no answer is not an answer")
        self.assertFalse(
            self.judged(task, "make zebra-check", lambda work: (work / "answer.txt").write_text("x")),
            "a question leaves the tree as it was",
        )

    def test_the_picture_task_wants_its_one_colour(self):
        task = "28-read-a-picture"
        self.assertTrue(self.judged(task, "red\n"))
        self.assertTrue(self.judged(task, "Red."))
        self.assertTrue(self.judged(task, "red\n\nAll 9,216 pixels are (220, 20, 20)."), "how it was found may follow")
        self.assertFalse(self.judged(task, "blue\n"))
        self.assertFalse(self.judged(task, "red or orange"), "one colour was asked for")
        self.assertFalse(self.judged(task, "reddish? I could not open it"), "`red` is a word, not a prefix")
        self.assertFalse(self.judged(task, ""))

    def test_the_edit_task_wants_the_one_line_and_nothing_else(self):
        task = "29-one-line-edit"
        right = 'name = "demo"\nhost = "127.0.0.1"\nport = 9090\ntimeout_secs = 30\n'
        self.assertTrue(self.judged(task, "done", lambda work: (work / "config.toml").write_text(right)))
        self.assertFalse(self.judged(task, "done"), "the untouched file is not the edit")
        self.assertFalse(
            self.judged(task, "done", lambda work: (work / "config.toml").write_text(right.replace("30", "60"))),
            "a second line changed",
        )

    def test_a_tree_only_task_is_judged_as_before_with_the_answer_beside_it(self):
        # Task 24's verifier was written before the answer was handed over;
        # it is given the file all the same and does not read it.
        self.assertFalse(self.judged("24-crlf-config", "whatever was said"))


if __name__ == "__main__":
    unittest.main()
