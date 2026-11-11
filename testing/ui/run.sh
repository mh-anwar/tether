#!/usr/bin/env bash
# Starts an isolated Tether (own config/data/socket) + the window on a virtual screen + the fake phone.
# Usage: testing/ui/run.sh start [dark] | stop | accept
# Then: testing/ui/shot.py / drive.py against display :57. Screenshots go wherever you point shot.py.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
# Worktrees share the main checkout's target/ through CARGO_TARGET_DIR.
BIN=${CARGO_TARGET_DIR:-$ROOT/target}/debug
# Keep this path short: Unix socket paths are limited to ~100 characters.
W=${TETHER_UI_DIR:-/tmp/tether-ui-$USER}
DISPLAY_NUM=:57
export XDG_CONFIG_HOME=$W/cfg XDG_DATA_HOME=$W/data XDG_RUNTIME_DIR=$W/run
case "${1:-}" in
  start)
    mkdir -p "$W"/cfg/tether "$W"/data "$W"/run && chmod 700 "$W"/run
    [ -f "$W/cfg/tether/config.json" ] || echo "{\"device_name\":\"Workstation\",\"mirror_notifications\":false,\"downloads_dir\":\"$W/dl\"}" > "$W/cfg/tether/config.json"
    pgrep -f "Xvfb $DISPLAY_NUM" >/dev/null || { Xvfb $DISPLAY_NUM -screen 0 1400x900x24 -nolisten tcp >/dev/null 2>&1 & echo $! > "$W/xvfb.pid"; sleep 1; }
    theme=""; [ "${2:-}" = dark ] && theme="Adwaita:dark"
    DISPLAY=$DISPLAY_NUM GTK_THEME=$theme dbus-run-session -- "$BIN/tether-app" > "$W/app.log" 2>&1 &
    sleep 2; pgrep -n -x tether-app > "$W/app.pid"
    "$BIN/examples/fake_phone" 127.0.0.1:1716 "$W/phone" > "$W/phone.log" 2>&1 & echo $! > "$W/phone.pid"
    echo "Started. First run: '$0 accept' to pair the fake phone." ;;
  accept) "$BIN/tether" accept ;;
  stop)
    # The app outlives its dbus-run-session wrapper, so stop each recorded pid, then the daemon.
    for f in app phone xvfb; do [ -f "$W/$f.pid" ] && kill "$(cat "$W/$f.pid")" 2>/dev/null || true; done
    for p in $(pgrep -x tether || true); do { tr "\0" "\n" < /proc/$p/environ | grep -q "XDG_DATA_HOME=$W/data" && kill "$p"; } 2>/dev/null || true; done
    echo "Stopped." ;;
  *) echo "usage: $0 start [dark] | accept | stop"; exit 2 ;;
esac
