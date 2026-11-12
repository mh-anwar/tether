# Getting onto Flathub

Flathub is the curated Flatpak store that GNOME Software, KDE Discover and most distros show. Status: the
app's store listing is written (`packaging/flatpak/`, passes `appstreamcli validate`); the Flatpak build
itself is not done yet (queue Q17).

## Blockers only the owner can clear
- **The GitHub repo must be public.** Flathub builds from public sources and links to them.
- **App id** `io.github.mh_anwar.Tether`: Flathub verifies `io.github.<user>` ids against the GitHub
  account, so it must match the account that owns the repo (GitHub's `-` becomes `_`).
- **License**: `GPL-3.0-or-later` (Cargo.toml, LICENSE). Change before going public if a different one is wanted.

## Work left (ours)
1. Manifest `io.github.mh_anwar.Tether.yml`: runtime `org.gnome.Platform` (ships WebKitGTK for Tauri),
   `rust-stable` SDK extension, cargo sources vendored with `flatpak-cargo-generator` (Flathub builds have
   no network).
2. Bundle `adb` (android-tools, built from source) and `scrcpy` (meson; SDL3 + FFmpeg) as modules and set
   `TETHER_ADB` / `TETHER_SCRCPY`, so phone apps need no download inside the Flatpak.
3. Permissions: `--share=network` (KDE Connect, adb), `--socket=wayland`, `--socket=fallback-x11`,
   `--device=dri`, `--socket=pulseaudio` (app sound), `--talk-name=org.freedesktop.Notifications`,
   `--own-name=org.mpris.MediaPlayer2.tether*` (media keys).
4. Start at login: inside the sandbox our systemd unit can't point at `/app/bin`; use the Background portal
   (`org.freedesktop.portal.Background.RequestBackground` with autostart) instead.
5. Clipboard sync with the window closed needs the display socket, which the sandbox has.
6. Screenshots in the metainfo (hosted in the public repo), then a CI job that builds the Flatpak with
   `flatpak/flatpak-github-actions` on every release.
7. Submit: fork `flathub/flathub`, add the manifest on a `new-pr` branch, open the PR, answer review.
