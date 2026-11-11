# Tether

Your Android phone on your Linux desktop: notifications, texts, calls, files, clipboard, and (planned)
your phone's screen and apps, on any desktop (GNOME, KDE, i3, Sway, and the rest).

Tether is a new desktop side for the KDE Connect protocol. On the phone you keep using the stock
**KDE Connect** app from Google Play or F-Droid; Tether replaces KDE Connect or Valent on the computer.

> Status: early. The background service, the command-line tool and the Messages window work.

## Install (from source)

Build dependencies (Debian/Ubuntu names): `pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev
libsoup-3.0-dev libjavascriptcoregtk-4.1-dev`.

```sh
cargo build --release -p tether -p tether-app
./target/release/tether-app           # the window; starts the background service itself
```

`tether daemon` runs the background service on its own (only one copy can run at a time).

Quit KDE Connect or Valent first: they use the same network ports (UDP and TCP 1716).

Open ports 1714-1764 (TCP and UDP) if your firewall blocks them.

## Pair your phone

1. Install KDE Connect on the phone and open it, on the same Wi-Fi as the computer.
2. Run `tether devices`. Your phone shows up as "not paired".
3. Run `tether pair -d <phone name>` and accept on the phone. Both screens show the same code.

Or start pairing from the phone and run `tether accept`.

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
