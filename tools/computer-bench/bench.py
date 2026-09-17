#!/usr/bin/env python3
"""The Computer Use bench's runner (docs/design/computer-use-bench.md §4).

Usage:
  bench.py setup                      the automation to create once, and its schedule
  bench.py precheck                   0 only when a scheduled start may take the desk
  bench.py check [--scenario ID…]     the hands lane only: each reference path, no model
  bench.py run [--scenario ID…] [--runs N] [--config ID…]
               [--max-minutes M] [--no-window-evidence]
  bench.py restore-clipboard DIR      put back a clipboard a killed runner kept in DIR

Two lanes over one scenario table (bench.json): the HANDS lane replays a
scenario's reference commands through zerocode-computer — no model, seconds a
run — and proves the scenario, its setup and its oracle; the LIVE lane gives
the scenario's one-line prompt to headless zo, which has the Computer tool and
no other. `run` walks the hands lane first, and a scenario's live runs count
only after its hands lane passed every run in the same session. Success is
always an oracle outside the model (file bytes, a tree, Calculator's display).
Live runs use the table's first config unless --config names others; named
configs take turns run by run, so an A/B shares the session's conditions.

The runner never lifts a stop and never acts after one: a person's stop, a
stop chord that can no longer be heard, a question left on the person's
screen, a model's stop or resume, or a signal to the runner (Ctrl+C, the pane
closing) ends the bench — zo is killed, the desk is left as it is, and the
person's clipboard, kept for the whole bench, is put back.

It drives the person's desktop, so it runs only as the "bench:computer"
automation (Run now, or a schedule whose precheck finds the Mac idle): the
automation's shell carries the launch token and a fenced evidence folder.
Exit: 0 completed (whatever the rate), 2 refused before anything moved,
3 stopped, 1 a runner error.
"""
import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import scenarios  # noqa: E402
import tally  # noqa: E402

SHIM = "zerocode-computer"
EVIDENCE_ENV = "ZEROCODE_RUN_EVIDENCE_DIR"
# What the runner sets for each live run; a config's env never names these.
RUN_ENV = (EVIDENCE_ENV, "ZO_TURN_DEADLINE_SECS", "ZO_SESSION_ROOT")
PASTEBOARD = os.path.join(HERE, "pasteboard.js")
# Oracle checks that read the model's own answer: a hands run has none.
MODEL_CHECKS = {"answer_digits"}
# Why the runner stopped, in its own words (tally reads the model's prefix).
CHORD_DEAF = "chord_deaf"
QUESTION_OPEN = "question_open"
NO_HELPER = "no_helper"


class Refused(Exception):
    """The bench would not start: nothing moved."""


class Stopped(Exception):
    """The bench ends here: the desk is not the runner's to touch."""


def now_ms():
    return int(time.time() * 1000)


def run_quiet(argv, **kwargs):
    try:
        return subprocess.run(argv, capture_output=True, text=True, **kwargs)
    except (OSError, subprocess.TimeoutExpired):
        return None


class Signals:
    """Ctrl+C, the pane closing, a kill: the first one ends the bench where it
    stands (a Stopped raised in the main thread); any after it is noted and
    ignored, so the cleanup it started — zo killed, the clipboard put back —
    runs to its end."""

    def __init__(self):
        self.first = None

    def install(self):
        for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            signal.signal(sig, self.caught)
        return self

    def quiet(self):
        """From here on (the cleanup) a signal is noted, never raised."""
        self.first = self.first or "cleanup"

    def caught(self, signum, _frame):
        if self.first is None:
            self.first = signal.Signals(signum).name
            raise Stopped(f"runner:{self.first}")


