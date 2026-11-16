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

## Window tests over WebDriver (preferred)
`testing/ui/wd.sh` runs `testing/ui/test_window.py` against the real window: virtual screen `:59`, fake
phone, `tauri-driver` on port 14444 (it starts the window through WebKitWebDriver). Tests find elements by
role and text and report every step, so layout changes don't break them and nothing needs eyeballing.
The checks include pairing through to the app grid and reconnecting with the connect announcement hidden.
Each run resets phone-apps state (stand-in adb/scrcpy, not paired for apps).
```sh
box heavy -- cargo build -p tether -p tether-app && box heavy -- cargo build -p tether-core --example fake_phone
testing/ui/run.sh start && testing/ui/run.sh accept && testing/ui/run.sh stop   # first time only: pair the fake phone
testing/ui/wd.sh                       # or: testing/ui/wd.sh path/to/other_test.py
```
Setup on a new machine: `cargo install tauri-driver --locked`; WebKitWebDriver is the package
`webkitgtk-webdriver` on Debian/Ubuntu (`webkit2gtk4.1-devel` ships it on Fedora). Add a check to
`test_window.py` for every new screen or flow. Screenshots (below) are still how to judge looks.

## Tray icon
`testing/ui/trayhost.py :57` is a stand-in bar tray (XEmbed): start it next to `run.sh start`, the icon docks
within 5 s at the top right of the virtual screen; click it with `drive.py :57 click 1230 30`.
`testing/ui/sniwatcher.py` is a stand-in desktop tray (StatusNotifierWatcher): run it with the service's
`DBUS_SESSION_BUS_ADDRESS` (from `/proc/<service pid>/environ`); it prints the title and icon sizes, calls
Activate, and the XEmbed icon leaves the bar while it runs.

## Window speed
`testing/ui/wd.sh testing/ui/perf_window.py` fills the window with 500 conversations and 1500 contacts and
times each kind of update, layout included; it fails if one incoming text takes over 50 ms. Run it after
changing how lists render (`swap`, `renderList`, `invalidate` in `ui/app.js`).

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

## Phone files (sftp)
`testing/phone-files/serve.sh WORKDIR DATA_HOME [PORT]` runs a user-level sshd that accepts the test
service's key and serves two screenshots; it prints the `FAKE_SFTP` value that makes `fake_phone` answer
`kdeconnect.sftp.request`. `wd.sh` starts it by itself once the service's key exists (from the second run
on) and checks Copy on the screenshot notification, the Files page (browse, create, rename, upload, save,
delete; the stand-in starts from a clean folder each run) and the Photos page.

## Updates (nothing published)
`testing/update/fake-release.sh DIR PORT [VERSION]` serves a stand-in GitHub release (latest.json, the
no-admin tarball built from the debug binaries with a marker line appended, SHA256SUMS). Set
`"update_url": "http://127.0.0.1:PORT/latest.json"` in the test config. Binaries under `target/` never
update, so copy them to a folder that mimics an install and point the UI script at it:
`CARGO_TARGET_DIR=/tmp/x testing/ui/run.sh start` with `/tmp/x/debug/{tether,tether-app,examples/fake_phone}`.
Then Ctrl+K, "Check for updates", Update, Restart Tether; `tail -c 40 /tmp/x/debug/tether` shows the marker.

## Phone apps (no phone needed)
`testing/apps/adb` and `testing/apps/scrcpy` stand in for the real helpers; their state is files in
`$FAKE_APPS_DIR` and every call is logged to `calls.log`. Export before `run.sh start` (the window starts the
daemon, which inherits them):
```sh
export FAKE_APPS_DIR=/tmp/tether-ui-$USER/apps TETHER_ADB=$PWD/testing/apps/adb TETHER_SCRCPY=$PWD/testing/apps/scrcpy
```
Act as the phone scanning the QR: decode it from a screenshot (`uv run --no-project --with zxing-cpp --with
pillow`), write `"<name> <password>" > $FAKE_APPS_DIR/scanned`. Pair-by-code accepts `123456`. The package
`com.example.broken` fails like an old phone. `touch $FAKE_APPS_DIR/codeopen` acts as the phone showing its
pairing-code screen (the form then fills in the address). `TETHER_SCRCPY=` (empty) shows the helper-download screen.
Use the stand-ins here rather than the real adb: its server listens on localhost:5037, shared with everyone on the box.

## Window pages: Files, Contacts, Keyboard, Devices
`test_window.py` covers each, and writes light and dark screenshots of them plus 720 and 1000 px layouts to
`/home/pippin/tether-wt/shots/` (edit `output` in the `screenshots` check for another folder). `wd.sh` starts
its own daemon and points the fake phone at the port it got (other agents' daemons hold 1716-1718), and sets
`FAKE_MOUSEPAD_LOG`: every `kdeconnect.mousepad.request` the fake phone receives is appended there as JSON
and answered with an `isAck` echo. Run one `wd.sh` at a time per `TETHER_UI_DIR` and display.
