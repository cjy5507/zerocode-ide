#!/bin/bash
# What is left after wave2: the pane lane, H₁ in zo's own clock, and axis E.
set -u
export PARITY_ROOT=/tmp/zo-agent-parity-20260907
export PARITY_ZO_BIN=${PARITY_ZO_BIN:?set PARITY_ZO_BIN}
log() { printf '%s  %s\n' "$(date +%H:%M:%S)" "$*"; }
log "pane lane"
bash "$PARITY_ROOT/harness/pane-wave.sh"
log "H1 in UTC"
bash "$PARITY_ROOT/harness/wave3.sh"
log DONE
