# Architecture

```
 Android phone (stock KDE Connect app)
        |  UDP 1716 announce, TCP 1716-1764 + TLS, payloads on 1739-1764
 tether-core  (crates/tether-core)    protocol, pairing, plugins; no UI, no global state
        |  Service API + broadcast events (Rust)
 tether daemon (crates/tether)         one per user, lock: $XDG_DATA_HOME/tether/daemon.lock
        |  JSON lines over $XDG_RUNTIME_DIR/tether/tether.sock (user-only)
 tether CLI   |   tether-app window (crates/tether-app + ui/)
```

## tether-core
- `identity`, `packet`: wire types. `cert`: self-signed cert, CN = device id; verification code.
- `tls`: accept any cert in the handshake, trust decided after by pinning (see decisions.md).
- `link`: TCP + TLS + v8 identity exchange, both roles. `payload`: file transfer sockets.
- `discovery`: UDP 1716 (exclusive bind). `store`: paired devices as JSON in `$XDG_DATA_HOME/tether/devices`.
- `service`: the state machine. One `Dev` per known device: identity, pinned cert, last address, live link,
  pair state, plugin data. Links are replaced, never duplicated; redials every 15 s while a paired device
  is away; pair requests expire after 30 s.
- `netwatch`: notices wake from sleep (boot clock vs monotonic clock) and network changes (routing table +
  outgoing address) by polling each 5 s tick; the service then redials, dropping links after long sleeps.
- `plugins/*`: pure functions from packets to `Update`s and reply packets, plus builders for what we send.
  Payload-bearing packets (files, icons) are handled in `service`, which owns the network.

## Daemon extras (crates/tether/src)
- `clipsync`: two-way clipboard sync on its own thread (arboard, polled each second). Guesses `DISPLAY` /
  `WAYLAND_DISPLAY` when started by systemd without them. `"clipboard_sync": false` turns it off.
- `apps`: phone apps in their own windows. Pairs Android wireless debugging (QR: `WIFI:T:ADB;S:name;P:pw;;`,
  then `adb mdns services` until the phone advertises that name, `adb pair`, `adb connect`), lists apps with
  `scrcpy --list-apps`, opens one with `scrcpy --new-display --start-app=<pkg>`. Pairing state is in memory;
  `$XDG_DATA_HOME/tether/apps-paired` marks that the phone was paired once (then it reconnects by itself).
  Discovery also asks the phone directly (`mdns_probe.rs`: a unicast DNS-SD query to `<KDE Connect phone
  IP>:5353`, answered per RFC 6762 6.7), so a firewall that drops multicast doesn't hide the phone. Code
  pairing uses the same search to fill in the pairing port. Changes in what's in sight are logged at info
  (`phone apps: services in sight`).
- `update`: once a day (first 90 s after start) asks GitHub's latest-release API (`update_url` in config),
  compares with the running version, and for self-updating installs (AppImage, an `install.sh` folder)
  downloads, checks the release's `SHA256SUMS` and renames the new files over the old (running copies keep
  the old inode). `.deb`/`.rpm`: on request, the package goes to Downloads and is opened. Dev builds
  (`target/`) and Flatpak never update. `exec.rs` holds the curl/wget and process helpers it shares with
  `apps`.
- `autostart`: `tether autostart on|off` writes a systemd user unit (XDG autostart fallback). The window
  turns it on at first launch and rewrites it each launch while on (follows a moved AppImage).

## Daemon API (crates/tether/src/api.rs)
Request `{"id","method","params"}`, reply `{"id","result"|"error"}`, events pushed as `{"event":...}`.
- Global: `identity` (plus `discoveryListening`: false while another KDE Connect app holds UDP 1716;
  `clipboardSync`: the daemon owns clipboard sync), `devices`, `refresh`, `connect {address}`.
- Phone apps (no device param): `apps_status` (helpers found, phase idle/waiting/pairing/connecting/
  downloading, error, QR as SVG while waiting, connected adb device, `phoneIp` from KDE Connect,
  `pairingAddr` once spotted), `apps_pair_qr`, `apps_pair_code {address, code}`, `apps_find_port` (code
  pairing: look for the phone's pairing port), `apps_doctor` (text report for `tether apps doctor`), `apps_cancel`, `update_status`, `update_check`,
  `update_install` (all return the update status: current, latest, kind, phase idle/checking/downloading/
  ready/installer, error, noReleases, installer path and command), `shutdown` (the window's restart after an
  update: the daemon exits, the window starts a fresh copy of itself, which starts the new daemon), `apps_download`, `apps_list`, `apps_open {package?, title}` (no package:
  the whole screen), `apps_menu` (pinned packages), `apps_menu_set {package, name, icon?, on}` (icon: a file from the
  notification-icon cache, copied to `tether/menu-icons/`) (writes
  `$XDG_DATA_HOME/applications/tether-app-<package>.desktop` running `tether apps <package>`; the daemon
  rewrites their Exec at start so a moved AppImage still works, via `Tether.AppImage --tether ...`). The window polls `apps_status` while the Apps page is open.
- Per device (`params.device`): `device_data`, `threads`, `thread {thread_id}`, `contacts`,
  `contact_name {number}`, `request_thread {thread_id, before?, count?}`, `request_threads`, `pair`, `accept`,
  `reject`, `unpair`, `ping`, `ring`, `mute`, `sms_send {addresses, body, sub_id?}`,
  `notification_dismiss {id}`, `notification_reply {reply_id, message}`, `notification_action {id, action}`,
  `clipboard {content}`, `share_url {url}`, `share_text {text}`, `share_file {path}` (resolves when done).
- Events: `{"event":"device", ...DeviceSummary}` and `{"event":"update","device_id","update":{"kind":...}}`
  with kinds battery, signal, notification, notification_removed, messages, call, call_ended, ping,
  clipboard, shared_text, shared_url, file_received, transfer_failed, contacts.

## Window (crates/tether-app, ui/)
- Rust side: keeps one socket connection, forwards `call(method, params)`, emits `tether` (daemon events) and
  `daemon` (`{connected}`) to the page, starts the daemon if missing (retries every 10 s). `copy_text` holds
  one clipboard handle for the app's life (X11 needs an owner).
- Page: `ui/app.js` holds all state in `state`, renders regions with `swap()` (keeps focus and typed text).
  Phone text is only set via `textContent`. Icons are inline SVG in `ui/icons.js`.
- Messages are cached in memory per thread; the phone is the source of truth and resends on connect.