class Clipboard:
    """The person's clipboard, every item and type, kept in a private folder
    for the whole bench (pasteboard.js): cleared before each run so no run
    reads it or inherits another's, put back at the end. The folder stays
    until the put-back worked; `bench.py restore-clipboard DIR` finishes the
    job after a runner that was killed outright."""

    def __init__(self, name=None):
        self.name = name
        self.dir = None

    def call(self, mode, folder=None):
        argv = ["osascript", "-l", "JavaScript", PASTEBOARD, mode, folder or self.dir or "-"]
        answer = run_quiet(argv + ([self.name] if self.name else []), timeout=60)
        if not answer or answer.returncode != 0:
            why = (answer.stderr.strip() if answer else "") or "no answer"
            raise RuntimeError(f"pasteboard {mode}: {why}")
        return answer.stdout.strip()

    def keep(self):
        folder = tempfile.mkdtemp(prefix="zc-bench-clipboard-")
        try:
            items = self.call("save", folder)
        except RuntimeError:
            shutil.rmtree(folder, ignore_errors=True)
            raise
        self.dir = folder  # kept: from here on it is put back
        self.call("clear")
        return items

    def clear(self):
        if self.dir:
            self.call("clear")

    def put_back(self):
        """True once the person's clipboard is back (or nothing was kept)."""
        if not self.dir:
            return True
        try:
            self.call("restore")
        except RuntimeError as why:
            print(f"bench: the clipboard is NOT back ({why}) — it is kept in {self.dir}; "
                  f"`python3 {os.path.join(HERE, 'bench.py')} restore-clipboard {self.dir}` puts it back", file=sys.stderr)
            return False
        shutil.rmtree(self.dir, ignore_errors=True)
        self.dir = None
        return True


