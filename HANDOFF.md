# Handoff

Goal: a Linux desktop app as good as Windows Phone Link, using the stock KDE Connect Android app.

## Verified
- Core protocol, pairing, payloads: 14 unit tests (handshake both TLS roles, cert/id mismatch rejected,
  200 KB payload roundtrip, SMS parsing, vCards, file-name sanitising).
- Two `tether daemon`s on one box: found each other via `tether connect`, paired with matching code,
  ping, file share, clipboard; the killed side was redialed and reconnected within 20 s unaided.
- Against real KDE Connect 26.08.2 (`testing/kdeconnect-peer/`, Tether run in a second container on the same
  Docker network): auto-discovery, pairing started from either side (codes shown), unpair from the peer,
  ping and shared text both ways, files both ways, reconnect after the peer restarted.
- From the host, payloads to/from the container fail: rootless Docker can't connect back to host ports.
  Run Tether in a container next to the peer for payload tests (see testing/kdeconnect-peer/README.md).
- Not yet: a real phone. Verification code formula matches the upstream source but wasn't compared on screen.
- Note: broadcast UDP doesn't loop back on this server; local tests use `tether connect 127.0.0.1`.

- Window (Messages screen + notifications column), checked under Xvfb with the fake phone
  (`crates/tether-core/examples/fake_phone.rs`): pairing screen and code, thread list with contact names and
  unread, opening a thread loads history, sending a text (confirmed by the phone, reply arrives), inline
  notification reply reaches the phone, Ctrl+K palette, copy code, dark theme.
  Not checked: drag-and-drop sending (no drag in Xvfb), narrow window layouts, the real phone.

## Next
1. Next screens, one at a time with a screenshot to the owner: calls, photos/files, phone screen, settings.
   Tray icon with unread counts. Ideas the owner may like: schedule texts, notification digest.
2. mDNS announcement (`_kdeconnect._udp`) alongside UDP; network-change and resume triggers for `refresh`.
3. System clipboard sync (X11 and Wayland), SMS attachments, notification icons in the UI.
4. Calls: Bluetooth hands-free (PipeWire/BlueZ HFP) for audio, answer and hang up.
5. Phone screen and apps: Android wireless debugging + scrcpy server, embedded.
6. Packaging: AppImage/Flatpak/.deb, systemd user service, autostart.
