default: verify

# just defaults to `sh` on every platform, which on Windows means whatever
# Git for Windows happens to have put on PATH. Pinning bash keeps the CI leg
# from depending on that luck — the recipes below use `VAR=x cmd` syntax that
# cmd.exe cannot read.
set windows-shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# CI와 동일한 게이트. 헤드리스 단계(test/lint/doc)는 웹뷰 스택을 빌드하지
# 않도록 zerocode-shell을 제외하고, 창은 shell-lint/shell-test 단계가 따로
# 게이트한다.
# The small source/runner checks answer before the Rust builds start.
verify: pii-check fmt-check package-runner-test quality-runner-test window-runner-test lint doc test shell-lint shell-test tools-test swift-test settings-browser-test window-browser-test profile-browser-test browser-observation-test browser-door-test knowledge-browser-test win-check-if-available

# The re-entry gate for private values (3.9 s over 1,687 tracked files, the
# median of 3 on an idle machine): a home directory, an office address, a
# mailbox, a key of a real shape, or a name this tree was cleared of, must
# not reach the public source snapshot.
# Everything it knows is the `RULES` table in the script (and `BANISHED`, which
# carries digests, never a word) — `pii-scan.py --table` prints it, and a site
# that cannot be rewritten waives itself where it stands. First in `verify`
# because it refuses before anything compiles.
pii-check:
    python3 tools/release/pii-scan.py

# --no-fail-fast here and in shell-test: cargo otherwise stops at the first
# failing test binary, and the release lane re-runs only the names that
# failed — the binaries after a flake would ship unrun (lane 29f88143 ran one
# of fourteen zo test targets and published).
test:
    cargo test --workspace --exclude zerocode-shell --no-fail-fast

lint:
    cargo clippy --workspace --exclude zerocode-shell --all-targets -- -D warnings

# 창 크레이트만의 게이트. 여기만 Tauri/웹뷰 스택을 컴파일하므로 첫 실행이
# 느린 건 이 단계 하나에 격리된다.
shell-lint:
    cargo clippy -p zerocode-shell --all-targets -- -D warnings

