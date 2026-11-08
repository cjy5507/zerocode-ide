#!/bin/sh
# The judge, kept where the model cannot reach it. $1 is a copy of the
# worked tree, $2 the task's original repo.
set -u
WORK=$1
ORIGIN=$2
cd "$WORK" || exit 2

[ ! -e legacy_math.py ] || { echo "FAIL: legacy_math.py still exists"; exit 1; }
if grep -q "legacy" app.py; then echo "FAIL: app.py still names legacy"; exit 1; fi
python3 -c "import app, math; assert app.diagonal(3, 4) == 5.0 and app.side_of_square(16) == 4.0; assert 'math' in open('app.py').read()" || { echo "FAIL: app behavior"; exit 1; }
echo PASS
