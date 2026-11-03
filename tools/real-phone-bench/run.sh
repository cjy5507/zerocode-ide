#!/bin/zsh
# run.sh [out.json] — the bench of the road that drives a real iPhone from inside (t-90894), end to end on
# iOS simulators, so it repeats on any Mac with Xcode and touches nobody's phone.
#
# It fetches the pinned WebDriverAgent (the tag, commit and tree hashes of
# crates/zerocode-core/src/agent_emulator/real_phone.rs), applies this repository's one patch, builds it for the
# simulator once (kept in $ZC_PHONE_BENCH_DIR), makes two simulators of its own, and runs the ignored test
# `emulator::wda::bench::the_bench`: speed (p50/p90 of the lock check, tree, marks, find, tap and typing),
# accuracy (the message errand 20 times: open Messages, choose a conversation, type Korean, send, and find the
# words in the conversation), a stale number after a screen change, and two devices and a desktop agent at once,
# with the Mac's front app read before and after. The simulators it made are deleted at the end.
set -eu
here=${0:A:h}
repo=${here:h:h}
out=${1:-$PWD/real-phone-bench.json}
dir=${ZC_PHONE_BENCH_DIR:-$HOME/Library/Caches/zerocode-real-phone-bench}
core=$repo/crates/zerocode-core/src/agent_emulator/real_phone.rs
pin() { sed -n "s/^pub const $1: &str = \"\\(.*\\)\";/\\1/p" $core }
tag=$(pin WDA_TAG); commit=$(pin WDA_COMMIT); tree=$(pin WDA_TREE)
mkdir -p $dir
if [ ! -d $dir/build/Build/Products ]; then
  rm -rf $dir/src && mkdir -p $dir/src && cd $dir/src
  g() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -c core.hooksPath=/dev/null -c transfer.fsckObjects=true "$@" }
  g init -q
  listed=$(g ls-remote https://github.com/appium/WebDriverAgent.git refs/tags/$tag | cut -f1)
  g fetch -q --depth 1 https://github.com/appium/WebDriverAgent.git $commit
  [ "$listed" = "$commit" ] && [ "$(g rev-parse FETCH_HEAD)" = "$commit" ] && [ "$(g rev-parse 'FETCH_HEAD^{tree}')" = "$tree" ] \
    || { echo "the fetched source is not the pinned one; nothing built" >&2; exit 2; }
  g checkout -q FETCH_HEAD
  g apply --check $repo/crates/zerocode-shell/src/emulator/wda/tap-no-wait.patch || { echo "the patch does not fit; nothing built" >&2; exit 3; }
  g apply $repo/crates/zerocode-shell/src/emulator/wda/tap-no-wait.patch
  taskpolicy -c utility xcodebuild build-for-testing -project WebDriverAgent.xcodeproj -scheme WebDriverAgentRunner \
    -destination 'generic/platform=iOS Simulator' -derivedDataPath $dir/build >$dir/build.log 2>&1
fi
plan=$(ls $dir/build/Build/Products/*.xctestrun | head -1)
model=${ZC_PHONE_BENCH_MODEL:-iPhone 17}
one=$(xcrun simctl create zc-phone-bench-a "$model"); two=$(xcrun simctl create zc-phone-bench-b "$model")
cleanup() { for d in $one $two; do xcrun simctl shutdown $d 2>/dev/null; xcrun simctl delete $d 2>/dev/null; done; true }
trap cleanup EXIT
xcrun simctl boot $one; xcrun simctl boot $two
xcrun simctl bootstatus $one -b >/dev/null; xcrun simctl bootstatus $two -b >/dev/null
cd $repo
ZC_PHONE_BENCH_SIM=$one ZC_PHONE_BENCH_SIM2=$two ZC_PHONE_BENCH_PLAN=$plan ZC_PHONE_BENCH_OUT=$out \
  ${ZC_PHONE_BENCH_EXE:-cargo test -p zerocode-shell --bin zerocode-shell --} emulator::wda::bench::the_bench --ignored --exact --nocapture
echo "report: $out"
