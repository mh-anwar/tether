# Tether: Agent Guide

Tether is a Linux desktop app that talks to phones over the KDE Connect protocol, aiming to be as good as
Windows Phone Link: messages, notifications, clipboard, files, calls through the computer, phone screen and apps
on the desktop, photos. Rust workspace: `crates/tether-core` (protocol, pairing, TLS, plugins) and
`crates/tether` (the app). The stock KDE Connect Android app must keep working against it. Target: any Linux
desktop (GNOME, KDE, i3 and other bare window managers), tested on the owner's Samsung phone.

This file is the rules every turn needs. The box's own rules (limits, `box heavy`, servers, no sudo) arrive
separately in your global instructions; they still apply.

## The owner

- Not a programmer, and not at this machine: they reach you from another device. Short, plain replies, no jargon.
- Say in one line what you're doing before anything that takes more than a minute.
- **Never block the chat.** Builds and tests slower than ~30 s run in the background; keep answering and report
  when they end. A new message from the owner gets answered first.
- The owner doesn't read messages while you work. Mid-work text is short progress updates only (what you're
  on now). Every question, decision and result goes in the final message at the end of the turn; never ask
  mid-turn and wait, pick a sensible default and note it in the final message instead.
- Questions go in the message itself, in full: what it's about in one line, the question, a concrete example,
  your recommendation. Ask everything in one round. Never "see the docs" or "see my earlier message".
- Steps only the owner can do (install on their laptop, pair the phone, grant a permission on the phone): one
  numbered list, one action per step, each with the exact click or command. Never send them to a terminal on
  this box; setup here is yours.
- Final report, in this order, skipping empty sections:
  1. **Done**: one bullet per change, what changed and why it matters.
  2. **Checked / not checked**: only checks that actually ran; say plainly what you could not check.
  3. **Problems**: anything broken, blocked or risky.
  4. **Your actions**: numbered steps only the owner can do.

## Working style

- Make the smallest correct change. Reuse existing helpers, types and patterns before adding new ones.
- After fixing a bug, search for the same bug elsewhere (same helper, sibling plugins, the other side of the
  protocol) and fix or report it. Say what else you changed.
- Flow: search, read the relevant ranges, edit, run the cheapest check, inspect the diff. Read each file once.
  Skip `target/`, `Cargo.lock`, generated code and vendored sources.
- Context budget: never dump raw output. Filter at the source (`| grep`, `| tail -30`) or save to a scratch
  file and grep it. This covers cargo output, logs, packet captures and JSON.
- Two strikes: if an approach fails twice, stop and report what you know and the next step. After ~5 tool
  calls with no new finding, stop and report findings, the open hypothesis, and what would settle it.
- No polling or waiting loops on CI, deploys or the owner. Check once later.
- Use cheap subagents for exploration and log reading, so only their conclusion enters context.
- **No half features**: if UI copy or a menu item promises a behaviour, the code delivers it, or the copy goes.
- Show before building anything the owner will see: agree the direction for a new screen first (a sketch or a
  short description they answer), build one screen, send a screenshot, then the next.

## Rust on a shared box

Rust builds are the heaviest thing on this machine. Keep them lean:
- `.cargo/config.toml` already caps jobs at 2 and drops debug info; don't undo it.
- Builds and tests: `box heavy -- cargo ...`. Iterate with `cargo check -p <crate>` and `cargo test -p <crate>
  <filter>`; full `cargo build` / `cargo clippy --workspace` once when the change is done.
- One `target/` for every checkout: worktrees set `CARGO_TARGET_DIR` to the main checkout's `target/` rather
  than growing their own.
- Watch `target/` size (`du -sh target`); `cargo clean` past ~15 GB. Add a dependency only when needed, with
  minimal features, and say why.
- Headless box: no display. GUI checks run under `Xvfb`; the owner judges the real feel on their laptop.

## Code

- `anyhow` in the app, `thiserror` in `tether-core`. No `unwrap()`/`expect()` on anything that comes from the
  network, the phone or the filesystem: a malformed packet must never crash the daemon.
- Protocol compatibility is the contract: match the KDE Connect packet types, fields and pairing behaviour
  exactly. When unsure, read the upstream source and cite it in a comment.
- Security: TLS and pairing are the trust boundary. Never accept an unpaired device's packets beyond pairing
  and identity, never log keys, certificates' private parts, message contents or notification text at info level.
- Reliability is a feature: reconnect on network change, suspend/resume and phone sleep without user action.
- Comments explain why, not what.

## UI taste (the owner notices)

- No emojis, no AI-sounding copy, no decorative highlight strips. Short, plain, human sentences that start with
  a capital letter; real plurals, never "device(s)".
- Icons as SVG, never font glyphs (✓, ▣ render as boxes on some systems).
- Something that only started (sending a file, connecting) shows progress, never a success check.
- Subtle corner rounding, not pill shapes. Every screen has loading, error, empty and focus states, works with
  keyboard only, respects reduced motion and doesn't rely on colour alone. Every screen has an obvious next step.
- When the owner flags one glitch, find every instance of that pattern and fix them all.

## Checks

Run the cheapest check that proves the change; climb only as far as the change reaches:

| Check | When |
| --- | --- |
| `cargo check -p <crate>`, the touched tests | while iterating |
| `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` | once, before finishing |
| A real run against the phone or a protocol test double | protocol, pairing, plugins |
| A screenshot under Xvfb, actually looked at | anything visible |

Never claim a check passed unless it ran. Don't relay a subagent's "tested" as proof. Before finishing, inspect
the diff and remove unrelated changes.

## Git and docs

- Commit with an area prefix (`core:`, `app:`, `plugin(<name>):`, `docs:`), only your own files by path
  (`git add <paths>`): another thread may be working in the same checkout. Never `git add .`.
- Git identity is set globally; never change git config. Push only when the owner asks.
- Never commit secrets, keys, certificates or real phone data; treat logs with message contents as sensitive.
- Keep it publishable: no hardcoded hostnames, usernames or device ids; configuration lives in config files.
  A README a stranger can follow.
- Keep docs current as you go, not at the end: a decision or a protocol trap goes in `docs/` with one line on
  why. Unfinished multi-session work gets a root `HANDOFF.md` (scope, what's verified, next steps); delete it
  once done.
