#!/usr/bin/env bash
# The zerocode-shell test suite twice, at the same time, both expected green.
#
# This is the hermeticity gate for the shell crate's orchestration scenarios:
# two worktrees running `cargo test -p zerocode-shell` at once must not make
# each other red. The crate is built ONCE (through the machine's shared build
# slot when it has one) and its test executables are then started twice, side
# by side — two `cargo test` invocations would not do: cargo holds the
# artifact-directory lock for the whole of a test run, so a second invocation
# waits for the first and the two never overlap. Each run lands in its own
# transcript and the summary says which run failed, on what, and whether the
# two actually ran at the same time.
#
#   scripts/test-shell-concurrent.sh                  # the whole crate, twice
#   scripts/test-shell-concurrent.sh orchestration::  # one family, twice
#
# Positional arguments go to every test executable as its own arguments — a
# name filter, `--test-threads N`, `--nocapture` and the rest of libtest's.
#
# Environment:
#   ZEROCODE_BUILD_RUNNER        a command prefix for the build step (defaults
#                                to build-coordination/run.py when it exists)
#   ZEROCODE_CONCURRENT_LOG_DIR  where the transcripts land (mktemp -d)
set -u

cd "$(dirname "$0")/.." || exit 2
repo="$PWD"
crate="$repo/crates/zerocode-shell"

# The build slot sets these; the build here must see the SAME profile or a
# later plain `cargo test` rebuilds the crate under a different fingerprint.
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0

coordination="$HOME/Library/Caches/dev.zerocode.app/build-coordination/run.py"
runner=()
if [ -n "${ZEROCODE_BUILD_RUNNER:-}" ]; then
  # shellcheck disable=SC2206 # a prefix, split on purpose
  runner=($ZEROCODE_BUILD_RUNNER)
elif [ -f "$coordination" ]; then
  runner=(python3 "$coordination")
fi

logs="${ZEROCODE_CONCURRENT_LOG_DIR:-$(mktemp -d -t zerocode-shell-concurrent)}"
mkdir -p "$logs" || exit 2
echo "transcripts: $logs"

echo "== build once =="
"${runner[@]}" cargo test -p zerocode-shell --no-run \
  --message-format=json-render-diagnostics >"$logs/build.jsonl"
built=$?
if [ "$built" -ne 0 ]; then
  echo "build failed (exit $built); see $logs/build.jsonl"
  exit "$built"
fi
# The build slot chats on stdout too; only the JSON lines name executables,
# and only the test-profile ones are harnesses — the crate's own binaries are
# built alongside and must not be started by a test script.
executables=()
while IFS= read -r exe; do
  [ -n "$exe" ] && executables+=("$exe")
done < <(jq -R 'fromjson? | select(.reason == "compiler-artifact" and .profile.test == true and .executable != null) | .executable' -r "$logs/build.jsonl")
if [ "${#executables[@]}" -eq 0 ]; then
  echo "no test executables were built; see $logs/build.jsonl"
  exit 2
fi
echo "test executables: ${#executables[@]}"

run_one() {
  local name="$1"
  shift
  {
    echo "start $(date '+%H:%M:%S')"
    local code=0
    local exe
    for exe in "${executables[@]}"; do
      echo "     Running $exe"
      # cargo runs a package's tests from its manifest directory.
      (cd "$crate" && CARGO_MANIFEST_DIR="$crate" "$exe" "$@")
      local one=$?
      if [ "$one" -ne 0 ]; then
        code=$one
      fi
    done
    echo "exit $code at $(date '+%H:%M:%S')"
  } >"$logs/$name.log" 2>&1
}

echo "== two runs at once =="
run_one a "$@" &
pid_a=$!
run_one b "$@" &
pid_b=$!
wait "$pid_a"
wait "$pid_b"

verdict=0
for name in a b; do
  log="$logs/$name.log"
  code="$(sed -n 's/^exit \([0-9]*\) at .*/\1/p' "$log" | tail -n 1)"
  [ -n "$code" ] || code=125
  echo "-- run $name: exit $code ($log)"
  sed -n 's/^start /   started /p; s/^exit [0-9]* at /   ended   /p' "$log"
  grep -E '^test result:' "$log" | sed 's/^/   /'
  failed="$(sed -n 's/^    \([A-Za-z0-9_:]*\)$/\1/p' "$log" | sort -u)"
  if [ -n "$failed" ]; then
    echo "   failed tests:"
    echo "$failed" | sed 's/^/     /'
  fi
  if [ "$code" -ne 0 ]; then
    verdict=1
  fi
done

if [ "$verdict" -eq 0 ]; then
  echo "== both green =="
else
  echo "== red: at least one run failed =="
fi
exit "$verdict"
