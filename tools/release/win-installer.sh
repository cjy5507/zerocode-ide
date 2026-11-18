#!/bin/bash
# tools/release/win-installer.sh <checkout> <target-dir>
#
# The window's Windows installer, cross-built on this Mac (t-113717): zo for
# x86_64-pc-windows-msvc, then the NSIS installer that carries it, with cargo-xwin
# as tauri's runner and Homebrew's makensis — the road tauri's own guide gives for
# a macOS host ("Windows Installer" → building on Linux and macOS). No MSI: WiX
# runs on Windows only. No key is held here: the lane's bundle-updater signs the
# installer with the updater key, as it signs the macOS archive. There is no
# Authenticode signature (README.md: SmartScreen).
#
# The release lane runs it in its scratch (lane.sh build-win), with the Google
# sign-in client in the environment; anyone can run it in a checkout that has
# node_modules beside it. Every build goes to <target-dir>: the window and zo
# under <target-dir>/x86_64-pc-windows-msvc/release/, the installer under its
# bundle/nsis/. The checkout's staged zo (crates/zerocode-shell/bin/zo, the
# macOS one after a release build) is put back as it was on the way out.
#
# Exit 0: the installer exists and carries the window, every helper it starts
# beside itself and zo. 1: a step failed (its words on stderr). 2: a tool or an
# argument is missing.
set -u

WIN_TRIPLE=x86_64-pc-windows-msvc
LLVM_BIN=${WIN_LLVM_BIN:-/opt/homebrew/opt/llvm/bin}   # clang-cl, llvm-lib, llvm-rc; cargo-xwin brings its own lld-link
ZO_PROFILE=${ZO_PROFILE:-release}
# tauri's NSIS output is ZeroCode_<version>_x64-setup.exe; the updater artifacts are the lane's (bundle-updater)
NO_UPDATER_ARTIFACTS='{"bundle":{"createUpdaterArtifacts":false}}'

say() { printf 'win-installer: %s\n' "$*"; }
fail() { printf 'win-installer: %s\n' "$*" >&2; exit "${2:-1}"; }

