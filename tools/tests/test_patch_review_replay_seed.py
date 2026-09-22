#!/usr/bin/env python3
"""Contract for `tools/patch-review-replay/seed.py` — it picks transcripts and
counts, and nothing else.

A seed row is a path, a message count, the model, how many successful edit
results the transcript holds and whether a vault sits beside it. The seeder
must read which files are transcripts the way compaction-replay does (it
loads that reading rather than copying it), must count a failed edit and a
non-mutation as nothing, and must carry no words: the replay reads the
transcript's body itself, patch by patch, through the product's own
functions.

Run: python3 tools/tests/test_patch_review_replay_seed.py   (stdlib only)
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
_spec = importlib.util.spec_from_file_location(
    "patch_review_replay_seed", REPO / "tools" / "patch-review-replay" / "seed.py"
)
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)


def text(role: str, words: str, model: str | None = None) -> dict:
    held = {"type": "message", "message": {"role": role, "blocks": [{"type": "text", "text": words}]}}
    if model:
        held["message"]["model"] = model
    return held


def result(tool: str, *, error: bool = False) -> dict:
    output = json.dumps({"filePath": "/w/a.rs", "structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "lines": ["-a", "+b"]}]})
    return {
        "type": "message",
        "message": {
            "role": "tool",
            "blocks": [{"type": "tool_result", "tool_use_id": "t1", "tool_name": tool, "output": output, "is_error": error}],
        },
    }


def transcript(*rows: dict) -> str:
    head = {"type": "session_meta", "version": 1, "session_id": "session-1-0", "created_at_ms": 1, "updated_at_ms": 1}
    return "".join(json.dumps(row) + "\n" for row in (head, *rows))


class Picking(unittest.TestCase):
    def sessions(self, tmp: Path) -> Path:
        path = tmp / "proj" / "sessions"
        path.mkdir(parents=True, exist_ok=True)
        return path

    def test_which_files_are_transcripts_is_compaction_replays_reading(self):
        compaction = sys.modules["compaction_replay_seed"]
        self.assertIs(seed.picker, compaction)
        self.assertTrue(seed.picker.is_transcript(Path("session-1-0.jsonl")))
        self.assertFalse(seed.picker.is_transcript(Path("session-1-0.vault.jsonl")))

    def test_a_mutation_is_the_runtimes_mutation(self):
        for name, is_one in [
            ("edit_file", True),
            ("write_file", True),
            ("MultiEdit", True),
            ("mcp__fs__write_file", True),
            ("read_file", False),
            ("bash", False),
            ("write_file_notes", False),
        ]:
            self.assertEqual(seed.is_edit(name), is_one, name)

    def test_a_transcript_without_an_edit_is_left_out(self):
        with tempfile.TemporaryDirectory() as raw:
            sessions = self.sessions(Path(raw))
            path = sessions / "session-1-0.jsonl"
            path.write_text(transcript(text("user", "just look"), result("read_file"), result("edit_file", error=True)))
            self.assertIsNone(seed.describe(path), "a read and a failed edit wrote nothing")

    def test_a_row_carries_counts_and_no_words(self):
        with tempfile.TemporaryDirectory() as raw:
            sessions = self.sessions(Path(raw))
            path = sessions / "session-2-0.jsonl"
            path.write_text(
                transcript(
                    text("user", "rename the flag"),
                    text("assistant", "on it", model="claude-opus-5"),
                    result("edit_file"),
                    result("MultiEdit"),
                    result("edit_file", error=True),
                )
            )
            (sessions / "session-2-0.vault.jsonl").write_text("")
            row = seed.describe(path)
            self.assertEqual(row["edits"], 2)
            self.assertEqual(row["messages"], 5)
            self.assertEqual(row["model"], "claude-opus-5")
            self.assertTrue(row["vault"])
            self.assertNotIn("rename the flag", json.dumps(row), "no words leave the transcript")
            self.assertNotIn("/w/a.rs", json.dumps(row), "no path a tool printed leaves either")

    def test_the_command_writes_the_window_and_the_rows(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            sessions = self.sessions(tmp)
            (sessions / "session-3-0.jsonl").write_text(transcript(text("user", "go"), result("write_file")))
            (sessions / "session-3-0.vault.jsonl").write_text("")
            out = tmp / "seed.json"
            ran = subprocess.run(
                [sys.executable, str(REPO / "tools/patch-review-replay/seed.py"), "--project", str(tmp), "--out", str(out)],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(ran.returncode, 0, ran.stderr)
            written = json.loads(out.read_text())
            self.assertEqual(written["regretTurns"], seed.REGRET_TURNS)
            self.assertEqual(len(written["transcripts"]), 1, "the vault beside it is not a second transcript")
            self.assertEqual(written["transcripts"][0]["edits"], 1)


class ConstantsMatchTheirSource(unittest.TestCase):
    """The numbers and names the seeder carries are copies of Rust constants,
    and a copy that drifts is a seed made for another window or another set
    of tools."""

    def named(self, path: str, name: str) -> int:
        source = (REPO / path).read_text()
        found = re.search(rf"const\s+{name}\s*:[^=]*=\s*([0-9_]+)\s*;", source)
        self.assertIsNotNone(found, f"{name} is no longer declared in {path}")
        return int(found.group(1).replace("_", ""))

    def listed(self, path: str, name: str) -> tuple[str, ...]:
        source = (REPO / path).read_text()
        found = re.search(rf"const\s+{name}\s*:[^=]*=\s*&\[(.*?)\];", source, re.S)
        self.assertIsNotNone(found, f"{name} is no longer declared in {path}")
        return tuple(re.findall(r'"([^"]+)"', found.group(1)))

    def test_the_window_is_the_seats_own_regret_turns(self):
        self.assertEqual(seed.REGRET_TURNS, self.named("crates/zerocode-core/src/jev.rs", "PATCH_REVIEW_REGRET_TURNS"))

    def test_the_mutation_tools_are_the_runtimes_own(self):
        runtime = "zo-ide/crates/runtime/src/compact/mod.rs"
        self.assertEqual(seed.EDIT_RESULT_TOOL_NAMES, self.listed(runtime, "EDIT_RESULT_TOOL_NAMES"))
        self.assertEqual(seed.EDIT_RESULT_TOOL_LEAF_VERBS, self.listed(runtime, "EDIT_RESULT_TOOL_LEAF_VERBS"))


if __name__ == "__main__":
    unittest.main()
