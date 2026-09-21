#!/bin/bash
# tools/release/publish-source.sh vX.Y.Z [--replace-history] [--dry-run] [--public-remote <url|dir>]
#
# The public source repository carries releases only: one squashed commit per
# release, each the tree of the private repository's release tag with the
# design notes (`docs/`, `zo-ide/docs/`) left out, judged clean by the
# personal-data scan, and parented on the previous public release commit —
# never a development commit, never the private history (the person's order,
# 2026-09-21: "깃 히스토리도 릴리즈만 올리고 기존 거 다 삭제").
#
#   --replace-history   the first snapshot: start an orphan lineage and force
#                       the public `main` over whatever stood there (backed up
#                       first — see docs/design/public-source-repository-20260921.md)
#   --dry-run           build the snapshot and print what would be pushed, push nothing
#   --public-remote     where the public repository is (default: the release
#                       repository the updater feed names, cjy5507/zerocode)
#
# rc: 0 published (or dry run complete) · 2 usage · 3 refused (tag missing,
#     scan red, public main is not a snapshot lineage and --replace-history not given)
set -u

usage() { sed -n 2,17p "$0" | sed 's/^# \{0,1\}//'; }

TAG=; REPLACE=0; DRY=0
PUBLIC_REMOTE=${PUBLIC_SOURCE_REMOTE:-https://github.com/cjy5507/zerocode.git}
PUBLIC_BRANCH=${PUBLIC_SOURCE_BRANCH:-main}
# Folders the public tree never carries. One table; the snapshot and the
# refusal message read the same list.
EXCLUDED_DIRS=(docs zo-ide/docs)
SNAPSHOT_TRAILER="Public-Snapshot-Of"
while [ $# -gt 0 ]; do
  case "$1" in
    --replace-history) REPLACE=1 ;;
    --dry-run) DRY=1 ;;
    --public-remote) shift; PUBLIC_REMOTE=${1:-} ;;
    -h|--help) usage; exit 0 ;;
    v[0-9]*) TAG=$1 ;;
    *) echo "publish-source: unknown argument '$1'" >&2; usage >&2; exit 2 ;;
  esac
  shift
done
[ -n "$TAG" ] || { usage >&2; exit 2; }

REPO=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "publish-source: not inside the source repository" >&2; exit 2; }
cd "$REPO" || exit 2
git rev-parse -q --verify "refs/tags/$TAG^{commit}" >/dev/null || { echo "publish-source: no tag $TAG here" >&2; exit 3; }
SOURCE_SHA=$(git rev-parse "refs/tags/$TAG^{commit}")
VERSION=${TAG#v}

WORK=$(mktemp -d "${TMPDIR:-/tmp}/zerocode-public-source.XXXXXX") || exit 3
trap 'rm -rf "$WORK"' EXIT
TREE=$WORK/tree
mkdir -p "$TREE"

# ------------------------------------------------------------ the snapshot --
# `git archive` hands over exactly the tag's tracked tree — no untracked file
# of this checkout, no build output — and the excluded folders are cut from it.
git archive --format=tar "$SOURCE_SHA" | tar -x -C "$TREE" || { echo "publish-source: archive of $TAG failed" >&2; exit 3; }
for dir in "${EXCLUDED_DIRS[@]}"; do rm -rf "${TREE:?}/$dir"; done

# ------------------------------------------------------- the public lineage --
PUB=$WORK/public
git init -q "$PUB"
git -C "$PUB" remote add origin "$PUBLIC_REMOTE"
PARENT=
if git -C "$PUB" fetch -q origin "$PUBLIC_BRANCH" 2>/dev/null; then
  HEAD_SHA=$(git -C "$PUB" rev-parse FETCH_HEAD)
  if git -C "$PUB" log -1 --format=%B "$HEAD_SHA" | grep -q "^$SNAPSHOT_TRAILER:"; then
    PARENT=$HEAD_SHA
  elif [ "$REPLACE" != 1 ]; then
    echo "publish-source: refused — public $PUBLIC_BRANCH ($HEAD_SHA) is not a release snapshot lineage; pass --replace-history to start one over it (back it up first)" >&2
    exit 3
  fi
elif [ "$REPLACE" != 1 ]; then
  echo "publish-source: public $PUBLIC_BRANCH does not exist yet — pass --replace-history to start the lineage" >&2
  exit 3
fi

cp -R "$TREE"/. "$PUB"/
git -C "$PUB" add -A
# The personal-data scan is the gate the snapshot must clear, and it reads
# only what git tracks — so it runs over the public index just staged, which
# is exactly the set of files about to be pushed and nothing else. Until the
# scan lands (t-5781) its absence is said aloud, never skipped in silence.
if [ -f "$REPO/tools/release/pii-scan.py" ]; then
  python3 "$REPO/tools/release/pii-scan.py" --root "$PUB" || { echo "publish-source: refused — the personal-data scan is red on $TAG's tree" >&2; exit 3; }
else
  echo "publish-source: WARNING pii-scan.py is not here yet — the snapshot is unscanned" >&2
fi
NOTES=$(awk -v v="$VERSION" '/^## \[/{p=($0 ~ "\\[" v "\\]")} p' "$TREE/CHANGELOG.md" 2>/dev/null | head -80)
{
  echo "release: $TAG"
  echo
  [ -n "$NOTES" ] && printf '%s\n\n' "$NOTES"
  echo "$SNAPSHOT_TRAILER: $SOURCE_SHA"
} > "$WORK/message"
if [ -n "$PARENT" ]; then
  git -C "$PUB" update-ref HEAD "$PARENT"
  git -C "$PUB" reset -q --soft "$PARENT"
fi
git -C "$PUB" -c user.name="${GIT_AUTHOR_NAME:-zerocode release}" -c user.email="${GIT_AUTHOR_EMAIL:-release@zerocode.dev}" commit -q -F "$WORK/message" || { echo "publish-source: nothing to commit for $TAG" >&2; exit 3; }
SNAP=$(git -C "$PUB" rev-parse HEAD)
git -C "$PUB" tag -f "$TAG" "$SNAP"
FILES=$(git -C "$PUB" ls-files | wc -l | tr -d ' ')
echo "publish-source: $TAG → snapshot $SNAP ($FILES files, parent ${PARENT:-none (orphan)}) of private $SOURCE_SHA"

if [ "$DRY" = 1 ]; then
  echo "publish-source: dry run — would push $PUBLIC_BRANCH$( [ "$REPLACE" = 1 ] && printf ' (force)') and tag $TAG to $PUBLIC_REMOTE"
  exit 0
fi
if [ "$REPLACE" = 1 ]; then
  git -C "$PUB" push -q --force origin "HEAD:refs/heads/$PUBLIC_BRANCH" || exit 3
else
  git -C "$PUB" push -q origin "HEAD:refs/heads/$PUBLIC_BRANCH" || exit 3
fi
git -C "$PUB" push -q -f origin "refs/tags/$TAG" || exit 3
echo "publish-source: pushed $PUBLIC_BRANCH and $TAG"
