#!/bin/bash
# tools/release/rollback.sh [--app] [--zo] — swap the .old copies back (default: both).
#
# The lane keeps <APP_DIR>/ZeroCode.app.old and ~/.local/bin/zo.old beside the
# live ones (design rule 1). Rolling back is the same beside-then-mv dance the
# other way, and installed.json takes the previous entry back from
# installed.prev.json so the window's 「new build ready」 stays honest. A second
# rollback flips forward again.
set -u
SELF_DIR=$(cd "$(dirname "$0")" && pwd)
eval "$(bash "$SELF_DIR/lane.sh" --table)"
app=0; zo=0
while [ $# -gt 0 ]; do
  case $1 in
    --app) app=1; shift ;;
    --zo) zo=1; shift ;;
    -h|--help) echo "usage: rollback.sh [--app] [--zo]"; exit 2 ;;
    *) echo "rollback: unknown argument $1"; exit 2 ;;
  esac
done
[ $app = 1 ] || [ $zo = 1 ] || { app=1; zo=1; }
now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
# The three below are lane.sh's installed.json functions, verbatim (this
# script sources only the table). A half is "sha at [version]" (t-3237).
installed_part() { [ -f "$1" ] || return 0; sed -n "s/.*\"$2\":{\"sha\":\"\([^\"]*\)\",\"at\":\"\([^\"]*\)\"\(,\"version\":\"\([^\"]*\)\"\)\{0,1\}}.*/\1 \2 \4/p" "$1"; }
part_json() { if [ -n "$1" ]; then set -- $1; printf '{"sha":"%s","at":"%s","version":"%s"}' "$1" "$2" "${3:-}"; else printf 'null'; fi; }
write_file() { printf '{"app":%s,"zo":%s}\n' "$(part_json "$2")" "$(part_json "$3")" > "$1.tmp" && mv -f "$1.tmp" "$1"; }
renow() { # "sha at [version]" -> the same pair with `at` = now
  [ -n "$1" ] || return 0; set -- $1; printf '%s %s %s' "$1" "$now" "${3:-}"
}
swap_installed() { # app|zo — installed <-> prev for that part, `at` = now, the version with its sha
  local a z pa pz cur prev
  a=$(installed_part "$INSTALLED" app); z=$(installed_part "$INSTALLED" zo)
  pa=$(installed_part "$INSTALLED_PREV" app); pz=$(installed_part "$INSTALLED_PREV" zo)
  case $1 in
    app) cur=$a; prev=$(renow "$pa"); write_file "$INSTALLED" "$prev" "$z"; write_file "$INSTALLED_PREV" "$cur" "$pz" ;;
    zo)  cur=$z; prev=$(renow "$pz"); write_file "$INSTALLED" "$a" "$prev"; write_file "$INSTALLED_PREV" "$pa" "$cur" ;;
  esac
}
rc=0
if [ $app = 1 ]; then
  cur=$RELEASE_APP_DIR/$APP_NAME; old=$cur.old; undo=$RELEASE_APP_DIR/.$APP_NAME.undo
  if [ -d "$old" ]; then
    rm -rf "$undo"
    { [ ! -d "$cur" ] || mv "$cur" "$undo"; } && mv "$old" "$cur" && { [ ! -d "$undo" ] || mv "$undo" "$old"; } \
      && swap_installed app && echo "app rolled back: $cur ← $old (the replaced one is now $old)" || { echo "app rollback failed"; rc=1; }
  else echo "app: nothing to roll back to ($old is missing)"; rc=1; fi
fi
if [ $zo = 1 ]; then
  cur=$RELEASE_ZO_BIN; old=$cur.old; new=$cur.new; undo=$cur.undo
  if [ -f "$old" ]; then
    # copy the old bytes to a fresh inode, keep the live inode as .undo, rename — never overwrite a running zo
    cp "$old" "$new" && { [ ! -f "$cur" ] || { rm -f "$undo"; ln "$cur" "$undo" 2>/dev/null || cp "$cur" "$undo"; }; } \
      && mv -f "$new" "$cur" && { [ ! -f "$undo" ] || mv -f "$undo" "$old"; } \
      && swap_installed zo && echo "zo rolled back: $cur ← $old" || { echo "zo rollback failed"; rc=1; }
  else echo "zo: nothing to roll back to ($old is missing)"; rc=1; fi
fi
exit $rc
