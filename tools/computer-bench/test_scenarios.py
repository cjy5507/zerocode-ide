"""tools/computer-bench/scenarios.py — each oracle, setup and fixture kind, and
the reference expander, on files and a scripted shim alone."""
import hashlib
import json
import os
import tempfile
import unittest

import scenarios
import tally


def shim_saying(answers):
    """A shim that answers each verb from `answers` (a result, or None to refuse)."""
    calls = []

    def shim(argv):
        calls.append(argv)
        result = answers.get(argv[0])
        if callable(result):
            result = result(argv)
        return None if result is None else {"ok": True, "result": result}

    shim.calls = calls
    return shim


class Oracles(unittest.TestCase):
    def test_file_text_passes_only_the_exact_sentence(self):
        sentence = "The quick brown fox jumps over 13 lazy dogs."
        spec = {"kind": "file_text", "path": "note.txt", "expect": sentence}
        with tempfile.TemporaryDirectory() as work:
            path = os.path.join(work, "note.txt")
            for body, passes in ((sentence, True), (sentence + "\n", True), (sentence + "\n\n", False),
                                 (sentence.replace("quick", "“quick”"), False), ("{\\rtf1 " + sentence + "}", False)):
                with open(path, "w", encoding="utf-8") as handle:
                    handle.write(body)
                passed, detail = scenarios.check(work, spec, shim_saying({}), "", {})
                self.assertEqual(passed, passes, f"{body!r}: {detail}")
            os.remove(path)
            passed, detail = scenarios.check(work, spec, shim_saying({}), "", {})
            self.assertFalse(passed)
            self.assertIn("note.txt", detail, "a failure says why")

    def test_display_digits_reads_through_grouping(self):
        spec = {"kind": "display_digits", "app": "Calculator", "expect": "7006652"}
        for shown, passes in (("3 static text 7,006,652", True), ("\t\t4 text 7 006 652", True), ("2 text 7.006.652", True),
                              ("2 text 7006651", False), ("5 text 1234×5678", False), ("6 button 7", False)):
            shim = shim_saying({"read": {"text": shown}})
            passed, _detail = scenarios.check("", spec, shim, "", {})
            self.assertEqual(passed, passes, shown)
        ocr_only = shim_saying({"read": lambda argv: {"text": "7,006,652"} if "--ocr" in argv else {"text": "1 window Calculator"}})
        self.assertTrue(scenarios.check("", spec, ocr_only, "", {})[0], "the pixels are read when the tree does not show it")
        answer = {"kind": "answer_digits", "expect": "7006652"}
        self.assertTrue(scenarios.check("", answer, shim_saying({}), "The answer is 7,006,652.", {})[0])
        self.assertFalse(scenarios.check("", answer, shim_saying({}), "About seven million.", {})[0])

    def test_tree_needs_every_file_where_it_belongs_and_nothing_else(self):
        specs = [
            {"kind": "pdf", "path": "tidy/inbox/r1.pdf", "title": "receipt 1"},
            {"kind": "text", "path": "tidy/inbox/notes.txt", "text": "notes\n"},
        ]
        spec = {"kind": "tree", "root": "tidy", "expect": {
            "inbox": "dir", "inbox/notes.txt": "fixture:tidy/inbox/notes.txt",
            "receipts": "dir", "receipts/r1.pdf": "fixture:tidy/inbox/r1.pdf"}}
        with tempfile.TemporaryDirectory() as work:
            hashes = scenarios.make_fixtures(work, specs)
            self.assertFalse(scenarios.check(work, spec, None, "", hashes)[0], "nothing moved yet")
            os.makedirs(os.path.join(work, "tidy", "receipts"))
            os.rename(os.path.join(work, "tidy/inbox/r1.pdf"), os.path.join(work, "tidy/receipts/r1.pdf"))
            with open(os.path.join(work, "tidy", "inbox", scenarios.DS_STORE), "w") as handle:
                handle.write("finder")
            passed, detail = scenarios.check(work, spec, None, "", hashes)
            self.assertTrue(passed, detail)
            with open(os.path.join(work, "tidy/receipts/r1.pdf"), "ab") as handle:
                handle.write(b"%")
            passed, detail = scenarios.check(work, spec, None, "", hashes)
            self.assertFalse(passed)
            self.assertIn("changed: receipts/r1.pdf", detail)
            with open(os.path.join(work, "tidy/stray.txt"), "w") as handle:
                handle.write("x")
            self.assertIn("extra: stray.txt", scenarios.check(work, spec, None, "", hashes)[1])

    def test_no_window_and_the_whole_oracle(self):
        shim = shim_saying({"list-all-windows": {"windows": [{"id": 3, "app": {"name": "TextEdit", "pid": 9}, "title": "note.txt — Edited"}]}})
        spec = {"kind": "no_window", "app": "TextEdit", "title": "note.txt"}
        self.assertFalse(scenarios.check("", spec, shim, "", {})[0])
        passed, detail = scenarios.check("", spec, shim_saying({"list-all-windows": None}), "", {})
        self.assertEqual((passed, detail), (False, "could not list the windows"), "a look that failed is not an empty desk")
        verdict = scenarios.oracle("", [spec, {"kind": "answer_digits", "expect": "1"}], shim, "1", {})
        self.assertEqual((verdict["pass"], [check["pass"] for check in verdict["checks"]]), (False, [False, True]))


