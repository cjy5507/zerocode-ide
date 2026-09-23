import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "vault-pairs-replay" / "seed.py"
spec = importlib.util.spec_from_file_location("vault_pairs_seed", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SeedTest(unittest.TestCase):
    def test_only_later_review_is_a_label_and_paths_are_hashed(self):
        with tempfile.TemporaryDirectory() as folder:
            ledger = Path(folder) / "vault-pairs.jsonl"
            entries = [
                {"kind": "label", "at": 2, "left": "wiki/a.md", "right": "wiki/b.md",
                 "leftModifiedMs": 1, "rightModifiedMs": 1, "actual": "none"},
                {"at": 3, "left": "wiki/a.md", "right": "wiki/b.md",
                 "leftModifiedMs": 1, "rightModifiedMs": 1, "outcome": "answered",
                 "proposal": "merge", "inputTokens": 100, "requests": 1},
                {"kind": "label", "at": 4, "left": "wiki/a.md", "right": "wiki/b.md",
                 "leftModifiedMs": 2, "rightModifiedMs": 1, "actual": "merge"},
            ]
            ledger.write_text("\n".join(json.dumps(row) for row in entries))
            result = module.seed(ledger)
            self.assertIsNone(result["samples"][0]["actual"])
            self.assertNotIn("wiki/a.md", json.dumps(result))
            self.assertEqual(result["samples"][0]["input_tokens"], 100)


if __name__ == "__main__":
    unittest.main()
