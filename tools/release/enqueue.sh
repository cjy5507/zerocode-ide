#!/bin/bash
# tools/release/enqueue.sh <sha> [--receipt <file> [--keep-only]] — put a sha on the release lane's queue.
#
# The lane (launchd dev.zerocode.release, QueueDirectories on the queue dir)
# picks it up oldest-first. Watch it with tools/release/status.sh [--wait <sha>].
# The old ~/.local/share/zerocode/release/release*.sh forward here.
#
# --receipt carries the coordinator's gate receipt for the tree the sha was cut
# from (coord_receipt.py make): the lane then runs only the gate recipes it left
# owed (t-47516). It is read here once, as the lane reads it — of zo line jobs the
# build line ran (RELEASE_BUILD_LINE) that say they are the coordinator's (RELEASE_COORD_RUN);
# a receipt that does not stand is refused and nothing is queued or kept — and kept as
# <receipts>/<sha>.json before the queue file appears, so the lane never takes the
# sha without it. --keep-only keeps it and queues nothing: for a lane the coordinator
# starts with the sha itself (lane.sh <sha>), which reads <receipts>/<sha>.json all the
# same (t-119757).
set -u
SELF_DIR=$(cd "$(dirname "$0")" && pwd)
eval "$(bash "$SELF_DIR/lane.sh" --table)"
usage() { echo "usage: enqueue.sh <sha> [--receipt <file> [--keep-only]]"; exit 2; }
sha= receipt= keep_only=0
while [ $# -gt 0 ]; do
  case $1 in
    --receipt) [ -n "${2:-}" ] || usage; receipt=$2; shift 2 ;;
    --keep-only) keep_only=1; shift ;;
    -h|--help) usage ;;
    *) [ -z "$sha" ] || usage; sha=$1; shift ;;
  esac
done
case $sha in
  "") usage ;;
  *[!0-9a-fA-F]*) echo "enqueue: '$sha' is not a sha (use a commit id, not a ref)"; exit 2 ;;
esac
[ ${#sha} -ge 7 ] || { echo "enqueue: '$sha' is too short for a sha"; exit 2; }
[ "$keep_only" = 0 ] || [ -n "$receipt" ] || { echo "enqueue: --keep-only keeps a receipt: name it with --receipt"; exit 2; }
if [ "${RELEASE_DRY_RUN:-0}" = 1 ]; then full=$sha
else
  full=$(git -C "$RELEASE_REPO" rev-parse --verify --quiet "$sha^{commit}" 2>/dev/null) \
    || { echo "enqueue: $sha is unknown to $RELEASE_REPO (fetch it there first)"; exit 1; }
fi
short=$(printf '%s' "$full" | cut -c1-8)
nothing="nothing queued"; [ "$keep_only" = 0 ] || nothing="nothing kept"
if [ -n "$receipt" ]; then
  said=$(python3 "$RECEIPT_PY" read --receipt "$receipt" --line "$RELEASE_BUILD_LINE" --run "$RELEASE_COORD_RUN" 2>&1) \
    || { echo "enqueue: the receipt is refused — $said; $nothing"; exit 1; }
  mkdir -p "$RECEIPTS" && cp "$receipt" "$RECEIPTS/$full.json.tmp" && mv -f "$RECEIPTS/$full.json.tmp" "$RECEIPTS/$full.json" \
    || { echo "enqueue: the receipt could not be kept under $RECEIPTS; $nothing"; exit 1; }
  echo "receipt $(printf '%s\n' "$said" | sed -n 's/^who //p') tree $(printf '%s\n' "$said" | sed -n 's/^tree //p' | cut -c1-8) kept for $short"
  if [ "$keep_only" = 1 ]; then
    echo "not queued (--keep-only): a lane started with $short reads it from $RECEIPTS/$full.json"
    exit 0
  fi
fi
mkdir -p "$QUEUE"
if [ -e "$QUEUE/$full" ]; then echo "enqueue: $short is already queued"; exit 0; fi
touch "$QUEUE/$full"
echo "queued $short → bash $SELF_DIR/status.sh --wait $short"
if [ "${RELEASE_DRY_RUN:-0}" != 1 ] && ! launchctl print "gui/$(id -u)/$LAUNCHD_LABEL" > /dev/null 2>&1; then
  echo "warning: $LAUNCHD_LABEL is not loaded — nothing will pick this up until: bash $SELF_DIR/install-launchd.sh"
fi
