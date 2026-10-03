"""The reflex bench (t-6767): the round's stimulus is the seed's alone, the
fixture's own record decides every success, and the runtime's receipts are a
claim that record must confirm. Nothing here moves the pointer: every run is
a record written in the shapes the fixture, the driver and the runner write."""
import collections
import copy
import hashlib
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


def piloted(seed=7, sources=("model", "model")):
    """`clean`'s run carried by an autopilot that re-planned once halfway:
    two runs, each with its receipts, its status and its report, the
    autopilot's account, the helper's starts and stops as the driver heard
    them, and the bench home's two ledgers — the reflex decision's rows and
    the plans'."""
    record = clean(seed)
    accepted, deadline = run_window(record)
    half = accepted + (deadline - accepted) // 2
    runs = [("rx-1", [r for r in record["receipts"] if r["decidedHostNs"] < half]),
            ("rx-2", [r for r in record["receipts"] if r["decidedHostNs"] >= half])]
    receipts, ended_runs = [], []
    for run, mine in runs:
        for seq, receipt in enumerate(mine, start=1):
            receipts.append({**receipt, "seq": seq, "run": run})
        clicks = sum(1 for receipt in mine if receipt["actionId"].startswith("click"))
        ended_runs.append({"runId": run, "receipts": f"reflex-{run}.jsonl",
                           "report": {"verified": True, "through": len(mine), "writeFailures": 0, "ended": True},
                           "status": {"state": "stopped", "reason": "request" if run == "rx-1" else "deadline",
                                      "receiptsIssued": len(mine), "fires": clicks, "othersHeard": 0,
                                      "monitor": "hearing"}})
    record["receipts"] = receipts
    stop_ns, start_ns = half, half + 40 * MS
    record["calls"] = [
        {"method": "reflexStart", "run": "rx-1", "askedNs": accepted - 30 * MS, "answeredNs": accepted - 20 * MS,
         "deadlineNs": deadline, "refused": None},
        {"method": "reflexStop", "run": "rx-1", "askedNs": stop_ns, "answeredNs": stop_ns + 5 * MS,
         "deadlineNs": None, "refused": None},
        {"method": "reflexStart", "run": "rx-2", "askedNs": start_ns - 10 * MS, "answeredNs": start_ns,
         "deadlineNs": deadline, "refused": None},
    ]
    plans = [{"run": run, "epoch": epoch, "planHash": f"h{epoch}", "source": source, "promptVersion": 1,
              "requests": 1, "rttMs": 900 + epoch, "refusals": 0}
             for epoch, ((run, _), source) in enumerate(zip(runs, sources), start=1)]
    record["ended"].update({
        "road": "autopilot",
        "runs": ended_runs,
        "report": ended_runs[-1]["report"],
        "status": ended_runs[-1]["status"],
        "autopilot": {"id": "ra-1", "l1": {"forced": "auto"},
                      "roads": {"memo": 0, "surrogate": 0, "jev": 3},
                      "applied": {"continue": 2, "pause": 0, "replan": 1},
                      "invalid": {"stale": 1, "epoch_mismatch": 0, "plan_mismatch": 0, "not_auto": 0},
                      "unanswered": 1, "door": {}, "wire": {}, "plans": plans,
                      "ended": {"reason": "deadline", "said": "the helper ended the run"}},
        "ledgers": {"decisions": "home/requests/reflex-decide.jsonl", "plans": "home/requests/reflex-plan.jsonl"},
    })
    record["decisions"] = [
        {"run": "rx-1", "decision": number, "road": "jev", "attempts": 1, "rttMs": rtt, "outcome": outcome,
         "provenance": {"epoch": 1, "planHash": "h1", "forced": True}, "applied": applied}
        for number, (rtt, outcome, applied) in enumerate(
            ((200, "answered", True), (300, "answered", True), (400, "answered", True),
             (500, "answered", False), (1000, "timeout", False)), start=1)
    ] + [{"label": "rx-1:1", "requestAt": 1, "kind": "executed_outcome"}]
    record["plans"] = [
        {"run": run, "epoch": epoch, "source": source, "model": "claude-haiku-4-5-20251001",
         "requests": 1, "rttMs": 900 + epoch, "tokens": {"input": 2_000, "output": 800}, "outcome": "answered"}
        for epoch, ((run, _), source) in enumerate(zip(runs, sources), start=1)
    ] + [{"label": "rx-1", "requestAt": 1, "kind": "executed_outcome", "share": {}}]
    record["run"]["autopilot"] = {"generator": "window", "l1": "auto"}
    record["run"]["config"] = f"{reflex.CONFIG}+autopilot-window"
    return record


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

    def test_the_glide_may_be_asked_for_in_place_of_the_tables(self):
        # A run measured at another pace asks for it — 16 ms is two pointer ticks, R10's fast
        # profile; left out, the plan glides as the table says. The glide is all that changes (t-26708).
        fast = reflex.plan(GEOMETRY, VALUES, self.bundle, reflex.contract(), pointer_ms=16)
        self.assertEqual(fast["pointer"]["duration_ms"], 16)
        self.assertEqual(self.plan["pointer"]["duration_ms"], VALUES["reflex_plan"]["pointer_ms"])
        self.assertEqual({key: fast[key] for key in fast if key != "pointer"},
                         {key: self.plan[key] for key in self.plan if key != "pointer"})


