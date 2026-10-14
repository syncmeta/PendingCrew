#!/usr/bin/env python3
"""Ordinary-user isolated install/11->13/unknown/rollback/Core recovery unit."""
import argparse,contextlib,fcntl,importlib.util,hashlib,json,os,pathlib,signal,socket,sqlite3,subprocess,sys,time,uuid

def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 a=argparse.ArgumentParser();a.add_argument('old_package');a.add_argument('new_package');a.add_argument('output');a.add_argument('--new-schema',type=int,choices=[13,14],default=13);args=a.parse_args()
 old,new=pathlib.Path(args.old_package).resolve(),pathlib.Path(args.new_package).resolve();out=pathlib.Path(args.output).resolve()
 if os.getuid()!=os.geteuid() or os.geteuid()==0:raise ValueError('actual ordinary nonroot runner identity required; sudo cannot establish personal installation acceptance')
 if sys.platform!='linux':raise ValueError('actual Linux lifecycle required')
 if out.exists():raise ValueError('fresh evidence directory required')
 out.mkdir(mode=0o700);prefix=out/'isolated-prefix';data=out/'synthetic-data';home=out/'home';work=out/'work'
 for p in [data,home,work]:p.mkdir(mode=0o700)
 if len(str(data/'crewd.sock').encode())>103:raise ValueError('use an explicit short private evidence directory for actual Unix socket')
 installer=pathlib.Path(__file__).with_name('install-crewd-linux-candidate.py')
 oldmeta=json.loads((old/'candidate.json').read_text());newmeta=json.loads((new/'candidate.json').read_text())
 if oldmeta['sourceRef']==newmeta['sourceRef'] or oldmeta['binarySHA256']==newmeta['binarySHA256']:raise ValueError('two real immutable versions required; same tar is not upgrade acceptance')
 env={'PATH':'/usr/bin:/bin','HOME':str(home),'XDG_CACHE_HOME':str(home/'cache'),'SHELL':'/usr/bin/false'}
 records=[]
 def install(label,action,request,expected,package=None,source=None,want=True):
  cmd=[sys.executable,str(installer),action,'--prefix',str(prefix),'--request-id',request,'--expected-current',expected or 'none']
  if package:cmd+=['--package',str(package)]
  if source:cmd+=['--source-ref',source]
  r=subprocess.run(cmd,capture_output=True,text=True,timeout=30)
  (out/(label+'.log')).write_text(r.stdout+r.stderr)
  if (r.returncode==0)!=want:raise ValueError('installer result mismatch: '+label)
  records.append({'phase':label,'command':cmd,'exit':r.returncode})
  return json.loads(r.stdout.splitlines()[-1]) if want else None
 def status(request):
  r=subprocess.run([sys.executable,str(installer),'status','--prefix',str(prefix),'--request-id',request],capture_output=True,text=True,check=True,timeout=10)
  return json.loads(r.stdout)
 def cli(label,*args,want=True):
  cmd=[str(prefix/'bin/crewd'),'--data-dir',str(data),*args]
  r=subprocess.run(cmd,env=env,capture_output=True,timeout=30);(out/(label+'.log')).write_bytes(r.stdout+r.stderr)
  if (r.returncode==0)!=want:raise ValueError('packaged CLI result mismatch: '+label)
  return r
 def dbversion(path=data/'crewd.db'):
  with contextlib.closing(sqlite3.connect(path.as_uri()+'?mode=ro',uri=True)) as c:return c.execute('PRAGMA user_version').fetchone()[0]
 def refusal_snapshot():
  # Finished daemon and explicitly closed observers precede this immutable read.
  # Never ignore a nonempty WAL or checkpoint on behalf of the old program.
  wal=data/'crewd.db-wal'
  if wal.exists() and wal.stat().st_size:raise ValueError('refusal baseline has nonempty WAL; preserve it instead of assuming main DB authority')
  with contextlib.closing(sqlite3.connect((data/'crewd.db').as_uri()+'?immutable=1&mode=ro',uri=True)) as c:
   schema=c.execute('SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name').fetchall()
   tables={}
   for kind,name,_,_ in schema:
    if kind!='table':continue
    quoted='"'+name.replace('"','""')+'"'
    rows=c.execute('SELECT * FROM '+quoted).fetchall()
    def encode(v):return {'bytesHex':v.hex()} if isinstance(v,bytes) else v
    tables[name]=sorted([[encode(v) for v in row] for row in rows],key=lambda row:json.dumps(row,sort_keys=True))
   logical={'version':c.execute('PRAGMA user_version').fetchone()[0],'applicationId':c.execute('PRAGMA application_id').fetchone()[0],'schema':schema,'tables':tables,'integrity':c.execute('PRAGMA integrity_check').fetchall()}
  return {'logical':logical,'physical':{p.name:{'sha256':sha(p),'size':p.stat().st_size} for p in sorted(data.glob('crewd.db*')) if p.is_file()}}
 def birth(pid):
  text=pathlib.Path('/proc',str(pid),'stat').read_text();return text[text.rfind(')')+2:].split()[19]
 def lease_held(pid):
  lease=prefix/'.runtime.lock'
  fd=os.open(lease,os.O_RDWR|os.O_CLOEXEC|os.O_NOFOLLOW)
  try:
   try:fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
   except BlockingIOError:pass
   else:raise ValueError('managed runtime did not retain shared lease')
   inode=os.fstat(fd).st_ino
   assert any(p.stat().st_ino==inode for p in pathlib.Path('/proc',str(pid),'fd').iterdir()),'actual daemon fd must retain lease across exec'
  finally:os.close(fd)
 def wrapper_survival():
  # Own synthetic wrapper intentionally exits immediately. Its child must retain
  # the managed lease until the actual daemon exits; no wrapper-lifetime claim.
  cmd=[str(prefix/'bin/crewd'),'--data-dir',str(data),'serve','--codex','/usr/bin/false','--claude','/usr/bin/false']
  log=out/'wrapper-child.log'
  wrapper='import subprocess,sys; f=open(sys.argv[1],"wb"); p=subprocess.Popen(sys.argv[2:],stdout=f,stderr=f); print(p.pid,flush=True)'
  r=subprocess.run([sys.executable,'-c',wrapper,str(log),*cmd],env=env,capture_output=True,text=True,check=True,timeout=10)
  pid=int(r.stdout.strip());observed=birth(pid)
  try:
   for _ in range(200):
    assert birth(pid)==observed
    probe=socket.socket(socket.AF_UNIX);probe.settimeout(.1)
    try:probe.connect(str(data/'crewd.sock'));break
    except OSError:time.sleep(.05)
    finally:probe.close()
   else:raise ValueError('wrapper survivor did not become ready')
   lease_held(pid)
   install('wrapper-death-live-child-refusal','install',str(uuid.uuid4()),active,package=new,want=False)
  finally:
   if birth(pid)!=observed:raise ValueError('own fixture child birth changed; refuse signal')
   os.kill(pid,signal.SIGTERM)
   for _ in range(400):
    try:
     text=pathlib.Path('/proc',str(pid),'stat').read_text();state=text[text.rfind(')')+2:].split()[0]
     if state=='Z':break
    except FileNotFoundError:break
    time.sleep(.05)
   else:raise ValueError('own wrapper child did not physically exit')
  records.append({'phase':'managed-wrapper-death-child-survival','ownChildPID':pid,'birth':observed,'wrapperExit':r.returncode,'liveChildLeaseRetained':True})
 @contextlib.contextmanager
 def serve(label):
  cmd=[str(prefix/'bin/crewd'),'--data-dir',str(data),'serve','--codex','/usr/bin/false','--claude','/usr/bin/false','--codex-env','HOME='+str(home),'--claude-env','HOME='+str(home)]
  with (out/(label+'.log')).open('w') as log:
   p=subprocess.Popen(cmd,env=env,stdout=log,stderr=log);s=None
   try:
    for _ in range(200):
     if p.poll() is not None:raise ValueError('installed runtime exited: '+label)
     s=socket.socket(socket.AF_UNIX);s.settimeout(10)
     try:s.connect(str(data/'crewd.sock'));break
     except OSError:s.close();s=None;time.sleep(.05)
    if s is None:raise ValueError('installed runtime readiness timeout')
    lease_held(p.pid)
    f=s.makefile('rb');seq=0
    def call(method,params):
     nonlocal seq
     seq+=1;s.sendall((json.dumps({'id':seq,'method':method,'params':params})+'\n').encode())
     while True:
      line=f.readline()
      if not line:raise ValueError('installed runtime result unexpectedly unknown')
      v=json.loads(line)
      if v.get('id')==seq:
       if 'error' in v:raise ValueError('installed RPC failed: '+method+': '+json.dumps(v['error']))
       return v['result']
    try:yield call
    finally:f.close();s.close();s=None
   finally:
    if s:s.close()
    if p.poll() is None:p.send_signal(signal.SIGTERM)
    try:p.wait(timeout=20)
    except subprocess.TimeoutExpired:p.kill();p.wait();raise ValueError('installed shutdown exceeded deadline')
    if p.returncode!=0:raise ValueError('installed shutdown failed')
 # Controlled related-process metadata negatives are distinct from actual
 # managed daemon acceptance below; no forced real PID reuse is claimed.
 spec=importlib.util.spec_from_file_location('installer_control',installer);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
 control=out/'source-controls';control.mkdir(mode=0o700);fakeprefix=control/'prefix';fakeprefix.mkdir(mode=0o700)
 (fakeprefix/'releases').mkdir(mode=0o700);(fakeprefix/'releases'/'retained').mkdir(mode=0o700)
 proc=control/'proc';proc.mkdir(mode=0o700)
 process=proc/'123';process.mkdir(mode=0o700)
 def meta(uid,exe=None,birth_value='100'):
  for file in process.iterdir():file.unlink()
  (process/'status').write_text('Uid: '+str(uid)+' '+str(uid)+' '+str(uid)+' '+str(uid)+'\n')
  fields=['S']+['0']*40;fields[19]=birth_value
  (process/'stat').write_text('123 (synthetic) '+' '.join(fields))
  (process/'cmdline').write_bytes((str(fakeprefix/'releases/retained/bin/crewd')+'\0').encode())
  if exe is not None:(process/'exe').symlink_to(exe)
 meta(os.geteuid()+1);module.inactive(fakeprefix,proc) # unrelated UID is not host veto
 meta(os.geteuid())
 try:module.inactive(fakeprefix,proc);raise AssertionError('related unreadable identity accepted')
 except ValueError:pass
 meta(os.geteuid(),str(fakeprefix/'releases/retained/bin/crewd'))
 try:module.inactive(fakeprefix,proc);raise AssertionError('related live binary accepted')
 except ValueError:pass
 meta(os.geteuid(),'/usr/bin/unrelated');module.inactive(fakeprefix,proc)
 # Mutate the related birth between the first exe lookup and recheck.
 meta(os.geteuid(),str(fakeprefix/'releases/retained/bin/crewd'))
 original_readlink=module.os.readlink
 def changing(path,*args,**kwargs):
  value=original_readlink(path,*args,**kwargs)
  if pathlib.Path(path)==process/'exe':
   fields=['S']+['0']*40;fields[19]='101';(process/'stat').write_text('123 (synthetic) '+' '.join(fields))
  return value
 module.os.readlink=changing
 try:
  try:module.inactive(fakeprefix,proc);raise AssertionError('related changed birth accepted')
  except ValueError as error:assert 'unknown' in str(error)
 finally:module.os.readlink=original_readlink
 broad=control/'broad';broad.mkdir(mode=0o755)
 try:module.private_directory(broad);raise AssertionError('broad prefix accepted')
 except ValueError:pass
 alias=control/'alias';alias.symlink_to(fakeprefix)
 try:module.private_directory(alias);raise AssertionError('aliased prefix accepted')
 except ValueError:pass
 (control/'result.json').write_text(json.dumps({'controlledMetadata':True,'unrelatedUIDAllowed':True,'relatedUnreadableRefused':True,'relatedLiveRefused':True,'birthChangedRefused':True,'wideModeRefused':True,'symlinkRefused':True,'actualPIDReuseTested':False},indent=2)+'\n')
 first=str(uuid.uuid4());made=install('install11','install',first,None,package=old);active=made['currentActivation']
 request=str(uuid.uuid4())
 with serve('installed11') as call:
  remote=call('remote.status',{});assert not remote['listening'] and not remote['configuration']['enabled']
  crew=call('crew.create',{'title':'Installer lifecycle synthetic work','workingDirectory':str(work)})['id']
  params={'crewId':crew,'requestId':request,'text':'pre12 exact requirement'};requirement=call('todo.create',params)['requirementId']
  install('running-runtime-refusal','install',str(uuid.uuid4()),active,package=new,want=False)
  assert status(first)['currentActivation']==active
 wrapper_survival()
 assert dbversion()==11
 cli('backup11','backup');backup11=max((data/'backups').glob('crewd-manual-*.db'),key=lambda p:p.stat().st_mtime_ns);assert dbversion(backup11)==11
 install('cas-refusal','install',str(uuid.uuid4()),'activations/'+str(uuid.uuid4()),package=new,want=False)
 # A real obstacle after durable preparation prevents pointer replacement.
 prepared=str(uuid.uuid4());(prefix/('.current-'+prepared)).mkdir()
 install('prepared-switch-failure','install',prepared,active,package=new,want=False)
 assert status(prepared)['outcome']=='prepared-unknown' and status(first)['currentActivation']==active
 # The outer caller deliberately drops the success receipt; only status resolves it.
 upgrade=str(uuid.uuid4());receipt=install('dropped-upgrade-receipt','install',upgrade,active,package=new);del receipt
 observed=status(upgrade);assert observed['outcome']=='committed-current';upactive=observed['currentActivation']
 before=len(list((prefix/'activations').iterdir()));install('same-request-readback','install',upgrade,active,package=new);assert len(list((prefix/'activations').iterdir()))==before
 install('request-payload-conflict','install',upgrade,active,package=old,want=False);assert status(upgrade)['currentActivation']==upactive
 postrequest=str(uuid.uuid4());postparams={'crewId':crew,'requestId':postrequest,'text':'post12 preserved requirement'}
 with serve('installed-new-upgrade') as call:
  assert call('todo.create',params)['requirementId']==requirement
  post=call('todo.create',postparams)['requirementId']
  remote=call('remote.status',{});assert not remote['listening'] and not remote['configuration']['enabled']
 assert dbversion()==args.new_schema,'formal Core migrations required; never set PRAGMA or count two same-schema packages as upgrade'
 cli('backup-new','backup');backup_new=max((data/'backups').glob('crewd-manual-*.db'),key=lambda p:p.stat().st_mtime_ns);assert dbversion(backup_new)==args.new_schema
 # Concurrent writers receive the same activation CAS. Exactly one may commit;
 # the other must refuse without changing DB bytes or overwriting the winner.
 concurrent_before={p.name:sha(p) for p in data.glob('crewd.db*') if p.is_file()}
 children=[]
 for index in range(2):
  rid=str(uuid.uuid4());cmd=[sys.executable,str(installer),'install','--prefix',str(prefix),'--request-id',rid,'--expected-current',upactive,'--package',str(new)]
  children.append((rid,cmd,subprocess.Popen(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)))
 outcomes=[]
 for index,(rid,cmd,child) in enumerate(children):
  stdout,stderr=child.communicate(timeout=30);(out/('concurrent-'+str(index)+'.log')).write_text(stdout+stderr)
  outcomes.append((rid,child.returncode));records.append({'phase':'concurrent-writer-'+str(index),'command':cmd,'exit':child.returncode})
 assert sum(code==0 for _,code in outcomes)==1,'exclusive writer/CAS must admit exactly one winner'
 winner=next(rid for rid,code in outcomes if code==0);upactive=status(winner)['currentActivation']
 assert status(winner)['outcome']=='committed-current'
 assert concurrent_before=={p.name:sha(p) for p in data.glob('crewd.db*') if p.is_file()}
 beforedb={p.name:sha(p) for p in data.glob('crewd.db*') if p.is_file()}
 rollback=str(uuid.uuid4());rolled=install('binary-pointer-rollback','rollback',rollback,upactive,source=oldmeta['sourceRef']);assert rolled['outcome']=='committed-current'
 assert beforedb=={p.name:sha(p) for p in data.glob('crewd.db*') if p.is_file()},'installer may never modify databases'
 before_refusal=refusal_snapshot()
 (out/'old-refusal-before.json').write_text(json.dumps(before_refusal,sort_keys=True,indent=2)+'\n')
 refused=cli('old11-refuses-new','serve','--codex','/usr/bin/false','--claude','/usr/bin/false',want=False)
 after_refusal=refusal_snapshot()
 (out/'old-refusal-after.json').write_text(json.dumps(after_refusal,sort_keys=True,indent=2)+'\n')
 assert '数据库版本 '+str(args.new_schema)+' 比本程序支持的 11 新，拒绝打开' in (refused.stdout+refused.stderr).decode(),'old binary must give the actual future-schema refusal'
 assert before_refusal['logical']==after_refusal['logical'],'old binary changed domain/schema/integrity despite refusal'
 assert before_refusal['physical']==after_refusal['physical'],'old binary changed physical DB files after closed-observer baseline; retain precise phase evidence'
 # The currently activated old11 binary must not restore a newer DB. Activate
 # the authoritative new binary first, then use its explicit normal restore CLI.
 restore_authority=str(uuid.uuid4());new_again=install('reactivate-new-for-formal-restore','install',restore_authority,rolled['currentActivation'],package=new)
 cli('formal-new-restores11','restore','--from',str(backup11));assert dbversion()==11
 assert any(dbversion(p)==args.new_schema for p in data.glob('crewd.db.replaced-*') if p.is_file() and not p.name.endswith(('-wal','-shm')))
 old_again=install('reactivate-old-after-formal-restore','rollback',str(uuid.uuid4()),new_again['currentActivation'],source=oldmeta['sourceRef'])
 with serve('recovered11') as call:assert call('todo.create',params)['requirementId']==requirement
 forward=str(uuid.uuid4());current=install('reactivate-new','install',forward,old_again['currentActivation'],package=new)
 cli('formal-new-restores-new-backup','restore','--from',str(backup_new))
 with serve('recovered-new') as call:
  assert call('todo.create',params)['requirementId']==requirement and call('todo.create',postparams)['requirementId']==post
 assert dbversion()==args.new_schema
 with contextlib.closing(sqlite3.connect((data/'crewd.db').as_uri()+'?mode=ro',uri=True)) as c:assert c.execute('PRAGMA integrity_check').fetchone()==('ok',)
 (out/'result.json').write_text(json.dumps({'passed':True,'oldSourceRef':oldmeta['sourceRef'],'newSourceRef':newmeta['sourceRef'],'oldBinarySHA256':oldmeta['binarySHA256'],'newBinarySHA256':newmeta['binarySHA256'],'schemas':[11,args.new_schema,11,args.new_schema],'explicitIsolatedPrefix':str(prefix),'runtimeSwitchRefused':True,'preparedFailureUnknownRetained':True,'droppedReceiptReadback':True,'requestPayloadConflictRefused':True,'rollbackNeverTouchedDB':True,'oldBinaryRefusedNewSchema':True,'ordinaryUID':os.geteuid(),'managedLeaseAcrossExec':True,'wrapperDeathChildLease':True,'concurrentWriterSingleCASWinner':True,'formalCoreRestoreOnly':True,'requestIdentitiesPreserved':[requirement,post],'phaseRecords':records},indent=2)+'\n')
 print('PASS ordinary-user explicit Linux install/upgrade/failure/unknown/rollback/authoritative Core restore/restart; no system service or model')
try:main()
except (ValueError,OSError,KeyError,AssertionError,subprocess.SubprocessError) as e:raise SystemExit('FAIL retained installer lifecycle evidence: '+str(e))
