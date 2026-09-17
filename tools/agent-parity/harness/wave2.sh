#!/bin/bash
# The 09-07 scripted wave, axes K..L. One table of invocations, exactly the
# 09-06 ones (`docs/analysis/zo-agent-parity-20260906.md`): a re-measurement
# that changed a prompt would not be one.
set -u
export PARITY_ROOT=/tmp/zo-agent-parity-20260907
export PARITY_ZO_BIN=${PARITY_ZO_BIN:?set PARITY_ZO_BIN}
H=$PARITY_ROOT/harness
log() { printf '%s  %s\n' "$(date +%H:%M:%S)" "$*"; }

drive() { log "START $*"; python3 "$H/drive.py" "$@"; }

axis() { # axis <subject> <axis-id> <timeout>
  local s=$1 a=$2 t=$3
  local prompt="PARITYAXIS:$a run the parity scenario"
  local extra=()
  case "$a" in
    K) prompt="PARITYAXIS:K delegate once" ;;
    H) prompt="PARITYAXIS:H arm the schedules" ;;
    G) extra=(--interrupt-on G_g3_started --followup "PARITYAXIS:G resume the workflow") ;;
    I) extra=(--repo "$PARITY_ROOT/repo-$s" --wt "$PARITY_ROOT/wt-$s/parity-i") ;;
    J) extra=(--fixture "$PARITY_ROOT/fixtures/context.txt") ;;
    L) if [ "$s" = zo ]; then prompt="/loop every 1m PARITYAXIS:L loop tick"
       else prompt="/loop 1m PARITYAXIS:L loop tick"; fi ;;
  esac
  drive --subject "$s" --axis "$a" --run "$a-$s" --prompt "$prompt" \
        --timeout "$t" --mode pty "${extra[@]+"${extra[@]}"}"
}

for s in zo claude; do
  axis $s K 90
  axis $s B 180
  axis $s C 180
  axis $s D 180
  axis $s F 180
  axis $s G 180
  axis $s I 180
  axis $s J 180
done
for s in zo claude; do axis $s H 230; done
for s in zo claude; do axis $s L 400; done
log DONE
