#!/bin/bash
# tools/release/status.sh [--wait <sha>] [--timeout <secs>] — one line from status.json.
#
#   <sha8> <phase> <elapsed> <outcome> [v<version>] [<reason>] [skipped=<phase>(<why>),…] [unlisted=<n>(unlisted.txt)]
#
# A phase the lane skipped aloud — bundle-updater without a key, publish
# without RELEASE_PUBLISH=1 — is named at the end with its reason. A red
# outside flakes.txt the lane judged solo for this sha is counted last; the
# names and their judgments are in unlisted.txt beside installed.json (t-9741).
#
# --wait polls (POLL_SECS from lane.sh's table) until that sha has an outcome,
# then exits 0 on green, 1 on red, 3 on refused, 5 on superseded (a queued
# descendant took its place, t-4123). status.json is the running sha's; a sha
# the lane already finished is answered from its own out/lane-<sha8>.log, so
# a wait cannot hang once the lane has moved on. Without --wait it prints the
# current line and exits 0 (1 if there is no status file yet).
set -u
SELF_DIR=$(cd "$(dirname "$0")" && pwd)
eval "$(bash "$SELF_DIR/lane.sh" --table)"
want=; timeout=0
while [ $# -gt 0 ]; do
  case $1 in
    --wait) want=${2:-}; shift 2 ;;
    --timeout) timeout=${2:-0}; shift 2 ;;
    -h|--help) echo "usage: status.sh [--wait <sha>] [--timeout <secs>]"; exit 2 ;;
    *) echo "status: unknown argument $1"; exit 2 ;;
  esac
done
field() { sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p" "$STATUS"; }
raw() { sed -n "s/.*\"$1\":\([^,}]*\).*/\1/p" "$STATUS"; }
elapsed() { # since started_at, as Xm YYs
  local start now
  start=$(date -j -u -f %Y-%m-%dT%H:%M:%SZ "$1" +%s 2>/dev/null) || { echo "?"; return; }
  now=$(date +%s); printf '%dm%02ds' $(( (now - start) / 60 )) $(( (now - start) % 60 ))
}
skipped() { # -> "name(why),name(why)" or nothing
  grep -o '{"name":"[^"]*","rc":[0-9]*,"secs":[0-9]*,"skipped":"[^"]*"}' "$STATUS" 2>/dev/null \
    | sed 's/{"name":"\([^"]*\)".*"skipped":"\([^"]*\)"}/\1(\2)/' | paste -sd, -
}
unlisted() { # SHA8 -> how many unlisted reds the lane judged for it, or nothing
  local n; n=$(grep -c "^[^ ]* $1 " "$UNLISTED" 2>/dev/null)
  [ "${n:-0}" -gt 0 ] && printf '%s(%s)' "$n" "$(basename "$UNLISTED")"
}
line() {
  local sha phase outcome reason version skips judged
  sha=$(field sha); phase=$(field phase); outcome=$(raw outcome | tr -d '"'); reason=$(field reason)
  version=$(field version); skips=$(skipped); judged=$(unlisted "$(printf '%s' "$sha" | cut -c1-8)")
  [ "$outcome" = null ] && outcome=running
  printf '%s %s %s %s%s%s%s%s\n' "$(printf '%s' "$sha" | cut -c1-8)" "$phase" "$(elapsed "$(field started_at)")" "$outcome" \
    "${version:+ v$version}" "${reason:+ $reason}" "${skips:+ skipped=$skips}" "${judged:+ unlisted=$judged}"
}
if [ -z "$want" ]; then
  [ -f "$STATUS" ] || { echo "no status yet ($STATUS)"; exit 1; }
  line; exit 0
fi
exit_for() { case $1 in green) exit 0;; refused) exit 3;; superseded) exit 5;; *) exit 1;; esac; }
# The verdict a finished sha's own log ends with: `HH:MM:SS == <outcome> (…)`.
verdict_in_log() { sed -nE 's/^[0-9:]+ == (green|red|refused|superseded).*/\1/p' "$OUT/lane-$1.log" 2>/dev/null | tail -1; }
want8=$(printf '%s' "$want" | cut -c1-8)
t0=$(date +%s)
while :; do
  if [ -f "$STATUS" ]; then
    sha=$(field sha); outcome=$(raw outcome | tr -d '"')
    case $sha in "$want"*)
      if [ "$outcome" != null ]; then
        line
        exit_for "$outcome"
      fi ;;
    esac
  fi
  verdict=$(verdict_in_log "$want8")
  if [ -n "$verdict" ]; then
    printf '%s %s (out/lane-%s.log)\n' "$want8" "$verdict" "$want8"
    exit_for "$verdict"
  fi
  if [ "$timeout" -gt 0 ] && [ $(( $(date +%s) - t0 )) -ge "$timeout" ]; then echo "timeout waiting for $want"; exit 4; fi
  sleep "$POLL_SECS"
done
