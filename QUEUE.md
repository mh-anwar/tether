# Work queue

The single list of what to do next, in priority order. Read it at the start of a session; update it before
finishing (move items, add what you found, note what's waiting on the owner). Detail lives in `docs/`.

Item format: `- [ ] Q<n> **Title**: what and why. Done when: the check that proves it.`
Status marks: `[ ]` open, `[~]` in progress (say who/which branch), `[x]` done (move to Done with the date).

## Waiting on the owner

- Q1 **Next big feature after phone apps**: calls on the PC (recommended) or photos. Phone apps was picked
  on 2026-11-11 as the differentiator (docs/positioning.md).
- Q17a **Make the GitHub repo public** when ready: Flathub needs it (docs/flathub.md).
- Q2 **Extra ideas to approve or drop**: schedule a text to send later; quiet mode (phone notifications
  collected into one summary instead of popping up all day).

## Now

- [~] Q3 **Try it on the real Samsung phone**: packages are built by `.github/workflows/release.yml`
  (.deb, .rpm, AppImage, tarball + install.sh), start at login is on, README has install and pairing steps.
  Waiting on the owner to install and pair. Done when: the owner pairs the phone and texts from the laptop.
- [~] Q5 **Phone apps in their own windows** (branch t3/find-tether-differentiator): Apps page, QR or code
  pairing over wireless debugging, app list, open an app or the whole screen, one-click scrcpy download,
  `tether apps <name>`. Verified with stand-in adb/scrcpy and the real scrcpy download. Done when: an app
  from the owner's Samsung opens in a window on the laptop (needs a release with this in it).

## Next

- [ ] Q4 **Calls on the PC**: Bluetooth hands-free (BlueZ + PipeWire HFP, the computer as the headset) for
  audio, answer, decline, hang up and dial; call banner in the window. KDE Connect only reports calls, so
  this is a separate channel (see docs/decisions.md). Done when: a call to the phone is answered and heard
  on the laptop.
- [ ] Q17 **Flathub**: Flatpak manifest with bundled adb + scrcpy, Background portal for start at login,
  screenshots, CI build, then the Flathub PR. Store listing is written (packaging/flatpak). Needs Q17a.
  Done when: `flatpak install flathub io.github.mh_anwar.Tether` works.
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
- [ ] Q18 **Real app icons in the Apps page**: start with icons the phone already sends with notifications,
  matched by app name; letters are the fallback.
- [ ] Q19 **Phone apps in the desktop's app menu**: "Add to app menu" writes a `.desktop` file running
  `tether apps <name>`, so rofi, GNOME and KDE launchers start phone apps directly.
- [ ] Q16 **Forked Android app** only if a feature can't go through KDE Connect (for example call control
  without Bluetooth).


## Done

- 2026-11-11 Phone apps (Apps page, QR pairing, helper download), positioning, Flathub store listing.
- 2026-11-11 Packages (CI), start at login, warning + takeover when KDE Connect holds the port.
- 2026-11-11 Clipboard sync in the daemon (window closed too); reconnect after sleep and network changes.
- 2026-11-11 Media: player card (art, seek, controls), `tether media`, phone's player on desktop media keys
  (MPRIS: playerctl, GNOME/KDE widgets, bar modules).
- 2026-11-11 Picture messages: thumbnails in threads, full picture or video on click, save to Downloads.

- 2026-11-11 Protocol core: discovery, TLS, pairing, payloads, plugins; verified against KDE Connect 26.08.2.
- 2026-11-11 Daemon with local API, CLI, desktop notification mirroring, auto-redial.
- 2026-11-11 Messages window with notifications column, codes, Ctrl+K, drop to send, dark theme.
- 2026-11-11 Private GitHub repo: github.com/mh-anwar/tether.
