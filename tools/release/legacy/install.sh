#!/usr/bin/env bash
set -euo pipefail

OFFICIAL_REPO="cjy5507/zerocode"
RELEASE_DOWNLOAD_BASE="https://github.com/${OFFICIAL_REPO}/releases/download"
LATEST_MANIFEST_URL="https://github.com/${OFFICIAL_REPO}/releases/latest/download/manifest.txt"
MAX_ASSET_SIZE=104857600
SUPPORTED_TARGETS=(
  "aarch64-apple-darwin"
  "x86_64-apple-darwin"
  "x86_64-unknown-linux-gnu"
)

fail() {
  printf 'install.sh: %s\n' "$*" >&2
  exit 1
}

if [[ -z "${HOME:-}" ]]; then
  fail "HOME must be set"
fi

case "$(uname -s):$(uname -m)" in
  Darwin:arm64 | Darwin:aarch64) target="aarch64-apple-darwin" ;;
  Darwin:x86_64) target="x86_64-apple-darwin" ;;
  Linux:x86_64 | Linux:amd64) target="x86_64-unknown-linux-gnu" ;;
  *) fail "unsupported platform: $(uname -s) $(uname -m)" ;;
esac

manifest_url="$LATEST_MANIFEST_URL"
test_base=""
curl_protocol="=https"
if [[ -n "${ZO_INSTALLER_TEST_BASE:-}" ]]; then
  if [[ "${ZO_INSTALLER_TEST_ONLY:-}" != "1" ]]; then
    fail "ZO_INSTALLER_TEST_BASE requires ZO_INSTALLER_TEST_ONLY=1"
  fi
  test_base="${ZO_INSTALLER_TEST_BASE%/}"
  case "$test_base" in
    http://127.0.0.1:* | http://localhost:*) ;;
    *) fail "test base must use HTTP on localhost or 127.0.0.1" ;;
  esac
  manifest_url="${test_base}/manifest.txt"
  curl_protocol="=http"
fi

install_dir="${HOME}/.local/bin"
destination="${install_dir}/zo"
mkdir -p "$install_dir"
if [[ -L "$destination" ]]; then
  fail "refusing symlink destination: $destination"
fi
if [[ -e "$destination" && ! -f "$destination" ]]; then
  fail "destination is not a regular file: $destination"
fi

manifest_file="$(mktemp "${install_dir}/.zo-manifest.XXXXXX")"
binary_file=""
version_file=""
marker_file=""
cleanup() {
  [[ -z "$manifest_file" ]] || rm -f "$manifest_file"
  [[ -z "$binary_file" ]] || rm -f "$binary_file"
  [[ -z "$version_file" ]] || rm -f "$version_file"
  [[ -z "$marker_file" ]] || rm -f "$marker_file"
}
trap cleanup EXIT INT TERM

curl --fail --silent --show-error --location \
  --proto "$curl_protocol" --tlsv1.2 \
  "$manifest_url" --output "$manifest_file"

[[ "$(tail -c 1 "$manifest_file" | wc -l | tr -d ' ')" == "1" ]] || \
  fail "manifest must end with a newline"
if LC_ALL=C grep -q $'\r' "$manifest_file"; then
  fail "manifest must use Unix newlines"
fi
line_count="$(wc -l < "$manifest_file" | tr -d ' ')"
[[ "$line_count" -eq 6 ]] || \
  fail "manifest must contain schema, version, base, and three asset rows"
