#!/usr/bin/env python3
"""Contract for `tools/routing-replay/seed.py` — it picks transcripts and
ledgers and counts, and nothing else.

A seed row is a path and counts: a transcript's messages, the turns a person
began and the agents those turns started; a ledger's rows, the ones carrying
the chat probe's answer and the ones carrying an attempt key. The seeder must
read which files are transcripts the way compaction-replay does (it loads that
reading rather than copying it), must count a harness note as the turn it
continues, and must carry no words: the replay reads the transcript's body
itself, turn by turn, through the product's own functions.

Run: python3 tools/tests/test_routing_replay_seed.py   (stdlib only)
"""

from __future__ import annotations

import importlib.util
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("routing_replay_seed", REPO / "tools" / "routing-replay" / "seed.py")
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)


def text(role: str, words: str) -> dict:
    return {"type": "message", "message": {"role": role, "blocks": [{"type": "text", "text": words}]}}


def call(name: str, prompt: str = "trace the slow start") -> dict:
    block = {"type": "tool_use", "id": f"toolu-{name}", "name": name, "input": json.dumps({"prompt": prompt})}
    return {"type": "message", "message": {"role": "assistant", "blocks": [block]}}


def transcript(*rows: dict, updated: int = 1) -> str:
    head = {"type": "session_meta", "version": 1, "session_id": "session-1-0", "created_at_ms": 1, "updated_at_ms": updated}
    return "".join(json.dumps(row) + "\n" for row in (head, *rows))


def lines(*rows: dict) -> str:
    return "".join(json.dumps(row) + "\n" for row in rows)


