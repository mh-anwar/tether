"""Stand-in for a desktop's StatusNotifierWatcher (KDE, GNOME's AppIndicator extension, waybar).

Run on the session bus the Tether service uses. Waits for an item to register, then prints its
title and icon sizes, saves the largest icon as PNG if a path is given, and calls Activate.
Usage: sniwatcher.py [icon.png]

With --check RECEIVED_JSONL (see tray_e2e.sh) it tests the tray the way a desktop uses it instead:
tooltip, status and themed icon, the menu over com.canonical.dbusmenu, "Ring my phone" reaching the
phone (RECEIVED_JSONL is the fake phone's log of packets it received), the pause toggle, the
launcher badge, and "Quit Tether" stopping the service. Exits non-zero on the first failure.
"""
import asyncio
import json
import os
import sys

from dbus_next import Message, Variant
from dbus_next.aio import MessageBus
from dbus_next.service import ServiceInterface, method


class Watcher(ServiceInterface):
    def __init__(self):
        super().__init__("org.kde.StatusNotifierWatcher")
        self.items = asyncio.Queue()

    @method()
    def RegisterStatusNotifierItem(self, service: "s"):
        self.items.put_nowait(service)


def fail(what):
    print("FAIL", what, flush=True)
    sys.exit(1)


def rows(layout):
    """[(id, props)] of the root's children, props as plain values."""
    _, _, children = layout
    out = []
    for child in children:
        cid, props, _ = child.value
        out.append((cid, {k: v.value for k, v in props.items()}))
    return out


def label(props):
    if props.get("type") == "separator":
        return "-"
    text = props.get("label", "").replace("__", "_")
    if props.get("enabled") is False:
        text = f"({text})"
    if props.get("toggle-state") == 1:
        text = f"[x] {text}"
    return text


async def wait_for(check, seconds, what):
    for _ in range(int(seconds * 10)):
        if await check():
            return
        await asyncio.sleep(0.1)
    fail(what)


async def check_calls(menu):
    """A Bluetooth call puts Answer / Decline, then Hang up and Mute at the top of the menu."""
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    tel = await asyncio.create_subprocess_exec(
        "uv", "run", "-q", "--no-project", "--with", "dbus-fast", "python",
        os.path.join(root, "testing/ui/fake_telephony.py"),
        stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE)

    async def tell(cmd):
        tel.stdin.write((cmd + "\n").encode())
        await tel.stdin.drain()

    async def heard(prefix):
        while True:
            line = (await asyncio.wait_for(tel.stdout.readline(), 10)).decode()
            if not line:
                fail(f"phone never got {prefix}")
            if line.startswith(prefix):
                return line.strip()

    async def menu_rows():
        _, l = await menu.call_get_layout(0, -1, [])
        return {label(p): i for i, p in rows(l)}

    async def row(text, seconds=15):
        found = {}

        async def shown():
            found.update(await menu_rows())
            return text in found
        await wait_for(shown, seconds, f"no {text!r} row in the tray menu")
        return found[text]

    try:
        await heard("ready")
        await tell("gateway")
        await tell("ring +15550123")
        await menu.call_event(await row("Answer +15550123"), "clicked", Variant("s", ""), 0)
        await heard("called Answer")
        hang = await row("Hang up on +15550123")
        await menu.call_event(await row("Mute the microphone"), "clicked", Variant("s", ""), 0)
        await row("[x] Mute the microphone", 5)
        await menu.call_event(hang, "clicked", Variant("s", ""), 0)
        await heard("called Hangup")

        async def no_call():
            return not any(k.startswith("Hang up") for k in await menu_rows())
        await wait_for(no_call, 10, "call rows stayed after hanging up")
        print("calls: answered, muted and hung up from the tray")
    finally:
        # End of stdin stops the fake; killing only uv would leave its python holding our pipe.
        tel.stdin.close()
        try:
            await asyncio.wait_for(tel.wait(), 5)
        except asyncio.TimeoutError:
            tel.kill()


