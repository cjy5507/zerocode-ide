#!/usr/bin/env python3
"""Contract for `tools/compaction-replay/seed.py` — it picks transcripts and
counts, and nothing else.

A seed row is a path, a message count, the model, the recorded cuts and
whether a vault sits beside the transcript. The seeder must leave sidecars
alone (a vault, a rotated fragment, a todo store share the sessions
directory and the `.jsonl` extension), must read a recorded compaction's
`first_kept_message_index` as the cut it was, and must carry no words: the
replay reads the transcript's body itself, at the cut, through the product's
own functions.

Run: python3 tools/tests/test_compaction_replay_seed.py   (stdlib only)
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
_spec = importlib.util.spec_from_file_location(
    "compaction_replay_seed", REPO / "tools" / "compaction-replay" / "seed.py"
)
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)


def message(role: str, text: str | None = None, model: str | None = None) -> dict:
    blocks = [{"type": "text", "text": text}] if text is not None else [
        {"type": "tool_result", "tool_use_id": "t1", "tool_name": "Read", "output": "x" * 300, "is_error": False}
    ]
    held = {"type": "message", "message": {"role": role, "blocks": blocks}}
    if model:
        held["message"]["model"] = model
    return held


def transcript(messages: int, *, model: str | None = None, cuts: list[int] = ()) -> str:
    rows = [{"type": "session_meta", "version": 1, "session_id": "session-1-0", "created_at_ms": 1, "updated_at_ms": 1}]
    for n in range(messages):
        if n % 2 == 0:
            rows.append(message("user", f"turn {n}"))
        else:
            rows.append(message("assistant", "ok", model=model))
    for cut in cuts:
        rows.append({"type": "compaction", "count": 1, "removed_message_count": cut, "summary": "s", "first_kept_message_index": cut})
    return "".join(json.dumps(row) + "\n" for row in rows)


class Picking(unittest.TestCase):
    def sessions(self, tmp: Path) -> Path:
        path = tmp / "proj" / "sessions"
        path.mkdir(parents=True, exist_ok=True)
        return path

    def test_a_sidecar_is_not_a_transcript(self):
        for name, is_one in [
            ("session-1-0.jsonl", True),
            ("session-1-0.vault.jsonl", False),
            ("session-1-0.rot-1789.jsonl", False),
            ("session-1-0.todos.json", False),
            ("notes.jsonl", False),
        ]:
            self.assertEqual(seed.is_transcript(Path(name)), is_one, name)

    def test_a_transcript_too_short_for_a_boundary_is_left_out(self):
        with tempfile.TemporaryDirectory() as raw:
            sessions = self.sessions(Path(raw))
            short = sessions / "session-1-0.jsonl"
            short.write_text(transcript(seed.MIN_COMPACTABLE_MESSAGES + seed.SMALLEST_TAIL - 1))
            self.assertIsNone(seed.describe(short))
            long = sessions / "session-2-0.jsonl"
            long.write_text(transcript(seed.MIN_COMPACTABLE_MESSAGES + seed.SMALLEST_TAIL))
            self.assertIsNotNone(seed.describe(long))

    def test_a_row_carries_counts_the_model_and_the_recorded_cuts_and_no_words(self):
        with tempfile.TemporaryDirectory() as raw:
            sessions = self.sessions(Path(raw))
            path = sessions / "session-3-0.jsonl"
            path.write_text(transcript(20, model="claude-opus-5", cuts=[7, 15]))
            (sessions / "session-3-0.vault.jsonl").write_text("")
            row = seed.describe(path)
            self.assertEqual(row["messages"], 20)
            self.assertEqual(row["model"], "claude-opus-5")
            self.assertEqual(row["recordedCuts"], [7, 15])
            self.assertEqual(row["userTurns"], 10)
            self.assertTrue(row["vault"])
            self.assertNotIn("turn 0", json.dumps(row), "no words leave the transcript")

    def test_half_a_line_is_skipped_not_fatal(self):
        with tempfile.TemporaryDirectory() as raw:
            sessions = self.sessions(Path(raw))
            path = sessions / "session-4-0.jsonl"
            path.write_text('"role": "user"}\n' + transcript(14))
            self.assertEqual(seed.describe(path)["messages"], 14)

    def test_the_command_writes_the_window_and_the_rows(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            sessions = self.sessions(tmp)
            (sessions / "session-5-0.jsonl").write_text(transcript(16, cuts=[6]))
            (sessions / "session-5-0.vault.jsonl").write_text("")
            out = tmp / "seed.json"
            result = subprocess.run(
                [sys.executable, str(REPO / "tools/compaction-replay/seed.py"), "--project", str(tmp), "--out", str(out)],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            written = json.loads(out.read_text())
            self.assertEqual(written["regretTurns"], seed.REGRET_TURNS)
            self.assertEqual(len(written["transcripts"]), 1, "the vault beside it is not a second transcript")
            self.assertEqual(written["transcripts"][0]["recordedCuts"], [6])


class ConstantsMatchTheirSource(unittest.TestCase):
    """The numbers the seeder carries are copies of Rust constants, and a copy
    that drifts is a seed made for another boundary or another window."""

    def named(self, path: str, name: str) -> int:
        text = (REPO / path).read_text()
        found = re.search(rf"const\s+{name}\s*:[^=]*=\s*([0-9_]+)\s*;", text)
        self.assertIsNotNone(found, f"{name} is no longer declared in {path}")
        return int(found.group(1).replace("_", ""))

    def test_the_boundary_floor_is_the_compactors_own(self):
        self.assertEqual(
            seed.MIN_COMPACTABLE_MESSAGES,
            self.named("zo-ide/crates/runtime/src/compact/mod.rs", "MIN_COMPACTABLE_MESSAGES"),
        )

    def test_the_window_is_the_seats_own_regret_turns(self):
        self.assertEqual(
            seed.REGRET_TURNS,
            self.named("crates/zerocode-core/src/jev.rs", "COMPACTION_REGRET_TURNS"),
        )

    def test_the_smallest_tail_is_the_configs_default(self):
        text = (REPO / "zo-ide/crates/runtime/src/compact/mod.rs").read_text()
        default = text[text.index("impl Default for CompactionConfig") :]
        found = re.search(r"preserve_recent_messages:\s*([0-9_]+)", default)
        self.assertIsNotNone(found)
        self.assertEqual(seed.SMALLEST_TAIL, int(found.group(1)))


if __name__ == "__main__":
    unittest.main()
