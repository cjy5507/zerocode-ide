#!/usr/bin/env python3
"""A scripted headless `zo` for the bench's own tests, speaking zo's `--json`
lines as zo-ide/crates/zo-ide/src/ide/render.rs writes them (a test pins the
keys). It records its words, the measuring environment and its one stdin line
to $FAKE_STATE/zo-calls, then follows $FAKE_ZO_SCRIPT (JSON):
  ndjson        lines to print (usage, tool_call, wire_model)
  steps         rows to append to $ZEROCODE_RUN_EVIDENCE_DIR/steps.jsonl
  write         {relpath: text} written under the work folder (its cwd)
  state         {name: text} written into $FAKE_STATE (the desk it changes)
  last_message  what --last-message receives
  sleep_s       how long it takes before it ends
  exit          its exit code
"""
import json
import os
import sys
import time

state = os.environ["FAKE_STATE"]
if "--version" in sys.argv:
    print("zo 0.0-fake")
    sys.exit(0)
prompt = sys.stdin.read()
keep = ("ZEROCODE_RUN_EVIDENCE_DIR", "ZO_PROFILE_DISABLE_HOOK_REPORTER", "ZO_TURN_DEADLINE_SECS",
        "ZO_SESSION_ROOT", "ZEROCODE_SECOND_BRAIN", "ZO_DREAM", "ZO_AUTO_VERIFY", "ZO_COMPUTER_MARKS")
with open(os.path.join(state, "zo-calls"), "a") as log:
    clipboard = os.path.join(state, "clipboard")
    log.write(json.dumps({"argv": sys.argv[1:], "env": {name: os.environ.get(name) for name in keep},
                          "stdin": prompt, "cwd": os.getcwd(), "pid": os.getpid(),
                          "clipboard": open(clipboard).read() if os.path.exists(clipboard) else ""}) + "\n")
script = json.loads(os.environ.get("FAKE_ZO_SCRIPT") or "{}")
for line in script.get("ndjson", []):
    print(json.dumps(line), flush=True)
evidence = os.environ.get("ZEROCODE_RUN_EVIDENCE_DIR")
for row in script.get("steps", []):
    with open(os.path.join(evidence, "steps.jsonl"), "a") as handle:
        handle.write(json.dumps(row) + "\n")
for relpath, text in (script.get("write") or {}).items():
    with open(os.path.join(os.getcwd(), relpath), "w") as handle:
        handle.write(text)
for name, text in (script.get("state") or {}).items():
    with open(os.path.join(state, name), "w") as handle:
        handle.write(text)
if "--last-message" in sys.argv:
    with open(sys.argv[sys.argv.index("--last-message") + 1], "w") as handle:
        handle.write(script.get("last_message", ""))
time.sleep(script.get("sleep_s", 0))
sys.exit(script.get("exit", 0))
