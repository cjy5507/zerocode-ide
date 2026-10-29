#!/usr/bin/env bash
# Start ONE window from a synthetic profile and read back its boot timeline
# (t-20078). Everything it touches lives under --out: the window runs with
# HOME pointed there, so the person's state folders (~/Library/Application
# Support/dev.zerocode.app, ~/.zo) are on no road, and the window it starts is
# its own process — it never signals, types into or reads any other window.
#
#   measure.sh --binary <zerocode-shell> --profile <dir from the profile test>
#              --out <fresh dir> [--low-spec] [--runs N]
#              [--panes N] [--browser-tabs N] [--agent-cmd <command>]
#              [--settle-s S] [--then <command>]
#
# --profile is the folder `writes_the_synthetic_boot_profile` wrote
# (pane-layouts.json + orchestration.json). --low-spec runs the window under
# `taskpolicy -b`, the efficiency-core profile: wall-clock startup is valid
# there, frame-rate numbers are not (the throttle holds animation at ~10 Hz).
#
# The same started window also stands for the steady-state census (t-82914):
# --panes keeps the first N terminals of the profile (a tab of two, the last
# one alone when N is odd), --browser-tabs sets how many tabs it restores,
# --agent-cmd replaces the stand-in agents' `sleep` with a command of their own
# (a producer of dense output, say). Once the timeline is complete the window
# is held --settle-s seconds and then --then runs, with WINDOW_PID (the window
# this script started), RUN_AT (the run's number) and OUT in its environment;
# the window is ended when --then returns. Run 1 alone is the cutover boot:
# with --warm-data every run is an ordinary warm start.
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

binary="" profile="" out="" low=false runs=1 home_dir="" save_data="" warm_data=""
panes="" browser_tabs=$BROWSER_TABS agent_cmd="sleep 86400" settle_s=0 then_cmd=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary) binary="$2"; shift 2 ;;
    --profile) profile="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --low-spec) low=true; shift ;;
    --runs) runs="$2"; shift 2 ;;
    # A fixed home path, so the checkouts the layouts name keep their paths
    # from one invocation to the next.
    --home) home_dir="$2"; shift 2 ;;
    # After run 1 (the cutover boot) keep the data folder here...
    --save-data) save_data="$2"; shift 2 ;;
    # ...and start from it: every run is then an ordinary warm start, and one
    # launch each. A launch of a copied bundle leaves macOS a code-sign clone
    # behind (about 0.2 GiB), so launches are counted, not free.
    --warm-data) warm_data="$2"; shift 2 ;;
    --panes) panes="$2"; shift 2 ;;
    --browser-tabs) browser_tabs="$2"; shift 2 ;;
    --agent-cmd) agent_cmd="$2"; shift 2 ;;
    --settle-s) settle_s="$2"; shift 2 ;;
    --then) then_cmd="$2"; shift 2 ;;
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

home="${home_dir:-$out/home}"
[[ "$home" = /* && "$home" != "$HOME" && "$home" != "$HOME"/Library* && "$home" != "$HOME"/.zo* ]] \
  || { echo "--home must be an absolute path outside the person's state folders" >&2; exit 2; }
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
  printf '#!/bin/sh\nexec %s\n' "$agent_cmd" > "$home/.local/bin/$agent"
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
  if [[ -n "$warm_data" ]]; then
    rm -rf "$data"; mkdir -p "$(dirname "$data")"; cp -R "$warm_data" "$data"
  elif [[ "$at" = 1 ]]; then
    rm -rf "$data"; mkdir -p "$data"
    cp "$profile/pane-layouts.json" "$data/pane-layouts.json"
    cp "$profile/orchestration.json" "$data/orchestration.json"
    # Browser tabs to put back, so the Chromium side has its restore to do. The
    # address is a closed local port: a tab opens and fails fast, no network.
    python3 - "$data/preferences.json" "$browser_tabs" <<'PY'
import json, sys
path, count = sys.argv[1], int(sys.argv[2])
tabs = [{"url": f"http://127.0.0.1:9/probe-{n}"} for n in range(count)]
json.dump({"_meta": {"format": 1, "revision": 1},
           "data": {"browser": {"restore_tabs": True, "open_tabs": tabs}}},
          open(path, "w"))
PY
    # Re-key the layouts onto this run's real checkouts, keeping the first
    # --panes terminals. Leaves are positional: a tab cut to one leaf keeps the
    # maps' entries for leaf 0 only.
    python3 - "$data/pane-layouts.json" "$worktrees" "$panes" <<'PY'
import json, sys
path, named, keep = sys.argv[1], sys.argv[2].split(":"), sys.argv[3]
held = json.load(open(path))
tabs = [tab for rows in held.values() for tab in rows]
if keep:
    left, kept = int(keep), []
    for tab in tabs:
        if left <= 0:
            break
        if left == 1 and tab["root"]["type"] == "split":
            tab["root"] = {"type": "leaf"}
            for key in ("buffers", "agents", "running", "titles", "names", "terms"):
                if isinstance(tab.get(key), dict):
                    tab[key] = {leaf: one for leaf, one in tab[key].items() if leaf == "0"}
        left -= 2 if tab["root"]["type"] == "split" else 1
        kept.append(tab)
    tabs = kept
rekeyed = {}
for index, tab in enumerate(tabs):
    rekeyed.setdefault(named[index % len(named)], []).append(tab)
json.dump(rekeyed, open(path, "w"))
PY
  fi
  $low && prefix=(taskpolicy -b)
  # The environment a Dock launch gets, and nothing of the caller's: a pane of
  # the person's window carries its tokens, hook endpoint and router keys, and a
  # measured window must not inherit them. No credential store either — the
  # measured window never reads or writes the keychain (t-82914).
  ( cd "$repo" && exec env -i HOME="$home" USER="$USER" LOGNAME="${LOGNAME:-$USER}" SHELL=/bin/zsh \
      TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-en_US.UTF-8}" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
      ZEROCODE_BYPASS_SINGLE_INSTANCE_LOCK=1 ZEROCODE_DISABLE_CREDENTIAL_STORE=1 \
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
  if kill -0 "$pid" 2>/dev/null && [[ "$settle_s" != 0 || -n "$then_cmd" ]]; then
    sleep "$settle_s"
    if [[ -n "$then_cmd" ]]; then
      WINDOW_PID="$pid" RUN_AT="$at" OUT="$out" bash -c "$then_cmd" > "$out/then-$at.log" 2>&1 \
        || echo "run $at: --then exited $?" >&2
    fi
  fi
  # Our own child only: the pid this script started.
  # SIGKILL: the window answers SIGTERM with its goodbye walk, and this is a
  # throwaway profile whose end state nobody wants.
  kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  if [[ -n "$save_data" && "$at" = 1 ]]; then
    rm -rf "$save_data"; cp -R "$data" "$save_data"
    rm -f "$save_data/$TIMELINE_FILE"
  fi
  cat "$out/timeline-$at.json"; echo
}

for at in $(seq 1 "$runs"); do run_once "$at"; done

# The bundle shim holds a copy of the Chromium framework (hundreds of MB), and
# the synthetic home grows a profile of its own: neither is wanted afterwards,
# only the timelines are. `:?` stops the removal if --out ever came up empty.
rm -rf "${out:?}/ZeroCode.app" "${home:?}"
