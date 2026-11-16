# Handoff (2026-11-16, v1.1.0)

Read first: `AGENTS.md` (house rules), `QUEUE.md` (work queue), `docs/README.md` (knowledge base index).
`main` = what shipped in v1.1.0. Work goes through PRs now (branch, PR, green CI, squash-merge).

## State

- Every KDE Connect feature a user expects works both ways against the simulator (`tether_core::sim`), the
  end-to-end suite (`crates/tether/tests/protocol_e2e.rs`), the feature suites (`testing/computer`,
  `testing/media`) and the WebDriver window tests (`testing/ui/wd.sh`). docs/testing.md has the commands.
- Earlier, the protocol core, pairing, texts and notifications were verified against real KDE Connect
  26.08.2 (`testing/kdeconnect-peer`) and the owner's Samsung.
- 1.1.0 added one window per phone app with Close (#8), the tray menu, tooltip and unread marks (#9),
  notification icons and the phone camera where the phone still offers it (#10), and a Flatpak bundle built
  by the release workflow (#11). The Flatpak's portal dialogs and tray were checked only by code and CI,
  not in a real sandboxed session.
- Not verified on a real phone: everything listed in QUEUE "Needs a real phone". That is the next step,
  and the only one that needs the owner.

## Traps found this session

- `tether-app` embeds `ui/` at compile time; `build.rs` now passes a hash of the files so edits recompile it
  (docs/gotchas.md).
- Android advertises plugins that are switched off, so capabilities alone can't tell whether a feature is
  set up; the readiness checklist only says "Working" after data arrived.
- `kdeconnect.runcommand`'s `commandList` is a JSON string, not an object (upstream and the Android app).
- The repo's ruleset needs an approving review and allows squash/rebase only; the lead merges its own PRs
  with `gh pr merge --squash --admin` once CI's Rust job is green.

Delete this file once the real-phone checks in QUEUE are done.
