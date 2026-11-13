"""Stand-in for a desktop's StatusNotifierWatcher (KDE, GNOME's AppIndicator extension, waybar).

Run on the session bus the Tether service uses. Waits for an item to register, then prints its
title and icon sizes, saves the largest icon as PNG if a path is given, and calls Activate.
Usage: sniwatcher.py [icon.png]
"""
import asyncio
import sys

from dbus_next.aio import MessageBus
from dbus_next.service import ServiceInterface, method


class Watcher(ServiceInterface):
    def __init__(self):
        super().__init__("org.kde.StatusNotifierWatcher")
        self.items = asyncio.Queue()

    @method()
    def RegisterStatusNotifierItem(self, service: "s"):
        self.items.put_nowait(service)


async def main():
    bus = await MessageBus().connect()
    watcher = Watcher()
    bus.export("/StatusNotifierWatcher", watcher)
    await bus.request_name("org.kde.StatusNotifierWatcher")
    name = await asyncio.wait_for(watcher.items.get(), 30)
    print("registered", name.split("-")[0])
    intro = await bus.introspect(name, "/StatusNotifierItem")
    item = bus.get_proxy_object(name, "/StatusNotifierItem", intro).get_interface("org.kde.StatusNotifierItem")
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
