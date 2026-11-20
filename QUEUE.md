# Work queue

The single list of what to do next, in priority order. Read it at the start of a session; update it before
finishing (move items, add what you found, note what's waiting on the owner). Detail lives in `docs/`.

Item format: `- [ ] Q<n> **Title**: what and why. Done when: the check that proves it.`
Status marks: `[ ]` open, `[~]` in progress (say who/which branch), `[x]` done (move to Done with the date).

## Waiting on the owner

- Q20 **Launch plan** (docs/positioning.md, "Going viral"): approve the order: public repo, Flathub, demo
  video, then posts.
- Q17a **Make the GitHub repo public**: owner said not yet (2026-11-12). Flathub and auto-update (Q29) wait on
  it: a private repo's releases can't be seen by installed copies.

## Needs a real phone

Everything below is built and passes the simulator, the WebDriver window tests and CI. Each needs one
check on a real Android phone with the stock KDE Connect app (the owner's Samsung).

- [~] Q35 **1.0 features on a real phone** (2026-11-16): phone as touchpad/keyboard (X11 XTest; Wayland
  through ydotool/wtype/dotool), presentation remote, run commands (list shows on the phone, `commandList`
  is a JSON string as upstream), lock, volume, ring this computer, the computer's battery on the phone,
  desktop players controlled from the phone (art included), pause/mute on calls, this computer's
  notifications on the phone, multi-file shares from the phone, the Files page (sftp), Contacts, typing on
  the phone (Remote keyboard), the readiness checklist, mDNS discovery on a network without broadcast.
  Done when: each works from the owner's phone, or its issue is filed here.
- [~] Q25/Q26b/Q34 **Phone-apps pairing on the real phone** (QR, 6-digit code, reconnect without mDNS):
  released in 0.3.1-0.3.2, checked with stand-ins. Done when: an app from the owner's Samsung opens in a
  window on the laptop.
- [~] Q38 **App windows on a real phone** (2026-11-16): one window per app (a second click says it's open),
  Close on the Apps page, an Android 9 phone sharing the "Phone screen" window, the package command shown
  on Arch/Fedora/openSUSE when scrcpy is missing. Checked with the stand-ins (`apps_e2e`, WebDriver).
  Done when: two apps from the owner's Samsung are open side by side and close from the Apps page.
- [~] Q31 **New window replaces an outdated service** (0.3.1). Done when: installing over a running older
  version shows the new behaviour without a reboot.
- [~] Q32/Q33 **Screenshot Copy/Save, Photos page**: checked with a stand-in sftp server. Done when: they
  work with "Filesystem expose" on the Samsung.
- [~] Q10 **Tray icon** on the owner's i3 and on KDE/GNOME (checked with stand-in trays). Done when: the
  menu, tooltip and badge look right on a real KDE panel, GNOME with the AppIndicator extension, and i3bar.
- [~] Q2 **Scheduled texts and quiet mode** (2026-11-19, branch `t3/scheduled-messages-quiet-mode`): Send
  later in the composer (service sends at the time, waits for the phone if it's away), quiet mode summary
  pop-ups. Checked with unit tests and the fake phone in the WebDriver test. Done when: a text scheduled
  for a few minutes ahead arrives from the owner's Samsung with the window closed, and a quiet-mode
  summary shows on the owner's desktop and opens the notifications.
- [~] Q29 **Auto-update** goes live once the repo is public (Q17a).

## Next

- [ ] Q4 **Calls on the PC**: Bluetooth hands-free (BlueZ + PipeWire HFP, the computer as the headset) for
  audio, answer, decline, hang up and dial; call banner in the window. KDE Connect only reports calls, so
  this is a separate channel (see docs/decisions.md). Done when: a call to the phone is answered and heard
  on the laptop. Status 2026-11-19: built on PipeWire 1.4's `org.pipewire.Telephony` (calls.rs, call
  banner, Call buttons, Ctrl+K "Call a number", setup steps); passes a fake telephony service in unit
  and window tests (`testing/ui/test_calls.py`). Waits on the owner: pair the phone with the laptop over
  Bluetooth and try a real call (sound both ways, answer, decline, dial). No mute yet (the API has none;
  next: mute the laptop microphone through PipeWire).
- [ ] Q17 **Flathub**: the Flatpak builds and ships with releases; left: source from a tag, bundled adb +
  scrcpy, Dynamic Launcher for pinned apps, screenshots, lint, then the Flathub PR (docs/flathub.md, "Work
  left"). Needs Q17a.
- [ ] Q37 **Window CI job blocking**: the WebDriver job in `.github/workflows/ci.yml` is still
  `continue-on-error`; make it required once it has passed reliably for a week.

## Later

- [ ] Q13 **Drag-and-drop send** verified for real (needs a session with a window manager or the owner).
- [ ] Q18 **Real app icons for every app**: icons now come from notifications; the rest show letters. Next:
  pull icons from the phone over adb.
- [ ] Q23 **Dialogs**: the command palette as a proper combobox for screen readers (focus return is done).
- [ ] Q24 **Phone-app workspaces** (idea): "Work" opens several pinned phone apps at once; dropping a file
  on a pinned app opens it in that app on the phone.
- [ ] Q16 **Forked Android app** only if a feature can't go through KDE Connect (for example call control
  without Bluetooth).

## Done

- 2026-11-16 v1.1.0: phone apps open once each in their own window and close from the Apps page, Android 9
  phones share one "Phone screen" window (#8). Tray polish: right-click menu (Open Tether, Ring my phone,
  Send files, Pause notification pop-ups, Quit Tether) over dbusmenu and as our own menu on XEmbed bars;
  tooltip with phone, battery and unread count; NeedsAttention with unread; one-colour icon on GNOME/KDE
  panels; no X11 on Wayland; unread count on the dock icon; Settings note when GNOME has no tray (#9,
  `testing/ui/tray_e2e.sh`, `tray_xembed.sh`). Q36 take a photo with the phone's camera
  (`kdeconnect.photo.request`, shown only for phones that still offer it; saved to Downloads and shown) and
  Q14 notification app icons with a letter when the icon is missing (#10, stand-in phone only). Flatpak:
  manifest `packaging/flatpak/io.github.mh_anwar.Tether.yml`, a `flatpak` release job that attaches
  `Tether-x86_64.flatpak` (in SHA256SUMS), start at login through the Background portal, tray name under the
  app id, the bundle points at the releases page for updates, and options the sandbox can't deliver are
  hidden (#11, docs/flathub.md).
- 2026-11-16 v1.0.0: every KDE Connect feature people expect, both directions. Phone controls the computer
  (run commands, touchpad and keyboard, presenter, lock, volume, ring this computer, battery to the phone,
  #2); desktop players on the phone, pause on calls, this computer's notifications on the phone, complete
  sharing from the phone (#3); Files, Contacts, Keyboard and Devices pages, several phones, narrow layouts
  Q12 (#4); mDNS discovery Q7 and untrusted-input limits Q22 (#5); phone readiness checklist Q21 (#6).
  KDE Connect phone simulator (`tether_core::sim`) with an end-to-end suite against the real service, and
  CI on every PR (fmt, clippy, tests, WebDriver window tests) (#1, Q30).
- 2026-11-12 v0.3.4: Settings screen (Q11), tray icon (Q10).
- 2026-11-12 v0.3.3: text cache (Q28), screenshot Copy/Save (Q32), window speed (Q27), Photos page (Q33).
- 2026-11-12 Q27: window speed with 500 conversations and 1500 contacts (`testing/ui/perf_window.py`): one
  incoming text 540 ms -> 34 ms.
- 2026-11-12 v0.3.2: paired phone apps reconnect without mDNS (Q34).
- 2026-11-12 v0.3.1: phone found past firewalls, 6-digit-only code pairing, "Show details", new window
  replaces an outdated service.
- 2026-11-12 v0.3.0: auto-update, phone-apps pairing fixes.
- 2026-11-12 v0.2.0 released; owner installed it, paired the Samsung, texts and notifications work.
- 2026-11-11 Phone apps (Apps page, QR pairing, helper download, pin to the desktop's app menu),
  positioning, Flathub store listing.
- 2026-11-11 Packages (CI), start at login, warning + takeover when KDE Connect holds the port.
- 2026-11-11 Clipboard sync in the daemon; reconnect after sleep and network changes.
- 2026-11-11 Media: player card, `tether media`, phone's player on desktop media keys (MPRIS).
- 2026-11-11 Picture messages: thumbnails in threads, full picture or video on click, save to Downloads.
- 2026-11-11 Protocol core: discovery, TLS, pairing, payloads, plugins; verified against KDE Connect 26.08.2.
- 2026-11-11 Daemon with local API, CLI, desktop notification mirroring, auto-redial.
- 2026-11-11 Messages window with notifications column, codes, Ctrl+K, drop to send, dark theme.
