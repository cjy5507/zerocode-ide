#!/usr/bin/env python3
"""The direct-against-delegation harness (`tools/delegation-eval/eval.py`, t-73910 W4-3).

It spends nothing: a plan starts no agent. Every planned run stays in the denominator,
a price or a verification it cannot read is unknown and never 0, and the sample seed
really fails its own acceptance until it is fixed.

Run: python3 tools/tests/test_delegation_eval.py   (stdlib only)
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
EVAL = REPO / "tools/delegation-eval/eval.py"
FIXED_TALLY = '''\
def tally(text):
    counts = {}
    for word in text.lower().split():
        counts[word] = counts.get(word, 0) + 1
    return counts


def top(text, n):
    counts = tally(text)
    return sorted(counts, key=lambda word: -counts[word])[:n]
'''


class EvalTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.out = self.root / "out"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.orc_calls = self.root / "orc-calls"
        orc = self.bin / "zerocode-orc"
        orc.write_text(f"#!/bin/sh\necho \"$@\" >> '{self.orc_calls}'\nexit 0\n")
        orc.chmod(0o755)
        self.env = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}"}

    def tearDown(self):
        self.tmp.cleanup()

    def eval(self, *args):
        said = subprocess.run([sys.executable, str(EVAL), *args], capture_output=True, text=True, env=self.env, timeout=60)
        self.assertEqual(said.returncode, 0, said.stdout + said.stderr)
        return said.stdout

    def plan(self, reps=1, seed=1, out=None):
        return self.eval("plan", "--reps", str(reps), "--seed", str(seed), "--agent", "claude",
                         "--model", "model-x", "--effort", "high", "--out", str(out or self.out))

    def runs(self, out=None):
        return json.loads(((out or self.out) / "plan.json").read_text())["runs"]

    def summary(self):
        return {(r["scenario"], r["condition"]): r for r in json.loads(self.eval("summarize", "--out", str(self.out), "--json"))["rows"]}

    def test_a_plan_holds_every_run_once_in_its_seeds_order_with_one_model_and_effort(self):
        printed = self.plan(reps=3, seed=7)
        runs = self.runs()
        self.assertEqual(len(runs), 6)
        self.assertEqual(sorted((r["condition"], r["rep"]) for r in runs),
                         sorted((c, n) for c in ("direct", "delegate") for n in (1, 2, 3)))
        self.plan(reps=3, seed=7, out=self.root / "again")
        self.assertEqual([r["run"] for r in self.runs(self.root / "again")], [r["run"] for r in runs])
        self.assertEqual(printed.count("--model model-x --effort high"), 6, "both conditions launch alike")
        self.assertIn("Do not summon", (self.out / runs[[r["condition"] for r in runs].index("direct")]["run"] / "brief.md").read_text())

    def test_nothing_it_does_starts_an_agent(self):
        self.plan()
        self.eval("prepare", "--out", str(self.out))
        self.eval("accept", "--out", str(self.out))
        self.eval("summarize", "--out", str(self.out))
        self.assertFalse(self.orc_calls.exists(), "the harness called zerocode-orc")

    def test_the_seed_fails_its_acceptance_until_it_is_fixed_and_a_changed_test_is_outside(self):
        self.plan()
        self.eval("prepare", "--out", str(self.out))
        first, second = self.runs()
        self.eval("accept", "--out", str(self.out))
        self.assertFalse(json.loads((self.out / first["run"] / "accept.json").read_text())["passed"])
        Path(first["checkout"], "tally.py").write_text(FIXED_TALLY)
        Path(second["checkout"], "tally.py").write_text(FIXED_TALLY)
        Path(second["checkout"], "test_tally.py").write_text("")
        self.eval("accept", "--out", str(self.out))
        self.assertTrue(json.loads((self.out / first["run"] / "accept.json").read_text())["passed"])
        outside = json.loads((self.out / second["run"] / "accept.json").read_text())
        self.assertFalse(outside["passed"])
        self.assertEqual(outside["outside"], ["test_tally.py"])

    def test_a_run_never_accepted_stays_in_the_denominator(self):
        self.plan()
        self.eval("prepare", "--out", str(self.out))
        direct = next(r for r in self.runs() if r["condition"] == "direct")
        Path(direct["checkout"], "tally.py").write_text(FIXED_TALLY)
        self.eval("accept", "--out", str(self.out), "--run", direct["run"])
        rows = self.summary()
        self.assertEqual((rows[("s01-small-fix", "direct")]["passed"], rows[("s01-small-fix", "direct")]["runs"]), (1, 1))
        self.assertEqual((rows[("s01-small-fix", "delegate")]["passed"], rows[("s01-small-fix", "delegate")]["runs"]), (0, 1))

    def dispatch(self, run, task, started, ended, verified, attempts=1):
        body = {"startedMs": started, "endedMs": ended, "time": {"task": {
            "firstStartedMs": started, "attempts": attempts, "afterReport": {"verifiedMs": verified}}}}
        (self.out / run / f"dispatch-{task}.json").write_text(json.dumps(body))

    def cost(self, run, task, usd, reason=None, complete=True):
        body = {"generation": {"historyComplete": complete, "sessionsKnown": 1, "sessionsLinked": 1,
                               "inputTokens": 10, "outputTokens": 5, "cacheReadTokens": 0, "cacheWriteTokens": 0,
                               "usd": usd, "usdReason": reason}}
        (self.out / run / f"cost-{task}.json").write_text(json.dumps(body))

    def test_a_price_it_cannot_read_is_unknown_never_zero(self):
        self.plan()
        delegate = next(r["run"] for r in self.runs() if r["condition"] == "delegate")
        self.dispatch(delegate, "t-1", 1000, 5000, 6000)
        self.dispatch(delegate, "t-2", 2000, 4000, 4500)
        self.cost(delegate, "t-1", 0.25)
        self.cost(delegate, "t-2", None, reason="unpriced_model")
        row = self.summary()[("s01-small-fix", "delegate")]
        self.assertIsNone(row["usdMedian"])
        self.assertEqual(row["tokensMedian"], 30)
        self.assertIn("dollars: unpriced_model", row["why"])
        self.assertIsNone(row["interventions"])
        printed = self.eval("summarize", "--out", str(self.out))
        line = next(l for l in printed.splitlines() if l.startswith("| s01-small-fix | delegate |"))
        self.assertEqual(line.split(" | ")[6], "unknown", "the dollars printed as a number")

    def test_the_wall_clock_runs_to_the_latest_verification_and_counts_overlap(self):
        self.plan()
        delegate = next(r["run"] for r in self.runs() if r["condition"] == "delegate")
        self.dispatch(delegate, "t-1", 1000, 5000, 9000, attempts=2)
        self.dispatch(delegate, "t-2", 2000, 4000, 4500)
        row = self.summary()[("s01-small-fix", "delegate")]
        self.assertEqual(row["wallMedianMs"], 8000)
        self.assertEqual(row["rework"], 1)
        self.assertEqual(row["mostAtOnce"], 2)
        direct = next(r["run"] for r in self.runs() if r["condition"] == "direct")
        self.dispatch(direct, "t-3", 1000, 3000, None)
        self.assertIsNone(self.summary()[("s01-small-fix", "direct")]["wallMedianMs"], "no verification, no wall clock")


if __name__ == "__main__":
    unittest.main(verbosity=1)
