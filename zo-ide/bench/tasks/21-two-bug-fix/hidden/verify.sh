#!/bin/sh
# The judge, kept where the model cannot reach it. $1 is a copy of the
# worked tree, $2 the task's original repo.
set -u
WORK=$1
ORIGIN=$2
cd "$WORK" || exit 2

cp "$ORIGIN/test_inventory.py" ./test_inventory.py || exit 2
python3 - <<'PY' || { echo "FAIL: hidden cases"; exit 1; }
import inventory
items = [{"name": "a", "qty": 2, "price": 1.0}, {"name": "b", "qty": 5, "price": 2.0}, {"name": "c", "qty": 1, "price": 3.0}]
assert inventory.total_units(items) == 8, inventory.total_units(items)
assert inventory.total_units([]) == 0
assert inventory.restock_needed(items, 2) == ["a", "c"], inventory.restock_needed(items, 2)
assert inventory.price_of(items, "b") == 2.0
PY
python3 -m unittest -q test_inventory >/dev/null 2>&1 || { echo "FAIL: visible tests"; exit 1; }
echo PASS
