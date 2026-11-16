# Flatpak and Flathub

Flathub is the curated Flatpak store that GNOME Software, KDE Discover and most distros show. Status: Tether
builds as a Flatpak (`packaging/flatpak/io.github.mh_anwar.Tether.yml`), and every release attaches
`Tether-x86_64.flatpak` (the `flatpak` job in `.github/workflows/release.yml`). It is not on Flathub yet.

## Building it

```sh
packaging/flatpak/cargo-sources.sh      # cargo-sources.json from Cargo.lock (needs curl and uv)
flatpak-builder --user --install --force-clean flatpak-build packaging/flatpak/io.github.mh_anwar.Tether.yml
flatpak run io.github.mh_anwar.Tether
```

Needs the GNOME 50 runtime and SDK and the `rust-stable` SDK extension from Flathub. Without a local
flatpak, the CI image works under (rootless) Docker:
`docker run --privileged -v "$PWD:/src" -w /src ghcr.io/flathub-infra/flatpak-github-actions:gnome-50 flatpak-builder ...`.

`cargo-sources.json` is generated, not committed: CI makes it from the tagged commit's `Cargo.lock`, so it
can't go stale. The Flathub repo will need a committed copy, regenerated with each release.

## How the sandbox changes Tether

- Start at login goes through the Background portal (`RequestBackground` with `autostart`, command
  `tether daemon`); the portal writes the autostart entry on the host. The sandbox can't see that entry,
  so `~/.var/app/io.github.mh_anwar.Tether/config/tether/autostart-portal` records the answer
  (`crates/tether/src/flatpak.rs`, `autostart.rs`).
- The tray icon owns `io.github.mh_anwar.Tether.StatusNotifierItem-<pid>-1` instead of
  `org.kde.StatusNotifierItem-<pid>-1`: an app may own names under its own id without a wildcard
  permission, and tray hosts accept any name.
- Phone apps: scrcpy and adb still download into the app's data folder
  (`~/.var/app/io.github.mh_anwar.Tether/data/tether/helpers`) over wireless debugging. USB debugging
  would need `--device=all`, which the manifest doesn't ask for.
- Updates: a copy from Flathub is updated by the store; the `.flatpak` bundle has no remote, so it checks
  GitHub and points at the releases page (`update.rs`, `origin` in `/.flatpak-info`).
- Hidden in the Flatpak, because the sandbox can't deliver them: sending this computer's notifications to
  the phone (needs `BecomeMonitor` on the session bus, which the D-Bus proxy refuses), pinning phone apps
  to the app menu (the sandbox's applications folder is private), and the "install updates by themselves"
  switch.
- Commands the phone runs execute inside the sandbox, so host programs aren't there; the Settings text
  says so. Remote mouse and keyboard need `dotool`/`ydotool`/`wtype`, which the sandbox lacks, so the
  phone's remote input reports itself unavailable unless X11 is in use. Volume control uses the
  runtime's `pactl`; screen lock uses `org.freedesktop.ScreenSaver` (no `loginctl` in the sandbox).
- Files from the phone can only be saved inside Downloads (`--filesystem=xdg-download`).

## Blockers only the owner can clear
- **The GitHub repo must be public.** Flathub builds from public sources and links to them.
- **App id** `io.github.mh_anwar.Tether`: Flathub verifies `io.github.<user>` ids against the GitHub
  account, so it must match the account that owns the repo (GitHub's `-` becomes `_`).
- **License**: `GPL-3.0-or-later` (Cargo.toml, LICENSE). Change before going public if a different one is wanted.

## Work left (ours)
1. Point the manifest's source at a release tag (`type: git`, `tag`, `commit`) instead of `type: dir`, and
   commit `cargo-sources.json` next to it in the Flathub repo.
2. Bundle `adb` (android-tools) and `scrcpy` (meson; SDL3 + FFmpeg) as modules and set `TETHER_ADB` /
   `TETHER_SCRCPY`, so phone apps need no download (Flathub prefers no runtime downloads).
3. Pinned phone apps through the Dynamic Launcher portal (`org.freedesktop.portal.DynamicLauncher`,
   `PrepareInstall` then `Install`), with Exec `flatpak run --command=tether io.github.mh_anwar.Tether apps <package>`.
4. Sending this computer's notifications: needs `--socket=session-bus` (Flathub asks for a reason) or a
   different source of notifications; until then it stays hidden.
5. Screenshots in the metainfo (hosted in the public repo).
6. Run `flatpak-builder-lint manifest` and `flatpak-builder-lint repo` and fix what they report.
7. Submit: fork `flathub/flathub`, add the manifest on a `new-pr` branch, open the PR, answer review.
