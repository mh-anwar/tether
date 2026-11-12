# Why Tether exists (positioning)

Checked 2026-11-11. Re-check when a competitor ships something big.

## What Linux already has

| | KDE Connect (desktop) | GSConnect | Valent | Tether |
| --- | --- | --- | --- | --- |
| Runs on | any desktop (KDE libs, tray icon) | GNOME Shell only | GTK4, GNOME-first | any desktop, one window |
| Notifications, clipboard, files, media, ring | yes | yes | yes | yes |
| Texts | separate SMS app, basic | basic window | yes | messages-first window, picture messages, codes card |
| Phone apps on the desktop | no | no | no | **yes: each app in its own window** |
| Calls with sound on the computer | no (only "phone is ringing") | no | no | planned (Bluetooth hands-free, Q4) |

The honest part: notifications, clipboard, files, media and texts are what every KDE Connect desktop
already does. Doing them in a nicer window is worth having, but it isn't a reason to switch.

## What only Tether does

1. **Phone apps as desktop windows.** Pick an app in the Apps page and it opens in its own window on the
   computer (with its sound on Android 11+), while the phone stays usable. Built on Android's wireless debugging and scrcpy
   (virtual displays, Android 10+). Setup is one QR scan. Phone Link offers this only on some Samsung
   phones on Windows; nothing on Linux has it built in. scrcpy on its own is a command-line tool for
   developers; Tether makes it a one-click feature next to your texts.
2. **Calls on the computer** (next, Q4): answer and talk through the laptop. KDE Connect can't carry audio;
   Bluetooth hands-free can.
3. **One app for every desktop**, including tiling window managers, with the phone's player on the media
   keys and the CLI for scripting.

## Ideas that would widen the gap (not built)

- Real app icons in the Apps page (from notification icons first, then pulled from the phone).
- Pin a phone app to the desktop's launcher (a `.desktop` file per app that runs `tether apps <name>`).
- Scheduled texts; a quiet mode that bundles phone notifications into one summary.

## Distribution

Goal: Flathub (see `flathub.md`). A Flatpak can bundle adb and scrcpy, so phone apps work without the
helper download.
