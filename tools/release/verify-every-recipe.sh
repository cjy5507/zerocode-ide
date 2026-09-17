#!/usr/bin/env bash
# Run every recipe the justfile's `verify` names, in order, even after one
# fails; exit 1 if any failed. Run from the justfile's directory.
#
# `just verify` stops at its first failed recipe. On 2026-09-11 (1.3.33) a
# knowledge-graph timing flake ended the root gate before win-check ran, and
# the lane's solo judgment re-runs only the failed recipe — so whatever came
# after a flake would have shipped unjudged. Here every recipe speaks, and the
# gate log carries every failure for the judgment.
set -u
recipes=$(sed -n 's/^verify:[[:space:]]*//p' justfile)
case $recipes in
  ''|*'('*|*'&&'*) exec just verify ;;   # no plain dependency list: the whole recipe, as before
esac
rc=0
for recipe in $recipes; do
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
  [ "$status" = 0 ] || rc=1
done
exit "$rc"