class Bench:
    def __init__(self, values, top):
        self.values = values
        self.top = top
        self.runner = os.path.join(top, values["runner_dir"])
        os.makedirs(self.runner, exist_ok=True)
        self.session = time.strftime("%Y%m%d-%H%M%S") + "-" + uuid.uuid4().hex[:6]
        self.work_root = os.path.join(os.environ.get("TMPDIR") or "/tmp", values["work_root_name"])
        self.zo = os.environ.get("BENCH_ZO_BIN") or "zo"
        self.clipboard = Clipboard(os.environ.get("BENCH_PASTEBOARD_NAME"))
        self.process = None
        self.left = []

    # ------------------------------------------------------------ the shim --
    def shim(self, argv, folder=None, timeout=None):
        """One `zerocode-computer … --json` call, logged into `folder` (the
        runner's own by default): the envelope, or None."""
        env = dict(os.environ)
        env[EVIDENCE_ENV] = folder or self.runner
        answer = run_quiet([SHIM, *argv, "--json"], env=env, timeout=timeout)
        if answer is None:
            return None
        return tally.first_json_line(answer.stdout) or tally.first_json_line(answer.stderr)

    def own(self):
        """The runner's own road: its calls log into top/runner."""
        return lambda argv: self.shim(argv)

    def status(self):
        return scenarios.result(self.shim(["status"]))

    def helper_status(self):
        """status, with the helper's session stood up when none stands."""
        status = self.status()
        if not isinstance(status.get("helper"), dict):
            self.shim(["screenshot"])  # a look stands the session; then read again
            status = self.status()
        return status

    @staticmethod
    def stop_reason(status):
        """Whether the window or the helper is stopped, and why."""
        helper = status.get("helper") if isinstance(status.get("helper"), dict) else {}
        if status.get("stopped"):
            return str(status["stopped"])
        if helper.get("stopped"):
            return str(helper.get("reason") or "helper")
        return None

    @staticmethod
    def unheard(status):
        """Why the stop chord cannot stop the bench now, or None. hotkeyHears
        is false while any process holds secure keyboard input: only the
        window's stop button would work then."""
        helper = status.get("helper")
        if not isinstance(helper, dict):
            return NO_HELPER
        if not helper.get("hotkeyArmed") or helper.get("hotkeyHears") is not True:
            return CHORD_DEAF
        return None

    def guard(self):
        """Before the runner opens or acts: the desk must still be the runner's
        to touch — nothing stopped, the chord armed and heard, no question on
        the person's screen. Anything else ends the bench (Stopped)."""
        status = self.helper_status()
        reason = self.stop_reason(status) or self.unheard(status) or (QUESTION_OPEN if status.get("confirming") else None)
        if reason:
            raise Stopped(reason)

    def refused_by_the_desk(self, answer):
        """A refusal that means the desk is not the runner's: end the bench."""
        error = (answer or {}).get("error")
        code = error.get("code") if isinstance(error, dict) else None
        if code in tally.STOP_CODES or code == tally.PERSON_ASKED_CODE:
            raise Stopped(self.stop_reason(self.status()) or code)

    # -------------------------------------------------------- the machine --
    @staticmethod
    def hid_idle_s():
        answer = run_quiet(["ioreg", "-c", "IOHIDSystem", "-d", "4"])
        match = re.search(r'"HIDIdleTime"\s*=\s*(\d+)', answer.stdout if answer else "")
        return int(match.group(1)) / 1e9 if match else 0.0

    @staticmethod
    def screen_locked():
        answer = run_quiet(["ioreg", "-n", "Root", "-d1"])
        return bool(answer and re.search(r'"CGSSessionScreenIsLocked"\s*=\s*Yes', answer.stdout))

    @staticmethod
    def tabbing_always():
        answer = run_quiet(["defaults", "read", "-g", "AppleWindowTabbingMode"])
        return bool(answer and answer.returncode == 0 and answer.stdout.strip() == "always")

    def versions(self):
        zo = run_quiet([self.zo, "--version"])
        macos = run_quiet(["sw_vers", "-productVersion"])
        return {
            "zo": zo.stdout.strip() if zo and zo.returncode == 0 else None,
            "macos": macos.stdout.strip() if macos and macos.returncode == 0 else None,
            "displays": scenarios.result(self.shim(["displays"])).get("displays"),
            "capabilities": scenarios.result(self.shim(["capabilities"])),
        }

    # ------------------------------------------------------------- before --
    def worst_case_minutes(self, chosen, lanes, runs, configs):
        hands = self.values["hands_lane_runs"] if "hands" in lanes else 0
        live = runs * len(configs) if "live" in lanes else 0
        grace = self.values["kill_grace_s"]
        return sum((hands + live) * (scenario["budget_s"] + grace) for scenario in chosen) / 60

    def preflight(self, chosen, max_minutes, worst):
        """Every read-only check; refuses with every reason at once."""
        reasons = []
        if self.screen_locked():
            reasons.append("the screen is locked")
        smoke = run_quiet(["sh", os.path.join(HERE, "smoke.sh")], env=dict(os.environ, **{EVIDENCE_ENV: self.runner}), timeout=120)
        if not smoke or "SMOKE GREEN" not in smoke.stdout:
            reasons.append("the looks-only smoke is not green: " + (smoke.stdout.strip().splitlines()[-1] if smoke and smoke.stdout.strip() else "no answer"))
        status = self.helper_status()
        helper = status.get("helper")
        stopped = self.stop_reason(status)
        if stopped:
            reasons.append(f"the operator is stopped ({stopped})")
        if status.get("confirming"):
            reasons.append("a question is open on the person's screen")
        if not isinstance(helper, dict):
            reasons.append(f"no helper session stands (status says {helper!r})")
        else:
            if not helper.get("hotkeyArmed"):
                reasons.append("the stop chord is not armed")
            elif helper.get("hotkeyHears") is not True:
                reasons.append("the stop chord cannot be heard now (a process holds secure keyboard input"
                               + ("" if "hotkeyHears" in helper else ", or this ZeroCode is older than the bench") + ")")
            if helper.get("hotkey") != status.get("hotkey"):
                reasons.append(f"the armed chord {helper.get('hotkey')!r} is not the one the window names {status.get('hotkey')!r}")
            budget = (helper.get("budget") or {}).get("perSession") or 0
            if budget and helper.get("actions", 0) >= self.values["budget_fraction_max"] * budget:
                reasons.append(f"{helper.get('actions')} of {budget} session actions are spent — lifting the budget is the person's call")
            last = helper.get("lastActionAt")
            if isinstance(last, int) and now_ms() - last < self.values["inter_run_quiet_s"] * 1000:
                reasons.append("another operator acted a moment ago")
        permissions = scenarios.result(self.shim(["permissions"])).get("permissions") or []
        missing = [row.get("id") for row in permissions if isinstance(row, dict) and row.get("status") != "granted"]
        if missing or not permissions:
            reasons.append("permissions not granted: " + (", ".join(map(str, missing)) or "none reported"))
        for scenario in chosen:
            try:
                scenarios.desk_ready(scenario, self.own())
            except scenarios.SetupFailed as why:
                reasons.append(f"{scenario['id']}: {why}")
            if scenario.get("tabbing_sensitive") and self.tabbing_always():
                reasons.append(f"{scenario['id']}: windows open as tabs (AppleWindowTabbingMode always) — a folder would land in the person's window")
        if worst > max_minutes:
            reasons.append(f"the plan may take {worst:.0f} min, over --max-minutes {max_minutes:.0f}")
        if reasons:
            raise Refused("; ".join(reasons))
        return status

    def countdown(self, status):
        seconds = self.values["countdown_s"]
        chord = (status.get("helper") or {}).get("hotkey") or status.get("hotkey")
        print(f"bench: taking the desk in {seconds} s — hands off; {chord} stops it at any moment", flush=True)
        time.sleep(seconds)
        if self.hid_idle_s() < seconds - self.values["countdown_idle_tolerance_s"]:
            raise Refused("a key or the mouse moved during the countdown: the person is still here")

    def quiet_between_runs(self):
        quiet = self.values["inter_run_quiet_s"]
        time.sleep(quiet)
        if self.hid_idle_s() < quiet:
            raise Stopped("person_returned")

    # --------------------------------------------------------------- runs --
    def folder(self, lane, scenario, k, config=None):
        name = "-".join([lane, scenario["id"]] + ([config["id"]] if config else []) + [str(k)])
        path = os.path.join(self.top, name)
        os.makedirs(path, exist_ok=True)  # the window admits only a folder that exists
        return path

    def work(self, lane, scenario, k, config=None):
        name = "-".join([scenario["id"], lane] + ([config["id"]] if config else []) + [str(k)])
        path = os.path.join(self.work_root, self.session, name)
        os.makedirs(path, exist_ok=True)
        return path

    def write_run(self, folder, body):
        with open(os.path.join(folder, tally.BENCH_RUN), "w", encoding="utf-8") as handle:
            handle.write(scenarios.dump(body))

    def begin(self, scenario, folder, work, body, state):
        """The desk still the runner's, a clean clipboard, the desk as the
        bench left it, fixtures, then setup. Answers the fixtures' hashes, or
        None when the run was not set up (its body written, its teardown
        walked). A stop in setup writes the body and ends the bench."""
        self.guard()
        self.clipboard.clear()
        try:
            scenarios.desk_ready(scenario, self.own())
            hashes = scenarios.make_fixtures(work, scenario.get("fixtures") or [])
            scenarios.setup(work, scenario.get("setup") or [], self.own(), self.values, state, self.guard)
            body["setup"] = {"ok": True}
            return hashes
        except scenarios.SetupFailed as why:
            body["setup"] = {"ok": False, "why": str(why)}
            body["started_at_ms"] = body["ended_at_ms"] = now_ms()
            self.end(scenario, folder, work, body, state)
            return None
        except Stopped as why:
            body.update(stopped_by=str(why), stopped_in="setup")
            body["started_at_ms"] = body["ended_at_ms"] = now_ms()
            self.write_run(folder, body)
            self.leave(work, f"{scenario['id']}: its apps and {work}")
            raise

    def end(self, scenario, folder, work, body, state):
        """The run's file first, whatever happens next; then teardown, only
        while the desk is still the runner's, and what it left."""
        self.write_run(folder, body)
        try:
            self.guard()
            body["teardown_left"] = scenarios.teardown(work, scenario.get("teardown") or [], self.own(), self.values, state, self.work_root)
        except Stopped:
            self.leave(work, f"{scenario['id']}: its apps and {work}")
            raise
        self.left.extend(body["teardown_left"])
        self.write_run(folder, body)

    def leave(self, work, what):
        self.left.append(f"left as it is after the stop: {what}")

    def run_hands(self, scenario, k):
        folder, work = self.folder("hands", scenario, k), self.work("hands", scenario, k)
        body = {"schema": 1, "scenario": scenario["id"], "lane": "hands", "k": k, "config": {"id": "hands"}}
        state = {}
        hashes = self.begin(scenario, folder, work, body, state)
        if hashes is None:
            return False
        body["started_at_ms"] = now_ms()
        latencies, refused = [], None
        try:
            for argv in scenarios.expand(scenario["reference"]):
                self.guard()
                started = time.monotonic()
                answer = self.shim(argv, folder)
                latencies.append({"verb": argv[0], "ms": round((time.monotonic() - started) * 1000, 1)})
                if not scenarios.ok(answer):
                    self.refused_by_the_desk(answer)
                    refused = {"argv": argv, "error": (answer or {}).get("error")}
                    break
            body["step_latency_ms"] = latencies
            body["refused"] = refused
            checks = [spec for spec in scenario["oracle"] if spec["kind"] not in MODEL_CHECKS]
            body["oracle"] = scenarios.oracle(work, checks, self.own(), None, hashes)
        except Stopped as why:
            body.update(stopped_by=str(why), step_latency_ms=latencies, ended_at_ms=now_ms())
            self.write_run(folder, body)
            self.leave(work, f"{scenario['id']}: its apps and {work}")
            raise
        if refused:
            body["oracle"]["pass"] = False
        body["ended_at_ms"] = now_ms()
        body["run_wall_ms"] = body["ended_at_ms"] - body["started_at_ms"]
        self.end(scenario, folder, work, body, state)
        return bool(body["oracle"]["pass"])

    def zo_argv(self, work, last_message, config):
        argv = [self.zo, *self.values["zo_flags"], "--allowed-tools", ",".join(self.values["allowed_tools"]),
                "--cwd", work, "--last-message", last_message]
        if config.get("model"):
            argv += ["--model", config["model"]]
        if config.get("effort"):
            argv += ["--effort", config["effort"]]
        return argv

    def run_live(self, scenario, k, config, hands_proved):
        folder, work = self.folder("live", scenario, k, config), self.work("live", scenario, k, config)
        last_message = os.path.join(folder, "last-message.txt")
        prompt = f"{self.values['preamble']} {scenario['prompt']}"
        if "\n" in prompt:
            raise ValueError(f"{scenario['id']}: a prompt is one line (zo reads stdin a line at a time)")
        body = {"schema": 1, "scenario": scenario["id"], "lane": "live", "k": k, "hands_proved": hands_proved,
                "config": {"id": config["id"], "model_asked": config.get("model"), "effort": config.get("effort"),
                           "env": config_env(self.values, config), "wire_model": None}}
        state = {}
        hashes = self.begin(scenario, folder, work, body, state)
        if hashes is None:
            return
        try:
            self.guard()
        except Stopped as why:
            body.update(stopped_by=str(why), stopped_in="before zo")
            self.write_run(folder, body)
            self.leave(work, f"{scenario['id']}: its apps and {work}")
            raise
        before = self.status().get("helper")
        before = before if isinstance(before, dict) else {}
        env = live_env(self.values, config, os.environ)
        env.update(zip(RUN_ENV, (folder, str(scenario["budget_s"]), os.path.join(folder, "zo-sessions"))))
        body["started_at_ms"] = now_ms()
        stopped, timed_out, process = None, False, None
        try:
            with open(os.path.join(folder, "zo.ndjson"), "w", encoding="utf-8") as out, open(os.path.join(folder, "zo.stderr"), "w", encoding="utf-8") as err:
                process = self.process = subprocess.Popen(self.zo_argv(work, last_message, config), stdin=subprocess.PIPE, stdout=out, stderr=err,
                                                          env=env, cwd=work, text=True, start_new_session=True)
                process.stdin.write(prompt + "\n")
                process.stdin.close()
                stopped, timed_out = self.watch(process, folder, scenario["budget_s"])
        except Stopped as why:
            stopped = str(why)  # a signal to the runner while zo ran
        finally:
            if process is not None and process.poll() is None:
                self.kill(process)
            self.process = None
        body["ended_at_ms"] = now_ms()
        body["run_wall_ms"] = body["ended_at_ms"] - body["started_at_ms"]
        body["exit"] = {"code": process.returncode if process else None, "timed_out": timed_out, "killed": bool(stopped or timed_out)}
        stream = read_stream(os.path.join(folder, "zo.ndjson"))
        body["config"]["wire_model"] = stream["wire_model"]
        body["tokens_total"] = stream["tokens_total"]
        body["tool_calls"] = stream["tool_calls"]
        body["off_road"] = sorted(name for name in stream["tool_calls"] if name not in self.values["allowed_tools"])
        settled = True
        try:
            settled = self.settle_questions()
            self.shim(["evidence", "--last", str(self.values["flush_last"])], folder)
            steps = tally.read_steps(folder)
            body["forbidden_verbs"] = sorted({row.get("verb") for row in steps if row.get("verb") in self.values["forbid_verbs"]})
            after = self.status().get("helper")
            if isinstance(after, dict) and isinstance(before.get("actions"), int):
                body["hand_actions"] = after.get("actions", 0) - before["actions"]
            message = read_text(last_message)
            body["claimed"] = scenarios.claim(message, self.values["self_result_pattern"])
            body["oracle"] = scenarios.oracle(work, scenario["oracle"], self.own(), message, hashes)
        except Stopped as why:
            stopped = stopped or str(why)  # a signal while the run was being judged: it is not judged
        if stopped:
            body["stopped_by"] = stopped
        if stopped or not settled:
            self.write_run(folder, body)
            self.leave(work, f"{scenario['id']}: its apps and {work}")
            raise Stopped(stopped or QUESTION_OPEN)
        self.end(scenario, folder, work, body, state)

    def watch(self, process, folder, budget_s):
        """Poll until zo ends. The model's own stop or resume in the run's
        step log, a stop in status, a chord that went deaf, or a stopped
        refusal kills it and ends the bench; past the budget it is killed and
        failed. Answers (stopped_by, timed_out)."""
        deadline = time.monotonic() + budget_s + self.values["kill_grace_s"]
        seen, model, code = 0, None, None
        while process.poll() is None:
            time.sleep(self.values["status_poll_s"])
            rows = tally.read_steps(folder)
            for row in rows[seen:]:
                if row.get("verb") in ("resume", "stop"):
                    model = model or f"{tally.MODEL_STOP_PREFIX}{row['verb']}"
                elif not row.get("ok", True) and tally.refusal_code(row) in tally.STOP_CODES:
                    code = code or tally.refusal_code(row)
            seen = len(rows)
            status = self.status()
            reason = model or self.stop_reason(status) or code or self.unheard(status)
            if reason:
                self.kill(process)
                return reason, False
            if time.monotonic() > deadline:
                self.kill(process)
                return None, True
        return None, False

    def kill(self, process):
        for sig, wait in ((signal.SIGTERM, self.values["sigkill_after_s"]), (signal.SIGKILL, 5)):
            try:
                os.killpg(process.pid, sig)
            except (ProcessLookupError, PermissionError):
                return
            try:
                process.wait(timeout=wait)
                return
            except subprocess.TimeoutExpired:
                continue

    def settle_questions(self):
        """A question the model left open — a press's confirmation or a
        handoff — is still on the person's screen: wait for it to close (the
        window's own timeout ends it) before the oracle looks and teardown
        moves anything. False when it is still open past the table's bound."""
        deadline = time.monotonic() + self.values["question_wait_ms"] / 1000
        while self.status().get("confirming"):
            if time.monotonic() >= deadline:
                return False
            time.sleep(self.values["status_poll_s"])
        return True


