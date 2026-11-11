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
- **Build the same package set every time** (`-p tether -p tether-app`): building them separately changes
  dependency features and recompiles everything (5 min instead of 1 s).
