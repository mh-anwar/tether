"""Run via dbus-run-session -- uv run --with dbus-next --with python-xlib python testing/media/e2e.py."""
import asyncio
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BIN = Path(os.environ['CARGO_TARGET_DIR']) / 'debug'

async def main():
    work = Path(tempfile.mkdtemp(prefix='tm-media-'))
    procs = []
    env = os.environ.copy()
    env.update(TOKIO_WORKER_THREADS='2', RUST_LOG='info', DISPLAY=':72', MEDIA_TEST_DIR=str(work), XDG_CONFIG_HOME=str(work/'cfg'), XDG_DATA_HOME=str(work/'data'), XDG_RUNTIME_DIR=str(work/'run'), FAKE_PHONE_CONTROL=str(work/'control'))
    for p in ['cfg/tether', 'run', 'control', 'bin']:
        (work / p).mkdir(parents=True)
    (work/'run').chmod(0o700)
    (work/'cfg/tether/config.json').write_text(json.dumps({'device_name': 'Media test', 'send_notifications': True, 'notification_blocklist': ['Blocked App'], 'clipboard_sync': False, 'downloads_dir': str(work/'downloads'), 'auto_update':False}))
    (work/'art.png').write_bytes((ROOT/'testing/fake-art/icon-whatsapp.png').read_bytes())
    opener = work/'bin/xdg-open'
    opener.write_text('#!/bin/sh\nprintf "%s\\n" "$1" >> "$MEDIA_TEST_DIR/opened"\n')
    opener.chmod(0o755)
    env['PATH'] = str(work/'bin') + ':' + env['PATH']
    def spawn(args, log):
        f = (work/log).open('w')
        p = subprocess.Popen(args, env=env, stdout=f, stderr=subprocess.STDOUT)
        f.close(); procs.append(p); return p
    async def until(fn, label):
        for _ in range(150):
            try:
                if fn(): print('ok',label,flush=True); return
            except (OSError,ValueError): pass
            await asyncio.sleep(.1)
        raise AssertionError(label)
    async def api(method, params=None):
        r, writer = await asyncio.open_unix_connection(str(work/'run/tether/tether.sock'))
        writer.write((json.dumps({'id':1,'method':method,'params':params or {}})+'\n').encode()); await writer.drain()
        progress = []
        try:
            while line := await asyncio.wait_for(r.readline(),15):
                v = json.loads(line)
                if v.get('update',{}).get('kind') == 'transfer_progress': progress.append(v['update'])
                if v.get('id') == 1:
                    assert 'error' not in v, v
                    return v['result'], progress
        finally:
            writer.close(); await writer.wait_closed()
    def desktop_log(): return (work/'desktop.log').read_text()
    def desktop(s): return s in desktop_log()
    def packets():
        return [json.loads(x) for x in (work/'control/received.jsonl').read_text().splitlines()]
    seq = 0
    def send(kind, body, payload=None):
        nonlocal seq
        seq += 1
        v = {'id':seq,'type':kind,'body':body}
        if payload: v['testPayloadPath'] = str(payload)
        tmp = work/'control/send.tmp'; tmp.write_text(json.dumps(v)); tmp.rename(work/f'control/{seq:04}.json')
    def media(body): send('kdeconnect.mpris.request',{'player':'Desktop Test',**body})
    try:
        if not Path('/tmp/.X72-lock').exists():
            spawn(['Xvfb', ':72', '-screen', '0', '1400x900x24', '-nolisten', 'tcp'], 'xvfb.log')
            await asyncio.sleep(.5)
        spawn([os.sys.executable, str(ROOT/'testing/media/standin.py')], 'standin.log')
        await until(lambda:(work/'ready').exists(),'desktop stand-in ready')
        daemon = spawn([str(BIN/'tether'),'daemon'],'daemon.log')
        await until(lambda:(work/'run/tether/tether.sock').exists(),'daemon ready')
        # Read actual port from identity announcement is unnecessary: tests run on a free TCP port from the daemon log.
        import re
        await until(lambda: bool(re.search(r'on tcp (17\d\d)',(work/'daemon.log').read_text())), 'daemon listening')
        port = re.search(r'on tcp (17\d\d)',(work/'daemon.log').read_text()).group(1)
        spawn([str(BIN/'examples/fake_phone'),f'127.0.0.1:{port}',str(work/'phone')],'phone.log')
        await until(lambda: 'asked to pair' in (work/'phone.log').read_text(),'phone pair request')
        subprocess.run([str(BIN/'tether'),'accept'],env=env,check=True,stdout=subprocess.DEVNULL)
        await until(lambda:(work/'phone/paired').exists(),'phone paired')
        media({'requestPlayerList':True,'requestNowPlaying':True})
        await until(lambda:any('Desktop Test' in p['body'].get('playerList',[]) for p in packets()),'phone receives desktop player list')
        await until(lambda:any(p['body'].get('title')=='Desktop song' and p['body'].get('length')==180000 for p in packets()),'phone receives now playing in milliseconds')
        assert not any('tether' in n.lower() for p in packets() for n in p['body'].get('playerList',[]))
        media({'action':'Next'})
        await until(lambda:desktop('Next'),'phone Next reaches desktop')
        media({'setVolume':35,'setLoopStatus':'Track','setShuffle':True,'Seek':1000})
        await until(lambda:desktop('Volume 0.35') and desktop('Seek 1000'),'volume and relative seek')
        await until(lambda:any(p['body'].get('loopStatus')=='Track' and p['body'].get('shuffle') for p in packets()),'property signals pushed')
        media({'albumArtUrl':(work/'art.png').as_uri()})
        await until(lambda:(work/'control/payload.bin').exists(),'album art payload received')
        send('kdeconnect.telephony',{'event':'ringing'})
        await until(lambda:desktop('Pause'),'ringing pauses desktop')
        send('kdeconnect.telephony',{'event':'ringing','isCancel':'true'})
        await until(lambda:desktop('Play\n'),'call end resumes desktop')
        subprocess.run(['gdbus','call','--session','--dest','org.freedesktop.Notifications','--object-path','/org/freedesktop/Notifications','--method','org.freedesktop.Notifications.Notify','Editor','0',str(work/'art.png'),'Saved','Document','[]','{}','5000'],env=env,check=True,stdout=subprocess.DEVNULL)
        await until(lambda:any(p['body'].get('appName')=='Editor' for p in packets()),'desktop notification reaches phone')
        send('kdeconnect.notification.request',{'cancel':'tether-desktop:77'})
        await until(lambda:desktop('CloseNotification 77'),'phone dismisses desktop notification')
        def notify(app, title):
            subprocess.run(['gdbus','call','--session','--dest','org.freedesktop.Notifications','--object-path','/org/freedesktop/Notifications','--method','org.freedesktop.Notifications.Notify',app,'0','',title,'x','[]','{}','5000'],env=env,check=True,stdout=subprocess.DEVNULL)
        notify('Blocked App','Hidden')
        send('kdeconnect.notification',{'id':'phone-1','appName':'PhoneApp','title':'From phone','text':'hello','isClearable':True,'time':'1'})
        await until(lambda:desktop('Notify PhoneApp'),'phone notification mirrored to desktop')
        notify('Control App','Marker')
        await until(lambda:any(p['body'].get('appName')=='Control App' for p in packets()),'later desktop notification still forwarded')
        names = [p['body'].get('appName') for p in packets() if p['type']=='kdeconnect.notification']
        assert 'Blocked App' not in names, names
        assert 'PhoneApp' not in names, names
        print('ok blocklisted and mirrored notifications are not forwarded')
        # Talking mode with mute: ringing leaves the player alone, answering mutes it, hanging up restores the volume.
        cfg = work/'cfg/tether/config.json'
        conf = json.loads(cfg.read_text()); conf.update(pause_media='talking', mute_media=True); cfg.write_text(json.dumps(conf))
        before = desktop_log().count('Volume')
        send('kdeconnect.telephony',{'event':'ringing'})
        await asyncio.sleep(.8)
        assert desktop_log().count('Volume') == before, 'talking mode must not act on ringing'
        send('kdeconnect.telephony',{'event':'talking'})
        await until(lambda:desktop('Volume 0.0'),'answering mutes desktop player')
        send('kdeconnect.telephony',{'event':'talking','isCancel':'true'})
        await until(lambda:desktop_log().rstrip().endswith('Volume 0.35'),'hanging up restores the volume')
        source = work/'source'; source.write_text('shared bytes')
        devices, _ = await api('devices')
        result, progress = await api('share_files', {'device': devices[0]['id'], 'paths': [str(source),str(source)]})
        assert result['bytes'] == 24 and any(p['bytes'] == 12 and p['number_of_files'] == 2 for p in progress)
        await until(lambda:sum(1 for p in packets() if p['type']=='kdeconnect.share.request' and p['body'].get('numberOfFiles')==2)==2, 'multiple files sent to phone with progress')
        send('kdeconnect.share.request.update',{'numberOfFiles':2,'totalPayloadSize':24})
        for _ in range(2): send('kdeconnect.share.request',{'filename':'photo.txt','numberOfFiles':2,'totalPayloadSize':24,'open':True},source)
        await until(lambda:len(list((work/'downloads').glob('*.txt')))==2 and all(p.read_text()=='shared bytes' for p in (work/'downloads').glob('*.txt')),'multi-file receive uses unique names')
        await until(lambda:len((work/'opened').read_text().splitlines())>=2,'open flag opens received files')
        send('kdeconnect.share.request',{'url':'https://example.org/from-phone'})
        await until(lambda:'https://example.org/from-phone' in (work/'opened').read_text(),'shared URL opens browser')
        send('kdeconnect.share.request',{'text':'Text shared by phone'})
        await asyncio.sleep(.3)
        clip = subprocess.run([os.sys.executable,str(ROOT/'testing/ui/clip.py'),':72','get'],env=env,capture_output=True,text=True,check=True)
        assert clip.stdout.strip() == 'Text shared by phone', clip.stdout
        print('ok shared text is on desktop clipboard')
        await asyncio.sleep(.5)
        assert not any(p['body'].get('appName')=='Tether' for p in packets())
        print('ok Tether notifications excluded; artifacts',work)
    except Exception:
        print('Artifacts:',work)
        raise
    finally:
        for p in reversed(procs): p.terminate()
        for p in procs:
            try: p.wait(timeout=5)
            except subprocess.TimeoutExpired: p.kill(); p.wait()

asyncio.run(main())
