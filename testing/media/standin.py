"""Private-bus desktop MPRIS player and notification server for media e2e."""
import asyncio
import os
from pathlib import Path
from dbus_next import Variant
from dbus_next.aio import MessageBus
from dbus_next.service import ServiceInterface, method, dbus_property, signal
from dbus_next.constants import PropertyAccess

ROOT = Path(os.environ['MEDIA_TEST_DIR'])

def record(value):
    with (ROOT / 'desktop.log').open('a') as f:
        f.write(value + '\n')

class Root(ServiceInterface):
    def __init__(self):
        super().__init__('org.mpris.MediaPlayer2')
    @dbus_property(access=PropertyAccess.READ)
    def Identity(self) -> 's':
        return 'Desktop Test'

class Player(ServiceInterface):
    def __init__(self):
        super().__init__('org.mpris.MediaPlayer2.Player')
        self.playing = True
        self.volume = 0.6
        self.loop = 'None'
        self.shuffle = False
        self.pos = 9000000
    @method()
    def PlayPause(self):
        self.playing = not self.playing
        record('PlayPause')
        self.emit_properties_changed({'PlaybackStatus': self.PlaybackStatus})
    @method()
    def Play(self):
        self.playing = True
        record('Play')
        self.emit_properties_changed({'PlaybackStatus': self.PlaybackStatus})
    @method()
    def Pause(self):
        self.playing = False
        record('Pause')
        self.emit_properties_changed({'PlaybackStatus': self.PlaybackStatus})
    @method()
    def Stop(self):
        self.Pause()
    @method()
    def Next(self):
        record('Next')
    @method()
    def Previous(self):
        record('Previous')
    @method()
    def Seek(self, offset: 'x'):
        self.pos += offset
        record('Seek ' + str(offset))
        self.Seeked(self.pos)
    @signal()
    def Seeked(self, position: 'x') -> 'x':
        return position
    @dbus_property(access=PropertyAccess.READ)
    def PlaybackStatus(self) -> 's':
        return 'Playing' if self.playing else 'Paused'
    @dbus_property(access=PropertyAccess.READ)
    def Metadata(self) -> 'a{sv}':
        return {'xesam:title': Variant('s', 'Desktop song'), 'xesam:artist': Variant('as', ['Test artist']),
                'xesam:album': Variant('s', 'Test album'), 'mpris:length': Variant('x', 180000000),
                'mpris:artUrl': Variant('s', (ROOT / 'art.png').as_uri())}
    @dbus_property(access=PropertyAccess.READ)
    def Position(self) -> 'x':
        return self.pos
    @dbus_property()
    def Volume(self) -> 'd':
        return self.volume
    @Volume.setter
    def Volume(self, value: 'd'):
        self.volume = value
        record('Volume ' + str(value))
        self.emit_properties_changed({'Volume': value})
    @dbus_property()
    def LoopStatus(self) -> 's':
        return self.loop
    @LoopStatus.setter
    def LoopStatus(self, value: 's'):
        self.loop = value
        self.emit_properties_changed({'LoopStatus': value})
    @dbus_property()
    def Shuffle(self) -> 'b':
        return self.shuffle
    @Shuffle.setter
    def Shuffle(self, value: 'b'):
        self.shuffle = value
        self.emit_properties_changed({'Shuffle': value})
    @dbus_property(access=PropertyAccess.READ)
    def CanPause(self) -> 'b': return True
    @dbus_property(access=PropertyAccess.READ)
    def CanPlay(self) -> 'b': return True
    @dbus_property(access=PropertyAccess.READ)
    def CanGoNext(self) -> 'b': return True
    @dbus_property(access=PropertyAccess.READ)
    def CanGoPrevious(self) -> 'b': return True
    @dbus_property(access=PropertyAccess.READ)
    def CanSeek(self) -> 'b': return True

class Notifications(ServiceInterface):
    def __init__(self): super().__init__('org.freedesktop.Notifications')
    @method()
    def Notify(self, app: 's', replaces: 'u', icon: 's', title: 's', body: 's', actions: 'as', hints: 'a{sv}', timeout: 'i') -> 'u':
        record('Notify ' + app)
        return replaces or 77
    @method()
    def GetCapabilities(self) -> 'as': return ['body', 'actions']
    @method()
    def GetServerInformation(self) -> 'ssss': return ['Standin', 'Tether', '1', '1.2']
    @method()
    def CloseNotification(self, id: 'u'):
        record('CloseNotification ' + str(id))
        self.NotificationClosed(id, 3)
    @signal()
    def NotificationClosed(self, id: 'u', reason: 'u') -> 'uu': return [id, reason]

async def main():
    bus = await MessageBus().connect()
    bus.export('/org/mpris/MediaPlayer2', Root())
    bus.export('/org/mpris/MediaPlayer2', Player())
    bus.export('/org/freedesktop/Notifications', Notifications())
    await bus.request_name('org.mpris.MediaPlayer2.desktoptest')
    await bus.request_name('org.freedesktop.Notifications')
    (ROOT / 'ready').touch()
    await asyncio.Future()

asyncio.run(main())
