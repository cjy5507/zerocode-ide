#!/bin/sh
set -eu
base=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
exec python3 "$base/fixture.py" verify Q5 "$1"
