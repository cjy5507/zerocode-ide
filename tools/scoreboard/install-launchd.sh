#!/bin/bash
# tools/scoreboard/install-launchd.sh [--render] [--uninstall] [--hour H] [--minute M] [--label-suffix s]
#
# Installs the user launchd agent that runs beat.sh once a day (default 09:00
# local). Idempotent: an unchanged, loaded plist is left alone; a changed one
# is rewritten (temp + mv) and reloaded. `--render` prints the plist (the
# tests lint it). No zo session and no pane is needed for the clock to fire —
# that is the point (docs/design/scoreboard-beat-20260911.md §1).
set -u
SELF_DIR=$(cd "$(dirname "$0")" && pwd)
label=dev.zerocode.scoreboard
hour=9; minute=0; render=0; uninstall=0
while [ $# -gt 0 ]; do
  case $1 in
    --render) render=1; shift ;;
    --uninstall) uninstall=1; shift ;;
    --hour) hour=${2:?}; shift 2 ;;
    --minute) minute=${2:?}; shift 2 ;;
    --label-suffix) label="$label.${2:?}"; shift 2 ;;
    *) echo "install-launchd: unknown argument $1" >&2; exit 2 ;;
  esac
done
state=$HOME/.local/share/zerocode/scoreboard
plist=$HOME/Library/LaunchAgents/$label.plist
render_plist() {
  cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>$SELF_DIR/beat.sh</string>
  </array>
  <key>StartCalendarInterval</key>
  <dict>
    <key>Hour</key><integer>$hour</integer>
    <key>Minute</key><integer>$minute</integer>
  </dict>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
    <key>HOME</key><string>$HOME</string>
  </dict>
  <key>StandardOutPath</key><string>$state/launchd.log</string>
  <key>StandardErrorPath</key><string>$state/launchd.log</string>
</dict>
</plist>
PLIST
}
if [ "$render" = 1 ]; then render_plist; exit 0; fi
uid=$(id -u)
if [ "$uninstall" = 1 ]; then
  launchctl bootout "gui/$uid/$label" 2>/dev/null || :
  rm -f "$plist"; echo "uninstalled $label"; exit 0
fi
mkdir -p "$state" "$(dirname "$plist")"
tmp=$(mktemp "$plist.XXXXXX"); render_plist > "$tmp"
if [ -f "$plist" ] && cmp -s "$tmp" "$plist" && launchctl print "gui/$uid/$label" >/dev/null 2>&1; then
  rm -f "$tmp"; echo "unchanged $label (loaded)"; exit 0
fi
mv -f "$tmp" "$plist"
launchctl bootout "gui/$uid/$label" 2>/dev/null || :
launchctl bootstrap "gui/$uid" "$plist" || { echo "bootstrap failed for $label" >&2; exit 1; }
echo "installed $label → $SELF_DIR/beat.sh at $(printf '%02d:%02d' "$hour" "$minute") daily; log $state/beat.log"
