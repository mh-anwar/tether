# Tether

Your Android phone on your Linux desktop: texts, notifications, files, clipboard, and your phone's apps
in their own windows, on any desktop (GNOME, KDE, i3, Sway, and the rest).

Tether is a new desktop side for the KDE Connect protocol. On the phone you keep using the stock
**KDE Connect** app from Google Play or F-Droid; Tether replaces KDE Connect or Valent on the computer.

> Status: early. The background service, the command-line tool and the Messages window work.

## Install

Download from the [Releases page](../../releases) (x86_64):

- **Ubuntu, Debian, Mint, Pop!_OS**: the `.deb`. Open it, or `sudo apt install ./Tether_*.deb`.
- **Fedora, openSUSE**: the `.rpm`. `sudo dnf install ./Tether-*.rpm`.
- **Anything else**: the `.AppImage`. `chmod +x Tether_*.AppImage`, then run it.
- **No admin rights**: the `.tar.gz`. Unpack it and run `./install.sh` (installs into `~/.local`;
  `./install.sh --remove` uninstalls). It needs WebKitGTK 4.1 from your distribution.

Then open **Tether** from your app menu. It starts itself at every login from then on, so your phone stays
linked with the window closed. `tether autostart off` stops that.

**Updates**: Tether checks for a new version once a day. The AppImage and the no-admin install update
themselves (the window then offers a restart); `.deb` and `.rpm` installs get an Update button that saves
the new installer to Downloads and opens it. `"auto_update": false` in the config makes every update wait
for your click. `tether update` checks right away.

Quit KDE Connect or Valent first: they use the same network ports. Tether warns you if one is running.
If your firewall blocks them, open ports 1714-1764 (TCP and UDP).

### From source

Build dependencies (Debian/Ubuntu names): `pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev
libsoup-3.0-dev libjavascriptcoregtk-4.1-dev`.

```sh
cargo build --release -p tether -p tether-app
./target/release/tether-app           # the window; starts the background service itself
```

`tether daemon` runs the background service on its own (only one copy can run at a time). Packages are
built by `.github/workflows/release.yml` (push a `v*` tag, or run it by hand).

## Pair your phone

1. Install KDE Connect on the phone and open it, on the same Wi-Fi as the computer.
2. Open Tether. Your phone appears under "Connect your phone"; click **Pair**.
3. Accept on the phone. Both screens show the same code.

Or tap the computer's name in the KDE Connect app and click **Accept** in Tether. From a terminal:
`tether devices`, then `tether pair -d <phone name>` or `tether accept`.

If the phone never shows up (VPNs, guest Wi-Fi), run `tether connect <phone IP>`, or add the address to
`~/.config/tether/config.json`:

```json
{ "addresses": ["100.64.0.12"] }
```

## Use it

```sh
tether status                         # battery, signal, notifications
tether threads                        # text conversations
tether thread 42                      # one conversation
tether sms --to +15551234567 "On my way"
tether share ~/Pictures/photo.jpg     # or a https:// link
tether clip "some text"               # copy to the phone's clipboard
tether ring                           # find your phone
tether apps spotify                   # open a phone app in its own window (after setup in the window)
tether apps doctor                    # why phone apps can't find the phone
tether update                         # install a newer Tether now
tether watch                          # live events, as JSON lines
```

Phone notifications also appear as normal desktop notifications. Turn that off with
`"mirror_notifications": false` in the config.

## The window

- Messages: conversations with names from your phone's contacts, full history on demand, sending with a
  "Sending" state until the phone confirms.
- Notifications column: reply inline (WhatsApp and other messaging apps), run notification actions, dismiss
  on the phone.
- One-time codes from texts and notifications are pulled out with a Copy button.
- `Ctrl+K`: jump to a conversation, text a contact, ring or ping the phone.
- Drop files anywhere on the window to send them to the phone.
- Apps: open any phone app in its own window (with its sound on Android 11+), or the whole phone screen. Setup is once:
  turn on Wireless debugging on the phone and scan the QR code the Apps page shows. The pin on an app
  adds it to the computer's app menu (GNOME, KDE, rofi and others), so it starts like any other app. Needs Android 10 or
  newer and scrcpy 3.0 or newer, which the Apps page can download for you (or `TETHER_SCRCPY` and
  `TETHER_ADB` point at your own).
- Follows the system's light or dark theme.

## Development

`cargo run -p tether-core --example fake_phone -- 127.0.0.1:1716 /tmp/fake-phone` starts a stand-in phone
with made-up texts and notifications; accept it with `tether accept`.

## Layout

- `crates/tether-core`: the protocol (discovery, TLS, pairing, packets) and plugin logic.
- `crates/tether`: the daemon, its local API (`$XDG_RUNTIME_DIR/tether/tether.sock`) and the CLI.
- `crates/tether-app` + `ui/`: the window (Tauri, system WebKitGTK; plain HTML/CSS/JS, no bundler).
- `docs/`: protocol notes and design decisions.

License: GPL-3.0-or-later.
