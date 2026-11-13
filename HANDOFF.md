# Handoff (2026-11-12)

Read first: `AGENTS.md` (house rules), `QUEUE.md` (work queue), `docs/README.md` (knowledge base index).
Released: **v0.2.0** (github.com/mh-anwar/tether/releases/tag/v0.2.0, private repo). `main` = what shipped.

## Owner's feedback on v0.2.0 with the real Samsung (top priority, in QUEUE "Now")

Pairing the phone, texts and notifications work through the stock KDE Connect app, which the owner likes
(nothing to install on the phone; keep it that way). Problems they hit:

1. **The window feels slow.** Not measured yet. Suspects, none verified:
   - `ui/app.js` re-renders whole regions with `swap()` on every event; one `messages` packet with
     hundreds of threads rebuilds the full list (`renderList`, `renderThreadPane`). Batch with
     `requestAnimationFrame` and render only what changed.
   - `loadDevice()` waits for `device_data`, `threads` and `contacts` together before showing anything.
   - The window's daemon connection retries every 10 s if the daemon isn't up yet (`crates/tether-app`).
   Measure first: time from window open to first paint, and per-event render time with 500 threads
   (extend `crates/tether-core/examples/fake_phone.rs` to make that many).
2. **Notifications and messages take a while to appear.** Likely the phone's side: Tether asks for
   conversations on connect and the phone answers slowly, and the window shows nothing until then. Ideas:
   cache the last thread list on disk (as of today messages are memory-only, docs/architecture.md) so
   the window opens full; show "Loading from your phone" with what's cached meanwhile.
