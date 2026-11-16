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

## Desktop media and sharing
`CARGO_TARGET_DIR=target dbus-run-session -- uv run --no-project --with dbus-next --with python-xlib python testing/media/e2e.py`
uses a private bus, desktop player/notification server (`testing/media/standin.py`) and the real fake
phone. Build `tether`, `tether-app` and the `fake_phone` example first. It checks MPRIS list/metadata,
commands, property signals, art payload, call pause/resume, notification forwarding/dismissal and
shared files/URLs/text. It prints its temporary artifact folder. `FAKE_PHONE_CONTROL` enables an opt-in
fake-phone test inbox: numbered JSON packets, optional top-level `testPayloadPath`; received packets
are appended to `received.jsonl` and payloads saved as `payload.bin`. No real phone data is used.

It also covers the app blocklist, that mirrored phone notifications are not sent back, and the
"pause when talking" + "mute instead" call modes. `testing/media/window.sh [shot.png]` runs the window
checks (received-files list with Open / Show in folder and progress, the new Settings rows, Ctrl+K "Send
text or link") on its own display (`:72`), its own daemon port and `TETHER_UI_DIR`, so it works while another
daemon owns UDP/TCP 1716 and other tauri-drivers run (`WD_PORT`, `MEDIA_DISPLAY` override the defaults).
`xdg-open` is faked by a script in the e2e's PATH; clicking Open / Show in folder on the desktop
notification itself (needs a notification server that emits ActionInvoked) is not automated.
## Computer controls (stand-in phone)

With `CARGO_TARGET_DIR` pointing at your private target directory:

```sh
box heavy -- cargo build -p tether -p tether-app
box heavy -- cargo build -p tether-core --example fake_phone
uv run --no-project --with python-xlib python testing/computer/test_computer.py
```

The test owns Xvfb :71 if unused, a private daemon under `/tmp/tw-computer/protocol`, and a paired TLS
stand-in phone. It checks configured command execution, XTest pointer movement/presenter, keyboard
acknowledgements (including Unicode), helper-based lock/volume, ring toggling/stop, and all permission
switches. Mock loginctl/pactl/sound helpers are prepended to PATH. `FAKE_COMPUTER_DIR` on fake_phone
opts into append-only send.jsonl / received.jsonl packet control, isolated from normal demo behaviour.
The real-window `test_window.py` also checks adding, editing and removing a command.

For the focused editor check after that protocol test has seeded a paired device:

```sh
TETHER_UI_DIR=/tmp/tw-computer/protocol TETHER_SKIP_PHONE=1 WD_DISPLAY=:71 WD_PORT=14571 testing/ui/wd.sh testing/ui/test_commands.py
```

`TETHER_SKIP_PHONE=1` avoids connecting the UI stand-in to another daemon on shared port 1716;
`TETHER_PHONE_ADDRESS` can select an isolated daemon for broader window tests.

Looks: with the window running under Xvfb and the stand-in phone paired to that daemon, append
`{"id":1,"type":"kdeconnect.findmyphone.request","body":{}}` to `$FAKE_COMPUTER_DIR/send.jsonl` to show the
"finding this computer" card (Stop ends it), then `testing/ui/shot.py` as usual. Check Settings > "What your
phone can do on this computer" and "Commands your phone can run" the same way.

## Scriptable protocol peer and daemon integration tests

`cargo test --workspace` includes `crates/tether/tests/protocol_e2e.rs`: real daemon processes with
independent `/tmp/ts-*` XDG directories, random `TETHER_PORT` TCP/UDP ports, real TLS and real payload
sockets. No physical phone, display, D-Bus session, or Docker is required. Each daemon is killed and
reaped even when a test fails. Clipboard sync and desktop notification popups are disabled; incoming
clipboard content is asserted through API events. The suite includes the real 30-second pairing timeout
and normally takes around 40 seconds after compilation. Run just it with:

```sh
box heavy -- cargo test -p tether --test protocol_e2e -- --nocapture
```

On another machine omit `box heavy --`. `TETHER_PORT` must be a nonzero u16;
when set the daemon binds that exact TCP and UDP port, with no silent fallback. Normal installations
retain the standard ports. Discovery still accepts only protocol-standard advertised peer TCP ports.
The simulator temporarily uses a free loopback listener in 1717–1738 for discovery and the standard
1739–1764 payload range. Tests run concurrently without binding the host's standard discovery port.

Coverage: identity, UDP identity delivery and `connect` announcements, pairing in both directions,
rejection, expiration, unpair, ignoring untrusted traffic, certificate pin changes, ping, battery/signal,
notification list/reply/action/dismiss/icons, SMS threads/thread/send/attachment download, contacts,
clipboard API in both directions, share text/URL/files in both directions, ringing/mute, phone MPRIS
controls/album art, and dropped-link reconnect. UDP identity delivery is unicast to loopback because
broadcast does not loop back on some hosts; actual Wi-Fi discovery still needs a network compatibility
check. SFTP starts `testing/phone-files/serve.sh` and checks listing/download when user-level `sshd` and
`sftp` work; otherwise it prints a clear `SKIP` with the reason (`--nocapture` shows it). Hardware clipboard
ownership and Android permissions are covered separately above. New feature branches should add cases
for their new API methods when merged.

The old screenshot command and seeded responses still work:

```sh
cargo run -p tether-core --example fake_phone -- 127.0.0.1:1716 /tmp/phone /tmp/phone-control.sock
```

The optional third argument opens a Unix socket. Connect using a JSON-lines client (e.g. `socat -
UNIX-CONNECT:/tmp/phone-control.sock`). Each command gets `{"result":...}` or `{"error":"..."}`:

```json
{"command":"send","packet":{"type":"kdeconnect.battery","body":{"currentCharge":42,"isCharging":true}}}
{"command":"send_payload","packet":{"type":"kdeconnect.share.request","body":{"filename":"hello.txt"}},"bytes":[72,105]}
{"command":"wait_for","type":"kdeconnect.ping","match":{},"after":0,"timeout":5000}
{"command":"received"}
{"command":"pair"}
{"command":"accept"}
{"command":"reject"}
{"command":"unpair"}
{"command":"disconnect"}
{"command":"reconnect"}
```

`timeout` is milliseconds; `after` is a zero-based history offset. `match` compares body keys exactly
(a shallow subset). `received` keeps history across reconnects and returns `packet`, `payload` (byte
array), and `payload_error`. `identity`, `connect {address}`, and `discover {address}` are also supported.
Use a fresh directory to reset device identity/certificate and pairing. Socket paths must be short.
The screenshot wrapper automatically redials; use the Rust API for deterministic disconnect tests.

To add a case, copy `plugins_and_payloads` and use `tests/support/mod.rs`:

```rust,ignore
let api = start_daemon().await?;
let sim = api.paired().await?;
let after = sim.received().len();
api.device_call(&sim, "ring", json!({})).await?;
sim.wait_for("kdeconnect.findmyphone.request", json!({}), after, WAIT).await?;
sim.send(Packet::new("kdeconnect.battery", json!({"currentCharge":42}))).await?;
api.until("device_data", json!({"device":sim.identity().device_id}),
    |data| data["battery"]["charge"] == 42).await?;
```

`Simulator` lives in `tether_core::sim`; Android-style seeded request handlers live separately in
`sim/scenario.rs`, so new plugins can add responses without changing connection/control code.
Prefer `until`/`wait_for` with an observable assertion over fixed sleeps. Snapshot `received().len()`
before a command so old packets cannot satisfy a new assertion. Every API call has a deadline.

CI (`.github/workflows/ci.yml`) checks formatting, all-target clippy, and all workspace tests on Ubuntu
22.04. A separate non-blocking WebDriver job installs tauri-driver/WebKitWebDriver, seeds pairing and
runs `testing/ui/wd.sh` under Xvfb and a private D-Bus session; its logs are uploaded on failure.