def read_text(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read()
    except OSError:
        return ""


def read_stream(path):
    """What zo's --json lines say (zo-ide render.rs): tokens from the last
    usage line, each tool's calls by distinct id, and the wire model."""
    tokens, calls, wire = None, {}, None
    for line in read_text(path).splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = event.get("type")
        if kind == "usage":
            tokens = event.get("total_tokens")
        elif kind == "tool_call":
            calls.setdefault(event.get("name"), set()).add(event.get("id"))
        elif kind == "wire_model":
            wire = event.get("model")
    return {"tokens_total": tokens, "tool_calls": {name: len(ids) for name, ids in calls.items()}, "wire_model": wire}


def keep_awake():
    """The display stays awake while the bench holds the desk."""
    try:
        subprocess.Popen(["caffeinate", "-d", "-w", str(os.getpid())], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except OSError:
        pass


def print_setup():
    command = f"python3 {os.path.join(HERE, 'bench.py')} run"
    print(json.dumps({
        "name": "bench:computer",
        "workspace": os.path.dirname(os.path.dirname(HERE)),
        "command": command,
        "agent": None,
        "prompt": "computer bench",
        "evidence": "always",
        "close_when_done": False,
        "enabled": False,
        "schedule (optional)": {"at": "a quiet hour", "precheck": f"python3 {os.path.join(HERE, 'bench.py')} precheck"},
    }, indent=2))
    return 0


def config_env(values, config):
    """A config's own environment for zo: never a key the runner owns (its
    zo_env, or what it sets per run)."""
    env = config.get("env") or {}
    owned = sorted(set(env) & (set(values["zo_env"]) | set(RUN_ENV)))
    if owned:
        raise ValueError(f"config {config['id']}: {', '.join(owned)} is the runner's to set")
    return env


def live_env(values, config, inherited):
    """zo's environment for one live run, before the per-run keys: a key any
    config sets is never inherited, so each side of an A/B has only its own."""
    named = {key for other in values["configs"] for key in (other.get("env") or {})}
    env = {key: value for key, value in inherited.items() if key not in named}
    env.update(values["zo_env"])
    env.update(config_env(values, config))
    return env


def chosen_configs(values, asked):
    """The table's first config, or the ones --config names, in table order."""
    known = [config["id"] for config in values["configs"]]
    unknown = [name for name in asked if name not in known]
    if unknown:
        raise ValueError(f"no config {', '.join(unknown)} (the table has {', '.join(known)})")
    configs = [config for config in values["configs"] if config["id"] in asked] if asked else values["configs"][:1]
    for config in configs:
        config_env(values, config)
    return configs


def walk(bench, chosen, lanes, runs, configs):
    """The hands lane, then the live runs of each scenario it proved — the
    configs taking turns run by run."""
    proved = set()
    if "hands" in lanes:
        for scenario in chosen:
            passes = 0
            for k in range(1, bench.values["hands_lane_runs"] + 1):
                passes += bench.run_hands(scenario, k)
                bench.quiet_between_runs()
            if passes == bench.values["hands_lane_runs"]:
                proved.add(scenario["id"])
            else:
                print(f"bench: {scenario['id']} is invalid — its hands lane passed {passes}/{bench.values['hands_lane_runs']}; its live runs are skipped", flush=True)
    if "live" in lanes:
        for scenario in chosen:
            if scenario["id"] not in proved:
                continue
            for k in range(1, runs + 1):
                for config in configs:
                    bench.run_live(scenario, k, config, hands_proved=True)
                    bench.quiet_between_runs()


def main(argv):
    parser = argparse.ArgumentParser(prog="bench.py", description=__doc__.split("\n\n")[0])
    parser.add_argument("action", choices=["setup", "precheck", "check", "run", "restore-clipboard"])
    parser.add_argument("folder", nargs="?", help="restore-clipboard: the folder the runner named")
    parser.add_argument("--scenario", action="append", default=[])
    parser.add_argument("--runs", type=int)
    parser.add_argument("--config", action="append", default=[])
    parser.add_argument("--max-minutes", type=float)
    parser.add_argument("--no-window-evidence", action="store_true", help="development only: no fenced evidence folder")
    args = parser.parse_args(argv[1:])
    values = tally.table(os.environ.get("BENCH_TABLE"))
    chosen = [scenario for scenario in values["scenarios"] if not args.scenario or scenario["id"] in args.scenario]
    if args.action == "setup":
        return print_setup()
    if args.action == "restore-clipboard":
        keeper = Clipboard(os.environ.get("BENCH_PASTEBOARD_NAME"))
        keeper.dir = args.folder
        return 0 if args.folder and keeper.put_back() else 1
    if args.action == "precheck":
        idle = Bench.hid_idle_s() >= values["idle_before_scheduled_start_s"]
        running = any(scenarios.app_pids(app) != set() for scenario in chosen for app in scenario.get("apps_not_running") or [])
        return 0 if idle and not running and not Bench.screen_locked() else 1
    top = os.environ.get(EVIDENCE_ENV)
    if not top:
        if not args.no_window_evidence:
            print(f"bench: refused — run it as the bench:computer automation (no {EVIDENCE_ENV}: a hand-started terminal cannot drive the desktop)", file=sys.stderr)
            return 2
        top = os.path.join(os.environ.get("TMPDIR") or "/tmp", "zerocode-computer-bench-dev", time.strftime("%Y%m%d-%H%M%S"))
    os.makedirs(top, exist_ok=True)
    lanes = {"hands"} if args.action == "check" else {"hands", "live"}
    runs = args.runs or values["runs_per_scenario"]
    try:
        configs = chosen_configs(values, args.config)
    except ValueError as why:
        print(f"bench: refused — {why}", file=sys.stderr)
        return 2
    bench = Bench(values, top)
    worst = bench.worst_case_minutes(chosen, lanes, runs, configs)
    signals = Signals().install()
    took_desk, code = False, 0
    try:
        status = bench.preflight(chosen, args.max_minutes if args.max_minutes is not None else worst, worst)
        bench.countdown(status)
        took_desk = True
        keep_awake()
        items = bench.clipboard.keep()
        print(f"bench: the clipboard ({items} item(s)) is kept in {bench.clipboard.dir} until the bench ends", flush=True)
        walk(bench, chosen, lanes, runs, configs)
    except Refused as why:
        print(f"bench: refused — {why}", file=sys.stderr)
        return 2
    except Stopped as who:
        if not took_desk:
            print(f"bench: refused — {who} before the desk was taken", file=sys.stderr)
            return 2
        print(f"bench: stopped by {who} — the rest did not run; the runner lifted nothing", file=sys.stderr)
        code = 3
    finally:
        signals.quiet()
        if bench.process is not None and bench.process.poll() is None:
            bench.kill(bench.process)
        if not bench.clipboard.put_back():
            bench.left.append(f"the clipboard, kept in {bench.clipboard.dir}")
    return finish(top, bench, code)


def finish(top, bench, code):
    summary = tally.summarize(tally.collect(top, bench_only=True), bench.values)
    table_md = tally.markdown(summary, bench.versions())
    if bench.left:
        table_md += "\n" + "\n".join(f"- left: {what}" for what in bench.left) + "\n"
    with open(os.path.join(top, "table.md"), "w", encoding="utf-8") as handle:
        handle.write(table_md)
    print(table_md, end="")
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv))
