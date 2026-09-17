#!/usr/bin/env python3
"""A scripted `zerocode-computer` for the bench's own tests.

It answers the looks the onboarding walk and smoke make, grants the
screenshots permission after FAKE_GRANT_AFTER polls, and speaks the window's
own forms: an answer on stdout, a refusal on stderr with exit 1. Every call is
appended to $FAKE_STATE/calls (its words) and $FAKE_STATE/calls-evidence (the
evidence folder it was logged into, a tab, its words). A call that the window
would log writes its step line into $ZEROCODE_RUN_EVIDENCE_DIR/steps.jsonl.

Knobs: FAKE_HELPER (a helper status object, JSON; default: none stands),
FAKE_REFUSE (`verb:n` — the n-th call of that verb is refused), FAKE_STOP_ON
(`verb:reason` — that verb stops the helper for `reason` and is refused
`stopped`),
FAKE_WINDOWS (list-all-windows, JSON; `fail` refuses it), FAKE_READ_TEXT,
FAKE_CONFIRMING (open questions status reports, JSON list consumed one per
poll), FAKE_QUIT_STUCK (quit leaves the app running), FAKE_DESKTOP_UNMARKED (a
desktop marked look answers that it numbered nothing, and why). The desk's state lives in
$FAKE_STATE: `running` (apps up, one per line — the scripted `open` adds one,
quit takes it off) and `clipboard` (the pasteboard the scripted osascript
keeps: absent is empty), and what a scripted zo or FAKE_STOP_ON left there:
`stopped` (the helper is stopped; the file holds the reason), `deaf` (the
chord cannot be heard), `confirming` (how many more status reads report a
question open).
"""
import json
import os
import pathlib
import sys
import time

verb = sys.argv[1] if len(sys.argv) > 1 else ""
words = [word for word in sys.argv[1:] if word != "--json"]
state = pathlib.Path(os.environ["FAKE_STATE"])
evidence = os.environ.get("ZEROCODE_RUN_EVIDENCE_DIR", "")
with (state / "calls").open("a") as log:
    log.write(" ".join(sys.argv[1:]) + "\n")
with (state / "calls-evidence").open("a") as log:
    log.write(evidence + "\t" + " ".join(sys.argv[1:]) + "\n")

# The verbs the window leaves a step line for (run_evidence::captures).
LOGGED = {"activate", "key", "type", "hold-key", "mouse-click", "mouse-move", "wait-for", "quit", "window-close",
          "clipboard-write", "screenshot", "open", "launch", "stop", "resume", "handoff", "wait"}
ACTS = {"activate", "key", "type", "hold-key", "mouse-click", "mouse-move", "quit", "window-close", "clipboard-write", "open", "launch"}


def count(name):
    path = state / name
    n = int(path.read_text()) if path.exists() else 0
    path.write_text(str(n + 1))
    return n


def log_step(ok, error=None):
    if verb in LOGGED and evidence and os.path.isdir(evidence):
        steps = pathlib.Path(evidence) / "steps.jsonl"
        n = len(steps.read_text().splitlines()) + 1 if steps.exists() else 1
        row = {"n": n, "at_epoch_ms": int(time.time() * 1000), "tool": "computer", "verb": verb, "argv": words, "ok": ok}
        if verb in ACTS:
            row["acts"] = True
        if error:
            row["error"] = error
            row["code"] = json.loads(error)["error"]["code"]
        with steps.open("a") as handle:
            handle.write(json.dumps(row) + "\n")


def say(result):
    log_step(True)
    print(json.dumps({"ok": True, "result": result}))
    sys.exit(0)


def refuse(code, message):
    line = json.dumps({"ok": False, "error": {"code": code, "message": message}})
    log_step(False, line)
    print(line, file=sys.stderr)
    sys.exit(1)


stop_on = os.environ.get("FAKE_STOP_ON", "")
if stop_on and verb == stop_on.partition(":")[0]:
    (state / "stopped").write_text(stop_on.partition(":")[2] or "hotkey")
if (state / "stopped").exists() and verb in ACTS:
    refuse("stopped", "the operator is stopped (" + (state / "stopped").read_text() + ")")

refusal = os.environ.get("FAKE_REFUSE", "")
if refusal:
    refused_verb, _, at = refusal.partition(":")
    if verb == refused_verb and count("refuse-" + verb) + 1 == int(at or 1):
        refuse("element_not_found", "fake shim refused this call")

