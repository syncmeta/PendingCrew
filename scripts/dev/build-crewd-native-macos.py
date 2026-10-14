#!/usr/bin/python3
"""Build an isolated Native candidate from committed sources. Never launch/install.
Requires a recorded normal backend witness and a pre-resolved SPM cache.
"""
import argparse, hashlib, importlib.util, json, os, pathlib, plistlib, shlex, shutil, subprocess, tarfile

def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def save(p,v): p.write_text(json.dumps(v,sort_keys=True,indent=2)+'\n')
def git(root,*args): return subprocess.check_output(['git','-C',str(root),*args])
def run(argv,cwd,log):
 with log.open('a') as f:
  f.write('Command: '+shlex.join([str(v) for v in argv])+'\n');f.flush()
  result=subprocess.run(argv,cwd=cwd,stdout=f,stderr=subprocess.STDOUT)
  f.write('Exit: '+str(result.returncode)+'\n')
 if result.returncode: raise ValueError('Build failed; preserve '+str(log))
def copied_inputs(source,dest):
 # Flatten only in-tree links, including framework version aliases. Detect
 # cycles instead of letting a copied alias escape or recurse forever.
 root=source.resolve()
 def copy(p,target,ancestors):
  resolved=p.resolve()
  if not resolved.is_relative_to(root): raise ValueError('dependency symlink escapes its input tree')
  if p.is_dir():
   if resolved in ancestors: raise ValueError('dependency directory link cycle')
   target.mkdir(parents=True,exist_ok=True)
   for child in p.iterdir():
    if child.name!='.git': copy(child,target/child.name,ancestors|{resolved})
  elif p.is_file():
   target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(p,target)
  else: raise ValueError('dependency input must be a regular file/directory')
 copy(source,dest,set())
def regular_inventory(root):
 rows=[]
 for p in sorted(root.rglob('*')):
  if p.is_symlink(): raise ValueError('evidence snapshot cannot contain symlinks')
  if p.is_file(): rows.append({'path':p.relative_to(root).as_posix(),'sha256':sha(p)})
 return rows

