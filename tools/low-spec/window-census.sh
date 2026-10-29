#!/usr/bin/env bash
# The window's whole cost on a quiet machine, normal and low-spec, in one
# repeatable plan (t-82914) — the same commands give a fix its before and after.
#
#   window-census.sh <stage> --data <folder> [options]
#
# Stages, in the order the plan takes them:
#   prep      the shipped UI unpacked (--ref, default the newest v* tag) with the
#             crate files the harness reads beside it (<ui>/../crates/...), and
#             the synthetic boot profile written by --test-binary (the
#             zerocode-shell test executable; nothing is built here)
#   boot      the installed binary started from the profile in a home of its own,
#             normal and low (taskpolicy -b), --boot-runs each; run 1 is the
#             cutover boot and is kept apart
#   steady    the same window held with 1, 5 and 10 panes and with 3 browser tabs:
#             after --settle-s, one memory sample (window.py) and one idle stretch
#             (idle.py), --runs each; panes-10 again under taskpolicy -b
#   idle-30m  panes-10 held for thirty minutes untouched: idle.py each minute,
#             window.py beside it
#   dense     panes-10 whose agents run --dense-cmd (a producer that ends by
#             itself): the window's CPU while the output flows, normal and low
#   harness   the page harness through tools/low-spec/profile.mjs, each level
#             (normal, low, cpu4x) with the probes that level can judge
#   person    the person's own window, read only: idle.py for ten minutes now,
#             and window.py for --follow-minutes in the background
#
# --levels normal,low,cpu4x and --scenes panes-10-normal,... pick part of a
# stage, so the plan can run in two sittings. --ui <name> unpacks and serves a
# second release's page for an after — prep --ref v1.1.105 --ui v1.1.105 puts it
# in <data>/v1.1.105/ui with its own crate files, and the harness writes
# <data>/harness-v1.1.105.
#
# What it never does: type into, signal or read the screen of the person's
# window (the person stage reads counters and region sizes of its processes);
# build anything; start more than one measured thing at a time. Every stage
# refuses to start while the one-minute load is over --max-load or a cargo
# process runs, and writes the conditions of each run beside its numbers.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
INSTALLED="/Applications/ZeroCode.app/Contents/MacOS/zerocode-shell"
# The test that writes the synthetic profile, and the checkouts it is told to
# name: measure.sh re-keys the layouts onto its own checkouts, so these are labels.
PROFILE_TEST="boot_timeline::boot_profile::writes_the_synthetic_boot_profile"
PROFILE_WORKTREES="/synthetic/one:/synthetic/two:/synthetic/three"

stage="${1:-}"; shift || true
data="" ref="" test_binary="" runs=5 boot_runs=6 settle_s=60 idle_s=45 max_load=6
dense_cmd="" follow_minutes=120 probes_normal="" probes_low="" probes_cpu4x=""
levels="" scenes="" start_timeout=90 ui_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --data) data="$2"; shift 2 ;;
    --ref) ref="$2"; shift 2 ;;
    --test-binary) test_binary="$2"; shift 2 ;;
    --runs) runs="$2"; shift 2 ;;
    --boot-runs) boot_runs="$2"; shift 2 ;;
    --settle-s) settle_s="$2"; shift 2 ;;
    --idle-s) idle_s="$2"; shift 2 ;;
    --max-load) max_load="$2"; shift 2 ;;
    --dense-cmd) dense_cmd="$2"; shift 2 ;;
    --follow-minutes) follow_minutes="$2"; shift 2 ;;
    --probes-normal) probes_normal="$2"; shift 2 ;;
    --probes-low) probes_low="$2"; shift 2 ;;
    --probes-cpu4x) probes_cpu4x="$2"; shift 2 ;;
    --levels) levels="$2"; shift 2 ;;
    --scenes) scenes="$2"; shift 2 ;;
    --start-timeout) start_timeout="$2"; shift 2 ;;
    --ui) ui_dir="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ "$data" = /* ]] || { echo "--data <absolute folder> is required" >&2; exit 2; }
mkdir -p "$data"
profile="$data/profile"
ui="$data/ui" harness="$data/harness"
[[ -z "$ui_dir" ]] || { ui="$data/$ui_dir/ui"; harness="$data/harness-$ui_dir"; }
# What window-boot.mjs reads from <ui>/.. — outside ui/, so unpacked beside it, one set per release.
HARNESS_READS=(crates/zerocode-shell/src/terminal_themes.json crates/zerocode-core/src/agent.rs
  crates/zerocode-shell/src/explorer_policy.json)

# The page probes each level can judge: memory is read on normal only (a
# scheduling policy does not change what a scene holds), frame rows on normal
# and cpu4x (taskpolicy -b coalesces frames), idle CPU on normal and low (a CDP
# throttle stops the page rather than slowing it, so it shows no CPU).
probes_normal="${probes_normal:-calibrate,frame-supply,memory-webkit,idle-webkit,idle-chromium,pane-switch,conversation-perf,sidebar-team-perf,board-live-perf,coordinator-desk-perf,terminal-pacing,terminal-throughput}"
probes_low="${probes_low:-calibrate,frame-supply,idle-webkit,idle-chromium,conversation-perf,terminal-throughput}"
probes_cpu4x="${probes_cpu4x:-frame-supply,pane-switch,conversation-perf,sidebar-team-perf,board-live-perf,coordinator-desk-perf,terminal-pacing}"

