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
        for needed_after in ("LiveWriter", "value_writer", "ZEROCODE_WALK_PROBE_VALUE_KEY"):
            self.assertNotIn(needed_after, stripped)

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

    def test_success_is_the_pages_own_word(self):
        self.assertTrue(run.succeeded("press", {"oracle": {"count": 1}}))
        self.assertFalse(run.succeeded("press", {"oracle": {"count": 2}}))
        self.assertTrue(run.succeeded("type", {"oracle": {"searched": "London"}}))
        self.assertFalse(run.succeeded("type", {"oracle": {"searched": None}}))
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


if __name__ == "__main__":
    unittest.main()
