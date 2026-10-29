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
#              [--launch direct|background] [--start-timeout S]
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
# --launch background starts a whole copy of the installed app through
# LaunchServices without bringing it forward (`open -g -n`), the way the Dock
# would but behind the person's work: tao asks AppKit to activate a window it
# starts (`activateIgnoringOtherApps`), and a window started straight from a
# pane may take the person's keyboard. `front_guard.py` checks the window
# right after the start and every ten seconds while it is held; if it took the
# front or stands above the front app's windows, it is ended and the script
# stops with exit 4. Its cwd is then `/`, as a Dock start's is, and
# --low-spec applies `taskpolicy -b -p` to it the moment it is found.
#
# The Chromium helpers are not rebuilt: a bundle shim holds a copy of the
# installed app's Frameworks (read, never written), which is all the window
# needs to find them.
set -euo pipefail

# How long one start may take before it is called stuck, and how often the
# timeline file is looked at. Seconds; the slow start this was written for took
# about a hundred, so three times that is "stuck".
START_TIMEOUT=300
# How often the background window is checked for being in the person's way
# while it is held, seconds.
GUARD_EVERY_S=10
INSTALLED_APP="/Applications/ZeroCode.app"
GUARD="$(cd "$(dirname "$0")" && pwd)/front_guard.py"
POLL_INTERVAL=0.25
# Browser tabs stored for the restore: the person's window held a few.
BROWSER_TABS=3
INSTALLED_FRAMEWORKS="/Applications/ZeroCode.app/Contents/Frameworks"
TIMELINE_FILE="boot-timeline.json"
APP_ID="dev.zerocode.app"

binary="" profile="" out="" low=false runs=1 home_dir="" save_data="" warm_data=""
panes="" browser_tabs=$BROWSER_TABS agent_cmd="sleep 86400" settle_s=0 then_cmd="" launch=direct
start_timeout=$START_TIMEOUT
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
    --launch) launch="$2"; shift 2 ;;
    --start-timeout) start_timeout="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ -x "$binary" && -d "$profile" && "$out" = /* && ! -e "$out" ]] \
  || { echo "need --binary (executable), --profile (dir), --out (absolute, not existing)" >&2; exit 2; }
[[ -d "$INSTALLED_FRAMEWORKS" ]] || { echo "no installed Frameworks to borrow" >&2; exit 2; }
# The person's real home is never a target.
[[ "$out" != "$HOME"/Library* && "$out" != "$HOME"/.zo* ]] || { echo "--out is inside a state folder" >&2; exit 2; }

[[ "$launch" = direct || "$launch" = background ]] || { echo "--launch direct|background" >&2; exit 2; }
if [[ "$launch" = background ]]; then
  # LaunchServices starts a bundle: the whole installed app, its own binary
  # replaced only when --binary is another one.
  mkdir -p "$out/home"
  cp -R "$INSTALLED_APP" "$out/ZeroCode.app"
  cmp -s "$binary" "$out/ZeroCode.app/Contents/MacOS/zerocode-shell" \
    || cp "$binary" "$out/ZeroCode.app/Contents/MacOS/zerocode-shell"
else
  mkdir -p "$out/home" "$out/ZeroCode.app/Contents/MacOS"
  cp "$binary" "$out/ZeroCode.app/Contents/MacOS/zerocode-shell"
  # A copy, not a link: the helpers find the framework by a path relative to
  # themselves, and through a link that path leads back into the installed app.
  cp -R "$INSTALLED_FRAMEWORKS" "$out/ZeroCode.app/Contents/Frameworks"
