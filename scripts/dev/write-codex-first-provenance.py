#!/usr/bin/python3
"""Write pairing evidence for the immutable candidate builder; no app is executed."""
import hashlib,json,os,pathlib,re,shlex,shutil,subprocess,sys

def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def save(p,v):p.write_text(json.dumps(v,ensure_ascii=False,sort_keys=True,indent=2)+'\n')
def digest(rows):return hashlib.sha256(''.join(r['path']+'\t'+r['sha256']+'\n' for r in sorted(rows,key=lambda r:r['path'])).encode()).hexdigest()
def base(s):
 if not isinstance(s,str) or not s or pathlib.PurePosixPath(s).name != s or s in ['.','..'] or any(c in s for c in '\\\n\r\t'):raise ValueError('Evidence names must be relative basenames')
 return s
def inventory(root):
 rows=[]
 for p in sorted(root.rglob('*')):
  if p.is_symlink():raise ValueError('Source snapshots cannot contain symlinks')
  if p.is_file():
   path=p.relative_to(root).as_posix()
   if any(c in path for c in '\n\r\t'):raise ValueError('Control characters in source paths')
   rows.append({'path':path,'sha256':sha(p)})
 return sorted(rows,key=lambda row:row['path'])
def subset(rows,kind):
 return [r for r in rows if r['path'].startswith('crewd/')] if kind=='rust' else [r for r in rows if (r['path'].startswith('Sources/Mac/Crewd/') and r['path'].endswith('.swift')) or r['path'] in ['Sources/Mac/Design/CrewColorHash.swift','scripts/dev/build-codex-first-macos.sh','CandidateMain.swift']]
def logcheck(root,value):
 p=root/base(value['buildLog'])
 if p.is_symlink() or not p.is_file() or sha(p)!=value['buildLogSHA256']:raise ValueError('Build log missing or changed')
 lines=p.read_text().splitlines()
 for field,want in [('Snapshot',value['snapshotDirectory']),('InputDigest',value['rustInputDigest']),('Exit','0')]:
  if [line[len(field)+2:] for line in lines if line.startswith(field+': ')] != [want]:raise ValueError('Actual build log '+field+' differs')
 commands=[line[9:] for line in lines if line.startswith('Command: ')]
 if len(commands)!=1 or 'cargo' not in commands[0] or 'crewd' not in commands[0]:raise ValueError('Actual Rust build command missing')
 configs=[json.loads(line[len('BuildConfiguration: '):]) for line in lines if line.startswith('BuildConfiguration: ')]
 if configs != [value.get('buildConfiguration')]:raise ValueError('Actual logged Rust configuration differs')
def requested_configuration(cwd):
 rust=subprocess.check_output([os.environ.get('RUSTC','rustc'),'-Vv'],text=True,cwd=cwd).strip()
 host=next(line[6:] for line in rust.splitlines() if line.startswith('host: '))
 profile=os.environ.get('PENDINGCREW_BUILD_BACKEND_PROFILE','release')
 if profile not in ['debug','release']:raise ValueError('Choose an explicit debug or release backend profile')
 def flag(name,default):
  value=os.environ.get(name,default)
  if value not in ['0','1']:raise ValueError(name+' must be 0 or 1')
  return value=='1'
 features=sorted(set(filter(None,re.split(r'[,\s]+',os.environ.get('PENDINGCREW_BUILD_BACKEND_FEATURES','')))))
 return {'toolchain':rust.splitlines()[0],'cargoVersion':subprocess.check_output(['cargo','--version'],text=True,cwd=cwd).strip(),'target':os.environ.get('PENDINGCREW_BUILD_BACKEND_TARGET') or os.environ.get('CARGO_BUILD_TARGET') or host,'profile':profile,'defaultFeatures':flag('PENDINGCREW_BUILD_BACKEND_DEFAULT_FEATURES','1'),'features':features,'allFeatures':flag('PENDINGCREW_BUILD_BACKEND_ALL_FEATURES','0')}
