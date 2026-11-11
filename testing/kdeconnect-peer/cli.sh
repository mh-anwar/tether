#!/bin/sh
# Run kdeconnect-cli inside the peer, plus a few helpers kdeconnect-cli lacks.
#   ./cli.sh -l | --my-id | --pair -d ID | --ping -d ID | --help ...   (passed to kdeconnect-cli)
#   ./cli.sh add-ip 10.0.0.5        add a manual peer IP (kdeconnectd sends its identity there over UDP 1716)
#   ./cli.sh list-ips | clear-ips   show / clear manual peer IPs
#   ./cli.sh accept -d ID           accept an incoming pairing request (D-Bus; kdeconnect-cli has no flag)
#   ./cli.sh reject -d ID           reject it
#   ./cli.sh requests               list devices with pending pairing requests
#   ./cli.sh logs                   follow the daemon's log
set -e
export DOCKER_HOST="${DOCKER_HOST:-unix:///run/user/$(id -u)/docker.sock}"
NAME=tether-kdc-peer
X="docker exec $NAME"
PROPS=org.freedesktop.DBus.Properties

GD="gdbus call --session --dest org.kde.kdeconnect"
# customDevices is a read/write D-Bus property of the daemon; kdeconnectd persists it in its config.
ips() { $X $GD --object-path /modules/kdeconnect --method $PROPS.Get org.kde.kdeconnect.daemon customDevices \
          | grep -oE "'[^']+'" | tr -d "'"; }
setips() { # $@ = full list of IPs
  arr=""; for ip in "$@"; do arr="$arr${arr:+, }'$ip'"; done
  $X $GD --object-path /modules/kdeconnect --method $PROPS.Set org.kde.kdeconnect.daemon customDevices \
     "<@as [$arr]>" >/dev/null
}
devarg() { [ "$1" = "-d" ] && [ -n "$2" ] || { echo "usage: $0 $cmd -d DEVICE_ID" >&2; exit 2; }; echo "$2"; }

cmd="$1"
case "$cmd" in
  add-ip)   shift; [ -n "$1" ] || { echo "usage: $0 add-ip IP" >&2; exit 2; }
            setips $(ips | grep -vxF "$1") "$1"; ips ;;
  list-ips) ips ;;
  clear-ips) setips; ips ;;
  accept|reject) shift; id=$(devarg "$@")
            m=acceptPairing; [ "$cmd" = reject ] && m=cancelPairing
            $X $GD --object-path "/modules/kdeconnect/devices/$id" --method org.kde.kdeconnect.device.$m >/dev/null \
              && echo "$cmd: $id" ;;
  requests) $X $GD --object-path /modules/kdeconnect --method $PROPS.Get org.kde.kdeconnect.daemon pairingRequests \
              | grep -oE "'[^']+'" | tr -d "'" ;;
  logs)     docker logs -f "$NAME" ;;
  # kdeconnect-cli always tries D-Bus activation first; the bus has none on purpose, so drop that one line.
  *)        exec docker exec "$NAME" sh -c 'kdeconnect-cli "$@" 2>/tmp/cli.err; rc=$?
              grep -v "^error activating kdeconnectd" /tmp/cli.err >&2; exit $rc' sh "$@" ;;
esac
