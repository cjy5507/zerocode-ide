#!/bin/bash
# tools/release/lane.sh [<sha> ...] | --table | --plan
#
# The release lane, off the window (docs/design/release-lane-off-the-window.md).
# launchd starts it whenever the queue directory is not empty; it takes the
# lock, drains the queue oldest-first — a queued sha whose descendant is also
# queued is `superseded`, never gated (t-4123) — and for each sha: refuses on missing
# tools or a low disk, clones the sha into a scratch, trims any target over the
# cap, completes the root gate before the zo gate (each running only what the
# coordinator's gate receipt for the sha's tree left owed, when the sha carries
# one — t-47516), judges known flakes solo, pushes
# <sha>:main, builds the app and zo in parallel, swaps each beside-then-mv,
# cross-builds the Windows installer on a publish run (build-win,
# tools/release/win-installer.sh — t-113717), bundles the signed updater feed
# (bundle-updater), publishes the GitHub release when asked (publish;
# docs/design/versioned-auto-update.md §2.6), and sweeps. One trap covers
# every exit path — red, refused, SIGTERM, missing
# tool — so the scratch never outlives the lane. Each run is reported as any
# build reports itself (`zo build report`, through RELEASE_REPORT_ZO): the task
# board's build card reads that record and knows no lane.
#
# RELEASE_DRY_RUN=1 replaces every heavy step with a stub the caller steers via
# RELEASE_STUB_* (see `stub_*` below); tools/release/tests/test_lane.py pins the
# contract that way. The places below can be overridden by environment for the
# tests (RELEASE_HOME, RELEASE_REPO, RELEASE_SCRATCH_ROOT, RELEASE_APP_DIR,
# RELEASE_ZO_BIN, RELEASE_REPORT_ZO, RELEASE_FLAKES_FILE, RELEASE_NODE_MODULES).
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
RECLAIM_BELOW_GB=20         # after a target's last use in the run it is cleaned when free is under this: the
                            # build phases still need ~8 G above the floor (09-11 17:32: refused at 7 G with a
                            # finished 23 G root-gate on disk). A reclaimed gate starts cold next run (~+8 min).
CALM_LOAD=12                # a gate starts once the 1-minute load average is at or under this (the lane
                            # machine has 12 cores): at load 30–43 (two gates + a cross-compile + fseventsd,
                            # 2026-09-11) gate-root took 1697 s against 668–1179 s calm and gate-zo 22231 s
                            # against 649–1342 s, and six pty e2e timed out that pass solo
CALM_WAIT_SECS=900          # …waiting at most this long; a machine still loud after that runs and says so
CALM_POLL_SECS=30           # how often the wait looks at the load
# The steps the lane runs, in order, one a line — `name|` then what the step is called in each
# language of PHASE_LANGS (the window's, ui/shell-i18n.js LOCALES). Each run's build record and
# status.json carry the list as its plan, so a board draws this lane's steps — and any build's —
# in their own words and keeps no table of them (t-83513). PHASES is its first column.
PHASE_LANGS="ko en ja zh es"
PHASE_WORDS='archive|소스 묶기|Pack the source|ソースをまとめる|打包源码|Empaquetar el código
gate-root|결합 시험|Integration tests|結合テスト|集成测试|Pruebas de integración
gate-zo|zo 시험|zo tests|zo のテスト|zo 测试|Pruebas de zo
flakes|흔들린 시험 다시|Rerun flaky tests|不安定なテストの再実行|重跑不稳定的测试|Repetir pruebas inestables
push|올리기|Push|プッシュ|推送|Subir
build-app|앱 빌드|Build the app|アプリのビルド|构建应用|Compilar la app
swap-app|앱 바꿔 넣기|Swap in the app|アプリの入れ替え|替换应用|Cambiar la app
build-zo|zo 빌드|Build zo|zo のビルド|构建 zo|Compilar zo
swap-zo|zo 바꿔 넣기|Swap in zo|zo の入れ替え|替换 zo|Cambiar zo
build-win|Windows 설치 파일 빌드|Build the Windows installer|Windows インストーラーのビルド|构建 Windows 安装程序|Compilar el instalador de Windows
bundle-updater|업데이트 묶기|Bundle the updater|アップデーターの梱包|打包更新器|Empaquetar el actualizador
publish|발행|Publish|公開|发布|Publicar
sweep|정리|Clean up|後片付け|清理|Limpiar'
PHASES=; while IFS='|' read -r phase_name _; do PHASES="${PHASES:+$PHASES }$phase_name"; done <<< "$PHASE_WORDS"
# The build this lane is to a task board (`zo build report`): its id there, and its name in each
# language of PHASE_LANGS in that order, as PHASE_WORDS names a step.
BUILD_ID=release
BUILD_WORDS='새 버전 발행|Publishing a new version|新しいバージョンを公開|发布新版本|Publicando una versión nueva'
# A recipe of a gate that writes no line for long is said stalled, and past a cap ended red: on
# 10-29 v1.1.104's gate-zo stood 55 min in a deadlocked test, and neither its log nor the lane's
# got a line, so nobody knew (t-83513). Its usual time is the median of what it took in the
# newest QUIET_SAMPLES lanes that ended green (their gate logs' `<== verify recipe` lines).
QUIET_FACTOR=2              # silent this many times its usual time (and at least QUIET_FLOOR_SECS): 「멈춘 듯」 in the lane log and status.json
QUIET_CAP_FACTOR=3          # silent this many times its usual time (and at least QUIET_CAP_FLOOR_SECS): the recipe is ended red (its own processes, by parent id) and the gate goes on (10-29: zo's test, usual 955 s, said at 32 min, ended at 48)
QUIET_FLOOR_SECS=600        # never said sooner — a quick recipe waiting on cargo's build lock is not stalled; a recipe with no green past is said at this and never ended
QUIET_CAP_FLOOR_SECS=1200   # never ended sooner, however quick its usual time
QUIET_SAMPLES=5             # the green lanes a recipe's usual time is the median of (the board's estimates read five too)
QUIET_POLL_SECS=10          # how often the lane looks at a running gate's log
TARGET_LANES="root-gate zo-gate root-release zo-release win-release"
# The window harness (ui/tests/window.mjs) split across this many browsers in the lane's gates
# (ui/tests/window-shards.mjs). 2026-11-12, main 1ebcbba2c on this Mac: 1,723 s in one browser
# (v1.1.144's gate-root), 451 · 444 · 454 s split four ways — 6312/6312 each time, its four shares
# 436–450 s. The lane's own: `just verify` anywhere else (CI, another machine) stays one browser.
# A run that names its suites — the lane's solo judgment — is never split. A lane started with
# RELEASE_WINDOW_SHARDS=1 runs the harness in one browser again, with no commit; lane-env.sh hands
# the same value to a command run as the lane runs it.
RELEASE_WINDOW_SHARDS=${RELEASE_WINDOW_SHARDS:-4}
export WINDOW_SHARDS=$RELEASE_WINDOW_SHARDS
export CARGO_INCREMENTAL=0  # every cargo the lane runs, gates and solo judgments alike (their fingerprints must agree): a
                            # gate rebuilds the workspace crates every run (a new version, a new scratch), and the cache
                            # it leaves is the disk — root-gate held 11–12 G of its 18–26 G as incremental/, 7–8 folders a
                            # crate (one per way it is built: lib, test, clippy, doc…), and on 2026-10-01 a 22 G root-gate
                            # left 6 G free mid-lane; zo-ide's own profiles already say incremental = false (t-19990)
# The files a release commit always touches (bump.sh): a diff that changes only
# their version lines is the stamp, not a change to either tree.
RELEASE_STAMP_FILES="Cargo.toml Cargo.lock crates/zerocode-shell/tauri.conf.json zo-ide/Cargo.toml zo-ide/Cargo.lock"
# …and the release notes, where bump.sh opens the version's section. Of the gates'
# recipes only NOTES_READERS read them: pii-check scans every tracked file, and no
# test, build script or harness opens CHANGELOG.md (git grep, 2026-10-22). A
# coordinator's receipt for the tree before the notes leaves exactly these owed.
RELEASE_NOTES=CHANGELOG.md
NOTES_READERS=pii-check
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
RELEASE_WINDOWS=${RELEASE_WINDOWS:-1}          # 0 skips build-win aloud: that publish ships macOS alone, with no cold Windows build in front of it (t-113717)
UPDATER_PLATFORM=darwin-aarch64                # the latest.json platforms key this machine builds
UPDATER_ARCH=aarch64                           # ZeroCode_<version>_<arch>.app.tar.gz
UPDATER_FEED=latest.json
# The Windows installer (t-113717): on a publish run, after both swaps, the sha's own
# tools/release/win-installer.sh cross-builds zo and the window for WIN_TRIPLE here (cargo-xwin as
# tauri's runner, Homebrew's makensis — MSI needs WiX, which runs on Windows only) into the WIN_LANE
# target; bundle-updater signs it with the same updater key and adds its row to the feed.
WIN_TRIPLE=x86_64-pc-windows-msvc
WIN_ARCH=x64                                   # tauri's NSIS name for it: ZeroCode_<version>_x64-setup.exe, kept (the app takes only ZeroCode_ assets)
WIN_LANE=win-release                           # its target under $TARGETS, one of TARGET_LANES
WIN_UPDATER_PLATFORM=windows-x86_64            # its latest.json platforms key (the updater looks for windows-x86_64-nsis first, then this one)
WIN_INSTALLER_SH=tools/release/win-installer.sh   # in the sha's tree: a sha from before it builds no installer, said, never red
WIN_ROOM_GB=6                                  # free above DISK_FLOOR_GB a cold Windows build needs (estimate: root-release 3.9G + zo-release 1.2G, 11-18)
WIN_BLOCKS_PUBLISH=${WIN_BLOCKS_PUBLISH:-0}    # 0: a failed or disk-skipped build-win allows a macOS-only release, with a reason; 1: it stops the publish
ZO_BUILD_TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu"   # the manifest's rows in install.sh's order; the first is the host
BUILD_APP_CONFIG='{"bundle":{"createUpdaterArtifacts":false}}'   # the local install build makes no updater artifacts (tauri-cli refuses the key without plugins.updater); bundle-updater is the phase that does
COMPUTER_USE_HELPER='ZeroCode Computer Use.app'   # native UI helper under bundle Resources
APP_SIBLING_BINS='zerocode-mirror zerocode-pick'   # helper executables beside the shell in Contents/MacOS (Cargo.toml [[bin]]); tauri-cli 2.11.4 once dropped the second inferred one
APP_BIN=zerocode-shell                            # the window's own executable in Contents/MacOS
APP_ZO=Resources/bin/zo                           # the zo the app carries, under Contents (tauri.release.conf.json bundle.resources)
# The Google sign-in client's secret (the window's Antigravity login and zo's `/login google`):
# not in the source since audit 40-26 (t-105752). A build reads it from this variable at compile
# time (option_env!), and the lane gives it ONLY to the builds it installs and publishes — never
# to a gate. Its one home is the login keychain, as a generic password under this service name.
# After a build that carries the value, the release build folders ($RELEASE_HOME/target/root-release
# and zo-release) still hold it as text in their dep-info (.d) and fingerprint files: rustc writes
# the option_env! value on an `# env-dep:` line.
GOOGLE_OAUTH_SECRET_ENV=ZEROCODE_GOOGLE_OAUTH_CLIENT_SECRET
GOOGLE_OAUTH_SECRET_ITEM=dev.zerocode.release.google-oauth-client-secret
SECURITY_BIN=/usr/bin/security

