#!/bin/sh
# Writes packaging/flatpak/cargo-sources.json, the list of every crate in Cargo.lock that the
# Flatpak build vendors (Flathub builds have no network). Run it after Cargo.lock changes; CI runs it
# before each Flatpak build. Needs curl and uv (https://docs.astral.sh/uv/).
set -eu

# flatpak-builder-tools at a fixed commit, checked by hash, so a change upstream can't slip in.
COMMIT=41c20aa10819cdb2a4f3ca171758a96d1955c018
SHA256=0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380
URL="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$COMMIT/cargo/flatpak-cargo-generator.py"

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/flatpak-cargo-generator.py" "$URL"
echo "$SHA256  $tmp/flatpak-cargo-generator.py" | sha256sum -c --quiet -
# The script declares its own dependencies (PEP 723), which uv installs into a throwaway env.
uv run --quiet --script "$tmp/flatpak-cargo-generator.py" "$root/Cargo.lock" -o "$here/cargo-sources.json"
echo "Wrote $here/cargo-sources.json"