# The steady scenes, as name:level:measure.sh options; --scenes picks some by name-level.
STEADY_SCENES=(
  "panes-1:normal:--panes 1 --browser-tabs 0"
  "panes-5:normal:--panes 5 --browser-tabs 0"
  "panes-10:normal:--panes 10 --browser-tabs 0"
  "panes-1-tabs-3:normal:--panes 1 --browser-tabs 3"
  "panes-10:low:--panes 10 --browser-tabs 0"
)

# Whether --levels (comma-separated; empty means every level) names this one.
wanted() { [[ -z "$1" || ",$1," == *",$2,"* ]]; }

quiet_or_stop() {
  local load cargo
  load=$(sysctl -n vm.loadavg | awk '{print $2}')
  cargo=$( (pgrep -x cargo || true) | wc -l | tr -d ' ')
  if awk -v l="$load" -v m="$max_load" 'BEGIN{exit !(l > m)}' || [[ "$cargo" != 0 ]]; then
    echo "not quiet: load $load (limit $max_load), cargo processes $cargo — the stage waits for a quiet machine" >&2
    exit 3
  fi
}

conditions() {
  python3 - "$1" <<'PY'
import json, os, re, subprocess, sys, time
batt = subprocess.run(["pmset", "-g", "batt"], capture_output=True, text=True).stdout
settings = subprocess.run(["pmset", "-g"], capture_output=True, text=True).stdout
source = re.search(r"from '([^']+)'", batt)
charge = re.search(r"(\d+)%", batt)
json.dump({"at": time.strftime("%FT%T%z"), "load1": os.getloadavg()[0], "cores": os.cpu_count(),
           "power": source.group(1) if source else None, "charge": charge.group(1) if charge else None,
           "low_power_mode": bool(re.search(r"^\s*(?:low)?powermode\s+1\b", settings, re.M)),
           "installed": subprocess.run(["defaults", "read", "/Applications/ZeroCode.app/Contents/Info.plist",
                                        "CFBundleShortVersionString"], capture_output=True, text=True).stdout.strip()},
          open(sys.argv[1], "w"), indent=1)
PY
}

measure() {  # measure <out> [measure.sh options...]
  local out="$1"; shift
  [[ -f "$profile/pane-layouts.json" ]] || { echo "no profile: run the prep stage first" >&2; exit 2; }
  # Started behind the person's work and checked to stay there (measure.sh
  # --launch background): a window behind others may not paint, so a start that
  # never reaches its first frame ends after --start-timeout rather than 300 s.
  bash "$ROOT/tools/boot-profile/measure.sh" --binary "$INSTALLED" --profile "$profile" --out "$out" \
    --launch background --start-timeout "$start_timeout" "$@"
}

# One steady scene: boot, settle, one memory sample and one idle stretch per run.
steady_scene() {  # steady_scene <name> <low|normal> [measure.sh options...]
  local name="$1" level="$2"; shift 2
  local out="$data/steady/$name-$level" low=()
  [[ "$level" = low ]] && low=(--low-spec)
  local census="python3 '$ROOT/tools/memory-census/window.py' --out \"\$OUT/memory-\$RUN_AT\" --pid \"\$WINDOW_PID\""
  census+=" --minutes 0 --vmmap-every 1 --heap-at '' --label '$name-$level'"
  census+=" && python3 '$ROOT/tools/memory-census/idle.py' --out \"\$OUT/idle-\$RUN_AT\" --pid \"\$WINDOW_PID\""
  census+=" --seconds $idle_s --repeat 1 --label '$name-$level'"
  quiet_or_stop
  conditions "$data/steady/$name-$level.conditions.json"
  # One more run than counted: run 1 is the cutover boot.
  measure "$out" ${low[@]+"${low[@]}"} --runs $((runs + 1)) --settle-s "$settle_s" --then "$census" "$@"
}

