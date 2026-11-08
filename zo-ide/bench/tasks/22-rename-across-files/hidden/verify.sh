#!/bin/sh
# The judge, kept where the model cannot reach it. $1 is a copy of the
# worked tree, $2 the task's original repo.
set -u
WORK=$1
ORIGIN=$2
cd "$WORK" || exit 2

if grep -rn "calc_total" --include='*.py' . >/dev/null; then echo "FAIL: calc_total is still named"; exit 1; fi
out=$(python3 cli.py 2x1.5 3x2) || { echo "FAIL: cli.py crashed"; exit 1; }
[ "$out" = "total 9.00
cli: 9.00" ] || { echo "FAIL: cli output changed: $out"; exit 1; }
python3 -c "import billing; assert billing.compute_total([{'qty': 1, 'unit': 0.333}]) == 0.33" || { echo "FAIL: compute_total"; exit 1; }
echo PASS
