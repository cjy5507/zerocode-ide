#!/bin/sh
# The judge, kept where the model cannot reach it. $1 is a copy of the
# worked tree, $2 the task's original repo, $3 the model's last message.
set -u
WORK=$1
ORIGIN=$2
SAID=$3
cd "$WORK" || exit 2

# The picture is 96x96 of one red. A harness whose file reader opens a picture
# gets there in one call; one that keeps pictures behind another tool takes a
# lookup first. `requests` tells them apart.
# The answer names red and no other colour; a sentence around the word, or a
# line saying how it was found, is not what this task counts.
grep -qiw 'red' "$SAID" 2>/dev/null || { echo "FAIL: the answer was: $(head -c 200 "$SAID" 2>/dev/null)"; exit 1; }
if grep -qiwE 'blue|green|yellow|orange|purple|pink|black|white|brown|gr[ae]y' "$SAID"; then
    echo "FAIL: the answer names another colour: $(head -c 200 "$SAID")"; exit 1
fi
cmp -s shot.png "$ORIGIN/shot.png" || { echo "FAIL: shot.png was changed"; exit 1; }
echo PASS
