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

## In progress

- `tether call [answer|decline|hangup|pickup|mute|here|keys N|<number>]` (main.rs + client.rs). Written,
  not yet built. Next: build, run the scratch e2e (private bus + `testing/ui/fake_telephony.py` + daemon,
  then each subcommand), commit, mention it in README.
- Hold / switch between two calls: fakes updated (`HoldAndAnswer` swaps active and held/waiting; `ring`
  while active rings as "waiting"). Next: `Watcher::swap` (needs a held call, calls AG `HoldAndAnswer`),
  API `call_swap`, a "Switch to this call" button on a held call's card, a check in test_calls.py and
  the dbus test.

## Up next

- Calls in the tray menu (Answer / Hang up / Mute while a call is live).
- Real-phone checks: everything in QUEUE "Needs a real phone", plus the new call features.

Delete this file once the real-phone checks in QUEUE are done.
