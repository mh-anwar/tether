# Drives a virtual X display with synthetic input (python-xlib XTEST).
# Run: uv run --no-project --with python-xlib python testing/ui/drive.py :57 click X Y type "hi" key Return key ctrl+k sleep 1
# Usage: drive.py :57 click X Y | type "text" | key Return | key ctrl+k
import sys, time
from Xlib import X, XK, display
from Xlib.ext import xtest
d = display.Display(sys.argv[1])
def keysym(name):
    ks = XK.string_to_keysym(name)
    if ks == 0 and len(name) == 1: ks = ord(name)
    return d.keysym_to_keycode(ks)
def press(code, shift=False):
    sh = d.keysym_to_keycode(XK.string_to_keysym('Shift_L'))
    if shift: xtest.fake_input(d, X.KeyPress, sh)
    xtest.fake_input(d, X.KeyPress, code); xtest.fake_input(d, X.KeyRelease, code)
    if shift: xtest.fake_input(d, X.KeyRelease, sh)
    d.sync(); time.sleep(0.02)
args = sys.argv[2:]
while args:
    cmd = args.pop(0)
    if cmd == 'click':
        x, y = int(args.pop(0)), int(args.pop(0))
        xtest.fake_input(d, X.MotionNotify, x=x, y=y); d.sync(); time.sleep(0.05)
        xtest.fake_input(d, X.ButtonPress, 1); xtest.fake_input(d, X.ButtonRelease, 1); d.sync()
    elif cmd == 'type':
        for ch in args.pop(0):
            name = {' ': 'space', '!': 'exclam', ',': 'comma', '.': 'period', '?': 'question', "'": 'apostrophe', ':': 'colon'}.get(ch, ch)
            press(keysym(name), shift=ch.isupper() or ch in '!?:')
    elif cmd == 'key':
        parts = args.pop(0).split('+'); mods = [d.keysym_to_keycode(XK.string_to_keysym({'ctrl': 'Control_L', 'shift': 'Shift_L'}[m])) for m in parts[:-1]]
        for m in mods: xtest.fake_input(d, X.KeyPress, m)
        press(keysym(parts[-1]))
        for m in mods: xtest.fake_input(d, X.KeyRelease, m)
        d.sync()
    elif cmd == 'sleep':
        time.sleep(float(args.pop(0)))
    time.sleep(0.15)