def complete_configuration(value):
 if not isinstance(value,dict) or not isinstance(value.get('artifactFeatures'),list) or not isinstance(value.get('artifactProfile'),dict) or not value['artifactProfile'] or not isinstance(value.get('workspaceFeatures'),dict) or not value['workspaceFeatures']:raise ValueError('Actual compiler-artifact effective configuration is missing')
 return value

def verify_fresh_source(evidence,value,required=False):
 if value.get('freshWorkspaceArtifacts') is not True:
  if required:raise ValueError('Fresh owning-workspace compilation proof required for a new candidate')
  return 'legacy-byte-inventory-only'
 sides={}
 for field in ['sourcePreflight','productionGraph','workspaceArtifacts']:
  file=evidence/base(value[field+'File'])
  if file.is_symlink() or not file.is_file() or sha(file)!=value[field+'SHA256']:raise ValueError('Fresh compilation sidecar missing/changed: '+field)
  sides[field]=json.loads(file.read_text())
 pre=sides['sourcePreflight'];owner=pathlib.Path(pre['ownerWorkspace']);repo=owner.parent
 if not owner.is_absolute() or owner.name!='crewd' or pre['sourceRef']!=value['sourceRef']:raise ValueError('Owning workspace identity mismatch')
 if [{'path':r['path'],'sha256':r['sha256']} for r in pre['files']]!=value['files']:raise ValueError('Fresh preflight differs from complete frozen Rust input')
 for row in pre['files']:
  if row['absolutePath']!=str(repo/row['path']) or row['buildMtimeNS']<=row['priorMtimeNS']:raise ValueError('Fresh source preflight did not invalidate every owning input')
 graph=sides['productionGraph'];packages=graph['requiredWorkspacePackages'];required_targets=set();seen=set();actual=[]
 if not packages or graph['rootPackageID'] not in {p['packageID'] for p in packages}:raise ValueError('Required production graph/root missing')
 source_paths={str(repo/r['path']) for r in value['files']}
 for package in packages:
  if package['manifestPath'] not in source_paths or not package['targets']:raise ValueError('Production package outside frozen source')
  for target in package['targets']:
   if target['sourcePath'] not in source_paths:raise ValueError('Production target outside frozen source')
   key=(package['packageID'],package['manifestPath'],target['sourcePath'],target['name'],tuple(target['kind']))
   if key in required_targets:raise ValueError('Duplicate required production target')
   required_targets.add(key)
 package_ids={p['packageID'] for p in packages}
 for line in (evidence/base(value['buildLog'])).read_text().splitlines():
  try:message=json.loads(line)
  except json.JSONDecodeError:continue
  if not isinstance(message,dict) or message.get('reason')!='compiler-artifact':continue
  if message.get('package_id') not in package_ids:
   if pathlib.Path(message.get('manifest_path','')).is_relative_to(owner):raise ValueError('Unexpected workspace artifact outside required normal graph')
   continue
  target=message.get('target',{});key=(message['package_id'],message.get('manifest_path'),target.get('src_path'),target.get('name'),tuple(target.get('kind',[])))
  if key not in required_targets or message.get('fresh') is not False or message.get('profile',{}).get('test') is not False:raise ValueError('Actual Cargo workspace artifact lacks correct owning source/fresh production provenance')
  seen.add(key);actual.append({'packageID':key[0],'manifestPath':key[1],'sourcePath':key[2],'target':key[3],'fresh':False})
 if seen!=required_targets or actual!=sides['workspaceArtifacts']:raise ValueError('Actual raw Cargo artifacts differ from complete required graph or summary')
 return 'fresh-owning-workspace-production-graph'
