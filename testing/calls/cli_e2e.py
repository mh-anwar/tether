"""`tether call` end to end: a private session bus, the fake PipeWire telephony service
(testing/ui/fake_telephony.py), the stand-in wpctl and a `tether daemon` with its own folders.
Run from anywhere after `box heavy -- cargo build -p tether`:  python3 testing/calls/cli_e2e.py
Env: TETHER_UI_DIR (scratch folder, default /tmp/tw-cli), TETHER_PORT (default 14920)."""
import os, shutil, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.path.join(os.environ.get("CARGO_TARGET_DIR", ROOT + "/target"), "debug")
W = os.environ.get("TETHER_UI_DIR", "/tmp/tw-cli")
shutil.rmtree(W, ignore_errors=True)
for d in ("cfg/tether", "data", "run"):
    os.makedirs(f"{W}/{d}")
os.chmod(f"{W}/run", 0o700)
open(f"{W}/cfg/tether/config.json", "w").write('{"device_name":"CliTest","mirror_notifications":false,"mdns":false}')
env = dict(os.environ, XDG_CONFIG_HOME=f"{W}/cfg", XDG_DATA_HOME=f"{W}/data", XDG_RUNTIME_DIR=f"{W}/run",
           TETHER_PORT=os.environ.get("TETHER_PORT", "14920"), TOKIO_WORKER_THREADS="2",
           TETHER_WPCTL=ROOT + "/testing/calls/wpctl", FAKE_WPCTL_STATE=f"{W}/mic")
env.pop("DISPLAY", None)
bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"], stdout=subprocess.PIPE, text=True)
env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().strip()
tel = subprocess.Popen(["uv", "run", "-q", "--no-project", "--with", "dbus-fast", "python", ROOT + "/testing/ui/fake_telephony.py"],
                       env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
assert tel.stdout.readline().strip() == "ready", "fake telephony didn't start"
daemon = subprocess.Popen([BIN + "/tether", "daemon"], env=env, stdout=open(f"{W}/daemon.log", "w"), stderr=subprocess.STDOUT)


def tell(cmd):
    tel.stdin.write(cmd + "\n")
    tel.stdin.flush()


def heard(prefix):
    for line in tel.stdout:
        if line.startswith(prefix):
            return line.strip()
    raise AssertionError(f"phone never got {prefix}")


def call(*args, ok=True):
    r = subprocess.run([BIN + "/tether", "call", *args], env=env, capture_output=True, text=True)
    out = (r.stdout + r.stderr).strip()
    print(f"$ tether call {' '.join(args)}\n  {out}", flush=True)
    assert (r.returncode == 0) == ok, f"exit {r.returncode}"
    return out


def until(what, fn, seconds=15):
    for _ in range(seconds * 5):
        if fn():
            return
        time.sleep(0.2)
    raise AssertionError(f"timed out: {what}")


def mic():
    return open(f"{W}/mic").read().strip() if os.path.exists(f"{W}/mic") else ""


try:
    until("daemon socket", lambda: os.path.exists(f"{W}/run/tether/tether.sock"))
    assert "aren't set up" in call()
    tell("gateway")
    until("calls ready", lambda: "No call right now" in call())
    assert "Nothing is ringing" in call("answer", ok=False)
    tell("ring +15550123")
    until("ringing", lambda: "incoming: +15550123" in call())
    assert "Answering +15550123" in call("pickup")
    heard("called Answer")
    until("answered", lambda: "active: +15550123" in call())
    assert call("mute") == "Microphone off."
    assert mic() == "1" and "Microphone off." in call()
    assert call("keys", "1", "2#") == "Sent 12#."
    assert heard("called SendTones") == "called SendTones 12#"
    tell("ring +15550456")
    until("waiting", lambda: "waiting: +15550456" in call())
    assert "Declined +15550456" in call("decline")
    heard("called Hangup")
    assert "waiting" not in call(), "the daemon re-reads the calls before answering"
    assert call("pickup") == "Hung up."
    heard("called HangupAll")
    until("microphone given back", lambda: mic() == "0")
    assert "Calling 555 010 9999" in call("555", "010", "9999")
    assert heard("called Dial") == "called Dial 5550109999"
    until("dialing", lambda: "5550109999" in call())
    assert call("hangup") == "Hung up."
    assert "isn't a key" in call("keys", "E", ok=False)
    print("PASSED: tether call status, pickup, mute, keys, decline, hang up, dial")
finally:
    daemon.terminate()
    tel.stdin.close()
    try:
        tel.wait(5)
    except subprocess.TimeoutExpired:
        tel.kill()
    bus.terminate()