if verb == "permissions":
    n = count("polls")
    granted = n >= int(os.environ.get("FAKE_GRANT_AFTER", "0"))
    say({
        "permissions": [
            {"id": "accessibility", "status": "granted"},
            {"id": "screenshots", "status": "granted" if granted else "not-granted"},
        ],
        "helper_app_path": "/Applications/ZeroCode.app/Contents/Resources/ZeroCode Computer Use.app",
        "next_step": "" if granted else "Grant Screen Recording to ZeroCode Computer Use, then refresh.",
    })
if verb == "screenshot":
    say({"screenshot": {"data": "AAAA", "width": 10, "height": 10, "scale": 2}})
if verb == "list-all-windows":
    if os.environ.get("FAKE_WINDOWS") == "fail":
        refuse("accessibility_error", "fake shim could not list the windows")
    say({"windows": json.loads(os.environ.get("FAKE_WINDOWS") or '[{"id": 1, "app": {"name": "Finder", "pid": 1}, "title": "Desk", "x": 0, "y": 0, "width": 10, "height": 10}]')})
if verb == "read":
    text = os.environ.get("FAKE_READ_TEXT", "hello")
    say({"text": text, "lines": [{"text": line} for line in text.split("\n")]})
if verb == "observe":
    looked = {"screenshot": {"width": 10}, "changed": [], "changedShare": 0.0, "stuck": False}
    if "--marks" in sys.argv and "--app" not in sys.argv and os.environ.get("FAKE_DESKTOP_UNMARKED"):
        looked["marks"] = {"unavailable": os.environ["FAKE_DESKTOP_UNMARKED"]}
    elif "--marks" in sys.argv:
        looked["marks"] = {"lookId": "1.1", "candidates": 2, "omitted": 1, "items": [
            {"mark": 1, "elementIndex": 4, "role": "button", "label": "1", "x": 0, "y": 0, "width": 20, "height": 20,
             "centerX": 10, "centerY": 10}]}
    say(looked)
if verb == "click":
    say({"action": {"path": "accessibility", "actionName": "AXPress"}})
if verb == "evidence":
    say({"dir": evidence or "/tmp/evidence-fake", "count": 4, "frames": 3})
if verb == "cursor-position":
    # FAKE_MOVE_AT: the person moves the pointer from that read on.
    moved = count("cursor-reads") >= int(os.environ.get("FAKE_MOVE_AT", "1000000"))
    say({"x": 101 if moved else 100, "y": 100})
if verb == "status":
    count("status-polls")
    helper = json.loads(os.environ["FAKE_HELPER"]) if os.environ.get("FAKE_HELPER") else "idle"
    if isinstance(helper, dict) and (state / "stopped").exists():
        helper = dict(helper, stopped=True, reason=(state / "stopped").read_text().strip() or "hotkey")
    if isinstance(helper, dict) and (state / "deaf").exists():
        helper = dict(helper, hotkeyHears=False)
    open_now = 0
    left = state / "confirming"
    if left.exists():
        remaining = int(left.read_text().strip() or 0)
        open_now = 1 if remaining > 0 else 0
        left.write_text(str(max(remaining - 1, 0)))
    # A window that has not started a helper session answers it idle.
    say({"stopped": None, "hotkey": "control+option+escape", "helper": helper,
         "pace": {"perSecond": 10, "burst": 20}, "confirming": open_now})
if verb == "capabilities":
    say({"platform": "macos", "provider": "fake"})
if verb == "displays":
    say({"displays": [{"index": 0, "width": 1512, "height": 982, "scale": 2}]})
if verb == "recipe-list":
    say({"recipes": []})
if verb == "clipboard-read":
    say({"text": (state / "clipboard").read_text() if (state / "clipboard").exists() else ""})
if verb == "clipboard-write":
    (state / "clipboard").write_text(sys.argv[sys.argv.index("--text") + 1])
    say({"written": True})
if verb == "quit":
    app = sys.argv[sys.argv.index("--app") + 1]
    running = state / "running"
    if running.exists() and not os.environ.get("FAKE_QUIT_STUCK"):
        running.write_text("".join(line + "\n" for line in running.read_text().splitlines() if line != app))
    say({"terminated": not os.environ.get("FAKE_QUIT_STUCK")})
if verb in ("wait", "mouse-move", "activate", "key", "type", "hold-key", "window-close", "wait-for", "open", "stop", "resume"):
    say({"verb": verb})
if verb == "batch":
    commands = json.loads(sys.argv[sys.argv.index("--commands") + 1])
    steps = [{"n": n, "verb": c[0], "ok": True, "ms": 0, "result": {}} for n, c in enumerate(commands, 1)]
    say({"ran": len(steps), "of": len(steps), "elapsedMs": 0, "steps": steps})
refuse("unsupported_capability", "fake shim knows no " + verb)
