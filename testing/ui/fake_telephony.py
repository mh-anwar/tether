"""Stand-in for PipeWire's Bluetooth telephony service (org.pipewire.Telephony) on the session bus.

Mirrors the objects in pipewire spa/plugins/bluez5/telephony.c: /org/pipewire/Telephony/ag1 is the
phone (AudioGateway1 + AudioGatewayTransport1), its calls are /org/pipewire/Telephony/ag1/callN
(Call1). Driven by lines on stdin, so a window test can act as the phone:
  gateway            the phone connects for calls
  ring <number>      an incoming call
  remote_hangup      the other side hangs up every call
Every method call is printed as "called <Method> <args>" for the test to check.

Run: uv run --no-project --with dbus-fast python testing/ui/fake_telephony.py
"""

import asyncio
import sys

from dbus_fast import Variant
from dbus_fast.aio import MessageBus
from dbus_fast.service import PropertyAccess, ServiceInterface, dbus_property, method

ROOT = "/org/pipewire/Telephony"
AG = f"{ROOT}/ag1"


def log(*parts):
    print(*parts, flush=True)


class Call(ServiceInterface):
    def __init__(self, phone, path, number, state):
        super().__init__("org.pipewire.Telephony.Call1")
        self.phone, self.path, self.number, self.state = phone, path, number, state

    @method()
    def Answer(self):
        log("called Answer", self.path)
        self.set_state("active")

    @method()
    def Hangup(self):
        log("called Hangup", self.path)
        self.phone.remove(self)

    def set_state(self, state):
        self.state = state
        self.emit_properties_changed({"State": state})

    @dbus_property(access=PropertyAccess.READ)
    def LineIdentification(self) -> "s":
        return self.number

    @dbus_property(access=PropertyAccess.READ)
    def IncomingLine(self) -> "s":
        return ""

    @dbus_property(access=PropertyAccess.READ)
    def Name(self) -> "s":
        return ""

    @dbus_property(access=PropertyAccess.READ)
    def Multiparty(self) -> "b":
        return False

    @dbus_property(access=PropertyAccess.READ)
    def State(self) -> "s":
        return self.state


class Gateway(ServiceInterface):
    def __init__(self, phone):
        super().__init__("org.pipewire.Telephony.AudioGateway1")
        self.phone = phone

    @method()
    def Dial(self, number: "s"):
        log("called Dial", number)
        call = self.phone.add(number, "dialing")
        # The callee picks up a moment later, as a real call would.
        asyncio.get_running_loop().call_later(1.5, lambda: call.state == "dialing" and call.set_state("active"))

    @method()
    def HangupAll(self):
        log("called HangupAll")
        for c in list(self.phone.calls):
            self.phone.remove(c)

    @method()
    def SendTones(self, tones: "s"):
        log("called SendTones", tones)

    @method()
    def HoldAndAnswer(self):
        log("called HoldAndAnswer")

    @dbus_property(access=PropertyAccess.READ)
    def Address(self) -> "s":
        return "AA:BB:CC:DD:EE:FF"


class Transport(ServiceInterface):
    def __init__(self):
        super().__init__("org.pipewire.Telephony.AudioGatewayTransport1")
        self.state = "idle"

    @method()
    def Activate(self):
        log("called Activate")
        self.state = "active"
        self.emit_properties_changed({"State": "active"})

    @dbus_property(access=PropertyAccess.READ)
    def State(self) -> "s":
        return self.state


class Phone:
    def __init__(self, bus):
        self.bus, self.calls, self.next = bus, [], 0
        self.gateway, self.transport = Gateway(self), Transport()

    def connect(self):
        self.bus.export(AG, self.gateway)
        self.bus.export(AG, self.transport)

    def add(self, number, state):
        self.next += 1
        call = Call(self, f"{AG}/call{self.next}", number, state)
        self.calls.append(call)
        self.bus.export(call.path, call)
        return call

    def remove(self, call):
        call.set_state("disconnected")
        self.calls.remove(call)
        self.bus.unexport(call.path, call)
        if not self.calls:
            self.transport.state = "idle"
            self.transport.emit_properties_changed({"State": "idle"})


async def main():
    bus = await MessageBus().connect()
    phone = Phone(bus)
    await bus.request_name("org.pipewire.Telephony")
    log("ready")
    loop = asyncio.get_running_loop()
    while True:
        line = await loop.run_in_executor(None, sys.stdin.readline)
        if not line:
            return
        cmd, _, arg = line.strip().partition(" ")
        if cmd == "gateway":
            phone.connect()
        elif cmd == "ring":
            phone.add(arg, "incoming")
        elif cmd == "remote_hangup":
            for c in list(phone.calls):
                phone.remove(c)
        log("did", cmd)


asyncio.run(main())
