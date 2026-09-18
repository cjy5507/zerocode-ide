#!/bin/bash
# tools/release/bump.sh {patch|minor|major} [--repo <dir>] | --table
#
# The one hand that raises the version (docs/design/versioned-auto-update.md
# §2.1). The truth is the root Cargo.toml `[workspace.package] version`; the
# window's tauri.conf.json and zo-ide/Cargo.toml must say the same letters
# (source contract `the_three_versions_agree`), so this script moves all three
# together and opens a `## [x.y.z] — YYYY-MM-DD` section at the top of
# CHANGELOG.md listing the commit titles since the last `vX.Y.Z` tag by
# prefix, for a person to trim. It refuses (rc 3) when the three files already
# disagree, and when the tag `v<next>` exists — locally, or on the release
# repo asked through `gh api` (cjy5507/zerocode carries an older CLI's
# v1.2.3–v1.2.7; §2.2 coexistence rule). It neither commits nor tags: the
# person reads the section, then commits the six release files BY PATH —
# never `-am`: a checkout shared with another session's uncommitted work
# would sweep that work into the release (2026-09-16, v1.3.87's first cut).
#
# rc: 0 bumped · 1 write/lockfile update failed · 2 usage · 3 refused (versions disagree, tag exists, probe failed)
set -u

# ---------------------------------------------------------------- the table --
ROOT_CARGO=Cargo.toml                             # the truth: [workspace.package] version
TAURI_CONF=crates/zerocode-shell/tauri.conf.json  # top-level "version"
ZO_CARGO=zo-ide/Cargo.toml                        # [workspace.package] version
CHANGELOG=CHANGELOG.md
CHANGELOG_TITLE="# Changelog"
TAG_GLOB='v[0-9]*'                                # the tags that bound a section (vX.Y.Z, never `beta`)
MAX_PER_PREFIX=20                                 # titles listed per prefix; the rest is counted
PREFIX_ORDER="feat fix perf refactor docs test style chore release merge other"
# -------------------------------------------------------------------------------

SELF_DIR=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$SELF_DIR/../.." && pwd)

# The repository the tags live on is lane.sh's table to name — asked for here
# rather than spelled a second time, so the two hands of one release can never
# disagree about where it goes. A written name asks the wrong repository about
# its tags after a rename, and the refusal then reads as "v1.1.0 already
# exists" about a tag on a repo this history never had (zerocode ->
# zerocode-ide, 2026-09-18). `RELEASE_GITHUB_REPO` in the environment still
# wins, for a release cut against somewhere else on purpose.
release_github_repo() { # DIR -> owner/name
  RELEASE_REPO=$1 bash "$SELF_DIR/lane.sh" --table |
    sed -n "s/^RELEASE_GITHUB_REPO='\(.*\)'$/\1/p"
}

if [ "${1:-}" = "--table" ]; then
  RELEASE_GITHUB_REPO=${RELEASE_GITHUB_REPO:-$(release_github_repo "$REPO")}
  for k in ROOT_CARGO TAURI_CONF ZO_CARGO CHANGELOG CHANGELOG_TITLE TAG_GLOB MAX_PER_PREFIX PREFIX_ORDER RELEASE_GITHUB_REPO; do
    eval "printf \"%s='%s'\\n\" \"$k\" \"\$$k\""
  done
  exit 0
fi

usage() { echo "usage: bump.sh {patch|minor|major} [--repo <dir>]"; exit 2; }
part=
while [ $# -gt 0 ]; do
  case $1 in
    patch|minor|major) [ -z "$part" ] || usage; part=$1; shift ;;
    --repo) REPO=$(cd "${2:-}" 2>/dev/null && pwd) || { echo "bump: no such repo ${2:-}"; exit 2; }; shift 2 ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done
[ -n "$part" ] || usage
cd "$REPO" || exit 2

RELEASE_GITHUB_REPO=${RELEASE_GITHUB_REPO:-$(release_github_repo "$REPO")}

refuse() { echo "bump: refused — $1"; exit 3; }

# ------------------------------------------------------------- the versions --
# The version line under [workspace.package] — that section's own, not a
# dependency's `version = "…"` further down.
workspace_version() { # FILE
  awk 'BEGIN{s=0} /^\[/{s=($0=="[workspace.package]")} s && /^version[[:space:]]*=/{ split($0,q,"\""); print q[2]; exit }' "$1"
}
tauri_version() { sed -n 's/^  "version": "\([^"]*\)".*/\1/p' "$1" | head -1; }
set_workspace_version() { # FILE VERSION
  awk -v v="$2" 'BEGIN{s=0} /^\[/{s=($0=="[workspace.package]")} s && /^version[[:space:]]*=/{ $0="version = \"" v "\"" } {print}' "$1" > "$1.tmp" \
    && mv -f "$1.tmp" "$1"
}
set_tauri_version() { # FILE VERSION
  sed "s/^\(  \"version\": \"\)[^\"]*\(\".*\)$/\1$2\2/" "$1" > "$1.tmp" && mv -f "$1.tmp" "$1"
}

for f in "$ROOT_CARGO" "$TAURI_CONF" "$ZO_CARGO"; do [ -f "$f" ] || { echo "bump: $REPO/$f is missing"; exit 2; }; done
root_v=$(workspace_version "$ROOT_CARGO"); tauri_v=$(tauri_version "$TAURI_CONF"); zo_v=$(workspace_version "$ZO_CARGO")
[ -n "$root_v" ] || refuse "no [workspace.package] version in $ROOT_CARGO"
if [ "$root_v" != "$tauri_v" ] || [ "$root_v" != "$zo_v" ]; then
  refuse "the three versions disagree: $ROOT_CARGO=$root_v $TAURI_CONF=$tauri_v $ZO_CARGO=$zo_v"
