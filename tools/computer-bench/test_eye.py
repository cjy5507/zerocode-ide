"""tools/computer-bench/eye.py — the eye's measurement, through the scripted
shim: looks only, every number either measured or said to be missing."""
import json
import os
import pathlib
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace
import contextlib
import io

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE))

import eye  # noqa: E402


class ColdHelper(unittest.TestCase):
    def test_process_failure_cannot_become_a_successful_look(self):
        failed = subprocess.CompletedProcess([], 1, '{"ok":true}', '')
        with patch.object(eye.subprocess, "run", return_value=failed):
            self.assertFalse(eye.call(["screenshot"])[1]["ok"])

    def test_startup_look_precedes_pid_and_is_not_a_measured_sample(self):
        events = []
        def call(argv):
            events.append(tuple(argv))
            return 25.0, {"ok": True}
        def pid():
            self.assertEqual(events, [("screenshot",)])
            return 123
        with patch.object(eye, "call", side_effect=call), patch.object(eye, "helper_pid", side_effect=pid), \
                patch.object(eye, "cpu_ms", side_effect=range(16)), contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(eye.cost(SimpleNamespace(n=2, wait_ms=1, reads=2, idle_s=0)), 0)
        report = json.loads(out.getvalue())
        self.assertEqual(report["helper"], 123)
        self.assertEqual(report["warmup"], {"look": "screenshot", "wall_ms": 25.0, "ok": True})
        self.assertEqual(report["looks"][0]["n"], 2)
        self.assertEqual(report["looks"][0]["helper_cpu_ms_per_look"], 0.5)


@unittest.skipIf(os.name == "nt", "the scripted shim is a POSIX script")
class Eye(unittest.TestCase):
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
        # No helper process can be found under a scripted shim.
        (self.bin / "pgrep").write_text("#!/bin/sh\nexit 1\n")
        (self.bin / "pgrep").chmod(0o755)

    def test_a_cost_run_only_looks_and_names_what_it_could_not_measure(self):
        env = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}", "FAKE_STATE": str(self.state)}
        out = subprocess.run(["python3", str(HERE / "eye.py"), "cost", "--n", "3", "--wait-ms", "100", "--idle-s", "0", "--reads", "2"],
                             env=env, capture_output=True, text=True, timeout=120)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        report = json.loads(out.stdout)
        self.assertIn("no single zerocode-computer-use-macos process", report["helper"])
        self.assertEqual([row["look"] for row in report["looks"]], ["screenshot", "observe --diff"])
        self.assertIsNone(report["looks"][0]["helper_cpu_ms_per_look"], "never zero for unmeasured")
        verbs = {line.split()[0] for line in (self.state / "calls").read_text().splitlines()}
        self.assertEqual(verbs, {"screenshot", "observe", "wait-for", "read"}, "looks only")
        self.assertEqual(report["ocr_reads"]["n"], 2)
        self.assertIsNone(report["ocr_reads"]["rest_helper_cpu_p50_ms"], "never zero for unmeasured")

    def test_the_wait_reports_its_looks_and_reads_in_the_windows_words(self):
        before = eye.LOOKS_PATTERN.search("still absent after 5093 ms (6 looks, 0 matches)")
        self.assertEqual((before.group(1), before.group(2)), ("6", None), "every look read before the eye")
        gated = eye.LOOKS_PATTERN.search("still absent after 5003 ms (20 looks, 0 matches, 1 reads)")
        self.assertEqual((gated.group(1), gated.group(2)), ("20", "1"))
        runtime = (REPO / "crates/zerocode-shell/src/agent_tools_runtime.rs").read_text()
        left_out = eye.LOOKS_PATTERN.search(
            "still absent after 5003 ms (20 looks, 0 matches, 1 reads; ZeroCode's own window regions were excluded — the app may be behind them)")
        self.assertEqual((left_out.group(1), left_out.group(2)), ("20", "1"), "what a reading left out follows the reads")
        self.assertIn('"still {} after {} ms ({looks} looks, {count} matches{}{})"', runtime)
        self.assertIn('format!(", {} reads", gate.reads)', runtime)
        self.assertIn(f'const HELPER_EXECUTABLE: &str = "{eye.HELPER_EXECUTABLE}";',
                      (REPO / "crates/zerocode-shell/src/computer_use/macos.rs").read_text())


if __name__ == "__main__":
    unittest.main()
