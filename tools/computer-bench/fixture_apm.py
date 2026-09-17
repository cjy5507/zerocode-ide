#!/usr/bin/env python3
"""Owned native fixture APM measurement through the public Computer Use CLI.

prepare DIR [--mode alternate|random] compiles a small app, without launching.
open DIR checks idle/stop/hotkey, launches only that app, and takes a look.
begin DIR --actions 12 starts a bounded externally controlled measurement.
act DIR --labels Amber executes one explicitly chosen button, then looks.
finish DIR checks the complete plan and final stop state before accepting APM.
hands DIR --actions 180 replays the predictable fixture, refreshing each index.
hands DIR --actions 10 --input-path coordinate --batch-size 5 measures batches.
eyes DIR --rounds 10 compares existing latest-frame and settled desktop looks.
close DIR quits only its verified PID, unless stopped (then leaves it alone).

Announce desk acquisition/release in the ledger around open..close. Random
mode is for this agent's own LLM loop: read each look and choose one label in
the next tool call. No subprocess model is launched. The fixture's event
oracle is checked separately from the returned AX tree. No-op CLI success
does not pass. Waits, setup/quit, failed calls and batch parents earn no APM.
"""
import argparse
from functools import wraps
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import signal
import statistics
import subprocess
import sys
import time
import uuid

from bench import Bench, Signals, Stopped

HERE = Path(__file__).resolve().parent
# The macOS system Python's monotonic epoch can be process-local. Each CLI
# invocation must compare persisted timestamps in the same system clock.
CLOCK_ID = "CLOCK_UPTIME_RAW" if hasattr(time, "CLOCK_UPTIME_RAW") else "monotonic-system"


def uptime_ns():
    if hasattr(time, "CLOCK_UPTIME_RAW"):
        return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    return time.monotonic_ns()


def recorded(operation):
    """A failed/interrupted operation leaves a durable outcome, even via imports."""
    @wraps(operation)
    def run(self, *args, **kwargs):
        try:
            return operation(self, *args, **kwargs)
        except (Stopped, RuntimeError, ValueError, OSError, subprocess.TimeoutExpired) as error:
            if isinstance(error, ValueError) and not self.session.get("measurement") and not self.session.get("pending_action"):
                raise  # Bad arguments before measurement are not a desktop outcome.
            if "outcome" not in self.session:
                ended = uptime_ns()
                status = "interrupted" if isinstance(error, Stopped) else "failed"
                self.session["outcome"] = {"status": status, "reason": str(error), "at_ns": ended}
                measurement = self.session.get("measurement", {})
                if measurement.get("status") == "running":
                    measurement.update(status=status, finished_ns=ended)
                try:
                    self.save()
                except OSError as save_error:
                    print(f"could not persist fixture outcome: {save_error}", file=sys.stderr)
            raise
    return run


def write(path, value):
    temp = path.with_suffix(".new")
    temp.write_text(json.dumps(value, indent=2) + "\n")
    temp.replace(path)


def oracle_delta(before, after, labels):
    """An ordered, fresh button event for every claimed successful action."""
    events = after["events"][len(before["events"]):]
    return (after["owner"] == before["owner"] and after["pid"] == before["pid"]
            and after["count"] - before["count"] == len(labels)
            and after["errors"] == before["errors"]
            and len(events) == len(labels)
            and all(e["correct"] and e["label"] == label for e, label in zip(events, labels)))


def prepare(folder, mode):
    folder.mkdir(mode=0o700, parents=False, exist_ok=False)
    owner = uuid.uuid4().hex[:12]
    app = folder / f"APMFixture-{owner}.app"
    executable = app / "Contents/MacOS/APMFixture"
    executable.parent.mkdir(parents=True)
    with (app / "Contents/Info.plist").open("wb") as handle:
        plistlib.dump({"CFBundleIdentifier": f"dev.zerocode.bench.fixture.{owner}",
                      "CFBundleExecutable": "APMFixture", "CFBundleName": f"APMFixture-{owner}",
                      "CFBundlePackageType": "APPL", "NSHighResolutionCapable": True}, handle)
    result = subprocess.run(["swiftc", str(HERE / "APMFixture.swift"), "-o", str(executable)])
    print(json.dumps({"swiftc_exit": result.returncode}))
    if result.returncode:
        return result.returncode
    write(folder / "session.json", {"owner": owner, "app": str(app), "mode": mode,
                                    "executable": str(executable), "rounds": [], "clock": CLOCK_ID})
    return 0


