"""Real TLS/paired-phone integration. uv run --with python-xlib python testing/computer/test_computer.py"""
import json, os, re, socket, subprocess, time
from pathlib import Path
from Xlib import display
ROOT=Path(__file__).resolve().parents[2]
BIN=Path(os.environ.get('CARGO_TARGET_DIR',ROOT/'target'))/'debug'
W=Path(os.environ.get('TETHER_UI_DIR','/tmp/tw-computer'))/'protocol'
W.mkdir(parents=True, exist_ok=True)
env=os.environ.copy()
env.update(XDG_CONFIG_HOME=str(W/'cfg'),XDG_DATA_HOME=str(W/'data'),XDG_RUNTIME_DIR=str(W/'run'),DISPLAY=':71',FAKE_COMPUTER_DIR=str(W),PATH=f'{ROOT}/testing/computer:'+env['PATH'],RUST_LOG='info')
env.pop('WAYLAND_DISPLAY',None)
for p in [W/'cfg/tether',W/'run']: p.mkdir(parents=True,exist_ok=True)
(W/'run').chmod(0o700)
(W/'cfg/tether/config.json').write_text(json.dumps({'device_name':'Computer test','clipboard_sync':False,'mirror_notifications':False,'auto_update':False,'commands':[{'id':'touch','name':'Touch test','command':f'touch {W}/touched'}]}))
(W/'run/tether/tether.sock').unlink(missing_ok=True)  # a stale socket from an earlier run would end the wait below too early
for name in ['send.jsonl','received.jsonl','touched','calls.log','volume.json']:
 (W/name).unlink(missing_ok=True)
procs=[]
def wait(fn, timeout=20):
 end=time.monotonic()+timeout
 while time.monotonic()<end:
  try:
   value=fn()
   if value: return value
  except (OSError,ValueError,KeyError): pass
  time.sleep(.08)
 raise AssertionError('Timed out: '+str(fn))
def api(method,params=None):
 with socket.socket(socket.AF_UNIX) as s:
  s.settimeout(10);s.connect(str(W/'run/tether/tether.sock'))
  s.sendall((json.dumps({'id':1,'method':method,'params':params or {}})+'\n').encode())
  for line in s.makefile():
   r=json.loads(line)
   if r.get('id')==1:
    assert 'error' not in r,r
    return r['result']
def send(kind,body):
 with (W/'send.jsonl').open('a') as f:f.write(json.dumps({'id':1,'type':'kdeconnect.'+kind,'body':body})+'\n')
def packets(kind):
 return [p for line in (W/'received.jsonl').read_text().splitlines() if (p:=json.loads(line))['type']=='kdeconnect.'+kind]
def calls():return (W/'calls.log').read_text()
try:
 # This test owns only the processes it starts; existing displays are left alone.
 if not Path('/tmp/.X11-unix/X71').exists():
  procs.append(subprocess.Popen(['Xvfb',':71','-screen','0','1400x900x24','-nolisten','tcp'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL))
 wait(lambda:Path('/tmp/.X11-unix/X71').exists())
 log=(W/'daemon.log').open('w')
 procs.append(subprocess.Popen(['dbus-run-session','--',str(BIN/'tether'),'daemon'],env=env,stdout=log,stderr=log))
 wait(lambda:(W/'run/tether/tether.sock').exists())
 identity=api('identity'); port=identity['tcpPort']
 phone_log=(W/'phone.log').open('w')
 procs.append(subprocess.Popen([str(BIN/'examples/fake_phone'),f'127.0.0.1:{port}',str(W/'phone')],env=env,stdout=phone_log,stderr=phone_log))
 dev=wait(lambda: next((d for d in api('devices') if d['pair'] in ('requested_by_peer','paired')),None))
 if dev['pair']!='paired':api('accept',{'device':dev['id']})
 wait(lambda:any(d['pair']=='paired' and d['connected'] for d in api('devices')))
 wait(lambda:packets('runcommand'))
 send('runcommand.request',{'requestCommandList':True})
 wait(lambda:json.loads(packets('runcommand')[-1]['body']['commandList']).get('touch'))
 send('runcommand.request',{'key':'touch'});wait(lambda:(W/'touched').exists())
 print('ok command list and detached execution')
 x=display.Display(':71');root=x.screen().root;root.warp_pointer(300,300);x.sync()
 send('mousepad.request',{'dx':30,'dy':20});wait(lambda:(root.query_pointer().root_x,root.query_pointer().root_y)==(330,320))
 send('presenter',{'dx':10,'dy':5});wait(lambda:(root.query_pointer().root_x,root.query_pointer().root_y)==(340,325))
 send('presenter',{'stop':True})
 for body in [{'singleclick':True},{'doubleclick':True},{'middleclick':True},{'rightclick':True},{'singlehold':True},{'singlerelease':True},{'scroll':True,'dy':10},{'key':'é🦀','sendAck':True},{'specialKey':8,'ctrl':True,'sendAck':True}]:send('mousepad.request',body)
 wait(lambda:len(packets('mousepad.echo'))>=2)
 print('ok XTest pointer, presenter, clicks, keyboard, Unicode and acknowledgements')
 send('lock.request',{'setLocked':True});wait(lambda:'loginctl lock-session' in calls())
 send('lock.request',{'requestLocked':True});wait(lambda:packets('lock'))
 print('ok lock helper and state reply')
 send('systemvolume.request',{'requestSinks':True});wait(lambda:packets('systemvolume')[-1]['body']['sinkList'][0]['volume']==42)
 send('systemvolume.request',{'name':'test-sink','volume':67,'muted':True,'enabled':True})
 wait(lambda:any(p['body']['sinkList'][0]['volume']==67 and p['body']['sinkList'][0]['muted'] for p in packets('systemvolume')))
 print('ok sink list, volume, mute, default and change push')
 send('findmyphone.request',{});wait(lambda:api('computer_status')['ringing']);wait(lambda:'ring' in calls())
 send('findmyphone.request',{});wait(lambda:not api('computer_status')['ringing'])
 send('findmyphone.request',{});wait(lambda:api('computer_status')['ringing']);api('computer_ring_stop');wait(lambda:not api('computer_status')['ringing'])
 print('ok ring, second-request stop and window stop API')
 api('settings_set',{'allow_commands':False,'allow_input':False,'allow_lock':False,'allow_volume':False,'allow_ring':False})
 time.sleep(1)  # a sound stub started just before the stop may still be logging
 (W/'touched').unlink();before=calls();root.warp_pointer(300,300);x.sync()
 for kind,body in [('runcommand.request',{'key':'touch'}),('mousepad.request',{'dx':50}),('lock.request',{'setLocked':True}),('systemvolume.request',{'name':'test-sink','volume':20}),('findmyphone.request',{})]:send(kind,body)
 time.sleep(1)
 assert not (W/'touched').exists();px=root.query_pointer().root_x;assert px==300,px;assert calls()==before;assert not api('computer_status')['ringing']
 print('ok all permission switches block side effects')
 x.close()
finally:
 try:api('shutdown')
 except OSError:pass
 for p in reversed(procs):
  p.terminate()
  try:p.wait(timeout=5)
  except subprocess.TimeoutExpired:p.kill();p.wait()
