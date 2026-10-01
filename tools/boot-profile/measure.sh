#!/usr/bin/env bash
# Start ONE window from a synthetic profile and read back its boot timeline
# (t-20078). Everything it touches lives under --out: the window runs with
# HOME pointed there, so the person's state folders (~/Library/Application
# Support/dev.zerocode.app, ~/.zo) are on no road, and the window it starts is
# its own process — it never signals, types into or reads any other window.
#
#   measure.sh --binary <zerocode-shell> --profile <dir from the profile test>
#              --out <fresh dir> [--low-spec] [--runs N]
#
# --profile is the folder `writes_the_synthetic_boot_profile` wrote
# (pane-layouts.json + orchestration.json). --low-spec runs the window under
# `taskpolicy -b`, the efficiency-core profile: wall-clock startup is valid
# there, frame-rate numbers are not (the throttle holds animation at ~10 Hz).
#
# The Chromium helpers are not rebuilt: a bundle shim holds a copy of the
# installed app's Frameworks (read, never written), which is all the window
# needs to find them.
set -euo pipefail

# How long one start may take before it is called stuck, and how often the
# timeline file is looked at. Seconds; the slow start this was written for took
# about a hundred, so three times that is "stuck".
START_TIMEOUT=300
POLL_INTERVAL=0.25
# Browser tabs stored for the restore: the person's window held a few.
BROWSER_TABS=3
INSTALLED_FRAMEWORKS="/Applications/ZeroCode.app/Contents/Frameworks"
TIMELINE_FILE="boot-timeline.json"
APP_ID="dev.zerocode.app"

binary="" profile="" out="" low=false runs=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary) binary="$2"; shift 2 ;;
    --profile) profile="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --low-spec) low=true; shift ;;
    --runs) runs="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ -x "$binary" && -d "$profile" && "$out" = /* && ! -e "$out" ]] \
  || { echo "need --binary (executable), --profile (dir), --out (absolute, not existing)" >&2; exit 2; }
[[ -d "$INSTALLED_FRAMEWORKS" ]] || { echo "no installed Frameworks to borrow" >&2; exit 2; }
# The person's real home is never a target.
[[ "$out" != "$HOME"/Library* && "$out" != "$HOME"/.zo* ]] || { echo "--out is inside a state folder" >&2; exit 2; }

mkdir -p "$out/home" "$out/ZeroCode.app/Contents/MacOS"
cp "$binary" "$out/ZeroCode.app/Contents/MacOS/zerocode-shell"
# A copy, not a link: the helpers find the framework by a path relative to
# themselves, and through a link that path leads back into the installed app.
cp -R "$INSTALLED_FRAMEWORKS" "$out/ZeroCode.app/Contents/Frameworks"
shim="$out/ZeroCode.app/Contents/MacOS/zerocode-shell"

home="$out/home"
data="$home/Library/Application Support/$APP_ID"
repo="$home/work/project"
mkdir -p "$data" "$repo"
git -C "$repo" init -q
git -C "$repo" -c user.name=probe -c user.email=probe@example.invalid commit -q --allow-empty -m init
for name in a b; do
  git -C "$repo" worktree add -q "$home/work/project-$name" -b "probe-$name"
done
# The conversations the layouts wake run these stand-ins, not the person's real
# agents: each is a process that stays up, which is what a woken agent is to
# the window. They sit in the synthetic home, found through its own PATH.
mkdir -p "$home/.local/bin"
for agent in claude codex zo; do
  printf '#!/bin/sh\nexec sleep 86400\n' > "$home/.local/bin/$agent"
  chmod +x "$home/.local/bin/$agent"
done
printf 'export PATH="$HOME/.local/bin:$PATH"\n' > "$home/.zshenv"
canon() { (cd "$1" && pwd -P); }
worktrees="$(canon "$repo"):$(canon "$home/work/project-a"):$(canon "$home/work/project-b")"
printf '%s\n' "$worktrees" > "$out/worktrees.txt"

run_once() {
  local at="$1" prefix=()
  # Run 1 is the cutover boot (the legacy ledger becomes the store); later
  # runs start on the store the run before left, which is the ordinary start.
  rm -f "$data/$TIMELINE_FILE"
  if [[ "$at" = 1 ]]; then
    rm -rf "$data"; mkdir -p "$data"
    cp "$profile/pane-layouts.json" "$data/pane-layouts.json"
    cp "$profile/orchestration.json" "$data/orchestration.json"
    # Browser tabs to put back, so the Chromium side has its restore to do. The
    # address is a closed local port: a tab opens and fails fast, no network.
    python3 - "$data/preferences.json" "$BROWSER_TABS" <<'PY'
import json, sys
path, count = sys.argv[1], int(sys.argv[2])
tabs = [{"url": f"http://127.0.0.1:9/probe-{n}"} for n in range(count)]
json.dump({"_meta": {"format": 1, "revision": 1},
           "data": {"browser": {"restore_tabs": True, "open_tabs": tabs}}},
          open(path, "w"))
PY
    # Re-key the layouts onto this run's real checkouts.
    python3 - "$data/pane-layouts.json" "$worktrees" <<'PY'
import json, sys
path, named = sys.argv[1], sys.argv[2].split(":")
held = json.load(open(path))
tabs = [tab for rows in held.values() for tab in rows]
rekeyed = {}
for index, tab in enumerate(tabs):
    rekeyed.setdefault(named[index % len(named)], []).append(tab)
json.dump(rekeyed, open(path, "w"))
PY
  fi
  $low && prefix=(taskpolicy -b)
  ( cd "$repo" && exec env HOME="$home" ZEROCODE_BYPASS_SINGLE_INSTANCE_LOCK=1 \
      ${prefix[@]+"${prefix[@]}"} "$shim" >"$out/run-$at.stdout" 2>"$out/run-$at.stderr" ) &
  local pid=$!
  local waited=0
  until python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))['complete'] else 1)" \
      "$data/$TIMELINE_FILE" 2>/dev/null; do
    sleep "$POLL_INTERVAL"
    waited=$(python3 -c "print($waited + $POLL_INTERVAL)")
    if ! kill -0 "$pid" 2>/dev/null; then echo "run $at: the window exited early" >&2; break; fi
    if python3 -c "import sys; sys.exit(0 if $waited > $START_TIMEOUT else 1)"; then
      echo "run $at: still not complete after ${START_TIMEOUT}s" >&2; break
    fi
  done
  cp "$data/$TIMELINE_FILE" "$out/timeline-$at.json" 2>/dev/null || echo '{}' > "$out/timeline-$at.json"
  # Our own child only: the pid this script started.
  # SIGKILL: the window answers SIGTERM with its goodbye walk, and this is a
  # throwaway profile whose end state nobody wants.
  kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  cat "$out/timeline-$at.json"; echo
}

for at in $(seq 1 "$runs"); do run_once "$at"; done

# The bundle shim holds a copy of the Chromium framework (hundreds of MB), and
# the synthetic home grows a profile of its own: neither is wanted afterwards,
# only the timelines are. `:?` stops the removal if --out ever came up empty.
rm -rf "${out:?}/ZeroCode.app" "${out:?}/home"
