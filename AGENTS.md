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
- **Keep the chat free.** Builds and tests slower than ~30 s run in the background; keep answering and report
  when they end. A new message from the owner gets answered first.
- The owner doesn't read messages while you work. Mid-work text is short progress updates (one line before
  the first tool call, then what you're on now). Every question, decision and result goes in the final
  message at the end of the turn; instead of stopping to ask mid-turn, pick a sensible default and note it.
- Finish the whole task in the turn. Work through errors, timeouts and failing tools yourself. Stop early
  only for a real blocker: a missing credential, a file marked do-not-touch, or a decision only the owner
  can make.
- Ask the owner's approval only for messages to people outside the conversation, pushes and releases
  (unless asked), and risky or irreversible actions. Name the exact action when asking.
- Questions go in the message itself, in full: what it's about in one line, the question, a concrete example,
  your recommendation. Ask everything in one round, without pointing to docs or earlier messages.
- Steps only the owner can do (install on their laptop, pair the phone, grant a permission on the phone): one
  numbered list, one action per step, each with the exact click or command, on their own devices. Setup on
  this box is yours.
- Final report, readable by someone who wasn't there, in this order, skipping empty sections:
  1. **Done**: one bullet per change, what changed and why it matters.
  2. **Checked / not checked**: only checks that actually ran; say plainly what you could not check.
  3. **Problems**: anything broken, blocked or risky.
  4. **Your actions**: numbered steps only the owner can do.

## Working style

- Make the smallest correct change. Reuse existing helpers, types and patterns before adding new ones.
- The codebase is yours: consolidate duplicated code into shared helpers and fix related problems nearby.
- After fixing a bug, search for the same bug elsewhere (same helper, sibling plugins, the other side of the
  protocol) and fix or report it. Say what else you changed.
- Flow: search, read the relevant ranges, edit, run the cheapest check, inspect the diff. Read each file once.
  Skip `target/`, `Cargo.lock`, generated code and vendored sources.
- Context budget: keep raw output out of context. Filter at the source (`| grep`, `| tail -30`) or save to a scratch
  file and grep it. This covers cargo output, logs, packet captures and JSON.
- If an approach fails twice, switch to a different one rather than retrying it a third time.
- Check CI and deploys once later rather than in a waiting loop.
- Use subagents when parallel work or a fresh context pays for its cost (wide exploration, long logs);
  review your own diff yourself unless the owner asks for a reviewer.
- **No half features**: if UI copy or a menu item promises a behaviour, the code delivers it, or the copy goes.
- Show before building anything the owner will see: agree the direction for a new screen first (a sketch or a
  short description they answer), build one screen, send a screenshot, then the next.

## Rust on a shared box

Rust builds are the heaviest thing on this machine. Keep them lean:
- `.cargo/config.toml` caps jobs at 2 and drops debug info; keep those settings.
- Builds and tests: `box heavy -- cargo ...`. Iterate with `cargo check -p <crate>` and `cargo test -p <crate>
  <filter>`; full `cargo build` / `cargo clippy --workspace` once when the change is done.
- One `target/` for every checkout: worktrees set `CARGO_TARGET_DIR` to the main checkout's `target/` rather
  than growing their own.
- Watch `target/` size (`du -sh target`); `cargo clean` past ~15 GB. Add a dependency only when needed, with
  minimal features, and say why.
- Headless box: no display. GUI checks run under `Xvfb`; the owner judges the real feel on their laptop.

## Code

- `anyhow` in the app, `thiserror` in `tether-core`. Handle errors (no `unwrap()`/`expect()`) on anything from
  the network, the phone or the filesystem, so a malformed packet can't crash the daemon.
- Protocol compatibility is the contract: match the KDE Connect packet types, fields and pairing behaviour
  exactly. When unsure, read the upstream source and cite it in a comment.
- Security: TLS and pairing are the trust boundary. Accept only pairing and identity packets from unpaired
  devices. Keep keys and certificates' private parts out of logs, and message contents or notification
  text below info level.
- Reliability is a feature: reconnect on network change, suspend/resume and phone sleep without user action.
- Comments explain why, not what.

## UI taste (the owner notices)

- No emojis or AI-sounding copy, no decorative highlight strips. Short, plain, human sentences that start with
  a capital letter; real plurals, not "device(s)".
- Icons as SVG, not font glyphs (✓, ▣ render as boxes on some systems).
- Something that only started (sending a file, connecting) shows progress, not a success check.
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

Report a check as passed only if it ran in this session; a subagent's "tested" isn't proof. Before finishing,
review the full diff and remove unrelated changes.

## Git and docs

- Commit with an area prefix (`core:`, `app:`, `plugin(<name>):`, `docs:`), staging only your own files by
  path (`git add <paths>`, not `git add .`): another thread may be working in the same checkout.
- Git identity is set globally; leave git config as is. Push only when the owner asks.
- Keep secrets, keys, certificates and real phone data out of commits; treat logs with message contents as
  sensitive.
- Keep it publishable: no hardcoded hostnames, usernames or device ids; configuration lives in config files.
  A README a stranger can follow.
- **Start of a session**: read `QUEUE.md` (the work queue) and `docs/README.md` (knowledge base index), then
  the docs page for the area you touch. **End of a session**: update `QUEUE.md` (status, new items, what waits
  on the owner) and add anything learned to `docs/` (traps go in `docs/gotchas.md`, symptom first).
- Keep docs current as you go, not at the end: a decision or a protocol trap goes in `docs/` with one line on
  why. Unfinished multi-session work gets a root `HANDOFF.md` (scope, what's verified, next steps); delete it
  once done.
