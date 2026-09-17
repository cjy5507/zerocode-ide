#!/usr/bin/env python3
"""Live ZeroCode acceptance harness; only observed navigation buttons.

This is the Computer Use skill's harness-only allow-self path, not a general
self-control tool. It never types or answers a dialog. PID and labels must
come from a fresh live observation. Run one navigation action, inspect its
result, then decide the next case.
"""
import argparse
import base64
import json
from pathlib import Path
import re
import shutil
import subprocess
import time

from bench import Bench, Stopped

NAVIGATION = {"작업 상황판", "워크스페이스 보드", "지식 그래프", "아티팩트"}


def call(*args):
    run = subprocess.run(["zerocode-computer", *args, "--json"], text=True, capture_output=True)
    answer = json.loads(next((s for s in (run.stdout + run.stderr).splitlines() if s.startswith("{")), "{}"))
    if run.returncode or answer.get("ok") is not True:
        raise RuntimeError(str(answer.get("error") or "Computer Use refused"))
    return answer["result"]


def guard():
    status = call("status")
    if status.get("stopped") or status.get("confirming") or status.get("helper", {}).get("stopped"):
        raise Stopped("operator stopped or a confirmation is open")
    if status.get("helper", {}).get("hotkeyHears") is not True:
        raise Stopped("stop hotkey unavailable")
    if Bench.hid_idle_s() < 3 or Bench.screen_locked():
        raise Stopped("operator active or screen locked")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--label", choices=sorted(NAVIGATION), required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    guard()
    app = f"pid:{args.pid}"
    before = call("get-app-state", "--app", app, "--no-screenshot")
    snapshot = before["snapshot"]
    if snapshot.get("app", {}).get("bundleId") != "dev.zerocode.app":
        raise RuntimeError("target is not the installed ZeroCode app")
    if re.search(r"^\s*\d+ (?:sheet|dialog)\b", snapshot["treeText"], re.M):
        raise Stopped("a dialog is open; navigation harness will not touch it")
    found = call("find", "--app", app, "--text", args.label)
    matches = [m for m in found["matches"] if m.get("label") == args.label
               and isinstance(m.get("frame"), dict) and m.get("role") in ("AXButton", "AXCheckBox") and "AXPress" in m.get("actions", [])]
    if found.get("coordinateSpace") != "screen":
        raise RuntimeError("navigation frames are not screen coordinates")
    if not matches or found.get("truncated"):
        raise RuntimeError("navigation control is missing or ambiguous to observation")
    # The application rail is the leftmost exact control. A transcript row
    # merely containing the same words cannot match this label equality.
    matches.sort(key=lambda m: m["frame"]["x"])
    if len(matches) > 1 and matches[0]["frame"]["x"] == matches[1]["frame"]["x"]:
        raise RuntimeError("ambiguous navigation controls")
    target = matches[0]
    window = snapshot["window"]
    if found.get("window", {}).get("id") != window["id"]:
        raise RuntimeError("window changed between observations")
    x, y = target["frame"]["centerX"], target["frame"]["centerY"]
    windows = call("list-all-windows", "--all-layers")["windows"]
    if any("layer" not in w for w in windows):
        raise RuntimeError("the provider did not return every-layer window geometry")
    at_point = next((w for w in windows if not w.get("overlay") and w.get("alpha", 1) > 0 and w["x"] <= x < w["x"] + w["width"] and
                     w["y"] <= y < w["y"] + w["height"]), None)
    if not at_point or at_point["id"] != window["id"] or at_point["app"]["pid"] != args.pid:
        raise Stopped("another window covers the navigation control")
    guard()
    result = call("mouse-click", "--x", str(x), "--y", str(y), "--allow-self")
    # This wait is a test observation boundary, not an input pacing policy.
    time.sleep(0.2)
    after = call("get-app-state", "--app", app)
    (args.out / "state.json").write_text(json.dumps(after, ensure_ascii=False))
    (args.out / "tree.txt").write_text(after["snapshot"]["treeText"])
    screenshot = after.get("screenshot") or {}
    data = screenshot.get("data") or screenshot.get("base64")
    if data:
        (args.out / "screen.png").write_bytes(base64.b64decode(data))
    elif screenshot.get("path"):
        shutil.copyfile(screenshot["path"], args.out / "screen.png")
    print(json.dumps({"clicked": args.label, "window": window["id"], "frame": target["frame"],
                      "action": result, "screenshotKeys": list(screenshot), "out": str(args.out)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
