#!/bin/zsh
# cargo-turn.sh <run-id> <command…> — one heavy cargo at a time per run: waits for the run's lock folder,
# runs the command, frees the lock. A dead holder's lock is taken over. Use for cargo build/test/clippy of
# zerocode-shell or zo-ide; a small core-only cargo may run without it.
#
# A turn ends with its command (t-89544). On 11-02 a timer set around this script ended the script and left
# five `node ui/tests/window.mjs` and their Chromes running for five hours: a foreground command runs on after
# its shell is gone, and a shell holds any trap until the command returns. So the command runs in the
# background with this script waiting, and every signal that can be caught, a timer's ALRM among them, ends the
# command and everything under it first: TERM, a moment, KILL. Each pid is found by its parent, never by a name,
# and the command stays in this script's process group, so whoever ends the group still reaches it. A KILL of
# this script alone cannot be caught: what it leaves, the window ends by itself.
#
# The installed copy is build-line/cargo-turn.sh, its locks beside it.
set -u
run=$1; shift
lock=${CARGO_TURN_LOCKS:-${0:A:h}}/cargo-turn-$run.lock
grace=${CARGO_TURN_GRACE_SECS:-5}
n=0
until mkdir $lock 2>/dev/null; do
  if [ -f $lock/pid ] && ! kill -0 $(cat $lock/pid) 2>/dev/null; then rm -rf $lock; continue; fi
  n=$((n+1)); [ $((n % 60)) = 0 ] && echo "cargo-turn: waiting for $run's turn ($((n/60)) min, held by $(cat $lock/pid 2>/dev/null))" >&2
  sleep 1
done
echo $$ > $lock/pid
command_pid=
tree_of() { local p; for p in $(pgrep -P "$1" 2>/dev/null); do tree_of "$p"; done; echo "$1"; }
any_alive() { local p; for p in "$@"; do kill -0 "$p" 2>/dev/null && return 0; done; return 1; }
leave() {
  trap '' INT TERM HUP ALRM
  if [ -n "$command_pid" ]; then
    local pids p waited=0
    pids=($(tree_of "$command_pid"))
    for p in $pids; do kill -TERM "$p" 2>/dev/null; done
    while [ $waited -lt $grace ] && any_alive $pids; do sleep 1; waited=$((waited+1)); done
    for p in $pids; do kill -KILL "$p" 2>/dev/null; done
  fi
  rm -rf $lock
  exit $1
}
trap 'leave 130' INT
trap 'leave 129' HUP
trap 'leave 142' ALRM
trap 'leave 143' TERM
trap 'rm -rf $lock' EXIT
# zsh starts a `&` command with INT and QUIT ignored while job control is off, and every exec and child below
# keeps that; perl puts both back to their defaults and execs the command's own words, never a shell.
perl -e '$SIG{INT} = $SIG{QUIT} = "DEFAULT"; exec { $ARGV[0] } @ARGV or exit 127' "$@" <&0 &
command_pid=$!
wait $command_pid
rc=$?
command_pid=
exit $rc
