#!/usr/bin/env python3
"""Exercise the packaged Linux binary with fresh synthetic data and no model/LAN."""
import contextlib,hashlib,json,os,pathlib,platform,signal,socket,subprocess,sys,time,uuid

def main():
 if platform.system()!='Linux':raise ValueError('actual Linux runtime required')
 package=pathlib.Path(sys.argv[1]).resolve();out=pathlib.Path(sys.argv[2]).resolve()
 if out.exists():raise ValueError('fresh retained runtime evidence directory required')
 out.mkdir();program=package/'bin/crewd';expected=hashlib.sha256(program.read_bytes()).hexdigest()
 for d in ['home','data','work']:(out/d).mkdir(mode=0o700)
 env={'PATH':'/usr/bin:/bin','HOME':str(out/'home'),'XDG_CACHE_HOME':str(out/'home/cache'),'SHELL':'/usr/bin/false'}
 def command(*args):
  run=subprocess.run([str(program),'--data-dir',str(out/'data'),*args],env=env,capture_output=True,timeout=30)
  (out/(args[0]+'-'+str(time.time_ns())+'.log')).write_bytes(run.stdout+run.stderr)
  if run.returncode:raise ValueError('packaged CLI failed: '+args[0])
  return run
 @contextlib.contextmanager
 def serve(label):
  argv=[str(program),'--data-dir',str(out/'data'),'serve','--codex','/usr/bin/false','--claude','/usr/bin/false','--codex-env','HOME='+str(out/'home'),'--claude-env','HOME='+str(out/'home')]
  with (out/(label+'.log')).open('w') as log:
   process=subprocess.Popen(argv,env=env,stdout=log,stderr=log);s=None
   try:
    for _ in range(200):
     if process.poll() is not None:raise ValueError('packaged daemon exited before readiness')
     s=socket.socket(socket.AF_UNIX);s.settimeout(10)
     try:s.connect(str(out/'data/crewd.sock'));break
     except OSError:s.close();s=None;time.sleep(.05)
    if s is None:raise ValueError('packaged daemon socket readiness timed out')
    f=s.makefile('rb');sequence=0
    def call(method,params):
     nonlocal sequence
     sequence+=1;s.sendall((json.dumps({'id':sequence,'method':method,'params':params})+'\n').encode())
     while True:
      line=f.readline()
      if not line:raise ValueError('unexpected result loss in packaged smoke')
      value=json.loads(line)
      if value.get('id')==sequence:
       if 'error' in value:raise ValueError('RPC failed '+method+': '+json.dumps(value['error']))
       return value['result']
    try:yield call
    finally:f.close();s.close();s=None
   finally:
    if s:s.close()
    if process.poll() is None:process.send_signal(signal.SIGTERM)
    try:process.wait(timeout=20)
    except subprocess.TimeoutExpired:
     process.kill();process.wait();raise ValueError('packaged shutdown exceeded deadline')
    if process.returncode!=0:raise ValueError('packaged daemon shutdown failure')
 request=str(uuid.uuid4())
 with serve('first-open') as call:
  remote=call('remote.status',{})
  if remote['listening'] or remote['configuration']['enabled']:raise ValueError('remote unexpectedly enabled')
  crew=call('crew.create',{'title':'Linux distribution synthetic work','workingDirectory':str(out/'work')})['id']
  params={'crewId':crew,'requestId':request,'text':'packaged release persists across restore'}
  made=call('todo.create',params)
 command('backup');backups=list((out/'data/backups').glob('crewd-manual-*.db'))
 if len(backups)!=1:raise ValueError('actual protective backup missing')
 command('restore','--from',str(backups[0]));command('check')
 with serve('restored-open') as call:
  recovered=call('todo.create',params)
  if recovered['requirementId']!=made['requirementId']:raise ValueError('restored request identity duplicated')
  remote=call('remote.status',{})
  if remote['listening'] or remote['configuration']['enabled']:raise ValueError('restore unexpectedly activated remote')
 if hashlib.sha256(program.read_bytes()).hexdigest()!=expected:raise ValueError('packaged executable changed during smoke')
 (out/'result.json').write_text(json.dumps({'passed':True,'binarySHA256':expected,'sourceRef':json.loads((package/'candidate.json').read_text())['sourceRef'],'providerProcesses':'disabled /usr/bin/false','remoteEnabled':False,'freshData':True,'originalRequestId':request,'requirementId':made['requirementId'],'packagedReopenBackupRestore':True},indent=2)+'\n')
 print('PASS actual packaged Linux release/work/reopen/backup restore/request identity/default-off remote')
try:main()
except (ValueError,OSError,KeyError,subprocess.SubprocessError) as e:raise SystemExit('FAIL: '+str(e))