SELF_DIR=$(cd "$(dirname "$0")" && pwd)
RELEASE_REPO=${RELEASE_REPO:-$(cd "$SELF_DIR/../.." && pwd)}
# The repository this lane releases, as git names the folder every checkout of it shares
# (`--git-common-dir`). status.json says it, and a board shows the lane only on a project whose
# checkout shares that very folder — the window tells it by the folder's identity on disk, never
# by how a path is spelled or what the repository is called (t-83513).
RELEASE_GIT_COMMON_DIR=${RELEASE_GIT_COMMON_DIR:-$(git -C "$RELEASE_REPO" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)}
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
# The zo each run is reported through (`zo build report`): the installed one. A dry run's steps are
# stubs, and reach no board unless the run is given a zo to report through.
RELEASE_REPORT_ZO=${RELEASE_REPORT_ZO-$([ "$RELEASE_DRY_RUN" = 1 ] || printf '%s' "$RELEASE_ZO_BIN")}
RELEASE_STUB_LOG=${RELEASE_STUB_LOG:-}
UPDATER_KEY=${UPDATER_KEY:-$RELEASE_HOME/keys/updater.key}   # `tauri signer generate -w`; its .pub beside it; never in the tree (README.md)
UPDATER_KEY_PASSWORD_FILE=${UPDATER_KEY_PASSWORD_FILE:-$UPDATER_KEY.password}   # optional; absent means the key has no password
FEED_PY=$SELF_DIR/updater_feed.py
UNITS_PY=$SELF_DIR/workspace_units.py   # drops the repository's own build units, and build-script runs copied in from another target, from a target (trim_targets)
RECEIPT_PY=$SELF_DIR/coord_receipt.py   # reads a coordinator's gate receipt again (receipt_cover); enqueue.sh reads it at the door
# A receipt's jobs are ones the build line ran — `zo line`, whose folder is zo's home's line/
# (crates/zerocode-core/src/line.rs): each job's record in its jobs/, passed with rc 0, its log
# and rc file in its logs/. Their owner and run are only what the jobs say of themselves, held to
# the coordinator's: coord-<label> of RELEASE_COORD_RUN, or its -root and -zo jobs (t-47516,
# t-76396, t-119757). The run has no default: the coordinator's launch names its own, and a lane
# given none takes no receipt (the old default, run-4275, outlived its run and refused every one).
RELEASE_BUILD_LINE=${RELEASE_BUILD_LINE:-${ZO_HOME:-$HOME/.zo}/line}
RELEASE_COORD_RUN=${RELEASE_COORD_RUN:-}
LEGACY_INSTALL_SH=$SELF_DIR/legacy/install.sh   # the old CLI's installer, verbatim (§2.7)
APP_INSTALL_SH=$SELF_DIR/install.sh             # the app's installer: reads the feed beside it, so it goes up with every release
# -------------------------------------------------------------------------------
# The root gate's browser harnesses (ui/tests/*.mjs) resolve Playwright through
# NODE_PATH, not only through the node_modules symlink the scratch gets.
export NODE_PATH=$RELEASE_NODE_MODULES
# …and no gate builds with the Google client's secret, whatever the caller's environment held:
# only the builds below are given it (with_oauth_secret).
unset "$GOOGLE_OAUTH_SECRET_ENV"

QUEUE=$RELEASE_HOME/queue
LOCK=$RELEASE_HOME/lock
OUT=$RELEASE_HOME/out
TARGETS=$RELEASE_HOME/target
WORK=$RELEASE_HOME/work
STATUS=$RELEASE_HOME/status.json
INSTALLED=$RELEASE_HOME/installed.json
INSTALLED_PREV=$RELEASE_HOME/installed.prev.json
UNLISTED=$RELEASE_HOME/unlisted.txt   # every unlisted red the lane judged solo, with its result: the evidence a flakes.txt line is written from
RECEIPTS=$RELEASE_HOME/receipts       # <sha>.json: the coordinator's gate receipt a queued sha carries (enqueue.sh --receipt)
PASSED=$RELEASE_HOME/passed           # <sha>-<root|zo>: a gate this lane ran whole on that sha and saw green — the sha's next run skips it (gate_passed)

if [ "${1:-}" = "--table" ]; then
  for k in DISK_FLOOR_GB TARGET_CAP_GB RECLAIM_BELOW_GB POLL_SECS THROTTLE_SECS ZO_PROFILE SOLO_RUNS CALM_LOAD CALM_WAIT_SECS \
           CALM_POLL_SECS PHASES PHASE_LANGS BUILD_ID BUILD_WORDS QUIET_FACTOR QUIET_CAP_FACTOR QUIET_FLOOR_SECS QUIET_CAP_FLOOR_SECS QUIET_SAMPLES QUIET_POLL_SECS \
           RELEASE_GIT_COMMON_DIR TARGET_LANES CARGO_INCREMENTAL WINDOW_SHARDS TOOLS \
           LAUNCHD_LABEL LAUNCHD_PATH APP_NAME RELEASE_REPO RELEASE_HOME RELEASE_SCRATCH_ROOT RELEASE_APP_DIR \
           RELEASE_ZO_BIN RELEASE_REPORT_ZO RELEASE_FLAKES_FILE QUEUE LOCK OUT STATUS INSTALLED INSTALLED_PREV UNLISTED RECEIPTS PASSED RECEIPT_PY \
           RELEASE_BUILD_LINE RELEASE_COORD_RUN RELEASE_NOTES NOTES_READERS UI_SUITE_CHOOSER \
           RELEASE_GITHUB_REPO RELEASE_PUBLISH RELEASE_CHANNEL RELEASE_LEGACY_MANIFEST RELEASE_WINDOWS UPDATER_KEY \
           UPDATER_PLATFORM UPDATER_ARCH UPDATER_FEED ZO_BUILD_TARGETS COMPUTER_USE_HELPER APP_SIBLING_BINS APP_BIN APP_ZO \
           WIN_TRIPLE WIN_ARCH WIN_LANE WIN_UPDATER_PLATFORM WIN_INSTALLER_SH WIN_ROOM_GB WIN_BLOCKS_PUBLISH \
           GOOGLE_OAUTH_SECRET_ENV GOOGLE_OAUTH_SECRET_ITEM SECURITY_BIN; do
    eval "printf \"%s='%s'\\n\" \"$k\" \"\$$k\""
  done
  exit 0
fi

