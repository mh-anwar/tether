"""Missed-call pop-up end to end: a private session bus with a stand-in notification server
(fake_notifications.py) and fake PipeWire telephony, a `tether daemon`, and the fake phone sending
kdeconnect.telephony missedCall. The pop-up must offer Call back and Send a text, and Call back must
make the phone dial. Needs: box heavy -- cargo build -p tether, and the fake_phone example built.
Run: python3 testing/calls/missed_e2e.py   Env: TETHER_UI_DIR (default /tmp/tw-missed), TETHER_PORT (14910)."""
import json, os, re, shutil, subprocess, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.path.join(os.environ.get("CARGO_TARGET_DIR", ROOT + "/target"), "debug")
W = os.environ.get("TETHER_UI_DIR", "/tmp/tw-missed")
shutil.rmtree(W, ignore_errors=True)
for d in ("cfg/tether", "data", "run", "control"):
    os.makedirs(f"{W}/{d}")
os.chmod(f"{W}/run", 0o700)
open(f"{W}/cfg/tether/config.json", "w").write('{"device_name":"MissedTest","mirror_notifications":true,"clipboard_sync":false,"mdns":false}')
env = dict(os.environ, XDG_CONFIG_HOME=f"{W}/cfg", XDG_DATA_HOME=f"{W}/data", XDG_RUNTIME_DIR=f"{W}/run",
           TETHER_PORT=os.environ.get("TETHER_PORT", "14910"), FAKE_PHONE_CONTROL=f"{W}/control", TOKIO_WORKER_THREADS="2")
env.pop("DISPLAY", None)
bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"], stdout=subprocess.PIPE, text=True)
env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().strip()


def py(script, out):
    return subprocess.Popen(["uv", "run", "-q", "--no-project", "--with", "dbus-fast", "python", script],
                            env=env, stdin=subprocess.PIPE, stdout=out, text=True)


notes = py(ROOT + "/testing/calls/fake_notifications.py", open(f"{W}/notes.log", "w"))
tel = py(ROOT + "/testing/ui/fake_telephony.py", subprocess.PIPE)
assert tel.stdout.readline().strip() == "ready", "fake telephony didn't start"
tel.stdin.write("gateway\n")
tel.stdin.flush()
daemon = subprocess.Popen([BIN + "/tether", "daemon"], env=env, stdout=open(f"{W}/daemon.log", "w"), stderr=subprocess.STDOUT)
procs = [daemon]


def until(what, fn, seconds=20):
    for _ in range(seconds * 5):
        v = fn()
        if v:
            return v
        time.sleep(0.2)
    raise AssertionError(f"timed out: {what}")


def popups():
    return [json.loads(l) for l in open(f"{W}/notes.log").read().splitlines() if l.startswith("{")]


try:
    port = until("daemon listening", lambda: re.search(r"on tcp (\d+)", open(f"{W}/daemon.log").read())).group(1)
    procs.append(subprocess.Popen([BIN + "/examples/fake_phone", f"127.0.0.1:{port}", f"{W}/phone"], env=env,
                                  stdout=open(f"{W}/phone.log", "w"), stderr=subprocess.STDOUT))
    until("pair request", lambda: "asked to pair" in open(f"{W}/phone.log").read())
    subprocess.run([BIN + "/tether", "accept"], env=env, stdout=subprocess.DEVNULL, check=True)
    until("paired", lambda: os.path.exists(f"{W}/phone/paired"))
    until("calls ready", lambda: "calls: ready" in open(f"{W}/daemon.log").read())
    pkt = {"id": 1, "type": "kdeconnect.telephony", "body": {"event": "missedCall", "phoneNumber": "+15550199", "contactName": "Bob"}}
    open(f"{W}/control/t.tmp", "w").write(json.dumps(pkt))
    os.rename(f"{W}/control/t.tmp", f"{W}/control/0001.json")
    note = until("missed-call pop-up", lambda: next((n for n in popups() if n.get("summary") == "Missed call"), None))
    print("pop-up:", note)
    assert note["body"] == "Bob", note
    assert "callback" in note["actions"] and "text" in note["actions"], note["actions"]
    notes.stdin.write(f"click {note['id']} callback\n")
    notes.stdin.flush()
    for line in tel.stdout:
        if line.startswith("called Dial"):
            assert line.strip() == "called Dial +15550199", line
            break
    print("PASSED: missed-call pop-up offers Call back and Send a text; Call back dials")
finally:
    for p in procs:
        p.terminate()
    for p in (notes, tel):
        p.stdin.close()
        try:
            p.wait(5)
        except subprocess.TimeoutExpired:
            p.kill()
    bus.terminate()
