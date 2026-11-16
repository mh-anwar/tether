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

## Window tests
- **WebDriver package not found**: on Debian/Ubuntu it's `webkitgtk-webdriver`, not `webkit2gtk-driver`.
- **Apps page shows "Reconnecting" in a test**: an earlier run left `$XDG_DATA_HOME/tether/apps-paired`;
  `wd.sh` removes it, `run.sh` doesn't.

## Updates
- **Restart after an update starts nothing**: once a running binary's file is replaced, Linux reports its
  path as `<path> (deleted)`; strip that suffix before re-running it (`update::exe_path`, `relaunch`).
- **Restart starts the old version from an AppImage**: the window's own binary lives in the old AppImage's
  mount; re-run `$APPIMAGE`, the file that was replaced.

## Window
- **Window freezes while texts arrive**: every phone update used to redraw whole lists at once. Updates now
  call `invalidate(part)` and are drawn once per frame; `swap` leaves nodes that are already in place;
  the conversation list reuses rows and draws 80 at a time. Calling `renderList()` straight from an event
  handler brings the freeze back. `nameFor` uses an index; don't search `state.contacts` per row.

## Tray
- **No tray icon on GNOME**: GNOME has no tray without the "AppIndicator and KStatusNotifierItem Support"
  extension. With it the icon appears within 10 s (the service looks for the watcher on that interval).
- **Icon with a black square on a transparent bar**: a tray that draws with transparency must publish
  `_NET_SYSTEM_TRAY_VISUAL` (a 32-bit visual); without it the icon is mixed with the bar colour read
  from the screen, which a compositor can get wrong.

## Phone files
- **Copy/Save on a screenshot says the phone didn't share its files**: the phone's "Filesystem expose"
  plugin is off or lacks storage permission, so it never answers `kdeconnect.sftp.request` (no error
  packet). On the phone: KDE Connect, this computer, Plugin settings, Filesystem expose.
- **Photos previews are slow on the first visit**: the phone has no small versions to hand out, so each
  preview downloads the whole photo once (several MB). They are kept afterwards. HEIC photos and videos
  are not listed (no decoder).
- **No "Take a photo" in Ctrl+K or on the Photos page**: current KDE Connect Android releases dropped their
  photo plugin, so the phone doesn't list `kdeconnect.photo.request` and Tether hides the command. Older
  releases (and forks that kept it) show it.
- **Screenshot buttons only in English**: notifications are matched on the word "screenshot" in the app
  name, title or text; other languages miss them.

## Phone apps (adb)
- **QR pairing never finds the phone**: adb (37, from the scrcpy bundle) finds phones over multicast DNS
  (Openscreen, on by default; `ADB_MDNS_OPENSCREEN=0` switches to the system's Bonjour). A firewall that
  drops UDP 5353 hides the phone. Tether now also asks the phone directly (unicast to its port 5353).
  First check: `tether apps doctor` (helper versions, `adb mdns check`, what adb heard, what the phone
  answered directly, the KDE Connect phone IP).
- **`adb pair` code in arguments**: visible to other users in /proc; we pass it on stdin (adb reads it
  when the argument is left out, `client/commandline.cpp`).

- **Phone stuck on "Pairing device" after scanning the phone-apps QR code**: the phone did scan and is waiting
  for the computer to connect to its pairing port; the computer never heard the phone's mDNS announcement
  (firewall dropping multicast, or the phone ignoring legacy-unicast questions). Tether now also asks from its
  own `<ip>:5353` with the unicast-reply bit and, failing that, tries the password on the phone's open ports.

- **QR pairing succeeds but Apps stays on “Reconnecting to your phone”**: pairing and app connections
  use separate random ports. The pairing fallback did not carry through to `finish_connect` or automatic
  reconnects; both still depended on mDNS. Both now share direct-port discovery, with a one-minute sweep
  cooldown and three-second adb connection attempts; verified online addresses are saved separately as
  `apps-connect-address` for reconnects when announcements are hidden. This addresses a code gap;
  the owner's Samsung still needs to confirm the fix.
- **The window shows old UI after editing `ui/` (a merge, a checkout)**: the files are embedded when
  `tether-app` compiles, Cargo only watched the `ui` folder's own mtime, and a rerun build script alone
  doesn't recompile the crate. `crates/tether-app/build.rs` now watches each file and passes a hash of
  their contents as `TETHER_UI_HASH`, which `main.rs` reads with `env!`, so any edit recompiles the window.