fi
case $root_v in *.*.*) ;; *) refuse "$root_v is not a semver triple";; esac
IFS=. read -r major minor patch <<EOF
$root_v
EOF
case $part in
  patch) patch=$(( patch + 1 )) ;;
  minor) minor=$(( minor + 1 )); patch=0 ;;
  major) major=$(( major + 1 )); minor=0; patch=0 ;;
esac
next="$major.$minor.$patch"
tag="v$next"

# ------------------------------------------------------------------ the tag --
[ -z "$(git tag -l "$tag")" ] || refuse "tag $tag already exists here"
# A checkout whose `origin` names no GitHub repository cannot be asked about
# the tag: `repos//git/ref/tags/...` is a 404 for every tag there is, and that
# reads back as "the tag is free" — which is how a duplicate gets cut.
[ -n "$RELEASE_GITHUB_REPO" ] \
  || refuse "origin names no GitHub repository in $REPO — set RELEASE_GITHUB_REPO to name the one $tag goes on"
if command -v gh > /dev/null 2>&1; then
  probe=$(gh api "repos/$RELEASE_GITHUB_REPO/git/ref/tags/$tag" 2>&1); rc=$?
  if [ "$rc" = 0 ]; then refuse "tag $tag already exists on $RELEASE_GITHUB_REPO"
  else case $probe in *"HTTP 404"*) ;; *) refuse "could not ask $RELEASE_GITHUB_REPO about $tag: $probe";; esac
  fi
else
  echo "bump: gh not found — $RELEASE_GITHUB_REPO was not asked about $tag"
fi

# ------------------------------------------------------------- the section --
last=$(git describe --tags --abbrev=0 --match "$TAG_GLOB" HEAD 2>/dev/null)
if [ -n "$last" ]; then range="$last..HEAD"; since="_since $last"
else range=HEAD; since="_first section: no earlier vX.Y.Z tag, every commit so far"; fi
count=$(git rev-list --count $range)
since="$since ($count commits)_"
today=$(date +%F)
tmp=$(mktemp "${TMPDIR:-/tmp}/bump.XXXXXX"); titles="$tmp.titles"; section="$tmp.section"
trap 'rm -f "$tmp" "$tmp".*' EXIT
# "<prefix>\t<title>" newest first; a title with no `word:` / `word(scope):` /
# `word+` head is `other`. The first word of a compound `test(x)+docs(y):` counts.
git log --format=%s $range \
  | awk '{ if (match($0, /^[a-z]+(\([^)]*\))?[:+]/)) { p = $0; sub(/[(:+].*$/, "", p); print p "\t" $0 } else print "other\t" $0 }' > "$titles"
{
  printf '## [%s] — %s\n\n%s\n' "$next" "$today" "$since"
  # Prefixes the table does not name come just before `other`, sorted.
  extras=$(cut -f1 "$titles" | sort -u | awk -v order=" $PREFIX_ORDER " 'index(order, " " $0 " ") == 0')
  for p in $PREFIX_ORDER; do
    if [ "$p" = other ]; then plist="$extras other"; else plist=$p; fi
    for q in $plist; do
      n=$(awk -F'\t' -v p="$q" '$1==p' "$titles" | wc -l | tr -d ' ')
      [ "$n" -gt 0 ] || continue
      printf '\n### %s\n' "$q"
      awk -F'\t' -v p="$q" -v cap="$MAX_PER_PREFIX" '$1==p && k<cap { k++; sub(/^[^\t]*\t/, ""); print "- " $0 }' "$titles"
      [ "$n" -le "$MAX_PER_PREFIX" ] || printf -- '- … %d more %s commits\n' $(( n - MAX_PER_PREFIX )) "$q"
    done
  done
} > "$section"

# ------------------------------------------------------------------ write --
set_workspace_version "$ROOT_CARGO" "$next" || exit 1
set_tauri_version "$TAURI_CONF" "$next" || exit 1
set_workspace_version "$ZO_CARGO" "$next" || exit 1
# Derive workspace directories from the version-file table, so both lockfiles
# follow the new package version without fetching or upgrading dependencies.
if command -v cargo > /dev/null 2>&1; then
  for manifest in "$ROOT_CARGO" "$ZO_CARGO"; do
    workspace=$(dirname "$manifest")
    (cd "$workspace" && cargo update --workspace --offline) || {
      echo "bump: lockfile update failed in $workspace; version files are already $next — fix the lockfile before committing" >&2
      exit 1
    }
  done
else
  echo "bump: cargo not found — skipping workspace lockfiles for $ROOT_CARGO and $ZO_CARGO" >&2
fi
if [ -f "$CHANGELOG" ]; then
  { head -n 1 "$CHANGELOG"; printf '\n'; cat "$section"; printf '\n'; tail -n +2 "$CHANGELOG" | sed '/./,$!d'; } > "$CHANGELOG.tmp"
else
  { printf '%s\n\n' "$CHANGELOG_TITLE"; cat "$section"; } > "$CHANGELOG.tmp"
fi
mv -f "$CHANGELOG.tmp" "$CHANGELOG"
echo "$next"
echo "bump: $root_v → $next in $ROOT_CARGO $TAURI_CONF $ZO_CARGO; $CHANGELOG opens [$next] with $count commits"
echo "bump: next — read the section, then commit the release files by path (never -am, another session's edits may sit in this checkout):"
echo "  git add Cargo.toml crates/zerocode-shell/tauri.conf.json zo-ide/Cargo.toml Cargo.lock zo-ide/Cargo.lock CHANGELOG.md && git commit -m \"release: $tag\" && git tag $tag"
exit 0
