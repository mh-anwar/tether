# Gotchas

Traps that already cost time. Add new ones here with the symptom first.

## Building
- **5-minute rebuild for no reason**: building `-p tether` and `-p tether-app` in separate commands changes
  dependency features, so everything recompiles. Always build them together.
- **Change not picked up**: `cargo build ... --example fake_phone` builds only the example, not the listed
  binaries. Build binaries and examples in separate commands.
- **`target/` size**: ~3.3 GB with the window app. `cargo clean` past ~15 GB.

## This box
- **Phones/daemons don't find each other locally**: broadcast UDP doesn't loop back on this host. Use
  `tether connect 127.0.0.1`.
- **"path must be shorter than SUN_LEN"**: the Unix socket path is over ~100 characters; use a short
  `XDG_RUNTIME_DIR` (the scratchpad path is too long).
- **Docker peer can't send files to the host**: rootless Docker can't connect back to host ports. Test
  payloads container-to-container.
- **Two daemons running**: killing `dbus-run-session` leaves the `tether-app` child alive, and it restarts its
  own daemon. Kill the recorded app pid (`testing/ui/run.sh stop` does). The daemon lock now prevents a
  second daemon anyway.
- **Text in a typewriter font**: only Liberation fonts are installed and `system-ui` resolved to the mono face;
  the CSS lists named sans fonts before `system-ui`.

## Protocol (details in protocol.md)
- The phone speaks TLS 1.2 only; the side that opened TCP is the TLS **server**.
- `pair:true` to a phone that thinks it's paired makes it **unpair**; a re-request with a timestamp after a
  reset must be treated as a new request.
- Telephony `isCancel` is the string `"true"`; there is no request packet for battery or signal.
- Sharing UDP 1716 with KDE Connect/Valent (SO_REUSEADDR) splits the phone's announcements; bind exclusively.

## Window
- **Copied text vanishes on X11**: the clipboard owner must stay alive; keep one `arboard::Clipboard`.
- **Page stuck on "Starting Tether"**: the "connected" event can fire before the page listens; the page also
  polls `identity` every 1.5 s until connected.
