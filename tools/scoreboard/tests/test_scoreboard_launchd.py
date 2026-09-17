#!/usr/bin/env python3
"""The scoreboard beat's clock: the plist the installer renders and the beat's refusals.

Run: python3 tools/scoreboard/tests/test_scoreboard_launchd.py   (stdlib only)
"""
import os
import plistlib
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]


class Plist(unittest.TestCase):
    def test_the_rendered_plist_parses_and_names_the_daily_beat(self):
        out = subprocess.run(["bash", str(HERE / "install-launchd.sh"), "--render", "--hour", "9", "--minute", "0"],
                             capture_output=True, text=True, check=True).stdout
        plist = plistlib.loads(out.encode())
        self.assertEqual(plist["Label"], "dev.zerocode.scoreboard")
        self.assertEqual(plist["ProgramArguments"][1], str(HERE / "beat.sh"))
        self.assertEqual(plist["StartCalendarInterval"], {"Hour": 9, "Minute": 0})
        self.assertIn(".local/bin", plist["EnvironmentVariables"]["PATH"])

    def test_every_script_parses(self):
        for name in ("beat.sh", "install-launchd.sh"):
            subprocess.run(["bash", "-n", str(HERE / name)], check=True)


class Beat(unittest.TestCase):
    def test_the_beat_refuses_aloud_without_zo_or_a_baseline_and_logs_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp) / "state"
            env = dict(os.environ, SCOREBOARD_STATE=str(state), SCOREBOARD_ZO=str(Path(tmp) / "no-zo"),
                       SCOREBOARD_BASELINE=str(Path(tmp) / "no-baseline.json"))
            r = subprocess.run(["bash", str(HERE / "beat.sh")], env=env, capture_output=True, text=True)
            self.assertEqual(r.returncode, 3)
            log = (state / "beat.log").read_text()
            self.assertIn("refused: no zo", log)
            fake_zo = Path(tmp) / "zo"
            fake_zo.write_text("#!/bin/sh\nexit 0\n"); fake_zo.chmod(0o755)
            env["SCOREBOARD_ZO"] = str(fake_zo)
            r = subprocess.run(["bash", str(HERE / "beat.sh")], env=env, capture_output=True, text=True)
            self.assertEqual(r.returncode, 3)
            self.assertIn("refused: no baseline", (state / "beat.log").read_text())

    def test_a_green_run_defers_into_the_windows_inbox_and_logs_a_summary(self):
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp) / "state"
            baseline = Path(tmp) / "baseline.json"; baseline.write_text("{}")
            # A stand-in zo that answers the report shape and writes the inbox line itself.
            fake_zo = Path(tmp) / "zo"
            fake_zo.write_text(
                "#!/bin/sh\n"
                "for a in \"$@\"; do case \"$prev\" in --defer) mkdir -p \"$(dirname \"$a\")\"; "
                "printf '{\"key\":\"disk\",\"observed\":\"9 G\",\"baseline\":\"25 G\",\"words\":\"w\",\"found_at\":1}\\n' >> \"$a\";; esac; prev=$a; done\n"
                "printf '{\"window\":{\"requests\":30,\"breaks\":0,\"disk_free_gb\":9},\"findings\":[{\"key\":\"disk\"}],\"deferred\":1}\\n'\n"
                # zo prints its exit reason after the JSON; the summary must still parse.
                "printf 'zo: process exit reason=scoreboard\\n'\n")
            fake_zo.chmod(0o755)
            env = dict(os.environ, SCOREBOARD_STATE=str(state), SCOREBOARD_ZO=str(fake_zo),
                       SCOREBOARD_BASELINE=str(baseline))
            r = subprocess.run(["bash", str(HERE / "beat.sh")], env=env, capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            self.assertIn("green requests=30 breaks=0 findings=1 deferred=1 disk=9G", (state / "beat.log").read_text())
            pending = state / "pending.jsonl"
            self.assertTrue(pending.exists(), "the finding reached the inbox beside the beat state, where the window reads it")
            self.assertIn('"key":"disk"', pending.read_text())
            self.assertTrue((state / "last-run.json").exists())


if __name__ == "__main__":
    unittest.main(verbosity=1)
