#!/bin/sh
# Run the gate, show only its last 5 lines, and exit non-zero when it failed.
#
# The gate's own status, never the pipeline's: `./gate.sh | tail -5` reports
# tail's exit code, which is 0 whatever the gate did. Capture first, then read
# `$?`, then trim.
set -u
out=$(./gate.sh "${1:-green}" 2>&1)
rc=$?
printf '%s\n' "$out" | tail -5
exit "$rc"
