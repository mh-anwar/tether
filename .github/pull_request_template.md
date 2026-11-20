<!--
Every PR description is a permanent record of what changed and why: the squash merge copies it into the
commit message, so `git log` and the closed PRs together are the project's history. Fill every section;
write "None" rather than deleting one. Plain sentences, for someone who wasn't there.
-->

## Summary

What this PR does and why, in two to four sentences.

## New features

- **Name**: what a person can now do, where it is in the window, and how it works underneath.

## Changed behaviour

- What worked one way before and works another way now, and why it changed.

## Removed

- What is gone (features, settings, copy, code paths) and why.

## Why (decisions and alternatives)

- The choices made, the alternatives considered, and why they lost. Link docs/decisions.md entries.

## Data, settings and interfaces

Anything stored or exposed that another version, another device or another program could depend on.

| Kind | Name | Change | Notes |
| --- | --- | --- | --- |
| Config key (`config.json`) | `example_key` | Added | Default, allowed values |
| File on disk | `~/.local/share/tether/example.json` | Added | Format, permissions, migration |
| Local API method | `example_method` | Added | Parameters, result |
| Window event | `example` | Added | When it is pushed |
| Window storage (`localStorage`) | `tether.example` | Added | Per window only |
| Protocol packet / plugin | `kdeconnect.example` | Unchanged | Upstream citation |
| Sandbox permission (Flatpak) | `--talk-name=...` | Added | Why it is needed |

Migration for existing installs: what happens to old data, or "None needed".

## Fixes

- Bugs fixed along the way: the symptom, the cause, the fix.

## Work queue

- QUEUE.md items started, finished or added (Q numbers).

## Checked / not checked

- Checks that ran for this PR, with results.
- What could not be checked, and why.

## Known problems and follow-ups

- Anything left broken, risky or unfinished.

## Owner actions

1. Steps only the owner can do on their own devices, or "None".
