#!/usr/bin/env bash
# Stand-in for the phone's file server ("Filesystem expose"): a user-level OpenSSH sshd on
# 127.0.0.1:$PORT that accepts Tether's key (DATA/tether/key.der, created on its first run) and
# serves ROOT with two screenshots and five camera photos (plus a video, which is skipped). Prints the FAKE_SFTP value for examples/fake_phone; runs
# until killed. Usage: serve.sh WORKDIR DATA_HOME [PORT]
set -euo pipefail
W=$1 DATA=$2 PORT=${3:-2299}
ART=$(cd "$(dirname "$0")/../fake-art" && pwd)
S=$W/sftp; rm -rf "$S/root" # a clean phone each run, so tests can create and delete files
mkdir -p "$S/root/DCIM/Screenshots"
[ -f "$S/hostkey" ] || ssh-keygen -q -t ed25519 -f "$S/hostkey" -N ''
cp "$ART/photo-cake.jpg" "$S/root/DCIM/Screenshots/Screenshot_20261101-090000.jpg"
cp "$ART/photo-dog.jpg" "$S/root/DCIM/Screenshots/Screenshot_20261112-185300.jpg"
touch -d 2026-11-01 "$S/root/DCIM/Screenshots/Screenshot_20261101-090000.jpg"
mkdir -p "$S/root/DCIM/Camera"
n=0; for f in photo-dog art-debussy photo-cake art-kob art-mezz; do
  n=$((n + 1)); cp "$ART/$f.jpg" "$S/root/DCIM/Camera/20261112_18000$n.jpg"; touch -d "2026-11-12 18:0$n" "$S/root/DCIM/Camera/20261112_18000$n.jpg"
done
: > "$S/root/DCIM/Camera/20261112_180009.mp4"
python3 - "$DATA/tether/key.der" > "$S/key.pem" <<'PY'
import base64, sys
b = base64.b64encode(open(sys.argv[1], "rb").read()).decode()
print("-----BEGIN PRIVATE KEY-----", *[b[i:i + 64] for i in range(0, len(b), 64)], "-----END PRIVATE KEY-----", sep="\n")
PY
chmod 600 "$S/key.pem" && ssh-keygen -y -f "$S/key.pem" > "$S/authorized_keys"
cat > "$S/sshd_config" <<CFG
Port $PORT
ListenAddress 127.0.0.1
HostKey $S/hostkey
AuthorizedKeysFile $S/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
PidFile $S/sshd.pid
Subsystem sftp internal-sftp
CFG
echo "127.0.0.1:$PORT:$USER:$S/root"
exec /usr/sbin/sshd -D -e -f "$S/sshd_config" 2> "$S/sshd.log"