class Desk(Bench):
    # Reuse the existing runner's stop/hotkey/question checks, without its
    # automation-only multi-app setup or clipboard access.
    def __init__(self, folder):
        self.folder = folder
        self.session = json.loads((folder / "session.json").read_text())
        self.last_ms = 0

    def save(self):
        write(self.folder / "session.json", self.session)

    def guard(self):
        if self.session.get("outcome"):
            raise Stopped(f"fixture session ended: {self.session['outcome']['reason']}")
        if self.session.get("pending_action"):
            raise Stopped("previous fixture action has an unknown outcome; do not replay it")
        if self.session.get("clock") != CLOCK_ID:
            raise Stopped("fixture clock is missing or incompatible; retained session cannot be resumed")
        super().guard()
        began = self.session.get("watch_hid_since_ns")
        if began is not None:
            # Semantic fixture presses do not post HID events. If another
            # input path resets HID idle, conservatively stop that run too.
            # Never lift an input interruption automatically.
            sampled_from = uptime_ns()
            idle_s = self.hid_idle_s()
            sampled_to = uptime_ns()
            last_input = sampled_from - idle_s * 1e9
            if last_input > began + 100_000_000:
                self.session["hid_interruption"] = {"watch_started_ns": began,
                    "sampled_from_ns": sampled_from, "sampled_to_ns": sampled_to,
                    "idle_s": idle_s, "source": "unknown"}
                raise Stopped("HID idle reset during the fixture run; input source unknown")

    def shim(self, argv, folder=None, timeout=45):
        started = uptime_ns()
        child = subprocess.Popen(["zerocode-computer", *argv, "--json"],
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 text=True, start_new_session=True)
        try:
            stdout, stderr = child.communicate(timeout=timeout)
        except BaseException:
            # Stop queued CLI work when this runner is interrupted. The
            # product additionally checks whether the batch caller remains.
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
            raise
        self.last_ms = (uptime_ns() - started) / 1e6
        line = next((line for line in (stdout + stderr).splitlines() if line.startswith("{")), "{}")
        answer = json.loads(line)
        with (self.folder / "calls.jsonl").open("a") as handle:
            handle.write(json.dumps({"argv": argv, "exit": child.returncode,
                                     "wall_ms": self.last_ms, "answer": answer}) + "\n")
        if child.returncode != 0:
            answer["ok"] = False
        return answer

    def checked(self, argv):
        answer = self.shim(argv)
        self.refused_by_the_desk(answer)
        if answer.get("ok") is not True:
            raise RuntimeError(f"{argv[0]} refused: {answer}")
        return answer["result"]

    def snapshot(self):
        state = json.loads((self.folder / "oracle.json").read_text())
        if state["owner"] != self.session["owner"]:
            raise RuntimeError("fixture owner changed")
        pid = state["pid"]
        comm = subprocess.run(["ps", "-p", str(pid), "-o", "comm="],
                              capture_output=True, text=True)
        if comm.returncode or comm.stdout.strip() != self.session["executable"]:
            raise RuntimeError("fixture PID no longer belongs to its executable")
        return state

    def look(self):
        state = self.snapshot()
        result = self.checked(["get-app-state", "--app", f"pid:{state['pid']}", "--no-screenshot"])
        # Persist only this fixture's AX tree. The model uses it to choose.
        self.session["last_look"] = result
        self.session.setdefault("loop_started_ns", uptime_ns())
        self.save()
        return result

    @recorded
    def open(self):
        if (self.folder / "oracle.json").exists():
            raise RuntimeError("fixture already launched; use its current session")
        self.guard()
        idle = self.hid_idle_s()
        if self.screen_locked() or idle < 3:
            raise Stopped(f"operator active or locked (HID idle {idle:.2f}s)")
        args = [str(self.folder / "oracle.json"), self.session["owner"], self.session["mode"]]
        self.checked(["launch", "--app", self.session["app"], "--args", shlex.join(args), "--wait-ready", "3000"])
        for _ in range(30):
            if (self.folder / "oracle.json").exists():
                break
            time.sleep(0.1)
        initial = self.snapshot()
        if initial["events"] or initial["count"] or initial["errors"]:
            raise RuntimeError("fixture did not start empty")
        self.session["watch_hid_since_ns"] = uptime_ns()
        return self.look()

    @recorded
    def act(self, labels, input_path="semantic", reuse_state=False):
        entered = uptime_ns()
        if not labels or len(labels) > 20 or any(label not in ("Amber", "Blue") for label in labels):
            raise ValueError("1..20 explicit Amber/Blue labels required")
        if self.session["mode"] == "random" and len(labels) != 1:
            raise ValueError("random mode requires a model observation between actions")
        if input_path == "semantic" and len(labels) != 1:
            raise ValueError("semantic actions require a fresh index for each step")
        # "text": the control by what it reads (click --text, the matcher run on
        # a fresh tree at the press) — no look between steps, so a batch may
        # carry many; the oracle still reads the fixture's own event file.
        self.guard()
        measurement = self.session.get("measurement")
        if measurement:
            if measurement["status"] != "running":
                raise ValueError("measurement already ended")
            if self.summary()["attempted_actions"] + len(labels) > measurement["expected_actions"]:
                raise ValueError("action would exceed the measurement plan")
        before = self.snapshot()
        reused_before = False
        reused_after = False
        # App-specific coordinates stay inside our fixture and avoid moving
        # the person's pointer. Find fresh frame geometry before each batch;
        # indexes come only from a validated current or post-action state.
        if input_path == "text":
            commands = [["click", "--app", f"pid:{before['pid']}", "--role", "button", "--text", label,
                         "--no-screenshot"] for label in labels]
        elif reuse_state and input_path == "semantic":
            observed = fixture_observation(self.session.get("last_look"), before["pid"], self.session["owner"])
            if observed["count"] != before["count"] or observed["errors"] != before["errors"]:
                observed = fixture_observation(self.look(), before["pid"], self.session["owner"])
            else:
                reused_before = True
            commands = [["perform-secondary-action", "--app", f"pid:{before['pid']}",
                         "--element-index", str(observed["buttons"][labels[0]]), "--action", "AXPress",
                         "--no-screenshot"]]
        else:
            found = self.checked(["get-app-state", "--app", f"pid:{before['pid']}", "--no-screenshot"])
            matches = self.checked(["find", "--app", f"pid:{before['pid']}", "--role", "button"])
            buttons = button_points(matches, found["snapshot"]["window"])
            commands = [["click", "--app", f"pid:{before['pid']}", "--x", str(buttons[label][0]),
                         "--y", str(buttons[label][1])] for label in labels]
            if input_path == "semantic":
                match = next(match for match in matches["matches"] if match.get("label") == labels[0])
                if "AXPress" not in match.get("actions", []):
                    raise RuntimeError("owned button does not advertise AXPress")
                commands = [["perform-secondary-action", "--app", f"pid:{before['pid']}",
                             "--element-index", str(match["index"]), "--action", "AXPress", "--no-screenshot"]]
        self.guard()
        started = uptime_ns()
        self.session["pending_action"] = {"labels": labels, "input_path": input_path,
                                          "started_ns": started}
        self.save()
        answer = self.shim(commands[0] if len(commands) == 1 else ["batch", "--commands", json.dumps(commands)])
        input_ms = self.last_ms
        self.refused_by_the_desk(answer)
        after = self.snapshot()
        look = None
        if reuse_state and answer.get("ok") is True:
            candidate = answer.get("result")
            try:
                observed = fixture_observation(candidate, after["pid"], self.session["owner"])
                if observed["count"] == after["count"] and observed["errors"] == after["errors"]:
                    look = candidate
                    reused_after = True
                    self.session["last_look"] = look
            except ValueError:
                pass
        if look is None:
            look = self.look()
        visible = fixture_observation(look, after["pid"], self.session["owner"])
        passed = (answer.get("ok") is True and oracle_delta(before, after, labels)
                  and visible["count"] == after["count"] and visible["errors"] == after["errors"])
        ended = uptime_ns()
        previous = self.session.get("loop_ended_ns", self.session.get("loop_started_ns", entered))
        round_ = {"labels": labels, "input_path": input_path, "reuse_state": reuse_state, "reused_before": reused_before, "reused_after": reused_after, "passed": passed, "successful_actions": len(labels) if passed else 0,
                  "input_ms": input_ms, "wall_ms": (ended - started) / 1e6,
                  "between_calls_ms": max(0, entered - previous) / 1e6,
                  "prepare_ms": (started - entered) / 1e6,
                  "verify_ms": max(0, (ended - started) / 1e6 - input_ms),
                  "count_before": before["count"], "count_after": after["count"],
                  "errors_after": after["errors"]}
        self.session["rounds"].append(round_)
        self.session.pop("pending_action", None)
        self.session["loop_ended_ns"] = ended
        self.save()
        if not passed:
            raise RuntimeError(f"action oracle failed: {round_}")
        return {"round": round_, "look": look, "summary": self.summary()}

    def summary(self):
        measurement = self.session.get("measurement", {})
        rows = self.session["rounds"][measurement.get("start_round", self.session.get("measurement_start_round", 0)):]
        actions = sum(row["successful_actions"] for row in rows)
        input_ms = sum(row["input_ms"] for row in rows)
        started = measurement.get("started_ns", self.session.get("loop_started_ns", 0))
        ended = measurement.get("finished_ns", self.session.get("loop_ended_ns", started))
        timing_trusted = self.session.get("clock") == CLOCK_ID and ended >= started
        wall_ms = (ended - started) / 1e6 if timing_trusted else None
        pending = self.session.get("pending_action", {})
        complete = measurement.get("status") == "completed"
        loop_apm = 60000 * actions / wall_ms if wall_ms and wall_ms > 0 else None
        timings = {key: sum(row.get(key, 0) for row in rows)
                   for key in ("between_calls_ms", "prepare_ms", "input_ms", "verify_ms")}
        # Final guard, persistence and any unanswered attempt still cost time.
        timings["unattributed_ms"] = max(0, wall_ms - sum(timings.values())) if wall_ms is not None else None
        return {"mode": self.session["mode"], "successful_actions": actions,
                "attempted_actions": sum(len(row["labels"]) for row in rows) + len(pending.get("labels", [])),
                "unverified_actions": len(pending.get("labels", [])),
                "measurement_status": measurement.get("status", "not_started"),
                "measurement_complete": complete, "accepted_apm": loop_apm if complete else None,
                "expected_actions": measurement.get("expected_actions"),
                "controller": measurement.get("controller", "unspecified"),
                "clock": self.session.get("clock"), "timing_trusted": timing_trusted,
                "llm_apm": None,  # An external caller's identity is not proof of model decisions.
                "input_ms": input_ms, "input_apm": 60000 * actions / input_ms if input_ms else None,
                "loop_wall_ms": wall_ms, "loop_apm": loop_apm,
                "timing_ms": timings,
                "all_correct": bool(rows) and not pending and all(row["passed"] for row in rows)}

    @recorded
    def begin(self, actions, controller="external"):
        if not 1 <= actions <= 300 or controller not in ("external", "observed-rule"):
            raise ValueError("measurement needs 1..300 actions and a known controller")
        self.guard()
        if "measurement" in self.session:
            raise ValueError("one measurement per fixture session; never reset a run")
        self.look()
        started = uptime_ns()
        self.session["measurement"] = {"status": "running", "expected_actions": actions,
            "controller": controller, "start_round": len(self.session["rounds"]), "started_ns": started}
        self.session["loop_started_ns"] = started
        self.session["loop_ended_ns"] = started
        self.save()
        return {"look": self.session["last_look"], "summary": self.summary()}

    @recorded
    def finish(self):
        self.guard()  # The last action can also be followed by a stop/HID reset.
        measurement = self.session.get("measurement", {})
        if measurement.get("status") != "running":
            raise ValueError("no running measurement to finish")
        report = self.summary()
        if not report["all_correct"] or report["successful_actions"] != measurement["expected_actions"]:
            raise RuntimeError("measurement did not complete every planned action correctly")
        measurement.update(status="completed", finished_ns=uptime_ns())
        self.save()
        return self.summary()

    @recorded
    def hands(self, actions, batch_size, input_path, reuse_state=False):
        if self.session["mode"] != "alternate" or not 1 <= actions <= 300 or not 1 <= batch_size <= 20:
            raise ValueError("hands: alternate mode, 1..300 actions, batch size 1..20 required")
        # Wall includes each status, fresh look, oracle read and post-action
        # look. It excludes launch and pre-run human/model preparation.
        if input_path == "semantic" and batch_size != 1:
            raise ValueError("semantic actions require batch size 1")
        self.begin(actions, controller="observed-rule")
        completed = 0
        while completed < actions:
            # Choose only from the observed UI. The oracle file verifies the
            # result; its private next-target field is never a controller input.
            next_ = fixture_observation(self.session["last_look"], self.snapshot()["pid"], self.session["owner"])["next"]
            labels = [next_ if n % 2 == 0 else ("Blue" if next_ == "Amber" else "Amber")
                      for n in range(min(batch_size, actions - completed))]
            self.act(labels, input_path, reuse_state=reuse_state)
            completed += len(labels)
        return self.finish()

    @recorded
    def close(self):
        self.guard()  # A stop also forbids cleanup.
        state = self.snapshot()
        result = self.checked(["quit", "--app", f"pid:{state['pid']}"])
        if result.get("terminated") is not True:
            raise RuntimeError(f"owned fixture did not terminate: {result}")
        self.session["closed"] = result
        self.save()
        return {"quit": result, "summary": self.summary()}

    @recorded
    def eyes(self, rounds):
        if self.session["mode"] != "alternate" or not 1 <= rounds <= 20:
            raise ValueError("eyes requires alternate mode and 1..20 rounds")
        self.guard()
        if self.hid_idle_s() < 3:
            raise Stopped("operator HID input is recent")
        viewer = "fixture-" + self.session["owner"]
        # Warm the existing ScreenEye through the public CLI. Do not create
        # another streaming/capture loop. This is a latency comparison; the
        # app event/AX oracle proves the action, not freshness of these pixels.
        self.checked(["observe", "--viewer", viewer])
        samples = {"latest": [], "settle": []}
        for n in range(rounds):
            for style in (["latest", "settle"] if n % 2 == 0 else ["settle", "latest"]):
                self.act([self.snapshot()["next"]])
                argv = ["observe", "--viewer", viewer]
                if style == "settle":
                    argv += ["--settle"]
                answer = self.checked(argv)
                samples[style].append({"wall_ms": self.last_ms,
                                       "settle": answer.get("settle"),
                                       "image": answer.get("image") or answer.get("screenshot")})
                self.guard()
        report = {"rounds": rounds, "successful_actions": rounds * 2,
                  "all_correct": True, "hid_interrupted": False,
                  "pixel_freshness_verified": False,
                  "samples": samples,
                  "p50_ms": {style: statistics.median(row["wall_ms"] for row in rows)
                             for style, rows in samples.items()}}
        write(self.folder / "eyes-summary.json", report)
        return report


