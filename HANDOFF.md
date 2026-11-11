# Handoff

Goal: a Linux desktop app as good as Windows Phone Link, using the stock KDE Connect Android app.

## Verified
- Core protocol, pairing, payloads: 14 unit tests (handshake both TLS roles, cert/id mismatch rejected,
  200 KB payload roundtrip, SMS parsing, vCards, file-name sanitising).
- Two `tether daemon`s on one box: found each other via `tether connect`, paired with matching code,
  ping, file share, clipboard; the killed side was redialed and reconnected within 20 s unaided.
- Not yet: a real phone, or the real KDE Connect desktop (test double in `testing/kdeconnect-peer/`).
- Note: broadcast UDP doesn't loop back on this server; local tests use `tether connect 127.0.0.1`.

## Next
1. Interop test against the real kdeconnectd container (pair, ping, share both ways).
2. Desktop app (Tauri, client of the daemon socket): agree the screen design with the owner first.
   Screens: messages, notifications, calls, files/photos, phone screen, settings.
3. mDNS announcement (`_kdeconnect._udp`) alongside UDP; network-change and resume triggers for `refresh`.
4. System clipboard sync (X11 and Wayland), SMS attachments, notification icons in the UI.
5. Calls: Bluetooth hands-free (PipeWire/BlueZ HFP) for audio, answer and hang up.
6. Phone screen and apps: Android wireless debugging + scrcpy server, embedded.
7. Packaging: AppImage/Flatpak/.deb, systemd user service, autostart.
