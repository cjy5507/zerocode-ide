#!/bin/bash
# Sign only a staged helper, never the executable of a running application.
set -euo pipefail
SIGNING_DIR=$(cd "$(dirname "$0")" && pwd)
app=${1:?usage: sign-computer-use.sh helper.app}
bundle_id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  exec bash "$SIGNING_DIR/sign-developer-id.sh" "$app" "$bundle_id"
fi
if fingerprint=$(bash "$SIGNING_DIR/ensure-local-identity.sh"); then
  requirement="=designated => identifier \"$bundle_id\" and certificate leaf = H\"$fingerprint\""
  codesign --force --sign "$fingerprint" --timestamp=none --requirements "$requirement" "$app"
else
  echo 'Computer Use identity=adhoc: local signing unavailable; rebuilt helpers may need permissions --reset' >&2
  codesign --force --sign - "$app"
fi
codesign --verify --strict "$app"
