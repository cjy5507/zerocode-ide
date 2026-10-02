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
set -u
strict_win_check=1
[ "${1-}" = --as-written ] && strict_win_check=0
recipes=$(sed -n 's/^verify:[[:space:]]*//p' justfile)
case $recipes in
  ''|*'('*|*'&&'*) exec just verify ;;   # no plain dependency list: the whole recipe, as before
esac
rc=0
red=
for recipe in $recipes; do
  # Log the strict name so the lane classifies a failed cross-check as a
  # real recipe failure, rather than the development gate's skipped success.
  case $recipe in win-check-if-available) [ "$strict_win_check" = 1 ] && recipe=win-check ;; esac
  # The lane reads each failure against the recipe it came from (failed_tests):
  # a recipe that failed without naming a test is a red of its own.
  echo "==> verify recipe $recipe"
  started=$SECONDS
  just "$recipe"; status=$?
  # Which recipe the gate's time went to (t-4004: 674 → 1208 s, and no log
  # said where). The lane's parser keys on `==>` alone; this line is inert to it.
  echo "<== verify recipe $recipe rc=$status $(( SECONDS - started ))s"
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
