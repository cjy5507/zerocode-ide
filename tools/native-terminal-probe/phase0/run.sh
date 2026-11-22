#!/usr/bin/env bash
# Phase 0 of the native terminal comparison, end to end. Every stage is meant to
# run as a job on the machine's build line (zo line), from this checkout:
#
#   zo line add --owner <owner> --cap 40m -- bash tools/native-terminal-probe/phase0/run.sh record  OUT [ROWS COLS]
#   zo line add --owner <owner> --cap 40m -- bash tools/native-terminal-probe/phase0/run.sh build
#   zo line add --owner <owner> --cap 40m -- bash tools/native-terminal-probe/phase0/run.sh capture REC OUT
#   zo line add --owner <owner> --quiet --cap 60m -- bash tools/native-terminal-probe/phase0/run.sh timing REC OUT normal 5
#   zo line add --owner <owner> --quiet --cap 60m -- bash tools/native-terminal-probe/phase0/run.sh timing REC OUT low 5
#   zo line add --owner <owner> --quiet --cap 60m -- bash tools/native-terminal-probe/phase0/run.sh timing REC OUT cpu4x 5
#   bash tools/native-terminal-probe/phase0/run.sh report OUT
#
# REC is a folder `record` wrote; OUT is any folder outside the checkout.
# `record` writes the cases at 50 rows × 170 columns unless told otherwise: at
# the page's cell (Menlo 14 px, 8.43 × 16.09 points) that is 1433 × 805 points,
# which a GPUI window holds whole on a 14-inch MacBook's built-in display (its
# visible frame is 1512 × 888 points with the Dock below, about 1450 wide with
# it at a side). macOS keeps a titled window inside that frame, and both GPUI
# passes refuse a window that cannot hold the grid; the window harness's
# 60 × 220 pane needs 1854 × 966 points.
# Levels follow tools/low-spec/profile.mjs: normal; low = `taskpolicy -b` (the
# efficiency cores at background QoS) on both renderers' processes; cpu4x = the
# page's CPU slowed 4x over CDP, which exists for the DOM only.
#
# `timing` runs under the conditions the coordinators set (m-118419, m-118430).
# GPUI draws no frames for a window macOS reports not visible, so its pass opens
# a pop-up window (PHASE0_WINDOW_KIND, default popup): above ordinary windows,
# on every Space, never taking the keyboard, titled as a measurement window that
# closes by itself. It covers most of a 14-inch screen for about half a minute.
# A round starts only when the keyboard, mouse and trackpad have rested
# PHASE0_GATE_IDLE_S (600) seconds, the one-minute load is under
# PHASE0_GATE_LOAD (12), macOS reports the thermal state nominal, the display is
# awake and PHASE0_GATE_FREE_GIB (16) GiB are free. It waits for that up to
# PHASE0_GATE_WAIT_S (600) seconds a round and PHASE0_GATE_BUDGET_S (1800) in
# all; a round whose wait runs out is skipped and logged. An input event during
# a pass breaks the round: the GPUI pass ends at once (its window closes), the
# DOM pass is judged at its end; the samples stay and the next attempt waits for
# the gate again. Rounds are tried until ROUNDS passes of each renderer count
# under rounds.mjs, or PHASE0_ATTEMPTS (2 × ROUNDS + 2) attempts are used; the
# stage exits 3 when they do not ("no ranking"). The display stays awake while
# the stage runs (`caffeinate -d`, tied to this shell). PHASE0_CONTROL_FRAMES
# (default 120) sets how many control frames follow each case: frames with
# nothing fed, and for GPUI frames redrawing the unchanged screen.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PROBE=$(cd "$HERE/.." && pwd)
ROOT=$(cd "$PROBE/../.." && pwd)
# The probe's own target folder, never a build line's slot (the line names one in
# CARGO_TARGET_DIR): the replay one job builds is the one the next job runs.
# PHASE0_TARGET_DIR names another.
export CARGO_TARGET_DIR="${PHASE0_TARGET_DIR:-$PROBE/target}"
GPUI_BIN="$CARGO_TARGET_DIR/release/phase0-gpui"
RECORD_BIN="$CARGO_TARGET_DIR/release/phase0-record"

