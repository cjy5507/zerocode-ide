#!/bin/sh
# The judge, kept where the model cannot reach it. $1 is a copy of the
# worked tree, $2 the task's original repo.
set -u
WORK=$1
ORIGIN=$2
cd "$WORK" || exit 2

python3 - "$ORIGIN/handlers.py" <<'PY' || { echo "FAIL: handlers.py"; exit 1; }
import ast, sys
import handlers
assert handlers.handle_refund({"paid": False, "amount": 50}) == {"status": "rejected", "amount": 0}
assert handlers.handle_refund({"paid": True, "amount": 50, "fee": 5}) == {"status": "refunded", "amount": 45}
assert handlers.handle_refund({"paid": True, "amount": 50}) == {"status": "refunded", "amount": 50}
def bodies(source):
    tree = ast.parse(source)
    return {node.name: ast.dump(node) for node in tree.body if isinstance(node, ast.FunctionDef) and node.name != "handle_refund"}
before = bodies(open(sys.argv[1]).read())
after = bodies(open("handlers.py").read())
assert before == after, sorted(set(before.items()) ^ set(after.items()))[:2]
PY
echo PASS
