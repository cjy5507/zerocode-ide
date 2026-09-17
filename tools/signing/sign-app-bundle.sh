#!/bin/bash
# Sign a whole ZeroCode.app inside-out with this machine's stable local identity
# so every bundle macOS attributes a TCC grant to — above all dev.zerocode.app,
# the ScreenCapture *responsible* process — keeps one code requirement across
# rebuilds. An adhoc app changes its cdhash every build, so its Screen Recording
# grant (the "ZeroCode" row) breaks on each install; a leaf-pinned requirement
# survives. Falls back to adhoc, aloud, when no local identity can be made.
set -euo pipefail
SIGNING_DIR=$(cd "$(dirname "$0")" && pwd)
app=${1:?usage: sign-app-bundle.sh ZeroCode.app}

fingerprint=$(bash "$SIGNING_DIR/ensure-local-identity.sh" || true)
[ -n "$fingerprint" ] ||
  echo 'ZeroCode identity=adhoc: local signing unavailable; a rebuilt app needs ScreenCapture re-grant' >&2

# Never `--options runtime` here. Hardened runtime turns on library validation,
# which admits only libraries signed by Apple or by the process's own Team ID —
# and a self-signed identity (adhoc too) has no Team ID, so the shell rejected
# its own Chromium Embedded Framework at dlopen ("mapping process and mapped
# file (non-platform) have different Team IDs") and exited before the window
# (1.3.11, 09-09). Hardened runtime belongs to Developer ID signing, which has
# a Team ID and is outside this local flow (README). The entitlements still
# ride along: the same plist a Developer ID re-sign would carry.
sign_bundle() { # APP-OR-EXE  [BUNDLE-ID]  [ENTITLEMENTS]
  local path=$1 id=${2:-} entitlements=${3:-}
  local extra=(--force)
  [ -z "$entitlements" ] || extra+=(--entitlements "$entitlements")
  if [ -z "$fingerprint" ]; then
    codesign "${extra[@]}" --sign - "$path"
  elif [ -n "$id" ]; then
    codesign "${extra[@]}" --sign "$fingerprint" --timestamp=none \
      --requirements "=designated => identifier \"$id\" and certificate leaf = H\"$fingerprint\"" "$path"
  else
    codesign "${extra[@]}" --sign "$fingerprint" --timestamp=none "$path"
  fi
}
bundle_id() { /usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$1/Contents/Info.plist"; }

# Inside-out: nested code first, then the sealing outer app.
cef_entitlements=
framework="$app/Contents/Frameworks/Chromium Embedded Framework.framework"
if [ -d "$framework" ]; then
  cef_entitlements="$SIGNING_DIR/../../crates/zerocode-shell/chromium.entitlements.plist"
  # CEF's Libraries directory is not discovered by codesign --deep.
  while IFS= read -r -d '' binary; do
    case "$(file -b "$binary")" in
      *Mach-O*) sign_bundle "$binary" ;;
    esac
  done < <(find "$framework" -type f -print0)
  sign_bundle "$framework"
  for name in 'ZeroCode Helper' 'ZeroCode Helper (GPU)' 'ZeroCode Helper (Renderer)' 'ZeroCode Helper (Plugin)' 'ZeroCode Helper (Alerts)'; do
    helper="$app/Contents/Frameworks/$name.app"
    [ -d "$helper" ] || { echo "missing Chromium helper: $helper" >&2; exit 1; }
    sign_bundle "$helper" "$(bundle_id "$helper")" "$cef_entitlements"
  done
fi
helper="$app/Contents/Resources/ZeroCode Computer Use.app"
[ -d "$helper" ] && sign_bundle "$helper" "$(bundle_id "$helper")"
for bin in zerocode-mirror zerocode-pick zerocode-cef-helper; do
  [ -f "$app/Contents/MacOS/$bin" ] && sign_bundle "$app/Contents/MacOS/$bin"
done
# zo rides as a bundle resource (tauri.release.conf.json bin/zo), where no
# nested-code walk looks for code: it left the linker adhoc, the one Mach-O a
# lane-built 1.3.48 carried unsigned by this identity.
zo="$app/Contents/Resources/bin/zo"
[ -f "$zo" ] && sign_bundle "$zo"
sign_bundle "$app" "$(bundle_id "$app")" "$cef_entitlements"
codesign --verify --strict "$app"
# The package smoke's nested-code gate: every Mach-O signed like the app, and
# every nested executable started once from the signed bytes.
bash "$SIGNING_DIR/../../scripts/native-package-smoke.macos.sh" --nested "$app"