say() { echo "== $*"; }
load() { uptime | sed 's/.*load averages*: *//'; }
idle_s() { ioreg -c IOHIDSystem | awk '/HIDIdleTime/ {print int($NF / 1000000000); exit}'; }
now_ms() { node -e 'console.log(Date.now())'; }
# macOS's thermal state: 0 nominal, 1 fair, 2 serious, 3 critical (9: could not be read).
thermal_now() { osascript -l JavaScript -e 'ObjC.import("Foundation"); $.NSProcessInfo.processInfo.thermalState' 2>/dev/null || echo 9; }
# The gate's facts as one line: HID idle (the smaller of IOHIDSystem's and the
# HID event source's count), one-minute load, thermal state, display asleep (0/1), free GiB.
facts_now() {
  local idle cg asleep
  idle=$(idle_s)
  cg=$(osascript -l JavaScript -e 'ObjC.import("CoreGraphics"); Math.floor($.CGEventSourceSecondsSinceLastEventType(1, 0xFFFFFFFF))' 2>/dev/null || echo 0)
  [ "${cg:-0}" -lt "${idle:-0}" ] && idle=$cg
  asleep=$(osascript -l JavaScript -e 'ObjC.import("CoreGraphics"); $.CGDisplayIsAsleep($.CGMainDisplayID())' 2>/dev/null || echo 1)
  echo "idle=${idle:-0} load1=$(sysctl -n vm.loadavg | awk '{print $2}') thermal=$(thermal_now) displayAsleep=$asleep freeGiB=$(df -k / | awk 'NR==2 {print int($4 / 1048576)}')"
}
gate_ok() {
  # shellcheck disable=SC2046 # five numbers, split on purpose
  set -- $(echo "$1" | tr '=' ' ' | awk '{print $2, $4, $6, $8, $10}')
  [ "${1:-0}" -ge "$gate_idle" ] || return 1
  awk -v l="${2:-99}" -v m="$gate_load" 'BEGIN { exit !(l < m) }' || return 1
  [ "${3:-9}" = 0 ] && [ "${4:-1}" = 0 ] && [ "${5:-0}" -ge "$gate_free" ]
}
# Waits for the gate up to gate_wait seconds (and what is left of gate_budget); gate_facts holds what it saw last.
wait_gate() {
  local spent=0
  while :; do
    gate_facts=$(facts_now)
    gate_ok "$gate_facts" && return 0
    { [ "$spent" -ge "$gate_wait" ] || [ "$waited" -ge "$gate_budget" ]; } && return 1
    sleep 15
    spent=$((spent + 15))
    waited=$((waited + 15))
  done
}

