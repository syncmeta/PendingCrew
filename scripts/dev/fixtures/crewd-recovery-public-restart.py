import pathlib,subprocess,os,sys,json,time,hashlib,importlib.util,signal,shutil
import crewd_recovery_common as flow
a,witness=flow.qualified_args()
birth=flow.birth
binary=pathlib.Path(a.binary)
out=pathlib.Path(a.out);out.mkdir(mode=0o700)
for p in ['data','home','cache','config','work']:(out/p).mkdir(mode=0o700)
data=out/'data';registry=data/'exec-registry.json';logpath=out/'input.ndjson'
codex=pathlib.Path(a.fake_codex);claude=pathlib.Path(a.fake_claude);assert codex.exists() and claude.exists()
env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(out/'home'),'XDG_CACHE_HOME':str(out/'cache'),'CLAUDE_CONFIG_DIR':str(out/'config'),'TMPDIR':str(out),'SHELL':'/usr/bin/false','LANG':'C.UTF-8','GIT_CONFIG_NOSYSTEM':'1','GIT_CONFIG_GLOBAL':'/dev/null'}
result={'layer':'actual exact strong normal production recovery; no SDK/model','binarySHA':flow.sha(binary),'sourceRef':witness['sourceRef'],'originalWitness':a.witness,'fixtureSHA':flow.sha(__file__),'flowLibrarySHA':flow.sha(pathlib.Path(flow.__file__)),'fakeCodexSHA':flow.sha(codex),'fakeClaudeSHA':flow.sha(claude),'events':[]};daemons=[];connections=[];isolated=[];physical=None
def census():
 return {p.name:{'sha':flow.sha(p),'mode':p.stat().st_mode&0o777,'size':p.stat().st_size} for p in data.iterdir() if p.is_file() and p.name!='crewd.lock'}
def cli(tag,args,ok=None):
 start=time.monotonic();r=subprocess.run([str(binary),'--data-dir',str(data),*args],env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=8)
 (out/(tag+'.stdout')).write_bytes(r.stdout);(out/(tag+'.stderr')).write_bytes(r.stderr)
 result['events'].append({'cli':tag,'exit':r.returncode,'elapsed':time.monotonic()-start})
 if ok is not None:assert (r.returncode==0)==ok,(tag,r.returncode,r.stderr)
 return r
counter=0
def start():
 global counter
 counter+=1;f=open(out/f'serve{counter}.log','xb');p=subprocess.Popen([str(binary),'--data-dir',str(data),'serve','--codex',str(codex),'--codex-env','FAKE_CODEX_LOG='+str(logpath),'--claude',str(claude),'--claude-env','FAKE_CLAUDE_MODE=ready'],env=env,stdin=subprocess.DEVNULL,stdout=f,stderr=f);daemons.append(p)
 b=flow.wait_for(lambda:birth(p.pid),'owned daemon kernelbirth');assert b['ppid']==os.getpid()
 flow.wait_for(lambda: flow.socket_owned(data/'crewd.sock',b) or p.poll()!=None,'actual new daemon kernel-qualified socket after recovery or refusal');assert p.poll()==None
 c=flow.Public(data/'crewd.sock',b);connections.append(c);return p,c