3. **QR pairing for phone apps failed on the real phone** (2026-11-11 session: direct unicast query to the
   phone's port 5353 added in `mdns_probe.rs`, plus `tether apps doctor`; needs the real phone to confirm.
   If the doctor shows "no answer" from the phone directly and adb hears nothing, the next fallback is
   code pairing, which now needs only the 6 digits.) (same Wi-Fi). Unknown what the phone showed.
   Ask the owner: did the phone say "Pairing" then fail, or did nothing happen after scanning?
   Facts: we ship adb 37 (scrcpy 5.0.1 bundle); its mDNS discovery (Openscreen) is on by default
   (`client/transport_mdns.cpp`, `ADB_MDNS_OPENSCREEN` only switches it off). Hypotheses:
   - The laptop's firewall drops multicast DNS (UDP 5353) or the phone's pairing port: adb never sees
     `_adb-tls-pairing._tcp`. Check `adb mdns check` / `adb mdns services` on the owner's laptop.
   - An older adb from the distro (Ubuntu ships adb 34) runs the adb server; our newer client should
     replace it, but check.
   - Our match is by instance name (`tether-xxxxxx`); some Android builds may advertise differently.
     Log what `adb mdns services` returns while waiting (without the password) so a failure is diagnosable.
   Without mDNS there is no way to find the pairing port (it's random), so the fallback is the
   pairing-code path (point 4), made easy.
   Add a "Diagnostics" view (or `tether apps doctor`) that prints helper paths/versions, `adb mdns check`,
   seen services and the KDE Connect phone IP.
4. **Pairing with a code is too hard.** (2026-11-11 session: done as described below, checked with stand-ins.) Typing IP:port plus a code is developer stuff. Fix: prefill the
   IP from KDE Connect (`phone_ips`), ask only for the port and the 6 digits, big inputs, paste support,
   and plain words ("the numbers after the colon"). Better still, avoid it by fixing QR (point 3).
5. **Wants an icon in the status bar / tray** (QUEUE Q10). On i3 the bar shows only XEmbed tray icons;
   Tauri's tray is StatusNotifierItem (AppIndicator). GNOME needs the AppIndicator extension. Plan: SNI via
   Tauri's tray (KDE, Sway/waybar, GNOME with extension) and check i3bar; if i3bar can't show it, offer
   `tether status --i3blocks` style output for bar modules.

## State of the code (verified on this box, not on the phone unless said)
- Protocol core, pairing, files, clipboard, media, SMS incl. pictures: verified against real KDE Connect
  26.08.2 in Docker and the owner's phone (pairing, texts, notifications).
- Phone apps (`crates/tether/src/apps.rs`, Apps page in `ui/app.js`): verified only with stand-ins
  (`testing/apps/adb`, `testing/apps/scrcpy`); the helper download was verified for real.
- Pinned phone apps write `~/.local/share/applications/tether-app-<pkg>.desktop` with the app's icon.
- Advisor review (GPT-6.1 Sol) fixes are in: pairing cert swap, safe filenames, redial timing, etc.
  Remaining advisor items are QUEUE Q21-Q24.

## How to test here
`docs/testing.md`: fake phone + window under Xvfb (`testing/ui/run.sh`), stand-in adb/scrcpy, two
daemons on one box. No KVM here, so no Android emulator. Never run the real adb on this box (port 5037 is
shared with other users). Gotchas in `docs/gotchas.md` (e.g. don't `pgrep -f`/`pkill -f` a pattern that
appears in your own command line).

## Session 2026-11-11/12 (released as v0.3.0)
- Points 3 and 4 are built and checked with stand-ins; they wait on the owner's real phone.
- Auto-update (QUEUE Q29) shipped in 0.3.0; it can only see releases once the repo is public.
- Window tests now run over WebDriver (`testing/ui/wd.sh`, docs/testing.md); grow them (Q30).
- **Start with Q31, then Q26b** (owner feedback on 0.3.0 with a screenshot): the 0.3.0 window was talking
  to the still-running 0.2.0 background service, so the owner never saw 0.3.0's pairing fixes, and the code
  form shows too many fields. Q25 (QR) can only be judged after Q31 is fixed. Release 0.3.1 once both are done.
- Then: Q27 (slow window, measure first; write the timing as a WebDriver check), then Q28 (thread
  cache), then Q10 (tray).

## Session 2026-11-12 evening (released as v0.3.1, owner approved push + release)
Owner on 0.3.0: QR pairing for phone apps: phone stuck on "Pairing device" (it scanned; laptop never connected).
Owner rule: never expect people to run commands like `tether apps doctor`; everything from the window.
Built (workspace tests, clippy -D warnings, fmt pass; NOT yet WebDriver check or screenshot of the new code form):
- `mdns_probe::probe` also asks from `<our ip>:5353` with the QU bit; `open_ports` sweeps the phone's ephemeral
  ports; `apps::pair_by_sweep` tries the QR password / code on each open port (after ~4 s with no answer, every 8 s).
- `apps_pair_code` takes `address` optional; the code form is one 6-digit box (+ "Enter the address yourself").
- "Show details" (apps_doctor text + Copy details) under pairing errors in the window.
- Q31: daemon `identity` returns `version`; the window stops a service of another version (shutdown, or kill by
  the socket peer's pid if it's an old one without shutdown) and starts its own (`replace_if_outdated`).
Next: clippy + workspace tests, WebDriver check of the code form (testing/ui/wd.sh), Xvfb screenshot for the
owner, README (drop `tether apps doctor` as the user path). Wait for the owner's result with 0.3.1 on the real
phone (Q25/Q26b/Q31). Then Q32 (screenshot copy), Q33.

Delete this file once the five points above are done or moved into QUEUE/docs.

## Follow-up: paired, then stuck reconnecting (2026-11-11)
Owner confirmed the QR scan worked, then showed Apps stuck on “Reconnecting to your phone”. Found a
code gap: `finish_connect` and `maybe_autoconnect` still required mDNS even though pairing had a port
fallback. Both now use `connect_phone`, which tries the remembered verified address, advertised ports,
then direct port discovery on the known phone. No overlapping automatic jobs, one-minute sweep
cooldown, three-second connect attempts, online transport verification. Q34 tracks real-phone validation;
these changes are local and require a release before the owner can try them.

Validation this session: `cargo check -p tether -p tether-app`, combined binary build, targeted online
transport test, workspace tests, clippy with warnings denied, fmt check, and seven WebDriver checks
passed. The real window reached the app grid using stand-in adb/scrcpy, then reconnected after mDNS
connect announcements were hidden. Direct discovery on a real Android phone is still unverified.

## Session 2026-11-12 night (v0.3.2 released, owner approved push + release)
- 0.3.2 = the Reconnecting fix above (Q34). Waits on the owner's Samsung.
- Q28 built: texts and contacts cached on disk (`tether-core/src/cache.rs`); checked with the fake phone off.
- Q32 built: Copy/Save on screenshot notifications over the phone's sftp plugin (`tether/src/phonefiles.rs`),
  checked against a stand-in sshd (`testing/phone-files/serve.sh`), clipboard holds image/png. Needs
  "Filesystem expose" on the phone; real Samsung unverified (which folder, which notification text).
- Both are unreleased on `main`. Next: Q33 (phone files in the window, same sftp path), Q27, Q10.
- Later the same night (all unreleased on this branch, ahead of `main`): Q27 window speed (measured with
  `testing/ui/perf_window.py`), Q33 first screen: Photos page (`ui/app.js` photos section,
  `phonefiles.rs`). Next: owner's verdict on Photos, then drag-out, tray icon (Q10).
- Then Settings screen (Q11, done) and tray icon (Q10, `crates/tether/src/tray.rs`), released as 0.3.4.
  Owner granted full autonomy on 2026-11-12, releases included (memory: working-preferences).
  Next ideas: drag a photo out of the Photos page, Q21 phone readiness checklist, Q12 narrow layouts, Q4 calls.
