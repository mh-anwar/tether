# Work queue

The single list of what to do next, in priority order. Read it at the start of a session; update it before
finishing (move items, add what you found, note what's waiting on the owner). Detail lives in `docs/`.

Item format: `- [ ] Q<n> **Title**: what and why. Done when: the check that proves it.`
Status marks: `[ ]` open, `[~]` in progress (say who/which branch), `[x]` done (move to Done with the date).

## Waiting on the owner

- Q1 **Pick the next big feature**: calls on the PC (recommended), phone screen and apps, photos, or an
  installable package to try Tether with the real phone.
- Q2 **Extra ideas to approve or drop**: schedule a text to send later; quiet mode (phone notifications
  collected into one summary instead of popping up all day).

## Now

- [ ] Q3 **Try it on the real Samsung phone**: an installable build for the owner's laptop (AppImage or .deb,
  plus a systemd user service and autostart) and a short pairing guide. Everything so far is tested against a
  stand-in phone and real KDE Connect desktop only. Done when: the owner pairs the phone and texts from the
  laptop.

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
- [ ] Q7 **mDNS discovery** (`_kdeconnect._udp`) alongside UDP broadcast; trigger `refresh` on network change
  and resume from suspend. Done when: the phone reconnects within 10 s after the laptop wakes up.
- [ ] Q8 **Clipboard sync in the daemon** (X11 and Wayland), so it works with the window closed; a setting to
  turn it off. Done when: copying on either side pastes on the other with the window closed.

## Later

- [ ] Q9 **Picture messages**: download MMS attachments (`sms.request_attachment`) and show them in threads.
- [ ] Q10 **Tray icon** with unread count; closing the window keeps Tether running.
- [ ] Q11 **Settings screen**: device name, phone addresses for VPNs, notification mirroring, downloads folder,
  unpair.
- [ ] Q12 **Narrow window layouts** checked with screenshots (720 px and 1000 px wide).
- [ ] Q13 **Drag-and-drop send** verified for real (needs a session with a window manager or the owner).
- [ ] Q14 **Notification icons** shown in the column (payload download exists; check with the real phone).
- [ ] Q15 **Media controls** for what plays on the phone (mpris plugin).
- [ ] Q16 **Forked Android app** only if a feature can't go through KDE Connect (for example call control
  without Bluetooth).

## Done

- 2026-11-11 Protocol core: discovery, TLS, pairing, payloads, plugins; verified against KDE Connect 26.08.2.
- 2026-11-11 Daemon with local API, CLI, desktop notification mirroring, auto-redial.
- 2026-11-11 Messages window with notifications column, codes, Ctrl+K, drop to send, dark theme.
- 2026-11-11 Private GitHub repo: github.com/mh-anwar/tether.