schema_line="$(sed -n '1p' "$manifest_file")"
version_line="$(sed -n '2p' "$manifest_file")"
base_line="$(sed -n '3p' "$manifest_file")"
[[ "$schema_line" == "schema=1" ]] || fail "unsupported manifest schema"
[[ "$version_line" == version=* ]] || fail "manifest version line is missing"
version="${version_line#version=}"
[[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || \
  fail "invalid semantic version: $version"
[[ "$base_line" == base=* ]] || fail "manifest base line is missing"
download_base="${base_line#base=}"
expected_base="${RELEASE_DOWNLOAD_BASE}/v${version}"
if [[ -n "$test_base" ]]; then
  [[ "$download_base" == "$test_base" ]] || fail "invalid test release download base"
else
  [[ "$download_base" == "$expected_base" ]] || fail "invalid release download base"
fi

selected_name=""
selected_hash=""
selected_size=""
for index in "${!SUPPORTED_TARGETS[@]}"; do
  line="$(sed -n "$((index + 4))p" "$manifest_file")"
  [[ "$line" == asset=* ]] || fail "invalid asset row"
  IFS='|' read -r row_target row_name row_hash row_size extra <<< "${line#asset=}"
  [[ -z "${extra:-}" && -n "$row_size" ]] || fail "invalid asset row"
  expected_target="${SUPPORTED_TARGETS[index]}"
  [[ "$row_target" == "$expected_target" ]] || \
    fail "expected asset target $expected_target, found $row_target"
  expected_name="zo-v${version}-${row_target}"
  [[ "$row_name" == "$expected_name" ]] || fail "invalid asset name for $row_target"
  [[ "$row_hash" =~ ^[0-9a-f]{64}$ ]] || fail "invalid SHA-256 for $row_target"
  [[ "$row_size" =~ ^[0-9]+$ ]] || fail "invalid asset size for $row_target"
  (( row_size > 0 && row_size <= MAX_ASSET_SIZE )) || \
    fail "asset size out of range for $row_target"
  if [[ "$row_target" == "$target" ]]; then
    selected_name="$row_name"
    selected_hash="$row_hash"
    selected_size="$row_size"
  fi
done
[[ -n "$selected_name" ]] || fail "no release asset for target $target"

binary_file="$(mktemp "${install_dir}/.zo-install.XXXXXX")"
curl --fail --silent --show-error --location \
  --proto "$curl_protocol" --tlsv1.2 \
  "${download_base}/${selected_name}" --output "$binary_file"
actual_size="$(wc -c < "$binary_file" | tr -d ' ')"
[[ "$actual_size" == "$selected_size" ]] || fail "release asset size mismatch"
if command -v sha256sum >/dev/null 2>&1; then
  actual_hash="$(sha256sum "$binary_file" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual_hash="$(shasum -a 256 "$binary_file" | awk '{print $1}')"
else
  fail "sha256sum or shasum is required"
fi
[[ "$actual_hash" == "$selected_hash" ]] || fail "release asset SHA-256 mismatch"
chmod 0755 "$binary_file"

version_file="$(mktemp "${install_dir}/.zo-version.XXXXXX")"
"$binary_file" --version > "$version_file" 2>/dev/null &
version_pid=$!
version_complete=0
for _ in {1..100}; do
  if ! kill -0 "$version_pid" 2>/dev/null; then
    version_complete=1
    break
  fi
  sleep 0.1
done
if [[ "$version_complete" -ne 1 ]]; then
  kill "$version_pid" 2>/dev/null || true
  wait "$version_pid" 2>/dev/null || true
  fail "downloaded binary version check timed out"
fi
if ! wait "$version_pid"; then
  fail "downloaded binary failed its version check"
fi
LC_ALL=C grep -Fxq "  Version          ${version}" "$version_file" || \
  fail "downloaded binary did not report manifest version $version"

if [[ -n "${ZO_CONFIG_HOME:-}" ]]; then
  config_home="$ZO_CONFIG_HOME"
elif [[ -n "${ZO_HOME:-}" ]]; then
  config_home="$ZO_HOME"
else
  config_home="${HOME}/.zo"
fi
mkdir -p "$config_home"
marker_file="$(mktemp "${config_home}/.managed-install.XXXXXX")"
canonical_install_dir="$(cd "$install_dir" && pwd -P)"
printf 'schema=1\npath=%s/zo\n' "$canonical_install_dir" > "$marker_file"
chmod 0600 "$marker_file"

if [[ -L "$destination" ]]; then
  fail "refusing symlink destination: $destination"
fi
mv -f "$binary_file" "$destination"
binary_file=""
mv -f "$marker_file" "${config_home}/managed-install"
marker_file=""

printf 'Installed zo v%s to %s\n' "$version" "$destination"
printf 'Ensure %s is on your PATH.\n' "$install_dir"
