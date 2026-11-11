# Headless KDE Connect desktop peer for testing Tether without a phone.
# Debian testing ships a current kdeconnect (protocol v8).
FROM debian:testing-slim

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      kdeconnect dbus libglib2.0-bin ca-certificates procps iproute2 \
 && rm -rf /var/lib/apt/lists/*

# Run as an unprivileged user; its config (identity key + certificate, paired devices) lives in a volume.
RUN useradd -m -u 1000 peer && mkdir -p /home/peer/.config/kdeconnect && chown -R peer:peer /home/peer/.config
USER peer
WORKDIR /home/peer
ENV QT_QPA_PLATFORM=offscreen \
    LANG=C.UTF-8 \
    HOME=/home/peer \
    XDG_RUNTIME_DIR=/tmp/runtime-peer \
    DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/runtime-peer/bus

COPY session.conf /etc/tether-peer/session.conf
COPY --chmod=0755 entrypoint.sh /usr/local/bin/entrypoint.sh
EXPOSE 1716/udp 1716/tcp
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
