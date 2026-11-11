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
  will go over Bluetooth hands-free (the way Phone Link does it). Screen and apps will use Android's wireless
  debugging (scrcpy-style), which needs no root.
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