case "$stage" in
  prep)
    [[ -x "$test_binary" ]] || { echo "--test-binary <zerocode-shell test executable> is required" >&2; exit 2; }
    ref="${ref:-$(git -C "$ROOT" describe --tags --abbrev=0 --match 'v*')}"
    rm -rf "$ui" "$profile"; mkdir -p "$ui" "$profile"
    git -C "$ROOT" archive "$ref" ui | tar -x -C "$ui" --strip-components 1
    git -C "$ROOT" archive "$ref" "${HARNESS_READS[@]}" | tar -x -C "$(dirname "$ui")"
    echo "$ref $(git -C "$ROOT" rev-parse "$ref^{commit}")" > "$ui.ref"
    # Nothing of the caller's environment: the profile names only what this run gives it.
    env -i HOME="$data" PATH=/usr/bin:/bin ZEROCODE_DISABLE_CREDENTIAL_STORE=1 \
      ZEROCODE_BOOT_PROFILE_DIR="$profile" ZEROCODE_BOOT_PROFILE_WORKTREES="$PROFILE_WORKTREES" \
      "$test_binary" --ignored --exact "$PROFILE_TEST" --test-threads=1
    ls -l "$profile"
    ;;
  boot)
    for level in normal low; do
      wanted "$levels" "$level" || continue
      quiet_or_stop
      low=(); [[ "$level" = low ]] && low=(--low-spec)
      conditions "$data/boot-$level.conditions.json"
      measure "$data/boot-$level" ${low[@]+"${low[@]}"} --runs "$boot_runs"
    done
    ;;
  steady)
    for scene in "${STEADY_SCENES[@]}"; do
      IFS=: read -r name level options <<< "$scene"
      wanted "$scenes" "$name-$level" || continue
      # shellcheck disable=SC2086 # the options are words on purpose
      steady_scene "$name" "$level" $options
    done
    ;;
  idle-30m)
    for level in normal low; do
      wanted "$levels" "$level" || continue
      quiet_or_stop
      low=(); [[ "$level" = low ]] && low=(--low-spec)
      conditions "$data/idle-30m-$level.conditions.json"
      census="python3 '$ROOT/tools/memory-census/window.py' --out \"\$OUT/memory\" --pid \"\$WINDOW_PID\""
      census+=" --minutes 30 --interval-s 60 --vmmap-every 10 --heap-at '' --label idle-30m-$level >/dev/null &"
      census+=" python3 '$ROOT/tools/memory-census/idle.py' --out \"\$OUT/idle\" --pid \"\$WINDOW_PID\""
      census+=" --seconds 60 --repeat 30 --label idle-30m-$level; wait"
      measure "$data/idle-30m-$level" ${low[@]+"${low[@]}"} --runs 1 --panes 10 --browser-tabs 0 \
        --settle-s "$settle_s" --then "$census"
    done
    ;;
  dense)
    [[ -n "$dense_cmd" ]] || { echo "--dense-cmd <a producer that ends by itself> is required" >&2; exit 2; }
    for level in normal low; do
      wanted "$levels" "$level" || continue
      quiet_or_stop
      low=(); [[ "$level" = low ]] && low=(--low-spec)
      conditions "$data/dense-$level.conditions.json"
      census="python3 '$ROOT/tools/memory-census/idle.py' --out \"\$OUT/idle-\$RUN_AT\" --pid \"\$WINDOW_PID\""
      census+=" --seconds $idle_s --repeat 1 --label dense-$level"
      measure "$data/dense-$level" ${low[@]+"${low[@]}"} --runs $((runs + 1)) --panes 10 --browser-tabs 0 \
        --agent-cmd "$dense_cmd" --settle-s 5 --then "$census"
    done
    ;;
  harness)
    for read in "${HARNESS_READS[@]}"; do
      [[ -f "$(dirname "$ui")/$read" ]] || { echo "no UI snapshot at $ui: run the prep stage first" >&2; exit 2; }
    done
    for level in normal low cpu4x; do
      wanted "$levels" "$level" || continue
      quiet_or_stop
      case "$level" in normal) probes="$probes_normal" ;; low) probes="$probes_low" ;; cpu4x) probes="$probes_cpu4x" ;; esac
      conditions "$harness-$level.conditions.json"
      ZEROCODE_WINDOW_UI="$ui" node "$ROOT/tools/low-spec/profile.mjs" run --level "$level" --runs "$runs" \
        --probe "$probes" --out "$harness/$level"
    done
    done_levels=()
    for level in normal low cpu4x; do
      if [[ -f "$harness/$level/runs.json" ]]; then done_levels+=("$harness/$level"); fi
    done
    node "$ROOT/tools/low-spec/profile.mjs" report "${done_levels[@]}" > "$harness/table.md"
    ;;
  person)
    conditions "$data/person.conditions.json"
    # The window's own record of its last start: phase names and numbers only.
    timeline="$HOME/Library/Application Support/dev.zerocode.app/boot-timeline.json"
    version=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("app_version",""))' "$timeline")
    cp "$timeline" "$data/person-boot-timeline-v$version.json"
    pid=$(ps -axo pid=,args= | awk '$2 == "/Applications/ZeroCode.app/Contents/MacOS/zerocode-shell" {print $1}')
    [[ $(printf '%s\n' "$pid" | grep -c .) = 1 ]] || { echo "not exactly one installed window: $pid" >&2; exit 2; }
    nohup python3 "$ROOT/tools/memory-census/window.py" --out "$data/person/memory" --pid "$pid" \
      --minutes "$follow_minutes" --interval-s 60 --vmmap-every 15 --heap-at '' --label person \
      > "$data/person-memory.log" 2>&1 &
    echo "following window $pid for $follow_minutes minutes in the background (pid $!)"
    quiet_or_stop
    python3 "$ROOT/tools/memory-census/idle.py" --out "$data/person/idle" --pid "$pid" --seconds 60 --repeat 10 \
      --label person
    ;;
  *)
    echo "usage: window-census.sh prep|boot|steady|idle-30m|dense|harness|person --data <folder> [options]" >&2
    exit 2 ;;
esac
