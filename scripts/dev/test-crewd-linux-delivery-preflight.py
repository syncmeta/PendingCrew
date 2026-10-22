#!/usr/bin/env python3
"""Complete non-compiling delivery entry/diagnostic controls in private fixtures."""
import argparse,hashlib,importlib.util,json,os,pathlib,shutil,subprocess,sys,tarfile

def managed_recovery_preflight(package,out,repo):
 """Exact packaged Linux proof/header gate only; no SQLite or daemon execution."""
 import types
 subprocess.run([sys.executable,'-B',str(repo/'scripts/dev/check-crewd-linux-candidate.py'),str(package)],check=True)
 spec=importlib.util.spec_from_file_location('managed',repo/'scripts/dev/crewd_managed_launch.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
 m.sys=types.SimpleNamespace(platform='linux') # Configuration query only, never Linux execution.
 w=json.loads((package/'normal-build-witness.json').read_text())
 assert w['sourceRef']=='5821310a7a9e99daba1a784b8f7b4af0c2af5e45' and w['rustInputDigest']=='6d16f90874edd4a577f27748ef9647148591fe72380412badadd6cf964e7e708'
 assert m.supported_schema(package)==15
 assert m.certified_core14_recovery(package,15), 'exact R2 managed recovery source is not admitted'
 data=out/'private-data';data.mkdir(mode=0o700);db=data/'crewd.db';wal=data/'crewd.db-wal'
 header=bytearray(4096);header[:16]=b'SQLite format 3\0';header[16:18]=(4096).to_bytes(2,'big');header[18:20]=bytes([2,2]);header[60:64]=(15).to_bytes(4,'big');header[68:72]=(1129469764).to_bytes(4,'big')
 db.write_bytes(header);wal.write_bytes(b'synthetic nonempty pending WAL for launcher gate only')
 for f in [db,wal]:f.chmod(0o600)
 def physical():return {f.name:hashlib.sha256(f.read_bytes()).hexdigest() for f in data.iterdir()}
 before=physical();m.readonly_header(data,15,pending_wal_recovery=True);assert physical()==before
 def refused(label,fn):
  try:fn()
  except (ValueError,OSError,KeyError):pass
  else:raise AssertionError('managed recovery negative admitted: '+label)
  assert physical()==before;return label
 checks=[refused('pending-offline-reader',lambda:m.readonly_header(data,15))]
 copy=out/'package-copy';shutil.copytree(package,copy);wp=copy/'normal-build-witness.json';cp=copy/'candidate.json';original_w=wp.read_bytes();original_c=cp.read_bytes()
 def witness_case(label,change):
  value=json.loads(original_w);change(value);wp.write_text(json.dumps(value));candidate=json.loads(original_c);candidate['witnessSHA256']=hashlib.sha256(wp.read_bytes()).hexdigest();cp.write_text(json.dumps(candidate))
  try:
   def attempt():
    recover=m.certified_core14_recovery(copy,15);m.readonly_header(data,15,pending_wal_recovery=recover)
   return refused(label,attempt)
  finally:wp.write_bytes(original_w);cp.write_bytes(original_c)
 for label,change in [('unreviewed-digest',lambda v:v.update(rustInputDigest='f'*64)),('missing-fresh-proof',lambda v:v.update(freshWorkspaceArtifacts=False)),('wrong-normal-profile',lambda v:v['buildConfiguration'].update(profile='debug')),('missing-source-row',lambda v:v['files'].pop()),('changed-graph-hash',lambda v:v.update(productionGraphSHA256='0'*64))]:checks.append(witness_case(label,change))
 binary=copy/'bin/crewd';saved=binary.read_bytes();binary.write_bytes(saved+b'changed')
 try:checks.append(refused('changed-ELF',lambda:m.certified_core14_recovery(copy,15)))
 finally:binary.write_bytes(saved)
 future=bytearray(db.read_bytes());future[60:64]=(16).to_bytes(4,'big');db.write_bytes(future);before=physical();checks.append(refused('future-format',lambda:m.readonly_header(data,15,pending_wal_recovery=True)))
 assert m.CORE15_RECOVERY_INPUTS['673cd2bff312b747ca71bff29aa0d3abe2ba04872ed68108f8966827f4f99d50']==443
 result={'passed':True,'sourceRef':w['sourceRef'],'binarySHA256':w['binarySHA256'],'rustInputDigest':w['rustInputDigest'],'moduleSHA256':hashlib.sha256((repo/'scripts/dev/crewd_managed_launch.py').read_bytes()).hexdigest(),'negativeControls':checks,'exactPackageAndAllProofVerified':True,'staticHeaderDelegationOnly':True,'configurationQueryControlledToLinux':True,'daemonOrSQLiteExecuted':False,'LinuxRuntimeClaim':False,'noRustCompilation':True}
 (out/'result.json').write_text(json.dumps(result,indent=2)+'\n');print('PASS exact R2 managed recovery source/proof/header gates; no daemon or SQLite execution')

def main():
 a=argparse.ArgumentParser();a.add_argument('package');a.add_argument('output');a.add_argument('--managed-recovery-only',action='store_true');args=a.parse_args();package=pathlib.Path(args.package).resolve();out=pathlib.Path(args.output).resolve();repo=pathlib.Path(__file__).resolve().parents[2]
 if out.exists():raise ValueError('fresh complete preflight evidence required')
 out.mkdir(mode=0o700)
 if args.managed_recovery_only:managed_recovery_preflight(package,out,repo);return
 env=dict(os.environ);env.update(PYTHONDONTWRITEBYTECODE='1',GIT_CONFIG_GLOBAL='/dev/null',GIT_CONFIG_NOSYSTEM='1');records=[]
 def run(label,argv,success,contains=None):
  result=subprocess.run(argv,env=env,capture_output=True,text=True,timeout=30);(out/(label+'.log')).write_text(result.stdout+result.stderr)
  if (result.returncode==0)!=success or contains and contains not in result.stdout+result.stderr:raise ValueError('actual preflight mismatch: '+label)
  records.append({'label':label,'command':argv,'exit':result.returncode});return result
 # Execute actual parser/help/diagnostic entrances, not merely compile() syntax.
 for name in ['install-crewd-linux-candidate.py','prepare-crewd-linux-install-baseline.py','build-crewd-linux-candidate.py','test-crewd-linux-reader-clean.py','smoke-crewd-linux-install-upgrade.py']:
  run(name+'-help',[sys.executable,str(repo/'scripts/dev'/name),'--help'],True)
 run('reader-cache-and-dirty-contract',[sys.executable,str(repo/'scripts/dev/test-crewd-linux-reader-clean.py'),str(package),str(out/'reader-controls')],True)
 # Invoke the production member validator with adversarial structure; hash-bound
 # real baseline extraction is separately checked before this workflow step.
 spec=importlib.util.spec_from_file_location('actual_baseline',repo/'scripts/dev/prepare-crewd-linux-install-baseline.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
 good=tarfile.TarInfo('crewd-linux-x86_64/bin/crewd');good.mode=0o755;module.validate_members([good]);negative=[]
 for label,name,kind,mode in [('absolute','/outside',tarfile.REGTYPE,0o644),('parent','crewd-linux-x86_64/../outside',tarfile.REGTYPE,0o644),('foreign','other/file',tarfile.REGTYPE,0o644),('symlink','crewd-linux-x86_64/link',tarfile.SYMTYPE,0o644),('hardlink','crewd-linux-x86_64/link',tarfile.LNKTYPE,0o644),('setuid','crewd-linux-x86_64/file',tarfile.REGTYPE,0o4755)]:
  m=tarfile.TarInfo(name);m.type=kind;m.mode=mode
  try:module.validate_members([m]);raise AssertionError('unsafe member accepted: '+label)
  except ValueError:negative.append(label)
 try:module.validate_members([good,good]);raise AssertionError('duplicate accepted')
 except ValueError:negative.append('duplicate')
 bad_archive=out/'bad-archive';bad_archive.mkdir(mode=0o700);(bad_archive/'crewd-linux-x86_64-7b1849d9.tar.gz').write_bytes(b'not the original archive')
 target=out/'must-not-extract';run('canonical-hash-refusal',[sys.executable,str(repo/'scripts/dev/prepare-crewd-linux-install-baseline.py'),str(bad_archive),str(target)],False,'original tar digest mismatch');assert not target.exists()
 installer=repo/'scripts/dev/install-crewd-linux-candidate.py';prefix=out/'private-prefix';prefix.mkdir(mode=0o700)
 run('status-absent-readonly',[sys.executable,str(installer),'status','--prefix',str(prefix)],True,'absent')
 assert not list(prefix.iterdir())
 run('root-prefix-refusal',[sys.executable,str(installer),'status','--prefix','/'],False,'owner-private prefix')
 wide=out/'wide';wide.mkdir(mode=0o755);run('wide-prefix-refusal',[sys.executable,str(installer),'status','--prefix',str(wide)],False,'0700')
 alias=out/'alias';alias.symlink_to(prefix);run('alias-prefix-refusal',[sys.executable,str(installer),'status','--prefix',str(alias)],False,'canonical')
 (prefix/'current').write_text('uncontrolled');run('uncontrolled-pointer-refusal',[sys.executable,str(installer),'status','--prefix',str(prefix)],False,'controlled activation');(prefix/'current').unlink()
 run('invalid-action-parser',[sys.executable,str(installer),'not-an-action','--prefix',str(prefix)],False,'invalid choice')
 # Supported-platform builder branch tests stop before recorder/Cargo and have
 # no Rust sources. Platform query control on Mac is explicitly not Linux build.
 fixture=out/'builder-entry';files=fixture/'scripts/dev';files.mkdir(parents=True);shutil.copy2(repo/'scripts/dev/build-crewd-linux-candidate.py',files/'build-crewd-linux-candidate.py')
 subprocess.run(['git','init','-q','--template=',str(fixture)],env=env,check=True);subprocess.run(['git','-C',str(fixture),'add','.'],env=env,check=True);subprocess.run(['git','-C',str(fixture),'-c','user.name=PendingCrew Fixture','-c','user.email=fixture@pendingcrew.invalid','-c','commit.gpgsign=false','-c','core.hooksPath=/dev/null','commit','-q','-m','test: complete entry preflight'],env=env,check=True)
 shim=['-c','import platform,runpy,sys;platform.system=lambda:"Linux";platform.machine=lambda:"x86_64";sys.argv=sys.argv[1:];runpy.run_path(sys.argv[0],run_name="__main__")'] if sys.platform!='linux' else []
 builder=files/'build-crewd-linux-candidate.py';existing=out/'existing';existing.mkdir(mode=0o700)
 run('existing-output-refusal',[sys.executable,*shim,str(builder),str(existing)],False,'fresh output required')
 (fixture/'untracked-input').write_text('synthetic dirt');forbidden=out/'forbidden-build';run('untracked-input-refusal',[sys.executable,*shim,str(builder),str(forbidden)],False,'clean committed input required');assert not forbidden.exists()
 unsupported=['-c','import platform,runpy,sys;platform.system=lambda:"Unsupported";sys.argv=sys.argv[1:];runpy.run_path(sys.argv[0],run_name="__main__")'];run('unsupported-platform-refusal',[sys.executable,*unsupported,str(builder),str(forbidden)],False,'actual Linux x86_64');assert not forbidden.exists()
 input_files=[{'path':'scripts/dev/'+name,'sha256':hashlib.sha256((repo/'scripts/dev'/name).read_bytes()).hexdigest()} for name in ['test-crewd-linux-delivery-preflight.py','test-crewd-linux-reader-clean.py','build-crewd-linux-candidate.py','check-crewd-linux-candidate.py','prepare-crewd-linux-install-baseline.py','install-crewd-linux-candidate.py','smoke-crewd-linux-install-upgrade.py','write-codex-first-provenance.py']]
 (out/'result.json').write_text(json.dumps({'passed':True,'actualInputFiles':input_files,'owningHEAD':subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip(),'selection':'actual-working-file-bytes','platform':sys.platform,'noRustCompilation':True,'noDaemonInstallOrModel':True,'controlledPlatformQueriesAreNotLinuxBuild':True,'safeTarNegatives':negative,'entryControls':records},indent=2)+'\n');print('PASS complete non-compiling Linux delivery entry/cache/dirty/safe-extract/diagnostic controls; actual Linux lifecycle remains separate')
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,AssertionError,subprocess.SubprocessError) as e:raise SystemExit('FAIL retained complete delivery preflight: '+str(e))
