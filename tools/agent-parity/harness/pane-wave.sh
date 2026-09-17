#!/bin/bash
# The pane half: sub-agents as real panes of the installed ZeroCode window.
# One at a time, and every pane this wave leaves behind is ended before the
# next run, so no run is measured beside another run's child.
set -u
export PARITY_ROOT=/tmp/zo-agent-parity-20260907
export PARITY_ZO_BIN=${PARITY_ZO_BIN:?set PARITY_ZO_BIN}
H=$PARITY_ROOT/harness
KEEP=$(tmux list-panes -F '#{pane_id}' | tr '\n' ' ')
log() { printf '%s  %s\n' "$(date +%H:%M:%S)" "$*"; }
log "panes already standing (kept): $KEEP"

sweep() { # end every pane this wave opened; a teammate that is still seated is one
  for p in $(tmux list-panes -F '#{pane_id}'); do
    case " $KEEP " in *" $p "*) ;; *) log "sweeping $p"; tmux kill-pane -t "$p" >/dev/null 2>&1 ;; esac
  done
}

run() { # run <axis> <name> <timeout>
  log "START $2"
  python3 "$H/drive.py" --subject zo --axis "$1" --run "$2" \
    --prompt "PARITYAXIS:$1 run the parity scenario" --timeout "$3" --mode pty --panes
  sweep
}

for i in 1 2 3 4 5; do run A "AP$i-zo" 120; done
run M "M-zo" 200
log DONE
