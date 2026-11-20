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
- `cache`: the newest 30 texts per thread and the contacts of each paired phone in
  `$XDG_DATA_HOME/tether/cache/<id>.json` (0600, deleted on unpair), written at most every 5 s tick after
  texts or contacts arrive and loaded at start, so the window opens full before the phone reconnects.
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
  `scrcpy --list-apps`, opens one with `scrcpy --new-display --start-app=<pkg>` and keeps one window per
  app (a second open returns `alreadyOpen`; Close on the Apps page kills that scrcpy). scrcpy gets `ADB`
  set to the adb Tether found. The phone's API level comes from `getprop ro.build.version.sdk` once per
  connection; below 29 (Android 9) apps start on the phone's own screen in the one "Phone screen" window,
  and later apps are brought to the front there with `adb shell monkey -p <pkg> -c ...LAUNCHER 1`. Pairing state is in memory;
  `$XDG_DATA_HOME/tether/apps-paired` marks that the phone was paired once (then it reconnects by itself).
  Discovery also asks the phone directly (`mdns_probe.rs`: a unicast DNS-SD query to `<KDE Connect phone
  IP>:5353`, answered per RFC 6762 6.7), so a firewall that drops multicast doesn't hide the phone. Code
  pairing uses the same search to fill in the pairing port. Changes in what's in sight are logged at info
  (`phone apps: services in sight`). If discovery is hidden, first connections and reconnects also try
  open ports at the connected KDE Connect phone's address (same fallback as pairing). Only an online
  adb transport counts as connected; its address is saved in `apps-connect-address` and retried later.
  Automatic reconnects do not overlap; port discovery runs at most once per minute per attempt.
- `update`: once a day (first 90 s after start) asks GitHub's latest-release API (`update_url` in config),
  compares with the running version, and for self-updating installs (AppImage, an `install.sh` folder)
  downloads, checks the release's `SHA256SUMS` and renames the new files over the old (running copies keep
  the old inode). `.deb`/`.rpm`: on request, the package goes to Downloads and is opened. Dev builds
  (`target/`) and Flatpak never update. `exec.rs` holds the curl/wget and process helpers it shares with
  `apps`.
