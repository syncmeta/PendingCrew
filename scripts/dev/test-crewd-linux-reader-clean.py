#!/usr/bin/env python3
"""Actual old/new reader controls in private Git fixtures; never compile or install."""
import argparse,hashlib,json,os,pathlib,shutil,subprocess,sys
ORIGINAL='5f3f759568c88b84b5949fe0a109f3f35e44c1db'
def main():
 a=argparse.ArgumentParser();a.add_argument('package');a.add_argument('output');args=a.parse_args()
 package=pathlib.Path(args.package).resolve();out=pathlib.Path(args.output).resolve();repo=pathlib.Path(__file__).resolve().parents[2]
 if out.exists():raise ValueError('fresh reader control evidence required')
 out.mkdir(mode=0o700);env=dict(os.environ);env.pop('PYTHONDONTWRITEBYTECODE',None);env.update(GIT_CONFIG_GLOBAL='/dev/null',GIT_CONFIG_NOSYSTEM='1')
 results=[]
 for label in ['original-red','fixed-green']:
  fixture=out/label;files=fixture/'scripts/dev';files.mkdir(parents=True)
  rows=[]
  for name in ['check-crewd-linux-candidate.py','write-codex-first-provenance.py','build-crewd-linux-candidate.py']:
   source='scripts/dev/'+name
   data=subprocess.check_output(['git','-C',str(repo),'show',ORIGINAL+':'+source]) if label=='original-red' else (repo/source).read_bytes()
   (files/name).write_bytes(data);rows.append({'path':source,'sha256':hashlib.sha256(data).hexdigest(),'inputSource':ORIGINAL if label=='original-red' else 'current-committed-reader'})
  subprocess.run(['git','init','-q','--template=',str(fixture)],env=env,check=True)
  subprocess.run(['git','-C',str(fixture),'add','.'],env=env,check=True)
  subprocess.run(['git','-C',str(fixture),'-c','user.name=PendingCrew Fixture','-c','user.email=fixture@pendingcrew.invalid','-c','commit.gpgsign=false','-c','core.hooksPath=/dev/null','commit','-q','-m','test: readonly reader inputs'],env=env,check=True)
  # Mac's system Python redirects bytecode externally by default. Explicitly
  # choose the Linux-default layout in this private fixture, never the checkout.
  command=[sys.executable,'-c','import sys,runpy;sys.dont_write_bytecode=False;sys.pycache_prefix=None;sys.argv=sys.argv[1:];runpy.run_path(sys.argv[0],run_name="__main__")',str(files/'check-crewd-linux-candidate.py'),str(package)]
  before=subprocess.check_output(['git','-C',str(fixture),'status','--porcelain'],env=env,text=True)
  result=subprocess.run(command,env=env,capture_output=True,text=True,timeout=30)
  after=subprocess.check_output(['git','-C',str(fixture),'status','--porcelain'],env=env,text=True)
  caches=[p.relative_to(fixture).as_posix() for p in files.rglob('*.pyc')]
  (out/(label+'-actual-reader.log')).write_text(result.stdout+result.stderr) # After captured status; evidence separate from assertions.
  if before or result.returncode:raise ValueError('strict original package verification failed in reader control')
  if label=='original-red':
   if after!='?? scripts/dev/__pycache__/\n' or not caches:raise ValueError('original reader cache-write RED not reproduced')
  elif after or caches:raise ValueError('fixed reader changed its own clean input')
  results.append({'label':label,'command':command,'sourceFiles':rows,'beforeStatus':before,'afterStatus':after,'exit':result.returncode,'bytecodeFiles':caches,'strictPackageVerified':True})
 dirty_guard = None
 if sys.platform == 'linux':
  consumer=files/'write-codex-first-provenance.py';consumer.write_bytes(consumer.read_bytes()+b'\n# synthetic tracked-source dirt\n')
  forbidden=out/'forbidden-build'
  command=[sys.executable,str(files/'build-crewd-linux-candidate.py'),str(forbidden)]
  result=subprocess.run(command,env=env,capture_output=True,text=True,timeout=10)
  (out/'strict-dirty-refusal.log').write_text(result.stdout+result.stderr)
  if result.returncode==0 or 'clean committed input required' not in result.stderr or 'Actual owning checkout status' not in result.stderr or 'write-codex-first-provenance.py' not in result.stderr or forbidden.exists():raise ValueError('tracked-source dirty guard lost or build began')
  dirty_guard={'actualLinux':True,'command':command,'exit':result.returncode,'compileStarted':False,'outputCreated':False,'trackedSourceRefused':True}
 (out/'result.json').write_text(json.dumps({'passed':True,'python':sys.version,'platform':sys.platform,'originalReaderSource':ORIGINAL,'actualOriginalFailedRunnerStatusRecovered':False,'linuxDefaultCacheLayout':True,'controls':results,'strictDirtyGuardControl':dirty_guard},indent=2)+'\n')
 print('PASS original cache-write RED and fixed read-only GREEN with unchanged strict package verification; no build/install')
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,subprocess.SubprocessError) as e:raise SystemExit('FAIL retained readonly-reader controls: '+str(e))