try:
 p,c=start();identity=c.identity
 # Root and real child group retain formal scope/owner policy.
 def cmd(op,params,want):
  req=c.prepared(op,params);flow.save(out/(req['requestId']+'.prepared.json'),req);c.submit(req);return req,c.phase(req,want)
 _,home=cmd('crew.createFirstMessage',dict(crew=dict(title='recovery root',workingDirectory=str(out/'work')),text='ROOT_RECOVERY'),['committed'])
 _,child=cmd('crew.createFirstMessage',dict(crew=dict(title='recovery worker',workingDirectory=str(out/'work'),parentId=home['result']['crew']['id']),text='CHILD_RECOVERY'),['committed'])
 request,launched=cmd('session.start',dict(crewId=child['result']['crew']['id'],role='worker',executor='codex',brief='RECOVERY_ONCE_BODY_11',isolation=False),['launched']);physical=launched['result']['birth'];assert flow.same(birth(physical['pid']),physical)
 def bodycount():return sum(x.get('method')=='turn/start' and 'RECOVERY_ONCE_BODY_11' in json.dumps(x) for x in [json.loads(s) for s in logpath.read_text().splitlines()]) if logpath.exists() else 0
 flow.wait_for(lambda:bodycount()==1,'actual original body reader');before=logpath.read_bytes();flow.save(out/'original-launched.json',launched)
 # Hard daemon death leaves real registry and original provider to recover.
 c.close();p.kill();assert p.wait(5)==-9;assert registry.exists();flow.save(out/'registry-after-kill.json',json.loads(registry.read_text()))
 p,c=start();settled=c.lookup(request);assert settled['historical'] and settled['phase']=='launched',settled
 assert c.identity['dbId']==identity['dbId'] and c.identity['runtimeBindingId']!=identity['runtimeBindingId']
 assert not flow.same(birth(physical['pid']),physical)
 try:os.kill(-physical['pgid'],0)
 except ProcessLookupError:pass
 else:raise AssertionError('original group remains after new admission')
 assert c.submit(request)['phase']=='launched';assert bodycount()==1 and logpath.read_bytes()==before
 result['events'].append({'publicRestart':True,'originalBirthAndGroupGone':True,'historicalLaunchedNotCurrentCompletion':settled,'bodyCount':bodycount(),'duplicateNoReplay':True})
 quitreq=c.prepared('crewd.hibernate',dict(deadlineSecs=0));c.submit(quitreq);assert p.wait(15)==0;c.close()
 # Real legacy empty-boot child cannot be signalled or retired merely by PID.
 kid=subprocess.Popen(['/bin/sleep','60'],start_new_session=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);isolated.append(kid);b=flow.wait_for(lambda:birth(kid.pid),'isolated child birth');record={k:b[k] for k in ['pid','pgid','startTime','bootId']};record.update(bootId='',command='isolated-control')
 registry.write_text(json.dumps({'isolated-legacy':record}));os.chmod(registry,0o600);original=registry.read_bytes();snap=census()
 inspected=cli('inspect-live',['recovery','inspect','--json'],True);assert json.loads(inspected.stdout)['signalsPermitted']==False and registry.read_bytes()==original
 unresolved=cli('reconcile-live',['recovery','reconcile','--json'],False);assert not json.loads(unresolved.stdout)['resolved'];assert registry.read_bytes()==original and kid.poll()==None
 refused=cli('serve-unresolved',['serve','--codex','/usr/bin/false','--claude','/usr/bin/false'],False);assert b'unresolved orphan' in refused.stderr and not (data/'crewd.sock').exists() and registry.read_bytes()==original and kid.poll()==None
 # Guard priority: future main + live original obligation must stay entirely untouched.
 db=data/'crewd.db'
 assert db.exists(),[x.name for x in data.iterdir()]
 bytes0=db.read_bytes();raw=bytearray(bytes0);raw[60:64]=(15).to_bytes(4,'big');db.write_bytes(raw);snap=census()
 cli('inspect-future',['recovery','inspect','--json'],True);assert census()==snap
 cli('reconcile-future',['recovery','reconcile','--json'],False);assert census()==snap and kid.poll()==None
 cli('serve-future',['serve','--codex','/usr/bin/false','--claude','/usr/bin/false'],False);assert census()==snap and kid.poll()==None
 db.write_bytes(bytes0)
 # Only this test's exact unreaped direct child is stopped, outside product diagnostic.
 assert flow.same(birth(kid.pid),b);kid.kill();kid.wait(5)
 gone=cli('reconcile-confirmed',['recovery','reconcile','--json'],True);j=json.loads(gone.stdout);assert j['resolved'] and all(not x['signalSent'] and x['confirmedGone'] for x in j['reports']) and json.loads(registry.read_text())=={}
 p,c=start();assert c.lookup(request)['historical'] and c.lookup(request)['phase']=='launched' and bodycount()==1 and logpath.read_bytes()==before
 q=c.prepared('crewd.hibernate',dict(deadlineSecs=0));c.submit(q);assert p.wait(15)==0;c.close();result['status']='PASS'
except BaseException as e:result['status']='FAIL';result['error']=repr(e)
finally:
 for c in connections:
  try:c.close()
  except OSError:pass
 for p in daemons:
  if p.poll() is None:
   p.terminate()
   try:p.wait(10)
   except subprocess.TimeoutExpired:p.kill();p.wait(5)
 for k in isolated:
  if k.poll() is None:k.kill();k.wait(5)
 result['specialInodeEvidence']=flow.special_inode_evidence(out)
 result['retainedRegistry']=flow.retained_registry(data);result['providerPhysicalAfter']=flow.physical_observation(physical)
 result['daemonTerminal']=[p.returncode for p in daemons];result['isolatedTerminal']=[k.returncode for k in isolated];flow.save(out/'result.json',result);print(json.dumps({'status':result['status'],'error':result.get('error'),'result':str(out/'result.json')}),flush=True)
sys.exit(0 if result['status']=='PASS' else 1)
