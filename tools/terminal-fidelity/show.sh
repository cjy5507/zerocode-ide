#!/bin/sh
# Draw the fixture in THIS terminal and hold it until Enter.
#
#   sh tools/terminal-fidelity/show.sh <capture-name> [out-dir]
#
# Before drawing, writes what the terminal told the shell (size, TERM,
# COLORTERM, TERM_PROGRAM) to <out-dir>/env-<capture-name>.txt, because which
# colour road a program takes is decided by those variables, not by the
# renderer being measured.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
name=${1:?usage: show.sh <capture-name> [out-dir]}
out=${2:-$here/../../output/terminal-fidelity}
mkdir -p "$out"
size=$(stty size)
rows=${size% *}
cols=${size#* }
{
  printf 'name=%s\nrows=%s\ncols=%s\n' "$name" "$rows" "$cols"
  env | grep -E '^(TERM|COLORTERM|TERM_PROGRAM|TERM_PROGRAM_VERSION|LANG|LC_ALL|LC_CTYPE)=' | sort
} >"$out/env-$name.txt"
need_rows=$(python3 -c 'import sys; sys.path.insert(0, sys.argv[1]); import layout; print(layout.ROWS)' "$here")
need_cols=$(python3 -c 'import sys; sys.path.insert(0, sys.argv[1]); import layout; print(layout.COLS)' "$here")
if [ "$rows" -lt "$need_rows" ] || [ "$cols" -lt "$need_cols" ]; then
  echo "show.sh: this terminal is ${cols}x${rows}; the fixture needs ${need_cols}x${need_rows}" >&2
  exit 2
fi
restore() {
  stty echo 2>/dev/null || true
  printf '\033[0m\033[?25h\033[2J\033[H'
}
trap restore EXIT INT TERM
stty -echo
python3 "$here/fidelity.py" fixture
IFS= read -r _ || true
