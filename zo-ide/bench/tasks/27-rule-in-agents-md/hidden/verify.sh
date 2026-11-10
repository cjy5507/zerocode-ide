#!/bin/sh
# The judge, kept where the model cannot reach it. $1 is a copy of the
# worked tree, $2 the task's original repo, $3 the model's last message.
set -u
WORK=$1
ORIGIN=$2
SAID=$3
cd "$WORK" || exit 2

# The command stands in AGENTS.md and nowhere else: a harness that carries the
# workspace's rules into the prompt answers at once, one that does not has to
# find the file first. Either passes; `requests` tells them apart.
grep -q 'make zebra-check' "$SAID" 2>/dev/null || { echo "FAIL: the answer was: $(head -c 200 "$SAID" 2>/dev/null)"; exit 1; }
[ -z "$(git status --porcelain 2>/dev/null)" ] || { echo "FAIL: a question changed the tree"; git status --porcelain | head -5; exit 1; }
echo PASS