class Fixture(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "darwin", "the fixture is an AppKit app")
    def test_the_fixture_builds_as_the_runner_builds_it(self):
        # prepare's own flags: Swift 6, every warning an error.
        built = subprocess.run(["swiftc", "-typecheck", "-swift-version", "6", "-warnings-as-errors", "-parse-as-library", str(reflex.HERE / "FixtureSupport.swift"), str(reflex.SOURCE)],
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
    goal = request.get("goal")
    if goal:
        # What the driver was handed: the home it keeps its ledgers in, and whether a Jev key came — never the key.
        (run / "env.json").write_text(json.dumps({"home": os.environ.get("ZO_CONFIG_HOME"),
            "jevKey": bool(os.environ.get("TYPESAFE_API_KEY")), "keys": sorted(k for k in os.environ if k.endswith("_KEY"))}))
    while (not goal or request.get("generator") == "stub") and not (run / "plan.json").exists():
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

    def run_desk(self, seed, autopilot=None, keychain=None, pointer_ms=None, kind=None, **env):
        desk = reflex.Desk(self.folder, self.values, LIMITS)
        with mock.patch.object(reflex.Bench, "hid_idle_s", return_value=VALUES["reflex_safety"]["idle_s"] + 1), \
                mock.patch.object(reflex.Bench, "screen_locked", return_value=False), \
                mock.patch.object(reflex.Desk, "another_operator", return_value=None), \
                mock.patch.object(reflex, "other_benches", return_value=[]), \
                mock.patch.object(reflex, "keychain", keychain or (lambda service, value=False: None)), \
                mock.patch.dict(os.environ, env):
            return (desk.run(seed, self.driver, "/nowhere/helper.app", autopilot=autopilot, pointer_ms=pointer_ms,
                             kind=kind),
                    self.folder / f"run-{seed}")

    def test_a_run_of_a_kind_plays_that_kinds_round_and_its_row_and_config_say_so(self):
        # The runner's short round (run_s above) ends before the table's first sweeper could arrive;
        # this round lasts past the latest launch and the longest flight, so an avoid round holds one.
        table, safety = self.values["reflex_kind"]["avoid"], self.values["reflex_safety"]
        latest_arrival_ms = self.values["reflex_round"]["lead_ms"] + table["every_ms"][1] + table["flight_ms"][1]
        self.values["reflex_round"]["run_s"] = latest_arrival_ms / 1_000 - safety["prep_s"] + safety["poll_ms"] / 1_000
        result, run = self.run_desk(39, kind="avoid")
        self.assertEqual(json.loads((run / "round.json").read_text()).get("kind"), "avoid")
        self.assertTrue(json.loads((run / "round.json").read_text())["schedule"].get("sweeps"), "the round has sweepers")
        self.assertEqual(json.loads((run / "run.json").read_text()).get("kind"), "avoid")
        self.assertEqual(result["config"], f"{reflex.CONFIG}+kind-avoid")
        self.assertIn("kind", result["measure"] or {}, "the kind's own numbers are measured")

    def test_a_run_may_ask_for_a_glide_and_its_config_says_so(self):
        # The plan the runner writes for the hand glides as asked, the run's row and its config
        # name the pace, and a run that asks for nothing is the table's (t-26708).
        result, run = self.run_desk(37, pointer_ms=16)
        self.assertEqual(json.loads((run / "plan.json").read_text())["pointer"]["duration_ms"], 16)
        self.assertEqual(json.loads((run / "run.json").read_text())["pointerMs"], 16)
        self.assertEqual(result["config"], f"{reflex.CONFIG}+pointer16")
        result, run = self.run_desk(38)
        self.assertEqual(json.loads((run / "plan.json").read_text())["pointer"]["duration_ms"],
                         VALUES["reflex_plan"]["pointer_ms"])
        self.assertEqual(json.loads((run / "run.json").read_text())["pointerMs"], VALUES["reflex_plan"]["pointer_ms"])
        self.assertEqual(result["config"], reflex.CONFIG)

    def test_an_autopilot_round_hands_over_the_goal_and_keeps_its_ledgers_home(self):
        secret = "k-" + "x" * 24
        jev = VALUES["reflex_goal"]["keys"]["jev"]
        keychain = lambda service, value=False: secret if service.endswith(jev) else None
        result, run = self.run_desk(34, autopilot={"generator": reflex.STUB}, keychain=keychain)
        request = json.loads((run / "request.json").read_text())
        self.assertEqual(request["goal"], VALUES["reflex_goal"]["words"])
        self.assertEqual((request["generator"], request["l1"]), (reflex.STUB, VALUES["reflex_goal"]["l1"]))
        home = pathlib.Path(request["home"])
        self.assertEqual(home.parent, run, "the bench's zo home is the run's own")
        settings = json.loads((home / "settings.json").read_text())
        self.assertEqual(settings["smart"]["jev"]["workspaces"], [str(run)])
        handed = json.loads((run / "env.json").read_text())
        self.assertEqual(handed["home"], str(home))
        self.assertTrue(handed["jevKey"])
        self.assertIn(jev, handed["keys"])
        self.assertNotIn(reflex.generator_key_name(), handed["keys"], "a stand-in is handed no generator key")
        self.assertTrue((run / "plan.json").exists(), "the stand-in answers with the runner's plan")
        record = json.loads((run / "run.json").read_text())
        self.assertEqual(record["autopilot"], {"generator": reflex.STUB, "l1": VALUES["reflex_goal"]["l1"]})
        self.assertEqual(result["config"], f"{reflex.CONFIG}+autopilot-{reflex.STUB}")
        for path in run.rglob("*"):
            if path.is_file():
                self.assertNotIn(secret, path.read_text(errors="replace"), f"the key reached {path.name}")

    def test_a_round_may_force_another_word_on_the_decision_and_its_config_says_so(self):
        result, run = self.run_desk(36, autopilot={"generator": reflex.STUB, "l1": "shadow"})
        self.assertEqual(json.loads((run / "request.json").read_text())["l1"], "shadow")
        self.assertEqual(result["config"], f"{reflex.CONFIG}+autopilot-{reflex.STUB}-l1shadow")

    def test_the_windows_login_generator_is_not_refused_for_an_absent_api_key(self):
        _, run = self.run_desk(35, autopilot={"generator": "window"})
        self.assertEqual(json.loads((run / "request.json").read_text())["generator"], "window")
        self.assertFalse((run / "plan.json").exists(), "the runner writes no model plan")

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


class RealtimeTimings(unittest.TestCase):
    def statuses(self, record, samples, run="rx-1"):
        record.setdefault("statuses", []).extend(
            {"ns": T0 + ms * MS, "status": {"runId": run, "framesEvaluated": frames}}
            for ms, frames in samples)

    def test_observation_uses_counter_deltas_not_poll_count_or_configured_fps(self):
        record = clean()
        self.statuses(record, [(100, 40), (120, 40), (200, 42)])
        seen = reflex.measure(record, VALUES, LIMITS)["observation"]
        self.assertEqual(seen["configured_hz"], LIMITS["frames_per_second"])
        self.assertIsNone(seen["capture_hz"], "evaluated frames do not count every camera capture")
        run = seen["runs"][0]
        self.assertEqual((run["run"], run["samples"], run["evaluated_frames"]), ("rx-1", 3, 2))
        self.assertEqual((run["sampled_s"], run["evaluated_hz"]), (0.1, 20.0))
        self.assertEqual(run["poll_interval_ms"], reflex.spread([20, 80]))

    def test_replans_keep_their_counter_windows_separate(self):
        record = piloted()
        self.statuses(record, [(100, 100), (200, 104)])
        self.statuses(record, [(180, 0), (380, 2)], run="rx-2")
        self.statuses(record, [(100, 0), (200, 999)], run="another-run")
        runs = reflex.measure(record, VALUES, LIMITS)["observation"]["runs"]
        self.assertEqual([(run["run"], run["evaluated_frames"], run["evaluated_hz"]) for run in runs],
                         [("rx-1", 4, 40.0), ("rx-2", 2, 10.0)])

    def test_missing_or_regressing_observation_samples_are_unknown_not_zero_hz(self):
        for samples in ([], [(100, 2)], [(100, 2), (200, 1)], [(200, 2), (100, 3)], [(100, 2), (100, 3)]):
            with self.subTest(samples=samples):
                record = clean()
                self.statuses(record, samples)
                run = reflex.measure(record, VALUES, LIMITS)["observation"]["runs"][0]
                self.assertIsNone(run["evaluated_hz"])
                self.assertIsNone(run["evaluated_frames"])
        record = clean()
        self.statuses(record, [(100, 2), (200, 2)])
        self.assertEqual(reflex.measure(record, VALUES, LIMITS)["observation"]["runs"][0]["evaluated_hz"], 0.0)

    def test_invalid_counters_cannot_become_observation_samples(self):
        for value in (None, True, -1, "2", 2.5):
            with self.subTest(value=value):
                record = clean()
                self.statuses(record, [(100, 0), (200, value)])
                run = reflex.measure(record, VALUES, LIMITS)["observation"]["runs"][0]
                self.assertEqual(run["samples"], 1)
                self.assertIsNone(run["evaluated_hz"])

    def test_first_event_freshness_includes_partial_input_not_refusals_without_input(self):
        record = clean()
        limit = LIMITS["max_frame_age_ns"]
        at = T0 + 1_000 * MS
        record["receipts"] = [
            {"events": 3, "outcome": "done", "firstEventHostNs": at, "firstEventFrameHostNs": at - limit},
            {"events": 1, "outcome": "cancelled", "firstEventHostNs": at, "firstEventFrameHostNs": at - limit - 1},
            {"events": 0, "outcome": "stale", "firstEventHostNs": None, "firstEventFrameHostNs": None},
        ]
        seen = reflex.first_event_freshness(record, LIMITS)
        self.assertEqual((seen["input_receipts"], seen["timed"], seen["unknown"], seen["stale"]), (2, 2, 0, 1))
        self.assertEqual((seen["input_events"], seen["events_without_age"]), (4, 2))
        self.assertEqual(seen["max_age_ms"], limit / MS)

    def test_missing_receipts_do_not_claim_zero_input_events(self):
        record = clean()
        record["receipts"] = []
        seen = reflex.first_event_freshness(record, LIMITS)
        self.assertIsNone(seen["input_events"])
        self.assertIsNone(seen["events_without_age"])
        self.assertIsNone(seen["stale"])
        record["receipts"] = [{"events": 0, "outcome": "stale"}]
        self.assertEqual(reflex.first_event_freshness(record, LIMITS)["input_events"], 0)

    def test_missing_or_invalid_age_is_not_a_fresh_input(self):
        for value in (None, 0, True, "bad", T0 + 1):
            with self.subTest(value=value):
                record = clean()
                record["receipts"] = [{"events": 2, "firstEventHostNs": T0, "firstEventFrameHostNs": value}]
                seen = reflex.first_event_freshness(record, LIMITS)
                self.assertEqual((seen["input_receipts"], seen["timed"], seen["unknown"]), (1, 0, 1))
                self.assertIsNone(seen["stale"])
                self.assertEqual(seen["events_without_age"], 2)
                self.assertEqual(seen["age_ms"], reflex.spread([]))
        record["receipts"].append({"outcome": "done"})
        seen = reflex.first_event_freshness(record, LIMITS)
        self.assertIsNone(seen["input_events"])
        self.assertIsNone(seen["events_without_age"])

    def effect_record(self):
        record = clean()
        event = copy.deepcopy(reflex.hits(record)[0])
        target = next(target for target in record["schedule"]["targets"] if target["id"] == event["judged"]["hit"])
        before = {"seq": 1, "ns": event["evNs"] - MS, "phase": target["phase"], "shown": [target["id"]]}
        after = {"seq": 2, "ns": event["rxNs"] + 10 * MS, "phase": target["phase"], "shown": []}
        record.update(events=[event], frames=[before, after])
        return record, target

    def test_effect_uses_the_fixture_update_not_receipt_completion(self):
        record, _ = self.effect_record()
        seen = reflex.effect_numbers(record)
        self.assertEqual((seen["hits"], seen["confirmed"], seen["unconfirmed"]), (1, 1, 0))
        self.assertEqual(seen["press_to_fixture_update_ms"], reflex.spread([10.5]))
        self.assertEqual(seen["receive_to_fixture_update_ms"], reflex.spread([10]))
        self.assertEqual(seen["basis"], "fixture_update_not_screen_capture")
        record["receipts"] = []
        self.assertEqual(reflex.effect_numbers(record), seen)

    def test_expiry_missing_frames_and_unknown_shown_are_not_verified_effects(self):
        for change in ("expired", "no_before", "no_after", "still_present", "unknown_shown", "before_receive"):
            with self.subTest(change=change):
                record, target = self.effect_record()
                if change == "expired":
                    record["frames"][1]["ns"] = reflex.at_ns(record, target["expireMs"])
                elif change == "no_before":
                    record["frames"] = record["frames"][1:]
                elif change == "no_after":
                    record["frames"] = record["frames"][:1]
                elif change == "still_present":
                    record["frames"][1]["shown"] = [target["id"]]
                elif change == "unknown_shown":
                    record["frames"][1].pop("shown")
                elif change == "before_receive":
                    record["frames"][1]["ns"] = record["events"][0]["rxNs"] - 1
                seen = reflex.effect_numbers(record)
                self.assertEqual((seen["confirmed"], seen["unconfirmed"]), (0, 1))
                self.assertEqual(seen["press_to_fixture_update_ms"], reflex.spread([]))

    def test_status_rows_are_loaded_and_metrics_survive_the_tally(self):
        record, _ = self.effect_record()
        self.statuses(record, [(100, 40), (200, 42)])
        with tempfile.TemporaryDirectory() as folder:
            path = pathlib.Path(folder)
            path.joinpath("status.jsonl").write_text("".join(json.dumps(row) + "\n" for row in record["statuses"]))
            self.assertEqual(reflex.load(path)["statuses"], record["statuses"])
            judged = reflex.judged(record, VALUES, LIMITS)
            path.joinpath(tally.REFLEX_RUN).write_text(json.dumps(judged))
            row = tally.measure(folder)
            for name in ("observation", "first_event_freshness", "effect"):
                self.assertEqual(row[name], judged["measure"][name])
                judged["measure"].pop(name)
            path.joinpath(tally.REFLEX_RUN).write_text(json.dumps(judged))
            legacy = tally.measure(folder)
            self.assertTrue(all(legacy[name] is None for name in ("observation", "first_event_freshness", "effect")))


class DeliveryTimings(unittest.TestCase):
    def test_event_posting_and_receiving_have_separate_latency(self):
        record = clean()
        measured = reflex.measure(record, VALUES, LIMITS)
        count = measured["hits"]
        self.assertEqual(measured["press_to_receive_ms"], reflex.spread([0.5] * count))
        self.assertEqual(measured["appear_to_receive_ms"], reflex.spread([150.5] * count))

        delayed = copy.deepcopy(record)
        for event in delayed["events"]:
            event["rxNs"] += 75 * MS
        slower = reflex.measure(delayed, VALUES, LIMITS)
        self.assertEqual(slower["appear_to_press_ms"], measured["appear_to_press_ms"])
        self.assertEqual(slower["press_to_receive_ms"], reflex.spread([75.5] * count))
        self.assertEqual(slower["appear_to_receive_ms"], reflex.spread([225.5] * count))
        self.assertEqual((slower["apm"], slower["verified_apm"]), (measured["apm"], measured["verified_apm"]))

    def test_absent_or_invalid_receive_times_are_missing_samples_not_zero_latency(self):
        for value in (None, 0, True, "bad", float("nan"), float("inf"), T0 - 1):
            with self.subTest(value=value):
                record = clean()
                for event in record["events"]:
                    if value is None:
                        event.pop("rxNs")
                    else:
                        event["rxNs"] = value
                measured = reflex.measure(record, VALUES, LIMITS)
                self.assertGreater(measured["appear_to_press_ms"]["n"], 0)
                self.assertEqual(measured["press_to_receive_ms"], reflex.spread([]))
                self.assertEqual(measured["appear_to_receive_ms"], reflex.spread([]))

    def test_actual_appearance_is_required_for_receive_reaction(self):
        record = clean()
        record["frames"] = []
        measured = reflex.measure(record, VALUES, LIMITS)
        self.assertGreater(measured["appear_to_press_ms"]["n"], 0)
        self.assertGreater(measured["press_to_receive_ms"]["n"], 0)
        self.assertEqual(measured["appear_to_receive_ms"], reflex.spread([]),
                         "the scheduled appearance cannot stand in for a rendered frame")

    def test_receive_timings_reach_the_json_and_rendered_summary(self):
        record = clean()
        judged = reflex.judged(record, VALUES, LIMITS)
        with tempfile.TemporaryDirectory() as run:
            pathlib.Path(run, tally.REFLEX_RUN).write_text(json.dumps(judged))
            row = tally.measure(run)
            summary = tally.summarize_reflex([row], VALUES)
            entry = next(iter(summary.values()))
            self.assertEqual(row["press_to_receive_ms"]["n"], row["hits"])
            self.assertEqual(entry["median_press_to_receive_p50"], 0.5)
            self.assertEqual(entry["median_appear_to_receive_p95"], 150.5)
            self.assertEqual(entry["press_to_receive_n"], row["hits"])
            self.assertEqual(entry["appear_to_receive_n"], row["hits"])
            rendered = tally.render_reflex(summary)
            self.assertEqual(len({line.count("|") for line in rendered.splitlines()}), 1)
            for text in ("appear→receive", "press→receive", "run medians", "150.5", "0.5"):
                self.assertIn(text, rendered)

            for name in ("press_to_receive_ms", "appear_to_receive_ms"):
                judged["measure"].pop(name)
            pathlib.Path(run, tally.REFLEX_RUN).write_text(json.dumps(judged))
            old = next(iter(tally.summarize_reflex([tally.measure(run)], VALUES).values()))
            self.assertIsNone(old["median_press_to_receive_p50"])
            self.assertIsNone(old["median_appear_to_receive_p95"])
            self.assertEqual((old["press_to_receive_n"], old["appear_to_receive_n"]), (0, 0))


class Autopilot(unittest.TestCase):
    """A goal in place of a plan (t-10223 R9): the autopilot's runs and plans
    judged from the files the driver and the bench home keep."""

    def test_an_autopilot_run_is_judged_on_every_run_it_started_and_every_plan_it_ran(self):
        record = piloted()
        verdict = reflex.verdict(record, VALUES, LIMITS)
        self.assertEqual(verdict["verdict"], "pass", failed(verdict))
        self.assertEqual(len(verdict["checks"]), 10)
        self.assertEqual(verdict["checks"][-1]["check"], "every plan is the model's")
        self.assertEqual(len(reflex.verdict(clean(), VALUES, LIMITS)["checks"]), 9, "a person's plan has nine")
        # A plan the bench's stand-in wrote is never counted as the model's.
        stub = reflex.verdict(piloted(sources=("model", "stub")), VALUES, LIMITS)
        self.assertEqual(failed(stub), ["every plan is the model's"])
        none = piloted()
        none["ended"]["autopilot"]["plans"] = []
        self.assertIn("every plan is the model's", failed(reflex.verdict(none, VALUES, LIMITS)))
        # Every run's trace is whole, not only the last one's.
        short = piloted()
        short["receipts"] = [receipt for receipt in short["receipts"] if (receipt["run"], receipt["seq"]) != ("rx-1", 1)]
        self.assertIn("the trace is whole", failed(reflex.verdict(short, VALUES, LIMITS)))
        # An autopilot that ended itself early is judged, never excused as aborted.
        paused = piloted()
        paused["ended"]["runs"][-1]["status"]["reason"] = "request"
        paused["ended"]["status"] = paused["ended"]["runs"][-1]["status"]
        paused["ended"]["autopilot"]["ended"] = {"reason": "paused", "said": "paused"}
        self.assertNotEqual(reflex.verdict(paused, VALUES, LIMITS)["verdict"], "aborted")
        # The runner's stop still aborts it.
        stopped = piloted()
        stopped["run"]["stoppedBy"] = "person"
        self.assertEqual(reflex.verdict(stopped, VALUES, LIMITS)["verdict"], "aborted")
        # A helper that ended the last run on a person's input aborts it too.
        touched = piloted()
        touched["ended"]["status"] = {**touched["ended"]["status"], "reason": "external_input"}
        touched["ended"]["runs"][-1]["status"] = touched["ended"]["status"]
        self.assertEqual(reflex.verdict(touched, VALUES, LIMITS)["verdict"], "aborted")

    def test_the_roads_are_the_autopilots_account_and_add_up_to_what_was_carried_out(self):
        measured = reflex.measure(piloted(), VALUES, LIMITS)
        autopilot = measured["autopilot"]
        self.assertEqual(measured["roads"]["jev"]["n"], 3)
        self.assertEqual(measured["roads"]["memo"]["n"], 0)
        self.assertEqual(measured["roads"]["escalated"]["n"], 0)
        self.assertEqual(measured["roads"]["l0"]["n"], sum(run["status"]["fires"] for run in piloted()["ended"]["runs"]))
        self.assertEqual(autopilot["applied"], {"continue": 2, "pause": 0, "replan": 1})
        self.assertEqual(autopilot["invalid"]["stale"], 1)
        self.assertEqual(autopilot["unanswered"], 1)
        self.assertEqual(autopilot["ended"], "deadline")
        self.assertEqual(autopilot["plans"], 2)
        self.assertEqual(autopilot["sources"], {"model": 2})
        self.assertTrue(autopilot["roads_add_up"])
        # A road count that is not what was carried out is said so, and fails its floor.
        off = piloted()
        off["ended"]["autopilot"]["roads"]["jev"] = 2
        measured = reflex.measure(off, VALUES, LIMITS)
        self.assertFalse(measured["autopilot"]["roads_add_up"])
        self.assertFalse(reflex.floors(measured, VALUES)["roads_add_up"])
        # An escalated end is counted as the runs that ended so.
        escalated = piloted()
        escalated["ended"]["autopilot"]["ended"] = {"reason": "escalated", "said": "-"}
        self.assertEqual(reflex.measure(escalated, VALUES, LIMITS)["roads"]["escalated"]["n"], 1)
        # A person's plan asks the decision nothing: every road but the hand's is none.
        hand = reflex.measure(clean(), VALUES, LIMITS)
        self.assertEqual({road: row["n"] for road, row in hand["roads"].items() if road != "l0"},
                         {"memo": 0, "surrogate": 0, "jev": 0, "escalated": 0})
        self.assertIsNone(hand["autopilot"])

    def test_the_new_columns_come_from_the_fixture_the_helper_and_the_ledgers(self):
        record = piloted()
        measured = reflex.measure(record, VALUES, LIMITS)
        first = min(event["evNs"] for event in record["events"] if event["kind"] == "down")
        self.assertEqual(measured["goal_to_first_press_ms"], (first - T0) / 1e6)
        self.assertEqual(measured["replan_gap_ms"], {"n": 1, "p50": 40.0, "p95": 40.0, "p99": 40.0})
        l1 = measured["l1"]
        self.assertEqual(l1["asked"], 5)
        self.assertEqual(l1["answered"], 4)
        self.assertEqual(l1["forced"], 5)
        self.assertEqual(l1["rtt_ms"]["p50"], 400.0)
        self.assertEqual(l1["rtt_ms"]["n"], 5)
        cost = measured["cost"]
        self.assertEqual(cost["tokens"], {"input": 4_000, "output": 1_600})
        self.assertEqual(cost["plan_requests"], 2)
        self.assertAlmostEqual(cost["usd"], 4_000 * 1.0 / 1e6 + 1_600 * 5.0 / 1e6)
        self.assertEqual(measured["plan_rtt_ms"]["n"], 2)
        # A model with no price is counted in tokens and never guessed in dollars.
        unpriced = piloted()
        unpriced["plans"][0]["model"] = "a-model-with-no-price"
        self.assertIsNone(reflex.measure(unpriced, VALUES, LIMITS)["cost"]["usd"])
        # A stand-in's plans cost nothing and say so.
        stub = piloted(sources=("stub", "stub"))
        for row in stub["plans"]:
            row.update(tokens=None, model=None)
        self.assertEqual(reflex.measure(stub, VALUES, LIMITS)["cost"], {"tokens": {"input": 0, "output": 0},
                                                                          "plan_requests": 2, "usd": 0.0})
        # A person's plan has a first press too, and no re-plan, question or plan cost.
        hand = reflex.measure(clean(), VALUES, LIMITS)
        self.assertIsNotNone(hand["goal_to_first_press_ms"])
        self.assertEqual(hand["replan_gap_ms"]["n"], 0)
        self.assertIsNone(hand["l1"])
        self.assertIsNone(hand["cost"])

    def test_the_autopilots_floors_are_the_designs(self):
        record = piloted()
        gate = reflex.floors(reflex.measure(record, VALUES, LIMITS), VALUES)
        for name in ("model_plans", "roads_add_up", "l1_forced"):
            self.assertTrue(gate[name], name)
        self.assertTrue(reflex.judged(record, VALUES, LIMITS)["success"])
        stub = reflex.judged(piloted(sources=("stub", "stub")), VALUES, LIMITS)
        self.assertFalse(stub["floors"]["model_plans"])
        self.assertFalse(stub["success"], "a stand-in's plans never make the gate")
        self.assertEqual(stub["config"], f"{reflex.CONFIG}+autopilot-window")
        unforced = piloted()
        unforced["decisions"][0]["provenance"]["forced"] = False
        self.assertFalse(reflex.floors(reflex.measure(unforced, VALUES, LIMITS), VALUES)["l1_forced"])
        # A person's plan is held to the floors it always was.
        self.assertNotIn("model_plans", reflex.floors(reflex.measure(clean(), VALUES, LIMITS), VALUES))

    def test_the_keys_go_to_the_driver_by_the_windows_names_and_only_there(self):
        goal = VALUES["reflex_goal"]
        harness = (pathlib.Path(reflex.HERE).parents[1] / "crates/zerocode-harness/src/lib.rs").read_text()
        self.assertIn(f'pub const SERVICE_KEYCHAIN_SERVICE_PREFIX: &str = "{goal["keys"]["prefix"]}";', harness)
        self.assertIn(f'pub const TYPESAFE_API_KEY_ENV: &str = "{goal["keys"]["jev"]}";', harness)
        read = []

        def keychain(service, value=False):
            read.append((service, value))
            return "k-" + service if service.endswith(goal["keys"]["jev"]) else None

        env, why = reflex.keys_for(VALUES, reflex.STUB, keychain)
        self.assertIsNone(why)
        self.assertEqual(sorted(env), [goal["keys"]["jev"]])
        self.assertEqual(env[goal["keys"]["jev"]], "k-" + goal["keys"]["prefix"] + goal["keys"]["jev"])
        # The window's login road does not require an API key.
        env, why = reflex.keys_for(VALUES, "window", keychain)
        self.assertEqual(sorted(env), [goal["keys"]["jev"]])
        self.assertIsNone(why)
        self.assertFalse(any(value for service, value in read if not service.endswith(goal["keys"]["jev"])),
                         "a key that is not there is looked for, never read")
        # No Jev key: the stand-in round runs, its questions refused at the door, and says so.
        env, why = reflex.keys_for(VALUES, reflex.STUB, lambda service, value=False: None)
        self.assertEqual(env, {})
        self.assertIsNone(why)


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

    def test_an_autopilot_run_is_tallied_with_the_autopilots_columns(self):
        with tempfile.TemporaryDirectory() as folder:
            for name, record in (("hand", clean()), ("pilot", piloted())):
                os.mkdir(os.path.join(folder, name))
                with open(os.path.join(folder, name, tally.REFLEX_RUN), "w", encoding="utf-8") as handle:
                    json.dump(reflex.judged(record, VALUES, LIMITS), handle)
            rows = {os.path.basename(row["folder"]): row for row in tally.collect(folder)}
        pilot = rows["pilot"]
        self.assertEqual(pilot["roads"]["jev"]["n"], 3)
        self.assertEqual(pilot["applied"], {"continue": 2, "pause": 0, "replan": 1})
        self.assertEqual(pilot["plans"], 2)
        self.assertTrue(pilot["roads_add_up"])
        self.assertEqual(pilot["replan_gap_ms"]["p50"], 40.0)
        self.assertEqual(pilot["l1_rtt_ms"]["p50"], 400.0)
        self.assertEqual(pilot["tokens"], {"input": 4_000, "output": 1_600})
        self.assertIsNotNone(pilot["goal_to_first_press_ms"])
        self.assertIsNone(rows["hand"]["l1_rtt_ms"])
        # The whole tally reads the same rows: no reflex column is mistaken for one of its own.
        tally.summarize(list(rows.values()), VALUES)
        self.assertEqual(pilot["not_carried_out"]["stale"], 1)
        summary = tally.summarize_reflex(list(rows.values()), VALUES)
        entry = next(entry for entry in summary.values() if entry["runs"] == 1 and entry["median_plans"])
        self.assertEqual(entry["median_l1_rtt_p50"], 400.0)
        self.assertAlmostEqual(entry["usd"], 4_000 / 1e6 + 1_600 * 5 / 1e6)
        rendered = tally.render_reflex(summary)
        for column in ("goal→press ms", "re-plan gap ms", "L1 RTT p50/p95 ms", "applied c/p/r", "tokens in/out", "$"):
            self.assertIn(column, rendered)


class DeskHooks(unittest.TestCase):
    def test_r9_seed_bytes_and_default_plan_and_judgment_survive_the_hooks(self):
        expected = {
            11: 'fd9c4b8b4ead56bc9753c9210b026351c6c4fb2ade4e92c9fa0fdee680175e5a',
            12: '0f9a460f4aa505141b621ddc931ded72f1a16cb5f078cfee2172bfeac20d1886',
            13: '2b5738b07d47ce61750936ad6db87b0310c0fc3a040f69ec4531e5960fa22eaa',
        }
        with tempfile.TemporaryDirectory() as folder:
            path = pathlib.Path(folder)
            reflex.write_atomic(path / 'session.json', {'owner': 'owner', 'bundle': 'dev.zerocode.bench.reflex'})
            desk = reflex.Desk(path, VALUES, LIMITS)
            for seed, digest in expected.items():
                wire = json.dumps(desk.round(seed), sort_keys=True, separators=(',', ':')).encode()
                self.assertEqual(hashlib.sha256(wire).hexdigest(), digest)
                self.assertEqual(desk.result(clean(seed)), reflex.judged(clean(seed), VALUES, LIMITS))
            self.assertEqual(desk.plan(GEOMETRY, None),
                             reflex.plan(GEOMETRY, VALUES, desk.session['bundle'], reflex.contract(), table_limits=LIMITS))


if __name__ == "__main__":
    unittest.main()


def of_kind(kind, seed=7, press_after_ms=60):
    """A record of a round of `kind` that did its job, in the shapes the fixture
    writes for it: every target pressed `press_after_ms` after it was armed (a
    timing round shows it as a preview before that), every sweeper cleared by a
    move to its pad, every panel's card pressed in time."""
    record = clean(seed)
    schedule = reflex.schedule(seed, VALUES, kind=kind)
    record["schedule"] = schedule
    accepted, deadline = run_window(record)
    events, frames = [], []
    for target in schedule["targets"]:
        appear, expire = T0 + target["appearMs"] * MS, T0 + target["expireMs"] * MS
        armed = T0 + target.get("armMs", target["appearMs"]) * MS
        if appear < accepted or expire > deadline:
            continue
        if armed > appear:
            frames.append({"seq": len(frames) + 1, "ns": appear, "phase": target["phase"], "shown": [f"{target['id']}:preview"]})
        frames.append({"seq": len(frames) + 1, "ns": armed, "phase": target["phase"], "shown": [target["id"]]})
        at = armed + press_after_ms * MS
        events.append({"n": 0, "kind": "down", "evNs": at, "rxNs": at + MS // 2, "x": target["x"], "y": target["y"],
                       "sourcePid": HELPER, "userData": 99, "button": 0, "judged": {"hit": target["id"]}})
        events.append({"n": 0, "kind": "up", "evNs": at + 8 * MS, "rxNs": at + 8 * MS + MS // 2, "x": target["x"],
                       "y": target["y"], "sourcePid": HELPER, "userData": 99, "button": 0})
    scenes = []
    for sweep in schedule.get("sweeps") or []:
        launched = T0 + sweep["appearMs"] * MS
        arrived = launched + sweep["flightMs"] * MS
        if launched < accepted or arrived > deadline:
            continue
        moved = launched + 90 * MS
        events.append({"n": 0, "kind": "move", "evNs": moved, "rxNs": moved + MS // 2, "x": sweep["padX"], "y": sweep["padY"],
                       "sourcePid": HELPER, "userData": 99})
        scenes.append({"kind": "sweep", "id": sweep["id"], "launchedNs": launched, "arrivedNs": arrived, "outcome": "clear"})
    events.sort(key=lambda event: event["evNs"])
    for number, event in enumerate(events, start=1):
        event["n"] = number
    hits = sum(1 for event in events if "hit" in (event.get("judged") or {}))
    record.update(events=events, frames=frames, scenes=scenes, receipts=[])
    record["fixture"].update(downs=hits, ups=hits, hits=hits, misses={}, frames=len(frames))
    record["run"]["kind"] = kind
    return record


class Kinds(unittest.TestCase):
    """The round's kinds (t-26708; the coordinator's 19:14 framing): `plain`
    is today's round; `timing` shows each target as a preview and arms it
    after a while, so a press is measured against the moment it became
    right; `avoid` sends a sweeper toward the resting pointer with a pad to
    move to; `panel` shows each card among decoys for a limited time. Every
    number is the seed's and the table's; nothing here moves the pointer."""

    def test_plain_is_todays_round_and_a_kind_is_the_seeds_alone(self):
        self.assertEqual(reflex.schedule(7, VALUES, kind="plain"), reflex.schedule(7, VALUES))
        self.assertEqual(reflex.schedule(7, VALUES, kind="timing"), reflex.schedule(7, VALUES, kind="timing"))
        self.assertNotEqual(reflex.schedule(7, VALUES, kind="timing"), reflex.schedule(7, VALUES))
        self.assertNotEqual(reflex.schedule(7, VALUES, kind="avoid"), reflex.schedule(8, VALUES, kind="avoid"))

    def test_a_timing_round_arms_each_target_after_its_preview_and_lives_from_the_arm(self):
        drawn = reflex.schedule(7, VALUES, kind="timing")
        table, life = VALUES["reflex_kind"]["timing"], VALUES["reflex_target"]["life_ms"]
        self.assertGreater(len(drawn["targets"]), 100)
        for target in drawn["targets"]:
            self.assertIn("armMs", target)
            self.assertTrue(table["preview_ms"][0] <= target["armMs"] - target["appearMs"] <= table["preview_ms"][1], target)
            self.assertTrue(life[0] <= target["expireMs"] - target["armMs"] <= life[1], target)
            phase = drawn["phases"][target["phase"]]
            self.assertTrue(phase["startMs"] <= target["appearMs"] and target["expireMs"] <= phase["endMs"], target)

    def test_an_avoid_round_launches_sweepers_one_at_a_time_with_a_pad_on_the_far_side(self):
        drawn = reflex.schedule(7, VALUES, kind="avoid")
        table, box = VALUES["reflex_kind"]["avoid"], reflex.field(VALUES)
        sweeps = drawn.get("sweeps") or []
        self.assertGreaterEqual(len(sweeps), 10)
        middle = box["x"] + box["width"] / 2
        for sweep in sweeps:
            self.assertTrue(table["flight_ms"][0] <= sweep["flightMs"] <= table["flight_ms"][1], sweep)
            self.assertEqual((sweep["width"], sweep["height"], sweep["padRadius"]),
                             (table["width_pt"], table["height_pt"], table["pad_radius_pt"]))
            from_left = sweep["fromX"] <= box["x"]
            self.assertTrue(from_left or sweep["fromX"] >= box["x"] + box["width"], "launched from outside the field")
            self.assertTrue(sweep["padX"] > middle if from_left else sweep["padX"] < middle, "the pad is on the far side")
            self.assertTrue(box["x"] + sweep["padRadius"] <= sweep["padX"] <= box["x"] + box["width"] - sweep["padRadius"])
            self.assertTrue(box["y"] + sweep["padRadius"] <= sweep["padY"] <= box["y"] + box["height"] - sweep["padRadius"])
        for before, after in zip(sweeps, sweeps[1:]):
            self.assertLessEqual(before["appearMs"] + before["flightMs"] + table["every_ms"][0], after["appearMs"])

    def test_a_panel_round_shows_each_card_among_the_tables_decoys_for_its_limit(self):
        drawn = reflex.schedule(7, VALUES, kind="panel")
        table = VALUES["reflex_kind"]["panel"]
        beside = collections.Counter(decoy.get("beside") for decoy in drawn["decoys"])
        self.assertGreater(len(drawn["targets"]), 30)
        for target in drawn["targets"]:
            self.assertEqual(beside[target["id"]], table["cards"] - 1, target["id"])
            self.assertTrue(table["life_ms"][0] <= target["expireMs"] - target["appearMs"] <= table["life_ms"][1], target)
        for decoy in drawn["decoys"]:
            target = next(row for row in drawn["targets"] if row["id"] == decoy["beside"])
            self.assertEqual((decoy["appearMs"], decoy["expireMs"]), (target["appearMs"], target["expireMs"]),
                             "a card shows with its panel")

    def test_the_round_file_names_its_kind_and_carries_the_kinds_colours(self):
        drawn = reflex.the_round(OWNER, 7, VALUES, LIMITS, kind="avoid")
        self.assertEqual(drawn.get("kind"), "avoid")
        for name in ("preview", "sweeper", "pad"):
            self.assertIn(name, drawn["palette"])
        plain = reflex.the_round(OWNER, 7, VALUES, LIMITS)
        self.assertEqual(plain, reflex.the_round(OWNER, 7, VALUES, LIMITS, kind="plain"))
        self.assertNotIn("sweeper", plain["palette"], "today's round bytes are untouched")
        source = reflex.SOURCE.read_text()
        for word in ("armMs", "sweeps", "padX", "preview"):
            self.assertIn(word, source, f"the fixture reads {word}")

    def test_a_kinds_plan_reads_the_kinds_colours_and_an_avoid_plan_moves_to_the_pad(self):
        bundle = f"dev.zerocode.bench.reflex.{OWNER}"
        avoid = reflex.plan(GEOMETRY, VALUES, bundle, reflex.contract(), kind="avoid")
        by_id = {detector["id"]: detector for detector in avoid["detectors"]}
        self.assertIn("pad", by_id)
        added = VALUES["reflex_kind_palette"]
        for detector in by_id.values():
            classes = [(c["r"], c["g"], c["b"]) for c in detector["color"]["classes"]]
            for name in ("preview", "sweeper", "pad"):
                self.assertIn((added[name]["r"], added[name]["g"], added[name]["b"]), classes, detector["id"])
        self.assertNotIn("anchors", by_id["pad"]["color"], "the pad reads under either strip colour; the wire carries no empty list")
        rule = next(row for row in avoid["rules"] if row["detector"] == "pad")
        self.assertGreater(rule["priority"], max(row["priority"] for row in avoid["rules"] if row["detector"] != "pad"))
        macro = next(row for row in avoid["macros"] if row["id"] == rule["macro_id"])
        self.assertEqual([(action["kind"], action["target"]) for action in macro["actions"]], [("move", "pad")])
        macros = {row["id"]: row for row in avoid["macros"]}
        self.assertLessEqual(sum(row["max_fires"] * len(macros[row["macro_id"]]["actions"]) for row in avoid["rules"]),
                             LIMITS["max_expanded_actions"])
        timing = reflex.plan(GEOMETRY, VALUES, bundle, reflex.contract(), kind="timing")
        self.assertEqual({detector["id"] for detector in timing["detectors"]}, set(reflex.COLOURS), "timing adds no detector")
        self.assertEqual(reflex.plan(GEOMETRY, VALUES, bundle, reflex.contract(), kind="plain"),
                         reflex.plan(GEOMETRY, VALUES, bundle, reflex.contract()))

    def test_a_timing_rounds_numbers_read_the_press_against_the_arm_and_an_early_press_is_wrong(self):
        record = of_kind("timing")
        measured = reflex.measure(record, VALUES, LIMITS)
        self.assertIn("kind", measured)
        timing = measured["kind"]["timing"]
        self.assertEqual(timing["arm_to_press_ms"]["p50"], 60)
        self.assertEqual(timing["arm_to_press_ms"]["n"], measured["hits"])
        self.assertEqual(timing["early"], 0)
        self.assertEqual(reflex.verdict(record, VALUES, LIMITS)["verdict"], "pass", failed(reflex.verdict(record, VALUES, LIMITS)))
        early = of_kind("timing")
        first = next(event for event in early["events"] if event["kind"] == "down")
        early["events"].append({**first, "n": 0, "evNs": first["evNs"] - 100 * MS, "rxNs": first["rxNs"] - 100 * MS,
                                "judged": {"miss": "early", "near": first["judged"]["hit"]}})
        early["events"].append({**first, "n": 0, "kind": "up", "evNs": first["evNs"] - 92 * MS, "rxNs": first["rxNs"] - 92 * MS,
                                "judged": None})
        early["fixture"]["downs"] += 1
        early["fixture"]["ups"] += 1
        measured = reflex.measure(early, VALUES, LIMITS)
        self.assertEqual(measured["kind"]["timing"]["early"], 1)
        self.assertEqual(measured["wrong_by_kind"].get("early"), 1)
        self.assertIn("no wrong input", failed(reflex.verdict(early, VALUES, LIMITS)))

    def test_an_avoid_rounds_numbers_count_the_sweepers_cleared_and_the_move_that_cleared_them(self):
        record = of_kind("avoid")
        measured = reflex.measure(record, VALUES, LIMITS)
        self.assertIn("kind", measured)
        avoid = measured["kind"]["avoid"]
        self.assertGreaterEqual(avoid["launched"], 10)
        self.assertEqual((avoid["clear"], avoid["struck"]), (avoid["launched"], 0))
        self.assertEqual(avoid["clear_share"], 1.0)
        self.assertEqual(avoid["reaction_ms"]["p50"], 90, "the first move after the launch")
        self.assertEqual(avoid["reaction_ms"]["n"], avoid["launched"])
        struck = of_kind("avoid")
        struck["scenes"][0]["outcome"] = "struck"
        measured = reflex.measure(struck, VALUES, LIMITS)
        self.assertEqual(measured["kind"]["avoid"]["struck"], 1)
        self.assertLess(measured["kind"]["avoid"]["clear_share"], 1.0)
        self.assertFalse(reflex.floors(measured, VALUES)["kind"]["avoid"], "one struck sweeper of this many fails the floor")
        self.assertTrue(reflex.floors(reflex.measure(record, VALUES, LIMITS), VALUES)["kind"]["avoid"])

    def test_a_panel_rounds_numbers_are_the_in_time_share_against_the_random_pick(self):
        record = of_kind("panel")
        measured = reflex.measure(record, VALUES, LIMITS)
        self.assertIn("kind", measured)
        panel = measured["kind"]["panel"]
        self.assertEqual(panel["panels"], measured["targets_due"]["run"])
        self.assertEqual(panel["in_time"], panel["panels"])
        self.assertEqual(panel["in_time_share"], 1.0)
        self.assertEqual(panel["baseline_share"], 1 / VALUES["reflex_kind"]["panel"]["cards"])
        gate = reflex.floors(measured, VALUES)
        self.assertTrue(gate["kind"]["panel"])
        late = of_kind("panel", press_after_ms=VALUES["reflex_kind"]["panel"]["life_ms"][1] + 50)
        self.assertLess(reflex.measure(late, VALUES, LIMITS)["kind"]["panel"]["in_time_share"], 1.0)

    def test_a_plain_round_measures_no_kind_and_its_floors_are_untouched(self):
        measured = reflex.measure(clean(), VALUES, LIMITS)
        self.assertIsNone(measured.get("kind"))
        self.assertNotIn("kind", reflex.floors(measured, VALUES))
