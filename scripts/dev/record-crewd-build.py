#!/usr/bin/python3
"""Prepare full committed Rust inputs before a build, then observe its actual process.
Never builds unless the owner explicitly invokes build with an offline locked Cargo argv.
"""
import hashlib,json,os,pathlib,re,shlex,shutil,subprocess,sys,tarfile

def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def digest(rows):return hashlib.sha256(''.join(r['path']+'\t'+r['sha256']+'\n' for r in sorted(rows,key=lambda r:r['path'])).encode()).hexdigest()
def write(p,v):p.write_text(json.dumps(v,indent=2,sort_keys=True)+'\n')
def git(repo,*args):return subprocess.check_output(['git','-C',str(repo),*args])
def inputs(snapshot):
 rows=[]
 for p in sorted(snapshot.rglob('*')):
  if p.is_symlink():raise ValueError('Rust snapshot symlink forbidden')
  if p.is_file():rows.append({'path':p.relative_to(snapshot).as_posix(),'sha256':sha(p)})
 if not rows or not all(r['path'].startswith('crewd/') for r in rows):raise ValueError('snapshot must contain all and only Rust scope')
 return rows
def verify(repo,rows):
 tracked={p.decode() for p in git(repo,'ls-files','-z','--','crewd').split(b'\0') if p}
 if tracked!={r['path'] for r in rows}:raise ValueError('working Rust inventory differs from frozen ref')
 if git(repo,'ls-files','--others','--exclude-standard','-z','--','crewd').strip(b'\0'):raise ValueError('untracked Rust files would change actual build input')
 for r in rows:
  p=repo/r['path']
  if p.is_symlink() or not p.is_file() or sha(p)!=r['sha256']:raise ValueError('working Rust input differs: '+r['path'])
