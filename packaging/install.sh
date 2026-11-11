#!/bin/sh
# Installs Tether for the current user only (no admin rights needed).
#   ./install.sh            install or update
#   ./install.sh --remove   uninstall (your pairing and messages stay in ~/.local/share/tether)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bin="$HOME/.local/bin"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/512x512/apps"

if [ "${1:-}" = "--remove" ]; then
    "$bin/tether" autostart off 2>/dev/null || true
    rm -f "$bin/tether" "$bin/tether-app" "$apps/tether.desktop" "$icons/tether.png"
    echo "Tether removed. Running copies stop at the next login."
    exit 0
fi

# ldconfig lives in /sbin, which often isn't on a normal user's PATH.
ldconfig=$(command -v ldconfig || echo /sbin/ldconfig)
if ! "$ldconfig" -p 2>/dev/null | grep -q libwebkit2gtk-4.1; then
    echo "Tether's window needs WebKitGTK 4.1. Install it first:"
    echo "  Debian/Ubuntu: sudo apt install libwebkit2gtk-4.1-0"
    echo "  Fedora:        sudo dnf install webkit2gtk4.1"
    echo "  Arch:          sudo pacman -S webkit2gtk-4.1"
    echo "(The command-line tool works without it.)"
fi

mkdir -p "$bin" "$apps" "$icons"
install -m 755 "$here/tether" "$here/tether-app" "$bin/"
install -m 644 "$here/icon.png" "$icons/tether.png"
cat > "$apps/tether.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Tether
Comment=Your phone on your desktop
Exec="$bin/tether-app"
Icon=tether
Categories=Utility;Network;
EOF

"$bin/tether" autostart on
echo "Installed. Open Tether from your app menu, or run: $bin/tether-app"
case ":$PATH:" in *":$bin:"*) ;; *) echo "Tip: add $bin to your PATH to use the tether command.";; esac
