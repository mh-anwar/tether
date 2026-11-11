# KDE Connect test peer

A headless KDE Connect desktop daemon (`kdeconnectd` 26.08.2 from Debian testing, protocol version 8) in rootless
Docker, so Tether's discovery, TLS, pairing and packets can be tested without a phone.

Files: `Dockerfile`, `entrypoint.sh` (starts a private D-Bus session bus, then `kdeconnectd`), `session.conf` (that
bus's config), `run.sh` (build and start), `cli.sh` (control it).

## Build and start

```sh
./run.sh            # builds the image the first time, (re)starts the container, prints the peer's device id
./run.sh --build    # force a rebuild (after editing the Dockerfile)
```

The container is called `tether-kdc-peer`, restarts by itself after reboots (`--restart unless-stopped`) and keeps
its identity (certificate, key, paired devices) in the Docker volume `tether-kdc-peer-config`. Its device name is
`tether-kdc-peer`.

Stop / remove / wipe identity:

```sh
docker stop tether-kdc-peer        # start again with: docker start tether-kdc-peer
docker rm -f tether-kdc-peer       # remove the container (identity is kept in the volume)
docker volume rm tether-kdc-peer-config   # forget identity and pairings (only after removing the container)
```

`DOCKER_HOST` must point at the rootless daemon (`unix:///run/user/$(id -u)/docker.sock`); the scripts set it.

## Device id

```sh
./cli.sh --my-id
```

## Where Tether reaches it

**TCP `127.0.0.1:1764`** (or this host's LAN IP, port 1764). Host port 1716 stays free for Tether.

Why not `--network host`: with rootless Docker, "host" networking only shares rootlesskit's private network
namespace (slirp4netns), not the real host's interfaces, so the peer would neither see Tether's broadcasts nor be
reachable on the host's 1716. So the peer runs on the normal bridge network with its TCP and UDP 1716 published
on host port 1764 (change with `PEER_PORT=...` / `PEER_UDP_PORT=...`).

What works through that, and what does not:

| Direction | Works? | Notes |
| --- | --- | --- |
| Tether opens TCP to `127.0.0.1:1764`, sends its identity, then acts as TLS server | Yes (verified) | The normal "I heard your broadcast, connecting" path. Give Tether a manual peer `127.0.0.1:1764`. |
| Peer sends its UDP identity to Tether | Yes (verified) | `./cli.sh add-ip <host LAN IP>`; it arrives on host UDP 1716 from the host's LAN IP, but advertises `tcpPort: 1716` (its port inside the container). Tether must connect to port 1764 instead, so use the manual address. |
| Tether broadcasts / sends UDP to the peer and waits for it to connect back | No | The peer sees the sender as the Docker bridge gateway inside rootlesskit's namespace and its TCP connect-back never reaches the host. |
| Peer's broadcasts reach the LAN or real phones | No | Broadcasts stay inside slirp4netns, so the peer never interferes with the real network. |

Fully protocol-faithful alternative (not set up by default): `PEER_PORT=1716 PEER_UDP_PORT=1765 ./run.sh` gives the
peer host TCP 1716; Tether then keeps UDP 1716 and takes TCP 1717 or higher (allowed: the identity packet carries
`tcpPort`), and after `./cli.sh add-ip <host LAN IP>` Tether can follow the peer's announcement as-is.

## Pairing

Tether asks to pair:

```sh
./cli.sh requests              # device ids that sent a pairing request
./cli.sh accept -d DEVICE_ID   # or: ./cli.sh reject -d DEVICE_ID
```

The peer asks Tether to pair (Tether must be connected, i.e. listed as reachable):

```sh
./cli.sh -l                    # list known devices and their state
./cli.sh --pair -d DEVICE_ID
```

Other useful commands (anything not listed here is passed to `kdeconnect-cli`, see `./cli.sh --help`):

```sh
./cli.sh --ping -d ID            ./cli.sh --ping-msg "hi" -d ID
./cli.sh --share-text "hi" -d ID ./cli.sh --share /path/in/container -d ID
./cli.sh --encryption-info -d ID # both certificate fingerprints
./cli.sh --unpair -d ID
./cli.sh add-ip IP | list-ips | clear-ips   # manual peer IPs (the daemon's customDevices setting)
./cli.sh logs                    # follow the daemon log
```

`accept`, `reject`, `requests` and the IP commands go straight to the daemon's D-Bus API
(`org.kde.kdeconnect.device.acceptPairing`, the `customDevices` property) because `kdeconnect-cli` has no flags for them.

## Notes

- The session bus has no D-Bus service activation on purpose: otherwise an early `kdeconnect-cli` call starts a
  second daemon and the first one exits as a duplicate, stopping the container.
- Harmless log noise: no BlueZ, no notification server.
- The image is about 1.2 GB: Debian's `kdeconnect` package pulls in Qt 6, QML and KDE Frameworks even with
  `--no-install-recommends`.

## Testing file transfers

From the host, payload connections can't reach the container's ports or come back to the host. Run Tether in
a second container on the same Docker network instead:

```sh
docker run -d --name tether-under-test \
  -v "$PWD/target/debug/tether:/usr/local/bin/tether:ro" \
  -e XDG_CONFIG_HOME=/tmp/cfg -e XDG_DATA_HOME=/tmp/data -e XDG_RUNTIME_DIR=/tmp/run \
  --entrypoint /usr/local/bin/tether tether-kdc-peer:latest daemon
docker exec -e XDG_RUNTIME_DIR=/tmp/run tether-under-test tether connect <peer container IP>:1716
```
