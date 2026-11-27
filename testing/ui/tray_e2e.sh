#!/usr/bin/env bash
# Tray end to end over D-Bus, as on a Wayland KDE desktop: a private session bus, no X display, an
# isolated service paired with the fake phone, and sniwatcher.py --check as the desktop's tray.
# Usage: testing/ui/tray_e2e.sh
# Needs: box heavy -- cargo build -p tether && box heavy -- cargo build -p tether-core --example fake_phone
# Env: TETHER_UI_DIR (scratch folder, default /tmp/tether-ui-$USER), TETHER_PORT (default 14740).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${CARGO_TARGET_DIR:-$ROOT/target}/debug
W=${TETHER_UI_DIR:-/tmp/tether-ui-$USER}/tray
PORT=${TETHER_PORT:-14740}
rm -rf "$W" && mkdir -p "$W"/cfg/tether "$W"/data "$W"/run "$W"/fc && chmod 700 "$W"/run
echo '{"device_name":"Workstation","mirror_notifications":true,"clipboard_sync":false,"mdns":false}' > "$W/cfg/tether/config.json"
export XDG_CONFIG_HOME=$W/cfg XDG_DATA_HOME=$W/data XDG_RUNTIME_DIR=$W/run TETHER_PORT=$PORT
export XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland TOKIO_WORKER_THREADS=2
unset DISPLAY WAYLAND_DISPLAY
# Call mute goes through a stand-in wpctl.
export TETHER_WPCTL=$ROOT/testing/calls/wpctl FAKE_WPCTL_STATE=$W/mic-muted

pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

dbus-daemon --session --nofork --print-address=3 3>"$W/bus" & pids+=($!)
for _ in $(seq 50); do [ -s "$W/bus" ] && break; sleep 0.1; done
export DBUS_SESSION_BUS_ADDRESS=$(head -1 "$W/bus")

# The stand-in tray first, so the service finds it on its first look.
env -u RUST_LOG PYTHONUNBUFFERED=1 uv run -q --no-project --with dbus-next \
  python "$ROOT/testing/ui/sniwatcher.py" --check "$W/fc/received.jsonl" > "$W/watcher.log" 2>&1 & watcher=$!
sleep 3
"$BIN/tether" daemon > "$W/daemon.log" 2>&1 & daemon=$!; pids+=($daemon)
sleep 1
FAKE_COMPUTER_DIR=$W/fc "$BIN/examples/fake_phone" "127.0.0.1:$PORT" "$W/phone" > "$W/phone.log" 2>&1 & pids+=($!)
for _ in $(seq 50); do grep -q 'asked to pair' "$W/phone.log" && break; sleep 0.2; done
"$BIN/tether" accept >/dev/null

status=0
wait "$watcher" || status=$?
cat "$W/watcher.log"
# "Quit Tether" should have stopped the service.
if kill -0 "$daemon" 2>/dev/null; then echo "FAIL service still running after Quit"; status=1; fi
exit $status
