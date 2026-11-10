#!/usr/bin/env bash
# tools/release/lane-env.sh <command…> — run a command the way the release lane's
# launchd job runs its gates, from any checkout (2026-11-10).
#
#   just lane-env shell-test                  # a recipe, as the lane's gate runs it
#   just lane-env test -p zerocode-core       # its arguments go along
#   tools/release/lane-env.sh <command…>      # any command
#
# A test that passes in a terminal and fails only in the lane is most often
# reading something the lane does not have:
#   · the environment — launchd gives the job HOME and PATH (lane.sh's
#     LAUNCHD_PATH, `lane.sh --table`) and nothing a terminal carries: no TERM,
#     COLORTERM, LANG, TMPDIR, SSH_*, RUST_* or the others below. 2026-11-10: zo's diff band test
#     drew no colour band where COLORTERM was unset (a CI runner), green in every
#     pane here;
#   · a terminal — the job has no controlling terminal (/dev/tty does not open)
#     and reads /dev/null;
#   · open files — launchd's soft limit, read from `launchctl limit maxfiles`
#     (256 on this Mac).
# Kept when set, because the lane sets each to a value of its own and a
# checkout's are the equivalent: CARGO_TARGET_DIR (where cargo builds), NODE_PATH
# (where node finds Playwright) and CARGO_INCREMENTAL (the lane's is 0; it
# changes only what cargo caches, so a checkout keeps its own rather than
# rebuilding its crates once for nothing a test sees).
#
# The exit code is the command's (128+N when a signal ended it). A Ctrl-C or a
# TERM sent here reaches the command's whole process group: it runs in a session
# of its own, where the terminal's signals no longer reach it.
set -u
if [ $# -eq 0 ]; then
  echo "lane-env: name a command — just lane-env <recipe> [args…], or tools/release/lane-env.sh <command…>" >&2
  exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
lane_path=$(bash "$here/lane.sh" --table | sed -n "s/^LAUNCHD_PATH='\(.*\)'$/\1/p")
if [ -z "$lane_path" ]; then
  echo "lane-env: lane.sh --table printed no LAUNCHD_PATH" >&2
  exit 2
fi
files=$(launchctl limit maxfiles 2>/dev/null | awk '{ print $2 }')
case $files in ''|*[!0-9]*) files=256 ;; esac
keep=(HOME="$HOME" PATH="$lane_path")
[ -z "${CARGO_TARGET_DIR-}" ] || keep+=(CARGO_TARGET_DIR="$CARGO_TARGET_DIR")
[ -z "${NODE_PATH-}" ] || keep+=(NODE_PATH="$NODE_PATH")
[ -z "${CARGO_INCREMENTAL-}" ] || keep+=(CARGO_INCREMENTAL="$CARGO_INCREMENTAL")
# Python only for setsid and setrlimit. Whatever its launcher adds to its own
# environment (Apple's /usr/bin/python3 is an xcrun shim that sets SDKROOT and
# CPATH) stays out: the command gets exactly the names given here.
exec env -i "${keep[@]}" python3 -I -c '
import os, resource, signal, sys
files, command = int(sys.argv[1]), sys.argv[2:]
names = ("HOME", "PATH", "CARGO_TARGET_DIR", "NODE_PATH", "CARGO_INCREMENTAL")
environment = {name: os.environ[name] for name in names if name in os.environ}
child = os.fork()
if child == 0:
    os.setsid()
    hard = resource.getrlimit(resource.RLIMIT_NOFILE)[1]
    resource.setrlimit(resource.RLIMIT_NOFILE, (min(files, hard), hard))
    null = os.open(os.devnull, os.O_RDONLY)
    os.dup2(null, 0)
    try:
        os.execvpe(command[0], command, environment)
    except OSError as error:
        print(f"lane-env: {command[0]}: {error.strerror}", file=sys.stderr)
        os._exit(127)
def forward(signum, frame):
    try:
        os.killpg(child, signum)
    except ProcessLookupError:
        pass
for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
    signal.signal(signum, forward)
while True:
    try:
        _, status = os.waitpid(child, 0)
        break
    except InterruptedError:
        continue
sys.exit(os.waitstatus_to_exitcode(status) if os.WIFEXITED(status) else 128 + os.WTERMSIG(status))
' "$files" "$@"
