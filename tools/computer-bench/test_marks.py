"""tools/computer-bench/marks.py — the marks' measurement, through the
scripted shim: a cost run reads every place twice and gives the switch its
verdict, and a click run never starts without the person's word."""
import json
import os
import pathlib
import stat
import subprocess
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent


@unittest.skipIf(os.name == "nt", "the scripted shim is a POSIX script")
class Marks(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = pathlib.Path(self.tmp.name)
        self.bin, self.state = root / "bin", root / "state"
        self.bin.mkdir()
        self.state.mkdir()
        shim = self.bin / "zerocode-computer"
        shim.write_text((HERE / "fake-shim.py").read_text())
        shim.chmod(shim.stat().st_mode | stat.S_IEXEC)

    def run_marks(self, *args, **knobs):
        env = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}", "FAKE_STATE": str(self.state), **knobs}
        return subprocess.run(["python3", str(HERE / "marks.py"), *args], env=env, capture_output=True, text=True, timeout=120)

    def calls(self):
        return (self.state / "calls").read_text().splitlines() if (self.state / "calls").exists() else []

    def test_a_cost_run_reads_each_place_with_and_without_marks(self):
        out = self.run_marks("cost", "--app", "Finder", "--n", "3")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        report = json.loads(out.stdout)
        self.assertEqual([row["place"] for row in report["cost"]], ["desktop", "--app Finder"])
        self.assertEqual(report["cost"][0]["candidates_placed_omitted"], [2, 1, 1])
        self.assertTrue(report["affordable"])
        looks = [line for line in self.calls() if line.startswith("observe")]
        self.assertEqual(len(looks), 12, "three rounds of two looks at two places")
        self.assertFalse([line for line in self.calls() if not line.startswith("observe")], "looks only")

    def test_a_look_that_numbers_nothing_is_timed_as_what_it_costs(self):
        out = self.run_marks("cost", "--n", "2", FAKE_DESKTOP_UNMARKED="window_not_found: no window to mark")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        row = json.loads(out.stdout)["cost"][0]
        self.assertEqual((row["place"], row["candidates_placed_omitted"], row["legend_tokens"]),
                         ("desktop", "window_not_found: no window to mark", 0))
        self.assertIn("overhead_ms", row)

    def test_a_click_run_needs_the_persons_word(self):
        out = self.run_marks("click")
        self.assertEqual(out.returncode, 2)
        self.assertIn("--i-am-here", out.stderr)
        self.assertEqual(self.calls(), [], "nothing was asked of the desktop")


if __name__ == "__main__":
    unittest.main()