def main():
 a=argparse.ArgumentParser();a.add_argument('output');a.add_argument('--ref',default='HEAD');a.add_argument('--backend',required=True);a.add_argument('--witness',required=True);a.add_argument('--spm',required=True);a.add_argument('--derived-data',required=True);a.add_argument('--execute',action='store_true');args=a.parse_args()
 repo=pathlib.Path(__file__).resolve().parents[2];out=pathlib.Path(args.output).resolve()
 if out.exists(): raise ValueError('fresh output directory required; earlier evidence must be retained')
 ref=git(repo,'rev-parse','--verify',args.ref+'^{commit}').decode().strip()
 out.mkdir();snapshot=out/'source-inputs';snapshot.mkdir();archive=out/'source.tar'
 git(repo,'archive','--format=tar','-o',str(archive),ref)
 with tarfile.open(archive) as t:
  for m in t.getmembers():
   p=pathlib.PurePosixPath(m.name)
   if p.is_absolute() or '..' in p.parts or not (m.isfile() or m.isdir()): raise ValueError('unsafe source archive member')
  t.extractall(snapshot)
 archive.unlink()
 if (repo/'scripts/dev/build-crewd-native-macos.py').read_bytes()!=(snapshot/'scripts/dev/build-crewd-native-macos.py').read_bytes(): raise ValueError('builder must match its selected committed input')
 spec=importlib.util.spec_from_file_location('proof',snapshot/'scripts/dev/write-codex-first-provenance.py');proof=importlib.util.module_from_spec(spec);spec.loader.exec_module(proof)
 source_rows=regular_inventory(snapshot);rust_rows=[r for r in source_rows if r['path'].startswith('crewd/')]
 requested=proof.requested_configuration(snapshot/'crewd')
 backend=pathlib.Path(args.backend).resolve();witness=pathlib.Path(args.witness).resolve()
 value,evidence=proof.verify_witness(witness,backend,proof.digest(rust_rows),requested)
 # Carry the original complete compiler inputs/log/toolchain/binary proof.
 shutil.copytree(evidence,out/'backend-evidence');value['evidenceDirectory']='backend-evidence';save(out/'backend-witness.json',value)
 spm=pathlib.Path(args.spm).resolve();deps=out/'dependency-inputs';deps.mkdir()
 lock=json.loads((snapshot/'PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved').read_text())
 pins=[]
 for pin in lock['pins']:
  name=pin['identity'];checkout=spm/'checkouts'/name
  if not checkout.is_dir(): raise ValueError('resolved package checkout missing: '+name)
  actual=git(checkout,'rev-parse','HEAD').decode().strip()
  if actual!=pin['state']['revision'] or git(checkout,'status','--porcelain').strip(): raise ValueError('package input differs from fixed resolution: '+name)
  copied_inputs(checkout,deps/'checkouts'/name);pins.append({'identity':name,'revision':actual})
 if (spm/'artifacts').exists(): copied_inputs(spm/'artifacts',deps/'artifacts')
 dependency_rows=regular_inventory(deps)
 entry=(snapshot/'Sources/PendingCrewEntry.swift')
 if not entry.exists():
  candidates=list(snapshot.rglob('PendingCrewEntry.swift'))
  if len(candidates)!=1: raise ValueError('unique Native entry source required')
  entry=candidates[0]
 if 'PENDINGCREW_NATIVE_CANDIDATE' not in entry.read_text(): raise ValueError('candidate compile entry is not frozen in selected source')
 manifest={'format':2,'sourceRef':ref,'sourceSelection':'committed-archive','snapshotDirectory':snapshot.name,'files':source_rows,'sourceInputDigest':proof.digest(source_rows),'rustInputDigest':proof.digest(rust_rows),'dependencyDirectory':deps.name,'dependencyFiles':dependency_rows,'dependencyInputDigest':proof.digest(dependency_rows),'packagePins':pins,'backendBuildConfiguration':value['buildConfiguration'],'guiBuildConfiguration':{'buildSystem':'xcodebuild','scheme':'PendingCrew','configuration':'Debug','destination':'platform=macOS,arch=arm64','compileCondition':'PENDINGCREW_NATIVE_CANDIDATE','swiftActiveCompilationConditions':'$(inherited) PENDINGCREW_NATIVE_CANDIDATE','packageResolution':'locked-pre-resolved','xcodeVersion':subprocess.check_output(['xcodebuild','-version'],text=True).strip(),'sdkVersion':subprocess.check_output(['xcrun','--sdk','macosx','--show-sdk-version'],text=True).strip()}}
 save(out/'build-inputs.json',manifest);(out/'source-head.txt').write_text(ref+'\n')
 if not args.execute:
  print('PREPARED only (no compilation or application launch):',out);return
 dd=pathlib.Path(args.derived_data).resolve()
 if dd.is_relative_to(out): raise ValueError('use the existing external DerivedData; no cold build cache in evidence')
 argv=['xcodebuild','-project',str(snapshot/'PendingCrew.xcodeproj'),'-scheme','PendingCrew','-configuration','Debug','-destination','platform=macOS,arch=arm64','-derivedDataPath',str(dd),'-clonedSourcePackagesDirPath',str(spm),'-disableAutomaticPackageResolution','-skipPackageUpdates','-jobs','2','CODE_SIGNING_ALLOWED=NO','SWIFT_ACTIVE_COMPILATION_CONDITIONS=$(inherited) PENDINGCREW_NATIVE_CANDIDATE','build']
 run(argv,snapshot,out/'gui-build.log')
 if regular_inventory(snapshot)!=source_rows: raise ValueError('target build changed its source snapshot')
 # Check every actual external package/artifact byte against the copied proof.
 for r in dependency_rows:
  p=spm/r['path']
  if not p.is_file() or sha(p)!=r['sha256']: raise ValueError('package input changed during build: '+r['path'])
 built=dd/'Build/Products/Debug/PendingCrew.app'
 if not built.is_dir(): raise ValueError('actual Xcode target app missing')
 app=out/'PendingCrew Native Candidate.app';shutil.copytree(built,app,symlinks=True)
 info=app/'Contents/Info.plist';plist=plistlib.loads(info.read_bytes());original=app/'Contents/MacOS'/plist['CFBundleExecutable']
 shutil.copy2(original,out/'gui.pre-sign')
 helper=app/'Contents/Helpers/crewd';helper.parent.mkdir(exist_ok=True);shutil.copy2(backend,helper);shutil.copy2(backend,out/'crewd.pre-sign')
 count=git(repo,'rev-list','--count',ref).decode().strip();version='0.0.'+count
 plist.update(CFBundleIdentifier='com.pendingname.pendingcrew.native-candidate',CFBundleName='PendingCrew Native Candidate',CFBundleDisplayName='PendingCrew · 开发候选 '+ref[:8],CFBundleShortVersionString=version,CFBundleVersion=count,PendingCrewSourceRef=ref,BuildStampCommit=ref,PendingCrewNativeCandidate=True)
 info.write_bytes(plistlib.dumps(plist))
 run(['codesign','--force','--deep','--sign','-',str(app)],snapshot,out/'signing.log')
 run(['codesign','--verify','--deep','--strict',str(app)],snapshot,out/'signature-check.log')
 save(out/'build-provenance.json',{'format':2,'sourceRef':ref,'inputManifestSHA256':sha(out/'build-inputs.json'),'gui':{'preSignFile':'gui.pre-sign','preSignSHA256':sha(out/'gui.pre-sign'),'buildLog':'gui-build.log','buildLogSHA256':sha(out/'gui-build.log')},'backend':{'mode':'verified-prebuilt','witnessFile':'backend-witness.json','witnessSHA256':sha(out/'backend-witness.json'),'preSignFile':'crewd.pre-sign','preSignSHA256':sha(out/'crewd.pre-sign'),'buildConfiguration':value['buildConfiguration']},'packaged':{'nativeSHA256':sha(original),'helperSHA256':sha(helper)},'candidateVersion':version,'appDirectory':app.name})
 print('BUILT isolated candidate; not launched/installed/released:',app)
if __name__=='__main__':
 try: main()
 except (ValueError,OSError,KeyError,subprocess.CalledProcessError) as e: raise SystemExit('FAIL: '+str(e))