- `phonefiles`: files from the phone's own file server (KDE Connect's sftp plugin, "Filesystem expose"
  on the phone). `Service::sftp_login` asks for it; the system `sftp` client logs in with our certificate's
  key (PEM in `data/tether/phone-files/key.pem`), the one-time password as fallback (`SSH_ASKPASS` runs
  `tether` itself, the password travels in an env var). First use: Copy/Save on screenshot notifications
  (desktop popup buttons and the window's notification card) fetch the newest file in
  `DCIM/Screenshots` or `Pictures/Screenshots`; Copy puts it on the clipboard as image/png.
  The Photos page lists the newest 60 pictures of `DCIM/Camera` and of the screenshot folders (`ls -1t`,
  images only), gets previews six at a time in one sftp session (full file downloaded, 360 px JPEG kept in
  `phone-files/thumbs/<hash of the phone path>.jpg`, turned upright from EXIF), and keeps the last 20
  full-size pictures in `phone-files/full`. The sftp login is reused for five minutes.
- `tray` (`tray/`): the tray icon, owned by the service. `sni.rs`: StatusNotifierItem over D-Bus (zbus) when
  `org.kde.StatusNotifierWatcher` is on the bus (KDE, GNOME's AppIndicator extension, waybar, Hyprland);
  works the same on Wayland. `xembed.rs`: otherwise an XEmbed icon (x11rb) docked into
  `_NET_SYSTEM_TRAY_S<n>` (i3bar, polybar), only in X11 sessions (no DISPLAY, `XDG_SESSION_TYPE=wayland` or a
  `WAYLAND_DISPLAY` without a session type means no X11 calls at all, so Xwayland isn't woken). Never both:
  KDE proxies XEmbed icons itself. The icon is the tray cut of the app icon (`icons/tray.png`, see `docs/logo/`) plus a red badge drawn in code (3x5 digits);
  pixmaps at 16-64 px. With nothing unread, `IconName` is `tether-tray-symbolic` on GNOME and KDE (a
  one-colour SVG the panel recolours for light and dark themes) and the installed app icon elsewhere, both
  served from `data/tether/tray-icons` through `IconThemePath`; with unread texts `IconName` is empty so the
  badged pixmap shows, `Status` is `NeedsAttention` and the attention pixmap is the badged icon. The tooltip
  lists each paired phone (battery, charging) and the unread count. The count is conversations whose newest
  text is incoming, unread on the phone and newer than what the window has shown (`seen_set` from the
  window, kept in `data/tether/seen.json`). Left click: `show_window` to connected windows; if none answers
  `window_shown` within 0.7 s, the service starts `tether-app`. Menu (`menu.rs`, one model for both):
  Open Tether, Ring my phone (one row per phone when several are connected), Send files... (opens the window's
  file picker; a window started for it fetches the request with `ui_pending`), Pause notification pop-ups
  (in memory until unticked or a restart; calls still pop up), Quit Tether (`quit` event closes windows, then
  the service exits 0, so systemd's Restart=on-failure leaves it stopped). SNI hosts get it over
  `com.canonical.dbusmenu` at `/MenuBar` (`LayoutUpdated` on change); XEmbed bars get an override-redirect
  window drawn with core X fonts, with pointer and keyboard grabs (arrows, Enter, Escape). The unread count
  also goes to docks as `com.canonical.Unity.LauncherEntry.Update` for each desktop entry that starts
  `tether-app` (resent every minute while non-zero). On GNOME with no tray at all, `settings_get` says
  `tray_missing` and the window's Settings shows a note until `tray_note_dismissed`.
- `autostart`: `tether autostart on|off` writes a systemd user unit (XDG autostart fallback); inside a
  Flatpak it asks the Background portal instead. The window turns it on at first launch and rewrites it
  each launch while on (follows a moved AppImage).
- `flatpak`: sandbox detection (`FLATPAK_ID`, `/.flatpak-info` and its `origin`) and the Background
  portal request. `identity` reports `flatpak`, and the window hides what the sandbox can't do (see
  docs/flathub.md).

## Daemon API (crates/tether/src/api.rs)
Request `{"id","method","params"}`, reply `{"id","result"|"error"}`, events pushed as `{"event":...}`.
- Global: `identity` (plus `discoveryListening`: false while another KDE Connect app holds UDP 1716;
  `clipboardSync`: the daemon owns clipboard sync), `devices`, `refresh`, `connect {address}`.
- Phone apps (no device param): `apps_status` (helpers found, phase idle/waiting/pairing/connecting/
  downloading, error, QR as SVG while waiting, connected adb device, `phoneIp` from KDE Connect,
  `pairingAddr` once spotted, `device.android` as "9"/"15", `ownWindows` false on Android 9 and older,
  `windows` open now, `installHint` a package command where the distro's scrcpy is new enough),
  `apps_windows` (`[{package, title}]`, package null for the phone screen), `apps_close {package?}`
  (no package: the phone screen window), `apps_pair_qr`, `apps_pair_code {address, code}`, `apps_find_port` (code
  pairing: look for the phone's pairing port), `apps_doctor` (text report for `tether apps doctor`), `apps_cancel`, `update_status`, `update_check`,
  `update_install` (all return the update status: current, latest, kind, phase idle/checking/downloading/
  ready/installer, error, noReleases, installer path and command), `shutdown` (the window's restart after an
  update: the daemon exits, the window starts a fresh copy of itself, which starts the new daemon), `apps_download`, `apps_list`, `apps_open {package?, title}` (no package:
  the whole screen), `apps_menu` (pinned packages), `photos_list {device}` (`folders: [{name, items: [phone path]}]`), `photos_thumbs {device, paths}` (phone path -> local preview or null), `photo_get|photo_copy|photo_save {device, path}`, `seen_set {seen: {thread id: date}}`, `window_shown` (tray click handshake), `ui_pending` (what the tray asked a newly started window to do, or null), `tray_extension_open` (opens the GNOME AppIndicator extension's page), `settings_get` / `settings_set {..}` (config.json keys `device_name`, `addresses`, `mirror_notifications`, `clipboard_sync`, `downloads_dir`, `auto_update`, `tray_note_dismissed`, plus `autostart`; other keys in the file are kept; returns `restart: true` when the service exits to reread them, the window starts it again), `screenshot_copy {device}`, `screenshot_save {device}` (returns `path`), `apps_menu_set {package, name, icon?, on}` (icon: a file from the
  notification-icon cache, copied to `tether/menu-icons/`) (writes
  `$XDG_DATA_HOME/applications/tether-app-<package>.desktop` running `tether apps <package>`; the daemon
  rewrites their Exec at start so a moved AppImage still works, via `Tether.AppImage --tether ...`). The window polls `apps_status` while the Apps page is open.
- Per device (`params.device`): `device_data`, `threads`, `thread {thread_id}`, `contacts`,
  `contact_name {number}`, `request_thread {thread_id, before?, count?}`, `request_threads`, `pair`, `accept`,
  `reject`, `unpair`, `ping`, `ring`, `mute`, `sms_send {addresses, body, sub_id?}`,
  `notification_dismiss {id}`, `notification_reply {reply_id, message}`, `notification_action {id, action}`,
  `clipboard {content}`, `share_url {url}`, `share_text {text}`, `share_file {path}` (resolves when done).
  `photo_take` (phone opens its camera; fails unless the phone lists `kdeconnect.photo.request`). Global:
  `photo_preview {path}` copies a picture from the downloads folder into `phone-files/full` so the window may
  show it (its asset scope doesn't include Downloads).
- Events: `{"event":"device", ...DeviceSummary}` and `{"event":"update","device_id","update":{"kind":...}}`
  with kinds battery, signal, notification, notification_removed, messages, call, call_ended, ping,
  clipboard, shared_text, shared_url, file_received, transfer_failed, contacts, photo_taken {path} (saved with
  received files), photo_cancelled.

## Window (crates/tether-app, ui/)
- Rust side: keeps one socket connection, forwards `call(method, params)`, emits `tether` (daemon events) and
  `daemon` (`{connected}`) to the page, starts the daemon if missing (retries every 10 s). `copy_text` holds
  one clipboard handle for the app's life (X11 needs an owner).
- Page: `ui/app.js` holds all state in `state`, renders regions with `swap()` (keeps focus and typed text).
  Phone text is only set via `textContent`. Icons are inline SVG in `ui/icons.js`.
- Messages are cached in memory per thread; the phone is the source of truth and resends on connect.

Phone tools: Files uses `files_list {device,path?}` and
`file_operation {device,action,path,target?,directory?}` (open/save/upload/mkdir/rename/delete),
sharing the Photos SFTP login cache. Listings batch metadata in one SFTP session; mutations surface
server errors. Open downloads a fresh copy and invokes xdg-open; Save uses the configured Downloads.
Contacts groups synced numbers by name and starts Messages with the selected number.
Keyboard sends ordered `keyboard {device,key?,specialKey?,shift?,ctrl?,alt?,super?}` calls;
core stores keyboard availability and emits keyboard_state/keyboard_echo updates. `tether type TEXT`
uses the same API. Native GTK file selection adds rfd with only its gtk3 backend.

## Desktop media and incoming shares
- `desktopmedia` watches session-bus MPRIS names, PropertiesChanged and Seeked. It excludes the
  `org.mpris.MediaPlayer2.tether` phone proxy. Phone requests reach it as authenticated `DesktopRequest`
  updates; the core never depends on D-Bus. Calls pause only playing desktop players and restore those
  players, or their saved per-player volume when mute-instead is enabled.
- `sendnotifications` uses a dedicated BecomeMonitor connection (eavesdrop fallback). Notify replies
  provide the actual notification IDs for phone dismissals. It is off by default; mirrored notifications
  carry `x-tether-mirrored` to prevent loops. An app-name blocklist is in Settings.
- `received` owns shared text's clipboard and handles file notifications and xdg-open with the window
  closed. The window keeps recent transfers in a "Received from your phone" section at the top of its notifications column (progress while receiving or sending); `received_list` recovers the
  daemon's recent completed files when reopening it. `share_files` sends one batch sequentially.
## Phone controls for this computer

`plugins/{runcommand,mousepad,presenter,lock,systemvolume,battery,findmyphone}.rs` contain wire
parsers/builders. Paired requests become `Update::Computer` and `computer.rs` performs desktop effects;
`remoteinput.rs` uses XTest on X11, dotool/ydotool on Wayland (wtype for keyboard-only use).
`computer.rs` polls lock, volume, command configuration and local battery every three seconds, pushing
changed state and sending a full snapshot on connect. Settings permissions are checked for every incoming
request; disabled requests are ignored and logged, including already-connected phones. Commands are
stored as `commands: [{id,name,command}]`; `commands_get` / `commands_set` and settings_get/settings_set
share validation. Commands execute as the logged-in user, without shell interpolation of packet fields.
`computer_status` restores the ring banner or pending commands-settings navigation in a new window;
`computer_ring_stop` stops ringing. Notification actions and a second phone request also stop it.

## Protocol simulator

`tether_core::sim::Simulator` owns a real TLS phone link, packet/payload history and a Unix JSON-lines
control socket. `sim/scenario.rs` holds the seeded Android-style request responses; `examples/fake_phone`
is the reconnecting screenshot wrapper. Daemon integration tests share a short-path XDG/process/API
fixture in `crates/tether/tests/support`; `TETHER_PORT` isolates daemon TCP and UDP listeners.

Phone setup: `readiness::phone_readiness` maps phone-direction capabilities to setup instructions. Both `devices` and `device_data` expose `incoming_capabilities`, `outgoing_capabilities`, and `readiness`. The `ready` flag means advertised, not verified permissions; Devices explains that limitation. Identity is retained with the trusted device and refreshed on reconnect. The window marks a feature "Working" only once its data has arrived (texts, notifications, contacts, battery, signal, players, keyboard state), because Android advertises plugins that are switched off too; "Nothing yet" shows the steps on the phone, "Not on this phone" means the phone never advertised it.
## LAN discovery and input limits

`tether-core::mdns` announces and browses `_kdeconnect._udp.local` alongside UDP identity broadcasts.
It shares the small DNS name codec with the adb probe, joins each active IPv4 interface, and refreshes
memberships and browsing every 15 seconds. Port 5353 uses reuse-address/reuse-port and multicast loopback
so Avahi can coexist. Settings > Finding your phone controls `mdns` (default true; restart applies it).
Both discovery paths use the same dial filtering and TLS/pinning path; TXT records never establish trust.
The shared link layer admits eight simultaneous unauthenticated exchanges across both TCP roles, each
with a ten-second deadline. Local API request lines are limited to 1 MiB before JSON parsing, without unbounded buffering.