fi
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
  # measured window must not inherit them (`open` hands its own environment on,
  # so it is cleared there too). No credential store either — the measured
  # window never reads or writes the keychain (t-82914).
  local clean=(env -i HOME="$home" USER="$USER" LOGNAME="${LOGNAME:-$USER}" SHELL=/bin/zsh
    TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-en_US.UTF-8}" PATH=/usr/bin:/bin:/usr/sbin:/sbin)
  local pid=""
  if [[ "$launch" = background ]]; then
    "${clean[@]}" /usr/bin/open -g -n "$out/ZeroCode.app" \
      --env ZEROCODE_BYPASS_SINGLE_INSTANCE_LOCK=1 --env ZEROCODE_DISABLE_CREDENTIAL_STORE=1 \
      --stdout "$out/run-$at.stdout" --stderr "$out/run-$at.stderr"
    # The window is LaunchServices' child, not ours: found by its own binary's path.
    for _ in $(seq 1 100); do
      pid=$(ps -axo pid=,args= | awk -v shim="$shim" '$2 == shim {print $1; exit}')
      [[ -n "$pid" ]] && break
      sleep 0.1
    done
    [[ -n "$pid" ]] || { echo "run $at: the window did not start" >&2; exit 4; }
    if $low; then taskpolicy -b -p "$pid"; fi
  else
    ( cd "$repo" && exec "${clean[@]}" ZEROCODE_BYPASS_SINGLE_INSTANCE_LOCK=1 ZEROCODE_DISABLE_CREDENTIAL_STORE=1 \
        ${prefix[@]+"${prefix[@]}"} "$shim" >"$out/run-$at.stdout" 2>"$out/run-$at.stderr" ) &
    pid=$!
  fi
  # A background window that took the front or stands over the person's work
  # is ended at once, and nothing more is started.
  out_of_the_way() {
    [[ "$launch" = background ]] || return 0
    python3 "$GUARD" check "$pid" >> "$out/guard-$at.txt" && return 0
    kill -KILL "$pid" 2>/dev/null || true
    echo "run $at: stopped — $(tail -1 "$out/guard-$at.txt")" >&2
    rm -rf "${out:?}/ZeroCode.app" "${home:?}"
    exit 4
  }
  local waited=0 polls=0
  until python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))['complete'] else 1)" \
      "$data/$TIMELINE_FILE" 2>/dev/null; do
    sleep "$POLL_INTERVAL"
    polls=$((polls + 1))
    # Once a second while it comes up: activation and the first window come early.
    (( polls % 4 == 1 )) && out_of_the_way
    waited=$(python3 -c "print($waited + $POLL_INTERVAL)")
    if ! kill -0 "$pid" 2>/dev/null; then echo "run $at: the window exited early" >&2; break; fi
    if python3 -c "import sys; sys.exit(0 if $waited > $start_timeout else 1)"; then
      echo "run $at: still not complete after ${start_timeout}s" >&2; break
    fi
  done
  out_of_the_way
  cp "$data/$TIMELINE_FILE" "$out/timeline-$at.json" 2>/dev/null || echo '{}' > "$out/timeline-$at.json"
  if kill -0 "$pid" 2>/dev/null && [[ "$settle_s" != 0 || -n "$then_cmd" ]]; then
    local watch=""
    if [[ "$launch" = background ]]; then
      ( while kill -0 "$pid" 2>/dev/null; do
          sleep "$GUARD_EVERY_S"
          python3 "$GUARD" check "$pid" >> "$out/guard-$at.txt" || { kill -KILL "$pid" 2>/dev/null; break; }
        done ) &
      watch=$!
    fi
    sleep "$settle_s"
    if [[ -n "$then_cmd" ]]; then
      WINDOW_PID="$pid" RUN_AT="$at" OUT="$out" bash -c "$then_cmd" > "$out/then-$at.log" 2>&1 \
        || echo "run $at: --then exited $?" >&2
    fi
    if [[ -n "$watch" ]]; then kill "$watch" 2>/dev/null || true; wait "$watch" 2>/dev/null || true; fi
    if [[ "$launch" = background ]] && ! kill -0 "$pid" 2>/dev/null; then
      echo "run $at: stopped — $(tail -1 "$out/guard-$at.txt")" >&2
      rm -rf "${out:?}/ZeroCode.app" "${home:?}"
      exit 4
    fi
  fi
  # Only the window this script started (its child, or the one found by this
  # run's own bundle path). SIGKILL: the window answers SIGTERM with its
  # goodbye walk, and this is a throwaway profile whose end state nobody wants.
  kill -KILL "$pid" 2>/dev/null || true
  if [[ "$launch" = background ]]; then
    while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
  else
    wait "$pid" 2>/dev/null || true
  fi
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
