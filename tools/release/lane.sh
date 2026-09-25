#!/bin/bash
# tools/release/lane.sh [<sha> ...] | --table
#
# The release lane, off the window (docs/design/release-lane-off-the-window.md).
# launchd starts it whenever the queue directory is not empty; it takes the
# lock, drains the queue oldest-first — a queued sha whose descendant is also
# queued is `superseded`, never gated (t-4123) — and for each sha: refuses on missing
# tools or a low disk, clones the sha into a scratch, trims any target over the
# cap, completes the root gate before the zo gate, judges known flakes solo, pushes
# <sha>:main, builds the app and zo in parallel, swaps each beside-then-mv,
# bundles the signed updater feed (bundle-updater), publishes the GitHub
# release when asked (publish; docs/design/versioned-auto-update.md §2.6), and
# sweeps. One trap covers every exit path — red, refused, SIGTERM, missing
# tool — so the scratch never outlives the lane.
#
# RELEASE_DRY_RUN=1 replaces every heavy step with a stub the caller steers via
# RELEASE_STUB_* (see `stub_*` below); tools/release/tests/test_lane.py pins the
# contract that way. The places below can be overridden by environment for the
# tests (RELEASE_HOME, RELEASE_REPO, RELEASE_SCRATCH_ROOT, RELEASE_APP_DIR,
# RELEASE_ZO_BIN, RELEASE_FLAKES_FILE, RELEASE_NODE_MODULES).
set -u

# ---------------------------------------------------------------- the table --
# The one place numbers, names and periods live (design §3.3). Nothing below
# this block spells a number of its own.
DISK_FLOOR_GB=10            # refuse (and keep the queue file) under this much free on /
TARGET_CAP_GB=20            # a warm target over this is cleaned — that one only (root-gate sat at 19G on 09-08 with 37G free: cold, the root gate costs minutes more than warm; the floor below still guards the disk)
POLL_SECS=15                # status.sh --wait poll period
THROTTLE_SECS=60            # launchd ThrottleInterval between lane launches
ZO_PROFILE=release          # cargo profile for the zo release build (fast-release is the candidate)
SOLO_RUNS=3                 # a known flake must pass this many solo runs
UI_SUITE_CHOOSER=WINDOW_SUITES   # the window runner's own chooser (ui/tests/window-runner.mjs): a failed browser check's
                            # suite is re-run alone through it — the whole harness took eight minutes a run (t-9741)
MAX_SOLO_JUDGED=4           # up to this many UNLISTED red names (root+zo together) are judged solo — a
                            # real regression fails solo too, and every run of the first day died on a
                            # NEW timing wait; more unlisted names at once is a real red. Names listed in
                            # flakes.txt never count: they fail as a cluster under load (1.3.27's six).
RECLAIM_BELOW_GB=20         # after a gate target's last use in the run it is cleaned when free is under this: the
                            # build phases still need ~8 G above the floor (09-11 17:32: refused at 7 G with a
                            # finished 23 G root-gate on disk). A reclaimed gate starts cold next run (~+8 min).
CALM_LOAD=12                # a gate starts once the 1-minute load average is at or under this (the lane
                            # machine has 12 cores): at load 30–43 (two gates + a cross-compile + fseventsd,
                            # 2026-09-11) gate-root took 1697 s against 668–1179 s calm and gate-zo 22231 s
                            # against 649–1342 s, and six pty e2e timed out that pass solo
CALM_WAIT_SECS=900          # …waiting at most this long; a machine still loud after that runs and says so
CALM_POLL_SECS=30           # how often the wait looks at the load
PHASES="archive gate-root gate-zo flakes push build-app swap-app build-zo swap-zo bundle-updater publish sweep"
TARGET_LANES="root-gate zo-gate root-release zo-release"
# The files a release commit always touches (bump.sh): a diff that changes only
# their version lines is the stamp, not a change to either tree.
RELEASE_STAMP_FILES="Cargo.toml Cargo.lock crates/zerocode-shell/tauri.conf.json zo-ide/Cargo.toml zo-ide/Cargo.lock"
TOOLS="cargo just node npm git swift codesign ditto python3 cmake ninja"   # python3 renders latest.json (updater_feed.py); gh only when publishing
LAUNCHD_LABEL=dev.zerocode.release
LAUNCHD_PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"   # sbin: sysctl (the load reading)
APP_NAME=ZeroCode.app
KILL_GRACE_SECS=2           # TERM to the children, then KILL after this long
# The release (docs/design/versioned-auto-update.md §2.2·§2.6·§2.7). The lane
# never raises the version — tools/release/bump.sh does, before the sha is queued.
# RELEASE_GITHUB_REPO is resolved from the checkout's own `origin` below, beside
# RELEASE_REPO — see the note there. The environment still wins.
RELEASE_PUBLISH=${RELEASE_PUBLISH:-0}          # 1 publishes the sha's release with gh — flipped per run by the coordinator, never here
RELEASE_CHANNEL=${RELEASE_CHANNEL:-stable}     # stable | beta (a prerelease; the moving `beta` tag carries the feed)
RELEASE_LEGACY_MANIFEST=${RELEASE_LEGACY_MANIFEST:-0}   # 1 sends the old zo CLI's manifest.txt·SHA256SUMS·zo-v<v>-<triple>×3·install.sh along — every target or refused (§2.7)
UPDATER_PLATFORM=darwin-aarch64                # the latest.json platforms key this machine builds
UPDATER_ARCH=aarch64                           # ZeroCode_<version>_<arch>.app.tar.gz
UPDATER_FEED=latest.json
ZO_BUILD_TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu"   # the manifest's rows in install.sh's order; the first is the host
BUILD_APP_CONFIG='{"bundle":{"createUpdaterArtifacts":false}}'   # the local install build makes no updater artifacts (tauri-cli refuses the key without plugins.updater); bundle-updater is the phase that does
COMPUTER_USE_HELPER='ZeroCode Computer Use.app'   # native UI helper under bundle Resources
APP_SIBLING_BINS='zerocode-mirror zerocode-pick'   # helper executables beside the shell in Contents/MacOS (Cargo.toml [[bin]]); tauri-cli 2.11.4 once dropped the second inferred one

SELF_DIR=$(cd "$(dirname "$0")" && pwd)
RELEASE_REPO=${RELEASE_REPO:-$(cd "$SELF_DIR/../.." && pwd)}
# The repository a release goes TO — which is not the one this source lives
# in. The feed has to be reachable without a token, so it is served from the
# public distribution repo while the source stays private (design
# versioned-auto-update.md §2.2), and reading it off this checkout's `origin`
# sent the publish to the source repo instead — where no installed app is
# looking (2026-09-18).
#
# The app ships the address it will poll in tauri.conf.json, so the lane reads
# the owner and name out of that same endpoint rather than keeping a second
# copy of them: the two hands cannot then disagree about where an update comes
# from. `RELEASE_GITHUB_REPO` in the environment still wins, for a release cut
# somewhere else on purpose.
github_repo_of() { # TAURI_CONF -> owner/name
  sed -n 's#.*https://github\.com/\([^/"]*\)/\([^/"]*\)/releases/.*#\1/\2#p' "$1" 2>/dev/null | head -1
}
RELEASE_GITHUB_REPO=${RELEASE_GITHUB_REPO:-$(github_repo_of "$RELEASE_REPO/crates/zerocode-shell/tauri.conf.json")}
RELEASE_HOME=${RELEASE_HOME:-$HOME/.local/share/zerocode/release}
RELEASE_SCRATCH_ROOT=${RELEASE_SCRATCH_ROOT:-/private/tmp}
RELEASE_APP_DIR=${RELEASE_APP_DIR:-/Applications}
RELEASE_ZO_BIN=${RELEASE_ZO_BIN:-$HOME/.local/bin/zo}
RELEASE_FLAKES_FILE=${RELEASE_FLAKES_FILE:-$SELF_DIR/flakes.txt}
RELEASE_NODE_MODULES=${RELEASE_NODE_MODULES:-$RELEASE_REPO/node_modules}
RELEASE_DRY_RUN=${RELEASE_DRY_RUN:-0}
RELEASE_STUB_LOG=${RELEASE_STUB_LOG:-}
UPDATER_KEY=${UPDATER_KEY:-$RELEASE_HOME/keys/updater.key}   # `tauri signer generate -w`; its .pub beside it; never in the tree (README.md)
UPDATER_KEY_PASSWORD_FILE=${UPDATER_KEY_PASSWORD_FILE:-$UPDATER_KEY.password}   # optional; absent means the key has no password
FEED_PY=$SELF_DIR/updater_feed.py
LEGACY_INSTALL_SH=$SELF_DIR/legacy/install.sh   # the old CLI's installer, verbatim (§2.7)
APP_INSTALL_SH=$SELF_DIR/install.sh             # the app's installer: reads the feed beside it, so it goes up with every release
# -------------------------------------------------------------------------------
# The root gate's browser harnesses (ui/tests/*.mjs) resolve Playwright through
# NODE_PATH, not only through the node_modules symlink the scratch gets.
export NODE_PATH=$RELEASE_NODE_MODULES

