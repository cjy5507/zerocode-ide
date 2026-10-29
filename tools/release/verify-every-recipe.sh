#!/usr/bin/env bash
# Run every recipe the justfile's `verify` names, in order, even after one
# fails; exit 1 if any failed. Run from the justfile's directory.
#
# `just verify` stops at its first failed recipe. On 2026-09-11 (1.3.33) a
# knowledge-graph timing flake ended the root gate before win-check ran, and
# the lane's solo judgment re-runs only the failed recipe — so whatever came
# after a flake would have shipped unjudged. Here every recipe speaks, and the
# gate log carries every failure for the judgment.
#
# `--as-written` is for a public CI leg (t-21326): the same every-recipe run,
# but with the recipes the justfile names — the leg has no cargo-xwin, so its
# `win-check-if-available` says SKIPPED aloud where the lane's would refuse.
#
# `--list` prints the recipes a run would run, one a line, and runs none; it
# fails when the justfile's `verify` is not a plain list. `--only "<a> <b>"`
# runs those of them alone, in the list's order: the release lane runs what a
# coordinator's receipt left owed (t-47516). A name outside the list is refused.
#
# `--stoppable` is the release lane's (t-83513): each recipe runs as a child this
# script waits for, and SIGUSR1 ends the one running — its own process tree, found
# by parent id, TERM then KILL — as a red recipe, and the run goes on to the next.
# The lane sends it when a recipe has written nothing past its cap (watch_gate in
# lane.sh): on 10-29 a deadlocked zo test held v1.1.104's gate 55 min in silence.
# Any other signal still ends the run, as below.
set -u
strict_win_check=1 list=0 only= only_given=0 stoppable=0
while [ $# -gt 0 ]; do
  case $1 in
    --as-written) strict_win_check=0; shift ;;
    --list) list=1; shift ;;
    --stoppable) stoppable=1; shift ;;
    --only) only=${2-}; only_given=1; shift 2 2>/dev/null || shift ;;
    *) echo "verify-every-recipe: unknown argument $1" >&2; exit 2 ;;
  esac
done
recipes=$(sed -n 's/^verify:[[:space:]]*//p' justfile)
case $recipes in
  ''|*'('*|*'&&'*)   # no plain dependency list: the whole recipe, as before
    [ "$list" = 0 ] && [ "$only_given" = 0 ] || { echo "verify-every-recipe: verify is not a plain list" >&2; exit 2; }
    exec just verify ;;
esac
# Log the strict name so the lane classifies a failed cross-check as a real
# recipe failure, rather than the development gate's skipped success.
[ "$strict_win_check" = 1 ] && recipes=$(printf '%s\n' $recipes | sed 's/^win-check-if-available$/win-check/')
if [ "$list" = 1 ]; then printf '%s\n' $recipes; exit 0; fi
if [ "$only_given" = 1 ]; then
  [ -n "$only" ] || { echo "verify-every-recipe: --only names no recipe" >&2; exit 2; }
  for recipe in $only; do
    case " $(printf '%s ' $recipes)" in *" $recipe "*) ;; *) echo "verify-every-recipe: $recipe is not a verify recipe" >&2; exit 2 ;; esac
  done
fi
# A process and everything under it, found by parent id: the running recipe's own tree.
tree_of() { local kid; printf '%s\n' "$1"; for kid in $(pgrep -P "$1" 2>/dev/null); do tree_of "$kid"; done; }
child= stopped=
# SIGUSR1 (--stoppable): the running recipe, ended — TERM to its tree, then KILL to
# what is left after a grace. Between recipes there is nothing to end.
stop_recipe() {
  local pids
  [ -n "$child" ] || return 0
  stopped=$recipe
  pids=$(tree_of "$child")
  kill -TERM $pids 2>/dev/null
  sleep 2
  kill -KILL $pids 2>/dev/null
  return 0
}
[ "$stoppable" = 0 ] || trap stop_recipe USR1
rc=0
red=
for recipe in $recipes; do
  [ "$only_given" = 0 ] || case " $only " in *" $recipe "*) ;; *) continue ;; esac
  # The lane reads each failure against the recipe it came from (failed_tests):
  # a recipe that failed without naming a test is a red of its own.
  echo "==> verify recipe $recipe"
  started=$SECONDS
  if [ "$stoppable" = 1 ]; then
    # A trapped signal ends `wait` at once, where a foreground child would hold
    # it until the child exits; the child is waited for again until it is gone.
    just "$recipe" & child=$!
    while :; do wait "$child"; status=$?; kill -0 "$child" 2>/dev/null || break; done
    child=
  else
    just "$recipe"; status=$?
  fi
  # Which recipe the gate's time went to (t-4004: 674 → 1208 s, and no log
  # said where). The lane's parser keys on `==>` alone; this line is inert to it.
  echo "<== verify recipe $recipe rc=$status $(( SECONDS - started ))s"
  if [ "$stopped" = "$recipe" ]; then
    # just's own words for a recipe ended by a signal, so the lane's parser reads
    # it as the red recipe it is (`recipe:<name>`, red for real: no test spoke).
    echo "error: recipe \`$recipe\` was terminated: no output past the release lane's cap"
    stopped= status=1
  fi
  # 128+N is a recipe (or just) ended by a signal: the lane tearing the gate
  # down children first. Starting the next recipe then would outlive the lane
  # in a scratch it is sweeping — stop with the signal's status instead.
  [ "$status" -ge 128 ] && exit "$status"
  [ "$status" = 0 ] && continue
  rc=1
  red="$red $recipe"
  # A GitHub run shows these on the job itself, so the reader needs no log.
  [ "${GITHUB_ACTIONS-}" = true ] && echo "::error title=verify::recipe $recipe failed (rc=$status)"
done
# A log is read from its end: the last line names every red recipe, not only
# the first one (t-21326). No backticks — the lane's parser keys on just's own
# "recipe `name` failed" words, and this line must stay nothing to it.
[ -n "$red" ] && echo "verify: red recipes:$red"
exit "$rc"
