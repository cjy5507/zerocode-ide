"""tools/computer-bench/bench.py — the runner, driven end to end through the
scripted shim and zo on PATH (and scripted ioreg, pgrep, defaults, open,
osascript): it refuses before anything moves, a stop — the person's, a deaf
chord, a signal to the runner, a model's own — ends it without another act,
the person's clipboard comes back whatever happened, only what setup opened
is quit, and its own calls stay out of the run folders. Plus the pins that
hold its words to the product's, and the real pasteboard keeper on a named
pasteboard (never the person's)."""
import json
import os
import pathlib
import re
import shutil
import signal
import stat
import subprocess
import tempfile
import time
import unittest
import uuid

import tally

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[1]
ARMED = {"stopped": False, "reason": None, "actions": 10, "hotkey": "control+option+escape", "hotkeyArmed": True,
         "hotkeyHears": True, "budget": {"perSecond": 10, "burst": 20, "perSession": 5000}, "lastActionAt": None}
PERSONS_CLIPBOARD = "--- a/the person's diff\n-- and a SQL comment"
USAGE = {"type": "usage", "total_tokens": 1234}
SYSTEM_FAKES = {
    "ioreg": """#!/bin/sh
case "$*" in
  *IOHIDSystem*) echo "  \\"HIDIdleTime\\" = ${FAKE_IDLE_NS:-1000000000000}" ;;
  *Root*) [ -n "${FAKE_LOCKED:-}" ] && echo '  "CGSSessionScreenIsLocked"=Yes' ;;
esac
exit 0
""",
    # An app is up when FAKE_RUNNING names it (the person's, pid 4242) or the
    # scripted open started it (pid 777, or what pid-<name> says now).
    "pgrep": """#!/bin/sh
name="$2"
case ",${FAKE_RUNNING:-}," in *",$name,"*) echo 4242; exit 0 ;; esac
if [ -f "$FAKE_STATE/running" ] && grep -qx "$name" "$FAKE_STATE/running"; then
  if [ -f "$FAKE_STATE/pid-$name" ]; then cat "$FAKE_STATE/pid-$name"; else echo 777; fi
  exit 0
fi
exit 1
""",
    "defaults": """#!/bin/sh
[ -n "${FAKE_TABBING:-}" ] || exit 1
echo "$FAKE_TABBING"
""",
    "open": """#!/bin/sh
echo "$*" >> "$FAKE_STATE/open"
[ "$1" = "-a" ] && echo "$2" >> "$FAKE_STATE/running"
exit 0
""",
    # pasteboard.js's three modes over $FAKE_STATE/clipboard.
    "osascript": """#!/bin/sh
mode="$4"; dir="$5"; clip="$FAKE_STATE/clipboard"
case "$mode" in
  save) if [ -f "$clip" ]; then cp "$clip" "$dir/manifest.json"; else : > "$dir/manifest.json"; fi; echo 1 ;;
  clear) rm -f "$clip"; echo 0 ;;
  restore)
    if [ -n "${FAKE_RESTORE_FAILS:-}" ]; then echo "the pasteboard refused the items" >&2; exit 1; fi
    if [ -s "$dir/manifest.json" ]; then cp "$dir/manifest.json" "$clip"; else rm -f "$clip"; fi; echo 1 ;;
esac
""",
    "caffeinate": "#!/bin/sh\nexit 0\n",
    "sw_vers": "#!/bin/sh\necho 15.0\n",
}


def scenario(name, **fields):
    row = {
        "id": name, "surface": "test", "apps_not_running": ["TextEdit"], "stale_window_titles": [],
        "fixtures": [{"kind": "text", "path": "note.txt", "text": "x\n"}],
        "setup": [{"kind": "open_fresh", "app": "TextEdit", "path": "note.txt"},
                  {"kind": "wait_window", "app": "TextEdit", "title": "note.txt", "own": True}],
        "prompt": "Say 42.", "budget_s": 5,
        "oracle": [{"kind": "no_window", "app": "TextEdit", "title": "never"}, {"kind": "answer_digits", "expect": "42"}],
        "reference": [["activate", "--app", "TextEdit"], ["type", "--text", "42"]],
        "teardown": [{"kind": "quit", "app": "TextEdit", "title": "note.txt"}],
    }
    row.update(fields)
    return row


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


