#!/bin/sh
# gate_check.sh must report the gate's verdict, not the pipeline's.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE" || exit 2
fails=0

out=$(./gate_check.sh green 2>&1); rc=$?
[ "$rc" -eq 0 ] || { echo "FAIL: a green gate must exit 0, got $rc"; fails=$((fails + 1)); }
[ "$(printf '%s\n' "$out" | wc -l | tr -d ' ')" = "5" ] || {
  echo "FAIL: a green gate must print 5 lines, got:"; printf '%s\n' "$out"; fails=$((fails + 1)); }

out=$(./gate_check.sh red 2>&1); rc=$?
[ "$rc" -ne 0 ] || { echo "FAIL: a red gate must exit non-zero, got $rc"; fails=$((fails + 1)); }
[ "$(printf '%s\n' "$out" | wc -l | tr -d ' ')" = "5" ] || {
  echo "FAIL: a red gate must print 5 lines, got:"; printf '%s\n' "$out"; fails=$((fails + 1)); }

[ "$fails" -eq 0 ] && echo "all gate_check tests passed"
exit "$fails"
