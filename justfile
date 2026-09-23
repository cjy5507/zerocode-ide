default: verify

# just defaults to `sh` on every platform, which on Windows means whatever
# Git for Windows happens to have put on PATH. Pinning bash keeps the CI leg
# from depending on that luck — the recipes below use `VAR=x cmd` syntax that
# cmd.exe cannot read.
set windows-shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# CI와 동일한 게이트. 헤드리스 단계(test/lint/doc)는 웹뷰 스택을 빌드하지
# 않도록 zerocode-shell을 제외하고, 창은 shell-lint/shell-test 단계가 따로
# 게이트한다.
verify: pii-check fmt-check lint doc test shell-lint shell-test tools-test swift-test window-runner-test settings-browser-test window-browser-test browser-observation-test browser-door-test knowledge-browser-test win-check-if-available

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
tools-test:
    python3 tools/release/tests/test_lane.py
    python3 tools/release/tests/test_bump.py
    python3 tools/release/tests/test_pii_scan.py
    python3 tools/signing/tests/test_signing.py
    python3 tools/signing/tests/test_sign_app_bundle.py
    python3 tools/scoreboard/tests/test_scoreboard_launchd.py
    python3 tools/computer-bench/test_batch_rtt.py
    python3 tools/computer-bench/test_tally.py
    python3 tools/computer-bench/test_onboarding.py
    python3 tools/computer-bench/test_scenarios.py
    python3 tools/computer-bench/test_bench.py
    python3 tools/computer-bench/test_marks.py
    python3 tools/computer-bench/test_eye.py
    python3 tools/computer-bench/test_fixture_apm.py
    python3 tools/tests/test_decision_shadow_summary.py
    python3 tools/tests/test_jev_token_diet_baseline.py
    python3 tools/tests/test_hedge_replay_seed.py
    python3 tools/tests/test_summon_replay_seed.py
    python3 tools/tests/test_compaction_replay_seed.py
    python3 tools/tests/test_agent_tool_replay_seed.py
    python3 tools/tests/test_browser_read_replay_seed.py
    python3 tools/tests/test_notify_replay_seed.py
    python3 tools/tests/test_mention_rerank_replay_seed.py
    python3 tools/tests/test_branching_replay_seed.py
    python3 tools/tests/test_judgment_cache_replay_seed.py
    python3 tools/tests/test_patch_review_replay_seed.py
    python3 tools/tests/test_label_audit_seed.py
    python3 tools/tests/test_claude_code_panel_rules.py
    python3 tools/tests/test_codegraph_bench.py

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

# 설정은 브라우저 두 창과 하나의 stateful backend 사이의 저장·rollback·
# revision 전파 계약이며, 같은 실제 레이아웃에서 키보드·WCAG·다국어·성능
# 예산도 검사한다. Playwright/Chromium이 없으면 이 gate 자체를 실행하지 못한
# 것이므로 suite가 명확한 실패로 끝낸다.
settings-browser-test:
    node ui/tests/settings.mjs

# The window harness's runner, on its own (t-4017): suites, selection, a
# suite's throw as one FAIL line, the report's text contract. Pure node, no
# browser — it says in 100 ms whether the harness can still report at all.
window-runner-test:
    node --test ui/tests/window-runner.test.mjs

# 설정뿐 아니라 전체 renderer interaction을 실제 layout engine에서 확인한다.
# 이 gate가 있어야 설정 변경이 탭/보드/온보딩 같은 인접 UI를 깨뜨리지 않았다는
# 전체 renderer interaction 계약도 CI와 로컬에서 같은 명령으로 검증된다.
window-browser-test:
    node ui/tests/window.mjs

# Guest error/network observation must also run in a fresh protocol session.
browser-observation-test:
    node ui/tests/browser-ring.mjs

# The browser door's page scripts (zcSelect, click, wait) against a page with
# hidden twins of its controls: the element a selector names is the first one
# a person could see. Reads the strings the Rust hands the pane.
browser-door-test:
    node ui/tests/browser-door.mjs

# The knowledge graph's 14 browser tests (first paint, frame gap, DOM budget,
# determinism, hex-literal ban, slicer, path). Outside the gate this file sat
# red for four days after t-2931 (2026-09-07 → 09-11) and nobody saw it.
knowledge-browser-test:
    node ui/tests/test-knowledge-graph.mjs

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

# Windows cross-compile of the whole workspace — the only place `cfg(windows)` /
# `cfg(not(unix))` code compiles on this Mac (the Windows CI leg builds it for
# real; when that leg is dark this is what stands in). Needs `cargo install
# cargo-xwin` and `brew install llvm` (`llvm-lib`), like zo-ide's `win-check`.
# 2026-09-10: found 7 product compile errors nobody had seen (tauri macOS-only
# APIs, a stub signature, a Rust-2024 extern block) and 3 unix-only tests.
win-check:
    PATH="/opt/homebrew/opt/llvm/bin:$PATH" cargo xwin check --workspace --all-targets --target x86_64-pc-windows-msvc

# The same cross-compile as a `verify` member: runs where cargo-xwin and
# llvm-lib are installed (this Mac, the release lane) and says SKIPPED aloud
# where they are not (a CI macOS runner — the Windows leg compiles for real
# there). A skip is printed, never silent, so a machine without the tools
# knows it did not look.
win-check-if-available:
    if ! command -v cargo-xwin >/dev/null 2>&1 || [ ! -x /opt/homebrew/opt/llvm/bin/llvm-lib ]; then echo "win-check SKIPPED: cargo-xwin or llvm-lib not installed (the Windows CI leg compiles this for real)"; elif [ ! -d "${CARGO_TARGET_DIR:-target}/x86_64-pc-windows-msvc" ]; then echo "win-check SKIPPED: cold Windows target — a fresh checkout would cross-compile every dependency (the release lane's scratch clone sat in it for 2 h on 2026-09-10); run \`just win-check\` once to warm this checkout"; else just win-check; fi

package-windows:
    npm run test:package-contract
    CI=true npm run tauri:release -- --bundles msi,nsis --ci --no-sign --config '{"bundle":{"createUpdaterArtifacts":false}}'
    npm run check:package -- windows
    if [[ "${GITHUB_ACTIONS:-}" == "true" && "${RUNNER_ENVIRONMENT:-}" == "github-hosted" ]]; then pwsh -NoProfile -File scripts/native-package-smoke.windows.ps1; else echo "Windows install/launch smoke runs only on an ephemeral GitHub-hosted runner"; fi

# rustdoc은 clippy가 못 보는 것을 잡는다 — 이름이 바뀐 타입을 가리키는 깨진
# 링크는 문서를 조용히 거짓말하게 만든다. 경고를 에러로 올려 게이트에 넣는다.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --exclude zerocode-shell --no-deps

fmt:
    cargo fmt --all

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