def verify_witness(path,binary,expected,requested,require_fresh=False):
 value=json.loads(path.read_text())
 if value.get('format')!=1 or not re.fullmatch('[0-9a-f]{40}',value.get('sourceRef','')):raise ValueError('Prebuilt witness format/ref missing')
 evidence=path.parent/base(value['evidenceDirectory']);root=evidence/base(value['snapshotDirectory'])
 if evidence.is_symlink() or root.is_symlink() or not root.is_dir():raise ValueError('Prebuilt evidence snapshot missing')
 actual=inventory(root)
 if actual!=sorted(value['files'],key=lambda r:r['path']) or not all(r['path'].startswith('crewd/') for r in actual):raise ValueError('Full Rust witness file inventory/hash differs')
 if digest(actual)!=expected or value['rustInputDigest']!=expected:raise ValueError('Prebuilt Rust inputs differ from frozen source')
 old=evidence/base(value['binaryFile'])
 if old.is_symlink() or not old.is_file() or sha(old)!=value['binarySHA256'] or sha(binary)!=value['binarySHA256']:raise ValueError('Prebuilt binary does not match actual witness')
 logcheck(evidence,value)
 verify_fresh_source(evidence,value,required=require_fresh)
 config=complete_configuration(value.get('buildConfiguration'))
 if any(config.get(key)!=want for key,want in requested.items()):raise ValueError('Prebuilt toolchain/target/profile/features differ from explicit build intent')
 return value,evidence