QUEUE=$RELEASE_HOME/queue
LOCK=$RELEASE_HOME/lock
OUT=$RELEASE_HOME/out
TARGETS=$RELEASE_HOME/target
WORK=$RELEASE_HOME/work
STATUS=$RELEASE_HOME/status.json
INSTALLED=$RELEASE_HOME/installed.json
INSTALLED_PREV=$RELEASE_HOME/installed.prev.json
UNLISTED=$RELEASE_HOME/unlisted.txt   # every unlisted red the lane judged solo, with its result: the evidence a flakes.txt line is written from

if [ "${1:-}" = "--table" ]; then
  for k in DISK_FLOOR_GB TARGET_CAP_GB RECLAIM_BELOW_GB POLL_SECS THROTTLE_SECS ZO_PROFILE SOLO_RUNS CALM_LOAD CALM_WAIT_SECS \
           CALM_POLL_SECS PHASES TARGET_LANES TOOLS \
           LAUNCHD_LABEL LAUNCHD_PATH APP_NAME RELEASE_REPO RELEASE_HOME RELEASE_SCRATCH_ROOT RELEASE_APP_DIR \
           RELEASE_ZO_BIN RELEASE_FLAKES_FILE QUEUE LOCK OUT STATUS INSTALLED INSTALLED_PREV UNLISTED UI_SUITE_CHOOSER \
           RELEASE_GITHUB_REPO RELEASE_PUBLISH RELEASE_CHANNEL RELEASE_LEGACY_MANIFEST UPDATER_KEY \
           UPDATER_PLATFORM UPDATER_ARCH UPDATER_FEED ZO_BUILD_TARGETS COMPUTER_USE_HELPER APP_SIBLING_BINS; do
    eval "printf \"%s='%s'\\n\" \"$k\" \"\$$k\""
  done
  exit 0
fi

# ------------------------------------------------------------------ helpers --
is_dry() { [ "$RELEASE_DRY_RUN" = "1" ]; }
now_s() { date +%s; }
now_iso() { date -u +%Y-%m-%dT%H:%M:%SZ; }
log() { printf '%s %s\n' "$(date +%T)" "$*" | tee -a "${LOG:-/dev/null}"; }
stub_log() { [ -n "$RELEASE_STUB_LOG" ] && printf '%s %s\n' "${SHA8:-lane}" "$*" >> "$RELEASE_STUB_LOG"; return 0; }
json_str() { printf '%s' "$1" | tr -d '\000-\037' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'; }
upper() { printf '%s' "$1" | tr 'a-z-' 'A-Z_'; }
stub() { # stub NAME [default] — the RELEASE_STUB_<NAME> value, or the default
  local v="RELEASE_STUB_$1"; printf '%s' "${!v:-${2:-}}"; }

# Every heavy step runs in the background and is waited for, so a trapped
# signal interrupts the wait at once instead of after the step ends.
HEAVY_PID=
heavy() { "$@" & HEAVY_PID=$!; wait "$HEAVY_PID"; local rc=$?; HEAVY_PID=; return $rc; }

kill_tree() { local p; for p in $(pgrep -P "$1" 2>/dev/null); do kill_tree "$p"; done; kill -TERM "$1" 2>/dev/null; }
kill_children() {
  local pids p; pids=$(pgrep -P $$ 2>/dev/null) || return 0
  for p in $pids; do kill_tree "$p"; done
  [ -n "$pids" ] && sleep "$KILL_GRACE_SECS"
  for p in $pids; do kill -KILL "$p" 2>/dev/null; done
  return 0
}

# --------------------------------------------------------------- status.json --
SHA=; SHA8=; SCRATCH=; LOG=; QUEUE_FILE=; STARTED_AT=; PHASE=; PHASE_T0=0
PHASES_JSON=; DISK_FREE_GB=0; OUTCOME=null; REASON=; SWEPT=1; LOCK_HELD=0
VERSION=; UPDATER_PUBKEY=; SKIP=; WHY=   # SKIP: why the phase being ended did nothing; WHY: a step's red in words

measure_disk() {
  if is_dry; then
    local phase_key
    phase_key=$(printf '%s' "${PHASE:-}" | tr '[:lower:]-' '[:upper:]_')
    DISK_FREE_GB=$(stub "DISK_FREE_GB_$phase_key" "$(stub DISK_FREE_GB 100)")
  else DISK_FREE_GB=$(df -g / | awk 'NR==2{print $4}'); fi
  DISK_FREE_GB=${DISK_FREE_GB:-0}
}
load_now() { # -> "<1-minute> <15-minute>" load averages, "? ?" when unreadable; the dry run's come from the stubs
  local loads
  if is_dry; then loads="$(stub LOAD1 0) $(stub LOAD15 0)"
  else loads=$(sysctl -n vm.loadavg 2>/dev/null | awk '{print $2, $4}'); fi
  printf '%s' "${loads:-? ?}"
}
# wait_for_calm NAME — runs under heavy: a gate does not start on a loud
# machine. Bounded (CALM_WAIT_SECS); a machine still loud afterwards gets the
# gate anyway, with the load in the log so a later flake judgment has its
# evidence. Every phase_end line carries the load for the same reason.
wait_for_calm() {
  local name=$1 t0 loads l1 elapsed budget poll
  if is_dry; then budget=$(stub CALM_WAIT_SECS 0); poll=0; else budget=$CALM_WAIT_SECS; poll=$CALM_POLL_SECS; fi
  t0=$(now_s)
  while :; do
    loads=$(load_now); l1=${loads%% *}; elapsed=$(( $(now_s) - t0 ))
    # An empty reading once passed as calm (`"" + 0 <= 12`) — under launchd,
    # before its PATH carried sysctl. Unknown is said, never judged calm.
    if [ "$l1" = "?" ]; then
      log "$name: load unreadable — running without the calm check"
      stub_log "load-unknown $name"
      return 0
    fi
    if awk -v l="$l1" -v c="$CALM_LOAD" 'BEGIN { exit !(l + 0 <= c + 0) }'; then
      [ "$elapsed" -gt 0 ] && log "$name: calm at load $l1 after ${elapsed}s"
      return 0
    fi
    if [ "$elapsed" -ge "$budget" ]; then
      log "$name: loud — load $l1 > $CALM_LOAD after ${elapsed}s, running anyway"
      stub_log "loud $name load=$l1"
      return 0
    fi
    [ "$elapsed" -gt 0 ] || log "$name: waiting for calm — load $l1 > $CALM_LOAD (up to ${budget}s)"
    sleep "$poll"
  done
}
write_status() {
  local outcome=$OUTCOME
  [ "$outcome" != null ] && outcome="\"$outcome\""
  printf '{"sha":"%s","phase":"%s","started_at":"%s","updated_at":"%s","phases":[%s],"disk_free_gb":%s,"outcome":%s,"reason":"%s","version":"%s","updater_pubkey":"%s"}\n' \
    "$SHA" "$PHASE" "$STARTED_AT" "$(now_iso)" "$PHASES_JSON" "$DISK_FREE_GB" "$outcome" "$(json_str "$REASON")" \
    "$(json_str "$VERSION")" "$(json_str "$UPDATER_PUBKEY")" \
    > "$STATUS.tmp" && mv -f "$STATUS.tmp" "$STATUS"
}
phase_begin() { PHASE=$1; PHASE_T0=$(now_s); write_status; log "== $PHASE"; }
phase_end() { # NAME RC [SECS] — a phase that set SKIP did nothing, and says why
  local name=$1 rc=$2 secs=${3:-$(( $(now_s) - PHASE_T0 ))} skipped=
  [ -z "$SKIP" ] || skipped=",\"skipped\":\"$(json_str "$SKIP")\""
  PHASES_JSON="${PHASES_JSON:+$PHASES_JSON,}{\"name\":\"$name\",\"rc\":$rc,\"secs\":$secs$skipped}"
  measure_disk; write_status
  log "$name rc=$rc ${secs}s free=${DISK_FREE_GB}G load=$(load_now | tr ' ' /)${SKIP:+ skipped: $SKIP}"
  SKIP=
}
# A successful step that used up the disk can resume after space is freed.
# A failed step stays red: low disk must not hide its failure or requeue the
# same broken sha indefinitely.
step_done() {
  phase_end "$@"
  if [ "$DISK_FREE_GB" -lt "$DISK_FLOOR_GB" ]; then
    if [ "$2" != 0 ]; then
      red "$1 rc=$2; disk: ${DISK_FREE_GB}G free < ${DISK_FLOOR_GB}G floor"
      return 1
    fi
    refuse "disk: ${DISK_FREE_GB}G free < ${DISK_FLOOR_GB}G floor after $1"
    [ -n "$QUEUE_FILE" ] && touch "$QUEUE_FILE"
    return 3
  fi
  return 0
}
refuse() { OUTCOME=refused; REASON=$1; log "refused: $1"; }
red() { OUTCOME=red; REASON=$1; log "red: $1"; }
superseded() { OUTCOME=superseded; REASON=$1; log "superseded: $1"; }
# Before a phase that has not started: a queued descendant makes this sha's
# remaining gates moot. Past push the build runs to its end — its artifacts
# are the sha's own, and a half-built swap is worse than a late one.
bow_out_if_superseded() { # PHASE
  local by; by=$(superseded_by "$SHA") || return 0
  superseded "superseded by $(printf '%s' "$by" | cut -c1-8) — queued before $1"
  return 1
}

# ------------------------------------------------------------ installed.json --
# Each half is {sha, at, version}: the version is the scratch's
# [workspace.package] version — the same letters as status.json's — so the
# window can say 「새 버전 {{version}}」 instead of a sha (t-3237). A file an
# older lane wrote has no version; its half reads back with an empty one,
# which the window takes as unknown. rollback.sh carries a copy of these
# three functions (it sources the table only); keep the two the same.
installed_part() { # FILE app|zo -> "sha at [version]" or nothing
  [ -f "$1" ] || return 0
  sed -n "s/.*\"$2\":{\"sha\":\"\([^\"]*\)\",\"at\":\"\([^\"]*\)\"\(,\"version\":\"\([^\"]*\)\"\)\{0,1\}}.*/\1 \2 \4/p" "$1"
}
part_json() { # "sha at [version]" -> {"sha":..,"at":..,"version":..} | null
  if [ -n "$1" ]; then set -- $1; printf '{"sha":"%s","at":"%s","version":"%s"}' "$1" "$2" "${3:-}"; else printf 'null'; fi
}
write_installed_file() { # FILE "app sha at version" "zo sha at version"
  printf '{"app":%s,"zo":%s}\n' "$(part_json "$2")" "$(part_json "$3")" > "$1.tmp" && mv -f "$1.tmp" "$1"
}
record_installed() { # app|zo SHA — that part only, with the lane's VERSION; the other keeps its value
  local part=$1 sha=$2 app zo papp pzo
  app=$(installed_part "$INSTALLED" app); zo=$(installed_part "$INSTALLED" zo)
  papp=$(installed_part "$INSTALLED_PREV" app); pzo=$(installed_part "$INSTALLED_PREV" zo)
  case $part in
    app) write_installed_file "$INSTALLED_PREV" "$app" "$pzo"; app="$sha $(now_iso) $VERSION" ;;
    zo)  write_installed_file "$INSTALLED_PREV" "$papp" "$zo"; zo="$sha $(now_iso) $VERSION" ;;
  esac
  write_installed_file "$INSTALLED" "$app" "$zo"
}

