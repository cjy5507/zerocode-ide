#!/bin/bash
# H₁ isolated: the SAME instant written in the clock zo judges in (UTC), so a
# miss can only be the scheduler and a hit can only be the timezone.
set -u
export PARITY_ROOT=/tmp/zo-agent-parity-20260907
export PARITY_ZO_BIN=${PARITY_ZO_BIN:?set PARITY_ZO_BIN}
export PARITY_H_CRON_TZ=utc
python3 "$PARITY_ROOT/harness/drive.py" --subject zo --axis H --run H-zo-utc \
  --prompt "PARITYAXIS:H arm the schedules" --timeout 230 --mode pty
