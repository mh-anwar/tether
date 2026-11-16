#!/usr/bin/env bash
# The XEmbed tray (i3bar, polybar) end to end on a virtual screen: the icon docks into trayhost.py,
# a right-click opens Tether's own menu, the keyboard picks "Ring my phone" and the fake phone gets
# the ring. Screenshots of the bar and the open menu go to $W/xembed-*.png; look at them.
# Usage: testing/ui/tray_xembed.sh
# Needs: debug builds of tether and the fake_phone example (see tray_e2e.sh).
# Env: TETHER_UI_DIR (default /tmp/tether-ui-$USER), TETHER_PORT (default 14740), WD_DISPLAY (default :58).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${CARGO_TARGET_DIR:-$ROOT/target}/debug
W=${TETHER_UI_DIR:-/tmp/tether-ui-$USER}/tray-x
PORT=${TETHER_PORT:-14740}
DISPLAY_NUM=${WD_DISPLAY:-:58}
rm -rf "$W" && mkdir -p "$W"/cfg/tether "$W"/data "$W"/run "$W"/fc && chmod 700 "$W"/run
echo '{"device_name":"Workstation","mirror_notifications":true,"clipboard_sync":false,"mdns":false}' > "$W/cfg/tether/config.json"
export XDG_CONFIG_HOME=$W/cfg XDG_DATA_HOME=$W/data XDG_RUNTIME_DIR=$W/run TETHER_PORT=$PORT
export XDG_CURRENT_DESKTOP=i3 XDG_SESSION_TYPE=x11 TOKIO_WORKER_THREADS=2 DISPLAY=$DISPLAY_NUM
unset WAYLAND_DISPLAY RUST_LOG

pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

Xvfb "$DISPLAY_NUM" -screen 0 1400x900x24 -nolisten tcp >/dev/null 2>&1 & pids+=($!)
sleep 1
# A session bus without a StatusNotifierWatcher, like i3.
dbus-daemon --session --nofork --print-address=3 3>"$W/bus" & pids+=($!)
for _ in $(seq 50); do [ -s "$W/bus" ] && break; sleep 0.1; done
export DBUS_SESSION_BUS_ADDRESS=$(head -1 "$W/bus")
uv run -q --no-project --with python-xlib python "$ROOT/testing/ui/trayhost.py" "$DISPLAY_NUM" > "$W/trayhost.log" 2>&1 & pids+=($!)
sleep 2
"$BIN/tether" daemon > "$W/daemon.log" 2>&1 & pids+=($!)
sleep 1
FAKE_COMPUTER_DIR=$W/fc "$BIN/examples/fake_phone" "127.0.0.1:$PORT" "$W/phone" > "$W/phone.log" 2>&1 & pids+=($!)
for _ in $(seq 50); do grep -q 'asked to pair' "$W/phone.log" && break; sleep 0.2; done
"$BIN/tether" accept >/dev/null

fail() { echo "FAIL $*"; exit 1; }
for _ in $(seq 100); do grep -q docked "$W/trayhost.log" && break; sleep 0.1; done
grep -q docked "$W/trayhost.log" || fail "the icon never docked"
echo "docked"
sleep 4  # the phone connects and its unread texts reach the badge
drive() { uv run -q --no-project --with python-xlib python "$ROOT/testing/ui/drive.py" "$DISPLAY_NUM" "$@"; }
shot() { uv run -q --no-project --with pillow python "$ROOT/testing/ui/shot.py" "$DISPLAY_NUM" "$1"; }
shot "$W/xembed-bar.png"
drive rclick 1230 30 sleep 0.5 key Down key Down
shot "$W/xembed-menu.png"
drive key Return
for _ in $(seq 50); do grep -q kdeconnect.findmyphone.request "$W/fc/received.jsonl" 2>/dev/null && break; sleep 0.1; done
grep -q kdeconnect.findmyphone.request "$W/fc/received.jsonl" || fail "Ring my phone didn't reach the phone"
echo "rang the phone from the menu"
# Escape and a click outside both close the menu without doing anything.
drive rclick 1230 30 sleep 0.5 key Escape rclick 1230 30 sleep 0.5 click 200 600
shot "$W/xembed-closed.png"
echo "PASS (look at $W/xembed-bar.png, xembed-menu.png, xembed-closed.png)"
