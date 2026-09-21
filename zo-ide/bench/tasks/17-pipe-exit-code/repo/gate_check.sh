#!/bin/sh
# Run the gate, show only its last 5 lines, and exit non-zero when it failed.
set -u
./gate.sh "${1:-green}" | tail -5
