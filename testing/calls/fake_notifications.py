"""Stand-in desktop notification server for tests: prints each Notify as JSON on stdout (after a
first "ready" line); stdin lines "click <id> <action>" emit ActionInvoked, as a click on a button would."""
import asyncio, json, sys
from dbus_fast.aio import MessageBus
from dbus_fast.service import ServiceInterface, method, signal

class N(ServiceInterface):
    def __init__(self):
        super().__init__("org.freedesktop.Notifications"); self.n = 40
    @method()
    def Notify(self, app: "s", rid: "u", icon: "s", summary: "s", body: "s", actions: "as", hints: "a{sv}", t: "i") -> "u":
        self.n += 1
        print(json.dumps({"id": self.n, "summary": summary, "body": body, "actions": actions}), flush=True)
        return self.n
    @method()
    def CloseNotification(self, id: "u"):
        print(json.dumps({"closed": id}), flush=True)
    @method()
    def GetCapabilities(self) -> "as":
        return ["actions", "body"]
    @method()
    def GetServerInformation(self) -> "ssss":
        return ["fake", "test", "1", "1.2"]
    @signal()
    def ActionInvoked(self, id_action) -> "us":
        return id_action
    @signal()
    def NotificationClosed(self, x) -> "uu":
        return x

async def main():
    bus = await MessageBus().connect()
    n = N(); bus.export("/org/freedesktop/Notifications", n)
    await bus.request_name("org.freedesktop.Notifications")
    print("ready", flush=True)
    loop = asyncio.get_running_loop()
    while True:
        line = await loop.run_in_executor(None, sys.stdin.readline)
        if not line: return
        _, i, a = line.split()
        n.ActionInvoked([int(i), a])
asyncio.run(main())
