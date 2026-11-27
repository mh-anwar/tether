# Handoff (live, 2026-11-26, branch `work/2026-11-26`)

Read first: `AGENTS.md` (house rules), `QUEUE.md` (work queue), `docs/README.md` (knowledge base index).
`main` = v1.1.0 plus #13/#14. This branch adds call features on top; draft PR #15 is open from it and
describes everything below. Work goes through PRs (branch, PR, green CI, squash-merge).

## Done on this branch (all checked with fakes, none on a real phone)

- Mute during calls: `calls/mic.rs` mutes the default microphone via `wpctl` (else `pactl`), reads it
  back each round, unmutes after the call only if Tether muted it. Window: Mute/Unmute button
  (aria-pressed, "Microphone off" text), Ctrl+K entry. API `call_mute {muted}`; status `can_mute`, `muted`.
- Keypad during an active call: AudioGateway1 `SendTones`, `call_tones {tones}`, `clean_tones`. Window:
  Keypad button opens a 12-key grid, typing digits works, Escape closes.
- Desktop pop-up for a ringing Bluetooth call with Answer / Decline / Open Tether (`calls/popup.rs`,
  over the watcher's own session connection, closed when the call stops ringing). KDE Connect's plain
  "Incoming call" pop-up is skipped while Bluetooth calling is ready (`calls::rings_here`).
- Switch between two calls: a waiting call's Answer holds the first (HoldAndAnswer); a held call's card
  has "Switch to this call" (`call_swap`, also in Ctrl+K). Mute shows only on the call being talked on.
- Decline with a text: Text button on a ringing card, three quick replies or your own, `sms_send`.
- Missed-call pop-up (KDE Connect telephony): "Send a text" (window event `compose`) and, when Bluetooth
  calling is ready, "Call back" (`calls::dial`).
- Tray: while a call is live, "Answer <caller>" / "Decline" or "Hang up on <caller>" and a "Mute the
  microphone" check row after Open Tether, and a first tooltip line (`TrayState.call`).
- Settings, Calls: "Calls through this computer" (Ready / Not set up yet + Show the steps, Call a number).
- `tether call [answer|decline|hangup|pickup|mute|here|keys N|<number>]`; README lists calls.
- Q23: command palette is an ARIA combobox.
- Test fixes: `tray_e2e.sh` expected a menu without "Quiet mode" (failed on main); the Commands editor
  window check could lose its Remove click during a re-render.

## How it's checked (all pass on the final build, 2026-11-26)

- `cargo test -p tether calls` (fake PipeWire + fake notification server + stand-in wpctl on a private
  bus), tray unit tests, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- `testing/ui/wd.sh testing/ui/test_calls.py` and `testing/ui/wd.sh` (full window suite).
- `testing/ui/tray_e2e.sh` (answers, mutes, hangs up from the tray), `testing/ui/tray_xembed.sh`.
- `python3 testing/calls/cli_e2e.py` (`tether call`), `python3 testing/calls/missed_e2e.py` (missed-call
  pop-up buttons through `testing/calls/fake_notifications.py`).

## In progress

- Nothing half-done.

## Up next

- Real-phone checks: everything in QUEUE "Needs a real phone", plus the new call features (keypad tones
  reach phone menus, Mute silences the far end, pop-up buttons on GNOME/KDE, Switch on a held call).
- API actions return before the watcher has re-read the calls, so `tether call` right after another
  action can see the old state (cli_e2e.py waits for it). Consider `self.refresh().await` in
  `Watcher::call_method`/`dial` instead of `poke`, once it's clear PipeWire updates its objects before
  replying.
- A "Text" button on the ringing call's desktop pop-up (decline + open the window on a new message);
  a Hold button for a single call once HoldAndAnswer with no other call is confirmed on real PipeWire.

Delete this file once the real-phone checks in QUEUE are done.
