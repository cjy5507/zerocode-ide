"""What the walk-judgment probe's driver promises: the build before sees the
harness without its after-only lines, a step is timed from its look to its
hand with the stand-in taken out, success is the page's own word, and the
table reads every row it was given."""

import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import run  # noqa: E402


def call(verb, start, ms):
    return {"verb": verb, "startMs": start, "ms": ms, "exit": 0}


class DriverTests(unittest.TestCase):
    def test_the_build_before_sees_the_harness_without_its_after_only_lines(self):
        source = (
            "keep one\n"
            "    // after-only {\n"
            "    let world = world.writing(x);\n"
            "    // after-only }\n"
            "keep two\n"
            "// after-only {\n"
            "fn value_writer() {}\n"
            "// after-only }\n"
        )
        self.assertEqual(run.strip_after_only(source), "keep one\nkeep two\n")
        # The harness itself carries balanced regions.
        probe = run.PROBE.read_text()
        self.assertEqual(probe.count("// after-only {"), probe.count("// after-only }"))
        stripped = run.strip_after_only(probe)
        self.assertFalse(
            [line for line in stripped.splitlines() if line.strip().startswith("// after-only")],
            "a region marker survived",
        )
        for needed_after in ("LiveWriter", "value_writer", "ZEROCODE_WALK_PROBE_VALUE_KEY",
                             "BROWSER_SETTLE_LATER_FLAG", "settle_said", "walked.cancelled",
                             "held"):
            self.assertNotIn(needed_after, stripped)
        # Both builds stand the same settling door in front of the window.
        for both in ("settle_by_eval", "struct Door", "struct Counting", "ZEROCODE_WALK_PROBE_OVERLAP"):
            self.assertIn(both, stripped)

    def test_a_step_runs_from_its_look_to_its_hand_without_the_stand_in(self):
        row = {
            "calls": [
                call("marks", 0, 30), call("click", 300, 40), call("type", 900, 30),
                call("find", 940, 20),
                call("marks", 970, 30), call("click", 1_250, 30), call("find", 1_290, 20),
            ],
            "standInMs": [50.0, 40.0],
        }
        steps = run.steps_of(row)
        self.assertEqual([step["kind"] for step in steps], ["type", "press"])
        self.assertAlmostEqual(steps[0]["ms"], 930 - 50)
        self.assertAlmostEqual(steps[1]["ms"], 1_280 - 970 - 40)
        # A look with no hand after it is no step.
        self.assertEqual(run.steps_of({"calls": [call("marks", 0, 30)]}), [])

    def test_a_walk_that_settles_later_is_timed_by_the_same_step_and_by_the_gap_between_hands(self):
        def inside(verb, start, ms, **what):
            said = call(verb, start, ms)
            said["inside"] = what
            return said
        # A press that left its settle for later: the look that finishes it
        # opens the next step; the stand-in snapshot inside any call is taken
        # out of both the step and the gap, the settle is not.
        row = {
            "calls": [
                inside("marks", 0, 30, standInMs=10.0),
                inside("click", 250, 60, previewMs=30.0, standInMs=10.0),
                inside("marks", 310, 120, settleMs=90.0, settle="ready", standInMs=10.0),
                call("find", 430, 20),
                inside("click", 520, 60, previewMs=30.0, standInMs=10.0),
                inside("marks", 580, 120, settleMs=88.0, settle="ready", standInMs=10.0),
                call("find", 700, 20),
            ],
        }
        steps = run.steps_of(row)
        self.assertEqual([step["kind"] for step in steps], ["press", "press"])
        self.assertAlmostEqual(steps[0]["ms"], 310 - 0 - 20)
        self.assertAlmostEqual(steps[1]["ms"], 580 - 310 - 20)
        self.assertEqual(run.press_gaps(row), [580 - 310 - 20])
        # A refused press is no hand.
        refused = {"calls": [call("click", 0, 10), dict(call("click", 50, 10), exit=1), call("click", 90, 10)]}
        self.assertEqual(run.press_gaps(refused), [90])

    def test_the_arms_name_a_build_and_whether_it_asks_ahead(self):
        self.assertEqual(run.ARMS["before"], ("before", False))
        self.assertEqual(run.ARMS["after"], ("after", False))
        self.assertEqual(run.ARMS["before-ahead"], ("before", True))
        self.assertEqual(run.ARMS["after-ahead"], ("after", True))
        self.assertEqual(run.SCENARIOS["steps"]["page"], "steps.html")
        self.assertIn('id="next"', (run.HERE / "steps.html").read_text(), "the steps page is ready at #next")
        self.assertEqual(run.SCENARIOS["steps"]["ready"], "#next")
        self.assertTrue((run.HERE / "steps.html").exists())

    def test_the_later_walk_is_the_steps_walk_on_the_page_answering_after_its_press(self):
        later, steps = run.SCENARIOS["later"], run.SCENARIOS["steps"]
        self.assertEqual({key: value for key, value in later.items() if key != "query"}, steps)
        self.assertEqual(later["query"], "delay=120")
        self.assertTrue(run.page_url(later).startswith("file://"))
        self.assertTrue(run.page_url(later).endswith("/steps.html?delay=120"), "the query is not quoted into the path")
        self.assertNotIn("?", run.page_url(steps))
        page = (run.HERE / "steps.html").read_text()
        # The step answers from a worker's timer, busy until it does; without
        # the query the press re-draws at once, as before.
        for needed in ("get('delay')", "new Worker(", "aria-busy", "if (!later) return draw();"):
            self.assertIn(needed, page)
        self.assertTrue(run.succeeded("later", {"oracle": {"count": 3}}))
        self.assertFalse(run.succeeded("later", {"oracle": {"count": 2}}))

    def test_success_is_the_pages_own_word(self):
        self.assertTrue(run.succeeded("press", {"oracle": {"count": 1}}))
        self.assertFalse(run.succeeded("press", {"oracle": {"count": 2}}))
        self.assertTrue(run.succeeded("type", {"oracle": {"searched": "London"}}))
        self.assertFalse(run.succeeded("type", {"oracle": {"searched": None}}))
        self.assertTrue(run.succeeded("steps", {"oracle": {"count": 3}}))
        self.assertFalse(run.succeeded("steps", {"oracle": {"count": 2}}))
        observed = {"container": {"chosen": 1}, "image": {"chosen": 1}, "row": {"chosen": 1}}
        self.assertTrue(run.succeeded("observe", {"rows": [{"observed": observed}]}))
        self.assertFalse(run.succeeded("observe", {"rows": [{}]}))

    def test_the_table_reads_every_row_and_percentiles_are_nearest_rank(self):
        self.assertEqual(run.percentile([3, 1, 2], 0.5), 2)
        self.assertEqual(run.percentile([1, 2, 3, 4], 0.95), 4)
        self.assertIsNone(run.percentile([], 0.5))
        rows = [
            {"scenario": "press", "label": "after", "walkMs": 400.0, "load": 9.0,
             "oracle": {"count": 1},
             "calls": [call("marks", 0, 30), call("click", 250, 30), call("find", 290, 20)],
             "rows": [{"outcome": "answered", "requests": 1, "model": "jev-1.13.0"}]},
            {"scenario": "type", "label": "after", "walkMs": 1_500.0, "load": 11.0,
             "oracle": {"searched": "London"}, "standInMs": [40.0],
             "calls": [call("marks", 0, 30), call("click", 250, 30), call("type", 900, 30)],
             "rows": [{"outcome": "answered", "requests": 1, "model": "jev-1.13.0",
                       "typed": {"source": "written", "model": "m", "valueMs": 600}}]},
        ]
        for row in rows:
            row["walk"] = 1
        summary = run.summarize(rows)
        self.assertEqual(run.summarize(rows, first=True), {}, "no walk opened its process")
        self.assertEqual(summary["press/after"]["ok"], 1)
        self.assertEqual(summary["press/after"]["press_p50"], 280)
        self.assertEqual(summary["type/after"]["type_p50"], 930 - 40)
        self.assertEqual(summary["type/after"]["values_written"], 1)
        self.assertEqual(summary["type/after"]["large_model_calls"], 0)
        self.assertIn("press/after", run.table_md(summary, "cmd", pathlib.Path("/out")))

    def test_the_table_counts_settles_aheads_and_the_questions_a_walk_asked(self):
        def inside(verb, start, ms, **what):
            said = call(verb, start, ms)
            said["inside"] = what
            return said
        row = {"scenario": "steps", "label": "after-ahead", "walk": 1, "walkMs": 900.0, "load": 9.0,
               "oracle": {"count": 3}, "asked": 1, "begun": 3,
               "overlapped": 2, "discarded": 0, "cancelled": 1,
               "calls": [call("marks", 0, 30), inside("click", 250, 40, previewMs=30.0),
                         inside("marks", 290, 90, settleMs=60.0, settle="ready"), call("find", 380, 20),
                         inside("click", 500, 40, previewMs=30.0),
                         inside("marks", 540, 300, settleMs=250.0, settle="not_ready"),
                         call("find", 840, 20)],
               "rows": [{"outcome": "answered", "requests": 1, "model": "jev-1.13.0"}]}
        cell = run.summarize([row])["steps/after-ahead"]
        self.assertEqual(cell["ok"], 1)
        self.assertEqual((cell["settled_ready"], cell["settled"]), (1, 2))
        self.assertEqual(cell["settle_p50"], 60.0)
        self.assertEqual(cell["preview_p50"], 30.0)
        self.assertEqual((cell["overlapped"], cell["discarded"], cell["cancelled"]), (2, 0, 1))
        self.assertEqual(cell["asks_per_walk"], 4)
        self.assertEqual(cell["gaps"], 1)
        self.assertIn("2/0/1", run.table_md({"steps/after-ahead": cell}, "cmd", pathlib.Path("/out")))


if __name__ == "__main__":
    unittest.main()
