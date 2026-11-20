#!/bin/bash
# tools/release/coord-gate.sh --who <owner> --logs <folder> [--calm-wait <secs>] <root|zo>...
#
# The coordinator's gate as a `zo line` job whose receipt the release lane takes
# (coord_receipt.py, tools/release/README.md "A coordinator's gate receipt", t-119757).
# Put it in from the top of the checkout it gates — the stacked tree, clean — under the
# owner it names, one job a gate (they run one after another: --quiet runs alone):
#
#   cd <stack> && zo line add --owner coord-<label>-root --first --quiet --cap 2h --notify run:<run> -- \
#     bash tools/release/coord-gate.sh --who coord-<label>-root --logs <folder> root
#   cd <stack> && zo line add --owner coord-<label>-zo --first --quiet --cap 1h --notify run:<run> -- \
#     bash tools/release/coord-gate.sh --who coord-<label>-zo --logs <folder> zo
#
# (both gates in one job, owner coord-<label>: `--who coord-<label> --logs <folder> root zo`).
# For each gate named it runs every recipe of that gate's list — verify-every-recipe.sh
# --list, the lane's own, the strict win-check included — as exactly `just <recipe>`: the
# root's from the top, zo's as `bash -c 'cd zo-ide && just <recipe>'` (key zo-<recipe>),
# each entry's output in <folder>/<key>.log, in the environment the lane's gates get
# (lane-env.sh), once the machine is as calm as the lane waits for before a gate
# (CALM_LOAD, at most --calm-wait seconds, the table's CALM_WAIT_SECS by default). It
# never goes to another tree: what it gates is the checkout the line ran it in, and the
# logs folder lies outside it.
#
# Its log — the job's own, <zo home>/line/logs/<id>.log — says, one a line:
#   # who: <owner>   # run: <its pane's `zerocode-orc run-current`>   # cwd: <the checkout>   # logs: <folder>
#   head <sha> tree <tree>
#   entry <key> <command>   then   RC <key>=<rc> (<n>s)     for every entry
#   verdict: bad=<n> (<gates> on <head>)
# the lines coord_receipt.py reads. `--in-lane-env` is its own second step, not a way in: a job
# put in with it skipped lane-env.sh and run-current, and its receipt is refused. It exits 0 when every entry was green and the checkout is
# still that head with its tracked files unchanged, 1 when one was not, and 2 when it could not
# start (it starts only in a clean checkout).
set -u
self_dir=$(cd "$(dirname "$0")" && pwd -P)
usage() { echo "usage: coord-gate.sh --who <coord-label[-root|-zo]> --logs <folder outside the checkout> [--calm-wait <secs>] <root|zo>..." >&2; exit 2; }
stop() { echo "STOP $*"; exit 2; }

if [ "${1-}" != --in-lane-env ]; then
  who= logs= calm= lanes=
  while [ $# -gt 0 ]; do
    case $1 in
      --who) [ -n "${2-}" ] || usage; who=$2; shift 2 ;;
      --logs) [ -n "${2-}" ] || usage; logs=$2; shift 2 ;;
      --calm-wait) case ${2-} in ''|*[!0-9]*) usage ;; esac; calm=$2; shift 2 ;;
      root|zo) case " $lanes " in *" $1 "*) usage ;; esac; lanes="$lanes $1"; shift ;;
      *) usage ;;
    esac
  done
  [ -n "$who" ] && [ -n "$logs" ] && [ -n "$lanes" ] || usage
  # The run is the pane's that put the job in: the line starts the job with that pane's environment.
  run=$(zerocode-orc run-current 2>/dev/null | python3 -c 'import json, sys; print(json.load(sys.stdin)["runId"])' 2>/dev/null)
  case $run in run-[0-9]*) ;; *) stop "no run: zerocode-orc run-current named none — put the job in from the coordinator's pane" ;; esac
  # shellcheck disable=SC2086
  exec bash "$self_dir/lane-env.sh" bash "$self_dir/coord-gate.sh" --in-lane-env "$who" "$run" "$logs" "${calm:--}" $lanes
fi