class Picking(unittest.TestCase):
    def project(self, tmp: Path) -> Path:
        path = tmp / "proj"
        (path / "sessions").mkdir(parents=True, exist_ok=True)
        (path / seed.STATE_DIR[0] / seed.STATE_DIR[1]).mkdir(parents=True, exist_ok=True)
        return path

    def test_which_files_are_transcripts_is_compaction_replays_reading(self):
        compaction = sys.modules["compaction_replay_seed"]
        self.assertIs(seed.picker, compaction)
        self.assertTrue(seed.picker.is_transcript(Path("session-1-0.jsonl")))
        self.assertFalse(seed.picker.is_transcript(Path("session-1-0.vault.jsonl")))

    def test_a_harness_note_continues_the_turn_before_it(self):
        rows = [
            text("user", "fix the flaky test"),
            text("user", "[zo: the check came back red] keep going"),
            text("user", "   "),
            text("assistant", "done"),
            text("user", "now the docs"),
        ]
        self.assertEqual(seed.person_turns(rows), 2, "the note and the blank line began nothing")

    def test_a_spawn_is_the_runtimes_fan_out_call(self):
        rows = [call(name) for name in ("Agent", "Task", "SpawnMultiAgent", "Workflow", "read_file", "bash")]
        self.assertEqual(seed.spawns(rows), 4)

    def test_a_transcript_no_person_spoke_in_is_left_out(self):
        with tempfile.TemporaryDirectory() as raw:
            path = self.project(Path(raw)) / "sessions" / "session-1-0.jsonl"
            path.write_text(transcript(text("user", "[zo: resumed]"), text("assistant", "hello")))
            self.assertIsNone(seed.describe(path, 0))

    def test_a_transcript_older_than_the_window_is_left_out(self):
        with tempfile.TemporaryDirectory() as raw:
            path = self.project(Path(raw)) / "sessions" / "session-1-0.jsonl"
            path.write_text(transcript(text("user", "go"), updated=5))
            self.assertIsNone(seed.describe(path, 6))
            self.assertIsNotNone(seed.describe(path, 5))

    def test_a_row_carries_counts_and_no_words(self):
        with tempfile.TemporaryDirectory() as raw:
            path = self.project(Path(raw)) / "sessions" / "session-2-0.jsonl"
            path.write_text(
                transcript(
                    text("user", "rename the flag in the settings page"),
                    call("Agent", "port the settings page"),
                    text("user", "[zo: the agent finished]"),
                    text("user", "and the docs"),
                    updated=9,
                )
            )
            row = seed.describe(path, 0)
            self.assertEqual((row["messages"], row["turns"], row["spawns"], row["updatedAt"]), (4, 2, 1, 9))
            written = json.dumps(row)
            for words in ("rename the flag", "port the settings page", "the agent finished", "and the docs"):
                self.assertNotIn(words, written, "no words leave the transcript")

    def test_a_ledger_counts_its_probe_answers_and_its_attempt_keys(self):
        with tempfile.TemporaryDirectory() as raw:
            path = self.project(Path(raw)) / seed.STATE_DIR[0] / seed.STATE_DIR[1] / seed.ROUTING_LEDGER
            path.write_text(
                lines(
                    {"probe": {"complexity": "small"}, "attempt": "s@1"},
                    {"probe": "not_asked", "attempt": "s@2"},
                    {"runId": "run-1"},
                    {"transition": "rise"},
                )
            )
            self.assertEqual(seed.ledger(path)["rows"], 4)
            self.assertEqual(seed.ledger(path)["probed"], 1, "a probe that was not asked carries no answer")
            self.assertEqual(seed.ledger(path)["attempted"], 3)

    def test_the_command_writes_transcripts_ledgers_and_outcomes(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            project = self.project(tmp)
            (project / "sessions" / "session-3-0.jsonl").write_text(transcript(text("user", "go"), updated=10**13))
            (project / "sessions" / "session-3-0.vault.jsonl").write_text(transcript(text("user", "go"), updated=10**13))
            state = project / seed.STATE_DIR[0] / seed.STATE_DIR[1]
            (state / seed.ROUTING_LEDGER).write_text(lines({"probe": {"complexity": "large"}}))
            (state / seed.ROUTE_OUTCOMES).write_text(lines({"routeKey": "route-tax"}, {"runId": "run-1"}))
            out = tmp / "seed.json"
            ran = subprocess.run(
                [sys.executable, str(REPO / "tools/routing-replay/seed.py"), "--project", str(tmp), "--out", str(out)],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(ran.returncode, 0, ran.stderr)
            written = json.loads(out.read_text())
            self.assertEqual(len(written["transcripts"]), 1, "the vault beside it is not a second transcript")
            self.assertEqual(written["transcripts"][0]["turns"], 1)
            self.assertEqual([row["probed"] for row in written["ledgers"]], [1])
            self.assertEqual([row["rows"] for row in written["outcomes"]], [2])


class ConstantsMatchTheirSource(unittest.TestCase):
    """The names the seeder carries are copies of Rust constants, and a copy
    that drifts counts another harness note, another set of spawns or
    another file as this one."""

    def declared(self, path: str, pattern: str) -> str:
        source = (REPO / path).read_text()
        found = re.search(pattern, source, re.S)
        self.assertIsNotNone(found, f"{pattern!r} is no longer found in {path}")
        return found.group(1)

    def test_a_harness_note_opens_as_the_runtime_says(self):
        opened = self.declared("zo-ide/crates/runtime/src/patch_review.rs", r'const\s+HARNESS_TAG_OPEN\s*:\s*&str\s*=\s*"([^"]+)"\s*;')
        self.assertEqual(seed.HARNESS_TAG_OPEN, opened)

    def test_the_spawn_tools_are_the_runtimes_fan_out_tools(self):
        body = self.declared("zo-ide/crates/runtime/src/conversation/tool.rs", r"fn\s+is_fan_out_tool\s*\([^)]*\)\s*->\s*bool\s*\{(.*?)\}")
        self.assertEqual(seed.SPAWN_TOOLS, tuple(re.findall(r'"([^"]+)"', body)))

    def test_the_routing_ledger_is_the_seats_own(self):
        row = self.declared("crates/zerocode-core/src/jev.rs", r"pub\s+const\s+ROUTING\s*:\s*JevUse\s*=\s*JevUse\s*\{(.*?)\n\};")
        self.assertEqual(seed.ROUTING_LEDGER, re.search(r'ledger:\s*"([^"]+)"', row).group(1))

    def test_the_outcomes_are_the_routers_own_file(self):
        outcome = "zo-ide/crates/runtime/src/model_router/outcome.rs"
        self.assertEqual(seed.ROUTE_OUTCOMES, self.declared(outcome, r'const\s+OUTCOME_FILE\s*:\s*&str\s*=\s*"([^"]+)"\s*;'))
        self.assertEqual(seed.STATE_DIR[1], self.declared(outcome, r'const\s+OUTCOME_DIR\s*:\s*&str\s*=\s*"([^"]+)"\s*;'))


if __name__ == "__main__":
    unittest.main()