# ------------------------------------------------------------------- steps --
# Each real step has a stub twin; is_dry picks. Real steps redirect their own
# output to a log under $OUT so the lane log stays one line per step.
have_tool() {
  if is_dry; then case " $(stub MISSING_TOOLS) " in *" $1 "*) return 1;; esac; return 0; fi
  command -v "$1" > /dev/null 2>&1
}
resolve_sha() { # short or long -> full, or nothing
  if is_dry; then printf '%s' "$1"; return 0; fi
  git -C "$RELEASE_REPO" rev-parse --verify --quiet "$1^{commit}" 2>/dev/null
}
target_dir() { printf '%s/%s' "$TARGETS" "$1"; }
target_kb() {
  local v; v=$(stub "TARGET_KB_$(upper "$1")")
  if is_dry && [ -n "$v" ]; then printf '%s' "$v"; return 0; fi
  du -sk "$(target_dir "$1")" 2>/dev/null | awk '{print $1}'
}
clean_target() { # LANE — empty it, keep the directory
  stub_log "clean $1"
  if is_dry; then rm -rf "$(target_dir "$1")"
  else CARGO_TARGET_DIR=$(target_dir "$1") cargo clean --manifest-path "$SCRATCH/Cargo.toml" > /dev/null 2>&1 || rm -rf "$(target_dir "$1")"
  fi
  mkdir -p "$(target_dir "$1")"
}
# release_gate_target LANE — called once LANE's gate target has had its last use
# in this run (its gate passed, or the solo judgments are done): on a tight disk
# it goes now, before the floor judges the phases still to come.
release_gate_target() {
  measure_disk
  [ "$DISK_FREE_GB" -lt "$RECLAIM_BELOW_GB" ] || return 0
  log "released $1 — ${DISK_FREE_GB}G free < ${RECLAIM_BELOW_GB}G after its last use in this run"
  clean_target "$1"
}
trim_targets() {
  local lane kb cap_kb=$(( TARGET_CAP_GB * 1024 * 1024 ))
  for lane in $TARGET_LANES; do
    mkdir -p "$(target_dir "$lane")"
    kb=$(target_kb "$lane"); kb=${kb:-0}
    if [ "$kb" -gt "$cap_kb" ]; then
      log "target $lane is $(( kb / 1024 / 1024 ))G > ${TARGET_CAP_GB}G cap — cleaning that one"
      clean_target "$lane"
    fi
  done
}

do_archive() {
  rm -rf "$SCRATCH"
  if is_dry; then
    stub_log "archive"; mkdir -p "$SCRATCH" && printf '%s\n' "$SHA" > "$SCRATCH/sha"
    return "$(stub ARCHIVE_RC 0)"
  fi
  # A shared clone, not `git archive`: build.rs stamps ZEROCODE_COMMIT from
  # `git rev-parse HEAD` in the tree it builds, and an archive has no HEAD to
  # answer with. The clone borrows objects (alternates), touches no
  # .git/worktrees ledger, and is swept with rm -rf alone (76 MB, 0.4 s).
  git clone -q --shared --no-checkout "$RELEASE_REPO" "$SCRATCH" > "$OUT/archive-$SHA8.log" 2>&1 \
    && git -C "$SCRATCH" checkout -q --detach "$SHA" >> "$OUT/archive-$SHA8.log" 2>&1 \
    && ln -s "$RELEASE_NODE_MODULES" "$SCRATCH/node_modules"
}

