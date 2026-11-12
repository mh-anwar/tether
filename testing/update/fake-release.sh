#!/usr/bin/env bash
# A stand-in GitHub release for testing updates without publishing anything.
# Usage: testing/update/fake-release.sh DIR PORT [VERSION]
# Builds DIR/latest.json (GitHub's "latest release" format), the no-admin tarball made from the
# current debug binaries (with a marker appended so the swap is visible) and SHA256SUMS, then serves
# DIR on 127.0.0.1:PORT in the foreground. Point Tether at it with
#   "update_url": "http://127.0.0.1:PORT/latest.json"   in config.json.
# The installed binaries end with the line "tether-fake-release VERSION" after a successful update.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${CARGO_TARGET_DIR:-$ROOT/target}/debug
DIR=${1:?usage: fake-release.sh DIR PORT [VERSION]}
PORT=${2:?usage: fake-release.sh DIR PORT [VERSION]}
VERSION=${3:-9.9.9}
URL=http://127.0.0.1:$PORT

rm -rf "$DIR" && mkdir -p "$DIR/tether-linux-x86_64"
for b in tether tether-app; do
  cp "$BIN/$b" "$DIR/tether-linux-x86_64/$b"
  printf '\ntether-fake-release %s\n' "$VERSION" >> "$DIR/tether-linux-x86_64/$b"
done
tar -czf "$DIR/tether-linux-x86_64.tar.gz" -C "$DIR" tether-linux-x86_64
rm -rf "$DIR/tether-linux-x86_64"
(cd "$DIR" && sha256sum tether-linux-x86_64.tar.gz > SHA256SUMS)
cat > "$DIR/latest.json" <<EOF
{"tag_name": "v$VERSION", "html_url": "$URL/",
 "assets": [
  {"name": "tether-linux-x86_64.tar.gz", "browser_download_url": "$URL/tether-linux-x86_64.tar.gz"},
  {"name": "SHA256SUMS", "browser_download_url": "$URL/SHA256SUMS"}]}
EOF
exec python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$DIR"
