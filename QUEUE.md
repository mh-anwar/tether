# Work queue

The single list of what to do next, in priority order. Read it at the start of a session; update it before
finishing (move items, add what you found, note what's waiting on the owner). Detail lives in `docs/`.

Item format: `- [ ] Q<n> **Title**: what and why. Done when: the check that proves it.`
Status marks: `[ ]` open, `[~]` in progress (say who/which branch), `[x]` done (move to Done with the date).

## Waiting on the owner

- Q1 **Next big feature after phone apps**: calls on the PC (recommended) or photos. Phone apps was picked
  on 2026-11-11 as the differentiator (docs/positioning.md).
- Q20 **Launch plan** (docs/positioning.md, "Going viral"): approve the order: public repo, Flathub, demo
  video, then posts.
- Q17a **Make the GitHub repo public**: owner said not yet (2026-11-12), more to build first. Flathub and
  auto-update (Q29) wait on it.
- Q2 **Extra ideas to approve or drop**: schedule a text to send later; quiet mode (phone notifications
  collected into one summary instead of popping up all day).

## Now

Owner feedback on v0.2.0 with the real phone (2026-11-12). Details and hypotheses: HANDOFF.md.

- [ ] Q31 **After an upgrade the new window talks to the old background service** (found 2026-11-12 from the
  owner's screenshot on 0.3.0: the error text was 0.2.0's, and the phone IP wasn't filled in because the
  old service doesn't send it). The old service (login service or the previous window's copy) keeps
  running, so none of 0.3.0's pairing fixes were actually live. Fix: `identity` returns the service's
  version; when it differs from the window's, the window stops it (`shutdown`, already used by the update
  restart) and starts its own copy; also re-point the login service at the new install (`autostart on`
  already runs at each launch). Done when: installing a new version over a running old one shows the new
  behaviour without a reboot.
- [ ] Q26b **Pairing by code is still too much** (owner, 2026-11-12, screenshot: "no one is going to do
  this"). The form showed IP, port and code fields at once, plus a QR-mode error ("scan again") in code mode.
  Direction: one 6-digit field and nothing else; Tether finds the phone's port itself (works once Q31 is
  fixed: the 0.3.0 service asks the phone directly). If the phone can't be found, say what to check on the
  phone instead of showing more fields; keep manual IP/port only behind a small "Enter the address
  yourself" link. Clear QR errors when switching mode. Show a sketch or screenshot to the owner first.
  Done when: the owner pairs by typing only the 6 digits.
- [~] Q25 **QR pairing for phone apps fails on the real phone** (same Wi-Fi). Done on this box: Tether now
  also asks the phone directly for its wireless-debugging ports (unicast to `<phone IP>:5353`, gets past
  firewalls that drop multicast), logs what it sees, and `tether apps doctor` prints a diagnosis. Not yet
  tried on the real phone. Done when: the owner scans the QR and an app opens; if it still fails, the
  owner's `tether apps doctor` output says why.
- [~] Q26 **Pairing by code is too hard**: 0.3.0 finds the port itself when it can, else asks for port (and IP
  if unknown). Owner found it still too much; continued as Q26b.
- [ ] Q27 **Window feels slow**: measure (open-to-first-paint, render time with 500 threads), then batch
  renders and render only what changed. Done when: numbers before/after, and the owner says it's snappy.
- [ ] Q28 **Notifications and messages appear late**: cache the thread list on disk so the window opens
  full, show cached data while the phone answers. Done when: reopening the window shows texts at once.
- [ ] Q10 **Icon in the status bar / tray** with unread count (owner asked). Closing the window already
  keeps the service running. Care on i3: i3bar shows only XEmbed icons, Tauri's tray is AppIndicator (SNI);
  GNOME needs the AppIndicator extension. Done when: the icon shows on the owner's i3 and on KDE/GNOME.