def fixture_observation(result, pid, owner):
    """Read visible fixture state and current AX indexes, never the oracle."""
    snapshot = result.get("snapshot", {}) if isinstance(result, dict) else {}
    app = snapshot.get("app", {})
    if app.get("pid") != pid or app.get("bundleId") != f"dev.zerocode.bench.fixture.{owner}":
        raise ValueError("response does not belong to the owned fixture")
    tree = snapshot.get("treeText", "")
    state = re.findall(r"^\s*\d+ text Next: (Amber|Blue) \| Count: (\d+) \| Errors: (\d+)$", tree, re.M)
    buttons = re.findall(r"^\s*(\d+) button (Amber|Blue)$", tree, re.M)
    if len(state) != 1 or len(buttons) != 2 or len({label for _, label in buttons}) != 2:
        raise ValueError("fixture state or advertised buttons are ambiguous")
    next_, count, errors = state[0]
    return {"next": next_, "count": int(count), "errors": int(errors),
            "buttons": {label: int(index) for index, label in buttons}}


def button_points(result, window):
    """Window-local coordinates from a fresh fixture observation."""
    if result.get("coordinateSpace") != "screen" or result.get("window", {}).get("id") != window["id"]:
        raise RuntimeError("button find and window observation disagree")
    points = {}
    for match in result.get("matches", []):
        label, frame = match.get("label"), match.get("frame")
        if label in ("Amber", "Blue") and isinstance(frame, dict):
            if label in points:
                raise RuntimeError("ambiguous fixture button")
            x = frame["x"] + frame["width"] / 2 - window["x"]
            y = frame["y"] + frame["height"] / 2 - window["y"]
            if not (0 < x < window["width"] and 0 < y < window["height"]):
                raise RuntimeError("button is outside owned window")
            points[label] = (x, y)
    if len(points) != 2:
        raise RuntimeError(f"fixture buttons did not expose fresh geometry: {result}")
    return points


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare", "open", "look", "begin", "act", "finish", "hands", "eyes", "close"])
    parser.add_argument("folder", type=Path)
    parser.add_argument("--mode", choices=["alternate", "random"], default="alternate")
    parser.add_argument("--labels", default="")
    parser.add_argument("--actions", type=int, default=120)
    parser.add_argument("--rounds", type=int, default=10)
    parser.add_argument("--batch-size", type=int, default=1)
    parser.add_argument("--input-path", choices=["semantic", "coordinate", "text"], default="semantic")
    parser.add_argument("--reuse-state", action="store_true", help="reuse a verified post-action AX state")
    parser.add_argument("--compact", action="store_true", help="print fixture tree and metrics only")
    args = parser.parse_args()
    signals = Signals().install()
    try:
        if args.command == "prepare":
            return prepare(args.folder.resolve(), args.mode)
        desk = Desk(args.folder.resolve())
        if args.command == "begin":
            result = desk.begin(args.actions)
        elif args.command == "act":
            result = desk.act(args.labels.split(","), args.input_path, reuse_state=args.reuse_state)
        elif args.command == "hands":
            result = desk.hands(args.actions, args.batch_size, args.input_path, reuse_state=args.reuse_state)
        elif args.command == "eyes":
            result = desk.eyes(args.rounds)
        else:
            result = getattr(desk, args.command)()
        if args.compact:
            look = result.get("look", result)
            result = {"tree": look.get("snapshot", {}).get("treeText"),
                      **({"summary": result} if "successful_actions" in result else {}),
                      **{key: result[key] for key in ("round", "summary", "quit") if key in result}}
        print(json.dumps(result, indent=2))
        return 0
    except Stopped as why:
        print(f"STOPPED: {why}; fixture left untouched in {args.folder}", file=sys.stderr)
        return 3
    except (RuntimeError, ValueError, OSError, subprocess.TimeoutExpired) as why:
        print(f"FAILED: {why}; fixture retained in {args.folder}", file=sys.stderr)
        return 1
    finally:
        signals.quiet()


if __name__ == "__main__":
    sys.exit(main())
