"""Stand-in for a bar's XEmbed tray (what i3bar and polybar provide). Usage: trayhost.py :DISPLAY [X Y]

Owns _NET_SYSTEM_TRAY_S0 and puts docked icons, 32 px, in a dark strip at X,Y (default 1210,10).
Prints "docked <window id>" for each icon. Runs until killed.
"""
import sys
from Xlib import X, display
from Xlib.protocol import event

d = display.Display(sys.argv[1])
x, y = (int(sys.argv[2]), int(sys.argv[3])) if len(sys.argv) > 3 else (1210, 10)
screen = d.screen()
bar = screen.root.create_window(x, y, 160, 40, 0, screen.root_depth, background_pixel=0x202428,
                                override_redirect=True, event_mask=X.SubstructureNotifyMask)
bar.map()
sel = d.intern_atom("_NET_SYSTEM_TRAY_S0")
opcode = d.intern_atom("_NET_SYSTEM_TRAY_OPCODE")
bar.set_selection_owner(sel, X.CurrentTime)
screen.root.send_event(event.ClientMessage(window=screen.root, client_type=d.intern_atom("MANAGER"),
                                           data=(32, [X.CurrentTime, sel, bar.id, 0, 0])),
                       event_mask=X.StructureNotifyMask)
d.flush()
count = 0
while True:
    e = d.next_event()
    if e.type == X.ClientMessage and e.client_type == opcode and e.data[1][1] == 0:
        icon = d.create_resource_object("window", e.data[1][2])
        icon.reparent(bar, 4 + count * 36, 4)
        icon.configure(width=32, height=32)
        icon.map()
        d.flush()
        count += 1
        print("docked", hex(icon.id), flush=True)
