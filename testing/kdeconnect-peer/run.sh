#!/bin/sh
# Start (or restart) the headless KDE Connect test peer.
# Rootless Docker: --network host would only share rootlesskit's private namespace, not the real host network,
# so the peer runs on the default bridge with its TCP/UDP 1716 published on PEER_PORT (default 1764) instead.
# That leaves host port 1716 free for Tether. See README.md.
set -e
cd "$(dirname "$0")"
export DOCKER_HOST="${DOCKER_HOST:-unix:///run/user/$(id -u)/docker.sock}"
NAME=tether-kdc-peer
PEER_PORT="${PEER_PORT:-1764}"            # host TCP port -> peer TCP 1716
PEER_UDP_PORT="${PEER_UDP_PORT:-$PEER_PORT}"  # host UDP port -> peer UDP 1716

if ! docker image inspect "$NAME:latest" >/dev/null 2>&1 || [ "$1" = "--build" ]; then
  box heavy -- docker build -t "$NAME:latest" . 2>/dev/null || docker build -t "$NAME:latest" .
fi
docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" \
  --hostname tether-kdc-peer \
  --restart unless-stopped \
  -p "0.0.0.0:$PEER_PORT:1716/tcp" -p "0.0.0.0:$PEER_UDP_PORT:1716/udp" \
  -v tether-kdc-peer-config:/home/peer/.config/kdeconnect \
  --memory 512m \
  "$NAME:latest" >/dev/null

# Wait for the daemon to answer on D-Bus, then print its identity.
# (kdeconnect-cli --my-id reads the config file, so ask the daemon itself over D-Bus.)
i=0; until docker exec "$NAME" gdbus call --session --dest org.kde.kdeconnect --object-path /modules/kdeconnect \
             --method org.kde.kdeconnect.daemon.selfId >/dev/null 2>&1; do
  i=$((i+1)); [ $i -gt 30 ] && { echo "kdeconnectd did not come up; see: docker logs $NAME" >&2; exit 1; }; sleep 1
done
echo "Peer running: $NAME, device id $(docker exec "$NAME" kdeconnect-cli --my-id 2>/dev/null)"
echo "Reach it over TCP at <this host's IP or 127.0.0.1>:$PEER_PORT (UDP published on $PEER_UDP_PORT)."
