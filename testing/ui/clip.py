# X11 CLIPBOARD helper (python-xlib, no xclip needed).
# Usage: clip.py :DISPLAY get | clip.py :DISPLAY set "text" [seconds]
import sys, time
from Xlib import X, Xatom, display
from Xlib.protocol import event

d = display.Display(sys.argv[1])
cmd = sys.argv[2]
win = d.screen().root.create_window(0, 0, 1, 1, 0, d.screen().root_depth)
CLIP = d.intern_atom('CLIPBOARD')
UTF8 = d.intern_atom('UTF8_STRING')
TEXT = d.intern_atom('TEXT')
TARGETS = d.intern_atom('TARGETS')
OUT = d.intern_atom('CLIP_OUT')

def wait(pred, secs):
    end = time.time() + secs
    while time.time() < end:
        while d.pending_events():
            r = pred(d.next_event())
            if r is not None:
                return r
        time.sleep(0.02)
    return None

if cmd == 'get':
    text = None
    for target in (UTF8, Xatom.STRING):
        win.convert_selection(CLIP, target, OUT, X.CurrentTime)
        d.flush()
        ev = wait(lambda e: e if e.type == X.SelectionNotify and e.requestor.id == win.id else None, 2)
        if ev is None or ev.property == X.NONE:
            continue
        prop = win.get_full_property(OUT, X.AnyPropertyType)
        if prop is not None:
            text = bytes(prop.value).decode('utf-8', 'replace')
            break
    if text is None:
        print('empty')
        sys.exit(1)
    print(text)

elif cmd == 'set':
    data = sys.argv[3].encode('utf-8')
    secs = float(sys.argv[4]) if len(sys.argv) > 4 else 10
    win.set_selection_owner(CLIP, X.CurrentTime)
    d.flush()

    def serve(e):
        if e.type != X.SelectionRequest:
            return None
        req = e.requestor
        prop = e.property or e.target  # ICCCM: obsolete clients send None; reply on the target atom
        if e.target == TARGETS:
            req.change_property(prop, Xatom.ATOM, 32, [TARGETS, UTF8, Xatom.STRING, TEXT])
        elif e.target in (UTF8, Xatom.STRING, TEXT):
            req.change_property(prop, e.target, 8, data)
        else:
            prop = X.NONE
        req.send_event(event.SelectionNotify(time=e.time, requestor=req, selection=e.selection,
                                             target=e.target, property=prop))
        d.flush()
        return None

    wait(serve, secs)
