# Handoff (live, 2026-11-26, branch `work/2026-11-26`)

Read first: `AGENTS.md` (house rules), `QUEUE.md` (work queue), `docs/README.md` (knowledge base index).
`main` = v1.1.0 plus #13/#14. This branch adds call features on top; draft PR open from it (see `gh pr
list`). Work goes through PRs (branch, PR, green CI, squash-merge).

## Done on this branch (all checked with fakes, none on a real phone)

- Mute during calls: `calls/mic.rs` mutes the default microphone via `wpctl` (else `pactl`), reads it
  back each round, unmutes after the call only if Tether muted it. Window: Mute/Unmute button
  (aria-pressed, "Microphone off" text), Ctrl+K entry. API `call_mute {muted}`; status `can_mute`, `muted`.
- Keypad during an active call: AudioGateway1 `SendTones`, `call_tones {tones}`, `clean_tones`. Window:
  Keypad button opens a 12-key grid, typing digits works, Escape closes.
- Desktop pop-up for a ringing Bluetooth call with Answer / Decline / Open Tether (`calls/popup.rs`,
  over the watcher's own session connection, closed when the call stops ringing). KDE Connect's plain
  "Incoming call" pop-up is skipped while Bluetooth calling is ready (`calls::rings_here`).
- Q23: command palette is an ARIA combobox.
- Tests: `cargo test -p tether calls` (fake PipeWire + fake notification server + stand-in wpctl on a
  private bus), `testing/ui/wd.sh testing/ui/test_calls.py`, `testing/ui/wd.sh` (all pass 2026-11-26).
- Switch between two calls: a waiting call's Answer holds the first (HoldAndAnswer); a held call's card
  has "Switch to this call" (`call_swap`, also in Ctrl+K). Mute shows only on the call being talked on.
- `tether call [answer|decline|hangup|pickup|mute|here|keys N|<number>]`, checked end to end against
  the fake phone with a scratch script (private bus + `testing/ui/fake_telephony.py` + daemon). README
  lists calls and the three most useful commands.
- Tray menu: while a call is live, "Answer <caller>" / "Decline" (ringing) or "Hang up on <caller>" and
  a "Mute the microphone" check row come right after Open Tether (`TrayState.call`, refreshed from the
  calls watcher). Unit-tested in `tray/menu.rs`.

## In progress

- Nothing half-done. Draft PR #15 describes everything above; keep its body current.

## Up next

- A check of the tray's call rows in `testing/ui/tray_e2e.sh` (needs fake telephony on that bus).
- Real-phone checks: everything in QUEUE "Needs a real phone", plus the new call features (keypad
  tones reach phone menus, Mute silences the far end, pop-up buttons on GNOME/KDE, HoldAndAnswer on a
  held call with no waiting call, which PipeWire may refuse).

Delete this file once the real-phone checks in QUEUE are done.
