# Gotchas

Traps that already cost time. Add new ones here with the symptom first.

## Building
- **5-minute rebuild for no reason**: building `-p tether` and `-p tether-app` in separate commands changes
  dependency features, so everything recompiles. Always build them together.
- **Change not picked up**: `cargo build ... --example fake_phone` builds only the example, not the listed
  binaries. Build binaries and examples in separate commands.
- **`target/` size**: ~3.3 GB with the window app. `cargo clean` past ~15 GB.

## Testing
- **`run.sh start` says Xvfb runs but the window can't open the display**: `pgrep -f "Xvfb :57"` also
  matches your own shell when the command line you ran contains that text. Don't mention it in the same
  command.

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

- **Binaries from this box fail on the laptop** ("GLIBC_2.4x not found"): build releases in CI on Ubuntu
  22.04 rather than shipping `target/` from here.
- **`ldconfig: not found` in scripts**: it's in /sbin, often not on a user's PATH.
- **Clipboard sync silent under systemd**: the user manager may lack `DISPLAY`; clipsync guesses `:0` and
  `wayland-0`. GNOME Wayland has no data-control protocol, so background clipboard reads can't work there.
- **X11 clipboard empty after a test tool exits**: no clipboard manager on Xvfb; the owner leaving clears it.
  `testing/ui/clip.py :N get|set` reads and sets it.

## Protocol (details in protocol.md)
- The phone speaks TLS 1.2 only; the side that opened TCP is the TLS **server**.
- `pair:true` to a phone that thinks it's paired makes it **unpair**; a re-request with a timestamp after a
  reset must be treated as a new request.
- **Device vanishes right after `tether connect` between two Tethers**: both sides dialed at once and each
  kept a different socket ("last link wins", same as upstream lanlinkprovider.cpp `addLink` and Android
  `addOrUpdateLink`, no id tie-break). Paired devices redial on the next tick; unpaired ones reappear at the
  next announcement. Run `connect` again.
- Telephony `isCancel` is the string `"true"`; there is no request packet for battery or signal.
- Sharing UDP 1716 with KDE Connect/Valent (SO_REUSEADDR) splits the phone's announcements; bind exclusively.

## Window
- **Copied text vanishes on X11**: the clipboard owner must stay alive; keep one `arboard::Clipboard`.
- **Page stuck on "Starting Tether"**: the "connected" event can fire before the page listens; the page also
  polls `identity` every 1.5 s until connected.

## Updates
- **Restart after an update starts nothing**: once a running binary's file is replaced, Linux reports its
  path as `<path> (deleted)`; strip that suffix before re-running it (`update::exe_path`, `relaunch`).
- **Restart starts the old version from an AppImage**: the window's own binary lives in the old AppImage's
  mount; re-run `$APPIMAGE`, the file that was replaced.

## Phone apps (adb)
- **QR pairing never finds the phone**: adb (37, from the scrcpy bundle) finds phones over multicast DNS
  (Openscreen, on by default; `ADB_MDNS_OPENSCREEN=0` switches to the system's Bonjour). A firewall that
  drops UDP 5353 hides the phone. Tether now also asks the phone directly (unicast to its port 5353).
  First check: `tether apps doctor` (helper versions, `adb mdns check`, what adb heard, what the phone
  answered directly, the KDE Connect phone IP).
- **`adb pair` code in arguments**: visible to other users in /proc; we pass it on stdin (adb reads it
  when the argument is left out, `client/commandline.cpp`).