async def check(bus, name, item, received):
    launcher = []
    layout_updates = []

    def on_message(msg):
        if msg.interface == "com.canonical.Unity.LauncherEntry" and msg.member == "Update":
            uri, props = msg.body
            launcher.append((uri, {k: v.value for k, v in props.items()}))
        if msg.interface == "com.canonical.dbusmenu" and msg.member == "LayoutUpdated":
            layout_updates.append(msg.body[0])

    bus.add_message_handler(on_message)
    for rule in ("type='signal',interface='com.canonical.Unity.LauncherEntry'",
                 "type='signal',interface='com.canonical.dbusmenu'"):
        await bus.call(Message(destination="org.freedesktop.DBus", path="/org/freedesktop/DBus",
                               interface="org.freedesktop.DBus", member="AddMatch", signature="s", body=[rule]))

    # The phone has connected once its name is in the tooltip.
    async def connected():
        tip = await item.get_tool_tip()
        return ": " in tip[3] and "not connected" not in tip[3]
    await wait_for(connected, 20, "the tooltip never showed a connected phone")
    tip = await item.get_tool_tip()
    print("tooltip", tip[2], "|", tip[3].replace("\n", " | "))
    if tip[2] != "Tether":
        fail("tooltip title")

    status, title = await item.get_status(), await item.get_title()
    icon_name, theme = await item.get_icon_name(), await item.get_icon_theme_path()
    print("status", status, "| title", title, "| icon", repr(icon_name))
    unread = "unread" in title
    if status != ("NeedsAttention" if unread else "Active"):
        fail(f"status {status} with title {title!r}")
    if unread and icon_name:
        fail("a themed icon would hide the unread badge")
    if not unread and icon_name != "tether-tray-symbolic":
        fail(f"icon name {icon_name!r} on KDE")
    if not os.path.isfile(os.path.join(theme, "hicolor/scalable/apps/tether-tray-symbolic.svg")):
        fail(f"no symbolic icon in {theme}")
    if not await item.get_attention_icon_pixmap():
        fail("no attention icon")

    path = await item.get_menu()
    intro = await bus.introspect(name, path)
    menu = bus.get_proxy_object(name, path, intro).get_interface("com.canonical.dbusmenu")
    _, layout = await menu.call_get_layout(0, -1, [])
    items = rows(layout)
    labels = [label(p) for _, p in items]
    print("menu", labels)
    expected = ["Open Tether", "-", "Ring my phone", "Send files…", "-",
                "Pause notification pop-ups", "Quiet mode", "-", "Quit Tether"]
    if labels != expected:
        fail(f"menu {labels}")
    ids = {label(p): i for i, p in items}

    before = len(open(received).read().splitlines()) if os.path.exists(received) else 0
    await menu.call_event(ids["Ring my phone"], "clicked", Variant("s", ""), 0)

    async def rang():
        if not os.path.exists(received):
            return False
        lines = open(received).read().splitlines()[before:]
        return any(json.loads(l).get("type") == "kdeconnect.findmyphone.request" for l in lines)
    await wait_for(rang, 5, "the phone never got kdeconnect.findmyphone.request")
    print("rang the phone")

    await menu.call_event(ids["Pause notification pop-ups"], "clicked", Variant("s", ""), 0)

    async def paused():
        _, l = await menu.call_get_layout(0, -1, [])
        return any(label(p) == "[x] Pause notification pop-ups" for _, p in rows(l))
    await wait_for(paused, 3, "pause didn't tick")
    if not layout_updates:
        fail("no LayoutUpdated after the menu changed")
    if "pop-ups paused" not in (await item.get_tool_tip())[3]:
        fail("tooltip doesn't say pop-ups are paused")
    print("paused pop-ups, layout revision", layout_updates[-1])

    if unread:
        if not launcher:
            fail("no launcher badge while there are unread texts")
        uri, props = launcher[-1]
        print("launcher", uri, props)
        if not uri.startswith("application://") or props.get("count", 0) < 1 or not props.get("count-visible"):
            fail(f"launcher badge {uri} {props}")

    await check_calls(menu)

    await menu.call_event(ids["Quit Tether"], "clicked", Variant("s", ""), 0)

    async def gone():
        reply = await bus.call(Message(destination="org.freedesktop.DBus", path="/org/freedesktop/DBus",
                                       interface="org.freedesktop.DBus", member="NameHasOwner",
                                       signature="s", body=[name]))
        return not reply.body[0]
    await wait_for(gone, 5, "Quit Tether didn't stop the service")
    print("quit: service stopped")
    print("PASS")


async def main():
    bus = await MessageBus().connect()
    watcher = Watcher()
    bus.export("/StatusNotifierWatcher", watcher)
    await bus.request_name("org.kde.StatusNotifierWatcher")
    name = await asyncio.wait_for(watcher.items.get(), 30)
    print("registered", name.split("-")[0], flush=True)
    intro = await bus.introspect(name, "/StatusNotifierItem")
    item = bus.get_proxy_object(name, "/StatusNotifierItem", intro).get_interface("org.kde.StatusNotifierItem")
    if len(sys.argv) > 2 and sys.argv[1] == "--check":
        await check(bus, name, item, sys.argv[2])
        return
    print("title", await item.get_title())
    pixmaps = await item.get_icon_pixmap()
    print("icons", [(w, h, len(data)) for w, h, data in pixmaps])
    if len(sys.argv) > 1:
        from PIL import Image
        w, h, data = pixmaps[-1]
        argb = bytes(data)
        rgba = b"".join(argb[i + 1:i + 4] + argb[i:i + 1] for i in range(0, len(argb), 4))
        Image.frombytes("RGBA", (w, h), rgba).save(sys.argv[1])
    await item.call_activate(0, 0)
    print("activated")
    await asyncio.sleep(12)  # stay up so the service sees a watcher and leaves the XEmbed tray


asyncio.run(main())
