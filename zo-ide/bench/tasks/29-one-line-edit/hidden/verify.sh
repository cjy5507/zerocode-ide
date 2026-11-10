#!/bin/sh
# The judge, kept where the model cannot reach it. $1 is a copy of the
# worked tree, $2 the task's original repo ($3, the model's last message, is
# not read: the tree is the answer).
set -u
WORK=$1
ORIGIN=$2
cd "$WORK" || exit 2

# The prompt gives the line word for word, so the change can be made without
# reading the file first. A harness that turns such an edit away until the
# file has been read costs a read and a second edit; `requests` shows it.
printf 'name = "demo"\nhost = "127.0.0.1"\nport = 9090\ntimeout_secs = 30\n' > expected.toml
cmp -s config.toml expected.toml || { echo "FAIL: config.toml is not the one line changed"; diff expected.toml config.toml | head -8; exit 1; }
echo PASS
