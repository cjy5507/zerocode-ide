#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "native-package-smoke.macos.sh requires macOS" >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
contract_field() {
  node "$root/scripts/package-contract.mjs" macos --field "$1"
}

stage=""
previous_archive=""
previous_revision=""
expected_apple_team_id=""
nested_app=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --nested)
      [[ $# -ge 2 && -d "$2" && -z "$nested_app" ]] || { echo "invalid --nested" >&2; exit 2; }
      nested_app="$2"
      shift 2
      ;;
    --stage)
      [[ $# -ge 2 && -n "$2" && -z "$stage" ]] || { echo "invalid --stage" >&2; exit 2; }
      stage="$2"
      shift 2
      ;;
    --previous-archive)
      [[ $# -ge 2 && -f "$2" && -z "$previous_archive" ]] || { echo "invalid --previous-archive" >&2; exit 2; }
      previous_archive="$2"
      shift 2
      ;;
    --previous-revision)
      [[ $# -ge 2 && "$2" =~ ^[0-9a-fA-F]{40}$ && -z "$previous_revision" ]] || { echo "invalid --previous-revision" >&2; exit 2; }
      previous_revision="$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]')"
      shift 2
      ;;
    --expected-apple-team-id)
      [[ $# -ge 2 && "$2" =~ ^[A-Z0-9]+$ && -z "$expected_apple_team_id" ]] || { echo "invalid --expected-apple-team-id" >&2; exit 2; }
      expected_apple_team_id="$2"
      shift 2
      ;;
    *)
      echo "usage: scripts/native-package-smoke.macos.sh [--stage directory] [--expected-apple-team-id id] [--previous-archive artifact.zip --previous-revision sha] | --nested ZeroCode.app" >&2
      exit 2
      ;;
  esac
done
if [[ -n "$previous_archive" || -n "$previous_revision" ]]; then
  [[ -n "$previous_archive" && -n "$previous_revision" ]] || { echo "previous archive and revision must be provided together" >&2; exit 2; }
fi
if [[ -n "$nested_app" && -n "$stage$previous_archive$expected_apple_team_id" ]]; then
  echo "--nested stands alone" >&2
  exit 2
fi

plist_value() {
  /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist"
}

signature_details() {
  codesign --display --verbose=4 "$1" 2>&1
}

# The nested-code gate (t-3716). A valid seal is not a runnable nested binary,
# and codesign --verify, even --deep, reads Contents/Resources as sealed data:
# a lane-built 1.3.48 passed it with an adhoc Resources/bin/zo, where
# notarization wants every nested Mach-O under the app's Developer ID with a
# secure timestamp and hardened runtime. So every Mach-O in the bundle is held
# to the app's own signer, and every nested executable is started once from
# the bytes that ship, with the argument that makes it answer and leave —
# before any panel, socket, or permission check. A nested executable with no
# row here fails the gate, so a new helper arrives with its probe.
nested_probe() { # PATH-UNDER-CONTENTS VERSION -> "exit|line it says|argument"
  case "$1" in
    Resources/bin/zo) echo "0|zo $2|--version" ;;
    MacOS/zerocode-pick) echo "2|usage: zerocode-pick|" ;;
    MacOS/zerocode-mirror) echo "127|zerocode-mirror: real binary path missing|" ;;
    "Resources/ZeroCode Computer Use.app/Contents/MacOS/zerocode-computer-use-macos")
      echo "2|usage: zerocode-computer-use-macos --agent|--agent" ;;
    # A Chromium subprocess host loads the bundled framework (a failed load
    # panics, 101) and, given no --type, execute_process answers -1.
    MacOS/zerocode-cef-helper | Frameworks/*.app/Contents/MacOS/*) echo "255||" ;;
    *) return 1 ;;
  esac
}

details_signer() { # SIGNATURE-DETAILS -> the leaf Authority= line, Signature=adhoc, or nothing
  local line
  while IFS= read -r line; do
    case "$line" in Authority=* | Signature=*) printf '%s\n' "$line"; return ;; esac
  done <<<"$1"
}

details_runtime() { # SIGNATURE-DETAILS -> runtime | none
  local line
  while IFS= read -r line; do
    case "$line" in CodeDirectory*flags=*runtime*) echo runtime; return ;; esac
  done <<<"$1"
  echo none
}

run_nested_probe() { # WORKDIR EXECUTABLE EXIT LINE [ARGUMENT]
  local work="$1" executable="$2" want="$3" line="$4" said="$1/said" child rc=0 polls=0
  local argv=("$2")
  [[ -z "${5:-}" ]] || argv+=("$5")
  env -i PATH=/usr/bin:/bin HOME="$work/home" TMPDIR="$work/tmp" "${argv[@]}" </dev/null >"$said" 2>&1 &
  child=$!
  # 10 ms polls: most probes answer in one or two.
  while kill -0 "$child" 2>/dev/null && ((polls < 2000)); do
    sleep 0.01
    polls=$((polls + 1))
  done
  if kill -0 "$child" 2>/dev/null; then
    kill -KILL "$child" 2>/dev/null || true
    wait "$child" 2>/dev/null || true
    echo "nested executable did not answer within 20 s: $executable" >&2
    return 1
  fi
  wait "$child" || rc=$?
  if [[ "$rc" != "$want" ]] || { [[ -n "$line" ]] && ! grep -qF -- "$line" "$said"; }; then
    echo "nested executable did not answer (exit $rc; want $want${line:+ and \"$line\"}): $executable" >&2
    sed -n '1,20p' "$said" >&2
    return 1
  fi
}

assert_nested_code() { # APP WORKDIR SIGNED(yes|no)
  local app="$1" work="$2" signed="$3" version main details signer="" runtime="" file kind relative probe want line argument
  local started=0
  version="$(plist_value "$app" CFBundleShortVersionString)"
  main="$(plist_value "$app" CFBundleExecutable)"
  mkdir -p "$work/home" "$work/tmp"
  if [[ "$signed" == yes ]]; then
    details="$(signature_details "$app")"
    signer="$(details_signer "$details")"
    runtime="$(details_runtime "$details")"
    [[ -n "$signer" ]] || { echo "no signer on $app" >&2; return 1; }
  fi
  while IFS= read -r -d '' file; do
    kind="$(file -b "$file")"
    [[ "$kind" == *Mach-O* ]] || continue
    relative="${file#"$app/Contents/"}"
    if [[ "$signed" == yes ]]; then
      details="$(signature_details "$file" || true)"
      if [[ "$(details_signer "$details")" != "$signer" ]]; then
        echo "nested code is not signed like the app ($signer): $relative says $(details_signer "$details")" >&2
        return 1
      fi
    fi
    [[ "$kind" == *executable* && "$relative" != "MacOS/$main" ]] || continue
    if [[ "$signed" == yes && "$(details_runtime "$details")" != "$runtime" ]]; then
      echo "nested executable's hardened runtime differs from the app's ($runtime): $relative" >&2
      return 1
    fi
    probe="$(nested_probe "$relative" "$version")" || { echo "nested executable has no launch probe: $relative (add its row to nested_probe)" >&2; return 1; }
    IFS='|' read -r want line argument <<<"$probe"
    run_nested_probe "$work" "$file" "$want" "$line" "$argument" || return 1
    started=$((started + 1))
  done < <(find "$app/Contents" -type f -print0)
  if [[ "$signed" == yes ]]; then
    echo "checked nested code in $app: every Mach-O signed as $signer, $started nested executables started"
  else
    echo "checked nested code in $app: $started nested executables started (unsigned: no signer claimed)"
  fi
}

if [[ -n "$nested_app" ]]; then
  # tools/signing/sign-app-bundle.sh runs this on the bundle it just signed.
  nested_work="$(mktemp -d "${TMPDIR:-/tmp}/zerocode-nested-code.XXXXXX")"
  trap 'rm -rf -- "$nested_work"' EXIT
  assert_nested_code "$nested_app" "$nested_work" yes
  exit 0
fi

product_name="$(contract_field productName)"
identifier="$(contract_field identifier)"
version="$(contract_field version)"
binary_name="$(contract_field binaryName)"
dmg="$(contract_field artifacts.dmg)"
built_app="$(contract_field artifacts.app)"
artifact="$(contract_field migration.artifact)"
legacy_directory="$(contract_field migration.legacyDirectory)"
migration_class="$(contract_field migration.class)"
before_base64="$(contract_field migration.beforeBase64)"
after_base64="$(contract_field migration.afterBase64)"
if [[ "$migration_class" != "local-data" ]]; then
  echo "unsupported migration fixture class: $migration_class" >&2
  exit 1
fi

sandbox="$(mktemp -d "${TMPDIR:-/tmp}/zerocode-native-package-smoke.XXXXXX")"
mount="$sandbox/mount"
isolated_home="$sandbox/home"
install_root="$sandbox/Applications"
installed_app="$install_root/$product_name.app"
log="$sandbox/app.log"
pid=""
mounted=false

terminate_app() {
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    for _ in {1..20}; do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.25
    done
    if kill -0 "$pid" 2>/dev/null; then
      kill -KILL "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
  fi
  pid=""
}

cleanup() {
  terminate_app
  if [[ "$mounted" == true ]]; then
    hdiutil detach "$mount" -quiet || hdiutil detach "$mount" -force -quiet || true
  fi
  if [[ -d "$sandbox" && "$(basename "$sandbox")" == zerocode-native-package-smoke.* ]]; then
    rm -rf -- "$sandbox"
  else
    echo "refusing to remove unexpected smoke path: $sandbox" >&2
  fi
}
trap cleanup EXIT INT TERM

assert_app_identity() {
  local app="$1"
  local expected_version="$2"
  local info="$app/Contents/Info.plist"
  plutil -lint "$info" >/dev/null
  [[ "$(plist_value "$app" CFBundleIdentifier)" == "$identifier" ]] || { echo "bundle identifier differs in $app" >&2; return 1; }
  [[ "$(plist_value "$app" CFBundleName)" == "$product_name" ]] || { echo "bundle name differs in $app" >&2; return 1; }
  [[ "$(plist_value "$app" CFBundleDisplayName)" == "$product_name" ]] || { echo "bundle display name differs in $app" >&2; return 1; }
  [[ "$(plist_value "$app" CFBundleExecutable)" == "$binary_name" ]] || { echo "bundle executable differs in $app" >&2; return 1; }
  [[ "$(plist_value "$app" CFBundleShortVersionString)" == "$expected_version" ]] || { echo "short version differs in $app" >&2; return 1; }
  [[ "$(plist_value "$app" CFBundleVersion)" == "$expected_version" ]] || { echo "bundle version differs in $app" >&2; return 1; }
}

assert_developer_id_signature() {
  local artifact="$1"
  local details="$2"
  printf '%s\n' "$details" | grep -q '^Authority=Developer ID Application:' || { echo "Developer ID authority differs in $artifact" >&2; return 1; }
  printf '%s\n' "$details" | grep -q "^TeamIdentifier=$expected_apple_team_id$" || { echo "Apple Team ID differs in $artifact" >&2; return 1; }
  printf '%s\n' "$details" | grep -q '^Timestamp=' || { echo "secure signing timestamp is missing in $artifact" >&2; return 1; }
}

assert_app_trust() {
  local app="$1"
  local details
  codesign --verify --deep --strict --verbose=2 "$app"
  details="$(signature_details "$app")"
  assert_developer_id_signature "$app" "$details"
  printf '%s\n' "$details" | grep -q '^CodeDirectory .*flags=.*(runtime)' || { echo "Hardened Runtime is missing in $app" >&2; return 1; }
  spctl --assess --type execute --verbose=4 "$app"
  xcrun stapler validate "$app"
}

assert_dmg_trust() {
  local source_dmg="$1"
  local details
  codesign --verify --strict --verbose=2 "$source_dmg"
  details="$(signature_details "$source_dmg")"
  assert_developer_id_signature "$source_dmg" "$details"
  spctl --assess --type open --context context:primary-signature --verbose=4 "$source_dmg"
  xcrun stapler validate "$source_dmg"
}

remove_installed_app() {
  if [[ -e "$installed_app" ]]; then
    [[ "$installed_app" == "$sandbox"/* ]] || { echo "refusing to replace unexpected app path: $installed_app" >&2; return 1; }
    rm -rf -- "$installed_app"
  fi
}

install_from_dmg() {
  local source_dmg="$1"
  local expected_version="$2"
  hdiutil verify "$source_dmg" >/dev/null
  if [[ -n "$expected_apple_team_id" ]]; then
    assert_dmg_trust "$source_dmg"
  fi
  hdiutil attach -readonly -nobrowse -mountpoint "$mount" "$source_dmg" >/dev/null
  mounted=true

  shopt -s nullglob
  local mounted_apps=("$mount"/*.app)
  if [[ ${#mounted_apps[@]} -ne 1 ]]; then
    echo "expected exactly one app in $source_dmg, found ${#mounted_apps[@]}" >&2
    return 1
  fi
  assert_app_identity "${mounted_apps[0]}" "$expected_version"
  local mounted_binary="${mounted_apps[0]}/Contents/MacOS/$binary_name"
  node "$root/scripts/check-package-artifacts.mjs" --binary "$mounted_binary"
  if [[ "$source_dmg" == "$dmg" ]] && ! cmp -s "$built_app/Contents/MacOS/$binary_name" "$mounted_binary"; then
    echo "current DMG app binary differs from the inspected release app" >&2
    return 1
  fi
  if [[ -n "$expected_apple_team_id" ]]; then
    # This is the app from the distributed DMG, not a parallel build output.
    assert_app_trust "${mounted_apps[0]}"
  fi
  remove_installed_app
  ditto --rsrc --extattr "${mounted_apps[0]}" "$installed_app"
  hdiutil detach "$mount" -quiet
  mounted=false
  assert_app_identity "$installed_app" "$expected_version"

  installed_binary="$installed_app/Contents/MacOS/$binary_name"
  if [[ ! -x "$installed_binary" ]]; then
    echo "installed app binary is not executable: $installed_binary" >&2
    return 1
  fi
  if [[ "$source_dmg" == "$dmg" ]]; then
    # The distributed bytes, installed: signed like the app when it claims a
    # Developer ID, and every nested executable starts.
    assert_nested_code "$installed_app" "$sandbox/nested" "$([[ -n "$expected_apple_team_id" ]] && echo yes || echo no)"
  fi
}

json_field() {
  node -e 'const value = process.argv[2].split(".").reduce((held, key) => held?.[key], JSON.parse(process.argv[1])); if (typeof value !== "string") process.exit(2); process.stdout.write(value)' "$1" "$2"
}

mkdir -p "$mount" "$isolated_home" "$install_root" "$sandbox/tmp" "$isolated_home/$legacy_directory"
assert_app_identity "$built_app" "$version"
if [[ -n "$expected_apple_team_id" ]]; then
  # The separately uploaded app archive is created from this exact bundle.
  assert_app_trust "$built_app"
fi

previous_dmg=""
previous_version=""
if [[ -n "$previous_archive" ]]; then
  previous_stage="$sandbox/previous-stage"
  mkdir "$previous_stage"
  entries="$sandbox/previous-archive-entries.txt"
  unzip -Z1 "$previous_archive" > "$entries"
  while IFS= read -r entry; do
    case "$entry" in
      ""|.|..|*/*|*\\*) echo "unsafe path in previous package archive" >&2; exit 1 ;;
    esac
  done < "$entries"
  ditto -x -k "$previous_archive" "$previous_stage"
  if find "$previous_stage" -type l -print -quit | grep -q .; then
    echo "previous package archive contains a symlink" >&2
    exit 1
  fi
  previous_manifest="$(node "$root/scripts/package-stage-manifest.mjs" verify macos "$previous_stage" --previous --revision "$previous_revision")"
  previous_version="$(json_field "$previous_manifest" version)"
  previous_dmg="$previous_stage/$(json_field "$previous_manifest" artifacts.dmg.file)"
  install_from_dmg "$previous_dmg" "$previous_version"
else
  install_from_dmg "$dmg" "$version"
fi

legacy="$isolated_home/$legacy_directory/$artifact"
target="$isolated_home/Library/Application Support/$identifier/$artifact"
before="$sandbox/before.json"
after="$sandbox/after.json"
node -e 'require("node:fs").writeFileSync(process.argv[1], Buffer.from(process.argv[2], "base64"))' "$before" "$before_base64"
node -e 'require("node:fs").writeFileSync(process.argv[1], Buffer.from(process.argv[2], "base64"))' "$after" "$after_base64"
cp "$before" "$legacy"

launch_app() {
  : > "$log"
  HOME="$isolated_home" CFFIXED_USER_HOME="$isolated_home" TMPDIR="$sandbox/tmp" \
    ZEROCODE_BYPASS_SINGLE_INSTANCE_LOCK=1 \
    "$installed_binary" >"$log" 2>&1 &
  pid=$!
}

wait_for_expected_state() {
  for _ in {1..80}; do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "installed app exited before exposing expected state" >&2
      sed -n '1,120p' "$log" >&2
      return 1
    fi
    if [[ -f "$target" ]] && cmp -s "$before" "$target"; then
      sleep 1
      kill -0 "$pid" 2>/dev/null
      return
    fi
    sleep 0.25
  done
  echo "installed app did not retain expected $artifact in isolated AppLocalData" >&2
  sed -n '1,120p' "$log" >&2
  return 1
}

launch_app
wait_for_expected_state
terminate_app

# Once the previous/fresh installation has activated platform state, a valid
# but different legacy value must never regain authority during replacement.
cp "$after" "$legacy"
if [[ -n "$previous_dmg" ]]; then
  install_from_dmg "$dmg" "$version"
fi
launch_app
wait_for_expected_state
terminate_app
launch_app
wait_for_expected_state
terminate_app

if [[ -n "$stage" ]]; then
  if [[ -e "$stage" ]]; then
    echo "artifact staging path already exists: $stage" >&2
    exit 1
  fi
  mkdir -p "$stage"
  ditto -c -k --sequesterRsrc --keepParent "$built_app" "$stage/$product_name.app.zip"
  cp "$dmg" "$stage/$(basename "$dmg")"
  node "$root/scripts/package-stage-manifest.mjs" write macos "$stage"
fi

if [[ -n "$previous_dmg" ]]; then
  echo "checked macOS $previous_version -> $version DMG replacement, launch, terminate, relaunch, identity, and isolated state retention"
else
  echo "checked macOS DMG install, launch, terminate, relaunch, identity, and isolated legacy migration"
fi
