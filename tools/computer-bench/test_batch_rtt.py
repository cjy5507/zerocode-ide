"""tools/computer-bench/batch_rtt.py through the scripted fake shim on PATH:
per round, five lone calls, one chain of five and one batch; the pace read
from status; the report's shape and the exit code's judgement."""
import json
import os
import pathlib
import stat
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from batch_rtt import pace

HERE = pathlib.Path(__file__).resolve().parent
SCRIPT = HERE / "batch_rtt.py"
FAKE = HERE / "fake-shim.py"


class Pace(unittest.TestCase):
    def read(self, table):
        with patch("batch_rtt.call", return_value=(1, {"result": {"pace": table}})):
            return pace()

    def test_unlimited_has_no_divisor_or_burst(self):
        self.assertEqual(self.read({"mode": "unlimited", "perSecond": None, "burst": None}), (None, None))
        self.assertEqual(self.read({"perSecond": 10, "burst": 20}), (10, 20))

    def test_invalid_modes_and_rates_fail_closed(self):
        for table in ({"mode": "unknown"}, {"mode": "unlimited", "perSecond": 10},
                      {"mode": "paced", "perSecond": 0, "burst": 20},
                      {"mode": "paced", "perSecond": float("nan"), "burst": 20}):
            with self.assertRaises(SystemExit):
                self.read(table)


class BatchRtt(unittest.TestCase):
    def run_bench(self, *args, env_extra=None):
        tmp = pathlib.Path(tempfile.mkdtemp())
        shim = tmp / "bin" / "zerocode-computer"
        shim.parent.mkdir()
        shim.write_text(FAKE.read_text())
        shim.chmod(shim.stat().st_mode | stat.S_IEXEC)
        state = tmp / "state"
        state.mkdir()
        env = {**os.environ, "PATH": f"{shim.parent}:{os.environ['PATH']}", "FAKE_STATE": str(state),
               **(env_extra or {})}
        out = subprocess.run(["python3", str(SCRIPT), *args], env=env, capture_output=True, text=True, timeout=120)
        calls = (state / "calls").read_text().splitlines() if (state / "calls").exists() else []
        return out, [c.split()[0] for c in calls]

    def test_a_round_is_five_lone_calls_one_chain_of_five_and_one_batch(self):
        out, verbs = self.run_bench("--rounds", "2", "--variant", "wait", "--bar", "10")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertEqual(verbs.count("batch"), 2, "one batch per round")
        self.assertEqual(verbs.count("wait"), 2 * (5 + 5), "five lone waits and a chain of five per round")
        report = json.loads(out.stdout)["wait"]
        self.assertEqual(report["singles"]["trips"], 5)
        self.assertEqual(report["batch"]["trips"], 1)
        self.assertIn("ratio_p50", report)
        self.assertIn("ratio_to_chain_p50", report)

    def test_the_default_run_never_touches_the_pointer(self):
        out, verbs = self.run_bench("--rounds", "1", "--bar", "10")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertNotIn("mouse-move", verbs)
        self.assertNotIn("status", verbs, "the wait variant needs no pace")
        self.assertEqual(list(json.loads(out.stdout)), ["wait"])

    def test_the_move_variant_reads_the_pointer_and_the_pace(self):
        out, verbs = self.run_bench("--rounds", "1", "--variant", "move", "--bar", "10")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertIn("status", verbs, "the pace is read, not assumed — even from an idle helper")
        self.assertEqual(verbs.count("cursor-position"), 1 + 5 + 1 + 1,
                         "the round's point, then before every move, the batch and the chain")
        self.assertEqual(verbs.count("mouse-move"), 5 + 5, "five lone moves and a chain of five")

    def test_a_round_the_person_moved_the_pointer_in_is_dropped_whole(self):
        # The third read is before the second lone move: the pointer moved.
        out, verbs = self.run_bench("--rounds", "1", "--variant", "move", "--bar", "10",
                                    env_extra={"FAKE_MOVE_AT": "2"})
        self.assertNotEqual(out.returncode, 0, "no round left to judge")
        self.assertIn("every round was dropped", out.stderr)
        self.assertEqual(verbs.count("mouse-move"), 1, "not one move after the person's hand moved it")
        self.assertNotIn("batch", verbs)

    def test_the_bar_judges_the_exit_code(self):
        out, _ = self.run_bench("--rounds", "1", "--variant", "wait", "--bar", "0")
        self.assertEqual(out.returncode, 1, "a batch no faster than nothing fails a zero bar")


if __name__ == "__main__":
    unittest.main()
