import importlib.util
import json
import re
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
REPO = ROOT.parent
SPEC = importlib.util.spec_from_file_location("command_guard_replay_seed", ROOT / "command-guard-replay" / "seed.py")
seed = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(seed)


def rust_const(path: Path, name: str) -> str:
    found = re.search(rf"pub const {name}: \w+ = ([^;]+);", path.read_text())
    assert found, name
    return found.group(1).strip()


class ConstantsMatchTheirSource(unittest.TestCase):
    def test_the_caps_are_the_use_tables(self):
        jev = REPO / "crates" / "zerocode-core" / "src" / "jev.rs"
        self.assertEqual(rust_const(jev, "TOOL_TEXT_GUARD_TEXT_CHAR_CAP"), "ROUTING_TASK_CHAR_CAP")
        self.assertEqual(rust_const(jev, "ROUTING_TASK_CHAR_CAP").replace("_", ""), str(seed.TEXT_CHAR_CAP))
        self.assertEqual(rust_const(jev, "COMMAND_GUARD_COMMAND_CHAR_CAP"), "RECALL_REQUEST_CHAR_CAP")
        self.assertEqual(rust_const(jev, "RECALL_REQUEST_CHAR_CAP").replace("_", ""), str(seed.COMMAND_CHAR_CAP))

    def test_the_sources_are_the_runtimes_words(self):
        guard = (REPO / "zo-ide" / "crates" / "runtime" / "src" / "tool_guard.rs").read_text()
        words = re.findall(r'Self::\w+ => "(\w+)",', guard)
        self.assertEqual(tuple(words), seed.SOURCES)


class TheSetsAreWhatTheyClaim(unittest.TestCase):
    def test_each_set_holds_at_least_sixty_cases(self):
        irreversible, safe = seed.command_cases()
        self.assertGreaterEqual(len(irreversible), seed.MIN_CASES)
        self.assertGreaterEqual(len(safe), seed.MIN_CASES)
        self.assertGreaterEqual(len(seed.injected_cases(64)), seed.MIN_CASES)
        self.assertGreaterEqual(len(seed.plain_cases(REPO, 64)), seed.MIN_CASES)

    def test_every_irreversible_case_names_what_it_does(self):
        irreversible, safe = seed.command_cases()
        for case in irreversible:
            self.assertTrue(case["expect"], case["id"])
            self.assertTrue(set(case["expect"]) <= {"irreversible", "outside"}, case["id"])
        self.assertTrue(all(case["expect"] == [] for case in safe))
        commands = [case["command"] for case in irreversible + safe]
        self.assertEqual(len(commands), len(set(commands)), "no command twice")
        self.assertTrue(all(len(command) <= seed.COMMAND_CHAR_CAP for command in commands))

    def test_every_injected_block_carries_its_order_inside_the_head(self):
        cases = seed.injected_cases(64)
        pairs = set()
        for at, case in enumerate(cases):
            source, carrier = seed.CARRIERS[at % len(seed.CARRIERS)]
            self.assertEqual(case["source"], source)
            order = next(order for order in seed.ORDERS if order.replace('"', "'") in case["text"] or order in case["text"])
            pairs.add((carrier, order))
            self.assertLessEqual(len(case["text"]), seed.TEXT_CHAR_CAP)
        self.assertEqual(len(pairs), len(cases), "no carrier meets the same order twice")
        used = {order for _, order in pairs}
        self.assertEqual(used, set(seed.ORDERS), "every order is asked")
        self.assertEqual({case["source"] for case in cases}, set(seed.SOURCES))

    def test_the_plain_set_is_tracked_files_by_path_and_never_their_words(self):
        cases = seed.plain_cases(REPO, 64)
        for case in cases:
            self.assertNotIn("text", case)
            path = case["path"]
            self.assertTrue((REPO / path).is_file(), path)
            self.assertFalse(any(part in path for part in seed.PLAIN_EXCLUDED_PARTS), path)
            self.assertNotIn(Path(path).name, seed.PLAIN_EXCLUDED_NAMES)
        self.assertEqual(sum(case["source"] == "browser" for case in cases), len(cases) // seed.FENCED_PLAIN_EVERY)


class NoOnesWordsRideIn(unittest.TestCase):
    def test_every_path_is_nobodys_and_every_address_reserved(self):
        irreversible, safe = seed.command_cases()
        written = json.dumps([irreversible, safe, seed.injected_cases(64)], ensure_ascii=False)
        for home in re.findall(r"/Users/([\w.-]+)", written):
            self.assertEqual(home, "dev")
        for host in re.findall(r"https?://([\w.-]+)", written):
            self.assertTrue(host.endswith(".invalid"), host)
        # A mailbox's domain ends in letters; `package@1.1.22` is a version.
        for mailbox in re.findall(r"[\w.+-]+@((?:[\w-]+\.)+[a-z]{2,})\b", written):
            self.assertTrue(mailbox.endswith(".invalid"), mailbox)

    def test_the_seed_extracts_and_computes_nothing(self):
        with tempfile.TemporaryDirectory() as temporary:
            built = seed.seed(REPO, [Path(temporary)], 7, 64)
        self.assertEqual(built["transcripts"], [])
        self.assertEqual(
            set(built),
            {"extractedAt", "textCharCap", "commandCharCap", "irreversible", "safe", "injected", "plain", "repo", "lookbackDays", "transcripts"},
            "cases and locators only: every number is the harness's",
        )


if __name__ == "__main__":
    unittest.main()
