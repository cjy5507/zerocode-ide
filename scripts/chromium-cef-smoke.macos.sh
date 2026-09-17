#!/usr/bin/env bash
# Actual macOS CEF child-view smoke. It intentionally builds no Rust/Node code.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "chromium-cef-smoke.macos.sh requires macOS" >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
binary=""
out=""
keep=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary)
      [[ $# -ge 2 && -f "$2" && -z "$binary" ]] || { echo "invalid --binary" >&2; exit 2; }
      binary="$2"
      shift 2
      ;;
    --out)
      [[ $# -ge 2 && "$2" = /* && -z "$out" ]] || { echo "--out must be one absolute path" >&2; exit 2; }
      out="$2"
      shift 2
      ;;
    --keep)
      keep=true
      shift
      ;;
    *)
      echo "usage: scripts/chromium-cef-smoke.macos.sh --binary /absolute/path/to/zerocode-shell --out /absolute/path/to/cef-smoke.png [--keep]" >&2
      exit 2
      ;;
  esac
done

[[ -n "$binary" && -n "$out" ]] || { echo "--binary and --out are required" >&2; exit 2; }
[[ -x "$binary" ]] || { echo "native shell is not executable: $binary" >&2; exit 2; }
[[ "$(basename "$binary")" != "zerocode-cef-helper" ]] || { echo "--binary must be the main native shell, not zerocode-cef-helper" >&2; exit 2; }
[[ ! -e "$out" ]] || { echo "refusing to overwrite screenshot: $out" >&2; exit 2; }
[[ -d "$(dirname "$out")" ]] || { echo "screenshot parent does not exist: $(dirname "$out")" >&2; exit 2; }

chromium_stage="$root/crates/zerocode-shell/bin/chromium"
frameworks="$chromium_stage/Frameworks"
fixture="$root/fixtures/chromium-cef-smoke/server.py"
[[ -x "$fixture" || -f "$fixture" ]] || { echo "missing loopback fixture: $fixture" >&2; exit 2; }
[[ -f "$frameworks/Chromium Embedded Framework.framework/Chromium Embedded Framework" ]] || { echo "missing staged CEF framework" >&2; exit 2; }
for helper in "ZeroCode Helper" "ZeroCode Helper (GPU)" "ZeroCode Helper (Renderer)" "ZeroCode Helper (Plugin)" "ZeroCode Helper (Alerts)"; do
  [[ -x "$frameworks/$helper.app/Contents/MacOS/$helper" ]] || { echo "missing staged CEF helper: $helper" >&2; exit 2; }
done

sandbox="$(mktemp -d "${TMPDIR:-/tmp}/zerocode-cef-smoke.XXXXXX")"
app="$sandbox/ZeroCode CEF Smoke.app"
home="$sandbox/home"
tmp="$sandbox/tmp"
state_root="$home/Library/Application Support/dev.zerocode.app"
request_log="$sandbox/requests.jsonl"
port_file="$sandbox/fixture-port"
app_log="$sandbox/app.log"
fixture_log="$sandbox/fixture.log"
app_pid=""
fixture_pid=""
finished=false

terminate() {
  local pid="$1"
  [[ -n "$pid" ]] || return 0
  if kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    for _ in {1..40}; do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -0 "$pid" 2>/dev/null && kill -KILL "$pid" 2>/dev/null || true
  fi
  wait "$pid" 2>/dev/null || true
}

stop_app() {
  osascript -l JavaScript -e "ObjC.import('AppKit'); \$.NSRunningApplication.runningApplicationWithProcessIdentifier($app_pid).terminate;" >/dev/null
  for _ in {1..150}; do
    if ! kill -0 "$app_pid" 2>/dev/null; then
      wait "$app_pid"
      app_pid=""
      if grep -q 'Chromium 종료 제한시간' "$app_log"; then
        echo "CEF did not destroy every browser before normal shutdown" >&2
        return 1
      fi
      return 0
    fi
    sleep 0.1
  done
  echo "the isolated app did not finish a normal quit" >&2
  tail -60 "$app_log" >&2
  return 1
}

cleanup() {
  terminate "$app_pid"
  terminate "$fixture_pid"
  if [[ "$finished" == true && "$keep" == false ]]; then
    rm -rf -- "$sandbox"
  else
    echo "CEF smoke evidence retained: $sandbox" >&2
  fi
}
trap cleanup EXIT INT TERM

mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$home/Library/Keychains" "$tmp" "$state_root"
# Stored cookies need encryption; never use the person's login keychain for this fixture.
keychain="$home/Library/Keychains/login.keychain-db"
HOME="$home" CFFIXED_USER_HOME="$home" security create-keychain -p smoke-fixture-only "$keychain"
HOME="$home" CFFIXED_USER_HOME="$home" security default-keychain -d user -s "$keychain"
HOME="$home" CFFIXED_USER_HOME="$home" security unlock-keychain -p smoke-fixture-only "$keychain"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>zerocode-shell</string>
  <key>CFBundleIdentifier</key><string>dev.zerocode.app</string>
  <key>CFBundleName</key><string>ZeroCode CEF Smoke</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.3.4</string>
  <key>CFBundleVersion</key><string>1.3.4</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
</dict></plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null
ditto "$binary" "$app/Contents/MacOS/zerocode-shell"
ditto "$frameworks" "$app/Contents/Frameworks"
# Signing applies only to this throwaway bundle; the person's identities stay untouched.
codesign --force --deep --sign - "$app" >/dev/null
codesign --verify --deep --strict "$app"

python3 "$fixture" --port-file "$port_file" --request-log "$request_log" >"$fixture_log" 2>&1 &
fixture_pid=$!
for _ in {1..50}; do
  [[ -s "$port_file" ]] && break
  kill -0 "$fixture_pid" 2>/dev/null || { cat "$fixture_log" >&2; exit 1; }
  sleep 0.1
done
[[ -s "$port_file" ]] || { echo "loopback fixture did not publish a port" >&2; exit 1; }
port="$(cat "$port_file")"
[[ "$port" =~ ^[0-9]+$ ]] || { echo "fixture returned an invalid port" >&2; exit 1; }
url="http://127.0.0.1:$port/"

profile_one="11111111111111111111111111111111"
profile_two="22222222222222222222222222222222"
printf '[{"id":"%s","name":"Smoke imported"},{"id":"%s","name":"Smoke separate"}]\n' "$profile_one" "$profile_two" > "$state_root/browser-profiles.json"
printf '%s\n' "$profile_one" > "$state_root/browser-default-profile"
printf '[{"url":"%s","name":"synthetic","value":"first-profile","domain":"127.0.0.1","path":"/","secure":false,"http_only":false,"expires_unix":2000000000},{"url":"%s","name":"synthetic-session","value":"first-session","domain":"127.0.0.1","path":"/","secure":false,"http_only":true}]\n' "$url" "$url" > "$state_root/browser-cookie-staging-$profile_one.json"

browser_cli="${ZEROCODE_BROWSER_BIN:-$(command -v zerocode-browser || true)}"
[[ -n "$browser_cli" && -x "$browser_cli" ]] || { echo "zerocode-browser shim is unavailable; run this from a ZeroCode terminal or set ZEROCODE_BROWSER_BIN" >&2; exit 2; }

launch_app() {
  printf '\n--- isolated app launch ---\n' >> "$app_log"
  (
    cd "$sandbox"
    HOME="$home" CFFIXED_USER_HOME="$home" TMPDIR="$tmp" \
      XDG_CONFIG_HOME="$home/.config" XDG_DATA_HOME="$home/.local/share" XDG_CACHE_HOME="$home/.cache" \
      ZEROCODE_BYPASS_SINGLE_INSTANCE_LOCK=1 \
      exec "$app/Contents/MacOS/zerocode-shell"
  ) >>"$app_log" 2>&1 &
  app_pid=$!
}

endpoint="$state_root/agent-hooks/endpoint.env"
browser_token="$state_root/agent-hooks/browser-token"
wait_for_bridge() {
  for _ in {1..150}; do
    if [[ -r "$endpoint" && -r "$browser_token" ]]; then
      if browser tabs >/dev/null 2>&1; then
        return 0
      fi
    fi
    kill -0 "$app_pid" 2>/dev/null || { echo "native app exited before its isolated browser bridge started" >&2; sed -n '1,160p' "$app_log" >&2; return 1; }
    sleep 0.1
  done
  echo "isolated browser bridge did not become ready" >&2
  sed -n '1,160p' "$app_log" >&2
  return 1
}

browser() {
  ZEROCODE_HOOK_ENDPOINT="$endpoint" ZEROCODE_BROWSER_TOKEN_FILE="$browser_token" \
    ZEROCODE_PANE_KEY= ZEROCODE_HOOK_PORT= ZEROCODE_HOOK_TOKEN= ZEROCODE_BROWSER_TOKEN= \
    ZEROCODE_RUN_EVIDENCE_DIR= "$browser_cli" "$@"
}

open_fixture() {
  local answer label rows
  for _ in {1..3}; do
    answer="$(browser open "$url")"
    label="$(printf '%s\n' "$answer" | grep -Eo 'browser-[0-9]+' | head -n1 || true)"
    [[ -n "$label" ]] || continue
    for _ in {1..30}; do
      rows="$(browser tabs)"
      if printf '%s\n' "$rows" | awk -F '\t' -v label="$label" '$1 == label { found=1 } END { exit !found }'; then
        printf '%s\n' "$label"
        return 0
      fi
      sleep 0.1
    done
  done
  echo "browser frontend did not acknowledge a native pane: $answer" >&2
  tail -60 "$state_root/window-errors.log" "$app_log" >&2 || true
  return 1
}

close_fixture() {
  local label="$1" rows
  browser close "$label" >/dev/null
  for _ in {1..50}; do
    kill -0 "$app_pid" 2>/dev/null || { echo "closing a browser pane closed the IDE" >&2; return 1; }
    rows="$(browser tabs)"
    if ! printf '%s\n' "$rows" | awk -F '\t' -v label="$label" '$1 == label { found=1 } END { exit !found }'; then
      return 0
    fi
    sleep 0.1
  done
  echo "native browser pane did not close" >&2
  return 1
}

wait_for_root_request() {
  local index="$1" expected="$2"
  for _ in {1..150}; do
    if python3 - "$request_log" "$index" "$expected" <<'PY'
import json
import sys

path, index, expected = sys.argv[1], int(sys.argv[2]), sys.argv[3]
try:
    rows = [json.loads(line) for line in open(path, encoding="utf-8") if line.strip()]
except FileNotFoundError:
    raise SystemExit(1)
roots = [row for row in rows if row.get("path") == "/"]
if len(roots) <= index:
    raise SystemExit(1)
cookie = roots[index].get("cookie", "")
persistent = "synthetic=first-profile" in cookie
session = "synthetic-session=first-session" in cookie
raise SystemExit(0 if persistent == session == (expected == "present") else 1)
PY
    then
      return
    fi
    sleep 0.1
  done
  echo "fixture request $index did not match expected isolated profile state ($expected)" >&2
  browser tabs >&2 || true
  sed -n '1,120p' "$app_log" >&2
  cat "$request_log" >&2 || true
  return 1
}

assert_page_proof() {
  local label="$1" expected="$2" proof
  for _ in {1..100}; do
    proof="$(browser eval "$label" "document.querySelector('#proof')?.textContent.trim()" 2>&1 || true)"
    [[ "$proof" == *"$expected"* ]] && return
    sleep 0.1
  done
  echo "embedded CEF eval did not report $expected" >&2
  printf '%s\n' "$proof" >&2
  return 1
}

# Profile one starts with the app-owned staged cookie. Its first loopback document request must carry it.
launch_app
wait_for_bridge
label_one="$(open_fixture)"
wait_for_root_request 0 present
assert_page_proof "$label_one" "fixture: imported"
browser screenshot "$label_one" --out "$out" --json >/dev/null
[[ -s "$out" ]] || { echo "embedded CEF did not create the requested PNG" >&2; exit 1; }
close_fixture "$label_one"
stop_app
[[ ! -e "$state_root/browser-cookie-staging-$profile_one.json" ]] || { echo "staged cookie was not consumed after successful CEF injection" >&2; exit 1; }

# The second profile has no staged or persisted synthetic value, so it must not inherit profile one's jar.
printf '%s\n' "$profile_two" > "$state_root/browser-default-profile"
launch_app
wait_for_bridge
label_two="$(open_fixture)"
wait_for_root_request 1 absent
assert_page_proof "$label_two" "fixture: separate"
close_fixture "$label_two"
stop_app

# Returning to the first profile proves its CEF cache persisted independently after the staged file was removed.
printf '%s\n' "$profile_one" > "$state_root/browser-default-profile"
launch_app
wait_for_bridge
label_three="$(open_fixture)"
wait_for_root_request 2 present
assert_page_proof "$label_three" "fixture: imported"
close_fixture "$label_three"
stop_app

finished=true
echo "passed: native CEF child pane, first-request import, isolated profiles, persistent/session cookies after restart, eval, and PNG: $out"