@unittest.skipIf(os.name == "nt", "the runner's fakes are POSIX scripts")
class Bench(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.bin = self.tmp / "bin"
        self.state = self.tmp / "state"
        self.top = self.tmp / "top"
        for folder in (self.bin, self.state, self.top, self.tmp / "work"):
            folder.mkdir()
        for name, source in (("zerocode-computer", (HERE / "fake-shim.py").read_text()), ("zo", (HERE / "fake-zo.py").read_text()), *SYSTEM_FAKES.items()):
            path = self.bin / name
            path.write_text(source)
            path.chmod(path.stat().st_mode | stat.S_IEXEC)
        (self.state / "clipboard").write_text(PERSONS_CLIPBOARD)
        raw = json.loads((HERE / "bench.json").read_text())
        for name, value in {"countdown_s": 0, "countdown_idle_tolerance_s": 0, "status_poll_s": 0.05, "inter_run_quiet_s": 0,
                            "kill_grace_s": 1, "sigkill_after_s": 1, "hands_lane_runs": 1, "runs_per_scenario": 1,
                            "question_wait_ms": 500, "setup_timeout_ms": 1000, "quit_wait_ms": 100}.items():
            raw[name]["value"] = value
        raw["scenarios"]["value"] = [scenario("t-answer"), scenario("t-second")]
        self.raw = raw

    def environment(self, table=None, **env):
        table_path = self.tmp / "bench.json"
        table_path.write_text(json.dumps(table or self.raw))
        environment = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}", "FAKE_STATE": str(self.state),
                       "ZEROCODE_RUN_EVIDENCE_DIR": str(self.top), "BENCH_TABLE": str(table_path),
                       "TMPDIR": str(self.tmp / "work"), "FAKE_HELPER": json.dumps(ARMED),
                       "FAKE_ZO_SCRIPT": json.dumps({"ndjson": [USAGE], "last_message": "The answer is 42.\nRESULT: PASS"})}
        environment.update(env)
        return environment

    def run_bench(self, *args, table=None, **env):
        started = time.monotonic()
        out = subprocess.run(["python3", str(HERE / "bench.py"), *args], env=self.environment(table, **env),
                             capture_output=True, text=True, timeout=120)
        out.seconds = time.monotonic() - started
        return out

    def calls(self):
        path = self.state / "calls-evidence"
        return [line.split("\t", 1) for line in path.read_text().splitlines()] if path.exists() else []

    def zo_calls(self):
        path = self.state / "zo-calls"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def opened(self):
        path = self.state / "open"
        return path.read_text().splitlines() if path.exists() else []

    def run_body(self, folder):
        return json.loads((self.top / folder / tally.BENCH_RUN).read_text())

    def clipboard(self):
        path = self.state / "clipboard"
        return path.read_text() if path.exists() else ""

    # ----------------------------------------------------- before anything --
    def test_preflight_refuses_a_stop_chord_that_is_not_armed_or_cannot_be_heard(self):
        for helper, words in ((dict(ARMED, hotkeyArmed=False), "stop chord is not armed"),
                              (dict(ARMED, hotkeyHears=False), "cannot be heard"),
                              ({key: value for key, value in ARMED.items() if key != "hotkeyHears"}, "older than the bench")):
            out = self.run_bench("run", FAKE_HELPER=json.dumps(helper))
            self.assertEqual(out.returncode, 2, out.stderr)
            self.assertIn(words, out.stderr)
        self.assertEqual(self.zo_calls(), [], "no zo is spawned")
        self.assertEqual(self.opened(), [], "nothing is opened")
        verbs = {words.split()[0] for _dir, words in self.calls() if words}
        self.assertFalse(verbs & {"activate", "key", "type", "mouse-click", "mouse-move"}, verbs)
        self.assertEqual(self.clipboard(), PERSONS_CLIPBOARD, "a refused start never touches the clipboard")

    def test_preflight_refuses_when_a_scenario_app_is_running_or_input_arrives_in_the_countdown(self):
        out = self.run_bench("run", FAKE_RUNNING="TextEdit")
        self.assertEqual(out.returncode, 2, out.stderr)
        self.assertIn("TextEdit is already running", out.stderr)
        raw = json.loads(json.dumps(self.raw))
        raw["countdown_s"]["value"] = 1
        out = self.run_bench("run", table=raw, FAKE_IDLE_NS="0")
        self.assertEqual(out.returncode, 2, out.stderr)
        self.assertIn("countdown", out.stderr)
        self.assertEqual(self.opened(), [], "nothing was set up")
        out = self.run_bench("run", FAKE_LOCKED="1")
        self.assertIn("screen is locked", out.stderr)

    def test_a_failed_window_look_refuses_rather_than_finding_no_window(self):
        raw = json.loads(json.dumps(self.raw))
        raw["scenarios"]["value"] = [scenario("t-stale", stale_window_titles=["note.txt"])]
        out = self.run_bench("run", table=raw, FAKE_WINDOWS="fail")
        self.assertEqual(out.returncode, 2, out.stderr)
        self.assertIn("could not list the windows", out.stderr)

    def test_a_hand_started_terminal_is_refused(self):
        environment = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}", "FAKE_STATE": str(self.state)}
        environment.pop("ZEROCODE_RUN_EVIDENCE_DIR", None)
        out = subprocess.run(["python3", str(HERE / "bench.py"), "run"], env=environment, capture_output=True, text=True, timeout=60)
        self.assertEqual(out.returncode, 2)
        self.assertIn("bench:computer", out.stderr)

    # ------------------------------------------------------------ the runs --
    def test_a_live_run_is_one_folder_one_line_prompt_one_tool_and_the_measuring_env(self):
        ndjson = [{"type": "wire_model", "model": "m1", "source": "flag"},
                  {"type": "tool_call", "id": "a", "name": "Computer", "summary": "", "status": "running"},
                  {"type": "tool_call", "id": "a", "name": "Computer", "summary": "", "status": "done"},
                  {"type": "tool_call", "id": "b", "name": "Computer", "summary": "", "status": "done"},
                  {"type": "usage", "total_tokens": 1234}]
        out = self.run_bench("run", "--scenario", "t-answer",
                             FAKE_ZO_SCRIPT=json.dumps({"ndjson": ndjson, "last_message": "The answer is 42.\nRESULT: PASS"}))
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        [call] = self.zo_calls()
        folder = str(self.top / "live-t-answer-default-1")
        self.assertEqual(call["env"]["ZEROCODE_RUN_EVIDENCE_DIR"], folder)
        self.assertEqual(call["env"]["ZO_PROFILE_DISABLE_HOOK_REPORTER"], "1")
        self.assertEqual(call["env"]["ZO_TURN_DEADLINE_SECS"], "5")
        self.assertTrue(call["env"]["ZO_SESSION_ROOT"].startswith(folder))
        self.assertEqual((call["env"]["ZEROCODE_SECOND_BRAIN"], call["env"]["ZO_DREAM"], call["env"]["ZO_AUTO_VERIFY"]), ("off", "0", "0"))
        self.assertEqual(call["stdin"].count("\n"), 1, "one line: zo reads stdin a line at a time")
        self.assertTrue(call["stdin"].startswith(self.raw["preamble"]["value"]))
        for flag in ("--json", "--no-spawn", "--permission-mode", "--cwd", "--last-message"):
            self.assertIn(flag, call["argv"])
        self.assertEqual(call["argv"][call["argv"].index("--allowed-tools") + 1], "Computer", "zo has the Computer tool and no other")
        self.assertEqual(call["clipboard"], "", "the run starts on an empty clipboard: the person's is not the model's to read")
        body = self.run_body("live-t-answer-default-1")
        self.assertEqual((body["tokens_total"], body["tool_calls"], body["config"]["wire_model"]), (1234, {"Computer": 2}, "m1"))
        self.assertEqual((body["oracle"]["pass"], body["claimed"], body["off_road"], body["hands_proved"]), (True, "PASS", [], True))
        self.assertEqual(self.run_body("hands-t-answer-1")["oracle"]["pass"], True, "run walks the hands lane first")
        self.assertTrue((self.top / "table.md").exists())
        self.assertIn("| t-answer | live | default | m1 | 1 | 1/1", out.stdout)
        self.assertEqual(self.clipboard(), PERSONS_CLIPBOARD, "the clipboard is the person's again")

    def test_only_what_setup_opened_is_quit(self):
        out = self.run_bench("check", "--scenario", "t-answer")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        words = [words for _dir, words in self.calls()]
        self.assertIn("quit --app TextEdit --json", words)
        self.assertNotIn("quit --app TextEdit --force --json", words, "a quit that went needs no force")
        # The person opens their own TextEdit while zo runs: a pid the run did not open.
        (self.state / "calls-evidence").unlink()
        out = self.run_bench("run", "--scenario", "t-answer",
                             FAKE_ZO_SCRIPT=json.dumps({"ndjson": [USAGE], "state": {"pid-TextEdit": "999\n"}, "last_message": "42"}))
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        quits = [words for _folder, words in self.calls() if words.startswith("quit")]
        self.assertEqual(len(quits), 1, f"only the hands run's own TextEdit was quit: {quits}")
        self.assertIn("TextEdit left running: it is not the process this run opened", out.stdout)
        self.assertEqual(self.run_body("live-t-answer-default-1")["teardown_left"], ["TextEdit left running: it is not the process this run opened"])

    def test_a_quit_that_hangs_is_forced_only_on_its_own_windows(self):
        out = self.run_bench("check", "--scenario", "t-answer", FAKE_QUIT_STUCK="1")
        self.assertIn("quit --app TextEdit --force --json", [words for _dir, words in self.calls()], "no window but its own: forced")
        (self.state / "calls-evidence").unlink()
        (self.state / "running").unlink()
        windows = [{"id": 5, "app": {"name": "TextEdit", "pid": 777}, "title": "The person's letter.rtf"}]
        out = self.run_bench("check", "--scenario", "t-answer", FAKE_QUIT_STUCK="1", FAKE_WINDOWS=json.dumps(windows))
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertNotIn("quit --app TextEdit --force --json", [words for _dir, words in self.calls()])
        self.assertIn("shows a window that is not the bench's", out.stdout)

    # ------------------------------------------------------------ the stops --
    def test_the_stop_chord_ends_the_bench_and_is_an_intervention(self):
        out = self.run_bench("run", FAKE_ZO_SCRIPT=json.dumps({"state": {"stopped": "hotkey"}, "sleep_s": 30, "last_message": "RESULT: PASS"}))
        self.assertEqual(out.returncode, 3, out.stderr)
        self.assertLess(out.seconds, 25, "zo was killed, not waited for")
        body = self.run_body("live-t-answer-default-1")
        self.assertEqual(body["stopped_by"], "hotkey")
        self.assertFalse((self.top / "live-t-second-default-1").exists(), "no later run starts")
        self.assertFalse(any(words.startswith("resume") for _dir, words in self.calls()), "the runner never lifts a stop")
        self.assertEqual(sum(1 for _folder, words in self.calls() if words.startswith("quit")), 2,
                         "the two hands runs' quits only: nothing is quit after the stop")
        row = tally.measure(str(self.top / "live-t-answer-default-1"))
        self.assertEqual((row["success"], row["interventions"], row["aborted"]), (None, 1, True))
        self.assertIn("left as it is after the stop", out.stdout)
        self.assertEqual(self.clipboard(), PERSONS_CLIPBOARD)

    def test_a_stop_in_the_hands_lane_ends_the_bench_before_anything_else_opens(self):
        out = self.run_bench("run", FAKE_STOP_ON="type:hotkey")
        self.assertEqual(out.returncode, 3, out.stdout + out.stderr)
        self.assertEqual(len(self.opened()), 1, f"only the first scenario's setup opened anything: {self.opened()}")
        self.assertEqual(self.zo_calls(), [])
        self.assertEqual(self.run_body("hands-t-answer-1")["stopped_by"], "hotkey")
        self.assertFalse((self.top / "hands-t-second-1" / tally.BENCH_RUN).exists())
        self.assertIsNone(tally.measure(str(self.top / "hands-t-answer-1"))["success"])

    def test_a_deaf_chord_mid_run_ends_the_bench(self):
        out = self.run_bench("run", "--scenario", "t-answer", FAKE_ZO_SCRIPT=json.dumps({"ndjson": [USAGE], "state": {"deaf": "1"}, "sleep_s": 30}))
        self.assertEqual(out.returncode, 3, out.stderr)
        self.assertLess(out.seconds, 25)
        self.assertEqual(self.run_body("live-t-answer-default-1")["stopped_by"], "chord_deaf")
        self.assertIsNone(tally.measure(str(self.top / "live-t-answer-default-1"))["success"], "not the model's doing: not judged")

    def test_a_model_resume_or_stop_ends_the_bench_and_fails_its_run(self):
        for verb in ("resume", "stop"):
            steps = [{"n": 1, "at_epoch_ms": 1, "tool": "computer", "verb": verb, "argv": [verb], "ok": True}]
            out = self.run_bench("run", "--scenario", "t-answer", FAKE_ZO_SCRIPT=json.dumps({"ndjson": [USAGE], "steps": steps, "sleep_s": 30}))
            self.assertEqual(out.returncode, 3, out.stderr)
            folder = str(self.top / "live-t-answer-default-1")
            self.assertEqual(self.run_body("live-t-answer-default-1")["stopped_by"], f"model:{verb}")
            self.assertIs(tally.measure(folder)["success"], False, "the model's own stop is judged, and failed")
            shutil.rmtree(self.top)
            self.top.mkdir()
            (self.state / "running").unlink(missing_ok=True)  # the stop left TextEdit up: the next bench would refuse

    def test_a_signal_to_the_runner_kills_zo_and_puts_the_clipboard_back(self):
        for sig in (signal.SIGINT, signal.SIGHUP):
            (self.state / "zo-calls").unlink(missing_ok=True)
            env = self.environment(FAKE_ZO_SCRIPT=json.dumps({"ndjson": [USAGE], "state": {"clipboard": "the model's copy"}, "sleep_s": 60}))
            bench = subprocess.Popen(["python3", str(HERE / "bench.py"), "run", "--scenario", "t-answer"], env=env,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
            deadline = time.monotonic() + 60
            while not self.zo_calls() and time.monotonic() < deadline:
                time.sleep(0.05)
            [call] = self.zo_calls()
            time.sleep(0.3)
            os.killpg(bench.pid, sig)  # what a terminal does to its foreground group
            out, err = bench.communicate(timeout=60)
            self.assertEqual(bench.returncode, 3, out + err)
            self.assertFalse(alive(call["pid"]), f"{sig.name}: zo is gone")
            self.assertEqual(self.run_body("live-t-answer-default-1")["stopped_by"], f"runner:{sig.name}")
            self.assertEqual(self.clipboard(), PERSONS_CLIPBOARD, f"{sig.name}: the person's clipboard is back")
            self.assertTrue((self.top / "table.md").exists())
            shutil.rmtree(self.top)
            self.top.mkdir()
            (self.state / "running").unlink(missing_ok=True)

    def test_a_question_left_open_is_waited_for_and_one_past_the_bound_ends_the_bench(self):
        out = self.run_bench("run", "--scenario", "t-answer", FAKE_ZO_SCRIPT=json.dumps({"ndjson": [USAGE], "state": {"confirming": "3"}, "last_message": "42"}))
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertEqual(sum(1 for _folder, words in self.calls() if words.startswith("quit")), 2, "the live run's teardown ran once it closed")
        shutil.rmtree(self.top)
        self.top.mkdir()
        (self.state / "calls-evidence").unlink()
        out = self.run_bench("run", FAKE_ZO_SCRIPT=json.dumps({"ndjson": [USAGE], "state": {"confirming": "100000"}, "last_message": "42"}))
        self.assertEqual(out.returncode, 3, out.stdout + out.stderr)
        self.assertIn("question_open", out.stderr)
        body = self.run_body("live-t-answer-default-1")
        self.assertTrue(body["oracle"]["pass"], "the run itself is judged: it ended before the question outlived the bound")
        self.assertFalse((self.top / "live-t-second-default-1").exists())
        self.assertEqual(sum(1 for _folder, words in self.calls() if words.startswith("quit")), 2,
                         "the hands runs' quits only: nothing moves while the question is on the person's screen")

    # --------------------------------------------------------- the judging --
    def test_a_run_past_its_budget_is_killed_and_failed(self):
        raw = json.loads(json.dumps(self.raw))
        raw["scenarios"]["value"] = [scenario("t-slow", budget_s=1)]
        out = self.run_bench("run", table=raw, FAKE_ZO_SCRIPT=json.dumps({"ndjson": [USAGE], "sleep_s": 30, "last_message": "RESULT: PASS"}))
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertLess(out.seconds, 25)
        body = self.run_body("live-t-slow-default-1")
        self.assertTrue(body["exit"]["timed_out"])
        self.assertIs(tally.measure(str(self.top / "live-t-slow-default-1"))["success"], False, "a run past its budget failed, whatever the oracle says")

    def test_a_zo_that_failed_on_its_own_or_never_answered_is_invalid_not_failed(self):
        for script, why in (({"exit": 3, "last_message": ""}, "zo exited 3"), ({"last_message": "42"}, "no model answer")):
            out = self.run_bench("run", "--scenario", "t-answer", FAKE_ZO_SCRIPT=json.dumps(script))
            self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
            row = tally.measure(str(self.top / "live-t-answer-default-1"))
            self.assertEqual((row["success"], row["invalid_reason"]), (None, why))
            self.assertIn("| t-answer | live | default | - | 1 | — |", out.stdout)
            shutil.rmtree(self.top)
            self.top.mkdir()

    def test_an_off_road_tool_or_forbidden_verb_fails_a_passing_oracle(self):
        script = {"ndjson": [{"type": "tool_call", "id": "x", "name": "Bash", "summary": "", "status": "done"}, USAGE],
                  "steps": [{"n": 1, "at_epoch_ms": 1, "tool": "computer", "verb": "run", "argv": ["run"], "ok": True}],
                  "last_message": "42 RESULT: PASS"}
        out = self.run_bench("run", "--scenario", "t-answer", FAKE_ZO_SCRIPT=json.dumps(script))
        self.assertEqual(out.returncode, 0, out.stderr)
        body = self.run_body("live-t-answer-default-1")
        self.assertEqual((body["off_road"], body["forbidden_verbs"], body["oracle"]["pass"]), (["Bash"], ["run"], True))
        self.assertIs(tally.measure(str(self.top / "live-t-answer-default-1"))["success"], False)

    def test_a_scenario_whose_hands_lane_fails_never_runs_live(self):
        out = self.run_bench("run", FAKE_REFUSE="type:1")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertIn("t-answer is invalid", out.stdout)
        self.assertEqual([call["env"]["ZEROCODE_RUN_EVIDENCE_DIR"] for call in self.zo_calls()], [str(self.top / "live-t-second-default-1")])
        summary = tally.summarize(tally.collect(str(self.top), bench_only=True))
        self.assertEqual(summary["t-answer|hands|hands"]["success_rate"], 0.0)
        self.assertNotIn("t-answer|live|default", summary)
        self.assertEqual(summary["t-second|hands|hands"]["success_rate"], 1.0)

    def test_two_configs_never_share_a_folder(self):
        raw = json.loads(json.dumps(self.raw))
        raw["configs"]["value"] = [{"id": "a", "model": "m-a", "effort": None},
                                   {"id": "b", "model": "m-b", "effort": "high", "env": {"ZO_COMPUTER_MARKS": "1"}}]
        out = self.run_bench("run", "--scenario", "t-answer", "--runs", "2", "--config", "b", "--config", "a", table=raw,
                             ZO_COMPUTER_MARKS="0")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        calls = self.zo_calls()
        folders = [call["env"]["ZEROCODE_RUN_EVIDENCE_DIR"] for call in calls]
        self.assertEqual(folders, [str(self.top / name) for name in ("live-t-answer-a-1", "live-t-answer-b-1", "live-t-answer-a-2", "live-t-answer-b-2")],
                         "the configs take turns run by run, in the table's order")
        self.assertEqual(len({call["cwd"] for call in calls}), 4, "and never a work folder")
        self.assertEqual([call["env"]["ZO_COMPUTER_MARKS"] for call in calls], [None, "1", None, "1"],
                         "a key a config sets is never inherited: the other side runs the core's switch as built")
        self.assertEqual(self.run_body("live-t-answer-b-1")["config"]["model_asked"], "m-b")
        self.assertEqual(self.run_body("live-t-answer-b-1")["config"]["env"], {"ZO_COMPUTER_MARKS": "1"})
        self.assertEqual(self.run_body("live-t-answer-a-1")["config"]["env"], {})
        summary = tally.summarize(tally.collect(str(self.top), bench_only=True))
        self.assertEqual((summary["t-answer|live|a"]["runs"], summary["t-answer|live|b"]["runs"]), (2, 2))

    def test_run_uses_the_first_config_and_refuses_one_it_cannot_run(self):
        raw = json.loads(json.dumps(self.raw))
        raw["configs"]["value"] = [{"id": "a", "model": None, "effort": None}, {"id": "b", "model": None, "effort": None}]
        out = self.run_bench("run", "--scenario", "t-answer", table=raw)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertEqual([call["env"]["ZEROCODE_RUN_EVIDENCE_DIR"] for call in self.zo_calls()], [str(self.top / "live-t-answer-a-1")])
        for configs, asked, why in (
                (raw["configs"]["value"], ["c"], "no config c"),
                ([{"id": "a", "model": None, "effort": None, "env": {"ZO_DREAM": "1"}}], [], "ZO_DREAM is the runner's"),
                ([{"id": "a", "model": None, "effort": None, "env": {"ZEROCODE_RUN_EVIDENCE_DIR": "/"}}], [], "ZEROCODE_RUN_EVIDENCE_DIR is the runner's")):
            before = len(self.zo_calls())
            raw["configs"]["value"] = configs
            out = self.run_bench("run", "--scenario", "t-answer", *[word for name in asked for word in ("--config", name)], table=raw)
            self.assertEqual(out.returncode, 2, out.stdout + out.stderr)
            self.assertIn(why, out.stderr)
            self.assertEqual(len(self.zo_calls()), before, "refused before anything moved")

    def test_the_runners_own_calls_stay_out_of_run_folders(self):
        out = self.run_bench("run", "--scenario", "t-answer")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        runner = str(self.top / "runner")
        hands = str(self.top / "hands-t-answer-1")
        for folder, words in self.calls():
            verb = words.split()[0]
            if verb in ("activate", "type"):
                self.assertEqual(folder, hands, words)
            elif verb != "evidence":
                self.assertEqual(folder, runner, f"the runner's own {words!r} went to {folder}")
        self.assertTrue((self.top / "hands-t-answer-1" / tally.STEPS).exists(), "the run folder stood before its first call")

    def test_a_clipboard_that_would_not_come_back_is_kept_and_put_back_later(self):
        out = self.run_bench("run", "--scenario", "t-answer", FAKE_RESTORE_FAILS="1")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertIn("the clipboard is NOT back", out.stderr)
        kept = re.search(r"restore-clipboard ([^\s`]+)", out.stderr).group(1)
        self.assertTrue(os.path.isdir(kept))
        self.assertEqual(self.clipboard(), "", "what the bench left, until the person asks")
        again = subprocess.run(["python3", str(HERE / "bench.py"), "restore-clipboard", kept], env=self.environment(),
                               capture_output=True, text=True, timeout=60)
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertEqual(self.clipboard(), PERSONS_CLIPBOARD)
        self.assertFalse(os.path.exists(kept), "the kept copy goes once it is back")

    # ------------------------------------------------------------ the pins --
    def test_the_table_is_whole(self):
        raw = json.loads((HERE / "bench.json").read_text())
        for name, entry in raw.items():
            if isinstance(entry, dict):
                self.assertTrue(entry.get("why"), f"{name} says why")
                self.assertIn("value", entry, name)
        kinds = (HERE / "scenarios.py").read_text()
        for row in raw["scenarios"]["value"]:
            self.assertNotIn("\n", row["prompt"], row["id"])
            self.assertGreater(row["budget_s"], 0)
            self.assertTrue(row["reference"] and row["teardown"] and row["oracle"], row["id"])
            for step in row["setup"] + row["oracle"] + row["teardown"] + row["fixtures"]:
                self.assertIn(f'"{step["kind"]}"', kinds, f"{row['id']}: {step['kind']} is a kind scenarios.py knows")
            opens = [step["app"] for step in row["setup"] if step["kind"] == "open_fresh"]
            owned = [step["app"] for step in row["setup"] if step["kind"] == "wait_window" and step.get("own")]
            self.assertEqual(opens, owned, f"{row['id']}: every app setup opens is claimed, so teardown quits it and no other")
            quits = [step["app"] for step in row["teardown"] if step["kind"] == "quit"]
            self.assertEqual(sorted(quits), sorted(opens), row["id"])

    def test_the_fake_zo_speaks_render_rs(self):
        render = (REPO / "zo-ide/crates/zo-ide/src/ide/render.rs").read_text()
        self.assertIn('"type": "usage", "total_tokens"', render)
        self.assertRegex(render, r'"type": "tool_call",\s*"id": tool_call_id\.0,\s*"name": name')
        self.assertIn('"type": "wire_model", "model"', render)

    def test_product_bounds_hold(self):
        values = tally.table()
        core = (REPO / "crates/zerocode-core/src/computer_use.rs").read_text()
        number = lambda name: int(re.search(rf"pub const {name}: u64 = ([\d_]+);", core).group(1).replace("_", ""))
        self.assertLessEqual(values["setup_timeout_ms"], number("COMPUTER_WAIT_FOR_MAX_MS"))
        self.assertLessEqual(values["quit_wait_ms"], number("COMPUTER_WAIT_FOR_MAX_MS"))
        self.assertIn("COMPUTER_LONGEST_DEADLINE_MS: u64 =\n    COMPUTER_HANDOFF_TIMEOUT_MS + COMPUTER_BRIDGE_GRACE_MS;", core)
        self.assertEqual(values["question_wait_ms"], number("COMPUTER_HANDOFF_TIMEOUT_MS") + number("COMPUTER_BRIDGE_GRACE_MS"))
        self.assertGreater(number("COMPUTER_HANDOFF_TIMEOUT_MS"), number("COMPUTER_CONFIRM_TIMEOUT_MS"), "the handoff is the longer question")
        for name, home in (("ZO_PROFILE_DISABLE_HOOK_REPORTER", "zo-ide/crates/zo-ide/src/ide/reporter.rs"),
                           ("ZO_TURN_DEADLINE_SECS", "zo-ide/crates/runtime/src/conversation/config.rs"),
                           ("ZO_SESSION_ROOT", "zo-ide/crates/runtime/src/session_control.rs"),
                           ("ZO_AUTO_VERIFY", "zo-ide/crates/tools/src/misc_tools/agent_tools/auto_verify.rs"),
                           ("ZO_DREAM", "zo-ide/crates/zo-ide/src/session/dreamer_hook.rs"),
                           ("ZEROCODE_SECOND_BRAIN", "zo-ide/crates/runtime/src/second_brain/mod.rs"),
                           ("ZEROCODE_RUN_EVIDENCE_DIR", "crates/zerocode-core/src/computer_use.rs")):
            self.assertIn(f'"{name}"', (REPO / home).read_text(), f"{name} lives in {home}")
            if name != "ZEROCODE_RUN_EVIDENCE_DIR":
                self.assertTrue(name in values["zo_env"] or name in ("ZO_TURN_DEADLINE_SECS", "ZO_SESSION_ROOT"), name)
        vault = (REPO / "zo-ide/crates/runtime/src/second_brain/mod.rs").read_text()
        self.assertIn(f'pub const VAULT_OFF: &str = "{values["zo_env"]["ZEROCODE_SECOND_BRAIN"]}";', vault)
        self.assertIn('const DREAM_ENV: &str = "ZO_DREAM";', (REPO / "zo-ide/crates/zo-ide/src/session/dreamer_hook.rs").read_text())
        self.assertEqual(values["zo_env"]["ZO_DREAM"], "0")
        args = (REPO / "zo-ide/crates/zo-ide/src/ide/args.rs").read_text()
        for flag in values["zo_flags"] + ["--allowed-tools", "--cwd", "--last-message", "--model", "--effort"]:
            if flag.startswith("--"):
                self.assertIn(f'"{flag}"', args, flag)
        computer = (REPO / "zo-ide/crates/tools/src/computer_tools.rs").read_text()
        for name in values["allowed_tools"]:
            self.assertIn(f'name: "{name}",', computer, f"{name} is the tool zo registers")
        marks = re.search(r'pub const COMPUTER_MARKS_ENV: &str = "(\w+)";', core).group(1)
        self.assertIn("std::env::var(COMPUTER_MARKS_ENV)", computer, "zo reads the core's switch word")
        sides = {config["id"]: (config.get("env") or {}).get(marks) for config in values["configs"]}
        self.assertEqual(sides, {"default": None, "marks": "1"}, "the A/B differs in the core's switch word alone")
        self.assertEqual(values["configs"][0]["id"], "default", "a plain run is the default side")


@unittest.skipUnless(shutil.which("osascript"), "the pasteboard keeper is macOS's")
class PasteboardKeeper(unittest.TestCase):
    """pasteboard.js on a named pasteboard: every item and type comes back
    byte for byte, and clear leaves nothing."""

    def keeper(self, mode, folder, name):
        out = subprocess.run(["osascript", "-l", "JavaScript", str(HERE / "pasteboard.js"), mode, folder, name],
                             capture_output=True, text=True, timeout=60)
        self.assertEqual(out.returncode, 0, out.stderr)
        return out.stdout.strip()

    def test_every_item_and_type_round_trips(self):
        name = f"zc-bench-test-{uuid.uuid4().hex}"
        with tempfile.TemporaryDirectory() as made, tempfile.TemporaryDirectory() as saved:
            items = [[("public.utf8-plain-text", PERSONS_CLIPBOARD.encode()), ("public.html", b"<b>x</b>")],
                     [("public.png", b"\x89PNG\r\n\x1a\n" + bytes(range(256))), ("public.file-url", b"file:///tmp/zc-bench.pdf")]]
            manifest = []
            for i, entry in enumerate(items):
                manifest.append([])
                for j, (kind, data) in enumerate(entry):
                    pathlib.Path(made, f"{i}-{j}.bin").write_bytes(data)
                    manifest[-1].append({"type": kind, "file": f"{i}-{j}.bin"})
            pathlib.Path(made, "manifest.json").write_text(json.dumps({"items": manifest}))
            self.assertEqual(self.keeper("restore", made, name), "2")
            self.assertEqual(self.keeper("save", saved, name), "2")
            back = json.loads(pathlib.Path(saved, "manifest.json").read_text())["items"]
            for entry, want in zip(back, items):
                got = {row["type"]: pathlib.Path(saved, row["file"]).read_bytes() for row in entry}
                self.assertEqual(got, dict(want), "every type of the item, byte for byte")
            self.assertEqual(len(back), len(items))
            self.keeper("clear", saved, name)
            with tempfile.TemporaryDirectory() as empty:
                self.assertEqual(self.keeper("save", empty, name), "0")


if __name__ == "__main__":
    unittest.main()