class Kinds(unittest.TestCase):
    def test_the_reference_expander_fills_every_loop(self):
        lines = scenarios.expand([["activate", "--app", "Finder"],
                                  {"for": "i", "in": ["1", "2"], "do": [["type", "--text", "r-{i}"], ["key", "--key", "cmd+c"]]}])
        self.assertEqual(lines, [["activate", "--app", "Finder"], ["type", "--text", "r-1"], ["key", "--key", "cmd+c"],
                                 ["type", "--text", "r-2"], ["key", "--key", "cmd+c"]])

    def test_fixtures_are_the_same_bytes_every_time(self):
        with tempfile.TemporaryDirectory() as one, tempfile.TemporaryDirectory() as two:
            specs = [{"kind": "pdf", "path": "a.pdf", "title": "receipt 1"}, {"kind": "png1", "path": "p.png"}, {"kind": "text", "path": "t.txt", "text": "x\n"}]
            self.assertEqual(scenarios.make_fixtures(one, specs), scenarios.make_fixtures(two, specs))
            with open(os.path.join(one, "a.pdf"), "rb") as handle:
                self.assertTrue(handle.read().startswith(b"%PDF-1.4"))
            with open(os.path.join(one, "t.txt"), "rb") as handle:
                self.assertEqual(hashlib.sha256(handle.read()).hexdigest(), scenarios.make_fixtures(one, specs[2:])["t.txt"])

    def test_setup_reads_before_it_is_judged_and_says_why_it_failed(self):
        values = {"setup_timeout_ms": 1000}
        basic = shim_saying({"read": {"text": "0 standard window Calculator\n\t1 text 0\n\t2 button AC"}})
        scenarios.setup("", [{"kind": "read_lacks", "app": "Calculator", "words": ["sin", "RPN"]},
                             {"kind": "expect_display", "app": "Calculator", "digits": "0"}], basic, values, {}, lambda: None)
        scientific = shim_saying({"read": {"text": "1 window Calculator\n2 button sin\n3 text 0"}})
        with self.assertRaises(scenarios.SetupFailed) as failed:
            scenarios.setup("", [{"kind": "read_lacks", "app": "Calculator", "words": ["sin"]}], scientific, values, {}, lambda: None)
        self.assertIn("sin", str(failed.exception))
        no_window = shim_saying({"wait-for": None})
        with self.assertRaises(scenarios.SetupFailed):
            scenarios.setup("", [{"kind": "wait_window", "app": "TextEdit", "title": "note.txt"}], no_window, values, {}, lambda: None)

    def test_a_calculator_that_kept_its_answer_is_cleared_or_not_set_up(self):
        # Calculator restores its last value on launch. The tree's first line
        # is element 0 and the keys have a 0 on them: neither is the display.
        values = {"setup_timeout_ms": 1000}
        steps = [{"kind": "clear", "app": "Calculator", "key": "escape", "times": 3},
                 {"kind": "expect_display", "app": "Calculator", "digits": "0"},
                 {"kind": "display_lacks", "app": "Calculator", "digits": "7006652"}]
        restored = "0 standard window Calculator\n\t4 text 7,006,652\n\t9 button 0"
        cleared = "0 standard window Calculator\n\t4 text 0\n\t9 button 0"

        def calculator(clears_after):
            presses = []
            shim = shim_saying({"activate": {}, "key": lambda argv: presses.append(argv) or {},
                                "read": lambda argv: {"source": "ocr", "lines": []} if "--ocr" in argv
                                else {"text": cleared if len(presses) >= clears_after else restored}})
            return shim, presses

        guarded = []
        shim, presses = calculator(clears_after=3)
        scenarios.setup("", steps, shim, values, {}, lambda: guarded.append(1))
        self.assertEqual([argv for argv in shim.calls if argv[0] in ("activate", "key")],
                         [["activate", "--app", "Calculator"]] + [["key", "--key", "escape"]] * 3)
        self.assertEqual(len(guarded), 4, "the guard runs before every act")
        shim, _presses = calculator(clears_after=99)
        with self.assertRaises(scenarios.SetupFailed) as failed:
            scenarios.setup("", steps, shim, values, {}, lambda: None)
        self.assertIn("does not show 0", str(failed.exception), "the 0 on a key and element 0 never pass for a clear display")
        ocr_shows = shim_saying({"read": lambda argv: {"source": "ocr", "lines": [{"text": "7,006,652"}]} if "--ocr" in argv else {"text": cleared}})
        with self.assertRaises(scenarios.SetupFailed) as failed:
            scenarios.setup("", steps[2:], ocr_shows, values, {}, lambda: None)
        self.assertIn("already shows 7006652 (ocr)", str(failed.exception))

    def test_the_desk_must_be_as_the_bench_left_it(self):
        row = {"apps_not_running": [], "stale_window_titles": ["note.txt"]}
        scenarios.desk_ready(row, shim_saying({"list-all-windows": {"windows": []}}))
        for windows, why in ((None, "could not list the windows"),
                             ({"windows": [{"id": 1, "app": {"name": "TextEdit"}, "title": "note.txt"}]}, "already open")):
            with self.assertRaises(scenarios.SetupFailed) as failed:
                scenarios.desk_ready(row, shim_saying({"list-all-windows": windows}))
            self.assertIn(why, str(failed.exception))

    def test_teardown_closes_its_windows_and_removes_only_its_own_work(self):
        with tempfile.TemporaryDirectory() as root:
            work = os.path.join(root, "session", "s-1")
            os.makedirs(work)
            shim = shim_saying({"list-all-windows": {"windows": [{"id": 7, "app": {"name": "Finder"}, "title": "bench-inbox"}]}, "window-close": {}})
            left = scenarios.teardown(work, [{"kind": "close_windows", "app": "Finder", "titles": ["bench-inbox"]}], shim, {"quit_wait_ms": 10}, {}, root)
            self.assertEqual(left, [])
            self.assertFalse(os.path.exists(work))
            self.assertIn(["window-close", "--id", "7"], shim.calls)
            outside = tempfile.mkdtemp()
            scenarios.remove_work(outside, root)
            self.assertTrue(os.path.isdir(outside), "nothing outside the bench's root is removed")
            os.rmdir(outside)
            os.makedirs(work)
            left = scenarios.teardown(work, [{"kind": "close_windows", "app": "Finder", "titles": ["bench-inbox"]}],
                                      shim_saying({"list-all-windows": None}), {"quit_wait_ms": 10}, {}, root)
            self.assertEqual(left, ["could not list the windows to close bench-inbox"])

    def test_the_claim_is_the_last_result_line(self):
        pattern = tally.table()["self_result_pattern"]
        self.assertEqual(scenarios.claim("done\nRESULT: FAIL no window\nRESULT: PASS", pattern), "PASS")
        self.assertIsNone(scenarios.claim("I think it worked.", pattern))


if __name__ == "__main__":
    unittest.main()
