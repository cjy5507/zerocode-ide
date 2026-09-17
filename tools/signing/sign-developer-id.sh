#!/bin/bash
# Sign staged nested code before Tauri seals its containing application.
# An explicit distribution identity must never fall back to local signing.
set -euo pipefail
path=${1:?usage: sign-developer-id.sh path [identifier] [entitlements]}
identifier=${2:-}
entitlements=${3:-}
identity=${APPLE_SIGNING_IDENTITY:?APPLE_SIGNING_IDENTITY is required}
args=(--force --options runtime --timestamp)
[ -z "$identifier" ] || args+=(--identifier "$identifier")
[ -z "$entitlements" ] || args+=(--entitlements "$entitlements")
codesign "${args[@]}" --sign "$identity" "$path"
codesign --verify --strict "$path"
