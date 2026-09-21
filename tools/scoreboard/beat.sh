#!/bin/bash
# tools/scoreboard/beat.sh — the scoreboard beat's clock hand (docs/design/scoreboard-beat-20260911.md).
#
# Runs `zo scoreboard` over the last day against the checked-in baseline and
# DEFERS its findings to the inbox file beside its own state; the window's
# standing-order beat reads that file and files them as ledger tasks from its
# own seat. A launchd clock has no pane identity, so the beat never speaks to
# the ledger itself. The inbox is NOT under the window's local data root: which
# root that is (the platform directory or the legacy ~/.zerocode/state) is the
# window's path authority to decide, and a shell script cannot ask it — the
# same reason the release lane's files live under ~/.local/share/zerocode.
# The window reads $STATE/pending.jsonl (scoreboard_inbox::BEAT_DIR). Every run
# leaves one line in beat.log and the last report in last-run.json.
set -u
SELF_DIR=$(cd "$(dirname "$0")" && pwd)
REPO=${SCOREBOARD_REPO:-$(cd "$SELF_DIR/../.." && pwd)}
HOME_DIR=${HOME:?}
STATE=${SCOREBOARD_STATE:-$HOME_DIR/.local/share/zerocode/scoreboard}
BASELINE=${SCOREBOARD_BASELINE:-$REPO/zo-ide/bench/scoreboard-baseline.json}
SINCE=${SCOREBOARD_SINCE:-24h}
ZO=${SCOREBOARD_ZO:-$HOME_DIR/.local/bin/zo}
PENDING="$STATE/pending.jsonl"
mkdir -p "$STATE"
stamp=$(date +%FT%T%z)
if [ ! -x "$ZO" ]; then
  echo "$stamp refused: no zo at $ZO" >> "$STATE/beat.log"; exit 3
fi
if [ ! -f "$BASELINE" ]; then
  echo "$stamp refused: no baseline at $BASELINE — write one with: zo scoreboard --since 7d --baseline $BASELINE --write-baseline" >> "$STATE/beat.log"; exit 3
fi
cd "$REPO" || exit 3
out=$("$ZO" scoreboard --since "$SINCE" --baseline "$BASELINE" --defer "$PENDING" 2>&1); rc=$?
printf '%s\n' "$out" > "$STATE/last-run.json"
if [ "$rc" != 0 ]; then
  echo "$stamp red rc=$rc $(printf '%s' "$out" | head -c 300)" >> "$STATE/beat.log"; exit "$rc"
fi
summary=$(printf '%s' "$out" | python3 -c 'import sys,json
raw=sys.stdin.read(); i=raw.find("{"); j=raw.rfind("}")
try:
    d=json.loads(raw[i:j+1]); w=d["window"]
    print("requests=%s breaks=%s findings=%s deferred=%s disk=%sG" % (w["requests"], w["breaks"], len(d["findings"]), d.get("deferred",0), w.get("disk_free_gb")))
except Exception as e: print("unparsed:", raw[:120].replace("\n"," "))' 2>/dev/null)
echo "$stamp green $summary" >> "$STATE/beat.log"