def main():
 action=sys.argv[1];out=pathlib.Path(sys.argv[2]).resolve()
 if action=='inputs':
  root=pathlib.Path(sys.argv[3]);ref=sys.argv[4];count=int(sys.argv[5]);rows=inventory(root)
  if not re.fullmatch('[0-9a-f]{40}',ref):raise ValueError('Frozen commit missing')
  save(out/'build-inputs.json',{'backendBuildConfiguration':requested_configuration(root/'crewd'),'format':1,'sourceRef':ref,'sourceSelection':'committed-archive','snapshotDirectory':root.name,'candidateVersion':'0.0.'+str(count),'files':rows,'guiInputDigest':digest(subset(rows,'gui')),'rustInputDigest':digest(subset(rows,'rust'))})
 elif action=='preflight-witness':
  manifest=json.loads((out/'build-inputs.json').read_text());verify_witness(pathlib.Path(sys.argv[3]),pathlib.Path(sys.argv[4]),manifest['rustInputDigest'],manifest['backendBuildConfiguration'])
 elif action=='log':
  manifest=json.loads((out/'build-inputs.json').read_text());kind=sys.argv[3];path=out/base(sys.argv[4]);argv=sys.argv[5:]
  path.write_text('Snapshot: '+manifest['snapshotDirectory']+'\nInputDigest: '+manifest[kind+'InputDigest']+'\nCommand: '+shlex.join(argv)+'\n')
 elif action=='final':
  manifest=json.loads((out/'build-inputs.json').read_text());root=out/manifest['snapshotDirectory']
  if inventory(root)!=manifest['files']:raise ValueError('Frozen inputs changed during compilation')
  mode=sys.argv[3];app=pathlib.Path(sys.argv[4]);witness=sys.argv[5] if len(sys.argv)>5 else ''
  requested=manifest['backendBuildConfiguration']
  gui={'inputDigest':manifest['guiInputDigest'],'preSignFile':'gui.pre-sign','preSignSHA256':sha(out/'gui.pre-sign'),'buildLog':'gui-build.log','buildLogSHA256':sha(out/'gui-build.log')}
  backend={'mode':mode,'inputDigest':manifest['rustInputDigest'],'preSignFile':'crewd.pre-sign','preSignSHA256':sha(out/'crewd.pre-sign')}
  if mode=='built-from-snapshot':
   artifacts=[];workspace={}
   for line in (out/'backend-build.log').read_text().splitlines():
    if not line.startswith('{'):continue
    try:item=json.loads(line)
    except json.JSONDecodeError:continue
    if item.get('reason')!='compiler-artifact':continue
    source=item.get('manifest_path','')
    if source.startswith(str(root/'crewd')+'/'):
     content=pathlib.Path(source).read_text()
     section=content.split('[package]',1)[1].split('\n[',1)[0]
     name=re.search(r'^name\s*=\s*[\"\']([^\"\']+)[\"\']',section,re.M)
     if not name:raise ValueError('Local artifact package identity missing')
     package=name.group(1)
     workspace[package]=sorted(set(item.get('features',[])))
    if item.get('target',{}).get('name')=='crewd' and 'bin' in item.get('target',{}).get('kind',[]) and item.get('executable') and not item.get('profile',{}).get('test'):artifacts.append(item)
   if len(artifacts)!=1:raise ValueError('Expected one actual production crewd compiler-artifact')
   artifact=artifacts[0]
   if sha(pathlib.Path(artifact['executable']))!=backend['preSignSHA256']:raise ValueError('Recorded actual Cargo executable differs from packaged input')
   config=dict(requested);config.update(artifactFeatures=sorted(set(artifact.get('features',[]))),artifactProfile=artifact['profile'],workspaceFeatures=workspace)
   complete_configuration(config);manifest['backendBuildConfiguration']=config;backend['buildConfiguration']=config
   with (out/'backend-build.log').open('a') as log:log.write('BuildConfiguration: '+json.dumps(config,sort_keys=True,separators=(',',':'))+'\n')
   backend.update(buildLog='backend-build.log',buildLogSHA256=sha(out/'backend-build.log'))
   evidence=out/'rust-evidence';snap=evidence/root.name;snap.mkdir(parents=True)
   shutil.copytree(root/'crewd',snap/'crewd');shutil.copyfile(out/'crewd.pre-sign',evidence/'crewd.input');shutil.copyfile(out/'backend-build.log',evidence/'backend-build.log')
   value={'format':1,'sourceRef':manifest['sourceRef'],'rustInputDigest':manifest['rustInputDigest'],'evidenceDirectory':'rust-evidence','snapshotDirectory':root.name,'files':subset(manifest['files'],'rust'),'binaryFile':'crewd.input','binarySHA256':backend['preSignSHA256'],'buildLog':'backend-build.log','buildLogSHA256':sha(evidence/'backend-build.log'),'buildConfiguration':config}
   logcheck(evidence,value);save(out/'rust-witness.json',value)
  elif mode=='verified-prebuilt':
   path=pathlib.Path(witness);value,evidence=verify_witness(path,out/'crewd.pre-sign',manifest['rustInputDigest'],requested)
   manifest['backendBuildConfiguration']=value['buildConfiguration'];backend['buildConfiguration']=value['buildConfiguration']
   shutil.copytree(evidence,out/'imported-rust-evidence');value['evidenceDirectory']='imported-rust-evidence';save(out/'witness.json',value)
   backend.update(witnessFile='witness.json',witnessSHA256=sha(out/'witness.json'))
  elif mode!='declared-unverified':raise ValueError('Unsupported source pairing mode')
  if mode=='declared-unverified':backend['buildConfiguration']=requested
  save(out/'build-inputs.json',manifest)
  save(out/'build-provenance.json',{'format':1,'sourceRef':manifest['sourceRef'],'inputManifestSHA256':sha(out/'build-inputs.json'),'gui':gui,'backend':backend,'packaged':{'guiSHA256':sha(app/'Contents/MacOS/PendingCrewCodexFirst'),'helperSHA256':sha(app/'Contents/Helpers/crewd')}})
 else:raise ValueError('Unsupported action')
def cli_main():
 # Imported verification is the same implementation as the CLI. Import must
 # never dispatch sys.argv or weaken fresh owning-source validation.
 try:
  main();return 0
 except (ValueError,KeyError,TypeError,OSError,subprocess.SubprocessError,IndexError) as e:
  print('FAIL: '+str(e),file=sys.stderr);return 1
if __name__ == '__main__':
 raise SystemExit(cli_main())
