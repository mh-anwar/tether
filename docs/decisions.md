# Decisions

- **New desktop side, not a fork.** KDE Connect's desktop app is tied to KDE Frameworks, Valent to GNOME; the
  goal is one app for every desktop. The phone keeps the stock KDE Connect app, so protocol compatibility is the
  contract. A fork of the Android app is reserved for features the protocol can't carry (call control).
- **Rust, rustls with `ring`.** No OpenSSL or CMake needed to build; `ring` avoids aws-lc's build tools.
- **Accept any certificate in the TLS handshake, pin afterwards.** KDE Connect has no CA; trust is the
  certificate pinned at pairing. Handshake signatures are still checked so the peer must own its key.
- **Daemon + local socket API.** The phone link must survive the window being closed; the UI and CLI are
  clients of `tether daemon` over a user-only Unix socket.
- **Active redial.** Paired phones that drop are redialed at their last address every 15 s, with a unicast
  announcement, instead of waiting for broadcasts. Broadcast doesn't cross VPNs and is flaky on some Wi-Fi.
- **Calls and screen mirroring are separate channels.** KDE Connect only reports calls; audio and answering
  go over Bluetooth hands-free (the way Phone Link does it). Screen and apps use Android's wireless
  debugging (scrcpy-style), which needs no root.
- **Calls through PipeWire's telephony D-Bus service, not oFono or our own HFP stack.** PipeWire 1.4
  (March 2025) speaks HFP as the hands-free unit itself and publishes `org.pipewire.Telephony` on the
  session bus, on by default (native backend, roles `hfp_hf` + `hfp_ag` by default). The sound already
  goes through the desktop's PipeWire, so there's nothing to route. oFono would need a system service,
  root setup and its own audio path; talking HFP ourselves would fight PipeWire for the RFCOMM profile.
  Cost: needs PipeWire 1.4+ (Ubuntu 25.04, Fedora 42, Debian 13, Arch); older systems get setup steps.
- **Window: Tauri with plain HTML/CSS/JS, no npm or bundler.** Uses the system WebKitGTK (small binary), and
  the UI builds with cargo alone. Phone text is only ever inserted as text, never as HTML.
- **The window is a client of the daemon** and starts it if needed; the daemon holds an exclusive lock
  (`daemon.lock`) so two copies can never share one identity.
- **Packages built on GitHub Actions, Ubuntu 22.04** (`.github/workflows/release.yml`): this box has glibc
  2.43, so binaries built here won't run on most laptops. Tauri's bundler makes .deb, .rpm and AppImage; a
  tarball with `packaging/install.sh` covers installs without admin rights.
- **Start at login by default**, like Phone Link: systemd user unit (i3 and other bare WMs don't run XDG
  autostart), falling back to XDG autostart. Not started on enable: the window's daemon may be running.
- **Clipboard sync lives in the daemon**, not the window, so it works with the window closed. The window
  only copies itself when the daemon reports it can't.
- **Wake/network detection by polling /proc**, not logind or NetworkManager D-Bus signals, so it behaves the
  same on every desktop.
- **Build the same package set every time** (`-p tether -p tether-app`): building them separately changes
  dependency features and recompiles everything (5 min instead of 1 s).
- **Phone apps through adb + scrcpy, not our own video pipeline.** scrcpy already starts an app on a
  virtual display (`--new-display --start-app`, scrcpy 3.0+, Android 10+) and opens a fast native window
  with sound; rewriting that buys nothing. Tether owns the parts scrcpy lacks: QR pairing, finding the phone,
  the app list, errors in plain words. Helpers come from `TETHER_ADB`/`TETHER_SCRCPY`, our download, next to
  the binary, then PATH.
- **Proposed, waiting on the owner (2026-11-19): phone apps without wireless debugging.** The owner finds
  Developer options and Wireless debugging too much to ask. Options weighed:
  - A small optional companion app, "Tether Screen" (recommended). MediaProjection streams the screen or
    a single app (Android 14+ can share one app), an AccessibilityService taps and types, and launching an
    app is an ordinary intent. It pairs through the existing KDE Connect trust (a QR from the Apps page), so
    there's no Developer options and no reconnect after a reboot or network change. Costs: a second app on
    the phone (KDE Connect stays as it is for everything else); Android 14+ asks "Start now" on the
    phone each time a sharing session starts (Phone Link skips this only because Samsung preinstalls it
    with system rights); and Play Store review of accessibility use, so F-Droid and a direct download first.
    Keep adb + scrcpy as an "Advanced" path for people who already use it.
  - Ruled out: Shizuku (needs wireless debugging or root to start), a USB cable once then `adb tcpip`
    (still Developer options, lost on reboot), Samsung DeX or Link to Windows (closed, Windows only),
    KDE Connect itself (its Android app has no screen sharing).
  - Cheap stopgap if the owner wants it first: after the first pairing, keep adb listening on a fixed port
    so Wireless debugging doesn't have to be switched on again until the phone reboots.
- One window per app, and Android 9 still works. A second click on an open app doesn't start another
  virtual display (each costs the phone memory and its own encoder); it says the app is open. Phones
  without virtual displays (before Android 10) get the apps on the phone's own screen in one shared
  window rather than an error, and the Apps page says so.
- **One-click helper download, pinned and checksummed** (scrcpy 5.0.1 static build, includes adb), because
  Ubuntu 24.04 and Debian 12 ship scrcpy 1.25, too old for app windows. Flatpak will bundle them instead.
- **Ask the phone directly as well as adb's multicast DNS**: we already know its IP from KDE Connect, and a
  reply to our own unicast query passes stateful firewalls that drop multicast. Hand-written DNS (~150
  lines) rather than a DNS crate: only PTR and SRV are needed.
- **Updates verified by the release's SHA256SUMS over HTTPS, not signed**: the checksum catches a damaged
  download; trust rests on GitHub and TLS, the same as the manual download it replaces. Signing (minisign)
  is the next step if releases ever come from elsewhere. Updates use curl/wget rather than an HTTP crate,
  like the helper download, to keep the daemon small.
- **Self-update only where no password is needed** (AppImage, `install.sh` folder); packages get their
  installer opened, never a silent `sudo`.
- **Several adb devices and none at the phone's address: refuse**, never guess, so apps can't open on an
  emulator or another phone while the window names yours.
- **Positioning**: Tether's reason to exist is what other KDE Connect desktops can't do (phone apps, calls),
  not a nicer copy of what they do. See positioning.md.