# gate LANE — runs in the background; writes "rc secs" to $WORK/gate-LANE.result
run_gate() {
  local lane=$1 t0 rc log="$OUT/gate-$1-$SHA8.log" target
  target=$(target_dir "$lane-gate"); t0=$(now_s)
  if is_dry; then
    stub_log "gate $lane target=$target"
    sleep "$(stub "GATE_$(upper "$lane")_SLEEP" 0)"
    rc=$(stub "GATE_$(upper "$lane")_RC" 0)
    { echo "stub gate $lane"; if [ "$rc" != 0 ]; then echo "==> verify recipe test"; echo "failures:"; local f; for f in $(stub "GATE_$(upper "$lane")_FAILS"); do echo "    $f"; done; echo "test result: FAILED."; fi
      local h r; h=$(stub "GATE_$(upper "$lane")_HARNESS" ""); r=$(stub "GATE_$(upper "$lane")_REPORT" "")
      [ -z "$h" ] || { echo "==> verify recipe $h-browser-test"; [ -z "$r" ] || cat "$r"; echo "error: recipe \`$h-browser-test\` failed on line 41 with exit code 1"; }
      local u; u=$(stub "GATE_$(upper "$lane")_UNNAMED" ""); [ -z "$u" ] || { echo "==> verify recipe $u"; echo "error: recipe \`$u\` failed on line 43 with exit code 101"; }
    } > "$log"
  else
    case $lane in
      root) ( cd "$SCRATCH" && CARGO_TARGET_DIR=$target "$SELF_DIR/verify-every-recipe.sh" ) > "$log" 2>&1; rc=$? ;;
      zo)   ( cd "$SCRATCH/zo-ide" && CARGO_TARGET_DIR=$target "$SELF_DIR/verify-every-recipe.sh" ) > "$log" 2>&1; rc=$? ;;
    esac
  fi
  echo "$rc $(( $(now_s) - t0 ))" > "$WORK/gate-$lane.result"
  stub_log "gate $lane done rc=$rc"
  return "$rc"
}
failed_tests() { # LOG -> sorted unique failure names, one a line, each read against its own recipe
                 # (the `==> verify recipe` marks): the test paths of a cargo `failures:` block; for a
                 # failed `<harness>-browser-test`, each check its report failed (`FAIL  <check>  — …`)
                 # as `ui:<harness>/<suite>:<check>` — the suite the `SUITE  <suite>` line over it named,
                 # `ui:<harness>:<check>` in a harness without suites (t-9741) — or `harness:<harness>`
                 # when it named none (it died before its report); and `recipe:<name>` for any other
                 # recipe that failed without naming a test — its tests never spoke, so no solo run can
                 # judge it (1.3.37: shell-test died compiling beside a named harness red)
  awk '
    /^==> verify recipe / {
      named = 0; harness = ""; suite = ""; checks = ""
      if ($4 ~ /^[a-z]+-browser-test$/) { harness = $4; sub(/-browser-test$/, "", harness) }
      next
    }
    /^failures:$/ { inblock = 1; next }
    inblock && /^test result/ { inblock = 0; next }
    inblock && /^    [A-Za-z0-9_:]+$/ { sub(/^    /, ""); print; named = 1; next }
    harness != "" && /^SUITE  / { suite = substr($0, 8); next }
    harness != "" && /^FAIL  / {
      check = substr($0, 7); cut = index(check, "  — "); if (cut) check = substr(check, 1, cut - 1)
      gsub(/\t/, " ", check)
      checks = checks "ui:" harness (suite == "" ? "" : "/" suite) ":" check "\n"
      next
    }
    /recipe `[^`]+` (failed|was terminated)/ {
      match($0, /recipe `[^`]+`/); r = substr($0, RSTART + 8, RLENGTH - 9)
      if (r ~ /^[a-z]+-browser-test$/) {
        if (checks != "") printf "%s", checks
        else { sub(/-browser-test$/, "", r); print "harness:" r }
        checks = ""
      }
      else if (!named) print "recipe:" r
    }
  ' "$1" 2>/dev/null | sort -u
}
known_flake() { # LANE NAME — listed in flakes.txt: a test path under its lane's prefix (substring, as
                # before), a browser check under `ui:` by the start of its name (t-9741)
  local k check; [ -f "$RELEASE_FLAKES_FILE" ] || return 1
  case $2 in
    ui:*)
      check=${2#ui:}; check=${check#*:}
      while IFS= read -r k; do
        [ -n "$k" ] || continue   # an empty `ui:` is the start of every name, so it lists none
        case $check in "$k"*) return 0;; esac
      done <<< "$(sed -n 's/^ui://p' "$RELEASE_FLAKES_FILE")"
      return 1 ;;
  esac
  for k in $(sed -n "s/^$1:\([A-Za-z0-9_:]*\).*/\1/p" "$RELEASE_FLAKES_FILE"); do
    case "$2" in *"$k"*) return 0;; esac
  done
  return 1
}
solo_unit() { # NAME -> what a solo run re-runs for it: a cargo test by its path; a browser check by its
              # suite (`ui:<harness>/<suite>`), or its harness whole when it has none (`harness:<harness>`)
  local unit
  case $1 in
    ui:*) unit=${1#ui:}; unit=${unit%%:*}
          case $unit in */*) printf 'ui:%s' "$unit" ;; *) printf 'harness:%s' "$unit" ;; esac ;;
    *) printf '%s' "$1" ;;
  esac
}
solo_plan() { # LANE LOG -> "<lane>\t<listed|unlisted|recipe>\t<unit>\t<name>", one line per failure name
  local lane=$1 f kind
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    case $f in
      recipe:*) kind=recipe ;;
      *) if known_flake "$lane" "$f"; then kind=listed; else kind=unlisted; fi ;;
    esac
    printf '%s\t%s\t%s\t%s\n' "$lane" "$kind" "$(solo_unit "$f")" "$f"
  done <<< "$(failed_tests "$2")"
}
plan_names() { # PLAN LANE -> 0 when the plan holds a failure name under that lane
  case $'\n'"$1" in *$'\n'"$2"$'\t'*) return 0 ;; esac
  return 1
}
# say_unlisted PLAN LANE UNIT PASSED VERDICT — each unlisted name that unit's solo runs judged: in the
# lane log, and kept in $UNLISTED with the day, the sha and its gate log, where the person writing a
# flakes.txt line finds the evidence for it (t-9741).
say_unlisted() {
  local lane kind unit f
  while IFS=$'\t' read -r lane kind unit f; do
    [ "$kind" = unlisted ] && [ "$lane" = "$2" ] && [ "$unit" = "$3" ] || continue
    log "unlisted red $lane:$f — judged solo $4/$SOLO_RUNS $5"
    printf '%s %s %s:%s — judged solo %s/%s %s (out/gate-%s-%s.log)\n' \
      "$(now_iso)" "$SHA8" "$lane" "$f" "$4" "$SOLO_RUNS" "$5" "$lane" "$SHA8" >> "$UNLISTED"
  done <<< "$1"
}
# ui_solo HARNESS/SUITE — that suite of the harness alone, chosen the way the window runner chooses
# (`$UI_SUITE_CHOOSER`, anchored so no other suite's name contains it), by the recipe the gate ran.
ui_solo() {
  local harness=${1%%/*} suite=${1#*/}
  env "$UI_SUITE_CHOOSER=^$(printf '%s' "$suite" | sed 's/[][\\.*^$+?(){}|]/\\&/g')\$" just "$harness-browser-test"
}
run_solo() { # LANE NAME RUN -> prints rc
  local lane=$1 name=$2 n=$3 rc log="$OUT/solo-$1-$SHA8-$3.log" target
  target=$(target_dir "$lane-gate")
  if is_dry; then
    # The stub's rc sequence is consumed across every solo of the sha; the
    # counter lives in a file because this runs inside $(...).
    local seq w last=0 i=0 nth=1; seq=$(stub SOLO_RCS 0)
    [ -f "$WORK/solo.n" ] && read -r nth < "$WORK/solo.n"; echo $(( nth + 1 )) > "$WORK/solo.n"
    stub_log "solo $lane $name run=$n"
    for w in $seq; do i=$(( i + 1 )); last=$w; [ "$i" -eq "$nth" ] && { echo "$w"; return 0; }; done
    echo "$last"; return 0
  fi
  case "$lane:$name" in
    # A node harness is judged by the very recipe the gate ran, so a new harness
    # needs no line here: whole when its checks have no suite to stand in (or it
    # named none), and one suite alone when the check that failed named its suite.
    root:harness:*) ( cd "$SCRATCH" && just "${name#harness:}-browser-test" ) > "$log" 2>&1; rc=$? ;;
    root:ui:*) ( cd "$SCRATCH" && ui_solo "${name#ui:}" ) > "$log" 2>&1; rc=$? ;;
    root:*) ( cd "$SCRATCH" && CARGO_TARGET_DIR=$target cargo test --workspace "$name" ) > "$log" 2>&1; rc=$? ;;
    zo:*)   ( cd "$SCRATCH/zo-ide" && ZO_DISABLE_KEYCHAIN=1 CARGO_TARGET_DIR=$target cargo test --workspace "$name" ) > "$log" 2>&1; rc=$? ;;
  esac
  echo "$rc"
}
do_push() {
  local log="$OUT/push-$SHA8.log"
  if is_dry; then stub_log "push"; return "$(stub PUSH_RC 0)"; fi
  git -C "$RELEASE_REPO" push origin "$SHA:main" > "$log" 2>&1
}
# Another session may push main past the gated sha while it gates. The gated
# sha is what was verified; if origin already carries it as an ancestor it is
# published, so build and swap it. Otherwise stop before any build.
sha_on_origin() {
  if is_dry; then stub_log "ancestor"; return "$(stub ANCESTOR_RC 0)"; fi
  git -C "$RELEASE_REPO" fetch -q origin main >> "$OUT/push-$SHA8.log" 2>&1
  git -C "$RELEASE_REPO" merge-base --is-ancestor "$SHA" origin/main
}
# is_ancestor ANCESTOR DESCENDANT — the repo's word; dry-run reads the stub
# ANCESTRY, space-separated `<descendant>:<ancestor>` pairs of full shas.
is_ancestor() {
  if is_dry; then case " $(stub ANCESTRY) " in *" $2:$1 "*) return 0;; *) return 1;; esac; fi
  git -C "$RELEASE_REPO" merge-base --is-ancestor "$1" "$2" 2>/dev/null
}
# superseded_by SHA -> the oldest queued sha that descends from it (t-4123).
# A descendant carries everything its ancestor has, so gating the ancestor
# first is an hour that buys nothing: 09-14 three bumps five minutes apart.
superseded_by() {
  local q
  for q in $(ls -tr "$QUEUE" 2>/dev/null); do
    [ "$q" != "$1" ] || continue
    case $q in *[!0-9a-f]*) continue;; esac
    if is_ancestor "$1" "$q"; then printf '%s' "$q"; return 0; fi
  done
  return 1
}
# build KIND(app|zo) — background; writes "rc secs" to $WORK/build-KIND.result
run_build() {
  local kind=$1 t0 rc log="$OUT/build-$1-$SHA8.log" target
  t0=$(now_s)
  case $kind in
    app)
      target=$(target_dir root-release)
      if is_dry; then
        stub_log "build-app target=$target"; sleep "$(stub BUILD_APP_SLEEP 0)"; rc=$(stub BUILD_APP_RC 0)
        [ "$rc" = 0 ] && { mkdir -p "$target/release/bundle/macos/$APP_NAME" && printf '%s\n' "$SHA" > "$target/release/bundle/macos/$APP_NAME/sha"; }
      else
        # stage-zo builds zo into the same warm target build-zo uses beside it: cargo's
        # build-directory lock serialises the two and the later one is a no-op, where a
        # cold zo-ide/target under the scratch cost 6 min of the app build (09-08).
        ( cd "$SCRATCH" && CI=true CARGO_TARGET_DIR=$target ZO_STAGE_TARGET_DIR=$(target_dir zo-release) npm run tauri:release -- --bundles app --ci --no-sign --config "$BUILD_APP_CONFIG" ) > "$log" 2>&1; rc=$?
        if [ "$rc" = 0 ] && ! is_dry; then
          # Sign the whole app inside-out with the machine's local identity: the
          # helper, the sibling helpers, and the outer dev.zerocode.app — whose
          # stable requirement is what lets the Screen Recording grant survive
          # rebuilds (adhoc changed the cdhash every build). See tools/signing.
          ( bash "$SCRATCH/tools/signing/sign-app-bundle.sh" "$target/release/bundle/macos/$APP_NAME" ) >> "$log" 2>&1
          rc=$?
        fi
        [ "$rc" = 0 ] && ! helper_in_bundle "$target" && { printf 'build-app: %s\n' "$WHY" >> "$log"; rc=1; }
      fi ;;
    zo)
      target=$(target_dir zo-release)
      if is_dry; then
        stub_log "build-zo target=$target"; sleep "$(stub BUILD_ZO_SLEEP 0)"; rc=$(stub BUILD_ZO_RC 0)
        [ "$rc" = 0 ] && { mkdir -p "$target/$ZO_PROFILE" && printf '%s\n' "$SHA" > "$target/$ZO_PROFILE/zo"; }
      else
        ( cd "$SCRATCH/zo-ide" && CARGO_TARGET_DIR=$target cargo build --profile "$ZO_PROFILE" -p zo-ide ) > "$log" 2>&1; rc=$?
      fi
      [ "$rc" = 0 ] && [ "$RELEASE_LEGACY_MANIFEST" = 1 ] && build_zo_cross "$target" "$log" ;;
  esac
  echo "$rc $(( $(now_s) - t0 ))" > "$WORK/build-$kind.result"
  return "$rc"
}
# The legacy manifest's other targets (§2.7): a cross build per triple after
# the host's, each on its own rc — a missing one does not redden build-zo (the
# host zo still swaps); publish refuses the manifest whole instead.
build_zo_cross() { # TARGET_DIR LOG
  local t crc builder
  for t in $ZO_BUILD_TARGETS; do
    [ "$t" = "${ZO_BUILD_TARGETS%% *}" ] && continue
    if is_dry; then
      case " $(stub ZO_CROSS_MISSING) " in *" $t "*) crc=1;; *) crc=0;; esac
      stub_log "build-zo cross=$t rc=$crc"
      [ "$crc" = 0 ] && { mkdir -p "$1/$t/$ZO_PROFILE" && printf '%s %s\n' "$SHA" "$t" > "$1/$t/$ZO_PROFILE/zo"; }
    else
      case $t in *-linux-*) builder="cargo zigbuild";; *) builder="cargo build";; esac
      ( cd "$SCRATCH/zo-ide" && CARGO_TARGET_DIR=$1 $builder --profile "$ZO_PROFILE" -p zo-ide --target "$t" ) >> "$2" 2>&1; crc=$?
      log "build-zo cross $t rc=$crc"
    fi
  done
  return 0
}
# The built app carries the Computer Use helper with its executable (the fixture the window launches).
helper_in_bundle() { # TARGET — every helper the window launches rides in the built app; WHY names the first missing one
  local app bin; app=$(ls -d "$1/release/bundle/macos/"*.app 2>/dev/null | head -1)
  [ -n "$app" ] || { WHY="no .app under $1/release/bundle/macos"; return 1; }
  [ -x "$app/Contents/Resources/$COMPUTER_USE_HELPER/Contents/MacOS/zerocode-computer-use-macos" ] || { WHY="$COMPUTER_USE_HELPER is not under the bundle Resources"; return 1; }
  for bin in $APP_SIBLING_BINS; do
    [ -x "$app/Contents/MacOS/$bin" ] || { WHY="$bin is not beside the shell in Contents/MacOS"; return 1; }
  done
}
copy_tree() { if is_dry; then cp -R "$1" "$2"; else ditto "$1" "$2"; fi; }
# Rule 1: never overwrite a running binary in place — beside, then mv, keep .old.
swap_app() {
  local app new="$RELEASE_APP_DIR/.$APP_NAME.new" cur="$RELEASE_APP_DIR/$APP_NAME" old="$RELEASE_APP_DIR/$APP_NAME.old"
  app=$(ls -d "$(target_dir root-release)/release/bundle/macos/"*.app 2>/dev/null | head -1)
  [ -n "$app" ] || { log "no .app bundle under $(target_dir root-release)"; return 1; }
  stub_log "swap-app"
  rm -rf "$new" && copy_tree "$app" "$new" || return 1
  rm -rf "$old"; [ -d "$cur" ] && { mv "$cur" "$old" || return 1; }
  mv "$new" "$cur" || return 1
  record_installed app "$SHA"
}
swap_zo() {
  local bin="$(target_dir zo-release)/$ZO_PROFILE/zo" new="$RELEASE_ZO_BIN.new" old="$RELEASE_ZO_BIN.old"
  [ -f "$bin" ] || { log "no zo binary at $bin"; return 1; }
  stub_log "swap-zo"
  cp "$bin" "$new" || return 1
  # The old inode stays reachable as zo.old (a hard link), so the replacement
  # is one rename with no moment in which `zo` does not exist.
  [ -f "$RELEASE_ZO_BIN" ] && { rm -f "$old"; ln "$RELEASE_ZO_BIN" "$old" 2>/dev/null || cp "$RELEASE_ZO_BIN" "$old"; }
  mv -f "$new" "$RELEASE_ZO_BIN" || return 1
  record_installed zo "$SHA"
}