# plan_json — PHASE_WORDS as status.json's `plan`: [{"name":…,"words":{"<lang>":…}}, …] in
# PHASE_LANGS' order, built once (no process per word). The words are the table's own, with
# no quote or backslash in them; those two are escaped anyway.
plan_json() {
  local line cells lang word i steps= words
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    IFS='|' read -r -a cells <<< "$line"
    words=; i=1
    for lang in $PHASE_LANGS; do
      word=${cells[$i]:-}; word=${word//\\/\\\\}; word=${word//\"/\\\"}
      words="${words:+$words,}\"$lang\":\"$word\""; i=$(( i + 1 ))
    done
    steps="${steps:+$steps,}{\"name\":\"${cells[0]}\",\"words\":{$words}}"
  done <<< "$PHASE_WORDS"
  printf '[%s]' "$steps"
}
PLAN_JSON=$(plan_json)
if [ "${1:-}" = "--plan" ]; then printf '%s\n' "$PLAN_JSON"; exit 0; fi

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
ONLY=; RECEIPT=   # a gate's recipes a coordinator's receipt left owed, and what that receipt did (receipt_cover)
RECEIPT_JUDGED=   # the failures a receipt that stood cleared solo for a gate it covered, as solo_plan writes them (flake_reals counts them)
STALLED=null      # status.json's `stalled` while a gate's recipe writes nothing past its say time (watch_gate), else null
WIN_INSTALLER=    # this run's Windows installer (build-win): the only one bundle-updater signs, never one an earlier run left
WIN_SHIPPED=0     # 1 once bundle-updater put that installer, its .sig and its feed row under out/ — publish sends the three or none
WIN_RED=          # why build-win left the release macOS-only (status.json, the end line, the board)
DRY_DISK_FREE_GB=  # a stubbed reading after a target was cleaned in this phase

measure_disk() {
  if is_dry; then
    local phase_key
    phase_key=$(printf '%s' "${PHASE:-}" | tr '[:lower:]-' '[:upper:]_')
    DISK_FREE_GB=${DRY_DISK_FREE_GB:-$(stub "DISK_FREE_GB_$phase_key" "$(stub DISK_FREE_GB 100)")}
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
  printf '{"sha":"%s","phase":"%s","started_at":"%s","updated_at":"%s","phases":[%s],"disk_free_gb":%s,"outcome":%s,"reason":"%s","version":"%s","updater_pubkey":"%s","git_common_dir":"%s","plan":%s,"stalled":%s}\n' \
    "$SHA" "$PHASE" "$STARTED_AT" "$(now_iso)" "$PHASES_JSON" "$DISK_FREE_GB" "$outcome" "$(json_str "$REASON")" \
    "$(json_str "$VERSION")" "$(json_str "$UPDATER_PUBKEY")" "$(json_str "$RELEASE_GIT_COMMON_DIR")" "$PLAN_JSON" "$STALLED" \
    > "$STATUS.tmp" && mv -f "$STATUS.tmp" "$STATUS"
}
phase_begin() {
  PHASE=$1; PHASE_T0=$(now_s); DRY_DISK_FREE_GB=
  write_status; report_build step --start "$PHASE"; log "== $PHASE"
}
phase_end() { # NAME RC [SECS] — a phase that set SKIP did nothing, and says why; a gate a receipt shaped says what it did (RECEIPT)
  local name=$1 rc=$2 secs=${3:-$(( $(now_s) - PHASE_T0 ))} said=
  if [ -n "$SKIP" ]; then said=",\"skipped\":\"$(json_str "$SKIP")\""
  elif [ -n "$RECEIPT" ]; then said=",\"receipt\":\"$(json_str "$RECEIPT")\""; fi
  PHASES_JSON="${PHASES_JSON:+$PHASES_JSON,}{\"name\":\"$name\",\"rc\":$rc,\"secs\":$secs$said}"
  measure_disk; write_status
  report_build step --end "$name" --rc "$rc" --secs "$secs" ${SKIP:+--skipped "$SKIP"}
  log "$name rc=$rc ${secs}s free=${DISK_FREE_GB}G load=$(load_now | tr ' ' /)${SKIP:+ skipped: $SKIP}${RECEIPT:+ receipt: $RECEIPT}"
  SKIP=; RECEIPT=
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

# -------------------------------------------------------------- build record --
# Each run, as any build reports itself (`zo build report`): the task board's build card draws a
# project's builds from those records alone and knows no lane. status.json stays, for the schedule
# and the queue's tools. A zo that cannot take the report (none, or one older than the verb) turns
# the reporting off for the run at its first word: the lane never waits on a board, nor fails for one.
REPORT=0
report_build() { # VERB [FLAG VALUE ...] — one change to this run's record
  [ "$REPORT" = 1 ] || return 0
  "$RELEASE_REPORT_ZO" build report "$1" --build "$BUILD_ID" --cwd "$RELEASE_REPO" "${@:2}" < /dev/null > /dev/null 2>&1
  return 0
}
report_begin() { # the run's record: the lane's name in each language, the sha, its log, and the plan its steps follow
  local words lang at=0
  REPORT=0
  [ -n "$RELEASE_REPORT_ZO" ] && [ -x "$RELEASE_REPORT_ZO" ] || return 0
  IFS='|' read -r -a words <<< "$BUILD_WORDS"
  set --
  for lang in $PHASE_LANGS; do set -- "$@" "--name-$lang" "${words[$at]:-}"; at=$(( at + 1 )); done
  "$RELEASE_REPORT_ZO" build report begin --build "$BUILD_ID" --cwd "$RELEASE_REPO" "$@" --sha "$SHA" --log "$LOG" \
    --plan-file - \
    <<< "$PLAN_JSON" > /dev/null 2>&1 && REPORT=1
  return 0
}
report_end() { # how the run ended, in the record's words: green passed, red failed, the rest cancelled
  local outcome=cancelled
  case $OUTCOME in green) outcome=passed ;; red) outcome=failed ;; esac
  report_build end --outcome "$outcome" --reason "$REASON"
}
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
  if is_dry; then
    rm -rf "$(target_dir "$1")" || return 1
    DRY_DISK_FREE_GB=$(stub "DISK_FREE_GB_AFTER_CLEAN_$(upper "$1")" "$DRY_DISK_FREE_GB")
  else CARGO_TARGET_DIR=$(target_dir "$1") cargo clean --manifest-path "$SCRATCH/Cargo.toml" > /dev/null 2>&1 || rm -rf "$(target_dir "$1")"
  fi
  mkdir -p "$(target_dir "$1")"
}
# release_gate_target LANE [NEEDED_GB] — clean a target after its last use, under
# the reclaim threshold. A caller that needs less room can stop reclaiming sooner.
release_gate_target() {
  local kb below=${2:-$RECLAIM_BELOW_GB}
  [ "$below" -le "$RECLAIM_BELOW_GB" ] || below=$RECLAIM_BELOW_GB
  measure_disk
  [ "$DISK_FREE_GB" -lt "$below" ] || return 0
  # What goes is said, the folder and its size, before it goes.
  kb=$(target_kb "$1"); kb=${kb:-0}
  log "released $1 ($(target_dir "$1"), $(( kb / 1024 ))M) — ${DISK_FREE_GB}G free < ${below}G after its last use in this run"
  clean_target "$1"
}
reclaim_finished_targets() { # NEEDED_GB — before Windows, only release targets whose last phase is past
  local needed=$1 lane last_use before=" $PHASES "
  before=${before%%" $PHASE "*}
  for lane in $TARGET_LANES; do
    [ "$DISK_FREE_GB" -lt "$needed" ] || break
    case $lane in
      root-gate|zo-gate) last_use=flakes ;;
      root-release) last_use=bundle-updater ;;  # the sealed app or the distribution build
      zo-release)
        last_use=swap-zo
        if [ "$RELEASE_LEGACY_MANIFEST" = 1 ]; then
          last_use=publish
        elif [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && [ "${ZO_STAGE_TARGET_DIR:-}" = "$(target_dir "$lane")" ]; then
          last_use=bundle-updater
        fi ;;
      "$WIN_LANE") last_use=build-win ;;
      *) continue ;;
    esac
    case " $before " in
      *" $last_use "*) release_gate_target "$lane" "$needed"; measure_disk ;;
    esac
  done
}
# drop_incremental LANE — the incremental caches earlier runs left in LANE's target
# (<profile>/incremental, and <triple>/<profile>/incremental for a cross build such as
# win-check's): under CARGO_INCREMENTAL=0 nothing reads or writes them again.
drop_incremental() {
  local dir kb; dir=$(target_dir "$1")
  kb=$(find "$dir" -mindepth 2 -maxdepth 3 -type d -name incremental -prune -exec du -sk {} + 2>/dev/null \
         | awk '{ s += $1 } END { print s + 0 }')
  [ "$kb" -gt 0 ] || return 0
  find "$dir" -mindepth 2 -maxdepth 3 -type d -name incremental -prune -exec rm -rf {} + 2>/dev/null
  stub_log "drop-incremental $1"
  log "target $1: removed $(( kb / 1024 ))M of incremental cache nothing reads (CARGO_INCREMENTAL=$CARGO_INCREMENTAL)"
}
# drop_workspace_units LANE — every unit of a crate this repository defines. The
# scratch is a fresh clone, so each run rebuilds those units anyway, and the ones an
# earlier version left (the version stamp moves their hash) are never read again: on
# 10-13 they were 10.4 of zo-gate's 12.4 G and 11.4 of root-gate's 19 G, and the cap's
# clean behind them cost v1.1.62's zo gate 245 registry crates. Registry units stay warm.
drop_workspace_units() {
  local out units bytes
  if is_dry; then out=$(python3 "$UNITS_PY" drop --target "$(target_dir "$1")" --names "$(stub WORKSPACE_PACKAGES)")
  else out=$(python3 "$UNITS_PY" drop --target "$(target_dir "$1")" --scratch "$SCRATCH")
  fi || { log "target $1: workspace units left in place, $UNITS_PY failed"; return 0; }
  read -r units bytes <<< "$out"
  [ "${units:-0}" -gt 0 ] 2>/dev/null || return 0
  stub_log "drop-workspace-units $1"
  log "target $1: removed $units workspace unit(s), $(( ${bytes:-0} / 1048576 ))M — the fresh scratch rebuilds them anyway"
}
# drop_foreign_runs LANE — every build-script run that ran in another target (its `root-output` is not
# its own `out`): a target copied in from a checkout. Cargo rewrites only the OUT_DIR of a copied run's
# output, never the paths its script wrote into files (2026-11-10: a worktree's target copied into
# root-gate kept tauri's permission lists naming that worktree, which was then removed, and v1.1.140's
# root gate stopped on them); without their folders and fingerprints they run again here.
drop_foreign_runs() {
  local out units bytes
  out=$(python3 "$UNITS_PY" foreign --target "$(target_dir "$1")") \
    || { log "target $1: build-script runs left unread, $UNITS_PY failed"; return 0; }
  read -r units bytes <<< "$out"
  [ "${units:-0}" -gt 0 ] 2>/dev/null || return 0
  stub_log "drop-foreign-runs $1"
  log "target $1: removed $units build-script run(s) that ran in another target (copied in) — they run again here"
}
trim_targets() {
  local lane kb cap_kb=$(( TARGET_CAP_GB * 1024 * 1024 ))
  for lane in $TARGET_LANES; do
    mkdir -p "$(target_dir "$lane")"
    drop_foreign_runs "$lane"
    drop_incremental "$lane"
    drop_workspace_units "$lane"
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

# ---------------------------------------------------------------- the watch --
# A gate's recipe that writes nothing for long (t-83513). While a gate runs the lane looks at its
# log every QUIET_POLL_SECS: the recipe is the log's last `==> verify recipe` mark, and its silence
# is how long the log has not grown. Silent past its say time, the lane log says `stalled: <gate>
# <recipe> — <span> without output` and status.json carries `stalled` (the board's 「멈춘 듯」);
# past its cap the lane sends the gate SIGUSR1, and verify-every-recipe.sh --stoppable ends that
# recipe's own processes red and goes on to the next recipe. The lane signals only the gate it
# started, by the pid it holds; nothing is found by name. Output that comes back takes the word back.

# lane_verdict LANE_LOG -> the outcome its newest run ended with (`HH:MM:SS == <outcome>`), or nothing
lane_verdict() { sed -nE 's/^[0-9:]+ == (green|red|refused|superseded)( .*)?$/\1/p' "$1" 2>/dev/null | tail -1; }
# recipe_usuals GATE(root|zo) -> "<recipe> <secs>" a line: the median a recipe took in the gate logs
# of the newest QUIET_SAMPLES lanes that ended green (a gate not owed left its log empty and is
# passed over); the upper middle of an even count, as the board's medians take it.
recipe_usuals() {
  local gate=$1 f sha8 n=0 logs=()
  for f in $(ls -t "$OUT"/lane-*.log 2>/dev/null); do
    [ "$(lane_verdict "$f")" = green ] || continue
    sha8=${f##*/lane-}; sha8=${sha8%.log}
    [ -s "$OUT/gate-$gate-$sha8.log" ] || continue
    logs+=("$OUT/gate-$gate-$sha8.log"); n=$(( n + 1 ))
    [ "$n" -lt "$QUIET_SAMPLES" ] || break
  done
  [ "$n" -gt 0 ] || return 0
  awk '$1 == "<==" && $2 == "verify" && $3 == "recipe" && $5 == "rc=0" && $6 ~ /^[0-9]+s$/ { print $4, $6 + 0 }' "${logs[@]}" \
    | sort -k1,1 -k2,2n \
    | awk '$1 != r { if (r != "") print r, v[int(n / 2) + 1]; r = $1; n = 0 } { v[++n] = $2 } END { if (r != "") print r, v[int(n / 2) + 1] }'
}
quiet_floor() { if is_dry; then stub QUIET_FLOOR_SECS "$QUIET_FLOOR_SECS"; else printf '%s' "$QUIET_FLOOR_SECS"; fi; }
quiet_cap_floor() { if is_dry; then stub QUIET_CAP_FLOOR_SECS "$QUIET_CAP_FLOOR_SECS"; else printf '%s' "$QUIET_CAP_FLOOR_SECS"; fi; }
quiet_poll() { if is_dry; then stub QUIET_POLL_SECS "$QUIET_POLL_SECS"; else printf '%s' "$QUIET_POLL_SECS"; fi; }
# quiet_limits RECIPE USUALS -> "<say> <cap> <usual>" seconds: said at QUIET_FACTOR times its usual
# time and at least the floor, ended at QUIET_CAP_FACTOR times it and at least the cap's floor; with
# no green past it is said at the floor and never ended (cap and usual 0) — nothing says what is
# long for it.
quiet_limits() {
  local usual say cap floor cap_floor
  floor=$(quiet_floor); cap_floor=$(quiet_cap_floor)
  usual=$(printf '%s\n' "$2" | awk -v r="$1" '$1 == r { print $2; exit }')
  if [ -z "$usual" ]; then printf '%s 0 0' "$floor"; return 0; fi
  say=$(( usual * QUIET_FACTOR )); [ "$say" -ge "$floor" ] || say=$floor
  cap=$(( usual * QUIET_CAP_FACTOR )); [ "$cap" -ge "$cap_floor" ] || cap=$cap_floor
  printf '%s %s %s' "$say" "$cap" "$usual"
}
span() { if [ "$1" -ge 60 ]; then printf '%dm' $(( $1 / 60 )); else printf '%ds' "$1"; fi; }
# watch_gate PID LOG — runs beside the gate (inside run_gate, itself under heavy) until PID is gone.
# Its status.json writes carry the phase's own state as the lane had it when the gate began.
watch_gate() {
  local pid=$1 log=$2 poll usuals size=-1 grown mark recipe= at now quiet say=0 cap=0 usual=0 said= ended= limits
  poll=$(quiet_poll); usuals=$(recipe_usuals "${PHASE#gate-}"); at=$(now_s)
  while kill -0 "$pid" 2>/dev/null; do
    sleep "$poll"
    now=$(now_s); grown=$(( $(wc -c < "$log" 2>/dev/null) + 0 ))
    if [ "$grown" != "$size" ]; then
      size=$grown; at=$now
      mark=$(grep -a '^==> verify recipe ' "$log" 2>/dev/null | tail -1); mark=${mark#==> verify recipe }
      # The lines the gate writes for a recipe the lane ended (`<==`, `was terminated`) are
      # not its output come back.
      if [ -n "$said" ] && { [ -z "$ended" ] || [ "$mark" != "$recipe" ]; }; then
        said=; STALLED=null; write_status; report_build stalled --clear
        [ "$mark" != "$recipe" ] || log "stalled: $PHASE $recipe — output again"
      fi
      if [ "$mark" != "$recipe" ]; then
        recipe=$mark; ended=
        limits=$(quiet_limits "$recipe" "$usuals"); read -r say cap usual <<< "$limits"
      fi
      continue
    fi
    [ -n "$recipe" ] || continue
    quiet=$(( now - at ))
    if [ -z "$said" ] && [ "$quiet" -ge "$say" ]; then
      said=1
      STALLED="{\"gate\":\"$PHASE\",\"recipe\":\"$(json_str "$recipe")\",\"quiet_since_ms\":$(( at * 1000 )),\"cap_secs\":$cap}"
      write_status
      # Not past the gate's end: the step after it would wear this one's silence.
      kill -0 "$pid" 2>/dev/null &&
        report_build stalled --step "$PHASE" --what "$recipe" --quiet-since-ms $(( at * 1000 )) --cap-secs "$cap"
      stub_log "stalled $PHASE $recipe"
      log "stalled: $PHASE $recipe — $(span "$quiet") without output$([ "$usual" -gt 0 ] && printf ' (usual %s)' "$(span "$usual")")$([ "$cap" -gt 0 ] && printf '; ended red at %s' "$(span "$cap")")"
    fi
    if [ -z "$ended" ] && [ "$cap" -gt 0 ] && [ "$quiet" -ge "$cap" ]; then
      ended=1
      stub_log "quiet-end $PHASE $recipe"
      log "stalled: $PHASE $recipe — $(span "$quiet") without output, past its cap: ending it red"
      kill -USR1 "$pid" 2>/dev/null
    fi
  done
}

# gate LANE — runs in the background; writes "rc secs" to $WORK/gate-LANE.result.
# Every recipe of the gate, or ONLY those a coordinator's receipt left owed, each one watched
# for silence while it runs (watch_gate). A dry run steered by RELEASE_STUB_GATE_<LANE>_JUSTDIR
# drives the real gate script over that folder's justfile instead of the stub, so the watch's
# cases run the very road a lane takes.
gate_dir() { case $1 in root) printf . ;; zo) printf zo-ide ;; esac; }   # the gate's justfile, in the scratch
run_gate() {
  local lane=$1 t0 rc log="$OUT/gate-$1-$SHA8.log" target dir pid
  target=$(target_dir "$lane-gate"); t0=$(now_s)
  dir=$(stub "GATE_$(upper "$lane")_JUSTDIR")
  if is_dry && [ -z "$dir" ]; then
    stub_log "gate $lane target=$target incremental=${CARGO_INCREMENTAL-unset} shards=${WINDOW_SHARDS-unset}${ONLY:+ only=$ONLY}"
    sleep "$(stub "GATE_$(upper "$lane")_SLEEP" 0)"
    rc=$(stub "GATE_$(upper "$lane")_RC" 0)
    { echo "stub gate $lane"; if [ "$rc" != 0 ]; then echo "==> verify recipe test"; echo "failures:"; local f; for f in $(stub "GATE_$(upper "$lane")_FAILS"); do echo "    $f"; done; echo "test result: FAILED."; fi
      local h r; h=$(stub "GATE_$(upper "$lane")_HARNESS" ""); r=$(stub "GATE_$(upper "$lane")_REPORT" "")
      [ -z "$h" ] || { echo "==> verify recipe $h-browser-test"; [ -z "$r" ] || cat "$r"; echo "error: recipe \`$h-browser-test\` failed on line 41 with exit code 1"; }
      local u; u=$(stub "GATE_$(upper "$lane")_UNNAMED" ""); [ -z "$u" ] || { echo "==> verify recipe $u"; echo "error: recipe \`$u\` failed on line 43 with exit code 101"; }
    } > "$log"
  else
    if is_dry; then stub_log "gate $lane target=$target incremental=${CARGO_INCREMENTAL-unset} shards=${WINDOW_SHARDS-unset} justdir${ONLY:+ only=$ONLY}"
    else dir="$SCRATCH/$(gate_dir "$lane")"; fi
    # exec: the pid the lane holds is the gate script's own, the one process it ever signals.
    ( cd "$dir" && CARGO_TARGET_DIR=$target exec "$SELF_DIR/verify-every-recipe.sh" --stoppable ${ONLY:+--only "$ONLY"} ) > "$log" 2>&1 &
    pid=$!
    watch_gate "$pid" "$log"
    wait "$pid"; rc=$?
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
                 # judge it (1.3.37: shell-test died compiling beside a named harness red) — and for
                 # every recipe ended by a signal (`was terminated`: the lane's cap, or just's words for
                 # any signal), names or not: what would have run after the stop never spoke, so a solo
                 # pass of the names it gave clears nothing (t-83513). A recipe whose own `<== verify
                 # recipe <name> rc=<n>` says it ended red, and whose own output named nothing — not even
                 # just's words for it (just killed, its log cut short by a full disk) — is `recipe:<name>`
                 # too: another recipe's names, judged solo, never clear it (t-121630, the review's P-1).
                 # So is one whose closing line says a signal ended it (rc 128 or more), names or not, as
                 # for `was terminated` above: just died before it could say so (t-123004, the review's R2).
                 # A failures block never runs on into the next recipe's output.
  awk '
    /^==> verify recipe / {
      named = 0; said = 0; inblock = 0; harness = ""; suite = ""; checks = ""
      if ($4 ~ /^[a-z]+-browser-test$/) { harness = $4; sub(/-browser-test$/, "", harness) }
      next
    }
    /^<== verify recipe / {
      if ($5 ~ /^rc=/ && $5 != "rc=0" && (!said || substr($5, 4) + 0 >= 128)) { print "recipe:" $4; said = 1 }
      next
    }
    /^failures:$/ { inblock = 1; next }
    inblock && /^test result/ { inblock = 0; next }
    inblock && /^    [A-Za-z0-9_:]+$/ { sub(/^    /, ""); print; named = 1; said = 1; next }
    harness != "" && /^SUITE  / { suite = substr($0, 8); next }
    harness != "" && /^FAIL  / {
      check = substr($0, 7); cut = index(check, "  — "); if (cut) check = substr(check, 1, cut - 1)
      gsub(/\t/, " ", check)
      checks = checks "ui:" harness (suite == "" ? "" : "/" suite) ":" check "\n"
      next
    }
    /recipe `[^`]+` (failed|was terminated)/ {
      match($0, /recipe `[^`]+`/); r = substr($0, RSTART + 8, RLENGTH - 9)
      said = 1
      if ($0 ~ /was terminated/) { printf "%s", checks; checks = ""; print "recipe:" r }
      else if (r ~ /^[a-z]+-browser-test$/) {
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
say_unlisted_ahead() { # PLAN — each unlisted name of PLAN said in the log before the solo runs judge it
  local lane kind unit f
  while IFS=$'\t' read -r lane kind unit f; do
    [ "$kind" != unlisted ] || log "unlisted red $lane:$f — judged solo"
  done <<< "$1"
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
# solo_road LANE NAME TOP TARGET — one solo run of NAME, as the flakes phase runs one, from the checkout TOP,
# cargo building in TARGET (the gate's own target, so nothing is built again; none: cargo's own place).
solo_road() {
  local lane=$1 name=$2 top=$3 target=$4
  case "$lane:$name" in
    # A node harness is judged by the very recipe the gate ran, so a new harness
    # needs no line here: whole when its checks have no suite to stand in (or it
    # named none), and one suite alone when the check that failed named its suite.
    root:harness:*) ( cd "$top" && just "${name#harness:}-browser-test" ) ;;
    root:ui:*) ( cd "$top" && ui_solo "${name#ui:}" ) ;;
    root:*) ( cd "$top" && { [ -z "$target" ] || export CARGO_TARGET_DIR="$target"; } && cargo test --workspace "$name" ) ;;
    zo:*)   ( cd "$top/zo-ide" && { [ -z "$target" ] || export CARGO_TARGET_DIR="$target"; } \
                && ZO_DISABLE_KEYCHAIN=1 cargo test --workspace "$name" ) ;;
  esac
}
run_solo() { # LANE NAME RUN AT -> prints rc (AT, the unit's place among its gate's units, names a coordinator's log alone)
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
  solo_road "$lane" "$name" "$SCRATCH" "$target" > "$log" 2>&1; rc=$?
  echo "$rc"
}
# flake_reals PLAN ROOT_RC ZO_RC [JUDGED] — what the flakes phase calls red for real in PLAN (solo_plan's lines
# of both gates) under those gates' ends, " <lane>:<name>" each, before any solo run; nothing when every name
# may be judged solo. JUDGED: the plan lines a coordinator's receipt already cleared solo for a gate it covered
# (receipt_cover) — not judged again, but counted toward MAX_SOLO_JUDGED as if this run had met them, since
# the bound is over both gates together. The lane's own flakes phase and a coordinator's gate (lane.sh
# --judge-flakes) read it alike.
flake_reals() {
  local plan=$1 lane kind unit f real= names=0   # unlisted names only — a listed cluster is what flakes.txt is for
  while IFS=$'\t' read -r lane kind unit f; do
    [ "$kind" != unlisted ] || names=$(( names + 1 ))
    # A recipe that failed without naming a test is real whatever else failed by name.
    [ "$kind" != recipe ] || real="$real $lane:$f"
  done <<< "$plan"
  while IFS=$'\t' read -r lane kind unit f; do
    [ "$kind" != unlisted ] || names=$(( names + 1 ))
  done <<< "${4-}"
  # A red with no failure names is a build/harness failure, not a test: real.
  [ "$2" = 0 ] || plan_names "$plan" root || real=" root:?$real"
  [ "$3" = 0 ] || plan_names "$plan" zo || real="$real zo:?"
  # Too many unlisted names at once is a regression, not a flake storm.
  if [ "$names" -gt "$MAX_SOLO_JUDGED" ]; then
    while IFS=$'\t' read -r lane kind unit f; do
      [ -z "$lane" ] || [ "$kind" = recipe ] || real="$real $lane:$f"
    done <<< "$plan"
  fi
  printf '%s' "$real"
}
# solo_units PLAN -> "<lane>\t<unit>", one solo road per unit however many of its names failed (a suite of a
# browser harness is re-run once a round for every check of it that was red), in the plan's order.
solo_units() { printf '%s\n' "$1" | awk -F'\t' 'NF && $2 != "recipe" && !seen[$1 FS $3]++ { print $1 FS $3 }'; }
# solo_judge PLAN — the judgment itself: each unit of PLAN run alone SOLO_RUNS times through run_solo, in the
# plan's order, each run said as `solo <lane> <unit> <run>/<SOLO_RUNS> rc=<rc>` in the log, each unit's
# unlisted names said with how it went (say_unlisted); the first red run ends it. 0 when every run of every
# unit was green; 1 at a red one, SOLO_RED saying which. One count, one order, one stop for the lane's flakes
# phase and a coordinator's gate alike (lane.sh --judge-flakes, t-121630).
solo_judge() {
  local lane unit i rc at root_at=0 zo_at=0
  SOLO_RED=
  # The list comes in on fd 3, so nothing a solo run starts can read it.
  while IFS=$'\t' read -r lane unit <&3; do
    [ -n "$lane" ] || continue
    if [ "$lane" = root ]; then root_at=$(( root_at + 1 )); at=$root_at; else zo_at=$(( zo_at + 1 )); at=$zo_at; fi
    for i in $(seq 1 "$SOLO_RUNS"); do
      rc=$(run_solo "$lane" "$unit" "$i" "$at"); log "solo $lane $unit $i/$SOLO_RUNS rc=$rc"
      if [ "$rc" != 0 ]; then
        say_unlisted "$1" "$lane" "$unit" $(( i - 1 )) red
        SOLO_RED="solo $lane $unit run $i rc=$rc"; return 1
      fi
    done
    say_unlisted "$1" "$lane" "$unit" "$SOLO_RUNS" green
  done 3<<< "$(solo_units "$1")"
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
# ------------------------------------------------- the Google sign-in client --
# Its secret lives in the login keychain ($GOOGLE_OAUTH_SECRET_ITEM, a generic password) and
# nowhere else we write: no file, no log, no status.json, no argument (the one leftover is rustc's own
# `# env-dep:` line in the release build folders' dep-info (.d) and fingerprint files). The lane is a launchd job in the
# person's gui domain (install-launchd.sh), so it reads that keychain while it is unlocked, and an
# item added with `-T /usr/bin/security` is read by `security` with no prompt. macOS only, like
# the lane: Linux and Windows have no such keychain and run no lane. The dry twin never calls
# the keychain — RELEASE_STUB_OAUTH_SECRET is the item's value, `missing` for no item and
# `empty` for an item with nothing in it.
DRY_OAUTH_SECRET=dry-run-google-oauth-client-secret
oauth_secret_howto() { printf 'security add-generic-password -U -a "$USER" -s %s -T %s -w   (it asks for the value; tools/release/README.md)' "$GOOGLE_OAUTH_SECRET_ITEM" "$SECURITY_BIN"; }
oauth_secret_on_file() { # -> 0 when the keychain holds the item; its value is not read here
  if is_dry; then [ "$(stub OAUTH_SECRET "$DRY_OAUTH_SECRET")" != missing ]; return; fi
  "$SECURITY_BIN" find-generic-password -s "$GOOGLE_OAUTH_SECRET_ITEM" > /dev/null 2>&1
}
oauth_secret() { # -> the value on stdout, for a build's own environment only; nothing when unreadable
  local v
  if is_dry; then
    v=$(stub OAUTH_SECRET "$DRY_OAUTH_SECRET")
    case $v in missing|empty) ;; *) printf '%s' "$v" ;; esac
    return 0
  fi
  "$SECURITY_BIN" find-generic-password -s "$GOOGLE_OAUTH_SECRET_ITEM" -w 2> /dev/null
}
oauth_secret_usable() { # VALUE — 0 for one non-empty line; WHY says what is wrong, never the value
  case $1 in
    '') WHY="the keychain item $GOOGLE_OAUTH_SECRET_ITEM is missing, empty or unreadable (a locked login keychain) — $(oauth_secret_howto)"; return 1 ;;
    *$'\n'*) WHY="the keychain item $GOOGLE_OAUTH_SECRET_ITEM holds more than one line — set it again: $(oauth_secret_howto)"; return 1 ;;
  esac
}
with_oauth_secret() { # VALUE CMD… — CMD in a subshell whose environment alone carries the value
  local value=$1; shift
  ( export "$GOOGLE_OAUTH_SECRET_ENV=$value"; "$@" )
}
# What a build made must carry the secret's own bytes. Not the `GOCSPX-` prefix: the app links the
# private-value table (tools/release/pii-rules.json, through zerocode-core's private_data.rs),
# which names the prefix, so a count of it is never 0 even in an app built without the secret.
# The value reaches grep as a pattern file on a pipe (printf is a builtin), never as an argument,
# and -c prints a count, never the line.
carries_oauth_secret() { # VALUE FILE… — 0 when every file holds it; WHY names the first that does not
  local value=$1 file n; shift
  for file in "$@"; do
    n=$(LC_ALL=C grep -c -a -F -f <(printf '%s\n' "$value") "$file" 2> /dev/null)
    [ "${n:-0}" -gt 0 ] 2> /dev/null \
      || { WHY="$file carries no built-in Google sign-in client (the keychain value occurs 0 times)"; return 1; }
  done
}
app_carries_oauth_secret() { # VALUE TARGET — the window's executable and the zo inside the app
  local app="$2/release/bundle/macos/$APP_NAME/Contents"
  carries_oauth_secret "$1" "$app/MacOS/$APP_BIN" "$app/$APP_ZO"
}
stub_binary() { # FILE… — a dry build's output: the sha, then what the build was given for the
  # Google client (RELEASE_STUB_BUILD_DROPS_SECRET=1 stands for a build that compiled without it)
  local file
  for file in "$@"; do
    mkdir -p "$(dirname "$file")" || return 1
    if [ "$(stub BUILD_DROPS_SECRET 0)" = 1 ]; then printf '%s\n' "$SHA" > "$file"
    else printf '%s\n%s\n' "$SHA" "${!GOOGLE_OAUTH_SECRET_ENV:-}" > "$file"; fi
  done
}
# The release builds themselves, each run only through with_oauth_secret.
build_app_release() { # TARGET
  cd "$SCRATCH" && CI=true CARGO_TARGET_DIR=$1 ZO_STAGE_TARGET_DIR=$(target_dir zo-release) \
    npm run tauri:release -- --bundles app --ci --no-sign --config "$BUILD_APP_CONFIG"
}
build_app_signed() { # TARGET PASSWORD — tauri reads the key from the path in the env, never its bytes on a command line; --ci takes the password from the env (empty = none)
  cd "$SCRATCH" && CI=true CARGO_TARGET_DIR=$1 TAURI_SIGNING_PRIVATE_KEY=$UPDATER_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD=$2 \
    npm run tauri:release -- --bundles app,dmg --ci
}
build_zo_release() { # TARGET [TRIPLE] — the host's zo, or one triple's (Linux through zigbuild)
  cd "$SCRATCH/zo-ide" || return 1
  case ${2:-} in
    '') CARGO_TARGET_DIR=$1 cargo build --profile "$ZO_PROFILE" -p zo-ide ;;
    *-linux-*) CARGO_TARGET_DIR=$1 cargo zigbuild --profile "$ZO_PROFILE" -p zo-ide --target "$2" ;;
    *) CARGO_TARGET_DIR=$1 cargo build --profile "$ZO_PROFILE" -p zo-ide --target "$2" ;;
  esac
}
# build KIND(app|zo) — background; writes "rc secs" to $WORK/build-KIND.result
run_build() {
  local kind=$1 t0 rc log="$OUT/build-$1-$SHA8.log" target secret
  t0=$(now_s)
  # Read here, for this build alone: the gates before it never had it.
  secret=$(oauth_secret)
  if ! oauth_secret_usable "$secret"; then
    printf 'build-%s: %s\n' "$kind" "$WHY" > "$log"; log "build-$kind: $WHY"
    echo "1 $(( $(now_s) - t0 ))" > "$WORK/build-$kind.result"
    return 1
  fi
  case $kind in
    app)
      target=$(target_dir root-release)
      if is_dry; then
        stub_log "build-app target=$target"; sleep "$(stub BUILD_APP_SLEEP 0)"; rc=$(stub BUILD_APP_RC 0)
        [ "$rc" = 0 ] && { mkdir -p "$target/release/bundle/macos/$APP_NAME" && printf '%s\n' "$SHA" > "$target/release/bundle/macos/$APP_NAME/sha"; }
        [ "$rc" = 0 ] && { with_oauth_secret "$secret" stub_binary "$target/release/bundle/macos/$APP_NAME/Contents/MacOS/$APP_BIN" \
                             "$target/release/bundle/macos/$APP_NAME/Contents/$APP_ZO" || rc=1; }
      else
        # stage-zo builds zo into the same warm target build-zo uses beside it: cargo's
        # build-directory lock serialises the two and the later one is a no-op, where a
        # cold zo-ide/target under the scratch cost 6 min of the app build (09-08). The
        # secret reaches that zo too: stage-zo hands cargo its whole environment.
        with_oauth_secret "$secret" build_app_release "$target" > "$log" 2>&1; rc=$?
        if [ "$rc" = 0 ] && ! is_dry; then
          # Sign the whole app inside-out with the machine's local identity: the
          # helper, the sibling helpers, and the outer dev.zerocode.app — whose
          # stable requirement is what lets the Screen Recording grant survive
          # rebuilds (adhoc changed the cdhash every build). See tools/signing.
          ( bash "$SCRATCH/tools/signing/sign-app-bundle.sh" "$target/release/bundle/macos/$APP_NAME" ) >> "$log" 2>&1
          rc=$?
        fi
        [ "$rc" = 0 ] && ! helper_in_bundle "$target" && { printf 'build-app: %s\n' "$WHY" >> "$log"; rc=1; }
      fi
      # An app without its Google sign-in client is never installed: build-app is red instead.
      [ "$rc" = 0 ] && ! app_carries_oauth_secret "$secret" "$target" \
        && { printf 'build-app: %s\n' "$WHY" >> "$log"; log "build-app: $WHY"; rc=1; } ;;
    zo)
      target=$(target_dir zo-release)
      if is_dry; then
        stub_log "build-zo target=$target"; sleep "$(stub BUILD_ZO_SLEEP 0)"; rc=$(stub BUILD_ZO_RC 0)
        [ "$rc" = 0 ] && { with_oauth_secret "$secret" stub_binary "$target/$ZO_PROFILE/zo" || rc=1; }
      else
        with_oauth_secret "$secret" build_zo_release "$target" > "$log" 2>&1; rc=$?
      fi
      [ "$rc" = 0 ] && ! carries_oauth_secret "$secret" "$target/$ZO_PROFILE/zo" \
        && { printf 'build-zo: %s\n' "$WHY" >> "$log"; log "build-zo: $WHY"; rc=1; }
      [ "$rc" = 0 ] && [ "$RELEASE_LEGACY_MANIFEST" = 1 ] && build_zo_cross "$target" "$log" "$secret" ;;
  esac
  echo "$rc $(( $(now_s) - t0 ))" > "$WORK/build-$kind.result"
  return "$rc"
}
# The legacy manifest's other targets (§2.7): a cross build per triple after
# the host's, each on its own rc — a missing one does not redden build-zo (the
# host zo still swaps); publish refuses the manifest whole instead.
build_zo_cross() { # TARGET_DIR LOG SECRET — each triple's zo carries the Google client, or it is not built
  local t crc
  for t in $ZO_BUILD_TARGETS; do
    [ "$t" = "${ZO_BUILD_TARGETS%% *}" ] && continue
    if is_dry; then
      case " $(stub ZO_CROSS_MISSING) " in *" $t "*) crc=1;; *) crc=0;; esac
      stub_log "build-zo cross=$t rc=$crc"
      [ "$crc" = 0 ] && { mkdir -p "$1/$t/$ZO_PROFILE" && printf '%s %s\n' "$SHA" "$t" > "$1/$t/$ZO_PROFILE/zo"; }
    else
      with_oauth_secret "$3" build_zo_release "$1" "$t" >> "$2" 2>&1; crc=$?
      # A triple whose zo lacks the client is a missing triple: publish refuses the manifest whole.
      [ "$crc" = 0 ] && ! carries_oauth_secret "$3" "$1/$t/$ZO_PROFILE/zo" \
        && { printf 'build-zo cross %s: %s\n' "$t" "$WHY" >> "$2"; rm -f "$1/$t/$ZO_PROFILE/zo"; crc=1; }
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

