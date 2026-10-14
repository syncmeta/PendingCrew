#!/usr/bin/python3
"""Prepare full committed Rust inputs before a build, then observe its actual process.
Never builds unless the owner explicitly invokes build with an offline locked Cargo argv.
"""
import hashlib,json,os,pathlib,re,shlex,shutil,subprocess,sys,tarfile,time

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
 return sorted(rows,key=lambda row:row['path'])
def verify(repo,rows):
 tracked={p.decode() for p in git(repo,'ls-files','-z','--','crewd').split(b'\0') if p}
 if tracked!={r['path'] for r in rows}:raise ValueError('working Rust inventory differs from frozen ref')
 if git(repo,'ls-files','--others','--exclude-standard','-z','--','crewd').strip(b'\0'):raise ValueError('untracked Rust files would change actual build input')
 for r in rows:
  p=repo/r['path']
  if p.is_symlink() or not p.is_file() or sha(p)!=r['sha256']:raise ValueError('working Rust input differs: '+r['path'])
def source_preflight(repo,rows,target,ref):
 touched=[];now=time.time_ns()
 for row in rows:
  p=repo/row['path'];before=p.stat();os.utime(p,ns=(before.st_atime_ns,max(now,before.st_mtime_ns+1)))
  touched.append(dict(row,absolutePath=str(p),priorMtimeNS=before.st_mtime_ns,buildMtimeNS=p.stat().st_mtime_ns))
 verify(repo,rows)
 return {'format':1,'ownerWorkspace':str(repo/'crewd'),'targetDirectory':str(target),'sourceRef':ref,'files':touched}
def normal_graph(repo,argv,triple):
 flags=[]
 for i,arg in enumerate(argv):
  if arg in ['--features','-F']:flags.extend(['--features',argv[i+1]])
  elif arg.startswith('--features=') or arg in ['--no-default-features','--all-features']:flags.append(arg)
 command=['cargo','metadata','--format-version=1','--locked','--offline','--filter-platform',triple]+flags
 metadata=json.loads(subprocess.check_output(command,cwd=repo/'crewd'))
 packages={p['id']:p for p in metadata['packages']};nodes={n['id']:n for n in metadata['resolve']['nodes']}
 roots=[p['id'] for p in metadata['packages'] if p['id'] in metadata['workspace_members'] and p['name']=='crewd']
 if len(roots)!=1:raise ValueError('exact normal crewd root package missing')
 visited=set();pending=roots[:]
 while pending:
  identity=pending.pop()
  if identity in visited:continue
  visited.add(identity)
  for dep in nodes[identity]['deps']:
   if any(k['kind'] in [None,'normal','build'] for k in dep['dep_kinds']):pending.append(dep['pkg'])
 required=[]
 for identity in sorted(visited & set(metadata['workspace_members'])):
  package=packages[identity];manifest=pathlib.Path(package['manifest_path']).resolve()
  if not manifest.is_relative_to(repo/'crewd'):raise ValueError('resolved workspace dependency belongs to another owner')
  targets=[]
  for t in package['targets']:
   if set(t['kind']) & {'lib','rlib','proc-macro','custom-build'} or (identity==roots[0] and t['name']=='crewd' and 'bin' in t['kind']):
    source=pathlib.Path(t['src_path']).resolve()
    if not source.is_relative_to(repo/'crewd'):raise ValueError('resolved workspace target source belongs to another owner')
    targets.append({'name':t['name'],'kind':t['kind'],'sourcePath':str(source)})
  if not targets:raise ValueError('required normal workspace package has no production target')
  required.append({'packageID':identity,'name':package['name'],'manifestPath':str(manifest),'targets':targets})
 return {'metadataCommand':command,'rootPackageID':roots[0],'requiredWorkspacePackages':required,'snapshotOnlyWorkspacePackages':sorted(set(metadata['workspace_members'])-visited)}