# ------------------------------------------------------- bundle-updater --
# The scratch's own facts; the dry twin answers from stubs.
scratch_version() { if is_dry; then stub VERSION 0.1.0; else python3 "$FEED_PY" version "$SCRATCH/Cargo.toml" 2>/dev/null; fi; }
scratch_pubkey() { # plugins.updater.pubkey of the scratch's tauri.conf.json, or nothing (U-B's section)
  if is_dry; then stub PLUGIN_PUBKEY ""; else python3 "$FEED_PY" conf-pubkey "$SCRATCH/crates/zerocode-shell/tauri.conf.json" 2>/dev/null; fi
}
out_dir() { printf '%s/%s' "$OUT" "$SHA8"; }
asset_base() { printf 'ZeroCode_%s_%s' "$VERSION" "$UPDATER_ARCH"; }
asset_url() { printf 'https://github.com/%s/releases/download/v%s/%s' "$RELEASE_GITHUB_REPO" "$VERSION" "$1"; }
# The signed build: tauri reads the key from the path in the env, never its
# bytes on a command line; --ci takes the password from the env (empty = none).
build_updater_bundle() { # TARGET PASSWORD LOG
  ( cd "$SCRATCH" && CI=true CARGO_TARGET_DIR=$1 TAURI_SIGNING_PRIVATE_KEY=$UPDATER_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD=$2 \
      npm run tauri:release -- --bundles app,dmg --ci ) > "$3" 2>&1
}
# The app the DMG and the updater archive carry must be SEALED, the way the
# installed app is. Tauri's bundler signs only when a distribution identity is
# in the env (APPLE_SIGNING_IDENTITY, with notarization when APPLE_ID·
# APPLE_PASSWORD·APPLE_TEAM_ID stand beside it); without one it ships the
# linker's adhoc signature over unsealed resources, and another Mac's
# Gatekeeper calls that "damaged" the moment quarantine is on it (v1.3.78,
# 2026-09-15: a colleague's MacBook Air). So without that identity the lane
# signs the bundle inside-out with the same signer the install phase uses,
# then packs the archive and the DMG again from the sealed app — Tauri had
# packed both before any signature stood — and signs the archive with the
# updater key. With the identity, Tauri's own seal is kept and only verified.
# Either way the bundle must verify --deep --strict before it is gathered.
seal_updater_bundle() { # TARGET PASSWORD LOG -> rc; WHY on failure
  local target=$1 password=$2 log=$3 app archive dmg staging rc
  app="$target/release/bundle/macos/$APP_NAME"
  [ -d "$app" ] || { WHY="no $APP_NAME under $target/release/bundle/macos"; return 1; }
  if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
    bash "$SCRATCH/tools/signing/sign-app-bundle.sh" "$app" >> "$log" 2>&1 || { WHY="sign-app-bundle.sh on the DMG's app ($log)"; return 1; }
    archive="$target/release/bundle/macos/$APP_NAME.tar.gz"
    rm -f "$archive" "$archive.sig"
    # COPYFILE_DISABLE: no AppleDouble `._` entries — the updater unpacks the
    # archive with its own tar and expects the bundle's files alone.
    ( cd "$target/release/bundle/macos" && COPYFILE_DISABLE=1 tar -czf "$APP_NAME.tar.gz" "$APP_NAME" ) >> "$log" 2>&1 || { WHY="pack the sealed app into $archive"; return 1; }
    ( cd "$SCRATCH" && TAURI_SIGNING_PRIVATE_KEY_PASSWORD=$password npx --no-install tauri signer sign -f "$UPDATER_KEY" "$archive" ) >> "$log" 2>&1 \
      || { WHY="sign $archive with the updater key ($log)"; return 1; }
    [ -f "$archive.sig" ] || { WHY="tauri signer left no $archive.sig"; return 1; }
    mkdir -p "$target/release/bundle/dmg" && rm -f "$target/release/bundle/dmg/"*.dmg
    dmg="$target/release/bundle/dmg/$(asset_base).dmg"
    staging=$(mktemp -d "${TMPDIR:-/tmp}/zerocode-dmg.XXXXXX") || { WHY="no staging folder for the DMG"; return 1; }
    cp -R "$app" "$staging/" && ln -s /Applications "$staging/Applications" \
      && hdiutil create -quiet -volname "${APP_NAME%.app}" -srcfolder "$staging" -ov -format UDZO "$dmg" >> "$log" 2>&1
    rc=$?; rm -rf "$staging"
    [ "$rc" = 0 ] || { WHY="hdiutil create $dmg rc=$rc ($log)"; return 1; }
    log "bundle-updater: sealed $APP_NAME with the local identity; archive and DMG packed again from it"
  fi
  codesign --verify --deep --strict "$app" >> "$log" 2>&1 || { WHY="the DMG's app does not verify (codesign --deep --strict, $log)"; return 1; }
}
# Runs in the foreground (SKIP/UPDATER_PUBKEY/WHY are its answers); only the
# build inside is heavy. Skips — no key, no plugins.updater yet — are green
# and said; a pubkey that is not the configured one is red, because the
# updater would refuse what that key signs.
do_bundle_updater() {
  local target pub conf_pub conf_fp out log="$OUT/bundle-updater-$SHA8.log" archive dmg rc password=
  target=$(target_dir root-release); out=$(out_dir)
  [ -n "$VERSION" ] || { WHY="no [workspace.package] version in the scratch"; return 1; }
  [ -f "$UPDATER_KEY" ] || { SKIP="no updater key at $UPDATER_KEY (tools/release/README.md)"; return 0; }
  pub="$UPDATER_KEY.pub"
  [ -f "$pub" ] || { SKIP="no public key at $pub"; return 0; }
  UPDATER_PUBKEY=$(python3 "$FEED_PY" fingerprint --file "$pub") || { WHY="$pub is not a minisign public key"; return 1; }
  conf_pub=$(scratch_pubkey)
  [ -n "$conf_pub" ] || { SKIP="no plugins.updater.pubkey in the scratch tauri.conf.json (U-B)"; return 0; }
  conf_fp=$(python3 "$FEED_PY" fingerprint --text "$conf_pub" 2>/dev/null)
  [ "$conf_fp" = "$UPDATER_PUBKEY" ] || { WHY="pubkey mismatch: key $UPDATER_PUBKEY, tauri.conf.json ${conf_fp:-unreadable} — the updater would refuse the signature"; return 1; }
  # The archive is only ever shipped by a publish run, which builds it again on its
  # own sha; an install run skips the second app,dmg build aloud (529 s on 09-08).
  [ "$RELEASE_PUBLISH" = 1 ] || { SKIP="RELEASE_PUBLISH=0 (the publish run builds the archive)"; return 0; }
  mkdir -p "$out" || return 1
  if is_dry; then
    stub_log "bundle-updater target=$target key=$UPDATER_KEY"; rc=$(stub BUNDLE_UPDATER_RC 0)
    [ "$rc" = 0 ] || { WHY="build rc=$rc"; return 1; }
    mkdir -p "$target/release/bundle/macos" "$target/release/bundle/dmg"
    printf 'stub archive %s\n' "$SHA" > "$target/release/bundle/macos/$APP_NAME.tar.gz"
    printf 'STUBSIG%s\n' "$SHA8" > "$target/release/bundle/macos/$APP_NAME.tar.gz.sig"
    printf 'stub dmg\n' > "$target/release/bundle/dmg/$(asset_base).dmg"
    printf '### stub\n- stub notes for %s\n' "$VERSION" > "$out/notes.md"
  else
    [ -f "$UPDATER_KEY_PASSWORD_FILE" ] && password=$(cat "$UPDATER_KEY_PASSWORD_FILE")
    heavy build_updater_bundle "$target" "$password" "$log"; rc=$?
    [ "$rc" = 0 ] || { WHY="tauri build rc=$rc ($log)"; return 1; }
    seal_updater_bundle "$target" "$password" "$log" || return 1
    python3 "$FEED_PY" section --changelog "$SCRATCH/CHANGELOG.md" --version "$VERSION" > "$out/notes.md" \
      || { WHY="CHANGELOG.md has no [$VERSION] section — run tools/release/bump.sh before queueing"; return 1; }
  fi
  archive="$target/release/bundle/macos/$APP_NAME.tar.gz"
  { [ -f "$archive" ] && [ -f "$archive.sig" ]; } || { WHY="no signed updater archive under $target/release/bundle/macos"; return 1; }
  cp "$archive" "$out/$(asset_base).app.tar.gz" && cp "$archive.sig" "$out/$(asset_base).app.tar.gz.sig" || { WHY="copy to $out"; return 1; }
  dmg=$(ls "$target/release/bundle/dmg/"*.dmg 2>/dev/null | head -1)
  [ -z "$dmg" ] || cp "$dmg" "$out/$(asset_base).dmg" || { WHY="copy dmg to $out"; return 1; }
  python3 "$FEED_PY" latest --version "$VERSION" --notes-file "$out/notes.md" --pub-date "$(now_iso)" \
      --platform "$UPDATER_PLATFORM" --sig-file "$out/$(asset_base).app.tar.gz.sig" --url "$(asset_url "$(asset_base).app.tar.gz")" \
      > "$out/$UPDATER_FEED.tmp" && mv -f "$out/$UPDATER_FEED.tmp" "$out/$UPDATER_FEED" || { WHY="$UPDATER_FEED"; return 1; }
  log "bundle-updater: $(asset_base).app.tar.gz + .sig + $UPDATER_FEED under $out (key $UPDATER_PUBKEY)"
}