# ------------------------------------------------------------- build-win --
# The Windows installer (t-113717), on a publish run only. It runs after both swaps and never beside
# build-app: win-installer.sh builds ui/dist and stages its zo where the bundle carries one
# (crates/zerocode-shell/bin/zo), both in the scratch build-app uses — it puts that zo back after.
win_asset() { printf 'ZeroCode_%s_%s-setup.exe' "$VERSION" "$WIN_ARCH"; }
build_win_release() { # TARGET LOG SECRET — the sha's own script; the Google client in its environment alone
  with_oauth_secret "$3" env ZO_PROFILE="$ZO_PROFILE" bash "$SCRATCH/$WIN_INSTALLER_SH" "$SCRATCH" "$1" > "$2" 2>&1
}
# rc 0 built (WIN_INSTALLER) or intentionally skipped (SKIP) · 1 red · 2 no room (WHY)
do_build_win() {
  local target release built log="$OUT/build-win-$SHA8.log" secret rc in_tree=0 needed=$(( DISK_FLOOR_GB + WIN_ROOM_GB ))
  [ "$RELEASE_PUBLISH" = 1 ] || { SKIP="RELEASE_PUBLISH=0 (only a publish ships the Windows installer)"; return 0; }
  [ "$RELEASE_WINDOWS" = 1 ] || { SKIP="RELEASE_WINDOWS=0 (this publish ships macOS alone)"; return 0; }
  if is_dry; then in_tree=$(stub WIN_INSTALLER_IN_TREE 1); else [ -f "$SCRATCH/$WIN_INSTALLER_SH" ] && in_tree=1; fi
  [ "$in_tree" = 1 ] || { SKIP="$SHA8's tree has no $WIN_INSTALLER_SH (a sha from before the Windows installer)"; return 0; }
  measure_disk
  if [ "$DISK_FREE_GB" -lt "$needed" ]; then
    reclaim_finished_targets "$needed"
    if [ "$DISK_FREE_GB" -lt "$needed" ]; then
      WHY="disk: ${DISK_FREE_GB}G free < ${DISK_FLOOR_GB}G floor + ${WIN_ROOM_GB}G for the Windows build"
      return 2
    fi
  fi
  target=$(target_dir "$WIN_LANE"); release="$target/$WIN_TRIPLE/release"; built="$release/bundle/nsis/$(win_asset)"
  # Read here, for this build alone, as build-app reads it.
  secret=$(oauth_secret)
  oauth_secret_usable "$secret" || { printf 'build-win: %s\n' "$WHY" > "$log"; return 1; }
  if is_dry; then
    stub_log "build-win target=$target"; rc=$(stub BUILD_WIN_RC 0)
    if [ "$rc" = 0 ]; then
      mkdir -p "$(dirname "$built")" && printf 'stub installer %s\n' "$SHA" > "$built" || rc=1
      if [ "$(stub BUILD_WIN_DROPS_SECRET 0)" = 1 ]; then stub_binary "$release/$APP_BIN.exe" "$target/$WIN_TRIPLE/$ZO_PROFILE/zo.exe" || rc=1
      else with_oauth_secret "$secret" stub_binary "$release/$APP_BIN.exe" "$target/$WIN_TRIPLE/$ZO_PROFILE/zo.exe" || rc=1; fi
    fi
  else
    heavy build_win_release "$target" "$log" "$secret"; rc=$?
  fi
  [ "$rc" = 0 ] || { WHY="$WIN_INSTALLER_SH rc=$rc ($log)"; return 1; }
  # A Windows window or zo without its Google sign-in client is never shipped either.
  carries_oauth_secret "$secret" "$release/$APP_BIN.exe" "$target/$WIN_TRIPLE/$ZO_PROFILE/zo.exe" \
    || { printf 'build-win: %s\n' "$WHY" >> "$log"; return 1; }
  [ -f "$built" ] || { WHY="no $(win_asset) under $(dirname "$built") ($log)"; return 1; }
  # Out of the target at once: on a tight disk the target goes right after this phase (judge_sha).
  mkdir -p "$(out_dir)" && cp "$built" "$(out_dir)/$(win_asset)" || { WHY="copy $(win_asset) to $(out_dir)"; return 1; }
  WIN_INSTALLER="$(out_dir)/$(win_asset)"
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
# It builds the app again into build-app's target, and that app is the one published: it is given
# the Google client's secret as build-app was (a build without it would rebuild the app without it).
build_updater_bundle() { # TARGET PASSWORD LOG
  local secret rc
  secret=$(oauth_secret)
  oauth_secret_usable "$secret" || { printf 'bundle-updater: %s\n' "$WHY" > "$3"; return 1; }
  with_oauth_secret "$secret" build_app_signed "$1" "$2" > "$3" 2>&1; rc=$?
  [ "$rc" = 0 ] || return "$rc"
  app_carries_oauth_secret "$secret" "$1" || { printf 'bundle-updater: %s\n' "$WHY" >> "$3"; return 1; }
}
# The app the DMG and the updater archive carry must be SEALED, the way the
# installed app is. Tauri's bundler signs only when a distribution identity is
# in the env (APPLE_SIGNING_IDENTITY, with notarization when APPLE_ID·
# APPLE_PASSWORD·APPLE_TEAM_ID stand beside it); without one it ships the
# linker's adhoc signature over unsealed resources, and another Mac's
# Gatekeeper calls that "damaged" the moment quarantine is on it (v1.3.78,
# 2026-09-15: a colleague's MacBook Air). So without that identity the app is
# build-app's own, signed inside-out by the install signer and installed as is:
# the lane packs the archive and the DMG from it and signs the archive with the
# updater key. It never signs that app again, because a re-sign moves the outer
# cdhash (4929830f to 9cfee161 on 10-13) and the updater would ship an app this
# machine never installed. With the identity, Tauri's own seal is kept and only verified.
# Either way the bundle must verify --deep --strict before it is gathered.
# Every asset the feed names is signed this one way — the macOS archive and the Windows installer.
sign_updater_asset() { # FILE PASSWORD LOG -> FILE.sig beside it; WHY on failure
  ( cd "$SCRATCH" && TAURI_SIGNING_PRIVATE_KEY_PASSWORD=$2 npx --no-install tauri signer sign -f "$UPDATER_KEY" "$1" ) >> "$3" 2>&1 \
    || { WHY="sign $1 with the updater key ($3)"; return 1; }
  [ -f "$1.sig" ] || { WHY="tauri signer left no $1.sig"; return 1; }
}
seal_updater_bundle() { # TARGET PASSWORD LOG -> rc; WHY on failure
  local target=$1 password=$2 log=$3 app archive dmg staging rc
  app="$target/release/bundle/macos/$APP_NAME"
  [ -d "$app" ] || { WHY="no $APP_NAME under $target/release/bundle/macos"; return 1; }
  if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
    archive="$target/release/bundle/macos/$APP_NAME.tar.gz"
    rm -f "$archive" "$archive.sig"
    # COPYFILE_DISABLE: no AppleDouble `._` entries — the updater unpacks the
    # archive with its own tar and expects the bundle's files alone.
    ( cd "$target/release/bundle/macos" && COPYFILE_DISABLE=1 tar -czf "$APP_NAME.tar.gz" "$APP_NAME" ) >> "$log" 2>&1 || { WHY="pack the sealed app into $archive"; return 1; }
    sign_updater_asset "$archive" "$password" "$log" || return 1
    mkdir -p "$target/release/bundle/dmg" && rm -f "$target/release/bundle/dmg/"*.dmg
    dmg="$target/release/bundle/dmg/$(asset_base).dmg"
    staging=$(mktemp -d "${TMPDIR:-/tmp}/zerocode-dmg.XXXXXX") || { WHY="no staging folder for the DMG"; return 1; }
    cp -R "$app" "$staging/" && ln -s /Applications "$staging/Applications" \
      && hdiutil create -quiet -volname "${APP_NAME%.app}" -srcfolder "$staging" -ov -format UDZO "$dmg" >> "$log" 2>&1
    rc=$?; rm -rf "$staging"
    [ "$rc" = 0 ] || { WHY="hdiutil create $dmg rc=$rc ($log)"; return 1; }
    log "bundle-updater: archive and DMG packed from build-app's sealed $APP_NAME, the installed one"
  fi
  codesign --verify --deep --strict "$app" >> "$log" 2>&1 || { WHY="the DMG's app does not verify (codesign --deep --strict, $log)"; return 1; }
}
# Runs in the foreground (SKIP/UPDATER_PUBKEY/WHY are its answers); only the
# build inside is heavy. Skips — no key, no plugins.updater yet — are green
# and said; a pubkey that is not the configured one is red, because the
# updater would refuse what that key signs.
# One build serves the install and the publish, so what the updater ships is what
# this machine installed; the dry twin answers from RELEASE_STUB_CDHASH_<KIND>.
app_cdhash() { # INSTALLED|SEALED APP -> its CDHash, or nothing
  if is_dry; then stub "CDHASH_$1" stub-cdhash; else codesign -dvvv "$2" 2>&1 | sed -n 's/^CDHash=//p' | head -1; fi
}
do_bundle_updater() {
  local target pub conf_pub conf_fp out log="$OUT/bundle-updater-$SHA8.log" archive dmg rc password= installed sealed rows
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
  # The archive is only ever shipped by a publish run; an install run skips the phase aloud.
  [ "$RELEASE_PUBLISH" = 1 ] || { SKIP="RELEASE_PUBLISH=0 (the publish run packs the archive)"; return 0; }
  mkdir -p "$out" || return 1
  if is_dry; then
    if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
      stub_log "bundle-updater target=$target key=$UPDATER_KEY"; rc=$(stub BUNDLE_UPDATER_RC 0)
      [ "$rc" = 0 ] || { WHY="build rc=$rc"; return 1; }
    fi
    stub_log "bundle-updater seal target=$target key=$UPDATER_KEY"
    mkdir -p "$target/release/bundle/macos" "$target/release/bundle/dmg"
    printf 'stub archive %s\n' "$SHA" > "$target/release/bundle/macos/$APP_NAME.tar.gz"
    printf 'STUBSIG%s\n' "$SHA8" > "$target/release/bundle/macos/$APP_NAME.tar.gz.sig"
    printf 'stub dmg\n' > "$target/release/bundle/dmg/$(asset_base).dmg"
    printf '### stub\n- stub notes for %s\n' "$VERSION" > "$out/notes.md"
  else
    [ -f "$UPDATER_KEY_PASSWORD_FILE" ] && password=$(cat "$UPDATER_KEY_PASSWORD_FILE")
    # Without a distribution identity build-app's app is the one shipped, so a second
    # tauri:release would only be thrown away (1105 s on 10-13, its zo stage cold).
    # Tauri signs and notarizes only inside its own build: with an identity it stays.
    if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
      heavy build_updater_bundle "$target" "$password" "$log"; rc=$?
      [ "$rc" = 0 ] || { WHY="tauri build rc=$rc ($log)"; return 1; }
    fi
    seal_updater_bundle "$target" "$password" "$log" || return 1
    python3 "$FEED_PY" section --changelog "$SCRATCH/CHANGELOG.md" --version "$VERSION" > "$out/notes.md" \
      || { WHY="CHANGELOG.md has no [$VERSION] section — run tools/release/bump.sh before queueing"; return 1; }
  fi
  if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
    installed=$(app_cdhash INSTALLED "$RELEASE_APP_DIR/$APP_NAME"); sealed=$(app_cdhash SEALED "$target/release/bundle/macos/$APP_NAME")
    { [ -n "$sealed" ] && [ "$sealed" = "$installed" ]; } \
      || { WHY="the sealed app is not the installed one: cdhash ${sealed:-unreadable}, installed ${installed:-unreadable}"; return 1; }
  fi
  archive="$target/release/bundle/macos/$APP_NAME.tar.gz"
  { [ -f "$archive" ] && [ -f "$archive.sig" ]; } || { WHY="no signed updater archive under $target/release/bundle/macos"; return 1; }
  cp "$archive" "$out/$(asset_base).app.tar.gz" && cp "$archive.sig" "$out/$(asset_base).app.tar.gz.sig" || { WHY="copy to $out"; return 1; }
  dmg=$(ls "$target/release/bundle/dmg/"*.dmg 2>/dev/null | head -1)
  [ -z "$dmg" ] || cp "$dmg" "$out/$(asset_base).dmg" || { WHY="copy dmg to $out"; return 1; }
  rows=(--platform "$UPDATER_PLATFORM" --sig-file "$out/$(asset_base).app.tar.gz.sig" --url "$(asset_url "$(asset_base).app.tar.gz")")
  # The Windows set rides whole or not at all (t-113717): this run's installer, its .sig, its feed row.
  if [ -n "$WIN_INSTALLER" ]; then
    if is_dry; then stub_log "bundle-updater sign-win $(win_asset) key=$UPDATER_KEY"; printf 'STUBWINSIG%s\n' "$SHA8" > "$WIN_INSTALLER.sig"
    else sign_updater_asset "$WIN_INSTALLER" "$password" "$log" || return 1; fi
    rows+=(--platform "$WIN_UPDATER_PLATFORM" --sig-file "$WIN_INSTALLER.sig" --url "$(asset_url "$(win_asset)")")
  fi
  python3 "$FEED_PY" latest --version "$VERSION" --notes-file "$out/notes.md" --pub-date "$(now_iso)" "${rows[@]}" \
      > "$out/$UPDATER_FEED.tmp" && mv -f "$out/$UPDATER_FEED.tmp" "$out/$UPDATER_FEED" || { WHY="$UPDATER_FEED"; return 1; }
  [ -z "$WIN_INSTALLER" ] || WIN_SHIPPED=1
  log "bundle-updater: $(asset_base).app.tar.gz + .sig${WIN_INSTALLER:+ + $(win_asset) + .sig} + $UPDATER_FEED under $out (key $UPDATER_PUBKEY)"
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
# The installer has no Authenticode signature yet, so a first install from the browser meets
# SmartScreen; an update runs the copy the app downloaded and verified itself, which carries no
# browser's mark of the web (Smart App Control, where a person turned it on, judges either).
win_note() {
  printf 'Windows (x64): %s — 코드 서명(Authenticode)이 아직 없어 SmartScreen이 「Windows의 PC 보호」를 띄우면 「추가 정보(More info)」 → 「실행(Run anyway)」을 누릅니다. 설치한 앱은 그 뒤로 스스로 업데이트됩니다.' "$(win_asset)"
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
  # The Windows installer and its .sig go up only beside the feed row bundle-updater wrote for them
  # in this run — never a pair an earlier run of this sha left under out/.
  [ "$WIN_SHIPPED" = 1 ] && assets="$assets $out/$(win_asset) $out/$(win_asset).sig"
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
  # The release page says how a Windows install starts (README.md says it too); the feed's notes,
  # written before this, are what an installed app shows and need no such line.
  [ "$WIN_SHIPPED" = 1 ] && printf '\n\n%s\n' "$(win_note)" >> "$notes"
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
# that half was built from — apart from version lines in the release stamp —
# owes that gate nothing: those bytes passed it then. The two halves of
# installed.json are the base (the app's sha for root, zo's for zo), so a
# folded ancestor never becomes the base. Any other changed path owes both
# gates: no path is known to lie outside either, since each reads files of the
# other half and of the docs — the root gate's pii-check reads every tracked
# file, core tests read zo-ide sources, the core build script turns skills/
# into code and zo tests read the root fixtures at run time. Anything the lane
# cannot tell — no install yet, git not answering, a path with a space or a
# control character in it (git's list is read NUL-separated) — runs the gate.
# Measured 2026-09-16 (three green runs): gate-root 1105–1615 s, gate-zo
# 876–1296 s, on releases that each changed one tree.
stamp_only() { # PATH SINCE — the file changed in version lines only
  if is_dry; then case " $(stub STAMP_REAL_CHANGES) " in *" $1 "*) return 1;; esac; return 0; fi
  local diff
  diff=$(git -C "$SCRATCH" diff -U0 "$2" "$SHA" -- "$1" 2>/dev/null) || return 1
  ! printf '%s\n' "$diff" \
    | grep -E '^[+-]' | grep -vE '^(\+\+\+|---)' | grep -qvE '^[+-][[:space:]]*"?version"?[[:space:]]*[:=]'
}
# real_changes SINCE STUB [DEFAULT] — REAL: the paths SINCE..SHA changed, one a line,
# but the stamp files whose diff is version lines alone; CHANGED: how many paths
# changed in all. 1 when the lane cannot tell (git not answering, a path with a space
# or a control character in it). The dry run reads the paths from the stub.
real_changes() {
  local since=$1 changed path
  REAL=; CHANGED=0
  if is_dry; then changed=$(stub "$2" "${3-}" | tr ' ' '\n')
  else changed=$(git -C "$SCRATCH" diff --name-only -z "$since" "$SHA" 2>/dev/null | tr '\n\0' '\001\n'
                 exit "${PIPESTATUS[0]}") || return 1; fi
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    CHANGED=$(( CHANGED + 1 ))
    case $path in *[[:space:]]*|*[[:cntrl:]]*) return 1 ;; esac
    case " $RELEASE_STAMP_FILES " in *" $path "*) stamp_only "$path" "$since" && continue ;; esac
    REAL="$REAL$path"$'\n'
  done <<< "$changed"
  return 0
}
gate_owed() { # root|zo — 0 when the gate must run; otherwise SKIP says why and 1
  local lane=$1 half since
  case $lane in root) half=app ;; zo) half=zo ;; esac
  since=$(installed_part "$INSTALLED" "$half"); since=${since%% *}
  [ -n "$since" ] || return 0
  # The dry default names a shared crate, so every gate a stub run does not
  # speak about is owed — as before this rule existed.
  real_changes "$since" CHANGED_PATHS "crates/zerocode-core/src/lib.rs" || return 0
  [ -z "$REAL" ] || return 0
  SKIP="$lane tree unchanged since ${since:0:8} — $CHANGED path(s) changed, none its own"
  return 1
}

