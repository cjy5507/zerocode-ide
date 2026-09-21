#!/bin/sh
# The work tree is copied to scratch and the hidden judge is dropped in there,
# so a model cannot pass by editing the tests it can see.
set -u
TASK=$(cd "$(dirname "$0")" && pwd)
WORK=$1
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/r49-check-17-XXXXXX")
trap 'rm -rf "$SCRATCH"' EXIT
cp -R "$WORK"/. "$SCRATCH"/ 2>/dev/null || { echo "FAIL: cannot copy work dir"; exit 2; }
sh "$TASK/hidden/verify.sh" "$SCRATCH" "$TASK/repo"
