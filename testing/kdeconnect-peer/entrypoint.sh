#!/bin/sh
# Starts a session bus at a fixed path (so `docker exec` / cli.sh can reach it), then kdeconnectd on that bus.
set -e
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
rm -f "$XDG_RUNTIME_DIR/bus"

dbus-daemon --config-file=/etc/tether-peer/session.conf --nofork --nopidfile &
i=0; while [ ! -S "$XDG_RUNTIME_DIR/bus" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done

exec kdeconnectd "$@"