# ------------------------------------------- the coordinator's gate receipt --
# A sha cut from a tree the coordinator already gated on the build line carries
# that gate's receipt (enqueue.sh --receipt [--keep-only], kept as $RECEIPTS/<sha>.json). The
# receipt only locates the build line's evidence and seals its bytes; coord_receipt.py
# reads all of it again — each zo line job's record (passed, rc 0, its command the
# checkout's own coord-gate.sh) and rc file, the owner and folder its log names, the head
# and tree its log opened with, one green RC line and one log per entry, a verdict of no
# red, one run, tree and checkout — holds every job to RELEASE_BUILD_LINE (its record in
# the line's jobs/) and the jobs' own word on owner and run to the coordinator's
# (coord-<label> of RELEASE_COORD_RUN, or its -root and -zo jobs), and counts a recipe as gated only where an
# entry ran exactly `just <recipe>` in that gate's folder from the top of the checkout.
# A lane given no RELEASE_COORD_RUN leaves a receipt unused.
# The receipt stands for this sha when git finds every job's head to hold the tree its
# jobs printed, and that tree to be the sha's apart from the stamp's version lines and
# the release notes edited in place (whose readers are then owed again), and every job
# ended after its head was committed. The gate then runs only what the receipt left
# owed — nothing, and the phase is skipped aloud, when it left nothing. Without a
# receipt, or with one refused, the gate runs whole, and the phase line and status.json
# say why. v1.1.84's lane spent 3168 + 1719 s on the tree the coordinator's l64 had
# gated green an hour before (t-47516).
gate_recipes() { # LANE -> the recipes its gate runs, one a line, as verify-every-recipe.sh lists them
  if is_dry; then stub "GATE_$(upper "$1")_RECIPES" test | tr ' ' '\n'; return 0; fi
  ( cd "$SCRATCH/$(gate_dir "$1")" && "$SELF_DIR/verify-every-recipe.sh" --list ) 2>/dev/null
}
commit_time() { # SHA -> its committer time, epoch seconds
  if is_dry; then stub COMMIT_TIME 0; return 0; fi
  git -C "$SCRATCH" show -s --format=%ct "$1" 2>/dev/null
}
tree_of() { # SHA PRINTED -> the tree git finds SHA to hold; the dry run says PRINTED, the tree its jobs printed
  if is_dry; then stub HEAD_TREE "${2-}"; return 0; fi
  git -C "$SCRATCH" rev-parse --verify --quiet "$1^{tree}" 2>/dev/null
}
# notes_edited SINCE — 0 when SINCE..SHA changed the release notes in place: modified
# (--diff-filter=M), a plain file (100644) on both sides. --name-only shows a link, a
# removal and a mode change as the same one path, and a linked notes file is what
# updater_feed.py would follow into the release (ZO's review of t-47516, P3-1).
notes_edited() {
  local raw
  if is_dry; then raw=$(stub NOTES_RAW ":100644 100644 1111111 2222222 M"$'\t'"$RELEASE_NOTES")
  else raw=$(git -C "$SCRATCH" diff-tree -r --raw --diff-filter=M "$1" "$SHA" -- "$RELEASE_NOTES" 2>/dev/null) || return 1; fi
  case $raw in ":100644 100644 "*" M"$'\t'"$RELEASE_NOTES") return 0 ;; esac
  return 1
}
receipt_cover() { # LANE — 0 when this sha's receipt stands, ONLY what it left owed; 1 when there is none or it is refused (RECEIPT says why)
  local lane=$1 file="$RECEIPTS/$SHA.json" facts kind rest head ended committed recipes r who= run= tree= logs= heads= covered= owed= notes= n=0 total=0
  local cleared= judged=
  ONLY=; RECEIPT=
  [ -f "$file" ] || return 1
  [ -n "$RELEASE_COORD_RUN" ] || { RECEIPT="unused: the lane was given no coordinator run (RELEASE_COORD_RUN)"; return 1; }
  facts=$(python3 "$RECEIPT_PY" read --receipt "$file" --dir "$(gate_dir "$lane")" \
            --line "$RELEASE_BUILD_LINE" --run "$RELEASE_COORD_RUN" 2>&1) \
    || { RECEIPT="refused: $facts"; return 1; }
  while read -r kind rest; do
    case $kind in
      who) who=$rest ;;
      run) run=$rest ;;
      tree) tree=$rest ;;
      logs) logs=$rest ;;
      recipe) covered="$covered $rest " ;;
      cleared) cleared="$cleared $rest" ;;   # a red entry of this gate's folder the receipt's flakes judgment cleared
      judged) case $rest in "$lane"$'\t'*) judged="$judged$rest"$'\n' ;; esac ;;   # one failure it cleared, solo_plan's line
      head) heads="$heads$rest"$'\n' ;;
    esac
  done <<< "$facts"
  [ -n "$tree" ] && [ -n "$heads" ] || { RECEIPT="refused: the receipt names no tree or head"; return 1; }
  while read -r head ended; do
    [ -n "$head" ] || continue
    real_changes "$head" RECEIPT_CHANGED_PATHS || { RECEIPT="refused: git cannot compare ${head:0:8} with $SHA8"; return 1; }
    [ "$(tree_of "$head" "$tree")" = "$tree" ] \
      || { RECEIPT="refused: ${head:0:8} does not hold the tree its jobs printed (${tree:0:8})"; return 1; }
    case $REAL in
      '') ;;
      "$RELEASE_NOTES"$'\n')
        notes_edited "$head" || { RECEIPT="refused: $RELEASE_NOTES is not a plain file edited in place since ${head:0:8}"; return 1; }
        notes=1 ;;
      *) RECEIPT="refused: ${head:0:8} holds another tree ($(printf '%s' "$REAL" | head -3 | tr '\n' ' ')…)"; return 1 ;;
    esac
    committed=$(commit_time "$head")
    [ "${ended:-0}" -gt "${committed:-x}" ] 2>/dev/null \
      || { RECEIPT="refused: a job on ${head:0:8} ended before that head was committed"; return 1; }
  done <<< "$heads"
  recipes=$(gate_recipes "$lane")
  [ -n "$recipes" ] || { RECEIPT="unused: the $lane gate's recipes are unknown (verify-every-recipe.sh --list)"; return 1; }
  for r in $recipes; do
    total=$(( total + 1 ))
    case $covered in
      *" $r "*) case "${notes:+ $NOTES_READERS }" in *" $r "*) ;; *) n=$(( n + 1 )); continue ;; esac ;;
    esac
    owed="$owed $r"
  done
  ONLY=${owed# }
  # The failures it cleared count toward this run's bound on names judged solo, as if the gate had run here.
  RECEIPT_JUDGED="$RECEIPT_JUDGED$judged"
  RECEIPT="$who of $run as its jobs say (the lane's coordinator run), tree ${tree:0:8}: $n/$total $lane recipes green${cleared:+ (red, cleared solo as the flakes phase clears a red gate:$cleared)}${notes:+, the notes changed}${ONLY:+, owes $ONLY} — logs $logs"
  return 0
}
gate_left() { # LANE — 0 when the gate still runs: whole, or ONLY what a receipt left; 1 when a receipt covered it all (SKIP says so)
  receipt_cover "$1" || { ONLY=; return 0; }
  [ -z "$ONLY" ] || return 0
  SKIP="receipt $RECEIPT"; RECEIPT=
  return 1
}

