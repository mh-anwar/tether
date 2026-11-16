#!/usr/bin/env bash
# Runs a WebDriver test against the real window: virtual screen, fake phone, tauri-driver (which starts
# the window through WebKitWebDriver). Same isolated Tether as run.sh (its config, pairing and data).
# Usage: testing/ui/wd.sh [test.py]   (default: testing/ui/test_window.py)
# Needs: tauri-driver (`cargo install tauri-driver --locked`), WebKitWebDriver (Debian/Ubuntu package
# webkitgtk-webdriver), a debug build, and the fake phone paired once (run.sh start + accept).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${CARGO_TARGET_DIR:-$ROOT/target}/debug
W=${TETHER_UI_DIR:-/tmp/tether-ui-$USER}
DISPLAY_NUM=${WD_DISPLAY:-:59}
PORT=${WD_PORT:-14444}
TEST=${1:-$ROOT/testing/ui/test_window.py}
export TOKIO_WORKER_THREADS=2 LP_NUM_THREADS=2 WEBKIT_DISABLE_COMPOSITING_MODE=1
export XDG_CONFIG_HOME=$W/cfg XDG_DATA_HOME=$W/data XDG_RUNTIME_DIR=$W/run DISPLAY=$DISPLAY_NUM
mkdir -p "$W"/cfg/tether "$W"/data "$W"/run && chmod 700 "$W"/run
[ -f "$W/cfg/tether/config.json" ] || echo "{\"device_name\":\"Workstation\",\"mirror_notifications\":false,\"downloads_dir\":\"$W/dl\"}" > "$W/cfg/tether/config.json"

# Every run starts from the same phone-apps state: stand-in adb/scrcpy, never paired for apps.
export FAKE_APPS_DIR=$W/apps-wd TETHER_ADB=$ROOT/testing/apps/adb TETHER_SCRCPY=$ROOT/testing/apps/scrcpy
rm -rf "$FAKE_APPS_DIR" "$W/data/tether/apps-paired" "$W/data/tether/apps-connect-address" && mkdir -p "$FAKE_APPS_DIR"

pids=()
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
  # The window starts its own daemon for this data folder; stop it too.
  for p in $(pgrep -x tether || true); do { tr "\0" "\n" < /proc/$p/environ | grep -q "XDG_DATA_HOME=$W/data" && kill "$p"; } 2>/dev/null || true; done
}
trap cleanup EXIT
"$BIN/tether" daemon > "$W/daemon.log" 2>&1 & pids+=($!)
sleep 2
DAEMON_PORT=$(sed -n 's/.* on tcp \([0-9]*\).*/\1/p' "$W/daemon.log" | head -1)
export FAKE_MOUSEPAD_LOG=$W/mousepad.jsonl
: > "$FAKE_MOUSEPAD_LOG"
Xvfb "$DISPLAY_NUM" -screen 0 1400x900x24 -nolisten tcp >/dev/null 2>&1 & pids+=($!)
# The phone's file server, once the service has created its key (after the first run).
if [ -f "$W/data/tether/key.der" ] && [ -x /usr/sbin/sshd ]; then
  "$ROOT/testing/phone-files/serve.sh" "$W" "$W/data" 2300 > "$W/sftp.out" & pids+=($!)
  sleep 1; export FAKE_SFTP=$(head -1 "$W/sftp.out")
fi
if [ "${TETHER_SKIP_PHONE:-0}" != 1 ]; then
  "$BIN/examples/fake_phone" "${TETHER_PHONE_ADDRESS:-127.0.0.1:$DAEMON_PORT}" "$W/phone" > "$W/phone.log" 2>&1 & pids+=($!)
fi
"${TAURI_DRIVER:-$HOME/.cargo/bin/tauri-driver}" --port "$PORT" --native-port $((PORT + 1)) > "$W/wd.log" 2>&1 & pids+=($!)
sleep 2
# RUST_LOG changes uv's own logging; keep it out.
env -u RUST_LOG PYTHONUNBUFFERED=1 WD_URL="http://127.0.0.1:$PORT" TETHER_APP="$BIN/tether-app" \
  uv run -q --no-project --with selenium python "$TEST"
