"""tools/computer-bench/onboarding.sh — the first-run walk a person takes once
per Mac: both permissions, one screenshot, the desktop's windows, a look with
OCR, evidence. Driven here through the scripted fake shim on PATH."""
import os
import pathlib
import stat
import subprocess
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SCRIPT = HERE / "onboarding.sh"
FAKE = HERE / "fake-shim.py"


class Onboarding(unittest.TestCase):
    def run_script(self, grant_after, max_secs="3", poll="0.2"):
        tmp = pathlib.Path(tempfile.mkdtemp())
        shim = tmp / "bin" / "zerocode-computer"
        shim.parent.mkdir()
        shim.write_text(FAKE.read_text())
        shim.chmod(shim.stat().st_mode | stat.S_IEXEC)
        state = tmp / "state"
        state.mkdir()
        env = {**os.environ, "PATH": f"{shim.parent}:{os.environ['PATH']}", "FAKE_STATE": str(state),
               "FAKE_GRANT_AFTER": str(grant_after), "ONBOARDING_POLL_SECS": poll,
               "ONBOARDING_MAX_SECS": max_secs}
        out = subprocess.run(["sh", str(SCRIPT)], env=env, capture_output=True, text=True, timeout=60)
        calls = (state / "calls").read_text().splitlines() if (state / "calls").exists() else []
        return out, calls

    def test_granted_at_once_walks_screenshot_windows_ocr_observe_and_evidence(self):
        out, calls = self.run_script(grant_after=0)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        verbs = [c.split()[0] for c in calls]
        for verb in ("permissions", "screenshot", "list-all-windows", "read", "observe", "evidence"):
            self.assertIn(verb, verbs, verbs)
        self.assertIn("ONBOARDING GREEN", out.stdout)
        self.assertIn("/tmp/evidence-fake", out.stdout, "the evidence folder is named for the person")

    def test_a_missing_grant_is_requested_once_then_polled_until_the_person_toggles(self):
        out, calls = self.run_script(grant_after=3)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        requests = [c for c in calls if c.startswith("permissions --id screenshots")]
        self.assertEqual(len(requests), 1, "the OS request and the settings pane open once, not on every poll")
        polls = [c for c in calls if c.startswith("permissions") and "--id" not in c]
        self.assertGreaterEqual(len(polls), 3, calls)
        self.assertIn("screenshots", out.stdout)

    def test_a_grant_that_never_comes_ends_red_naming_the_row(self):
        out, calls = self.run_script(grant_after=99, max_secs="1")
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("ONBOARDING RED", out.stdout)
        self.assertIn("screenshots", out.stdout + out.stderr)
        verbs = [c.split()[0] for c in calls]
        self.assertNotIn("screenshot", verbs, "no look is attempted without the grant")


if __name__ == "__main__":
    unittest.main()