# ------------------------------------------------ a gate this sha already passed --
# A run can end red after one of its gates passed: on 2026-11-08 v1.1.131's gate-root
# went green in 3,697 s, then the disk filled during gate-zo, and the next run of the
# same sha ran gate-root whole again. A gate that runs whole and passes now leaves
# $PASSED/<sha>-<lane>: its seconds, when it ended, and the sha-256 of the log it left
# and of the gate script that ran it. The next run of that very sha skips the gate
# aloud while both files are still those bytes, and keeps the log, so the flake
# judgment reads what it would have read had the gate just passed. Anything else runs
# the gate: a red gate (never recorded), a gate a receipt shaped (it ran only what the
# receipt left owed), a log or script that changed, another sha. The step that failed
# still ends its run red; only what passed is kept. The records go when the sha ends
# green or superseded (sweep_sha), as its receipt does.
file_sum() { [ -f "$1" ] && shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1; }
record_pass() { # LANE SECS — LANE's gate ran whole on this sha and passed
  local lane=$1 sum script
  [ -z "$ONLY" ] || return 0
  sum=$(file_sum "$OUT/gate-$lane-$SHA8.log"); script=$(file_sum "$SELF_DIR/verify-every-recipe.sh")
  [ -n "$sum" ] && [ -n "$script" ] || return 0
  mkdir -p "$PASSED" || return 0
  printf '%s %s %s %s\n' "$2" "$(now_iso)" "$sum" "$script" > "$PASSED/$SHA-$lane.tmp" \
    && mv -f "$PASSED/$SHA-$lane.tmp" "$PASSED/$SHA-$lane"
}
gate_passed() { # LANE — 0 when an earlier run of this sha passed LANE's gate whole and its log and script are unchanged (SKIP says so)
  local lane=$1 rec="$PASSED/$SHA-$1" secs= ended= sum= script=
  [ -f "$rec" ] || return 1
  read -r secs ended sum script < "$rec"
  if [ -z "$sum" ] || [ "$(file_sum "$OUT/gate-$lane-$SHA8.log")" != "$sum" ]; then
    log "gate-$lane: the pass an earlier run of $SHA8 recorded no longer matches gate-$lane-$SHA8.log — the gate runs"
    rm -f "$rec"; return 1
  fi
  if [ -z "$script" ] || [ "$(file_sum "$SELF_DIR/verify-every-recipe.sh")" != "$script" ]; then
    log "gate-$lane: verify-every-recipe.sh changed since an earlier run of $SHA8 passed this gate — the gate runs"
    rm -f "$rec"; return 1
  fi
  SKIP="passed whole in an earlier run of $SHA8, ended $ended in ${secs}s — gate-$lane-$SHA8.log kept"
  return 0
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
  # Before any gate, install runs and publish runs alike: without the Google sign-in client's
  # keychain item every build this run makes would be an app that cannot sign in to Google, and
  # that is better known now than after an hour of gates. Only the item's presence is read here.
  if ! oauth_secret_on_file; then
    red "google sign-in client: no keychain item $GOOGLE_OAUTH_SECRET_ITEM, or the login keychain is locked — add it once: $(oauth_secret_howto)"
    return 1
  fi

  phase_begin archive
  heavy do_archive; rc=$?
  step_done archive "$rc" || return $?
  [ "$rc" = 0 ] || { red "archive rc=$rc"; return 1; }
  VERSION=$(scratch_version); write_status
  [ -n "$VERSION" ] && report_build step --label "v$VERSION"
  trim_targets

  # The root verify recipe completes its cargo recipes before its browser
  # harnesses. Finish that entire gate before starting zo's cargo/e2e work;
  # solo browser judgments also finish below before either build starts.
  # This is a scheduling contract, not a switch to re-enable parallel gates.
  bow_out_if_superseded gate-root || return 1
  phase_begin gate-root
  if gate_passed root; then
    # Passed whole in an earlier run of this very sha: its log stays for the flake judgment.
    root_rc=0; stub_log "gate root passed before"
    release_gate_target root-gate   # no root solo will need it
    step_done gate-root 0 0 || return $?
  elif gate_owed root && gate_left root; then
    heavy wait_for_calm gate-root
    heavy run_gate root
    read_result "$WORK/gate-root.result"; root_rc=$RES_RC
    [ "$root_rc" = 0 ] && record_pass root "$RES_SECS"
    [ "$root_rc" = 0 ] && release_gate_target root-gate   # no root solo will need it
    step_done gate-root "$RES_RC" "$RES_SECS" || return $?
  else
    # A gate not owed — the tree unchanged since the last install, or every
    # recipe green in the coordinator's receipt — leaves an empty log: the flake
    # judgment below reads nothing from it, and the phase line says why it did not run.
    root_rc=0; : > "$OUT/gate-root-$SHA8.log"; stub_log "gate root not owed"
    step_done gate-root 0 0 || return $?
  fi
  bow_out_if_superseded gate-zo || return 1
  phase_begin gate-zo
  if gate_passed zo; then
    zo_rc=0; stub_log "gate zo passed before"
    release_gate_target zo-gate
    step_done gate-zo 0 0 || return $?
  elif gate_owed zo && gate_left zo; then
    heavy wait_for_calm gate-zo
    heavy run_gate zo
    read_result "$WORK/gate-zo.result"; zo_rc=$RES_RC
    [ "$zo_rc" = 0 ] && record_pass zo "$RES_SECS"
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
  real=$(flake_reals "$plan" "$root_rc" "$zo_rc" "$RECEIPT_JUDGED")
  if [ -n "$real" ]; then
    step_done flakes 1
    red "gate red for real: root rc=$root_rc zo rc=$zo_rc [$(printf '%s' "$real" | sed 's/^ //')]"; return 1
  fi
  # Every remaining name is judged solo, listed or not; an unlisted one is
  # named so the person can add it to flakes.txt once it has passed solo —
  # and named again with its judgment, which $UNLISTED keeps (t-9741).
  say_unlisted_ahead "$plan"
  if [ "$root_rc" != 0 ] || [ "$zo_rc" != 0 ]; then
    # The solo run is the judgment — flake or real — so it gets the calm
    # machine a gate gets. It follows the zo gate immediately, and a 2px anchor
    # or a poll count judged while that gate's compile is still flushing is not
    # a judgment: on 2026-09-18 three window assertions failed in the gate at
    # load 7.75, failed again in the solo right behind it, and were green every
    # time they were asked on an idle machine.
    heavy wait_for_calm flakes-solo
    if ! solo_judge "$plan"; then step_done flakes 1; red "$SOLO_RED"; return 1; fi
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

  # The Windows installer (t-113717): a publish run's, after both swaps. A red build leaves the
  # release macOS-only unless WIN_BLOCKS_PUBLISH=1, as a missing legacy zo triple and a refused
  # source snapshot leave the release itself alone; the step stays red on the board. Its target's
  # last use in this run is this phase (the installer is under out/ by now): on a tight disk it goes
  # before the floor judges the phases still to come.
  phase_begin build-win
  do_build_win; rc=$?
  if [ "$rc" = 2 ]; then
    if [ "$WIN_BLOCKS_PUBLISH" = 1 ]; then rc=1
    else
      rc=0; SKIP=$WHY
      WIN_RED="Windows 설치 파일 없이 나감: $WHY"
    fi
  elif [ -z "$SKIP" ]; then release_gate_target "$WIN_LANE"; fi
  step_done build-win "$rc" || return $?
  if [ "$rc" != 0 ]; then
    [ "$WIN_BLOCKS_PUBLISH" = 1 ] && { red "build-win: $WHY"; return 1; }
    # Never a quiet macOS-only release: the run ends green with this as its reason.
    WIN_RED="build-win red — the release goes out without the Windows installer: $WHY"
  fi
  [ -z "$WIN_RED" ] || log "$WIN_RED"

  phase_begin bundle-updater
  do_bundle_updater; rc=$?
  step_done bundle-updater "$rc" || return $?
  [ "$rc" = 0 ] || { red "bundle-updater: $WHY"; return 1; }
  phase_begin publish
  do_publish; rc=$?
  step_done publish "$rc" || return $?
  [ "$rc" = 3 ] && return 3
  [ "$rc" = 0 ] || { red "publish: $WHY"; return 1; }
  OUTCOME=green; REASON=$WIN_RED; return 0
}