def validate_artifacts(messages,graph,repo):
 seen=set();actual=[];workspace_features={};required={p['packageID']:p for p in graph['requiredWorkspacePackages']}
 for message in messages:
  if message.get('reason')!='compiler-artifact':continue
  identity=message.get('package_id');manifest=pathlib.Path(message.get('manifest_path','')).resolve()
  if identity not in required:
   if manifest.is_relative_to(repo/'crewd'):raise ValueError('unexpected workspace artifact outside frozen normal graph')
   continue
  package=required[identity];target=message.get('target',{});source=str(pathlib.Path(target.get('src_path','')).resolve())
  if str(manifest)!=package['manifestPath'] or not any(t['sourcePath']==source and t['name']==target.get('name') and t['kind']==target.get('kind') for t in package['targets']):raise ValueError('workspace artifact source/package identity differs from frozen owner graph')
  if message.get('fresh') is not False:raise ValueError('workspace artifact reused potentially stale shared cache: '+package['name'])
  if message.get('profile',{}).get('test') is not False:raise ValueError('test artifact cannot prove normal production input')
  seen.add((identity,source,target['name']));actual.append({'packageID':identity,'manifestPath':str(manifest),'sourcePath':source,'target':target['name'],'fresh':False});workspace_features[package['name']]=sorted(message.get('features',[]))
 for package in required.values():
  for target in package['targets']:
   if (package['packageID'],target['sourcePath'],target['name']) not in seen:raise ValueError('required production workspace artifact missing: '+package['name']+'/'+target['name'])
 return actual,workspace_features
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
  configuration={'compilerEnvironment':{name:os.environ.get(name) for name in ['RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CARGO_INCREMENTAL','CC','CFLAGS']},'toolchain':rustc.splitlines()[0],'cargoVersion':cargo_version,'target':triple,'profile':profile,'defaultFeatures':'--no-default-features' not in argv,'features':sorted(set(features)),'allFeatures':'--all-features' in argv}
  graph=normal_graph(repo,argv,triple);write(out/'normal-production-graph.json',graph)
  (out/'rustc-version.txt').write_text(rustc)
  preflight=source_preflight(repo,rows,target,evidence['sourceRef']);write(out/'source-preflight.json',preflight)
  log=out/'cargo-build.log'
  with log.open('w') as f:
   f.write('Snapshot: '+evidence['snapshotDirectory']+'\nInputDigest: '+evidence['rustInputDigest']+'\nCommand: cd crewd && '+shlex.join(argv)+'\n');f.flush()
   code=subprocess.run(argv,cwd=repo/'crewd',stdout=f,stderr=subprocess.STDOUT).returncode
   f.write('\nExit: '+str(code)+'\n')
  if code:raise ValueError('actual build failed; evidence retained without completed witness')
  verify(repo,rows)
  artifact=None;messages=[]
  for line in log.read_text().splitlines():
   try:message=json.loads(line)
   except json.JSONDecodeError:continue
   if not isinstance(message,dict):continue
   messages.append(message)
   if message.get('reason')=='compiler-artifact' and message.get('executable') and pathlib.Path(message['executable']).resolve()==binary and message.get('target',{}).get('name')=='crewd':artifact=message
  actual,workspace_features=validate_artifacts(messages,graph,repo)
  write(out/'workspace-artifacts.json',actual)
  if artifact is None or artifact.get('profile',{}).get('test') is not False:raise ValueError('actual normal crewd compiler-artifact evidence missing')
  configuration.update(artifactFeatures=sorted(artifact.get('features',[])),artifactProfile=artifact['profile'],workspaceFeatures=workspace_features)
  with log.open('a') as f:f.write('BuildConfiguration: '+json.dumps(configuration,sort_keys=True,separators=(',',':'))+'\n')
  if not binary.is_file() or binary.is_symlink():raise ValueError('expected actual crewd binary missing')
  saved=out/'crewd-presign';shutil.copy2(binary,saved)
  evidence.update(sourcePreflightFile='source-preflight.json',sourcePreflightSHA256=sha(out/'source-preflight.json'),productionGraphFile='normal-production-graph.json',productionGraphSHA256=sha(out/'normal-production-graph.json'),workspaceArtifactsFile='workspace-artifacts.json',workspaceArtifactsSHA256=sha(out/'workspace-artifacts.json'),freshWorkspaceArtifacts=True,buildConfiguration=configuration,toolchainFile="rustc-version.txt",toolchainSHA256=sha(out/"rustc-version.txt"),binaryFile=saved.name,binarySHA256=sha(saved),buildLog=log.name,buildLogSHA256=sha(log))
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
if __name__ == '__main__':
 try:main()
 except (ValueError,OSError,KeyError,IndexError,subprocess.CalledProcessError) as e:
  print('FAIL:',e,file=sys.stderr);sys.exit(1)
