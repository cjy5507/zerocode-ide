#!/bin/bash
# Rebuilds the dex the window pushes to put text into an Android field
# (crates/zerocode-shell/src/emulator/android_text). Needs a JDK and the
# Android SDK's platform 35 and build-tools 36.0.0; the dex is committed, so a
# build without the SDK still has it. After a rebuild, update INSERT_DEX_SHA256
# in crates/zerocode-shell/src/emulator/android.rs.
set -euo pipefail
here=$(cd "$(dirname "$0")/../.." && pwd)
src="$here/crates/zerocode-shell/src/emulator/android_text"
sdk=${ANDROID_HOME:-$HOME/Library/Android/sdk}
android_jar="$sdk/platforms/android-35/android.jar"
d8="$sdk/build-tools/36.0.0/d8"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
javac -source 8 -target 8 -nowarn -cp "$android_jar" -d "$work/classes" "$src/dev/zerocode/text/Insert.java"
"$d8" --release --min-api 26 --lib "$android_jar" --output "$work" "$work/classes/dev/zerocode/text/Insert.class"
cp "$work/classes.dex" "$src/insert.dex"
shasum -a 256 "$src/insert.dex"
