#!/usr/bin/env bash
# tools/release/install.sh — install the ZeroCode app (and its bundled `zo`)
# from the release feed, on macOS Apple Silicon.
#
#   curl -fsSL https://github.com/cjy5507/zerocode/releases/latest/download/install.sh | bash
#
# What it does, in order: reads the updater feed (`latest.json`, the same file
# the installed app's updater reads), downloads that version's `.app.tar.gz`,
# verifies the feed's minisign signature when `minisign` is on PATH (the
# public key is the one in the app's tauri.conf.json), unpacks it beside the
# install directory, swaps it in by one rename (a running app keeps its old
# inode, as the app's own updater does), clears the quarantine attribute so
# Gatekeeper does not refuse an ad-hoc-signed bundle on first launch, and
# links `zo` into `~/.local/bin`.
#
# Knobs (env): ZEROCODE_VERSION=v1.1.4 pins a release instead of latest;
# ZEROCODE_INSTALL_DIR (default /Applications); ZEROCODE_BIN_DIR (default
# ~/.local/bin); ZEROCODE_REPO (default cjy5507/zerocode).
#
# Published as a release asset by tools/release/lane.sh (`publish`), so the
# `latest/download/install.sh` link above always names the installer that
# matches the feed beside it.
set -euo pipefail

REPO=${ZEROCODE_REPO:-cjy5507/zerocode}
INSTALL_DIR=${ZEROCODE_INSTALL_DIR:-/Applications}
BIN_DIR=${ZEROCODE_BIN_DIR:-$HOME/.local/bin}
APP_NAME=ZeroCode.app
PLATFORM=darwin-aarch64
FEED=latest.json
# The updater's public key (crates/zerocode-shell/tauri.conf.json plugins.updater.pubkey):
# a base64 minisign public-key file.
PUBKEY_B64='dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDU3MkI2QUQzMEQ3OUJEQ0EKUldUS3ZYa04wMnByVi9kZUZlbXNnN3ZrOEZXQ2wxTlZVS3g3M2t6ZmYwZERZL2hOS0VHL2ZCakoK'

fail() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }
say() { printf 'install.sh: %s\n' "$*"; }

case "$(uname -s):$(uname -m)" in
  Darwin:arm64 | Darwin:aarch64) ;;
  *) fail "ZeroCode ships for macOS on Apple Silicon only ($PLATFORM); this machine is $(uname -s) $(uname -m)" ;;
esac
command -v curl > /dev/null || fail "curl is required"
command -v tar > /dev/null || fail "tar is required"

if [ -n "${ZEROCODE_VERSION:-}" ]; then
  base="https://github.com/$REPO/releases/download/${ZEROCODE_VERSION#v}"
  case $ZEROCODE_VERSION in v*) base="https://github.com/$REPO/releases/download/$ZEROCODE_VERSION" ;; esac
else
  base="https://github.com/$REPO/releases/latest/download"
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/zerocode-install.XXXXXX")
trap 'rm -rf "$work"' EXIT

say "reading the release feed ($base/$FEED)"
curl -fsSL "$base/$FEED" -o "$work/$FEED" || fail "could not read $base/$FEED"
# One platform in the feed today, read without a JSON tool a fresh Mac may
# not have: the block under our platform's key, then its url and signature.
block=$(tr -d '\n' < "$work/$FEED" | sed -n "s/.*\"$PLATFORM\"[[:space:]]*:[[:space:]]*{\([^}]*\)}.*/\1/p")
[ -n "$block" ] || fail "the feed names no $PLATFORM build"
url=$(printf '%s' "$block" | sed -n 's/.*"url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
signature=$(printf '%s' "$block" | sed -n 's/.*"signature"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
version=$(tr -d '\n' < "$work/$FEED" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
[ -n "$url" ] && [ -n "$version" ] || fail "the feed has no url or version for $PLATFORM"
archive="$work/$(basename "$url")"

say "downloading ZeroCode $version"
curl -fL --progress-bar "$url" -o "$archive" || fail "download failed: $url"

if command -v minisign > /dev/null && [ -n "$signature" ]; then
  printf '%s' "$PUBKEY_B64" | base64 -d > "$work/updater.pub"
  printf '%s' "$signature" | base64 -d > "$work/archive.sig"
  minisign -Vq -p "$work/updater.pub" -x "$work/archive.sig" -m "$archive" \
    || fail "the archive's signature does not verify against the updater key"
  say "signature verified (minisign)"
else
  say "signature not checked: minisign is not installed (brew install minisign); the download came over HTTPS from github.com"
fi

say "unpacking"
mkdir -p "$work/unpacked"
tar -xzf "$archive" -C "$work/unpacked" || fail "could not unpack $archive"
[ -d "$work/unpacked/$APP_NAME" ] || fail "the archive holds no $APP_NAME"
bundled_zo="$work/unpacked/$APP_NAME/Contents/Resources/bin/zo"
[ -x "$bundled_zo" ] || fail "the bundle carries no zo at Contents/Resources/bin/zo"

mkdir -p "$INSTALL_DIR" || fail "cannot create $INSTALL_DIR"
[ -w "$INSTALL_DIR" ] || fail "$INSTALL_DIR is not writable (set ZEROCODE_INSTALL_DIR, or run as a user who owns it)"
staged="$INSTALL_DIR/.$APP_NAME.installing.$$"
rm -rf "$staged"
mv "$work/unpacked/$APP_NAME" "$staged" || fail "could not stage the bundle in $INSTALL_DIR"
# Gatekeeper refuses an ad-hoc-signed bundle that carries the quarantine
# attribute (the app is not notarised); the app's own updater installs
# without the attribute, and so does this.
xattr -dr com.apple.quarantine "$staged" 2> /dev/null || true
previous="$INSTALL_DIR/.$APP_NAME.previous.$$"
if [ -d "$INSTALL_DIR/$APP_NAME" ]; then
  mv "$INSTALL_DIR/$APP_NAME" "$previous" || fail "could not move the installed app aside"
fi
if ! mv "$staged" "$INSTALL_DIR/$APP_NAME"; then
  [ -d "$previous" ] && mv "$previous" "$INSTALL_DIR/$APP_NAME"
  fail "could not move the new app into place"
fi
rm -rf "$previous"
say "installed $INSTALL_DIR/$APP_NAME ($version)"

mkdir -p "$BIN_DIR"
ln -sfn "$INSTALL_DIR/$APP_NAME/Contents/Resources/bin/zo" "$BIN_DIR/zo"
say "linked $BIN_DIR/zo -> $INSTALL_DIR/$APP_NAME/Contents/Resources/bin/zo"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "add $BIN_DIR to PATH to run zo from a shell: export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
installed=$("$BIN_DIR/zo" --version 2> /dev/null || true)
say "done: ${installed:-zo installed} — open $INSTALL_DIR/$APP_NAME to start ZeroCode"
