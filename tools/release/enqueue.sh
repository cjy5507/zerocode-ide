#!/bin/bash
# tools/release/enqueue.sh <sha> — put a sha on the release lane's queue.
#
# The lane (launchd dev.zerocode.release, QueueDirectories on the queue dir)
# picks it up oldest-first. Watch it with tools/release/status.sh [--wait <sha>].
# The old ~/.local/share/zerocode/release/release*.sh forward here.
set -u
SELF_DIR=$(cd "$(dirname "$0")" && pwd)
eval "$(bash "$SELF_DIR/lane.sh" --table)"
sha=${1:-}
case $sha in
  ""|-h|--help) echo "usage: enqueue.sh <sha>"; exit 2 ;;
  *[!0-9a-fA-F]*) echo "enqueue: '$sha' is not a sha (use a commit id, not a ref)"; exit 2 ;;
esac
[ ${#sha} -ge 7 ] || { echo "enqueue: '$sha' is too short for a sha"; exit 2; }
if [ "${RELEASE_DRY_RUN:-0}" = 1 ]; then full=$sha
else
  full=$(git -C "$RELEASE_REPO" rev-parse --verify --quiet "$sha^{commit}" 2>/dev/null) \
    || { echo "enqueue: $sha is unknown to $RELEASE_REPO (fetch it there first)"; exit 1; }
fi
mkdir -p "$QUEUE"
if [ -e "$QUEUE/$full" ]; then echo "enqueue: $(printf '%s' "$full" | cut -c1-8) is already queued"; exit 0; fi
touch "$QUEUE/$full"
short=$(printf '%s' "$full" | cut -c1-8)
echo "queued $short → bash $SELF_DIR/status.sh --wait $short"
if [ "${RELEASE_DRY_RUN:-0}" != 1 ] && ! launchctl print "gui/$(id -u)/$LAUNCHD_LABEL" > /dev/null 2>&1; then
  echo "warning: $LAUNCHD_LABEL is not loaded — nothing will pick this up until: bash $SELF_DIR/install-launchd.sh"
fi
