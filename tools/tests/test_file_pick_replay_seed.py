import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "file_pick_replay_seed", ROOT / "file-pick-replay" / "seed.py"
)
seed = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(seed)


class FilePickReplaySeedTests(unittest.TestCase):
    def test_seed_row_counts_edits_without_copying_prompt_or_tool_text(self):
        with tempfile.TemporaryDirectory() as temporary:
            transcript = Path(temporary) / "session-1790000000000-0.jsonl"
            rows = [
                {
                    "type": "message",
                    "message": {
                        "role": "user",
                        "model": "test-model",
                        "blocks": [{"type": "text", "text": "REQUEST_SENTINEL_FIXTURE"}],
                    },
                },
                {
                    "type": "message",
                    "message": {
                        "role": "tool",
                        "blocks": [
                            {
                                "type": "tool_result",
                                "tool_name": "edit_file",
                                "is_error": False,
                                "output": "FILE_BODY_SENTINEL_FIXTURE",
                            }
                        ],
                    },
                },
            ]
            transcript.write_text("\n".join(json.dumps(row) for row in rows) + "\n")

            row = seed.describe(transcript, Path(temporary))

        self.assertIsNotNone(row)
        self.assertEqual(row["messages"], 2)
        self.assertEqual(row["edit_results"], 1)
        self.assertEqual(row["model"], "test-model")
        rendered = json.dumps(row)
        self.assertNotIn("REQUEST_SENTINEL_FIXTURE", rendered)
        self.assertNotIn("FILE_BODY_SENTINEL_FIXTURE", rendered)
        self.assertNotIn("filePath", rendered)

    def test_only_successful_edit_results_count(self):
        with tempfile.TemporaryDirectory() as temporary:
            transcript = Path(temporary) / "session-1790000000000-0.jsonl"
            row = {
                "type": "message",
                "message": {
                    "role": "tool",
                    "blocks": [
                        {"type": "tool_result", "tool_name": "edit_file", "is_error": True},
                        {"type": "tool_result", "tool_name": "Read", "is_error": False},
                    ],
                },
            }
            transcript.write_text(json.dumps(row) + "\n")

            self.assertIsNone(seed.describe(transcript, Path(temporary)))


if __name__ == "__main__":
    unittest.main()
