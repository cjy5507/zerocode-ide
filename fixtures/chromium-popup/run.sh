#!/usr/bin/env bash
# Real CEF with no window on screen: fixtures/chromium-popup/run.sh <cancel|real|settled> <evidence-dir>
# `cancel` is what the window did before t-90671 and is expected to fail. `real` is
# a person's popup. `settled` is an agent's: the first request waits for a verdict.
# It builds nothing of the product and touches nothing of the person's: a
# temporary HOME, a loopback server, and a throwaway bundle that copies the
# installed window's engine files.
set -euo pipefail
mode="${1:?cancel, real or settled}"
out="${2:?an evidence directory}"
root="$(cd "$(dirname "$0")" && pwd)"
engine="${ZEROCODE_CEF_FRAMEWORKS:-/Applications/ZeroCode.app/Contents/Frameworks}"
binary="${ZEROCODE_POPUP_PROBE_BIN:-$root/probe/target/debug/chromium-popup-probe}"
[[ -x "$binary" ]] || { echo "build the probe first: cargo build in $root/probe" >&2; exit 2; }
[[ -d "$engine" ]] || { echo "no engine at $engine" >&2; exit 2; }
mkdir -p "$out"
sandbox="$(mktemp -d "${TMPDIR:-/tmp}/zerocode-popup-probe.XXXXXX")"
server_pid=""
probe_pid=""
stop() { [[ -z "$1" ]] || { kill -TERM "$1" 2>/dev/null || true; wait "$1" 2>/dev/null || true; }; }
finish() { stop "$probe_pid"; stop "$server_pid"; rm -rf -- "$sandbox"; }
trap finish EXIT INT TERM

app="$sandbox/Popup Probe.app"
mkdir -p "$app/Contents/MacOS" "$sandbox/home" "$sandbox/root"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>probe</string>
  <key>CFBundleIdentifier</key><string>dev.zerocode.popup-probe</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSUIElement</key><true/>
</dict></plist>
PLIST
ditto "$binary" "$app/Contents/MacOS/probe"
ditto "$engine" "$app/Contents/Frameworks"
codesign --force --deep --sign - "$app" >/dev/null

log="$out/$mode-pages.jsonl"
events="$out/$mode-events.jsonl"
rm -f "$log" "$events"
python3 "$root/server.py" --port-file "$sandbox/port" --log "$log" &
server_pid=$!
for _ in {1..50}; do [[ -s "$sandbox/port" ]] && break; sleep 0.1; done
origin="http://127.0.0.1:$(cat "$sandbox/port")"

HOME="$sandbox/home" CFFIXED_USER_HOME="$sandbox/home" TMPDIR="$sandbox" \
  "$app/Contents/MacOS/probe" --mode "$mode" --origin "$origin" --events "$events" \
  --root "$sandbox/root" >"$out/$mode-probe.log" 2>&1 &
probe_pid=$!
limit=90
while kill -0 "$probe_pid" 2>/dev/null && ((limit > 0)); do sleep 1; limit=$((limit - 1)); done
if ((limit == 0)); then echo "the probe did not finish in 90 s" >&2; fi
stop "$probe_pid"; probe_pid=""
python3 "$root/verdict.py" "$log" "$events" "$mode"