# -------------------------------------------------------------- publish --
zo_for_triple() { # TRIPLE -> the built zo's path, or nothing
  local target p; target=$(target_dir zo-release)
  if [ "$1" = "${ZO_BUILD_TARGETS%% *}" ]; then p="$target/$ZO_PROFILE/zo"; else p="$target/$1/$ZO_PROFILE/zo"; fi
  [ -f "$p" ] && printf '%s' "$p"
  return 0
}
tag_exists() { # TAG — on the release repo (a release, or a bare tag)
  if is_dry; then [ "$(stub TAG_EXISTS 0)" = 1 ]; return; fi
  gh release view "$1" --repo "$RELEASE_GITHUB_REPO" > /dev/null 2>&1 && return 0
  [ -n "$(git -C "$RELEASE_REPO" ls-remote --tags "https://github.com/$RELEASE_GITHUB_REPO" "refs/tags/$1" 2>/dev/null)" ]
}
# The old CLI's releases live on the same repo (§2.2): ours are known by their
# assets — the feed and a ZeroCode_ archive together — never by a tag name.
beta_release_kind() { # -> none | ours | foreign
  local names
  if is_dry; then stub BETA_RELEASE none; return 0; fi
  names=$(gh release view beta --repo "$RELEASE_GITHUB_REPO" --json assets -q '.assets[].name' 2>/dev/null) || { printf none; return 0; }
  case $names in *"$UPDATER_FEED"*) case $names in *ZeroCode_*) printf ours; return 0;; esac;; esac
  printf foreign
}
# rc 0 published (or skipped, said in SKIP) · 1 red (WHY) · 3 refused (REASON set)
do_publish() {
  local out tag notes assets= names= f t bin missing= spec= prerelease=0 kind=none rc flag= log="$OUT/publish-$SHA8.log"
  [ "$RELEASE_PUBLISH" = 1 ] || { SKIP="RELEASE_PUBLISH=0"; return 0; }
  out=$(out_dir); tag="v$VERSION"; notes="$out/notes.md"
  [ -f "$out/$UPDATER_FEED" ] || { WHY="no $UPDATER_FEED under $out — bundle-updater made no feed (see its skip)"; return 1; }
  is_dry || have_tool gh || { WHY="gh is not installed"; return 1; }
  if tag_exists "$tag"; then refuse "publish: tag $tag already exists on $RELEASE_GITHUB_REPO — bump the version (tools/release/bump.sh)"; return 3; fi
  [ "$RELEASE_CHANNEL" = beta ] && { prerelease=1; flag=--prerelease; kind=$(beta_release_kind); }
  [ "$kind" != foreign ] || { refuse "publish: the beta release on $RELEASE_GITHUB_REPO is not ours (no $UPDATER_FEED + ZeroCode_ asset) — left alone"; return 3; }
  for f in "$out/$(asset_base).app.tar.gz" "$out/$(asset_base).app.tar.gz.sig" "$out/$(asset_base).dmg" "$out/$UPDATER_FEED"; do
    [ -f "$f" ] && assets="$assets $f"
  done
  # The app installer rides beside the feed it reads, so
  # `releases/latest/download/install.sh` always names the installer that
  # matches `latest.json` next to it (the README's one-line install; the
  # link answered 404 from v1.1.0 to v1.1.4, when only the retired CLI's
  # installer had ever been an asset). The legacy manifest below overwrites
  # it with the old CLI's installer, verbatim, when that manifest is on.
  cp "$APP_INSTALL_SH" "$out/install.sh" || { WHY="copy $APP_INSTALL_SH"; return 1; }
  assets="$assets $out/install.sh"
  if [ "$RELEASE_LEGACY_MANIFEST" = 1 ]; then
    for t in $ZO_BUILD_TARGETS; do
      bin=$(zo_for_triple "$t")
      if [ -n "$bin" ]; then cp "$bin" "$out/zo-$tag-$t" || { WHY="copy zo $t"; return 1; }; spec="$spec $t=$out/zo-$tag-$t"
      else missing="$missing $t"; fi
    done
    if [ -n "$missing" ]; then
      rm -f "$out"/zo-"$tag"-*
      refuse "publish: the legacy manifest needs every target and zo is missing for$missing — no half manifest (§2.7)"; return 3
    fi
    cp "$LEGACY_INSTALL_SH" "$out/install.sh" || { WHY="copy $LEGACY_INSTALL_SH"; return 1; }
    python3 "$FEED_PY" manifest --version "$VERSION" --base "https://github.com/$RELEASE_GITHUB_REPO/releases/download/$tag" $spec \
      > "$out/manifest.txt" || { WHY="manifest.txt"; return 1; }
    ( cd "$out" && python3 "$FEED_PY" sums install.sh manifest.txt $(for t in $ZO_BUILD_TARGETS; do printf 'zo-%s-%s ' "$tag" "$t"; done) ) \
      > "$out/SHA256SUMS" || { WHY="SHA256SUMS"; return 1; }
    assets="$assets $out/manifest.txt $out/SHA256SUMS"
    for t in $ZO_BUILD_TARGETS; do assets="$assets $out/zo-$tag-$t"; done
  fi
  for f in $assets; do names="${names:+$names,}$(basename "$f")"; done
  # No --target: the release repo does not hold this source (§2.2 — ours are
  # known by their assets, never by the tag's commit), so the build's sha
  # rides in the notes instead.
  printf '\n\n빌드: %s\n' "$SHA" >> "$notes"
  if is_dry; then stub_log "publish $tag repo=$RELEASE_GITHUB_REPO prerelease=$prerelease notes=$notes assets=$names"; rc=$(stub PUBLISH_RC 0)
  else heavy gh release create "$tag" --repo "$RELEASE_GITHUB_REPO" --title "$tag" --notes-file "$notes" $flag $assets > "$log" 2>&1; rc=$?
  fi
  [ "$rc" = 0 ] || { WHY="gh release create $tag rc=$rc ($log)"; return 1; }
  log "publish: $tag on $RELEASE_GITHUB_REPO prerelease=$prerelease assets=$names"
  if [ "$prerelease" != 1 ]; then
    # The public source repository carries one squashed commit per release
    # (publish-source.sh): the tag's tree without the design notes, judged by
    # the personal-data scan. A refusal is said here and does not redden the
    # lane — the release above is already out; the first snapshot of a lineage
    # is a person's `--replace-history`, once, and a red scan is the person's
    # to clean. `is_dry` keeps the publish stub sequence unchanged.
    if is_dry; then :
    else
      ( cd "$RELEASE_REPO" && bash "$SELF_DIR/publish-source.sh" "$tag" ) >> "$log" 2>&1; src_rc=$?
      if [ "$src_rc" = 0 ]; then log "publish: source snapshot $tag on the public repository"
      else log "publish: source snapshot for $tag NOT made (rc=$src_rc, $log) — a lineage nobody started with --replace-history, or a red personal-data scan"
      fi
    fi
    return 0
  fi
  # beta: the moving `beta` prerelease carries the feed, so
  # …/releases/download/beta/latest.json follows the newest beta.
  if is_dry; then stub_log "beta-move $tag from=$kind assets=$UPDATER_FEED"; rc=$(stub BETA_MOVE_RC 0)
  else
    { [ "$kind" = none ] || gh release delete beta --repo "$RELEASE_GITHUB_REPO" --yes --cleanup-tag
      git -C "$RELEASE_REPO" tag -f beta "$SHA" && git -C "$RELEASE_REPO" push -f origin refs/tags/beta \
        && gh release create beta --repo "$RELEASE_GITHUB_REPO" --prerelease --title "beta → $tag" --notes-file "$notes" "$out/$UPDATER_FEED"
    } >> "$log" 2>&1; rc=$?
  fi
  [ "$rc" = 0 ] || { WHY="beta tag move rc=$rc ($log)"; return 1; }
  log "publish: beta → $tag"
}