- [~] Q5 **Phone apps in their own windows** (branch t3/find-tether-differentiator): Apps page, QR or code
  pairing over wireless debugging, app list, open an app or the whole screen, one-click scrcpy download,
  `tether apps <name>`, pins with real icons. Released in v0.2.0; verified only with stand-in adb/scrcpy.
  Done when: an app from the owner's Samsung opens in a window on the laptop.

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

- [~] Q29 **Auto-update**: built and released in 0.3.0. Checked here end to end with a stand-in release
  (no-admin install: check, download, verify, swap, restart). Live only once the repo is public (Q17a): a
  private repo's releases can't be seen. AppImage, .deb and .rpm paths are unit-tested only. Done when: the
  owner's install picks up a newer release by itself.

- [ ] Q30 **Grow the WebDriver tests** (`testing/ui/test_window.py`, started 2026-11-12 with 5 checks): send a
  text and see the reply, QR and code pairing through to the app grid, the update card with a stand-in
  release, notification actions. Then run them in CI under Xvfb. Done when: every screen has a check and
  CI runs them.

## Later

- [ ] Q11 **Settings screen**: device name, phone addresses for VPNs, notification mirroring, downloads folder,
  unpair, clipboard sync on/off, start at login (both exist as config/CLI only).
- [ ] Q12 **Narrow window layouts** checked with screenshots (720 px and 1000 px wide).
- [ ] Q13 **Drag-and-drop send** verified for real (needs a session with a window manager or the owner).
- [ ] Q14 **Notification icons** shown in the column (payload download exists; check with the real phone).
- [ ] Q18 **Real app icons for every app**: icons now come from notifications (apps that notified at least
  once); the rest still show letters. Next: pull icons from the phone over adb.
- [ ] Q21 **Phone readiness checklist** after pairing: notification access, SMS and contacts permissions,
  Samsung battery limits for KDE Connect, each with the exact place to tap on the phone (advisor review).
- [ ] Q22 **Limits on untrusted input**: cap concurrent unauthenticated handshakes and the local API's line
  length (advisor review; the API socket is already user-only).
- [ ] Q23 **Dialogs**: focus trap and focus return for the command palette and picture viewer; palette as
  a proper combobox for screen readers.
- [ ] Q24 **Phone-app workspaces** (idea): "Work" opens several pinned phone apps at once. And dropping a
  file on a pinned app opens it in that app on the phone.
- [ ] Q16 **Forked Android app** only if a feature can't go through KDE Connect (for example call control
  without Bluetooth).


## Done

- 2026-11-12 v0.3.0: auto-update, phone-apps pairing fixes (Q25/Q26 wait on the real phone).
- 2026-11-11 Phone-apps pairing: direct phone query past firewalls, `tether apps doctor`, code form that
  needs only the 6 digits (verified with stand-ins; Q25/Q26 wait on the real phone).
- 2026-11-12 v0.2.0 released; owner installed it, paired the Samsung, texts and notifications work.
- 2026-11-11 Phone apps (Apps page, QR pairing, helper download, pin to the desktop's app menu),
  positioning, Flathub store listing.
- 2026-11-11 Packages (CI), start at login, warning + takeover when KDE Connect holds the port.
- 2026-11-11 Clipboard sync in the daemon (window closed too); reconnect after sleep and network changes.
- 2026-11-11 Media: player card (art, seek, controls), `tether media`, phone's player on desktop media keys
  (MPRIS: playerctl, GNOME/KDE widgets, bar modules).
- 2026-11-11 Picture messages: thumbnails in threads, full picture or video on click, save to Downloads.

- 2026-11-11 Protocol core: discovery, TLS, pairing, payloads, plugins; verified against KDE Connect 26.08.2.
- 2026-11-11 Daemon with local API, CLI, desktop notification mirroring, auto-redial.
- 2026-11-11 Messages window with notifications column, codes, Ctrl+K, drop to send, dark theme.
- 2026-11-11 Private GitHub repo: github.com/mh-anwar/tether.