shift
who=$1 run=$2 logs=$3 calm=$4; shift 4
here=$(pwd -P)
echo "# who: $who"
echo "# run: $run"
echo "# cwd: $here"
case $logs in /*) ;; *) stop "the logs folder $logs is no absolute path" ;; esac
case $logs/ in "$here"/*) stop "the logs folder $logs is inside the checkout it gates" ;; esac
mkdir -p "$logs" 2>/dev/null && logs=$(cd "$logs" && pwd -P) || stop "cannot make the logs folder $logs"
case $logs/ in "$here"/*) stop "the logs folder $logs is inside the checkout it gates" ;; esac
echo "# logs: $logs"
[[ $who =~ ^coord-[A-Za-z0-9]+(-(root|zo))?$ ]] || stop "$who is not a coordinator's coord-<label>, coord-<label>-root or coord-<label>-zo"
top=$(git rev-parse --show-toplevel 2>/dev/null) && top=$(cd "$top" && pwd -P) || stop "$here is in no git checkout"
[ "$top" = "$here" ] || stop "the job runs in $here, not the top of its checkout ($top)"
dirty() { git status --porcelain 2>/dev/null | grep -vE '^\?\? node_modules/?$'; }   # the lane's scratch links node_modules too
[ -z "$(dirty)" ] || stop "the checkout is not clean: $(dirty | head -3 | tr '\n' ' ')"
head=$(git rev-parse HEAD) && tree=$(git rev-parse 'HEAD^{tree}') || stop "git cannot read HEAD"
echo "head $head tree $tree"

table=$(bash "$self_dir/lane.sh" --table)
said() { printf '%s\n' "$table" | sed -n "s/^$1='\(.*\)'$/\1/p"; }
calm_load=$(said CALM_LOAD); calm_poll=$(said CALM_POLL_SECS)
[ "$calm" != - ] || calm=$(said CALM_WAIT_SECS)
wait_for_calm() { # GATE — as the lane waits before a gate: the 1-minute load at or under CALM_LOAD, at most CALM seconds
  local t0=$SECONDS l1
  while :; do
    l1=$(sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}')
    if [ -z "$l1" ]; then echo "load unreadable before $1 — running without the calm check"; return 0; fi
    if awk -v l="$l1" -v c="$calm_load" 'BEGIN { exit !(l + 0 <= c + 0) }'; then echo "load $l1 before $1 (calm at or under $calm_load)"; return 0; fi
    if [ $(( SECONDS - t0 )) -ge "$calm" ]; then echo "load $l1 before $1 — still over $calm_load after ${calm}s, running anyway"; return 0; fi
    sleep "${calm_poll:-30}"
  done
}
# A word with anything but these letters goes in single quotes; no word here holds one.
quote() { local word out=; for word in "$@"; do case $word in ''|*[!A-Za-z0-9_./:=@%+-]*) out="$out '$word'" ;; *) out="$out $word" ;; esac; done; printf '%s' "${out# }"; }
bad=0
entry() { # KEY COMMAND... — said, run with its output in <logs>/<key>.log, closed with its RC line
  local key=$1 started rc
  shift
  echo "entry $key $(quote "$@")"
  started=$SECONDS
  "$@" > "$logs/$key.log" 2>&1 < /dev/null; rc=$?
  echo "RC $key=$rc ($(( SECONDS - started ))s)"
  [ "$rc" = 0 ] || bad=$(( bad + 1 ))
}
for lane in "$@"; do
  wait_for_calm "$lane"
  if [ "$lane" = root ]; then
    recipes=$(bash "$self_dir/verify-every-recipe.sh" --list) || recipes=
    [ -n "$recipes" ] || { echo "STOP the root gate's recipes are unknown (verify-every-recipe.sh --list)"; bad=$(( bad + 1 )); continue; }
    for recipe in $recipes; do entry "$recipe" just "$recipe"; done
  else
    recipes=$(cd zo-ide && bash "$self_dir/verify-every-recipe.sh" --list) || recipes=
    [ -n "$recipes" ] || { echo "STOP the zo gate's recipes are unknown (verify-every-recipe.sh --list in zo-ide)"; bad=$(( bad + 1 )); continue; }
    for recipe in $recipes; do entry "zo-$recipe" bash -c "cd zo-ide && just $recipe"; done
  fi
done
# At the end the head and every tracked file are what was gated; a file a recipe left untracked is
# no change to the tree, and the lane's scratch is left the same file.
edited() { git status --porcelain --untracked-files=no 2>/dev/null; }
if [ "$(git rev-parse HEAD 2>/dev/null)" != "$head" ] || [ -n "$(edited)" ]; then
  echo "STOP the checkout changed while the gate ran: $(git rev-parse --short=9 HEAD 2>/dev/null) $(edited | head -3 | tr '\n' ' ')"
  bad=$(( bad + 1 ))
fi
echo "verdict: bad=$bad ($* on ${head:0:9})"
[ "$bad" = 0 ]
