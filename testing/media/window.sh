#!/usr/bin/env bash
# Window test for received files, call/notification settings and "Send text or link" (testing/media/window.py).
# Private display, daemon port and data folder, so it never touches a daemon that owns UDP/TCP 1716:
# the daemon is started here (the window attaches to it) and the fake phone dials its actual port.
# Usage: CARGO_TARGET_DIR=... TETHER_UI_DIR=/tmp/tw-media testing/media/window.sh [shot.png]
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${CARGO_TARGET_DIR:-$ROOT/target}/debug
W=${TETHER_UI_DIR:-/tmp/tw-media}
D=${MEDIA_DISPLAY:-:72}
PORT=${WD_PORT:-14872}
rm -rf "$W"; mkdir -p "$W"/cfg/tether "$W"/data "$W"/run "$W"/control "$W"/dl && chmod 700 "$W"/run
echo "{\"device_name\":\"Workstation\",\"mirror_notifications\":false,\"clipboard_sync\":false,\"auto_update\":false,\"downloads_dir\":\"$W/dl\"}" > "$W/cfg/tether/config.json"
export XDG_CONFIG_HOME=$W/cfg XDG_DATA_HOME=$W/data XDG_RUNTIME_DIR=$W/run DISPLAY=$D FAKE_PHONE_CONTROL=$W/control
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT
[ -e "/tmp/.X${D#:}-lock" ] || { Xvfb "$D" -screen 0 1400x900x24 -nolisten tcp >/dev/null 2>&1 & pids+=($!); sleep 1; }
dbus-run-session -- bash -c '
  RUST_LOG=info "$0/tether" daemon > "$1/daemon.log" 2>&1 & dpid=$!
  trap "kill $dpid 2>/dev/null" EXIT
  for _ in $(seq 100); do grep -qE "on tcp 17[0-9]+" "$1/daemon.log" && break; sleep .1; done
  port=$(grep -oE "on tcp 17[0-9]+" "$1/daemon.log" | head -1 | grep -oE "[0-9]+$")
  "$0/examples/fake_phone" 127.0.0.1:$port "$1/phone" > "$1/phone.log" 2>&1 & ppid=$!
  trap "kill $dpid $ppid 2>/dev/null" EXIT
  "${TAURI_DRIVER:-$HOME/.cargo/bin/tauri-driver}" --port "$2" --native-port $(($2 + 1)) > "$1/wd.log" 2>&1 & tpid=$!
  trap "kill $dpid $ppid $tpid 2>/dev/null" EXIT
  sleep 2
  env -u RUST_LOG WD_URL="http://127.0.0.1:$2" TETHER_APP="$0/tether-app" TETHER_UI_DIR="$1" SHOT_OUT="${3:-}" \
    uv run -q --no-project --with selenium --with pillow python "$4/testing/media/window.py"
' "$BIN" "$W" "$PORT" "${1:-}" "$ROOT"
