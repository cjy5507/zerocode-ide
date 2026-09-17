#!/bin/bash
# tools/release/install-launchd.sh [--render] [--label-suffix <s>] [--home <dir>] [--uninstall] [--forwarders]
#
# Installs the user launchd agent that runs the lane whenever the queue
# directory is not empty. Idempotent: an unchanged plist that is already
# loaded is left alone; a changed one is rewritten (temp + mv) and reloaded.
#
#   --render          print the plist and stop (the tests lint this)
#   --label-suffix s  dev.zerocode.release.<s> — for an isolated self-test
#   --home dir        RELEASE_HOME the agent should use (default: the table's)
#   --uninstall       bootout and remove the plist
#   --env K=V         extra EnvironmentVariables (repeatable; the self-test passes RELEASE_DRY_RUN=1)
#   --forwarders      also turn ~/.local/share/zerocode/release/release*.sh into
#                     thin forwarders to enqueue.sh (temp + mv, never deleted)
set -u
SELF_DIR=$(cd "$(dirname "$0")" && pwd)
eval "$(bash "$SELF_DIR/lane.sh" --table)"
render=0; suffix=; uninstall=0; forwarders=0; extra_env=
while [ $# -gt 0 ]; do
  case $1 in
    --render) render=1; shift ;;
    --env) case ${2:?} in *=*) ;; *) echo "install-launchd: --env wants K=V"; exit 2;; esac
           extra_env="$extra_env
    <key>${2%%=*}</key><string>${2#*=}</string>"; shift 2 ;;
    --label-suffix) suffix=${2:?}; shift 2 ;;
    --home) RELEASE_HOME=${2:?}; shift 2 ;;
    --uninstall) uninstall=1; shift ;;
    --forwarders) forwarders=1; shift ;;
    -h|--help) sed -n '2,15p' "$0"; exit 2 ;;
    *) echo "install-launchd: unknown argument $1"; exit 2 ;;
  esac
done
label=$LAUNCHD_LABEL${suffix:+.$suffix}
plist=$HOME/Library/LaunchAgents/$label.plist
domain=gui/$(id -u)
queue=$RELEASE_HOME/queue
out=$RELEASE_HOME/out

# ProcessType Interactive: with no ProcessType launchd applies "light resource
# limits, throttling its CPU usage and I/O bandwidth" (launchd.plist(5)) — the
# lane's rustc ran at priority 20 against a terminal's 31, and the zo gate's
# test compile took 2–4× as long (t-4004). Interactive is a terminal's class,
# no more; the lane still waits for calm (CALM_LOAD) before each gate.
render_plist() {
  cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>$SELF_DIR/lane.sh</string>
  </array>
  <key>QueueDirectories</key>
  <array><string>$queue</string></array>
  <key>ThrottleInterval</key><integer>$THROTTLE_SECS</integer>
  <key>ExitTimeOut</key><integer>60</integer>
  <key>ProcessType</key><string>Interactive</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>$LAUNCHD_PATH</string>
    <key>HOME</key><string>$HOME</string>
    <key>RELEASE_HOME</key><string>$RELEASE_HOME</string>
    <key>RELEASE_REPO</key><string>$RELEASE_REPO</string>$extra_env
  </dict>
  <key>StandardOutPath</key><string>$out/launchd.log</string>
  <key>StandardErrorPath</key><string>$out/launchd.log</string>
</dict>
</plist>
EOF
}
loaded() { launchctl print "$domain/$label" > /dev/null 2>&1; }

if [ $render = 1 ]; then render_plist; exit 0; fi
if [ $uninstall = 1 ]; then
  loaded && launchctl bootout "$domain/$label"
  rm -f "$plist"; echo "$label: unloaded, $plist removed"; exit 0
fi

mkdir -p "$queue" "$out" "$(dirname "$plist")"
rm -f "$RELEASE_HOME/current-release-sha"   # the old marker nobody read (design §2.3)
render_plist > "$plist.tmp"
if [ -f "$plist" ] && cmp -s "$plist.tmp" "$plist" && loaded; then
  rm -f "$plist.tmp"; echo "$label: already installed and loaded ($plist)"
else
  plutil -lint "$plist.tmp" > /dev/null || { rm -f "$plist.tmp"; echo "install-launchd: rendered plist does not lint"; exit 1; }
  mv -f "$plist.tmp" "$plist"
  loaded && launchctl bootout "$domain/$label"
  launchctl bootstrap "$domain" "$plist" || { echo "install-launchd: bootstrap failed"; exit 1; }
  echo "$label: loaded ($plist) — queue $queue"
fi

if [ $forwarders = 1 ]; then
  old=$HOME/.local/share/zerocode/release
  mkdir -p "$old"
  for name in release.sh release-judge.sh release-tail.sh; do
    if pgrep -f "$old/$name" > /dev/null 2>&1; then echo "forwarders: $name is running right now — left as is, rerun later"; continue; fi
    {
      echo '#!/bin/bash'
      echo "# $name — the old lane moved into the repo (docs/design/release-lane-off-the-window.md);"
      echo "# this file only forwards to the queue so the old habit keeps working."
      echo "exec \"$SELF_DIR/enqueue.sh\" \"\$@\""
    } > "$old/.$name.new" && chmod +x "$old/.$name.new" && mv -f "$old/.$name.new" "$old/$name" && echo "forwarders: $old/$name → enqueue.sh"
  done
fi