[ $# -eq 2 ] || fail "usage: win-installer.sh <checkout> <target-dir>" 2
CHECKOUT=$(cd "$1" 2>/dev/null && pwd) || fail "no checkout at $1" 2
mkdir -p "$2" && TARGET=$(cd "$2" && pwd) || fail "cannot make the target folder $2" 2
SHELL_DIR=$CHECKOUT/crates/zerocode-shell
TAURI=$CHECKOUT/node_modules/.bin/tauri

for tool in cargo node makensis 7z python3; do
  command -v "$tool" > /dev/null 2>&1 || fail "$tool is not installed (makensis: brew install makensis; 7z: brew install p7zip)" 2
done
cargo xwin --version > /dev/null 2>&1 || fail "cargo-xwin is not installed (cargo install cargo-xwin)" 2
for tool in clang-cl llvm-lib llvm-rc; do
  [ -x "$LLVM_BIN/$tool" ] || fail "$LLVM_BIN/$tool is missing (brew install llvm)" 2
done
[ -x "$TAURI" ] || fail "no tauri CLI at $TAURI — link the repository's node_modules beside the checkout" 2
export PATH="$LLVM_BIN:$PATH"

VERSION=$(python3 "$CHECKOUT/tools/release/updater_feed.py" version "$CHECKOUT/Cargo.toml") \
  || fail "no [workspace.package] version in $CHECKOUT/Cargo.toml"

# The window's UI, as the release build ships it (tauri.release.conf.json build.frontendDist).
( cd "$CHECKOUT" && node scripts/build-ui-dist.mjs ) || fail "build-ui-dist failed"

# zo for Windows. It cannot run here, so the version it carries is read from its bytes: the
# `zo --version` word is the crate version compiled in (stage-zo.mjs runs it on the macOS side).
say "zo $VERSION for $WIN_TRIPLE"
( cd "$CHECKOUT/zo-ide" && CARGO_TARGET_DIR=$TARGET cargo xwin build --locked --profile "$ZO_PROFILE" -p zo-ide --bin zo --target "$WIN_TRIPLE" ) \
  || fail "cargo xwin build -p zo-ide failed"
ZO_EXE=$TARGET/$WIN_TRIPLE/$ZO_PROFILE/zo.exe
[ -f "$ZO_EXE" ] || fail "no zo.exe at $ZO_EXE"
[ "$(LC_ALL=C grep -c -a -F -e "$VERSION" "$ZO_EXE")" -gt 0 ] || fail "$ZO_EXE does not carry the workspace version $VERSION"

# Stage it where the bundle carries zo (bundle.resources "bin/zo"; the window installs it as
# zo.exe — zo_companion.rs), keeping the checkout's own staged zo aside until the end.
STAGED=$SHELL_DIR/bin/zo
mkdir -p "$SHELL_DIR/bin" || fail "cannot make $SHELL_DIR/bin"
put_back() { [ -f "$STAGED.host" ] && mv -f "$STAGED.host" "$STAGED"; }
trap put_back EXIT
[ -f "$STAGED" ] && { mv -f "$STAGED" "$STAGED.host" || fail "cannot set $STAGED aside"; }
cp "$ZO_EXE" "$STAGED.win" && mv -f "$STAGED.win" "$STAGED" || fail "cannot stage $ZO_EXE at $STAGED"

say "the window's NSIS installer for $WIN_TRIPLE"
( cd "$SHELL_DIR" && CI=true CARGO_TARGET_DIR=$TARGET "$TAURI" build --runner cargo-xwin --target "$WIN_TRIPLE" \
    --bundles nsis --ci --no-sign --config tauri.release.conf.json --config "$NO_UPDATER_ARTIFACTS" ) \
  || fail "tauri build --bundles nsis failed"

# tauri names it <productName>_<version>_<arch>-setup.exe, and a warm target keeps the installers of
# versions before: this version's, by name. The app's updater takes only ZeroCode_ assets
# (update_runtime.rs ASSET_PREFIX), so a renamed product fails here, not on a person's machine.
NSIS_DIR=$TARGET/$WIN_TRIPLE/release/bundle/nsis
INSTALLER=$(ls "$NSIS_DIR"/ZeroCode_"$VERSION"_*-setup.exe 2>/dev/null)
[ "$(printf '%s\n' "$INSTALLER" | grep -c .)" = 1 ] \
  || fail "expected one ZeroCode_${VERSION}_*-setup.exe under $NSIS_DIR, found: ${INSTALLER:-none} ($(ls "$NSIS_DIR" 2>/dev/null | tr '\n' ' '))"

# What the installer must carry: the window and each helper it starts from beside itself — every
# [[bin]] of the shell crate that needs no feature (zerocode-mirror: the panes' agent doors,
# hooks.rs mirror_bin) — and zo under bin/. Read from the installer itself, as 7-Zip lists it.
shell_bins() {
  awk '
    function done_bin() { if (inbin && name != "" && !req) print name; inbin = 0; name = ""; req = 0 }
    /^\[\[bin\]\]/ { done_bin(); inbin = 1; next }
    /^\[/ { done_bin(); next }
    inbin && /^name[ \t]*=/ { s = $0; sub(/^[^"]*"/, "", s); sub(/".*$/, "", s); name = s }
    inbin && /^required-features[ \t]*=/ { req = 1 }
    END { done_bin() }
  ' "$SHELL_DIR/Cargo.toml"
}
LISTING=$(7z l -slt -ba "$INSTALLER" 2>&1) || fail "7z cannot list $INSTALLER: $(printf '%s' "$LISTING" | tail -3)"
PATHS=$(printf '%s\n' "$LISTING" | sed -n 's/^Path = //p' | tr '\\' '/')
missing=
for want in $(shell_bins | sed 's/$/.exe/') bin/zo; do
  printf '%s\n' "$PATHS" | awk -v w="$want" '$0 == w || substr($0, length($0) - length(w)) == "/" w { found = 1 } END { exit !found }' \
    || missing="$missing $want"
done
[ -z "$missing" ] || fail "$(basename "$INSTALLER") does not carry:$missing"
say "$INSTALLER ($(wc -c < "$INSTALLER" | tr -d ' ') bytes) carries $(shell_bins | sed 's/$/.exe/' | tr '\n' ' ')bin/zo"