def main():
 mode,repo_arg,out_arg=sys.argv[1:4];repo=pathlib.Path(repo_arg).resolve();out=pathlib.Path(out_arg).resolve()
 if mode=='prepare':
  ref=git(repo,'rev-parse','--verify',sys.argv[4]+'^{commit}').decode().strip()
  if out.exists():raise ValueError('fresh evidence directory required')
  out.mkdir();archive=out/'source.tar';git(repo,'archive','--format=tar','-o',str(archive),ref,'crewd')
  snapshot=out/'rust-source';snapshot.mkdir()
  with tarfile.open(archive) as t:
   for m in t.getmembers():
    path=pathlib.PurePosixPath(m.name)
    if path.is_absolute() or '..' in path.parts or not (m.isdir() or m.isfile()):raise ValueError('unsafe archive member')
   t.extractall(snapshot)
  archive.unlink();rows=inputs(snapshot);verify(repo,rows)
  write(out/'input-witness.json',{'format':1,'sourceRef':ref,'evidenceDirectory':out.name,'snapshotDirectory':snapshot.name,'files':rows,'rustInputDigest':digest(rows)})
  print('PREPARED (no build):',out,'RustInputDigest:',digest(rows))
 elif mode=='build':
  completed=out.parent/(out.name+'-witness.json')
  if completed.exists():raise ValueError('completed witness already exists; do not overwrite evidence')
  binary=pathlib.Path(sys.argv[4]).resolve();argv=sys.argv[5:]
  if argv[:2]!=['cargo','build'] or '--offline' not in argv or '--locked' not in argv:raise ValueError('explicit offline locked Cargo build argv required')
  if '--message-format=json' not in argv:raise ValueError('actual compiler-artifact JSON evidence required')
  profile='release' if '--release' in argv else 'debug';target=pathlib.Path(os.environ.get('CARGO_TARGET_DIR',str(repo/'crewd/target'))).resolve()
  if binary!=target/profile/'crewd':raise ValueError('binary path does not belong to actual Cargo target/profile')
  evidence=json.loads((out/'input-witness.json').read_text());rows=inputs(out/evidence['snapshotDirectory'])
  if rows!=evidence['files'] or digest(rows)!=evidence['rustInputDigest']:raise ValueError('prepared snapshot was changed')
  verify(repo,rows)
  rustc=subprocess.check_output(['rustc','-Vv'],text=True);cargo_version=subprocess.check_output(['cargo','--version'],text=True).strip()
  host=next(x[6:] for x in rustc.splitlines() if x.startswith('host: '))
  triple=host;features=[]
  for i,arg in enumerate(argv):
   if arg=='--target':triple=argv[i+1]
   elif arg.startswith('--target='):triple=arg.split('=',1)[1]
   elif arg in ['--features','-F']:features.extend(argv[i+1].replace(',', ' ').split())
   elif arg.startswith('--features='):features.extend(arg.split('=',1)[1].replace(',', ' ').split())
  configuration={'toolchain':rustc.splitlines()[0],'cargoVersion':cargo_version,'target':triple,'profile':profile,'defaultFeatures':'--no-default-features' not in argv,'features':sorted(set(features)),'allFeatures':'--all-features' in argv}
  (out/'rustc-version.txt').write_text(rustc)
  log=out/'cargo-build.log'
  with log.open('w') as f:
   f.write('Snapshot: '+evidence['snapshotDirectory']+'\nInputDigest: '+evidence['rustInputDigest']+'\nCommand: cd crewd && '+shlex.join(argv)+'\n');f.flush()
   code=subprocess.run(argv,cwd=repo/'crewd',stdout=f,stderr=subprocess.STDOUT).returncode
   f.write('\nExit: '+str(code)+'\n')
  if code:raise ValueError('actual build failed; evidence retained without completed witness')
  verify(repo,rows)
  artifact=None;workspace_features={}
  for line in log.read_text().splitlines():
   try:message=json.loads(line)
   except json.JSONDecodeError:continue
   if not isinstance(message,dict) or message.get('reason')!='compiler-artifact':continue
   manifest=pathlib.Path(message.get('manifest_path','')).resolve()
   if manifest.is_relative_to(repo/'crewd'):
    package_match=re.search(r'(?m)^name\s*=\s*"([^"]+)"',manifest.read_text())
    if not package_match:raise ValueError('workspace package identity missing')
    package=package_match.group(1)
    workspace_features[package]=sorted(message.get('features',[]))
   if message.get('executable') and pathlib.Path(message['executable']).resolve()==binary and message.get('target',{}).get('name')=='crewd':artifact=message
  if artifact is None or artifact.get('profile',{}).get('test') is not False:raise ValueError('actual normal crewd compiler-artifact evidence missing')
  configuration.update(artifactFeatures=sorted(artifact.get('features',[])),artifactProfile=artifact['profile'],workspaceFeatures=workspace_features)
  with log.open('a') as f:f.write('BuildConfiguration: '+json.dumps(configuration,sort_keys=True,separators=(',',':'))+'\n')
  if not binary.is_file() or binary.is_symlink():raise ValueError('expected actual crewd binary missing')
  saved=out/'crewd-presign';shutil.copy2(binary,saved)
  evidence.update(buildConfiguration=configuration,toolchainFile="rustc-version.txt",toolchainSHA256=sha(out/"rustc-version.txt"),binaryFile=saved.name,binarySHA256=sha(saved),buildLog=log.name,buildLogSHA256=sha(log))
  write(completed,evidence)
  print('BUILT with actual command/exit/input/binary evidence:',completed)
 elif mode=='export':
  # Explicit upgrade of the first recorded layout; never a checker/builder fallback.
  completed=out.parent/(out.name+'-witness.json')
  if completed.exists():raise ValueError('completed witness already exists')
  evidence=json.loads((out/'backend-witness.json').read_text())
  rows=inputs(out/evidence['snapshotDirectory'])
  if rows!=evidence['files'] or digest(rows)!=evidence['rustInputDigest']:raise ValueError('recorded source snapshot mismatch')
  if evidence.get('evidenceDirectory')!=out.name:raise ValueError('evidence directory identity mismatch')
  for file_key,hash_key in [('binaryFile','binarySHA256'),('buildLog','buildLogSHA256'),('toolchainFile','toolchainSHA256')]:
   name=evidence[file_key]
   if pathlib.Path(name).name!=name or sha(out/name)!=evidence[hash_key]:raise ValueError('recorded binary/log evidence mismatch')
  log=(out/evidence['buildLog']).read_text()
  cfg=json.dumps(evidence['buildConfiguration'],sort_keys=True,separators=(',',':'))
  for line in ['Snapshot: '+evidence['snapshotDirectory'],'InputDigest: '+evidence['rustInputDigest'],'Exit: 0','BuildConfiguration: '+cfg]:
   if line not in log.splitlines():raise ValueError('recorded successful build/configuration marker missing')
  if not any(line.startswith('Command: ') and 'cargo build' in line for line in log.splitlines()):raise ValueError('recorded actual Cargo command missing')
  write(completed,evidence);print('EXPORTED existing verified evidence without rebuilding:',completed)
 else:raise ValueError('expected prepare, build or explicit export')
try:main()
except (ValueError,OSError,KeyError,IndexError,subprocess.CalledProcessError) as e:
 print('FAIL:',e,file=sys.stderr);sys.exit(1)
