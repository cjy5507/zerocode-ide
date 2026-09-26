"""The reflex bench (t-6767): the round's stimulus is the seed's alone, the
fixture's own record decides every success, and the runtime's receipts are a
claim that record must confirm. Nothing here moves the pointer: every run is
a record written in the shapes the fixture, the driver and the runner write."""
import copy
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest import mock

import fixture_reflex as reflex
import tally

VALUES = tally.table()
LIMITS = reflex.limits()
T0 = 5_000_000_000_000  # an uptime, in nanoseconds
MS = 1_000_000
HELPER = 4242
FIXTURE = 777
OWNER = "a1b2c3d4e5f6"
# The display and the fixture's window, as the window server names them.
GEOMETRY = {
    "display": {"index": 0, "x": 0, "y": 0, "width": 1512, "height": 982, "scale": 2},
    "window": {"id": 91, "x": 396, "y": 271, "width": 720, "height": 440},
}


def run_window(record):
    """The acting window of a record: the helper's acceptance to its deadline."""
    started = record["started"]
    return started["acceptedNs"], started["deadlineNs"]


def clean(seed=7):
    """A run that did its job: every target inside the acting window hit
    150 ms after it showed, by the hand, each click claimed and confirmed."""
    schedule = reflex.schedule(seed, VALUES)
    run_ns = VALUES["reflex_round"]["run_s"] * 1_000 * MS
    accepted = T0 + 2_400 * MS
    deadline = accepted + run_ns
    events, receipts, frames = [], [], []
    hits = 0

    def event(kind, at, x, y, **more):
        events.append({"n": len(events) + 1, "kind": kind, "evNs": at, "rxNs": at + MS // 2,
                       "x": x, "y": y, "sourcePid": HELPER, "userData": 99, **more})

    for target in schedule["targets"]:
        appear = T0 + target["appearMs"] * MS
        expire = T0 + target["expireMs"] * MS
        if appear < accepted or expire > deadline:
            continue
        frames.append({"seq": len(frames) + 1, "ns": appear, "phase": target["phase"], "shown": [target["id"]]})
        move_at = appear + 60 * MS
        for step in range(10):
            event("move", move_at + step * 8 * MS, target["x"] - 10 + step, target["y"])
        down = appear + 150 * MS
        event("down", down, target["x"], target["y"], button=0, judged={"hit": target["id"]})
        event("up", down + 50 * MS, target["x"], target["y"], button=0)
        hits += 1
        rule = f"hit_{target['colour']}"
        receipts.append({"seq": len(receipts) + 1, "ruleId": rule, "actionId": f"move_{target['colour']}",
                         "outcome": "done", "decidedHostNs": appear + 30 * MS, "admittedHostNs": appear + 31 * MS,
                         "captureWaitNs": 4 * MS, "firstEventHostNs": move_at, "firstEventFrameHostNs": appear + 50 * MS,
                         "downHostNs": None, "upHostNs": None, "endedHostNs": move_at + 80 * MS, "events": 10})
        receipts.append({"seq": len(receipts) + 1, "ruleId": rule, "actionId": f"click_{target['colour']}",
                         "outcome": "done", "decidedHostNs": appear + 30 * MS, "admittedHostNs": move_at + 81 * MS,
                         "captureWaitNs": 1 * MS, "firstEventHostNs": down + MS // 4, "firstEventFrameHostNs": down - 5 * MS,
                         "downHostNs": down + MS // 4, "upHostNs": down + 50 * MS, "endedHostNs": down + 51 * MS, "events": 2})
    return {
        "run": {"owner": OWNER, "seed": seed, "t0Ns": T0, "verdictNs": deadline + 700 * MS, "fixturePid": FIXTURE,
                "stoppedBy": None},
        "schedule": schedule,
        "geometry": {**copy.deepcopy(GEOMETRY), "helperPid": HELPER},
        "ready": {"owner": OWNER, "pid": FIXTURE, "readyNs": T0 - 3_000 * MS, "events": 0, "hits": 0, "misses": 0,
                  "pointerInside": True},
        "fixture": {"owner": OWNER, "pid": FIXTURE, "t0Ns": T0, "seed": seed, "frames": len(frames),
                    "firstFrameNs": T0, "lastFrameNs": deadline + 500 * MS, "downs": hits, "ups": hits,
                    "hits": hits, "misses": {}, "held": [], "becameActive": False},
        "frames": frames,
        "events": events,
        "started": {"runId": "rx-1", "requestNs": T0 + 2_300 * MS, "acceptedNs": accepted,
                    "answeredNs": accepted + 20 * MS, "deadlineNs": deadline},
        "receipts": receipts,
        "ended": {"report": {"verified": True, "through": len(receipts), "writeFailures": 0, "ended": True},
                  "status": {"state": "stopped", "reason": "deadline", "receiptsIssued": len(receipts),
                             "fires": hits, "othersHeard": 0, "monitor": "hearing"},
                  "endedNs": deadline + 5 * MS, "stoppedBy": None},
    }


def failed(verdict):
    return sorted(check["check"] for check in verdict["checks"] if not check["passed"])


class RedFirst(unittest.TestCase):
    """The brief's six (t-6723 §7, brief-realtime-R4.md) and the coordinator's F4."""

    def test_a_clean_run_passes_and_counts_its_hits(self):
        record = clean()
        verdict = reflex.verdict(record, VALUES, LIMITS)
        self.assertEqual(verdict["verdict"], "pass", failed(verdict))
        measured = reflex.measure(record, VALUES, LIMITS)
        self.assertEqual(measured["hits"], record["fixture"]["hits"])
        self.assertGreater(measured["hits"], 150, "the clean record hits the round's targets")
        self.assertEqual(measured["wrong_inputs"], 0)

    def test_no_op_and_restored_state_never_pass_the_oracle(self):
        # Nothing done: the hand never pressed, the runtime's own word aside.
        idle = clean()
        idle["events"] = [event for event in idle["events"] if event["kind"] == "move"]
        idle["fixture"].update(downs=0, ups=0, hits=0)
        verdict = reflex.verdict(idle, VALUES, LIMITS)
        self.assertEqual(verdict["verdict"], "fail")
        self.assertIn("the run acted", failed(verdict))
        self.assertIn("every done click is a fixture hit", failed(verdict))
        self.assertEqual(reflex.measure(idle, VALUES, LIMITS)["hits"], 0)
        # A fixture that came up holding an earlier round's hits.
        restored = clean()
        restored["ready"].update(hits=3, events=40)
        verdict = reflex.verdict(restored, VALUES, LIMITS)
        self.assertEqual(verdict["verdict"], "fail")
        self.assertIn("starts clean", failed(verdict))
        # Another fixture's record, or one started on another goal.
        for change in ({"owner": "another"}, {"t0Ns": T0 + 1}):
            foreign = clean()
            foreign["fixture"].update(change)
            self.assertIn("starts clean", failed(reflex.verdict(foreign, VALUES, LIMITS)))

    def test_waypoints_and_key_repeat_do_not_count_as_actions(self):
        record = clean()
        before = reflex.measure(record, VALUES, LIMITS)
        busy = clean()
        last = busy["events"][-1]["evNs"]
        for step in range(500):
            busy["events"].append({"n": 0, "kind": "move", "evNs": last + step * MS, "rxNs": last + step * MS,
                                   "x": 100, "y": 100 + step % 7, "sourcePid": HELPER, "userData": 99})
        after = reflex.measure(busy, VALUES, LIMITS)
        self.assertEqual(after["hits"], before["hits"], "a waypoint is not an action")
        self.assertEqual(after["apm"], before["apm"])
        self.assertEqual(reflex.verdict(busy, VALUES, LIMITS)["verdict"], "pass")
        # A held key: one press and its repeats. It is a wrong input in a round
        # that presses no key — once, however long it was held.
        keyed = clean()
        for step, repeat in enumerate((False, True, True, True, True)):
            keyed["events"].append({"n": 0, "kind": "key", "evNs": last + step * 30 * MS, "rxNs": last + step * 30 * MS,
                                    "x": 0, "y": 0, "sourcePid": HELPER, "userData": 99, "repeat": repeat, "key": 0})
        measured = reflex.measure(keyed, VALUES, LIMITS)
        self.assertEqual(measured["hits"], before["hits"])
        self.assertEqual(measured["wrong_inputs"], 1, "a key's repeats are one input")
        self.assertIn("no wrong input", failed(reflex.verdict(keyed, VALUES, LIMITS)))

    def test_stimuli_are_independent_of_policy_reads(self):
        # The round is the seed's: drawn twice it is the same, another seed another round.
        self.assertEqual(reflex.schedule(7, VALUES), reflex.schedule(7, VALUES))
        self.assertNotEqual(reflex.schedule(7, VALUES)["targets"], reflex.schedule(8, VALUES)["targets"])
        # The policy's input never carries the seed: the plan for two rounds is one plan.
        bundle = f"dev.zerocode.bench.reflex.{OWNER}"
        contract = reflex.contract()
        self.assertEqual(reflex.plan(GEOMETRY, VALUES, bundle, contract), reflex.plan(GEOMETRY, VALUES, bundle, contract))
        words = json.dumps(reflex.plan(GEOMETRY, VALUES, bundle, contract))
        for target in reflex.schedule(7, VALUES)["targets"][:20]:
            self.assertNotIn(target["id"], words)
        # What the oracle holds a run to comes from the round and the fixture,
        # never from what the runtime read or claimed.
        record = clean()
        told = clean()
        told["receipts"] = [dict(receipt, outcome="moved") for receipt in told["receipts"]]
        told["ended"]["status"]["fires"] = 3
        self.assertEqual(reflex.due(record, VALUES, LIMITS), reflex.due(told, VALUES, LIMITS))
        # Every scheduled target of the round fits its phase: one colour a phase,
        # one target at a time, a clean gap between them.
        schedule = reflex.schedule(7, VALUES)
        for before, after in zip(schedule["targets"], schedule["targets"][1:]):
            self.assertLess(before["expireMs"], after["appearMs"])
        for target in schedule["targets"]:
            phase = schedule["phases"][target["phase"]]
            self.assertEqual(target["colour"], phase["colour"])
            self.assertGreaterEqual(target["appearMs"], phase["startMs"])
            self.assertLessEqual(target["expireMs"], phase["endMs"])

    def test_wrong_or_late_actions_fail_even_when_cli_says_ok(self):
        record = clean()
        downs = [event for event in record["events"] if event["kind"] == "down"]
        # The fixture judged one press late and one on a decoy; every receipt still says done.
        downs[3]["judged"] = {"miss": "late", "near": downs[3]["judged"]["hit"]}
        downs[9]["judged"] = {"miss": "decoy"}
        record["fixture"]["hits"] -= 2
        record["fixture"]["misses"] = {"late": 1, "decoy": 1}
        self.assertTrue(all(receipt["outcome"] == "done" for receipt in record["receipts"]))
        verdict = reflex.verdict(record, VALUES, LIMITS)
        self.assertEqual(verdict["verdict"], "fail")
        self.assertIn("no wrong input", failed(verdict))
        self.assertIn("every done click is a fixture hit", failed(verdict))
        measured = reflex.measure(record, VALUES, LIMITS)
        self.assertEqual(measured["wrong_inputs"], 2)
        self.assertEqual(measured["claims"]["unconfirmed"], 2)
        # A hit the fixture named on a target of the other phase is not the round's.
        crossed = clean()
        down = next(event for event in crossed["events"] if event["kind"] == "down")
        other = next(target for target in crossed["schedule"]["targets"]
                     if target["colour"] != crossed["schedule"]["targets"][0]["colour"])
        down["judged"] = {"hit": other["id"]}
        self.assertIn("every hit is the round's", failed(reflex.verdict(crossed, VALUES, LIMITS)))

    def test_planner_and_tactical_time_are_in_the_wall(self):
        record = clean()
        measured = reflex.measure(record, VALUES, LIMITS)
        wall_min = (record["run"]["verdictNs"] - T0) / 60e9
        self.assertAlmostEqual(measured["apm"], measured["hits"] / wall_min, places=6)
        self.assertLess(measured["apm"], measured["apm_steady"], "the goal's wall holds the preparation")
        # The same hits after a longer preparation earn less.
        slow = clean()
        slow["run"]["t0Ns"] = slow["fixture"]["t0Ns"] = T0 - 20_000 * MS
        self.assertLess(reflex.measure(slow, VALUES, LIMITS)["apm"], measured["apm"])
        # A plan made before the goal came is refused, and so is a wall under the floor.
        early = clean()
        early["started"]["requestNs"] = T0 - MS
        self.assertIn("the wall starts at the goal", failed(reflex.verdict(early, VALUES, LIMITS)))
        short = clean()
        short["run"]["verdictNs"] = T0 + 30_000 * MS
        self.assertIn("the wall starts at the goal", failed(reflex.verdict(short, VALUES, LIMITS)))

    def test_trace_overflow_cannot_claim_a_verified_apm(self):
        for change in ({"verified": False}, {"through": 3}):
            record = clean()
            record["ended"]["report"].update(change)
            verdict = reflex.verdict(record, VALUES, LIMITS)
            self.assertEqual(verdict["verdict"], "fail")
            self.assertIn("the trace is whole", failed(verdict))
            self.assertIsNone(reflex.measure(record, VALUES, LIMITS)["verified_apm"])
        self.assertIsNotNone(reflex.measure(clean(), VALUES, LIMITS)["verified_apm"])

    def test_a_judgment_only_done_is_not_a_success_sample(self):
        # Every leaf says done and the run reports itself verified, but the
        # fixture received no press: nothing succeeded.
        record = clean()
        record["events"] = [event for event in record["events"] if event["kind"] == "move"]
        record["fixture"].update(downs=0, ups=0, hits=0)
        self.assertTrue(record["ended"]["report"]["verified"])
        measured = reflex.measure(record, VALUES, LIMITS)
        self.assertEqual((measured["hits"], measured["apm"]), (0, 0))
        self.assertEqual(measured["claims"]["done"], sum(1 for r in record["receipts"] if r["downHostNs"]))
        self.assertEqual(reflex.verdict(record, VALUES, LIMITS)["verdict"], "fail")


class Safety(unittest.TestCase):
    """A run moves the real pointer: it starts only when nobody is at the
    machine, and the first input that is not the hand's stops it for good."""

    def test_a_persons_input_stops_the_run_and_nothing_resumes_it(self):
        watch = reflex.Supervisor(helper_pid=HELPER)
        self.assertIsNone(watch.status({"state": "running", "monitor": "hearing", "othersHeard": 0}))
        stop = watch.status({"state": "paused", "reason": "external_input", "monitor": "hearing", "othersHeard": 1})
        self.assertEqual(stop, "stop")
        self.assertEqual(watch.stopped_by, "person")
        # A paused run is stopped, never resumed: whatever comes after asks nothing more.
        for later in ({"state": "running", "monitor": "hearing", "othersHeard": 1},
                      {"state": "paused", "reason": "external_input", "monitor": "hearing", "othersHeard": 2}):
            self.assertIsNone(watch.status(later))
        self.assertEqual(watch.sent, ["stop"])
        self.assertNotIn("resume", reflex.DRIVER_WORDS)

    def test_a_monitor_that_cannot_hear_or_an_input_from_elsewhere_stops_the_run(self):
        for status, why in (({"state": "running", "monitor": "interrupted", "othersHeard": 0}, "monitor"),
                            ({"state": "running", "monitor": "unavailable", "othersHeard": 0}, "monitor"),
                            ({"state": "running", "monitor": "hearing", "othersHeard": 1}, "person")):
            watch = reflex.Supervisor(helper_pid=HELPER)
            self.assertEqual(watch.status(status), "stop")
            self.assertEqual(watch.stopped_by, why)
        watch = reflex.Supervisor(helper_pid=HELPER)
        self.assertIsNone(watch.fixture_event({"kind": "move", "sourcePid": HELPER}))
        self.assertEqual(watch.fixture_event({"kind": "move", "sourcePid": 0}), "stop")
        self.assertEqual(watch.stopped_by, "person")
        # The record agrees: a run a person touched is not judged, never passed.
        touched = clean()
        touched["events"][5]["sourcePid"] = 0
        self.assertEqual(reflex.verdict(touched, VALUES, LIMITS)["verdict"], "aborted")
        stopped = clean()
        stopped["run"]["stoppedBy"] = "person"
        self.assertEqual(reflex.verdict(stopped, VALUES, LIMITS)["verdict"], "aborted")

    def test_another_round_standing_anywhere_keeps_this_one_from_starting(self):
        own = 500
        listing = "\n".join([
            f"{own} 1 python3 fixture_reflex.py run /tmp/bench --seed 1",       # this runner
            f"501 {own} /tmp/bench/ReflexFixture-a.app/Contents/MacOS/ReflexFixture r.json /tmp/run",  # its fixture
            "777 1 /usr/bin/python3 /Users/dev/elsewhere/tools/computer-bench/fixture_reflex.py run /tmp/b2 --seed 9",
            "778 1 /Users/dev/elsewhere/target/debug/deps/zerocode_shell-0f " + reflex.DRIVER_TEST + " --exact --ignored",
            "779 1 /tmp/b3/ReflexFixture-b.app/Contents/MacOS/ReflexFixture round.json /tmp/b3/run-1",
            "780 1 python3 fixture_reflex.py rehearse /tmp/b4 --seed 2",
            "781 1 /bin/zsh -c grep fixture_reflex",
        ])
        standing = reflex.other_benches(own, listing)
        self.assertEqual(len(standing), 4, standing)
        self.assertFalse(any(line.startswith("python3 fixture_reflex.py run /tmp/bench") for line in standing))
        self.assertEqual(reflex.other_benches(own, "\n".join(listing.splitlines()[:2])), [], "its own tree is no other round")
        # The runner refuses before anything shows: no fixture, no folder.
        folder = pathlib.Path(tempfile.mkdtemp())
        try:
            (folder / "session.json").write_text(json.dumps({"owner": OWNER, "app": "-", "executable": "/nowhere",
                                                            "bundle": f"{reflex.BUNDLE_PREFIX}.{OWNER}"}))
            desk = reflex.Desk(folder, VALUES, LIMITS)
            with mock.patch.object(reflex.Bench, "hid_idle_s", return_value=VALUES["reflex_safety"]["idle_s"] + 1), \
                    mock.patch.object(reflex.Bench, "screen_locked", return_value=False), \
                    mock.patch.object(reflex.Desk, "another_operator", return_value=None), \
                    mock.patch.object(reflex, "other_benches", return_value=[standing[0]]):
                with self.assertRaises(reflex.Refused) as refused:
                    desk.run(41, "/nowhere/driver", "/nowhere/helper.app")
            self.assertIn("another reflex round", str(refused.exception))
            self.assertFalse((folder / "run-41").exists())
        finally:
            import shutil
            shutil.rmtree(folder, ignore_errors=True)

    def test_a_run_the_helper_refused_is_not_judged_and_says_why(self):
        refused = clean()
        refused["started"] = None
        refused["receipts"] = []
        refused["ended"] = {"refused": {"code": "monitor_unavailable", "message": "Input Monitoring is not granted"}}
        result = reflex.judged(refused, VALUES, LIMITS)
        self.assertEqual(result["verdict"]["verdict"], "aborted")
        self.assertIn("monitor_unavailable", result["verdict"]["reason"])
        self.assertIsNone(result["success"])
        self.assertIsNone(result["measure"])

    def test_a_run_starts_only_after_two_idle_minutes_with_the_pointer_on_the_fixture(self):
        idle_s = VALUES["reflex_safety"]["idle_s"]
        ready = {"pointerInside": True}
        self.assertIsNone(reflex.refusal(idle_s=idle_s + 1, locked=False, ready=ready, values=VALUES))
        self.assertIn("idle", reflex.refusal(idle_s=idle_s - 1, locked=False, ready=ready, values=VALUES))
        self.assertIn("locked", reflex.refusal(idle_s=idle_s + 1, locked=True, ready=ready, values=VALUES))
        self.assertIn("pointer", reflex.refusal(idle_s=idle_s + 1, locked=False, ready={"pointerInside": False},
                                               values=VALUES))


class Plan(unittest.TestCase):
    """The plan the bench writes for the goal: read off the window the window
    server named and the table, in the contract's own words."""

    def setUp(self):
        self.bundle = f"dev.zerocode.bench.reflex.{OWNER}"
        self.plan = reflex.plan(GEOMETRY, VALUES, self.bundle, reflex.contract())

    def test_the_plan_speaks_the_contracts_words(self):
        golden = json.loads((reflex.FIXTURES / "reflex-contract/valid_basic.json").read_text())["plan"]
        self.assertEqual(set(self.plan), set(golden))
        self.assertEqual(self.plan["version"], golden["version"])
        self.assertEqual(self.plan["plan_hash"], "", "the driver hashes it with the core")
        self.assertEqual(self.plan["scope"], {"surface": "macos_desktop", "target": self.bundle})
        for detector in self.plan["detectors"]:
            self.assertEqual(set(detector), set(golden["detectors"][0]))
            self.assertEqual(set(detector["color"]) - {"anchors"}, set(golden["detectors"][0]["color"]))
        for rule in self.plan["rules"]:
            self.assertEqual(set(rule), set(golden["rules"][0]))
        self.assertEqual(self.plan["pointer"]["duration_ms"], VALUES["reflex_plan"]["pointer_ms"])

    def test_the_field_and_the_strip_are_where_the_window_is(self):
        window, hud, edge = GEOMETRY["window"], VALUES["reflex_hud_pt"], VALUES["reflex_target"]["edge_pt"]
        for detector in self.plan["detectors"]:
            roi = detector["roi"]
            self.assertEqual((roi["x"], roi["y"]), (window["x"] + edge, window["y"] + hud + edge))
            self.assertEqual((roi["width"], roi["height"]), (window["width"] - 2 * edge, window["height"] - hud - 2 * edge))
            colour = detector["color"]
            self.assertEqual((colour["reference_width"], colour["reference_height"]), (1512, 982))
            for anchor in colour["anchors"]:
                self.assertTrue(window["x"] <= anchor["x"] < window["x"] + window["width"])
                self.assertTrue(window["y"] <= anchor["y"] < window["y"] + hud)
        # A window on a second display is read against that display's own origin.
        moved = copy.deepcopy(GEOMETRY)
        moved["display"].update(x=1512, index=1)
        moved["window"]["x"] += 1512
        roi = reflex.plan(moved, VALUES, self.bundle, reflex.contract())["detectors"][0]["roi"]
        self.assertEqual(roi["x"], window["x"] + edge)
        # Every target and decoy of a round lies inside the field the plan reads.
        schedule = reflex.schedule(7, VALUES)
        for shape in schedule["targets"] + schedule["decoys"]:
            life_s = (shape["expireMs"] - shape["appearMs"]) / 1_000
            for x, y in ((shape["x"], shape["y"]),
                         (shape["x"] + shape.get("vx", 0) * life_s, shape["y"] + shape.get("vy", 0) * life_s)):
                self.assertTrue(edge + shape["radius"] <= x <= window["width"] - edge - shape["radius"], shape["id"])
                self.assertTrue(hud + edge + shape["radius"] <= y <= window["height"] - edge - shape["radius"], shape["id"])

    def test_each_colour_reads_only_under_its_own_strip(self):
        palette = VALUES["reflex_palette"]
        by_id = {detector["id"]: detector for detector in self.plan["detectors"]}
        self.assertEqual(set(by_id), set(reflex.COLOURS))
        for number, colour in enumerate(reflex.COLOURS, start=1):
            spec = by_id[colour]["color"]
            self.assertEqual(spec["layout"]["class"], number)
            self.assertTrue(all(anchor["class"] == number for anchor in spec["anchors"]))
            self.assertEqual([{key: c[key] for key in ("r", "g", "b")} for c in spec["classes"]],
                             [{key: palette[name][key] for key in ("r", "g", "b")} for name in reflex.COLOURS])
        rules = {rule["detector"]: rule for rule in self.plan["rules"]}
        self.assertEqual(set(rules), set(reflex.COLOURS))
        for rule in rules.values():
            self.assertEqual(rule["predicate"], {"op": "eq", "value": 1})
            self.assertEqual(rule["max_fires"], VALUES["reflex_plan"]["quota"])


class Fixture(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "darwin", "the fixture is an AppKit app")
    def test_the_fixture_builds_as_the_runner_builds_it(self):
        # prepare's own flags: Swift 6, every warning an error.
        built = subprocess.run(["swiftc", "-typecheck", "-swift-version", "6", "-warnings-as-errors", str(reflex.SOURCE)],
                               capture_output=True, text=True)
        self.assertEqual(built.returncode, 0, built.stderr)

    def test_the_round_file_carries_every_number_the_fixture_draws_with(self):
        drawn = reflex.the_round(OWNER, 7, VALUES, LIMITS)
        self.assertEqual(drawn["frameAgeNs"], LIMITS["max_frame_age_ns"])
        self.assertEqual(drawn["framesPerSecond"], LIMITS["frames_per_second"])
        self.assertEqual(drawn["schedule"], reflex.schedule(7, VALUES))
        source = reflex.SOURCE.read_text()
        for name in ("canvas", "hud", "palette", "framesPerSecond", "frameAgeNs", "schedule", "owner", "seed"):
            self.assertIn(f"let {name}:", source, f"the fixture reads {name} from the round")
        for colour in (*reflex.COLOURS, "ground", "curtain", "idle"):
            self.assertIn(colour, VALUES["reflex_palette"])


# A stand-in fixture: it comes up with the pointer on it, starts its round at
# the goal and keeps its record until it is ended — no window, no input.
FAKE_FIXTURE = textwrap.dedent(r"""
    import json, os, pathlib, signal, sys, time
    run = pathlib.Path(sys.argv[2])
    now = lambda: time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    state = {"owner": json.loads(pathlib.Path(sys.argv[1]).read_text())["owner"], "pid": os.getpid(),
             "frames": 0, "downs": 0, "ups": 0, "hits": 0, "misses": {}, "held": [], "events": 0}
    def flush(*_):
        state["lastFrameNs"] = now()
        (run / "fixture.json").write_text(json.dumps(state))
        if _:
            sys.exit(0)
    signal.signal(signal.SIGTERM, flush)
    (run / "ready.json").write_text(json.dumps({"owner": state["owner"], "pid": os.getpid(), "readyNs": now(),
        "events": 0, "hits": 0, "misses": 0, "pointerInside": True, "pointer": {"x": 10, "y": 20}}))
    while True:
        start = run / "start.json"
        if start.exists() and "t0Ns" not in state:
            state["t0Ns"] = json.loads(start.read_text())["t0Ns"]
            state["firstFrameNs"] = now()
        state["frames"] += 1
        flush()
        time.sleep(0.01)
""")

# A stand-in driver speaking the folder protocol of reflex/bench.rs. With
# FAKE_PERSON set it reports a paused run until the runner says stop.
FAKE_DRIVER = textwrap.dedent(r"""
    import json, os, pathlib, sys, threading, time
    run = pathlib.Path(os.environ["ZEROCODE_REFLEX_BENCH_DIR"])
    now = lambda: time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    request = json.loads((run / "request.json").read_text())
    stopped = threading.Event()
    threading.Thread(target=lambda: [stopped.set() for line in sys.stdin if line.strip() == "stop"], daemon=True).start()
    (run / "geometry.json").write_text(json.dumps({"display": {"index": 0, "x": 0, "y": 0, "width": 1512,
        "height": 982, "scale": 2}, "window": {"id": 1, "x": 100, "y": 100, "width": 720, "height": 440},
        "helperPid": os.getpid()}))
    while not (run / "plan.json").exists():
        time.sleep(0.01)
    accepted = now()
    deadline = accepted + int(request["seconds"] * 1e9)
    (run / "started.json").write_text(json.dumps({"runId": "rx-fake", "requestNs": accepted, "acceptedNs": accepted,
        "answeredNs": accepted, "deadlineNs": deadline}))
    person = bool(os.environ.get("FAKE_PERSON"))
    while now() < deadline and not stopped.is_set():
        status = {"state": "paused", "reason": "external_input", "monitor": "hearing", "othersHeard": 1} if person \
            else {"state": "running", "monitor": "hearing", "othersHeard": 0}
        with (run / "status.jsonl").open("a") as handle:
            handle.write(json.dumps({"ns": now(), "status": status}) + "\n")
        time.sleep(0.02)
    reason = "request" if stopped.is_set() else "deadline"
    (run / "ended.json").write_text(json.dumps({"report": {"verified": True, "through": 0, "ended": True},
        "status": {"state": "stopped", "reason": reason, "receiptsIssued": 0, "fires": 0},
        "endedNs": now(), "stoppedBy": "stop" if stopped.is_set() else None}))
""")


class Runner(unittest.TestCase):
    """The runner end to end with a stand-in fixture and driver: no window,
    no helper, no input."""

    def setUp(self):
        self.folder = pathlib.Path(tempfile.mkdtemp())
        fixture, driver = self.folder / "fixture.py", self.folder / "driver.py"
        fixture.write_text(FAKE_FIXTURE)
        driver.write_text(FAKE_DRIVER)
        for script in (fixture, driver):
            script.write_text("#!" + sys.executable + "\n" + script.read_text())
            script.chmod(0o755)
        (self.folder / "session.json").write_text(json.dumps({"owner": OWNER, "app": "-", "executable": str(fixture),
                                                              "bundle": f"{reflex.BUNDLE_PREFIX}.{OWNER}"}))
        self.values = copy.deepcopy(VALUES)
        self.values["reflex_safety"].update(announce_s=0, ready_s=5, prep_s=5, grace_s=5, poll_ms=10)
        self.values["reflex_round"]["run_s"] = 0.3
        self.driver = str(driver)

    def tearDown(self):
        import shutil
        shutil.rmtree(self.folder, ignore_errors=True)

    def run_desk(self, seed, **env):
        desk = reflex.Desk(self.folder, self.values, LIMITS)
        with mock.patch.object(reflex.Bench, "hid_idle_s", return_value=VALUES["reflex_safety"]["idle_s"] + 1), \
                mock.patch.object(reflex.Bench, "screen_locked", return_value=False), \
                mock.patch.object(reflex.Desk, "another_operator", return_value=None), \
                mock.patch.object(reflex, "other_benches", return_value=[]), \
                mock.patch.dict(os.environ, env):
            return desk.run(seed, self.driver, "/nowhere/helper.app"), self.folder / f"run-{seed}"

    def test_a_run_goes_from_the_goal_to_a_verdict_and_every_file_is_kept(self):
        result, run = self.run_desk(31)
        for name in ("round.json", "ready.json", "start.json", "request.json", "geometry.json", "plan.json",
                     "started.json", "status.jsonl", "ended.json", "fixture.json", "run.json", tally.REFLEX_RUN):
            self.assertTrue((run / name).exists(), name)
        record = json.loads((run / "run.json").read_text())
        self.assertIsNone(record["stoppedBy"])
        self.assertEqual(sorted(record["load"]), ["goal", "verdict"], "the machine's load is kept beside the run")
        self.assertLessEqual(record["t0Ns"], json.loads((run / "started.json").read_text())["requestNs"])
        self.assertGreater(record["verdictNs"], json.loads((run / "ended.json").read_text())["endedNs"])
        self.assertEqual(json.loads((run / "plan.json").read_text())["scope"]["target"],
                         f"{reflex.BUNDLE_PREFIX}.{OWNER}")
        # Nothing was hit, so the oracle fails the run — and it judged it, from the files.
        self.assertEqual(result["verdict"]["verdict"], "fail")
        self.assertIn("the run acted", [check["check"] for check in result["verdict"]["checks"] if not check["passed"]])
        # One run a seed: the folder that exists is a run that happened.
        with self.assertRaises(FileExistsError):
            self.run_desk(31)

    def test_a_persons_input_during_a_run_stops_it_and_the_run_is_not_judged(self):
        result, run = self.run_desk(32, FAKE_PERSON="1")
        record = json.loads((run / "run.json").read_text())
        self.assertEqual(record["stoppedBy"], "person")
        self.assertEqual(json.loads((run / "ended.json").read_text())["stoppedBy"], "stop")
        self.assertEqual(result["verdict"]["verdict"], "aborted")
        self.assertIsNone(result["success"])

    def test_nobody_idle_long_enough_means_no_window_and_no_run(self):
        desk = reflex.Desk(self.folder, self.values, LIMITS)
        with mock.patch.object(reflex.Bench, "hid_idle_s", return_value=VALUES["reflex_safety"]["idle_s"] - 1), \
                mock.patch.object(reflex.Bench, "screen_locked", return_value=False), \
                mock.patch.object(reflex.Desk, "another_operator", return_value=None):
            with self.assertRaises(reflex.Refused):
                desk.run(33, self.driver, "/nowhere/helper.app")
        self.assertFalse((self.folder / "run-33").exists(), "refused before the fixture came up")


class Retries(unittest.TestCase):
    def test_several_rules_a_colour_try_a_standing_target_again_inside_the_budget(self):
        bundle = f"dev.zerocode.bench.reflex.{OWNER}"
        one = reflex.plan(GEOMETRY, VALUES, bundle, reflex.contract())
        self.assertEqual(len(one["rules"]), len(reflex.COLOURS), "the table's default is one rule a colour")
        four = reflex.plan(GEOMETRY, VALUES, bundle, reflex.contract(), rules=4)
        self.assertEqual(len(four["rules"]), 4 * len(reflex.COLOURS))
        self.assertEqual(len({rule["id"] for rule in four["rules"]}), len(four["rules"]))
        for colour in reflex.COLOURS:
            mine = [rule for rule in four["rules"] if rule["detector"] == colour]
            self.assertEqual(sorted(rule["priority"] for rule in mine), [1, 2, 3, 4])
            self.assertTrue(all(rule["macro_id"] == f"tap_{colour}" and rule["predicate"] == {"op": "eq", "value": 1}
                                for rule in mine))
        self.assertEqual(four["detectors"], one["detectors"], "the retries change the rules, never what is read")
        for plan in (one, four, reflex.plan(GEOMETRY, VALUES, bundle, reflex.contract(), rules=16)):
            self.assertLessEqual(sum(rule["max_fires"] for rule in plan["rules"]) * 2, LIMITS["max_expanded_actions"])
            self.assertTrue(all(rule["max_fires"] >= 1 for rule in plan["rules"]))


class Tally(unittest.TestCase):
    def test_a_reflex_run_is_tallied_from_its_record(self):
        record = clean()
        with tempfile.TemporaryDirectory() as folder:
            result = reflex.judged(record, VALUES, LIMITS)
            with open(os.path.join(folder, tally.REFLEX_RUN), "w", encoding="utf-8") as handle:
                json.dump(result, handle)
            rows = tally.collect(folder)
            self.assertEqual(len(rows), 1)
            row = rows[0]
            self.assertEqual(row["lane"], "reflex")
            self.assertTrue(row["success"])
            self.assertEqual(row["hits"], record["fixture"]["hits"])
            summary = tally.summarize_reflex(rows, VALUES)
            entry = next(iter(summary.values()))
            self.assertEqual(entry["runs"], 1)
            self.assertEqual(entry["wrong_inputs"], 0)
            rendered = tally.render_reflex(summary)
            for column in ("APM (goal)", "APM (steady)", "wrong", "oracle", "decisions"):
                self.assertIn(column, rendered)


if __name__ == "__main__":
    unittest.main()