# The tools' own tests (stdlib python, ~85 s — the bench runner's end-to-end
# fakes are ~25 s of it — plus the signer's ~14 s and the PII gate's ~5 s): the
# release lane's dry-run contract, the version bump, the PII gate's table read
# from both sides, the app signer and its nested-code gate, the
# scoreboard beat's clock, the Computer Use bench, the Jev ledger reader and
# the token-diet baseline's definitions. They sat outside the gate until
# 2026-09-11 (the signer's until 09-12) — the same silence the knowledge graph
# tests had.
# Every script runs even after one fails, for the reason `test` keeps
# `--no-fail-fast`: the Windows leg stopped at its second script (test_lane.py)
# for its whole life, so 48 of the 50 never reported there (t-74384). The last
# line names every red script. The ones that drive macOS-only tools (codesign,
# launchd, swiftc, osascript, the release lane) say SKIPPED aloud elsewhere, as
# `swift-test` does.
# On Windows Python reads files as UTF-8 here, as it does on a Mac.
# The scripts run one after another, except the long ones marked `beside`: each of
# those starts at once and runs beside the rest, its output held and printed when
# it ends, its verdict counted with the others'. v1.1.138's gate spent 157 s of
# this recipe's 387 s in test_lane.py, 72 s in test_bench.py and 56 s in
# test_gate_receipt.py, each waiting on the one before it.
tools-test:
    @rc=0; red=; skipped=; held=$(mktemp -d "${TMPDIR:-/tmp}/tools-test.XXXXXX"); beside_n=0; \
    case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) export PYTHONUTF8=1 ;; esac; \
    t() { echo "python3 $*" >&2; python3 "$@" || { rc=1; red="$red $*"; }; }; \
    beside() { beside_n=$((beside_n + 1)); printf '%s' "$*" > "$held/$beside_n.cmd"; echo "python3 $* — beside the rest; its output when it ends" >&2; ( python3 "$@" > "$held/$beside_n.out" 2>&1; echo $? > "$held/$beside_n.rc" ) & }; \
    macos() { how=t; [ "$1" = beside ] && { how=beside; shift; }; why=$1; shift; if [ "$(uname -s)" = Darwin ]; then $how "$@"; else echo "SKIPPED python3 $* — macOS-only: $why"; skipped="$skipped $*"; fi; }; \
    beside -I -S -B tools/release/tests/test_gate_receipt.py --receipt-mode routine -v; \
    macos beside "the release lane runs on this Mac: launchd, plutil, POSIX signals, the build line's done folder" tools/release/tests/test_lane.py; \
    t tools/release/tests/test_cargo_turn.py; \
    t tools/release/tests/test_coord_receipt.py; \
    t tools/release/tests/test_prebatch.py; \
    t tools/release/tests/test_lock_answer.py; \
    t tools/release/tests/test_workspace_units.py; \
    t tools/release/tests/test_changed_crates.py; \
    t tools/release/tests/test_zo_ide_touched.py; \
    macos "win-check cross-compiles from a Mac (cargo-xwin, Homebrew llvm)" tools/release/tests/test_win_check.py; \
    t tools/release/tests/test_bump.py; \
    t tools/release/tests/test_pii_scan.py; \
    macos "signs with macOS codesign and the login keychain" tools/signing/tests/test_signing.py; \
    macos "signs a macOS .app bundle (codesign, Mach-O)" tools/signing/tests/test_sign_app_bundle.py; \
    macos "renders a launchd plist" tools/scoreboard/tests/test_scoreboard_launchd.py; \
    macos "times the helper's round trip through a shell on this Mac" tools/computer-bench/test_batch_rtt.py; \
    macos "builds the bench's AppKit fixtures with swiftc" tools/computer-bench/test_background_input.py; \
    t tools/computer-bench/test_tally.py; \
    macos "walks the macOS screen and accessibility permissions" tools/computer-bench/test_onboarding.py; \
    t tools/computer-bench/test_scenarios.py; \
    macos beside "scripts macOS ioreg, pgrep, defaults, open and osascript" tools/computer-bench/test_bench.py; \
    t tools/computer-bench/test_marks.py; \
    t tools/computer-bench/test_eye.py; \
    t tools/computer-bench/test_fixture_apm.py; \
    t tools/computer-bench/test_fixture_reflex.py; \
    t tools/computer-bench/test_fixture_support.py; \
    t tools/computer-bench/test_fixture_rts.py; \
    t tools/computer-bench/long-form/test_long_form.py; \
    t tools/tests/test_decision_shadow_summary.py; \
    t tools/tests/test_jev_token_diet_baseline.py; \
    t tools/tests/test_hedge_replay_seed.py; \
    t tools/tests/test_summon_replay_seed.py; \
    t tools/tests/test_summon_difficulty_replay_seed.py; \
    t tools/tests/test_compaction_replay_seed.py; \
    t tools/tests/test_agent_tool_replay_seed.py; \
    t tools/tests/test_browser_read_replay_seed.py; \
    t tools/tests/test_notify_replay_seed.py; \
    t tools/tests/test_mention_rerank_replay_seed.py; \
    t tools/tests/test_branching_replay_seed.py; \
    t tools/tests/test_judgment_cache_replay_seed.py; \
    t tools/tests/test_patch_review_replay_seed.py; \
    t tools/tests/test_routing_replay_seed.py; \
    t tools/tests/test_label_audit_seed.py; \
    t tools/tests/test_jev_seat_replay.py; \
    t tools/tests/test_jev_answered.py; \
    t tools/tests/test_vault_pairs_replay_seed.py; \
    t tools/tests/test_command_guard_replay_seed.py; \
    t tools/tests/test_ask_wait_replay_seed.py; \
    t tools/tests/test_mail_triage_replay_seed.py; \
    t tools/tests/test_question_discovery.py; \
    t tools/tests/test_jev_review_study.py; \
    t tools/tests/test_challenger_replay_seed.py; \
    t tools/tests/test_delegation_eval.py; \
    t tools/tests/test_claude_code_panel_rules.py; \
    t tools/tests/test_codegraph_bench.py; \
    t tools/tests/test_test_times.py; \
    t tools/tests/test_zo_bench_run_tasks.py; \
    macos "reads proc_pid_rusage through macOS libproc.dylib and mach_absolute_time" tools/tests/test_memory_census_idle.py; \
    macos "ends the measured CLI's process group (os.killpg)" tools/login-renewal-probe/test_probe.py; \
    wait; \
    for cmd in "$held"/*.cmd; do [ -e "$cmd" ] || continue; one=${cmd%.cmd}; echo "python3 $(cat "$cmd") — ended beside the rest:" >&2; cat "$one.out"; [ "$(cat "$one.rc" 2>/dev/null)" = 0 ] || { rc=1; red="$red $(cat "$cmd")"; }; done; rm -rf "$held"; \
    [ -z "$skipped" ] || echo "tools-test: skipped (macOS-only):$skipped"; \
    [ -z "$red" ] || { echo "tools-test: red scripts:$red" >&2; exit 1; }; exit $rc

# The native helpers' own tests (Swift, 178 + 5 on 2026-09-21): the pure Core
# the Computer Use helper's main.swift calls and the case tables it shares with
# the Rust core, then the iOS emulator helper's own Core — the part of its
# exporter that can be judged without a booted simulator. They sat in no recipe
# until the bench (E2) — the silence the tools' tests had. ~17 s + ~13 s cold
# (a fresh scratch builds each package in debug), ~2 s warm. Every package runs
# even after one fails, for the reason `test` keeps `--no-fail-fast`. Elsewhere
# it says SKIPPED aloud: both helpers are macOS-only.
swift-test:
    if [ "$(uname -s)" = Darwin ]; then rc=0; for helper in computer-use-macos ios-emulator-helper; do swift test --package-path "crates/zerocode-shell/native/$helper" || rc=1; done; exit $rc; else echo "swift-test SKIPPED: the native helpers are macOS-only"; fi

# 창의 순수 로직(서버 응답 파싱, porcelain, 브랜치 읽기) 단위 테스트.
# clippy --all-targets가 이미 테스트를 컴파일해 두므로 실행 비용만 든다.
shell-test:
    cargo test -p zerocode-shell --no-fail-fast

# Small runner checks: no browser, cargo build, or real-provider call.
package-runner-test:
    npm run test:package-contract

quality-runner-test:
    npm run test:quality-runner

# 설정은 브라우저 두 창과 하나의 stateful backend 사이의 저장·rollback·
# revision 전파 계약이며, 같은 실제 레이아웃에서 키보드·WCAG·다국어·성능
# 예산도 검사한다. Playwright/Chromium이 없으면 이 gate 자체를 실행하지 못한
# 것이므로 suite가 명확한 실패로 끝낸다.
settings-browser-test:
    node ui/tests/settings.mjs

# The window harness's runner, on its own (t-4017): suites, selection, a
# suite's throw as one FAIL line, the report's text contract. Pure node, no
# browser. It also checks pane lifetimes and the loaded scripts' semantic
# scopes before the Rust build and browser measurements start.
# Beside it the IPC census's arithmetic (t-20972): what a body weighs on the
# wire, how repeats and bursts are counted. And the script a hidden pane's page
# is told before its first script — no bridge (t-20972). And the frame budget's
# rule on a platform that cannot read the load: recorded, never judged (t-43414).
# And the error reporter, which never throws over the error it was handed when
# the terminal script stopped while loading (t-94962).
window-runner-test:
    status=0; node --test ui/tests/window-runner.test.mjs ui/tests/ipc-census.test.mjs ui/tests/hidden-pane-bridge.test.mjs ui/tests/machine-load.test.mjs ui/tests/background-surface-focus.test.mjs ui/tests/jev-real-summary.test.mjs ui/tests/show-error.test.mjs ui/tests/pane-lifecycle.test.mjs ui/tests/pane-close-door.test.mjs ui/tests/pane-late-answer.test.mjs ui/tests/pane-late-seed.test.mjs ui/tests/pane-late-log.test.mjs ui/tests/window-scope.test.mjs ui/tests/vm-fixtures.test.mjs || status=$?; [ "$status" -lt 128 ] || exit "$status"; node scripts/window-scope.mjs || status=$?; exit "$status"

# 설정뿐 아니라 전체 renderer interaction을 실제 layout engine에서 확인한다.
# 이 gate가 있어야 설정 변경이 탭/보드/온보딩 같은 인접 UI를 깨뜨리지 않았다는
# 전체 renderer interaction 계약도 CI와 로컬에서 같은 명령으로 검증된다.
# One harness a recipe (2026-11-10): just ends a recipe at its first red line, so
# a second script after window.mjs never ran when a window check was red — the
# v1.1.139 lane shipped without the profile-data checks having run. The release
# lane names a check by its recipe (`ui:<harness>/<suite>:<check>`) and judges a
# suite alone by running that recipe with WINDOW_SUITES, so each harness the
# lane judges is one script under one `<harness>-browser-test`.
window-browser-test:
    node ui/tests/window.mjs

# A browser profile's imported Chrome data in the window (t-79331).
profile-browser-test:
    node ui/tests/browser-profile-data.mjs

# Guest error/network observation must also run in a fresh protocol session.
# Every script runs, whatever the one before it said: the recipe is red when any
# was, and a script ended by a signal (128+N, the lane's cap) ends it at once.
browser-observation-test:
    rc=0; for script in browser-ring browser-form-fill; do status=0; node "ui/tests/$script.mjs" || status=$?; [ "$status" -lt 128 ] || exit "$status"; [ "$status" = 0 ] || rc=1; done; exit "$rc"

# The browser door's page scripts (zcSelect, click, wait) against a page with
# hidden twins of its controls: the element a selector names is the first one
# a person could see. Reads the strings the Rust hands the pane.
browser-door-test:
    rc=0; for script in browser-door expense-workflow; do status=0; node "ui/tests/$script.mjs" || status=$?; [ "$status" -lt 128 ] || exit "$status"; [ "$status" = 0 ] || rc=1; done; exit "$rc"

# The knowledge graph's 14 browser tests (first paint, frame gap, DOM budget,
# determinism, hex-literal ban, slicer, path). Outside the gate this file sat
# red for four days after t-2931 (2026-09-07 → 09-11) and nobody saw it.
knowledge-browser-test:
    node ui/tests/test-knowledge-graph.mjs

# The long-form Computer Use bench's page, fake desk and oracle in Chromium,
# without a model (t-37883): a scripted hand through the desktop verbs passes,
# a run that does nothing fails. A bench's own check, run by the build line
# beside a measurement rather than by `verify`. The desk asks the core's
# `door_text` example what the window's door makes of each line (t-89077).
long-form-test:
    cargo build -p zerocode-core --example door_text --example words_choose
    DOOR_TEXT="${CARGO_TARGET_DIR:-target}/debug/examples/door_text" WORDS_CHOOSE="${CARGO_TARGET_DIR:-target}/debug/examples/words_choose" node tools/computer-bench/long-form/test-desk.mjs

# The form bench's stand-in browser door, its shim and the runner of a real
# agent, held to what the window answers (t-41387): no model and no login — a
# shell script stands where `claude -p` stands. Builds the core's `door_text`
# example first, which says what the window says. A bench's own check, run by
# the build line, not by `verify`.
form-bench-test:
    cargo build -p zerocode-core --example door_text
    node tools/computer-bench/form-scenes/test-desk.mjs --door-text "${CARGO_TARGET_DIR:-target}/debug/examples/door_text"
    node tools/computer-bench/form-scenes/test-real.mjs --door-text "${CARGO_TARGET_DIR:-target}/debug/examples/door_text"
    node tools/computer-bench/form-scenes/test-kit.mjs
    node tools/computer-bench/form-scenes/test-drive.mjs

# Release builds use a deliberately small frontend tree. Keeping this separate
# from ui/ means browser fixtures and prototypes remain available to developers
# without being embedded in the desktop binary.
ui-dist:
    npm run build:ui-dist

ui-dist-check: ui-dist
    npm run check:ui-dist

# Unsigned packaging smoke gates. Distribution signing is tag-only in CI;
# these commands prove that each native bundler can produce and inspect both
# supported formats without pretending an unsigned artifact is releasable.
# macOS also mounts the DMG and launches from an isolated HOME. The Windows
# install/launch gate is intentionally GitHub-runner-only because Windows Known
# Folders cannot be redirected safely for a developer's real profile.
# `createUpdaterArtifacts` is on in tauri.conf.json for the release lane's
# bundle-updater phase; tauri-cli refuses it without plugins.updater, so the
# unsigned smoke turns it off the way the lane's local build does.
package-macos:
    npm run test:package-contract
    CI=true npm run tauri:release -- --bundles app,dmg --ci --no-sign --config '{"bundle":{"createUpdaterArtifacts":false}}'
    npm run check:package -- macos
    scripts/native-package-smoke.macos.sh

# Windows cross-compile of the whole workspace, linted — the only place
# `cfg(windows)` / `cfg(not(unix))` code compiles and meets clippy on this Mac
# (the Windows CI leg builds and lints it for real; when that leg is dark this is
# what stands in). Needs `cargo install cargo-xwin` and `brew install llvm`
# (`llvm-lib`), like zo-ide's `win-check`.
# 2026-09-10: found 7 product compile errors nobody had seen (tauri macOS-only
# APIs, a stub signature, a Rust-2024 extern block) and 3 unix-only tests.
# 2026-10-01: it only checked, so the first public run's Windows leg was the
# first to stop on lints that exist on Windows alone (items only macOS uses,
# needless returns, unused variables); it lints with the CI leg's `-D warnings`.
win-check:
    PATH="/opt/homebrew/opt/llvm/bin:$PATH" cargo xwin clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings

# The same cross-compile as a `verify` member: runs where cargo-xwin and
# llvm-lib are installed (this Mac, the release lane) and says SKIPPED aloud
# where they are not (a CI macOS runner — the Windows leg compiles for real
# there). A skip is printed, never silent, so a machine without the tools
# knows it did not look.
win-check-if-available:
    if ! command -v cargo-xwin >/dev/null 2>&1 || [ ! -x /opt/homebrew/opt/llvm/bin/llvm-lib ]; then echo "win-check SKIPPED: cargo-xwin or llvm-lib not installed (the Windows CI leg compiles this for real)"; elif [ ! -d "${CARGO_TARGET_DIR:-target}/x86_64-pc-windows-msvc" ]; then echo "win-check SKIPPED — not a check: cold Windows target. \`just win-check\` cross-compiles every dependency once (1–2 min on this Mac in 2026-10; the release lane's scratch clone sat in it 2 h on 2026-09-10), and \`just pre-handin\` runs it for a branch that changed Rust"; else just win-check; fi

# A recipe as the release lane's launchd job runs it — HOME and PATH only, no
# controlling terminal, launchd's open-file limit (tools/release/lane-env.sh) —
# for a test that is red in the lane and green in a terminal:
#   just lane-env shell-test        just lane-env test -p zerocode-core
[positional-arguments]
lane-env recipe *args:
    tools/release/lane-env.sh just "$@"

package-windows:
    npm run test:package-contract
    CI=true npm run tauri:release -- --bundles msi,nsis --ci --no-sign --config '{"bundle":{"createUpdaterArtifacts":false}}'
    npm run check:package -- windows
    if [[ "${GITHUB_ACTIONS:-}" == "true" && "${RUNNER_ENVIRONMENT:-}" == "github-hosted" ]]; then pwsh -NoProfile -File scripts/native-package-smoke.windows.ps1; else echo "Windows install/launch smoke runs only on an ephemeral GitHub-hosted runner"; fi

# The rustdoc flags, and the crate `doc` leaves out (its webview stack is a
# build of minutes), are shared with `pre-handin`, which builds the docs of the
# crates it tests.
doc_flags := "-D warnings"
doc_skip := "zerocode-shell"

# rustdoc은 clippy가 못 보는 것을 잡는다 — 이름이 바뀐 타입을 가리키는 깨진
# 링크는 문서를 조용히 거짓말하게 만든다. 경고를 에러로 올려 게이트에 넣는다.
doc:
    RUSTDOCFLAGS="{{doc_flags}}" cargo doc --workspace --exclude {{doc_skip}} --no-deps

fmt:
    cargo fmt --all

# What a worker runs before `worker_done`, and writes the `rc=` line of into the
# hand-in. On 2026-10-28 four gate reds cost the lane 1 h 43 m, and each was
# caught beforehand by a check of 4 s to 23 min. Every stage runs even after one
# fails (as `verify-every-recipe.sh` does), then one line says each stage's exit
# code and what to do next for a red one, and the recipe exits 1 if any was not 0.
#   Stage 1, always: the formatter of every Cargo workspace, the lock file of each
#   (a dependency a crate takes also changes the lock of a workspace that reads
#   that crate by path), the private-value gate (the gate's own `fmt-check`
#   recipes), the window's source contracts,
#   and the unit tests and the rustdoc (the gate's `doc` flags; not the crate
#   `doc` leaves out) of each crate the branch changed or reads a changed file
#   of (`tools/release/changed_crates.py` picks them from Cargo's package list,
#   the files the sources `include_str!`, and the diff against main — no list
#   here, and no workspace name either). A file only a `tests/` source reads
#   picks nothing: the contracts above are that target. NOT run: the crates that
#   depend on a changed crate, and integration tests other than the contracts —
#   the gate runs those (the last lines say so). The contracts build the window
#   once, so a first run in a fresh checkout takes minutes on every core,
#   whichever crate the branch changed.
#   Then the Windows build of each workspace the branch changed a Rust source or
#   manifest of (`changed_crates.py` names it): the gate's own `just win-check`
#   there, never `win-check-if-available` — whose SKIPPED on a cold target is
#   rc 0 and was counted as a check in a hand-in (2026-11-10). It cross-compiles
#   every dependency the first time in a checkout (1–2 min on this Mac, 2026-10),
#   and a machine without cargo-xwin and Homebrew llvm is red here, not skipped.
#   Stage 2, only when ui/ changed: the window suites you name, with
#   `just pre-handin a,b` (names or patterns, as WINDOW_SUITES takes them).
#   The whole window gate is 23 min on a quiet machine, so a worker never runs
#   it: without suites the stage stops, says which ui files changed and which
#   suites look at them, and the recipe exits 1. The load before the run is in
#   the last line: the window's frame budgets are judged only when the load is
#   at most the core count, so a red on a busier machine is worth a rerun.
# A branch cut from a stack tip, not from main, measures from that tip:
# `PRE_HANDIN_BASE=<tip> just pre-handin`. A stage that stopped is `2`; one that
# did not apply is `-`, and so is a lock cargo could not answer for offline: only
# cargo's own "--locked" refusal is red, as in prebatch.py.
pre-handin suites='':
    #!/usr/bin/env bash
    set -u
    suites={{quote(suites)}}
    doc_flags={{quote(doc_flags)}}
    doc_skip={{quote(doc_skip)}}
    broad_over=4    # a suite hint of more than 1/broad_over of the window's suites is not much of a hint
    names=(); codes=(); secs=(); hints=(); extras=()
    hint=; extra=; units=0
    note() {  # note <name> <code> [seconds]: what the last line says of a stage
        names+=("$1"); codes+=("$2"); secs+=("${3:-0}"); hints+=("$hint"); extras+=("$extra"); hint=; extra=
    }
    step() {  # step <name> <command…>: run it and note its exit code and seconds
        local name=$1 started=$SECONDS code; shift
        echo "== pre-handin: $name" >&2
        "$@"; code=$?
        note "$name" "$code" "$((SECONDS - started))"
    }
    lock_step() {  # lock_step <name> <folder>: prebatch's verdict; what it cannot answer is `-`
        local name=$1 started=$SECONDS code
        echo "== pre-handin: $name" >&2
        python3 tools/release/lock_answer.py "$2"; code=$?
        if [ "$code" -eq 3 ]; then
            echo "pre-handin: $name: no answer here (cargo could not read it offline, not a lock behind its manifest); the gate looks" >&2
            hint=; code=-
        fi
        note "$name" "$code" "$((SECONDS - started))"
    }
    hint='cargo fmt --all'
    step fmt just fmt-check
    workspaces=$(python3 tools/release/changed_crates.py --workspaces) || { hint='the message above says why'; note workspaces 1; workspaces=; }
    for workspace in $workspaces; do
        hint="the diff above names the files in $workspace/ — format those, not the whole workspace (rustfmt <file>)"
        step "fmt-$workspace" bash -c 'cd "$1" && just fmt-check' _ "$workspace"
    done
    hint='cargo metadata --offline, then commit the Cargo.lock it changed'
    lock_step lock .
    for workspace in $workspaces; do
        hint="(cd $workspace && cargo metadata --offline), then commit the Cargo.lock it changed: a dependency a crate it reads by path took changes it too"
        lock_step "lock-$workspace" "$workspace"
    done
    hint='the lines above name the file and the rule; take the value out or mask it, then just pii-check'
    step pii just pii-check
    hint='fix the failed Node checks above; install the pinned dependencies with npm ci if they are missing'
    step package-runner just package-runner-test
    hint='fix the failed real-runner policy checks above (these contact no provider)'
    step quality-runner just quality-runner-test
    hint='fix the failed window source/runner checks above before the Rust source contracts build'
    step window-runner just window-runner-test
    hint='fix the FAILED names above, then rerun one: cargo test -p zerocode-shell --test source_contracts <name>'
    step contracts cargo test -p zerocode-shell --test source_contracts
    picked=$(python3 tools/release/changed_crates.py ${PRE_HANDIN_BASE:+--base "$PRE_HANDIN_BASE"}); pick=$?
    hint='the message above says why; name the branch point with PRE_HANDIN_BASE=<commit>'
    note pick "$pick"
    if [ "$pick" -eq 0 ]; then
        while IFS=$'\t' read -r kind workspace package arguments; do
            [ "$kind" = crate ] || continue
            hint="fix the FAILED names above, then rerun one: (cd $workspace && cargo test -p $package $arguments <name>)"
            step "unit:$package" bash -c 'cd "$1" && cargo test -p "$2" $3' _ "$workspace" "$package" "$arguments"
            units=1
            if [ "$workspace" = . ] && [ "$package" != "$doc_skip" ]; then
                hint="fix the rustdoc warnings above: RUSTDOCFLAGS='$doc_flags' cargo doc -p $package --no-deps"
                step "doc:$package" env RUSTDOCFLAGS="$doc_flags" cargo doc -p "$package" --no-deps
            fi
        done <<< "$picked"
        for workspace in $(printf '%s\n' "$picked" | awk -F'\t' '$1 == "windows" { print $2 }'); do
            name=win; [ "$workspace" = . ] || name="win-$workspace"
            hint="fix what the cross-compile says above, then rerun (cd $workspace && just win-check); without cargo-xwin or Homebrew llvm here: cargo install cargo-xwin; brew install llvm"
            step "$name" bash -c 'cd "$1" && just win-check' _ "$workspace"
        done
        ui=$(printf '%s\n' "$picked" | awk -F'\t' '$1 == "ui" { print $2 }')
    else
        ui=
    fi
    load=$(uptime | sed 's/.*load averages*: *//')
    cores=$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu)
    if [ "$pick" -ne 0 ]; then
        echo "pre-handin: the window stage is undecided — the pick failed (message above), so which ui files changed is not known" >&2
        hint='fix the pick first (the message above), then rerun'
        note window 2
    elif [ -z "$ui" ]; then
        note window -
    elif [ -n "$suites" ]; then
        hint='the FAILED lines above name the checks; rerun one suite: WINDOW_SUITES=<name> node ui/tests/window.mjs'
        extra=" load $load, $cores cores"
        step window env WINDOW_SUITES="$suites" node ui/tests/window.mjs
    else
        echo "pre-handin: ui changed ($(echo "$ui" | wc -l | tr -d ' ') files, first: $(echo "$ui" | head -1)) — name the window suites to run:" >&2
        echo "  just pre-handin <a,b>   (names or patterns; the window harness registers the suite names)" >&2
        hinted=0; total=0
        read -r hinted total < <(printf '%s\n' "$picked" | awk -F'\t' '$1 == "suites" { print $2, $3 }')
        echo "  suites that look at the changed files ($hinted of $total): $(printf '%s\n' "$picked" | awk -F'\t' '$1 == "suite" { printf "%s%s", sep, $2; sep = "," }')" >&2
        if [ $((hinted * broad_over)) -gt "$total" ]; then
            echo "  that is more than 1/$broad_over of the window's suites: choose the few that draw what you changed, not all of them" >&2
        fi
        hint='name the suites above (or others that draw what you changed): just pre-handin <a,b>'
        note window 2
    fi
    line=; rc=0; next=
    for at in "${!names[@]}"; do
        line="$line ${names[$at]}=${codes[$at]}"
        [ "${codes[$at]}" = - ] || line="$line(${secs[$at]}s${extras[$at]})"
        case ${codes[$at]} in
            0|-) ;;
            *) rc=1; next="$next"$'\n'"  ${names[$at]}: ${hints[$at]}" ;;
        esac
    done
    if [ "$units" -ne 0 ]; then
        echo "pre-handin: units and docs ran for the crates the branch changed or reads files of, not the crates that depend on them, nor integration tests other than the contracts — the gate runs those" >&2
    fi
    echo "pre-handin:$line rc=$rc"
    [ -z "$next" ] || echo "next:$next"
    exit "$rc"

