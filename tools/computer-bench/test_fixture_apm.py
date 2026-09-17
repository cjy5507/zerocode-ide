"""Fixture measurement must fail a no-op and respect the existing desk stop."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

from bench import Bench, Stopped
from fixture_apm import CLOCK_ID, Desk, button_points, oracle_delta, fixture_observation, uptime_ns


class Oracle(unittest.TestCase):
    def setUp(self):
        self.before = {"owner": "owned", "pid": 123, "count": 7, "errors": 0,
                       "events": [{"label": "old"}]}

    def after(self):
        return {**self.before, "count": 9, "events": [*self.before["events"],
                {"correct": True, "label": "Amber"}, {"correct": True, "label": "Blue"}]}

    def test_no_op_and_replayed_events_do_not_earn_actions(self):
        self.assertFalse(oracle_delta(self.before, self.before, ["Amber", "Blue"]))
        after = self.after()
        self.assertTrue(oracle_delta(self.before, after, ["Amber", "Blue"]))
        self.assertFalse(oracle_delta(after, after, ["Amber", "Blue"]))

    def test_wrong_order_extra_events_errors_and_changed_owner_fail(self):
        for field, value in (("owner", "another"), ("pid", 124), ("count", 8), ("errors", 1)):
            self.assertFalse(oracle_delta(self.before, {**self.after(), field: value}, ["Amber", "Blue"]))
        self.assertFalse(oracle_delta(self.before, self.after(), ["Blue", "Amber"]))
        extra = self.after()
        extra["events"].append({"correct": True, "label": "Amber"})
        self.assertFalse(oracle_delta(self.before, extra, ["Amber", "Blue"]))


class Observation(unittest.TestCase):
    def observed(self, tree=None):
        return {"snapshot": {"app": {"pid": 123, "bundleId": "dev.zerocode.bench.fixture.owned"},
                             "treeText": tree or "2 text Next: Blue | Count: 9 | Errors: 0\n7 button Amber\n11 button Blue"}}

    def test_ui_state_supplies_target_and_current_indexes(self):
        result = fixture_observation(self.observed(), 123, "owned")
        self.assertEqual(result, {"next": "Blue", "count": 9, "errors": 0, "buttons": {"Amber": 7, "Blue": 11}})

    def test_foreign_missing_and_ambiguous_observations_fail(self):
        for result, pid, owner in ((self.observed(), 124, "owned"), (self.observed(), 123, "another"),
                                   (None, 123, "owned"),
                                   (self.observed("2 text Next: Blue | Count: 9 | Errors: 0\n7 button Amber\n11 button Amber"), 123, "owned"),
                                   (self.observed("2 text Next: Blue | Count: 9 | Errors: 0\n7 button Amber"), 123, "owned")):
            with self.assertRaises(ValueError):
                fixture_observation(result, pid, owner)


class PostActionReuse(unittest.TestCase):
    def observed(self, count, next_, amber=3, blue=4):
        return {"snapshot": {"app": {"pid": 123, "bundleId": "dev.zerocode.bench.fixture.owned"},
            "treeText": f"2 text Next: {next_} | Count: {count} | Errors: 0\n{amber} button Amber\n{blue} button Blue"}}

    def desk(self):
        desk = Desk.__new__(Desk)
        desk.session = {"clock": CLOCK_ID, "owner": "owned", "mode": "alternate", "rounds": [],
                        "last_look": self.observed(0, "Amber")}
        desk.guard = Mock()
        desk.save = Mock()
        desk.last_ms = 1
        desk.look = Mock()
        before = {"owner": "owned", "pid": 123, "count": 0, "errors": 0, "events": []}
        after = {**before, "count": 1, "events": [{"correct": True, "label": "Amber"}]}
        desk.snapshot = Mock(side_effect=[before, after])
        desk.shim = Mock(return_value={"ok": True, "result": self.observed(1, "Blue", 7, 11)})
        return desk

    def test_valid_post_action_state_is_reused_with_new_indexes(self):
        desk = self.desk()
        result = desk.act(["Amber"], reuse_state=True)
        self.assertTrue(result["round"]["passed"])
        self.assertTrue(result["round"]["reused_before"])
        self.assertTrue(result["round"]["reused_after"])
        self.assertEqual(fixture_observation(desk.session["last_look"], 123, "owned")["buttons"]["Blue"], 11)
        desk.look.assert_not_called()
        self.assertIn("--no-screenshot", desk.shim.call_args[0][0])

    def test_stale_response_requires_a_fresh_read_and_records_fallback(self):
        desk = self.desk()
        desk.shim.return_value = {"ok": True, "result": self.observed(0, "Amber")}
        desk.look.return_value = self.observed(1, "Blue")
        result = desk.act(["Amber"], reuse_state=True)
        self.assertTrue(result["round"]["passed"])
        self.assertFalse(result["round"]["reused_after"])
        desk.look.assert_called_once()

    def test_success_envelope_without_a_real_action_still_fails(self):
        desk = self.desk()
        before = {"owner": "owned", "pid": 123, "count": 0, "errors": 0, "events": []}
        desk.snapshot.side_effect = [before, before]
        desk.shim.return_value = {"ok": True, "result": self.observed(0, "Amber")}
        with self.assertRaisesRegex(RuntimeError, "oracle failed"):
            desk.act(["Amber"], reuse_state=True)
        self.assertEqual(desk.session["rounds"][0]["successful_actions"], 0)


class Geometry(unittest.TestCase):
    def setUp(self):
        self.window = {"id": 3, "x": 100, "y": 200, "width": 500, "height": 300}
        self.found = {"coordinateSpace": "screen", "window": {"id": 3}, "matches": [
            {"label": "Amber", "frame": {"x": 120, "y": 240, "width": 100, "height": 40}},
            {"label": "Blue", "frame": {"x": 320, "y": 240, "width": 100, "height": 40}}]}

    def test_screen_frames_are_converted_to_window_points(self):
        self.assertEqual(button_points(self.found, self.window), {"Amber": (70, 60), "Blue": (270, 60)})

    def test_wrong_window_ambiguous_and_outside_targets_fail_closed(self):
        cases = [copy.deepcopy(self.found) for _ in range(3)]
        cases[0]["window"]["id"] = 9
        cases[1]["matches"].append(cases[1]["matches"][0])
        cases[2]["matches"][0]["frame"]["x"] = 900
        for case in cases:
            with self.assertRaises(RuntimeError):
                button_points(case, self.window)


class Guards(unittest.TestCase):
    def desk(self):
        desk = Desk.__new__(Desk)
        desk.session = {"clock": CLOCK_ID, "mode": "alternate", "rounds": []}
        desk.guard = Mock(side_effect=Stopped("person"))
        desk.snapshot = Mock()
        desk.checked = Mock()
        desk.save = Mock()
        return desk

    def test_stop_forbids_actions_and_even_quit(self):
        desk = self.desk()
        for operation in (lambda: desk.act(["Amber"]), desk.close):
            with self.assertRaises(Stopped):
                operation()
        desk.snapshot.assert_not_called()
        desk.checked.assert_not_called()

    def test_launch_arms_hid_watcher_before_first_look(self):
        desk = self.desk()
        desk.guard = Mock()
        desk.hid_idle_s = Mock(return_value=10)
        desk.screen_locked = Mock(return_value=False)
        desk.snapshot.return_value = {"events": [], "count": 0, "errors": 0}
        desk.session.update(owner="owned", app="/private/fixture.app")
        desk.look = Mock()
        with tempfile.TemporaryDirectory() as folder, patch("fixture_apm.uptime_ns", return_value=1234):
            desk.folder = Path(folder)
            # The mocked launch publishes its empty oracle file.
            desk.checked.side_effect = lambda _argv: (desk.folder / "oracle.json").write_text("{}")
            desk.open()
        self.assertEqual(desk.session["watch_hid_since_ns"], 1234)
        desk.look.assert_called_once()

    def test_hid_watcher_yields_when_person_input_follows_run_start(self):
        desk = Desk.__new__(Desk)
        desk.session = {"clock": CLOCK_ID, "watch_hid_since_ns": 10_000_000_000}
        desk.hid_idle_s = Mock(return_value=1)
        with patch.object(Bench, "guard"), patch("fixture_apm.uptime_ns", return_value=20_000_000_000):
            with self.assertRaisesRegex(Stopped, "HID idle reset"):
                desk.guard()
            self.assertEqual(desk.session["hid_interruption"]["source"], "unknown")
            desk.hid_idle_s.return_value = 15
            desk.guard()

    def test_random_batches_and_stale_semantic_indexes_are_refused_before_action(self):
        desk = self.desk()
        for mode, path in (("random", "coordinate"), ("alternate", "semantic")):
            desk.session["mode"] = mode
            with self.assertRaises(ValueError):
                desk.act(["Amber", "Blue"], path)
        desk.guard.assert_not_called()

    def test_tally_uses_verified_actions_once_and_includes_loop_wall(self):
        desk = self.desk()
        desk.session.update(loop_started_ns=0, loop_ended_ns=10_000_000_000,
                            rounds=[{"labels": ["Amber", "Blue"], "successful_actions": 2,
                                     "input_ms": 1000, "passed": True},
                                    {"labels": ["Amber"], "successful_actions": 0,
                                     "input_ms": 1000, "passed": False}])
        report = desk.summary()
        self.assertEqual(report["successful_actions"], 2)
        self.assertEqual(report["attempted_actions"], 3)
        self.assertEqual(report["input_apm"], 60)
        self.assertEqual(report["loop_apm"], 12)
        self.assertFalse(report["all_correct"])


class Measurement(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.desk = PostActionReuse().desk()
        self.desk.folder = Path(self.tmp.name)
        self.desk.save = lambda: Desk.save(self.desk)
        self.desk.session.update(loop_started_ns=0, loop_ended_ns=0,
            measurement={"status": "running", "expected_actions": 1, "start_round": 0,
                         "controller": "external", "started_ns": 0})

    def test_complete_plan_requires_final_guard_and_includes_its_elapsed_time(self):
        self.desk.act(["Amber"], reuse_state=True)
        prefix = self.desk.summary()
        self.assertTrue(prefix["all_correct"])
        self.assertFalse(prefix["measurement_complete"])
        self.assertIsNone(prefix["accepted_apm"])
        for value in prefix["timing_ms"].values():
            self.assertGreaterEqual(value, 0)
        with patch("fixture_apm.uptime_ns", return_value=10_000_000_000):
            report = self.desk.finish()
        self.assertTrue(report["measurement_complete"])
        self.assertEqual(report["accepted_apm"], 6)
        self.assertIsNone(report["llm_apm"], "an external caller alone does not prove LLM decisions")
        self.assertEqual(Desk(self.desk.folder).summary(), report)

    def test_stop_after_the_last_correct_action_is_persisted_and_cannot_be_reset(self):
        self.desk.act(["Amber"], reuse_state=True)
        self.desk.guard.side_effect = Stopped("person interrupted")
        with patch("fixture_apm.uptime_ns", return_value=10_000_000_000):
            with self.assertRaises(Stopped):
                self.desk.finish()
        reloaded = Desk(self.desk.folder)
        report = reloaded.summary()
        self.assertTrue(report["all_correct"], "a correct prefix remains evidence")
        self.assertEqual(report["measurement_status"], "interrupted")
        self.assertEqual(report["loop_wall_ms"], 10000)
        self.assertIsNone(report["accepted_apm"])
        original = (self.desk.folder / "session.json").read_bytes()
        with patch.object(Bench, "guard") as guard:
            for operation in (lambda: reloaded.begin(1), lambda: reloaded.act(["Amber"]), reloaded.close):
                with self.assertRaises(Stopped):
                    operation()
            guard.assert_not_called()
        self.assertEqual((self.desk.folder / "session.json").read_bytes(), original)

    def test_action_transport_failure_keeps_the_unknown_attempt_without_credit(self):
        self.desk.shim.side_effect = RuntimeError("answer lost")
        with self.assertRaisesRegex(RuntimeError, "answer lost"):
            self.desk.act(["Amber"], reuse_state=True)
        report = Desk(self.desk.folder).summary()
        self.assertEqual(report["measurement_status"], "failed")
        self.assertEqual(report["attempted_actions"], 1)
        self.assertEqual(report["unverified_actions"], 1)
        self.assertEqual(report["successful_actions"], 0)
        self.assertFalse(report["all_correct"])
        self.assertIsNone(report["accepted_apm"])

    def test_crashed_action_cannot_be_replayed_after_reopening_the_runner(self):
        self.desk.session["pending_action"] = {"labels": ["Amber"], "input_path": "semantic"}
        self.desk.save()
        reloaded = Desk(self.desk.folder)
        with patch.object(Bench, "guard") as guard, self.assertRaisesRegex(Stopped, "unknown outcome"):
            reloaded.act(["Amber"])
        guard.assert_not_called()
        self.assertEqual(Desk(self.desk.folder).summary()["measurement_status"], "interrupted")

    def test_short_or_overlong_plans_do_not_pass(self):
        with self.assertRaisesRegex(RuntimeError, "every planned action"):
            self.desk.finish()
        self.assertEqual(self.desk.summary()["measurement_status"], "failed")
        self.assertIsNone(self.desk.summary()["accepted_apm"])
        self.desk.session.pop("outcome")
        self.desk.session["measurement"].update(status="running", expected_actions=0)
        with self.assertRaisesRegex(ValueError, "exceed"):
            self.desk.act(["Amber"], reuse_state=True)
        self.desk.shim.assert_not_called()
        self.desk.snapshot.assert_not_called()

    def test_hands_verifies_final_stop_and_declares_rule_controller(self):
        self.desk.begin = Mock()
        self.desk.finish = Mock(side_effect=Stopped("after last input"))
        self.desk.session["measurement"]["controller"] = "observed-rule"
        before, after = list(self.desk.snapshot.side_effect)
        self.desk.snapshot.side_effect = [before, before, after]
        with self.assertRaisesRegex(Stopped, "after last input"):
            self.desk.hands(1, 1, "semantic", reuse_state=True)
        self.desk.begin.assert_called_once_with(1, controller="observed-rule")
        self.assertEqual(self.desk.session["measurement"]["status"], "interrupted")


class SharedClock(unittest.TestCase):
    def test_cli_processes_share_an_epoch(self):
        before = uptime_ns()
        result = subprocess.run([sys.executable, "-c",
            "import json; from fixture_apm import CLOCK_ID, uptime_ns; "
            "print(json.dumps([CLOCK_ID, uptime_ns()]))"],
            cwd=Path(__file__).resolve().parent, capture_output=True, text=True, check=True)
        after = uptime_ns()
        clock, child = json.loads(result.stdout)
        self.assertEqual(clock, CLOCK_ID)
        self.assertLessEqual(before, child)
        self.assertLessEqual(child, after)

    def test_legacy_process_local_times_cannot_be_resumed_or_accepted(self):
        desk = Desk.__new__(Desk)
        desk.session = {"mode": "random", "rounds": [], "loop_started_ns": 3, "loop_ended_ns": 6,
                        "watch_hid_since_ns": 2}
        desk.save = Mock()
        with patch.object(Bench, "guard") as guard:
            with self.assertRaisesRegex(Stopped, "clock is missing"):
                desk.begin(1)
            guard.assert_not_called()
        report = desk.summary()
        self.assertFalse(report["timing_trusted"])
        self.assertIsNone(report["loop_apm"])
        self.assertIsNone(report["accepted_apm"])

    def test_preparation_persists_the_clock_with_the_session(self):
        from fixture_apm import prepare
        with tempfile.TemporaryDirectory() as parent, patch("fixture_apm.subprocess.run", return_value=Mock(returncode=0)):
            folder = Path(parent) / "fixture"
            self.assertEqual(prepare(folder, "random"), 0)
            self.assertEqual(Desk(folder).session["clock"], CLOCK_ID)


if __name__ == "__main__":
    unittest.main()
