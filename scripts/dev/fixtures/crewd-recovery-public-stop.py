import importlib.util,pathlib,subprocess,os,time,json,sys
import crewd_recovery_common as f
a,witness=f.qualified_args()
birth=f.birth
out=pathlib.Path(a.out);out.mkdir(mode=0o700)
for d in ['data','home','cache','config','work']:(out/d).mkdir(mode=0o700)
binary=pathlib.Path(a.binary);codex=pathlib.Path(a.fake_codex);cl=pathlib.Path(a.fake_claude);logpath=out/'input.ndjson';c=None;p=None;physical=None
result={'layer':'actual public Start/Stop current kernel birth/boot; exact strong normal, no SDK/model','fixtureSHA':f.sha(__file__),'binarySHA':f.sha(binary),'fakeCodexSHA':f.sha(codex),'sourceRef':witness['sourceRef'],'originalWitness':a.witness}
try:
 env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(out/'home'),'XDG_CACHE_HOME':str(out/'cache'),'CLAUDE_CONFIG_DIR':str(out/'config'),'TMPDIR':str(out),'SHELL':'/usr/bin/false'}
 with (out/'serve.log').open('xb') as log:p=subprocess.Popen([str(binary),'--data-dir',str(out/'data'),'serve','--codex',str(codex),'--codex-env','FAKE_CODEX_LOG='+str(logpath),'--claude',str(cl)],env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=log)
 b=f.wait_for(lambda:birth(p.pid),'actual owneddaemon');f.wait_for(lambda:(out/'data/crewd.sock').exists(),'publicsocket');c=f.Public(out/'data/crewd.sock',b)
 def cmd(op,params,want):
  r=c.prepared(op,params);f.save(out/(r['requestId']+'.prepared.json'),r);c.submit(r);return c.phase(r,want)
 home=cmd('crew.createFirstMessage',dict(crew=dict(title='stop root',workingDirectory=str(out/'work')),text='ROOT'),['committed'])
 group=cmd('crew.createFirstMessage',dict(crew=dict(title='stop child',workingDirectory=str(out/'work'),parentId=home['result']['crew']['id']),text='CHILD'),['committed'])
 launched=cmd('session.start',dict(crewId=group['result']['crew']['id'],role='worker',executor='codex',brief='BOOT_STOP_BODY_14',isolation=False),['launched']);physical=launched['result']['birth'];assert f.same(birth(physical['pid']),physical) and bool(physical['bootId'])
 f.wait_for(lambda:logpath.exists() and 'BOOT_STOP_BODY_14' in logpath.read_text(),'actual body input')
 stopped=cmd('session.stop',dict(target=c.rpc('session.controlTarget',dict(sessionId=launched['result']['sessionId'])),reason='whole boot identity stop gate'),['confirmed']);assert not f.same(birth(physical['pid']),physical)
 try:os.kill(-physical['pgid'],0)
 except ProcessLookupError:pass
 else:raise AssertionError('original physical group remains')
 result.update(launched=launched,stop=stopped,originalBirthAndGroupGone=True)
 q=c.prepared('crewd.hibernate',dict(deadlineSecs=0));c.submit(q);assert p.wait(15)==0;result['status']='PASS'
except BaseException as e:result.update(status='FAIL',error=repr(e))
finally:
 cleanupErrors=[]
 if c:
  try:c.close()
  except OSError as e:cleanupErrors.append({'connectionClose':repr(e)})
 if p and p.poll() is None:
  try:
   p.terminate()
   try:p.wait(10)
   except subprocess.TimeoutExpired:
    # This is only the exact unreaped direct Popen child, never a registry PID.
    p.kill();p.wait(5)
  except BaseException as e:cleanupErrors.append({'ownedChildCleanup':repr(e)})
 if p and p.poll() is None:cleanupErrors.append({'ownedChildUnreaped':True})
 if cleanupErrors:result.update(status='FAIL',cleanupErrors=cleanupErrors)
 result['specialInodeEvidence']=f.special_inode_evidence(out)
 result['retainedRegistry']=f.retained_registry(out/'data');result['providerPhysicalAfter']=f.physical_observation(physical)
 result['terminal']=p.returncode if p else None;f.save(out/'result.json',result);print(json.dumps({'status':result['status'],'error':result.get('error'),'result':str(out/'result.json')}),flush=True)
sys.exit(0 if result['status']=='PASS' else 1)