fmt-check:
    cargo fmt --all --check

# 디자인 프로토타입을 연다. 토큰 파일을 직접 링크하므로
# ui/tokens.css를 고치면 그대로 반영된다.
prototype:
    open ui/prototype/index.html

# 창을 띄운다. 첫 빌드는 웹뷰 스택 때문에 몇 분 걸린다.
shell:
    cargo run -p zerocode-shell

# 설치된 Electron 앱을 읽기 전용으로 추출한다(리버스 작업용).
# 결과는 .re-scratch/ 아래에만 쌓이고 커밋되지 않는다.
teardown app="/Applications/Orca.app":
    cargo run -q -p asar-reader -- extract "{{ app }}/Contents/Resources/app.asar" .re-scratch/asar

# 리버스 스크래치 비우기.
teardown-clean:
    rm -rf .re-scratch

# Deterministic workflow quality; separate from the regression gate (no providers).
baseline *args:
    node fixtures/quality-baseline/runner.mjs {{args}}

# The second brain's lint, over the vault ZEROCODE_SECOND_BRAIN names (or the
# one the window saved, or `--vault <dir>`). Prints the same table the window's
# vault health card paints — one producer, `zerocode_core::second_brain_lint` —
# and exits 1 while it holds a finding, so a cron or a hook can fail on it.
# `--json` prints the table itself. The fixture vault under fixtures/vault-lint
# is one of each finding and is what `cargo test -p zerocode-app` pins.
vault-lint *args:
    cargo run -q -p zerocode-app -- vault-lint {{args}}