sweep_sha() {
  [ "$SWEPT" = 0 ] || return 0
  SWEPT=1
  phase_begin sweep
  kill_children
  [ "$OUTCOME" = null ] && red "exit before an outcome"
  # The receipt is kept beside the lane log; it stays queued with a sha that may run again.
  if [ -f "$RECEIPTS/$SHA.json" ]; then
    cp -f "$RECEIPTS/$SHA.json" "$OUT/receipt-$SHA8.json"
    case $OUTCOME in green|superseded) rm -f "$RECEIPTS/$SHA.json" ;; esac
  fi
  # So is what its gates passed (gate_passed): kept while the sha may run again.
  case $OUTCOME in green|superseded) rm -f "$PASSED/$SHA-root" "$PASSED/$SHA-zo" ;; esac
  rm -rf "$SCRATCH" "$WORK"
  is_dry || git -C "$RELEASE_REPO" worktree prune > /dev/null 2>&1
  stub_log "sweep"
  phase_end sweep 0
  report_end
  log "== $OUTCOME${REASON:+ ($REASON)}"
}
run_sha() { # SHA [QUEUE_FILE]
  SHA=$1; SHA8=$(printf '%s' "$1" | cut -c1-8); QUEUE_FILE=${2:-}
  SCRATCH=$RELEASE_SCRATCH_ROOT/gate-$SHA8; LOG=$OUT/lane-$SHA8.log
  STARTED_AT=$(now_iso); PHASES_JSON=; OUTCOME=null; REASON=; SWEPT=0
  VERSION=; UPDATER_PUBKEY=; SKIP=; WHY=; ONLY=; RECEIPT=; RECEIPT_JUDGED=; STALLED=null; WIN_INSTALLER=; WIN_SHIPPED=0; WIN_RED=
  rm -rf "$WORK"; mkdir -p "$WORK"
  log "== lane $SHA8 start (dry=$RELEASE_DRY_RUN)"
  report_begin
  [ -d "$SCRATCH" ] && log "stale scratch $SCRATCH — swept on the way out"
  judge_sha; local rc=$?
  sweep_sha
  return $rc
}

