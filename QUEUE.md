# Work queue

The single list of what to do next, in priority order. Read it at the start of a session; update it before
finishing (move items, add what you found, note what's waiting on the owner). Detail lives in `docs/`.

Item format: `- [ ] Q<n> **Title**: what and why. Done when: the check that proves it.`
Status marks: `[ ]` open, `[~]` in progress (say who/which branch), `[x]` done (move to Done with the date).

## Waiting on the owner

- Q1 **Pick the next big feature**: calls on the PC (recommended), phone screen and apps, or photos.
- Q2 **Extra ideas to approve or drop**: schedule a text to send later; quiet mode (phone notifications
  collected into one summary instead of popping up all day).

## Now

- [~] Q3 **Try it on the real Samsung phone**: packages are built by `.github/workflows/release.yml`
  (.deb, .rpm, AppImage, tarball + install.sh), start at login is on, README has install and pairing steps.
  Waiting on the owner to install and pair. Done when: the owner pairs the phone and texts from the laptop.

## Next

- [ ] Q4 **Calls on the PC**: Bluetooth hands-free (BlueZ + PipeWire HFP, the computer as the headset) for
  audio, answer, decline, hang up and dial; call banner in the window. KDE Connect only reports calls, so
  this is a separate channel (see docs/decisions.md). Done when: a call to the phone is answered and heard
  on the laptop.
- [ ] Q5 **Phone screen and apps**: Android wireless debugging (pair code once) plus a scrcpy-style server,
  shown inside the window; launch a single app in its own window. Done when: the phone screen is visible and
  clickable from the laptop.
- [ ] Q6 **Photos**: recent photos from the phone (SFTP plugin, see docs/protocol.md) in a grid; drag a photo
  out to save it. Done when: the last 50 photos show within a few seconds.
- [ ] Q7 **mDNS discovery** (`_kdeconnect._udp`) alongside UDP broadcast, for networks that drop broadcast.
  Wake and network-change reconnects are done (netwatch). Done when: a phone on a broadcast-blocking network
  connects without `tether connect`.

## Later

- [ ] Q10 **Tray icon** with unread count. Closing the window already keeps the service running. Care on i3:
  i3bar shows only XEmbed icons, Tauri's tray is AppIndicator (SNI).
- [ ] Q11 **Settings screen**: device name, phone addresses for VPNs, notification mirroring, downloads folder,
  unpair, clipboard sync on/off, start at login (both exist as config/CLI only).
- [ ] Q12 **Narrow window layouts** checked with screenshots (720 px and 1000 px wide).
- [ ] Q13 **Drag-and-drop send** verified for real (needs a session with a window manager or the owner).
- [ ] Q14 **Notification icons** shown in the column (payload download exists; check with the real phone).
- [ ] Q16 **Forked Android app** only if a feature can't go through KDE Connect (for example call control
  without Bluetooth).


## Done

- 2026-11-11 Packages (CI), start at login, warning + takeover when KDE Connect holds the port.
- 2026-11-11 Clipboard sync in the daemon (window closed too); reconnect after sleep and network changes.
- 2026-11-11 Media: player card (art, seek, controls), `tether media`, phone's player on desktop media keys
  (MPRIS: playerctl, GNOME/KDE widgets, bar modules).
- 2026-11-11 Picture messages: thumbnails in threads, full picture or video on click, save to Downloads.

- 2026-11-11 Protocol core: discovery, TLS, pairing, payloads, plugins; verified against KDE Connect 26.08.2.
- 2026-11-11 Daemon with local API, CLI, desktop notification mirroring, auto-redial.
- 2026-11-11 Messages window with notifications column, codes, Ctrl+K, drop to send, dark theme.
- 2026-11-11 Private GitHub repo: github.com/mh-anwar/tether.