stage=${1:-}
shift || true
case "$stage" in
  record)
    out=${1:?record OUT [ROWS COLS]}; rows=${2:-50}; cols=${3:-170}
    say "recorder build (product toolchain; it needs no GPUI)"
    (cd "$PROBE" && cargo +1.94.0 build -p zerocode-phase0-record --release --locked) || exit 1
    stamp=$(date +%Y%m%d-%H%M%S)
    say "cases at $rows rows × $cols columns"
    for copy in a b; do "$RECORD_BIN" cases --out "$out/recordings-$stamp-$copy" --rows "$rows" --cols "$cols" || exit 1; done
    for copy in a b; do (cd "$out/recordings-$stamp-$copy" && shasum -a 256 -- * | sort -k2) > "$out/recordings-$stamp-$copy.sha256"; done
    if cmp -s "$out/recordings-$stamp-a.sha256" "$out/recordings-$stamp-b.sha256"; then
      say "the two recordings are byte-identical: $out/recordings-$stamp-a"
    else
      say "the two recordings differ"; diff "$out/recordings-$stamp-a.sha256" "$out/recordings-$stamp-b.sha256"; exit 1
    fi
    ;;
  build)
    say "GPUI replay, release, the features it measures with"
    (cd "$PROBE" && /usr/bin/time -l cargo +1.98.1 build --release --locked --features capture,timing --bin phase0-gpui) || exit 1
    ;;
  capture)
    rec=${1:?capture REC OUT}; out=${2:?capture REC OUT}
    cd "$ROOT" || exit 1
    node "$HERE/dom-replay.mjs" capture --recordings "$rec" --out "$out/dom" || exit 1
    "$GPUI_BIN" capture --recordings "$rec" --theme "$out/dom/dom-theme.json" --dom-fonts "$out/dom/dom-fonts.json" --out "$out/gpui" || exit 1
    node "$HERE/compare.mjs" --recordings "$rec" --dom "$out/dom" --gpui "$out/gpui" --out "$out/compare"
    ;;
  timing)
    rec=${1:?timing REC OUT LEVEL ROUNDS [CASES]}; out=${2:?}; level=${3:?}; rounds=${4:-5}; cases=${5:-}
    cd "$ROOT" || exit 1
    wrap=()
    [ "$level" = low ] && wrap=(taskpolicy -b)
    env_level=()
    [ "$level" = cpu4x ] && env_level=(env WINDOW_CPU_THROTTLE=4)
    theme="$out/theme/dom-theme.json"
    fonts="$out/theme/dom-fonts.json"
    if [ ! -f "$theme" ]; then
      # The page's resolved look and the font it drew ASCII in, for the GPUI grid: once, outside any timed run.
      node "$HERE/dom-replay.mjs" capture --recordings "$rec" --out "$out/theme" --cases sparse-log >/dev/null || exit 1
    fi
    case_args=()
    [ -n "$cases" ] && case_args=(--cases "$cases")
    if [ -n "$cases" ]; then
      ncases=$(echo "$cases" | tr ',' '\n' | grep -c .)
    else
      ncases=$(find "$rec" -maxdepth 1 -name '*.meta.json' | wc -l | tr -d ' ')
    fi
    if [ "$level" != cpu4x ] && [ ! -x "$GPUI_BIN" ]; then say "no GPUI replay at $GPUI_BIN: run.sh build first"; exit 1; fi
    gate_idle=${PHASE0_GATE_IDLE_S:-600}
    gate_load=${PHASE0_GATE_LOAD:-12}
    gate_free=${PHASE0_GATE_FREE_GIB:-16}
    gate_wait=${PHASE0_GATE_WAIT_S:-600}
    gate_budget=${PHASE0_GATE_BUDGET_S:-1800}
    attempts_max=${PHASE0_ATTEMPTS:-$((rounds * 2 + 2))}
    window_kind=${PHASE0_WINDOW_KIND:-popup}
    need_gpui=$rounds
    [ "$level" = cpu4x ] && need_gpui=0
    say "timing $level: $rounds counted passes of each renderer wanted, $ncases cases each · gate: HID idle ≥ $gate_idle s, load < $gate_load, thermal nominal, display awake, ≥ $gate_free GiB free; waits ≤ $gate_wait s a round, ≤ $gate_budget s in all; ≤ $attempts_max attempts · GPUI window: $window_kind"
    # The display stays awake while this stage runs; the assertion is tied to this shell and ends with it.
    caffeinate -d -w $$ &
    awake=$!
    trap 'kill "$awake" 2>/dev/null' EXIT
    # pass RENDERER DIR: 0 when the round can go on, 1 when it is broken.
    pass() {
      local renderer=$1 dir=$2 since end code idle_end thermal_before thermal_after
      thermal_before=$(thermal_now)
      since=$(now_ms)
      if [ "$renderer" = dom ]; then
        say "dom $level $(basename "$dir") · load $(load) · idle $(idle_s) s · thermal $thermal_before"
        ${env_level[@]+"${env_level[@]}"} ${wrap[@]+"${wrap[@]}"} node "$HERE/dom-replay.mjs" timing --recordings "$rec" --out "$dir/dom" --level "$level" --repeat 1 --idle "${PHASE0_CONTROL_FRAMES:-120}" ${case_args[@]+"${case_args[@]}"}
        code=$?
      else
        say "gpui $level $(basename "$dir") · load $(load) · idle $(idle_s) s · thermal $thermal_before · window $window_kind"
        # The replay ends itself on input after $since, on a hidden window, or when this shell is gone.
        ${wrap[@]+"${wrap[@]}"} /usr/bin/time -l "$GPUI_BIN" timing --recordings "$rec" --theme "$theme" --dom-fonts "$fonts" --out "$dir/gpui" --level "$level" --repeat 1 --idle "${PHASE0_CONTROL_FRAMES:-120}" --redraw "${PHASE0_CONTROL_FRAMES:-120}" --window-kind "$window_kind" --since-ms "$since" --runner-pid $$ ${case_args[@]+"${case_args[@]}"}
        code=$?
      fi
      end=$(now_ms)
      idle_end=$(idle_s)
      thermal_after=$(thermal_now)
      node "$HERE/rounds.mjs" pass "$dir/round.json" "$renderer" "$code" "$since" "$end" "$idle_end" "$thermal_before" "$thermal_after" || return 1
      say "$renderer $level $(basename "$dir") rc=$code · $(( (end - since) / 1000 )) s · HID idle at the end $idle_end s · thermal $thermal_before → $thermal_after"
      [ "$code" -eq 0 ] || return 1
      # An input event during the pass: the HID idle at its end is shorter than the pass.
      [ $(( (end - since) / 1000 )) -le "$idle_end" ]
    }
    valid_dom=0; valid_gpui=0; ran=0; attempt=0; waited=0
    while [ "$valid_dom" -lt "$rounds" ] || [ "$valid_gpui" -lt "$need_gpui" ]; do
      attempt=$((attempt + 1))
      [ "$attempt" -le "$attempts_max" ] || { say "timing $level: all $attempts_max attempts used"; break; }
      [ "$waited" -lt "$gate_budget" ] || { say "timing $level: the $gate_budget s of waiting for the gate are used"; break; }
      dir="$out/$level/r$attempt"
      mkdir -p "$dir"
      if wait_gate; then state=met; else state=skipped; fi
      node "$HERE/rounds.mjs" gate "$dir/round.json" "$level" "$attempt" "$ncases" "$state" "$gate_facts" || exit 1
      if [ "$state" = skipped ]; then say "attempt $attempt skipped: the gate was not met within its wait · $gate_facts"; continue; fi
      say "attempt $attempt: gate met · $gate_facts"
      ran=$((ran + 1))
      # The order flips every round run (A B, B A, …) so neither renderer always runs on the warmer machine.
      if [ $((ran % 2)) -eq 1 ]; then order="dom gpui"; else order="gpui dom"; fi
      for renderer in $order; do
        [ "$renderer" = dom ] && [ "$valid_dom" -ge "$rounds" ] && continue
        [ "$renderer" = gpui ] && [ "$valid_gpui" -ge "$need_gpui" ] && continue
        pass "$renderer" "$dir" || { say "attempt $attempt: the round is broken in its $renderer pass (rc or input above); its samples stay, and the next attempt waits for the gate again"; break; }
      done
      verdict=$(node "$HERE/rounds.mjs" judge "$dir")
      echo "$verdict" | sed 's/^/   /'
      echo "$verdict" | grep -q '^dom valid' && valid_dom=$((valid_dom + 1))
      echo "$verdict" | grep -q '^gpui valid' && valid_gpui=$((valid_gpui + 1))
      say "passes that count so far: DOM $valid_dom/$rounds, GPUI $valid_gpui/$need_gpui"
    done
    kill "$awake" 2>/dev/null
    if [ "$valid_dom" -ge "$rounds" ] && [ "$valid_gpui" -ge "$need_gpui" ]; then
      say "timing $level: the gate is met (DOM $valid_dom, GPUI $valid_gpui passes count)"
    else
      say "timing $level: the gate is NOT met (DOM $valid_dom/$rounds, GPUI $valid_gpui/$need_gpui passes count): no ranking"
      exit 3
    fi
    ;;
  report)
    out=${1:?report OUT}
    node "$HERE/report.mjs" "$out" --out "$out"
    ;;
  *)
    awk 'NR == 1 { next } /^set -u$/ { exit } { print }' "${BASH_SOURCE[0]}"
    exit 2
    ;;
esac