# ----------------------------------------- the flakes judgment of a coordinator's gate --
# coord-gate.sh runs each gate's recipes as entries of its own job — `just <recipe>` from the top, zo's
# in zo-ide — each one's output in <logs>/<key>.log, and coord_receipt.py reads that job again
# (t-119757). A red entry is judged as the flakes phase above judges a red gate: by these very functions,
# never a copy of them (t-121630).
#
#   lane.sh --flake-plan ENTRY...                          what the flakes phase makes of the entries
#   lane.sh --judge-flakes TOP TARGET LOGS CALM ENTRY...   …and the judgment, when it may judge them
#
# An ENTRY is four words, <root|zo> <recipe> <rc> <log>: one recipe of that gate, how it ended and its
# output. Each gate's entries are read, in their order, as that gate's log — `==> verify recipe <recipe>`
# over each one's output and `<== verify recipe <recipe> rc=<rc>` under it, as verify-every-recipe.sh
# writes the lane's — red when one of them was red, and
# the plan is the flakes phase's own (solo_plan, flake_reals, solo_units, SOLO_RUNS). --flake-plan prints
# it, one a line, and changes nothing:
#   runs <SOLO_RUNS>
#   real <lane>:<name> ...    what no solo run may clear, when anything is (and then nothing is judged)
#   plan <lane>\t<kind>\t<unit>\t<name>   each failure name, as solo_plan writes it (kind: listed, unlisted, recipe)
#   unit <lane> <unit>        each unit the judgment runs alone SOLO_RUNS times, in the order it runs them
# --judge-flakes prints the same and, when something was red and nothing is real, waits as the flakes
# phase waits (wait_for_calm flakes-solo, at most CALM seconds) and judges through solo_judge: each run
# from the checkout TOP, cargo building in TARGET (empty: cargo's own place), its output in
# LOGS/solo.<lane>.<at>.<run>.log (AT: the unit's place among its gate's units), said as the lane says it
# (`<time> solo <lane> <unit> <run>/<SOLO_RUNS> rc=<rc>`) on its standard output — the job's log — and
# nowhere in the lane's own home ($UNLISTED included). It exits 0 when nothing was red or every unit
# passed every run, 1 when a failure is real or a run was red, and 2 when it was asked wrong.
flake_entries() { # FOLDER ENTRY... — FOLDER/root.log and FOLDER/zo.log as above; FLAKE_RC_ROOT, FLAKE_RC_ZO: 1 when an entry of that gate was red
  local folder=$1 lane recipe rc log
  shift
  [ $# -gt 0 ] || return 2
  : > "$folder/root.log" && : > "$folder/zo.log" || return 2
  FLAKE_RC_ROOT=0; FLAKE_RC_ZO=0
  while [ $# -gt 0 ]; do
    [ $# -ge 4 ] || return 2
    lane=$1 recipe=$2 rc=$3 log=$4; shift 4
    case $lane in root|zo) ;; *) return 2 ;; esac
    [ -n "$rc" ] && [ -f "$log" ] || return 2
    # The output's last line may lack its newline: the next mark starts a line of its own. The closing
    # mark carries the entry's rc as verify-every-recipe.sh's does, so a red entry that named nothing in
    # its own log is red for real here as in the lane (failed_tests).
    { echo "==> verify recipe $recipe"; cat "$log"; echo; echo "<== verify recipe $recipe rc=$rc 0s"; } >> "$folder/$lane.log" || return 2
    if [ "$rc" != 0 ]; then
      if [ "$lane" = root ]; then FLAKE_RC_ROOT=1; else FLAKE_RC_ZO=1; fi
    fi
  done
}
case ${1:-} in
  --flake-plan|--judge-flakes)
    mode=$1; shift
    export LC_ALL=C   # one order of names (failed_tests' sort) for the gate that judged and the reader that judges again
    if [ "$mode" = --judge-flakes ]; then
      [ $# -ge 4 ] || { echo "usage: lane.sh --judge-flakes TOP TARGET LOGS CALM <root|zo> <recipe> <rc> <log>..." >&2; exit 2; }
      top=$1 target=$2 logs=$3 calm=$4; shift 4
      case $calm in ''|*[!0-9]*) echo "lane.sh --judge-flakes: CALM is seconds, not '$calm'" >&2; exit 2 ;; esac
      [ -d "$top" ] && [ -d "$logs" ] || { echo "lane.sh --judge-flakes: no checkout $top or no logs folder $logs" >&2; exit 2; }
    fi
    gates=$(mktemp -d "${TMPDIR:-/tmp}/flake-plan.XXXXXX") || exit 2
    trap 'rm -rf "$gates"' EXIT
    flake_entries "$gates" "$@" \
      || { echo "usage: lane.sh $mode … ENTRY... — each <root|zo> <recipe> <rc> <log>, its log a file" >&2; exit 2; }
    plan=$(solo_plan root "$gates/root.log"; solo_plan zo "$gates/zo.log")
    real=$(flake_reals "$plan" "$FLAKE_RC_ROOT" "$FLAKE_RC_ZO")
    echo "runs $SOLO_RUNS"
    [ -z "$real" ] || echo "real$real"
    printf '%s\n' "$plan" | sed -n 's/^./plan &/p'
    solo_units "$plan" | while IFS=$'\t' read -r lane unit; do [ -z "$lane" ] || echo "unit $lane $unit"; done
    [ "$mode" = --judge-flakes ] || exit 0
    [ -z "$real" ] || exit 1
    [ "$FLAKE_RC_ROOT$FLAKE_RC_ZO" != 00 ] || exit 0
    # Where the runs go and what is said of them are the gate's: its checkout, its target, its logs
    # folder and its own log — never the lane's log, out/ or unlisted.txt.
    UNLISTED=/dev/null; LOG=; CALM_WAIT_SECS=$calm
    run_solo() { solo_road "$1" "$2" "$top" "$target" > "$logs/solo.$1.$4.$3.log" 2>&1 < /dev/null; echo $?; }
    say_unlisted_ahead "$plan"
    wait_for_calm flakes-solo
    solo_judge "$plan" || { echo "flakes: red — $SOLO_RED"; exit 1; }
    echo "flakes: every unit passed $SOLO_RUNS solo runs"
    exit 0 ;;
esac

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
