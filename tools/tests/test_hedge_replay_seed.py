#!/usr/bin/env python3
"""Contract for `tools/hedge-replay/seed.py` — it must pick the rows the rule
picks, and refuse a seed that did not.

The sample a hedge's delay is read from is not "the ledger's latencies": it is
`Timed::sample` in `zo-ide/.../smart_router/jev_gate.rs`, which throws out
every row whose `elapsedMs` is not one request's own wait — a timeout (the
number is the wall), a memo answer (zero), a retried row (an attempt plus its
backoffs), a hedged row (the faster of two copies). A seeder that kept any of
them would hand `hedge::plan` a distribution the wire never saw, and the
replay's before/after would be a comparison of two fictions.

The two ledgers spell their columns differently — routing camelCase, recall
snake_case — so both spellings are tested; reading one only is how a whole
seat silently comes out as no firings at all.

Run: python3 tools/tests/test_hedge_replay_seed.py   (stdlib only)
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
_spec = importlib.util.spec_from_file_location("hedge_replay_seed", REPO / "tools" / "hedge-replay" / "seed.py")
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)


def routing_row(**over) -> dict:
    """A routing-shadow row as `decision-shadow.jsonl` holds it (camelCase)."""
    row = {
        "at": 1789600000000,
        "task": "0123456789abcdef",
        "rubricVersion": 1,
        "outcome": "answered",
        "elapsedMs": 300,
        "retries": 0,
        "cached": False,
    }
    row.update(over)
    return row


def recall_row(**over) -> dict:
    """A rerank-shadow row as `rerank-shadow.jsonl` holds it (snake_case)."""
    row = {
        "at": 1789600000000,
        "query": 1,
        "notes": 2,
        "rubric_version": 1,
        "outcome": "answered",
        "candidates": 8,
        "cached": False,
        "elapsed_ms": 300,
        "retries": 0,
    }
    row.update(over)
    return row


class SampleFilter(unittest.TestCase):
    def test_only_one_requests_own_latency_is_a_sample(self):
        self.assertEqual(seed.one_requests_own_latency(routing_row(elapsedMs=641)), 641)
        self.assertEqual(seed.one_requests_own_latency(recall_row(elapsed_ms=301)), 301)

    def test_a_timeout_is_the_wall_not_a_latency(self):
        self.assertIsNone(seed.one_requests_own_latency(routing_row(outcome="timeout", elapsedMs=1501)))

    def test_a_memo_answer_and_a_refusal_are_not_round_trips(self):
        self.assertIsNone(seed.one_requests_own_latency(routing_row(cached=True, elapsedMs=0)))
        self.assertIsNone(seed.one_requests_own_latency(routing_row(outcome="not_consented", elapsedMs=0)))

    def test_a_retried_row_is_an_attempt_plus_its_backoffs(self):
        self.assertIsNone(seed.one_requests_own_latency(routing_row(retries=1, elapsedMs=2400)))

    def test_a_hedged_row_is_the_faster_of_two_copies(self):
        """Left in, a hedge would walk its own delay down a sample of its own
        making — the delay names the firings, and the firings name the delay."""
        self.assertIsNone(seed.one_requests_own_latency(routing_row(hedgeFired=True, elapsedMs=640)))
        self.assertIsNone(seed.one_requests_own_latency(recall_row(hedge_fired=True, elapsed_ms=640)))


class Reconstruction(unittest.TestCase):
    """The self-check: a seed whose samples do not reproduce the delays the
    ledger recorded read the wrong rows, and must not be handed on."""

    def a_ledger(self, tmp: Path, name: str, rows: list[dict]) -> Path:
        path = tmp / "proj" / "state" / "smart-router" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("".join(json.dumps(row) + "\n" for row in rows))
        return path

    def test_a_window_of_answers_reproduces_the_delay_the_ledger_recorded(self):
        # Ten answers whose p75 is 600 and whose median is 400: the rule as it
        # stood named min(600, 1500 - 400) = 600.
        answers = [100, 200, 300, 350, 400, 450, 500, 600, 700, 900]
        rows = [routing_row(elapsedMs=ms) for ms in answers]
        rows.append(routing_row(hedgeFired=True, hedgeDelayMs=600, elapsedMs=800))
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            path = self.a_ledger(tmp, "decision-shadow.jsonl", rows)
            firings = seed.firings_in("routing", path)
        self.assertEqual(len(firings), 1)
        self.assertEqual(sorted(firings[0]["samples"]), answers)
        self.assertEqual(seed.reconstruction_holds(firings), (1, 1))

    def test_a_seed_that_kept_a_hedged_row_would_not_reproduce_the_delay(self):
        """The filter is load-bearing, not hygiene: leave one hedged row in and
        the reconstruction stops matching, which is what the self-check is
        for."""
        answers = [100, 200, 300, 350, 400, 450, 500, 600, 700, 900]
        firing = {"seat": "routing", "at": 1, "recordedDelayMs": 600, "outcome": "answered", "won": False}
        self.assertEqual(seed.reconstruction_holds([dict(firing, samples=answers)]), (1, 1))
        held, recorded = seed.reconstruction_holds([dict(firing, samples=[*answers, 800])])
        self.assertEqual((held, recorded), (0, 1), "a stray row must break the check")

    def test_a_firing_under_min_samples_is_kept_and_fails_reconstruction(self):
        """A pruned ledger can lose the firing's history. That is missing
        evidence, not permission to erase the firing from the denominator."""
        rows = [recall_row(elapsed_ms=300) for _ in range(seed.MIN_SAMPLES - 1)]
        rows.append(recall_row(hedge_fired=True, hedge_delay_ms=300, elapsed_ms=400))
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            path = self.a_ledger(tmp, "rerank-shadow.jsonl", rows)
            self.assertEqual(seed.reconstruction_holds(seed.firings_in("recall", path)), (0, 1))

    def test_half_a_line_from_a_pruned_ledger_is_skipped_not_fatal(self):
        """A ledger past `SHADOW_LEDGER_MAX_BYTES` keeps only its newer half,
        which can open mid-line."""
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            path = tmp / "proj" / "state" / "smart-router" / "decision-shadow.jsonl"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('sedMs": 400}\n' + json.dumps(routing_row(elapsedMs=500)) + "\n")
            self.assertEqual(len(seed.rows_of(path)), 1)


class CommandIntegrity(Reconstruction):
    def run_seed(self, rows, *, repeat_root=False):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            self.a_ledger(tmp, "decision-shadow.jsonl", rows)
            command = [sys.executable, str(REPO / "tools/hedge-replay/seed.py"), "--project", str(tmp), "--out", str(tmp / "seed.json")]
            if repeat_root:
                command += ["--project", str(tmp / ".")]
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            output = json.loads((tmp / "seed.json").read_text()) if (tmp / "seed.json").exists() else None
            return result, output

    def complete_rows(self):
        rows = [routing_row(at=n, elapsedMs=300) for n in range(seed.MIN_SAMPLES)]
        rows.append(routing_row(at=seed.MIN_SAMPLES, hedgeFired=True, hedgeDelayMs=300, elapsedMs=400))
        return rows

    def test_missing_delay_fails_instead_of_verifying_zero_of_zero(self):
        rows = self.complete_rows()
        del rows[-1]["hedgeDelayMs"]
        result, output = self.run_seed(rows)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIsNone(output)

    def test_insufficient_history_fails_instead_of_erasing_the_firing(self):
        result, output = self.run_seed(self.complete_rows()[1:])
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIsNone(output)

    def test_repeating_a_root_does_not_count_the_same_firing_twice(self):
        result, output = self.run_seed(self.complete_rows(), repeat_root=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(output["firings"]), 1)

    def test_a_deliberately_broken_recorded_delay_fails_the_command(self):
        rows = self.complete_rows()
        rows[-1]["hedgeDelayMs"] += 1
        result, output = self.run_seed(rows)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIsNone(output)


class ConstantsMatchTheirSource(unittest.TestCase):
    """The three numbers the seeder carries are copies of Rust constants, and a
    copy that drifts is a seed replayed against a wall or a window the wire
    never used — which no output would say aloud."""

    def named(self, path: str, name: str) -> int:
        text = (REPO / path).read_text()
        # `const NAME: type = value;` and nothing looser — a prose mention of
        # the same name in a doc comment must not be read as its value.
        found = re.search(rf"const\s+{name}\s*:[^=]*=\s*([0-9_]+)\s*;", text)
        self.assertIsNotNone(found, f"{name} is no longer declared in {path}")
        return int(found.group(1).replace("_", ""))

    def test_the_wall_is_the_seats_own_apply_deadline(self):
        self.assertEqual(
            seed.WALL_MS,
            self.named("crates/zerocode-core/src/jev.rs", "ROUTING_APPLY_DEADLINE_MS"),
        )

    def test_the_window_is_the_gates_own_sample_rows(self):
        self.assertEqual(
            seed.SAMPLE_ROWS,
            self.named(
                "zo-ide/crates/tools/src/misc_tools/smart_router/jev_gate.rs", "HEDGE_SAMPLE_ROWS"
            ),
        )

    def test_the_floor_is_the_rules_own_min_samples(self):
        self.assertEqual(
            seed.MIN_SAMPLES,
            self.named("crates/zerocode-core/src/jev/hedge.rs", "MIN_SAMPLES"),
        )


if __name__ == "__main__":
    unittest.main()
