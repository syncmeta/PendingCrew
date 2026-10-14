#!/usr/bin/env python3
"""Actual native normal14 managed Store causal controls, not Linux installation."""
import argparse,hashlib,json,os,pathlib,shutil,signal,socket,subprocess,sys,time,uuid
sys.dont_write_bytecode=True

def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def physical(data):
 return {p.name:{'sha256':digest(p),'size':p.stat().st_size,'mode':p.stat().st_mode&0o777} for p in sorted(data.glob('crewd.db*')) if p.is_file()}
def main():
 a=argparse.ArgumentParser();a.add_argument('normal');a.add_argument('witness');a.add_argument('output');args=a.parse_args()
 normal=pathlib.Path(args.normal).resolve();original=pathlib.Path(args.witness).resolve();out=pathlib.Path(args.output)
 if not out.is_absolute() or out.exists():raise ValueError('fresh explicit private output required')
 out.mkdir(mode=0o700);prefix=out/'prefix';prefix.mkdir(mode=0o700)
 for s in ['releases','.data-leases']: (prefix/s).mkdir(mode=0o700)
 (prefix/'.runtime.lock').write_bytes(b'');os.chmod(prefix/'.runtime.lock',0o600)
 package=prefix/'releases/normal14';package.mkdir(mode=0o700);(package/'bin').mkdir(mode=0o700)
 shutil.copy2(normal,package/'bin/crewd')
 witness=json.loads(original.read_text());assert witness['binarySHA256']==digest(normal)
 evidence=original.parent/witness['evidenceDirectory'];shutil.copytree(evidence,package/'normal-build')
 witness['evidenceDirectory']='normal-build';(package/'normal-build-witness.json').write_text(json.dumps(witness,sort_keys=True)+'\n')
 candidate={'sourceRef':witness['sourceRef'],'binarySHA256':digest(normal),'witnessSHA256':digest(package/'normal-build-witness.json'),'sourceProofTier':'fresh-owning-workspace-production-graph','distribution':'native-causal-control-not-Linux-package','platform':sys.platform}
 (package/'candidate.json').write_text(json.dumps(candidate,sort_keys=True)+'\n');(prefix/'current').symlink_to('releases');(prefix/'releases/release').symlink_to('normal14')
 module=pathlib.Path(__file__).with_name('crewd_managed_launch.py').resolve()
 launcher=out/'launcher.py';launcher.write_text('import importlib.util,sys\nsys.dont_write_bytecode=True\ns=importlib.util.spec_from_file_location("managed",sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)\ntry:m.main(__import__("pathlib").Path(sys.argv[2]),sys.argv[3:])\nexcept (ValueError,OSError,KeyError) as e:raise SystemExit("REFUSED managed launch: "+str(e))\n')
 data=out/'data';data.mkdir(mode=0o700);home=out/'home';home.mkdir(mode=0o700);work=out/'work';work.mkdir(mode=0o700)
 env={'PATH':'/usr/bin:/bin','HOME':str(home),'SHELL':'/usr/bin/false','PYTHONDONTWRITEBYTECODE':'1'}
 def argv(target):return [sys.executable,'-B',str(launcher),str(module),str(prefix),'--data-dir',str(target),'serve','--codex','/usr/bin/false','--claude','/usr/bin/false']
 def start(label,target=data):
  log=(out/(label+'.log')).open('wb');p=subprocess.Popen(argv(target),env=env,stdout=log,stderr=log)
  for _ in range(200):
   if p.poll() is not None:raise ValueError('actual managed daemon failed '+label)
   sock=socket.socket(socket.AF_UNIX);sock.settimeout(5)
   try:sock.connect(str(target/'crewd.sock'));break
   except OSError:sock.close();time.sleep(.05)
  else:p.terminate();p.wait();raise ValueError('readiness timeout')
  f=sock.makefile('rb');sequence=0
  def call(method,params):
   nonlocal sequence
   sequence+=1;sock.sendall((json.dumps({'id':sequence,'method':method,'params':params})+'\n').encode())
   while True:
    value=json.loads(f.readline())
    if value.get('id')==sequence:
     if 'error' in value:raise ValueError('actual RPC failure '+json.dumps(value['error']))
     return value['result']
  return p,sock,f,log,call
 def stop(handles,crash=False):
  p,sock,f,log,_=handles;f.close();sock.close()
  if p.poll() is None:p.send_signal(signal.SIGKILL if crash else signal.SIGTERM)
  p.wait(timeout=20);log.close();assert p.returncode==(-signal.SIGKILL if crash else 0)
 request=str(uuid.uuid4());h=start('actual14-initial')
 try:
  crew=h[4]('crew.create',{'title':'managed14 causal control','workingDirectory':str(work)})['id'];params={'crewId':crew,'requestId':request,'text':'actual committed before SIGKILL'};requirement=h[4]('todo.create',params)['requirementId']
 finally:stop(h,crash=True)
 before=physical(data);assert before['crewd.db-wal']['size']>32
 # Keep an exact pre-recovery crash copy for corrupt-WAL/future negatives.
 crash_copy=out/'crash-copy';crash_copy.mkdir(mode=0o700)
 for p in data.glob('crewd.db*'):
  if p.is_file():shutil.copy2(p,crash_copy/p.name)
 def refused(label,target,command=None):
  before=physical(target);cmd=argv(target) if command is None else [sys.executable,'-B',str(launcher),str(module),str(prefix),'--data-dir',str(target),command]
  r=subprocess.run(cmd,env=env,capture_output=True,timeout=20);(out/(label+'.log')).write_bytes(r.stdout+r.stderr)
  assert r.returncode!=0;assert physical(target)==before;return {'label':label,'exit':r.returncode,'allPhysicalFilesAndModesPreserved':True}
 checks=[refused('pending-offline-backup',data,'backup'),refused('pending-offline-check',data,'check')]
 h=start('actual14-managed-WAL-recovery')
 try:assert h[4]('todo.create',params)['requirementId']==requirement
 finally:stop(h)
 backup=subprocess.run([sys.executable,'-B',str(launcher),str(module),str(prefix),'--data-dir',str(data),'backup'],env=env,capture_output=True,timeout=20);assert backup.returncode==0
 (out/'actual14-backup.log').write_bytes(backup.stdout+backup.stderr)
 source=max((data/'backups').glob('crewd-manual-*.db'),key=lambda p:p.stat().st_mtime_ns)
 r=subprocess.run([sys.executable,'-B',str(launcher),str(module),str(prefix),'--data-dir',str(data),'restore','--from',str(source)],env=env,capture_output=True,timeout=20);assert r.returncode==0
 (out/'actual14-restore.log').write_bytes(r.stdout+r.stderr)
 h=start('actual14-restored-reopen')
 try:assert h[4]('todo.create',params)['requirementId']==requirement
 finally:stop(h)
 bad=out/'bad-wal';shutil.copytree(crash_copy,bad);os.chmod(bad,0o700);p=bad/'crewd.db-wal';b=bytearray(p.read_bytes());b[0]^=255;p.write_bytes(b);checks.append(refused('bad-WAL-no-touch',bad))
 future=out/'future-main';shutil.copytree(crash_copy,future);os.chmod(future,0o700);p=future/'crewd.db';b=bytearray(p.read_bytes());b[60:64]=(15).to_bytes(4,'big');p.write_bytes(b);checks.append(refused('future-main-no-touch',future))
 uncertified=out/'uncertified-wal';shutil.copytree(crash_copy,uncertified);os.chmod(uncertified,0o700)
 saved=(package/'normal-build-witness.json').read_bytes();saved_candidate=(package/'candidate.json').read_bytes();w=json.loads(saved);w['freshWorkspaceArtifacts']=False;(package/'normal-build-witness.json').write_text(json.dumps(w));c=json.loads(saved_candidate);c['witnessSHA256']=digest(package/'normal-build-witness.json');(package/'candidate.json').write_text(json.dumps(c))
 try:checks.append(refused('missing-fresh-proof-no-touch',uncertified))
 finally:(package/'normal-build-witness.json').write_bytes(saved);(package/'candidate.json').write_bytes(saved_candidate)
 result={'actualHost':sys.platform,'actualNormalSourceRef':witness['sourceRef'],'normalSHA256':digest(normal),'originalWitnessSHA256':digest(original),'moduleSHA256':digest(module),'sourceControlSHA256':digest(pathlib.Path(__file__)),'actualSIGKILL':True,'nonemptyCrashWAL':before,'sameVersionManagedRecovery':True,'restoredDurableRequestId':request,'requirementId':requirement,'formalBackupRestoreReopen':True,'negativeControls':checks,'LinuxInstallationClaim':False,'normalRecompiled':False,'models':'disabled /usr/bin/false'}
 (out/'result.json').write_text(json.dumps(result,indent=2)+'\n');print('PASS actual native normal14 managed crash recovery/no-touch/backup/restore controls; not Linux installation')
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,KeyError,AssertionError,subprocess.SubprocessError) as e:raise SystemExit('FAIL retained managed14 control: '+str(e))
