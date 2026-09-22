#!/usr/bin/env python3
"""Contract for `tools/mention-rerank-replay/seed.py` — it picks prompts and
says where they are, and nothing else.

A seed row is a store, a transcript path, a line, a working directory and
the relative path the person named. The seeder must leave sidecars alone,
must skip prompts the harness wrote, must pick only a relative path that
names a file under the prompt's own working directory, and must carry no
words: the replay reads the prompt itself, at that line, through the
product's own functions.

Run: python3 tools/tests/test_mention_rerank_replay_seed.py   (stdlib only)
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
    "mention_rerank_replay_seed", REPO / "tools" / "mention-rerank-replay" / "seed.py"
)
seed = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed
_spec.loader.exec_module(seed)


def zo_row(role: str, text: str) -> str:
    return json.dumps({"type": "message", "message": {"role": role, "blocks": [{"type": "text", "text": text}]}})


def claude_row(text, cwd: str) -> str:
    return json.dumps({"type": "user", "cwd": cwd, "message": {"role": "user", "content": text}})


class Picking(unittest.TestCase):
    def tree(self, tmp: Path) -> Path:
        work = tmp / "work"
        (work / "src" / "tui").mkdir(parents=True)
        (work / "src" / "tui" / "composer.rs").write_text("fn composer() {}\n")
        (work / "README.md").write_text("# readme\n")
        return work

    def test_a_sidecar_is_not_a_transcript(self):
        for name, is_one in [
            ("session-1-0.jsonl", True),
            ("session-1-0.vault.jsonl", False),
            ("session-1-0.rot-1789.jsonl", False),
            ("session-1-0.todos.json", False),
            ("notes.jsonl", False),
        ]:
            self.assertEqual(seed.is_zo_transcript(Path(name)), is_one, name)

    def test_only_a_relative_path_that_names_a_file_of_the_tree_is_picked(self):
        with tempfile.TemporaryDirectory() as raw:
            work = self.tree(Path(raw))
            named = seed.named_files(
                "fix the caret in @src/tui/composer.rs and `src/tui/composer.rs`; see https://x.y/a.rs, "
                "/abs/src/tui/composer.rs, src/missing.rs and README.md alone",
                work,
            )
            self.assertEqual(named, ["src/tui/composer.rs"], "once, relative, existing, with a separator")

    def test_a_prompt_the_harness_wrote_is_not_a_persons(self):
        self.assertTrue(seed.a_persons("look at src/tui/composer.rs"))
        self.assertFalse(seed.a_persons("This session is being continued from a previous conversation"))
        self.assertFalse(seed.a_persons("<tool_result>src/tui/composer.rs</tool_result>"))
        self.assertFalse(seed.a_persons("   "))

    def test_the_command_picks_both_stores_and_carries_no_words(self):
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            work = self.tree(tmp)
            zo = tmp / "zo" / "proj" / "sessions"
            zo.mkdir(parents=True)
            transcript = zo / "session-7-0.jsonl"
            transcript.write_text(
                "\n".join(
                    [
                        json.dumps({"type": "session_meta", "session_id": "session-7-0"}),
                        zo_row("user", "SENTINEL-WORD fix src/tui/composer.rs"),
                        zo_row("assistant", "ok"),
                        zo_row("user", "This session is being continued src/tui/composer.rs"),
                    ]
                )
                + "\n"
            )
            (zo / "session-7-0.cwd").write_text(json.dumps({"cwd": str(work)}))
            (zo / "session-7-0.vault.jsonl").write_text(zo_row("user", "src/tui/composer.rs") + "\n")
            claude = tmp / "claude" / "-work"
            claude.mkdir(parents=True)
            (claude / "abc.jsonl").write_text(
                claude_row([{"type": "text", "text": "OTHER-SENTINEL and src/tui/composer.rs please"}], str(work)) + "\n"
                + claude_row("no file here", str(work)) + "\n"
            )
            out = tmp / "seed.json"
            result = subprocess.run(
                [
                    sys.executable,
                    str(REPO / "tools/mention-rerank-replay/seed.py"),
                    "--zo", str(tmp / "zo"),
                    "--claude", str(tmp / "claude"),
                    "--out", str(out),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            written = json.loads(out.read_text())
            self.assertEqual(written["pageRows"], seed.PAGE_ROWS)
            rows = written["prompts"]
            self.assertEqual([row["store"] for row in rows], ["zo", "claude"])
            self.assertEqual(rows[0]["line"], 1, "the person's prompt, not the continuation")
            self.assertEqual(rows[0]["mention"], "src/tui/composer.rs")
            self.assertEqual(rows[0]["cwd"], str(work))
            self.assertEqual(rows[1]["path"], str(claude / "abc.jsonl"))
            text = out.read_text()
            self.assertNotIn("SENTINEL", text, "no words leave the transcripts")


class ConstantsMatchTheirSource(unittest.TestCase):
    """The number the seeder carries is a copy of a Rust constant, and a copy
    that drifts is a seed made for another page."""

    def test_the_page_is_the_seats_own_candidate_cap(self):
        text = (REPO / "crates/zerocode-core/src/jev.rs").read_text()
        found = re.search(r"const\s+MENTION_CANDIDATE_CAP\s*:[^=]*=\s*([0-9_]+)\s*;", text)
        self.assertIsNotNone(found, "MENTION_CANDIDATE_CAP is no longer declared")
        self.assertEqual(seed.PAGE_ROWS, int(found.group(1).replace("_", "")))


if __name__ == "__main__":
    unittest.main()
