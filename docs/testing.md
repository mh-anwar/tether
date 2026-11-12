# Testing

Climb only as far as the change reaches (see AGENTS.md "Checks"). Build with
`box heavy -- cargo build -p tether -p tether-app` (always that pair, see gotchas.md).

## Unit tests
`box heavy -- cargo test --workspace`: handshake in both TLS roles, cert/id mismatch, payload roundtrip, SMS
parsing, vCards, file-name sanitising.

## Two Tether daemons on one box
Give each its own `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_RUNTIME_DIR` (short paths). The second can't bind
UDP 1716 and runs announce-only on TCP 1717. Broadcast doesn't loop back here, so join them with
`XDG_RUNTIME_DIR=... tether connect 127.0.0.1` from the second.

## Real KDE Connect (protocol compatibility)
`testing/kdeconnect-peer/` (README there): KDE Connect 26.08.2 in rootless Docker. For anything with
payloads (files), run Tether in a second container on the same Docker network; from the host, payload
connections can't reach in or come back.

## Clipboard sync
Two daemons as above, the first on its own Xvfb (`DISPLAY=:58`), the second with `"clipboard_sync": false`.
Phone to desktop: `tether clip "x"` from the second, then `testing/ui/clip.py :58 get`. Desktop to phone:
`tether watch` on the second, `clip.py :58 set "y" 4` and look for a `clipboard` update.

## Packages
`gh workflow run release.yml` builds them on GitHub; `gh run download <id>` fetches them. `install.sh` can be
tried with a temporary `HOME` (it falls back to XDG autostart there, since systemd can't see that home).

## Stand-in phone + the window (UI)
`crates/tether-core/examples/fake_phone.rs` speaks the real protocol with made-up texts, contacts and
notifications (WhatsApp and Spotify with app icons as payloads); sent texts are echoed back and answered
after 4 s.

```sh
box heavy -- cargo build -p tether -p tether-app && box heavy -- cargo build -p tether-core --example fake_phone
testing/ui/run.sh start            # or: start dark
testing/ui/run.sh accept           # first time only, pairs the fake phone
uv run --no-project --with pillow python testing/ui/shot.py :57 /path/to/shot.png
uv run --no-project --with python-xlib python testing/ui/drive.py :57 click 380 190 type "Hi" key Return
testing/ui/run.sh stop
```
Look at every screenshot you take. Unset `RUST_LOG` when calling `uv` (it changes uv's logging).
The window is 1200x780 at the top-left (no window manager); there is no drag-and-drop or resizing.

## Phone apps (no phone needed)
`testing/apps/adb` and `testing/apps/scrcpy` stand in for the real helpers; their state is files in
`$FAKE_APPS_DIR` and every call is logged to `calls.log`. Export before `run.sh start` (the window starts the
daemon, which inherits them):
```sh
export FAKE_APPS_DIR=/tmp/tether-ui-$USER/apps TETHER_ADB=$PWD/testing/apps/adb TETHER_SCRCPY=$PWD/testing/apps/scrcpy
```
Act as the phone scanning the QR: decode it from a screenshot (`uv run --no-project --with zxing-cpp --with
pillow`), write `"<name> <password>" > $FAKE_APPS_DIR/scanned`. Pair-by-code accepts `123456`. The package
`com.example.broken` fails like an old phone. `TETHER_SCRCPY=` (empty) shows the helper-download screen.
Don't run the real adb here: its server listens on localhost:5037, shared with everyone on the box.
