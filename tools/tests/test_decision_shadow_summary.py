#!/usr/bin/env python3
"""Contract for `tools/decision_shadow_summary.py` — it must read the rows the
ledgers actually hold.

The routing shadow writes its rows in camelCase (`DecisionShadowRow` is
`#[serde(rename_all = "camelCase")]`: `elapsedMs`, `inputTokens`,
`rubricVersion`), the rerank shadow in snake_case (`RerankShadowRow`:
`elapsed_ms`, `input_tokens`, `rubric_version`). A reader that knows one
spelling reads the other ledger's latency as 0 ms and its tokens as absent —
a p50 that is wrong and looks plausible.

Run: python3 tools/tests/test_decision_shadow_summary.py   (stdlib only)
"""

import importlib.util
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location(
    "decision_shadow_summary", REPO / "tools" / "decision_shadow_summary.py"
)
summary = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = summary
_spec.loader.exec_module(summary)


def routing_row(elapsed_ms: int, tokens: int, cached: bool = False) -> dict:
    """A routing-shadow row as `decision-shadow.jsonl` holds it (camelCase)."""
    return {
        "at": 1789600000000,
        "attempt": "session-1-0@1",
        "task": "0123456789abcdef",
        "rubricVersion": 2,
        "model": "jev-1.13.0",
        "outcome": "answered",
        "elapsedMs": elapsed_ms,
        "retries": 0,
        "cached": cached,
        "inputTokens": tokens,
        "probe": {"complexity": "small"},
        "jev": {"complexity": {"choice": "small", "confidence": 0.5}},
    }


def rerank_row(elapsed_ms: int, tokens: int) -> dict:
    """A rerank-shadow row as `rerank-shadow.jsonl` holds it (snake_case)."""
    return {
        "at": 1789600000000,
        "query": 1,
        "notes": 2,
        "rubric_version": 1,
        "outcome": "answered",
        "candidates": 8,
        "cached": False,
        "elapsed_ms": elapsed_ms,
        "retries": 0,
        "model": "jev-1.13.0",
        "input_tokens": tokens,
    }


class ReadsBothSpellings(unittest.TestCase):
    def test_routing_rows_in_camel_case_carry_their_latency_and_tokens(self):
        rows = [routing_row(600, 900), routing_row(700, 1100), routing_row(5, 900, cached=True)]
        result = summary.summarise(rows)
        self.assertEqual(result["latency_ms"]["timed_rows"], 2, "the cached row costs no round trip")
        self.assertEqual(result["latency_ms"]["p50"], 600)
        self.assertEqual(result["latency_ms"]["max"], 700)
        self.assertEqual(result["input_tokens"]["total"], 2900)
        self.assertEqual(result["rubric_versions"], {"2": 3})

    def test_rerank_rows_in_snake_case_still_read(self):
        result = summary.summarise([rerank_row(400, 300), rerank_row(900, 500)])
        self.assertEqual(result["latency_ms"]["p50"], 400)
        self.assertEqual(result["latency_ms"]["p95"], 900)
        self.assertEqual(result["input_tokens"]["total"], 800)
        self.assertEqual(result["rubric_versions"], {"1": 2})

    def test_a_row_the_jev_door_refused_sent_nothing_and_is_not_timed(self):
        """Since the Jev door, every row says how many requests it sent. A refusal
        (`not_consented`, `budget`, `off`) and a recall send none; only a row
        with `requests` above zero is a round trip, and the lines the door
        withheld are counted."""
        sent = routing_row(640, 580)
        sent.update({"requests": 1, "redactedLines": 2})
        refused = routing_row(0, 0)
        refused.update({"outcome": "not_consented", "requests": 0, "redactedLines": 0})
        del refused["inputTokens"], refused["jev"]
        result = summary.summarise([sent, refused])
        self.assertEqual(result["latency_ms"]["timed_rows"], 1, "a refusal is no round trip")
        self.assertEqual(result["latency_ms"]["p50"], 640)
        self.assertEqual(result["redacted_lines"], 2)

    def test_a_probe_recorded_as_a_failure_string_is_left_out_of_agreement(self):
        row = routing_row(600, 900)
        row["probe"] = "timeout"
        result = summary.summarise([row])
        self.assertEqual(result["both_answered"], 0)


if __name__ == "__main__":
    unittest.main()