# --------------------------------------------------------------- one sha --
# ------------------------------------------------ which gates the sha owes --
# Each gate judges one tree: root (the window, its crates, the tools) and zo
# (zo-ide). A sha whose tree is byte for byte what the last green install of
# that half was built from — apart from the release stamp and the docs — owes
# that gate nothing: those bytes passed it then. The two halves of
# installed.json are the base (the app's sha for root, zo's for zo), so a
# folded ancestor never becomes the base. The crates zo reads from outside its
# own tree belong to both trees; they come off cargo's metadata, not a list
# kept by hand. Anything the lane cannot tell — no install yet, git or cargo
# not answering — runs the gate.
# Measured 2026-09-16 (three green runs): gate-root 1105–1615 s, gate-zo
# 876–1296 s, on releases that each changed one tree.
zo_shared_paths() { # -> repo-relative directories (trailing slash), space-separated, or nothing
  if is_dry; then # a stub of `-` is cargo saying nothing (an empty stub reads as the default)
    local s; s=$(stub ZO_SHARED_PATHS "crates/zerocode-core/ crates/model-prices/"); [ "$s" = "-" ] || printf '%s' "$s"; return 0
  fi
  # `--no-deps` needs no registry and no network: the workspace members and
  # their declared dependencies, a path dependency carrying its `path`.
  ( cd "$SCRATCH/zo-ide" && cargo metadata --format-version 1 --no-deps 2>/dev/null ) | python3 -c '
import json, os, sys
meta = json.load(sys.stdin)
scratch = os.path.realpath(sys.argv[1]); zo = os.path.join(scratch, "zo-ide")
out = set()
for package in meta["packages"]:
    for dependency in package.get("dependencies", []):
        where = dependency.get("path")
        if not where:
            continue
        where = os.path.realpath(where)
        if where == zo or where.startswith(zo + os.sep) or not where.startswith(scratch + os.sep):
            continue
        out.add(os.path.relpath(where, scratch) + "/")
print(" ".join(sorted(out)))' "$SCRATCH" 2>/dev/null
}
stamp_only() { # PATH SINCE — the file changed in version lines only
  if is_dry; then case " $(stub STAMP_REAL_CHANGES) " in *" $1 "*) return 1;; esac; return 0; fi
  ! git -C "$SCRATCH" diff -U0 "$2" "$SHA" -- "$1" 2>/dev/null \
    | grep -E '^[+-]' | grep -vE '^(\+\+\+|---)' | grep -qvE '^[+-][[:space:]]*"?version"?[[:space:]]*[:=]'
}
gate_owed() { # root|zo — 0 when the gate must run; otherwise SKIP says why and 1
  local lane=$1 half since changed shared path s owed= n
  case $lane in root) half=app ;; zo) half=zo ;; esac
  since=$(installed_part "$INSTALLED" "$half"); since=${since%% *}
  [ -n "$since" ] || return 0
  # The dry default names a shared crate, so every gate a stub run does not
  # speak about is owed — as before this rule existed.
  if is_dry; then changed=$(stub CHANGED_PATHS "crates/zerocode-core/src/lib.rs" | tr ' ' '\n')
  else changed=$(git -C "$SCRATCH" diff --name-only "$since" "$SHA" 2>/dev/null) || return 0; fi
  shared=$(zo_shared_paths); [ -n "$shared" ] || return 0
  n=0
  for path in $changed; do
    n=$(( n + 1 ))
    case " $RELEASE_STAMP_FILES " in *" $path "*) stamp_only "$path" "$since" && continue ;; esac
    case $path in docs/*|*.md) continue ;; esac
    case $lane in
      zo)   case $path in zo-ide/*) owed=$path ;; esac
            for s in $shared; do case $path in "$s"*) owed=$path ;; esac; done ;;
      root) case $path in zo-ide/*) ;; *) owed=$path ;; esac ;;
    esac
    [ -z "$owed" ] || return 0
  done
  SKIP="$lane tree unchanged since ${since:0:8} — $n path(s) changed, none its own"
  return 1
}

read_result() { # FILE -> sets RES_RC RES_SECS
  RES_RC=1; RES_SECS=0
  [ -f "$1" ] && read -r RES_RC RES_SECS < "$1"
  RES_RC=${RES_RC:-1}; RES_SECS=${RES_SECS:-0}
}
judge_sha() {
  local missing= t full root_rc zo_rc plan lane kind unit real f i rc reds=
  for t in $TOOLS; do have_tool "$t" || missing="$missing $t"; done
  if [ -n "$missing" ]; then refuse "tools:$missing"; return 3; fi
  measure_disk
  if [ "$DISK_FREE_GB" -lt "$DISK_FLOOR_GB" ]; then refuse "disk: ${DISK_FREE_GB}G free < ${DISK_FLOOR_GB}G floor"; return 3; fi
  full=$(resolve_sha "$SHA")
  if [ -z "$full" ]; then red "sha: $SHA is unknown to $RELEASE_REPO"; [ -n "$QUEUE_FILE" ] && rm -f "$QUEUE_FILE"; return 1; fi
  SHA=$full
  # Taken: from here on the sha leaves the queue whatever happens (a refusal
  # mid-way puts it back itself).
  [ -n "$QUEUE_FILE" ] && rm -f "$QUEUE_FILE"
  bow_out_if_superseded archive || return 1

  phase_begin archive
  heavy do_archive; rc=$?
  step_done archive "$rc" || return $?
  [ "$rc" = 0 ] || { red "archive rc=$rc"; return 1; }
  VERSION=$(scratch_version); write_status
  trim_targets

  # The root verify recipe completes its cargo recipes before its browser
  # harnesses. Finish that entire gate before starting zo's cargo/e2e work;
  # solo browser judgments also finish below before either build starts.
  # This is a scheduling contract, not a switch to re-enable parallel gates.
  bow_out_if_superseded gate-root || return 1
  phase_begin gate-root
  if gate_owed root; then
    heavy wait_for_calm gate-root
    heavy run_gate root
    read_result "$WORK/gate-root.result"; root_rc=$RES_RC
    [ "$root_rc" = 0 ] && release_gate_target root-gate   # no root solo will need it
    step_done gate-root "$RES_RC" "$RES_SECS" || return $?
  else
    # A gate not owed leaves an empty log: the flake judgment below reads
    # nothing from it, and the phase line says why it did not run.
    root_rc=0; : > "$OUT/gate-root-$SHA8.log"; stub_log "gate root not owed"
    step_done gate-root 0 0 || return $?
  fi
  bow_out_if_superseded gate-zo || return 1
  phase_begin gate-zo
  if gate_owed zo; then
    heavy wait_for_calm gate-zo
    heavy run_gate zo
    read_result "$WORK/gate-zo.result"; zo_rc=$RES_RC
    [ "$zo_rc" = 0 ] && release_gate_target zo-gate
    step_done gate-zo "$RES_RC" "$RES_SECS" || return $?
  else
    zo_rc=0; : > "$OUT/gate-zo-$SHA8.log"; stub_log "gate zo not owed"
    step_done gate-zo 0 0 || return $?
  fi

  bow_out_if_superseded flakes || return 1
  phase_begin flakes
  # One line per failure name — its lane, whether flakes.txt lists it, what a solo run re-runs for
  # it, the name — read line by line, so a browser check's name with spaces in it stays one name.
  plan=$(solo_plan root "$OUT/gate-root-$SHA8.log"; solo_plan zo "$OUT/gate-zo-$SHA8.log")
  real=; names=0   # unlisted names only — a listed cluster is what flakes.txt is for
  while IFS=$'\t' read -r lane kind unit f; do
    [ "$kind" != unlisted ] || names=$(( names + 1 ))
    # A recipe that failed without naming a test is real whatever else failed by name.
    [ "$kind" != recipe ] || real="$real $lane:$f"
  done <<< "$plan"
  # A red with no failure names is a build/harness failure, not a test: real.
  [ "$root_rc" = 0 ] || plan_names "$plan" root || real=" root:?$real"
  [ "$zo_rc" = 0 ] || plan_names "$plan" zo || real="$real zo:?"
  # Too many unlisted names at once is a regression, not a flake storm.
  if [ "$names" -gt "$MAX_SOLO_JUDGED" ]; then
    while IFS=$'\t' read -r lane kind unit f; do
      [ -z "$lane" ] || [ "$kind" = recipe ] || real="$real $lane:$f"
    done <<< "$plan"
  fi
  if [ -n "$real" ]; then
    step_done flakes 1
    red "gate red for real: root rc=$root_rc zo rc=$zo_rc [$(printf '%s' "$real" | sed 's/^ //')]"; return 1
  fi
  # Every remaining name is judged solo, listed or not; an unlisted one is
  # named so the person can add it to flakes.txt once it has passed solo —
  # and named again with its judgment, which $UNLISTED keeps (t-9741).
  while IFS=$'\t' read -r lane kind unit f; do
    [ "$kind" != unlisted ] || log "unlisted red $lane:$f — judged solo"
  done <<< "$plan"
  if [ "$root_rc" != 0 ] || [ "$zo_rc" != 0 ]; then
    # The solo run is the judgment — flake or real — so it gets the calm
    # machine a gate gets. It follows the zo gate immediately, and a 2px anchor
    # or a poll count judged while that gate's compile is still flushing is not
    # a judgment: on 2026-09-18 three window assertions failed in the gate at
    # load 7.75, failed again in the solo right behind it, and were green every
    # time they were asked on an idle machine.
    heavy wait_for_calm flakes-solo
    # One solo road per unit, however many of its names failed: a suite of a
    # browser harness is re-run once a round for every check of it that was red.
    # The list comes in on fd 3, so nothing a solo run starts can read it.
    while IFS=$'\t' read -r lane unit <&3; do
      [ -n "$lane" ] || continue
      for i in $(seq 1 "$SOLO_RUNS"); do
        rc=$(run_solo "$lane" "$unit" "$i"); log "solo $lane $unit $i/$SOLO_RUNS rc=$rc"
        if [ "$rc" != 0 ]; then
          say_unlisted "$plan" "$lane" "$unit" $(( i - 1 )) red
          step_done flakes 1; red "solo $lane $unit run $i rc=$rc"; return 1
        fi
      done
      say_unlisted "$plan" "$lane" "$unit" "$SOLO_RUNS" green
    done 3<<< "$(printf '%s\n' "$plan" | awk -F'\t' 'NF && $2 != "recipe" && !seen[$1 FS $3]++ { print $1 FS $3 }')"
  fi
  # A red gate's target served its solo judgments; now it too is done with.
  [ "$root_rc" = 0 ] || release_gate_target root-gate
  [ "$zo_rc" = 0 ] || release_gate_target zo-gate
  step_done flakes 0 || return $?

  phase_begin push
  heavy do_push; rc=$?
  if [ "$rc" != 0 ]; then
    if sha_on_origin; then log "push non-ff but $SHA8 is already on origin/main — building the gated sha"; rc=0
    else step_done push "$rc"; red "push: non-ff and $SHA8 is not on origin/main — nothing built"; return 1; fi
  fi
  step_done push "$rc" || return $?

  run_build app & local app_pid=$!
  run_build zo & local zo_build_pid=$!
  phase_begin build-app
  wait "$app_pid"
  read_result "$WORK/build-app.result"; step_done build-app "$RES_RC" "$RES_SECS" || return $?
  if [ "$RES_RC" = 0 ]; then
    phase_begin swap-app; heavy swap_app; rc=$?; step_done swap-app "$rc" || return $?
    [ "$rc" = 0 ] || reds="$reds swap-app"
  else reds="$reds build-app"; fi
  phase_begin build-zo
  wait "$zo_build_pid"
  read_result "$WORK/build-zo.result"; step_done build-zo "$RES_RC" "$RES_SECS" || return $?
  if [ "$RES_RC" = 0 ]; then
    phase_begin swap-zo; heavy swap_zo; rc=$?; step_done swap-zo "$rc" || return $?
    [ "$rc" = 0 ] || reds="$reds swap-zo"
  else reds="$reds build-zo"; fi
  if [ -n "$reds" ]; then red "red:$reds"; return 1; fi

  phase_begin bundle-updater
  do_bundle_updater; rc=$?
  step_done bundle-updater "$rc" || return $?
  [ "$rc" = 0 ] || { red "bundle-updater: $WHY"; return 1; }
  phase_begin publish
  do_publish; rc=$?
  step_done publish "$rc" || return $?
  [ "$rc" = 3 ] && return 3
  [ "$rc" = 0 ] || { red "publish: $WHY"; return 1; }
  OUTCOME=green; REASON=; return 0
}

sweep_sha() {
  [ "$SWEPT" = 0 ] || return 0
  SWEPT=1
  phase_begin sweep
  kill_children
  [ "$OUTCOME" = null ] && red "exit before an outcome"
  rm -rf "$SCRATCH" "$WORK"
  is_dry || git -C "$RELEASE_REPO" worktree prune > /dev/null 2>&1
  stub_log "sweep"
  phase_end sweep 0
  log "== $OUTCOME${REASON:+ ($REASON)}"
}
run_sha() { # SHA [QUEUE_FILE]
  SHA=$1; SHA8=$(printf '%s' "$1" | cut -c1-8); QUEUE_FILE=${2:-}
  SCRATCH=$RELEASE_SCRATCH_ROOT/gate-$SHA8; LOG=$OUT/lane-$SHA8.log
  STARTED_AT=$(now_iso); PHASES_JSON=; OUTCOME=null; REASON=; SWEPT=0
  VERSION=; UPDATER_PUBKEY=; SKIP=; WHY=
  rm -rf "$WORK"; mkdir -p "$WORK"
  log "== lane $SHA8 start (dry=$RELEASE_DRY_RUN)"
  [ -d "$SCRATCH" ] && log "stale scratch $SCRATCH — swept on the way out"
  judge_sha; local rc=$?
  sweep_sha
  return $rc
}

# ------------------------------------------------------------------- traps --
on_signal() { # NAME
  trap '' TERM INT HUP
  red "$1"
  exit 143
}
on_exit() {
  local rc=$?
  trap '' TERM INT HUP
  sweep_sha
  [ "$LOCK_HELD" = 1 ] && rm -rf "$LOCK"
  exit "$rc"
}
trap 'on_signal SIGTERM' TERM
trap 'on_signal SIGINT' INT
trap 'on_signal SIGHUP' HUP
trap on_exit EXIT

# -------------------------------------------------------------------- lock --
take_lock() {
  mkdir -p "$RELEASE_HOME"
  if mkdir "$LOCK" 2>/dev/null; then echo $$ > "$LOCK/pid"; LOCK_HELD=1; return 0; fi
  local pid; pid=$(cat "$LOCK/pid" 2>/dev/null)
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then echo "lane: another lane holds the lock (pid $pid) — it drains the queue"; return 1; fi
  echo "lane: stale lock (pid ${pid:-?} is gone) — taking it over"
  rm -rf "$LOCK"; mkdir "$LOCK" 2>/dev/null || return 1
  echo $$ > "$LOCK/pid"; LOCK_HELD=1
}
oldest_queued() { ls -tr "$QUEUE" 2>/dev/null | head -1; }

# -------------------------------------------------------------------- main --
take_lock || exit 0
mkdir -p "$QUEUE" "$OUT" "$TARGETS"
rm -f "$RELEASE_HOME/current-release-sha"   # the old marker nobody read (design §2.3)
rc=0
if [ $# -gt 0 ]; then
  for sha in "$@"; do run_sha "$sha"; rc=$?; [ $rc = 3 ] && break; done
else
  while next=$(oldest_queued); [ -n "$next" ]; do
    case $next in
      *[!0-9a-f]*|????????????????????????????????????????*?)  # not a sha: out of the queue so launchd stops firing
        mv -f "$QUEUE/$next" "$OUT/queue-junk-$next"; continue ;;
    esac
    run_sha "$next" "$QUEUE/$next"; rc=$?
    [ $rc = 3 ] && break
  done
fi
exit $rc
