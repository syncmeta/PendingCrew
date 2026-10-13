#!/bin/sh
# Small synthetic validator fixture only. It is not a build/release provenance claim.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-provenance-qa.XXXXXX)
echo "Provenance validator evidence: $fixture"
python3 - "$root" "$fixture" <<'PY'
import copy,hashlib,json,pathlib,plistlib,subprocess,sys
root,base=map(pathlib.Path,sys.argv[1:]);app=base/'Synthetic.app'
(app/'Contents/MacOS').mkdir(parents=True);(app/'Contents/Helpers').mkdir()
source='1'*40
plist={'CFBundleExecutable':'Synthetic','CFBundleIdentifier':'invalid.synthetic.validator-only','CFBundlePackageType':'APPL','CFBundleShortVersionString':'0.0.12','PendingCrewSourceRef':source}
(app/'Contents/Info.plist').write_bytes(plistlib.dumps(plist))
(base/'tiny.c').write_text('int main(void) { return 0; }\n')
subprocess.run(['xcrun','clang',str(base/'tiny.c'),'-o',str(base/'pre-gui')],check=True)
import shutil
shutil.copy2(base/'pre-gui',base/'pre-backend');shutil.copy2(base/'pre-gui',app/'Contents/MacOS/Synthetic');shutil.copy2(base/'pre-backend',app/'Contents/Helpers/crewd')
subprocess.run(['codesign','--force','--sign','-',str(app/'Contents/Helpers/crewd')],check=True)
subprocess.run(['codesign','--force','--sign','-',str(app)],check=True)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def dump(p,v):p.write_text(json.dumps(v,sort_keys=True,indent=2)+'\n')
(base/'source-head.txt').write_text(source+'\n');(base/'source-changes.patch').write_text('')
(base/'artifact-sha256.txt').write_text(sha(app/'Contents/MacOS/Synthetic')+'  '+str(app/'Contents/MacOS/Synthetic')+'\n'+sha(app/'Contents/Helpers/crewd')+'  '+str(app/'Contents/Helpers/crewd')+'\n')
check=[str(root/'scripts/dev/check-codex-first-artifact.sh'),str(app)]
old=subprocess.run(check,capture_output=True,text=True);(base/'old-check.log').write_text(old.stdout+old.stderr)
assert old.returncode!=0,'artifact entry point still accepts a missing-proof package'
snapshot=base/'source-snapshot';snapshot.mkdir()
paths=['Sources/Mac/Crewd/CrewdClient.swift','Sources/Mac/Crewd/CrewdWorkspace.swift','Sources/Mac/Design/CrewColorHash.swift','scripts/dev/build-codex-first-macos.sh','CandidateMain.swift','crewd/Cargo.toml','crewd/Cargo.lock','crewd/src/main.rs']
for path in paths:
 p=snapshot/path;p.parent.mkdir(parents=True,exist_ok=True);p.write_text('synthetic schema-control only: '+path+'\n')
rows=[{'path':p,'sha256':sha(snapshot/p)} for p in paths]
def digest(rows):return hashlib.sha256(''.join(r['path']+'\t'+r['sha256']+'\n' for r in sorted(rows,key=lambda x:x['path'])).encode()).hexdigest()
gui=[r for r in rows if not r['path'].startswith('crewd/')];rust=[r for r in rows if r['path'].startswith('crewd/')];gd,rd=digest(gui),digest(rust)
configuration={'toolchain':'rustc synthetic-validator-only','cargoVersion':'cargo synthetic-validator-only','target':'aarch64-apple-darwin','profile':'debug','defaultFeatures':True,'features':[],'allFeatures':False,'artifactFeatures':[],'artifactProfile':{'opt_level':'0','debuginfo':2,'debug_assertions':True,'overflow_checks':True,'test':False},'workspaceFeatures':{'crewd':[],'crewd-server':[]}}
manifest={'format':1,'sourceRef':source,'sourceSelection':'committed-archive','snapshotDirectory':snapshot.name,'candidateVersion':'0.0.12','backendBuildConfiguration':configuration,'files':rows,'guiInputDigest':gd,'rustInputDigest':rd};dump(base/'build-inputs.json',manifest)
for name,dig,command in [('gui.log',gd,'xcrun swiftc Sources/Mac/Crewd/*.swift CandidateMain.swift'),('backend.log',rd,'cd crewd && cargo build --locked -p crewd')]:
 (base/name).write_text(('BuildConfiguration: '+json.dumps(configuration,sort_keys=True,separators=(',',':'))+'\n' if name=='backend.log' else '')+'Snapshot: source-snapshot\nInputDigest: '+dig+'\nCommand: '+command+'\nExit: 0\nsynthetic validator-only control, not a real build observation\n')
proof={'format':1,'sourceRef':source,'inputManifestSHA256':sha(base/'build-inputs.json'),'gui':{'inputDigest':gd,'preSignFile':'pre-gui','preSignSHA256':sha(base/'pre-gui'),'buildLog':'gui.log','buildLogSHA256':sha(base/'gui.log')},'backend':{'buildConfiguration':configuration,'mode':'built-from-snapshot','inputDigest':rd,'preSignFile':'pre-backend','preSignSHA256':sha(base/'pre-backend'),'buildLog':'backend.log','buildLogSHA256':sha(base/'backend.log')},'packaged':{'guiSHA256':sha(app/'Contents/MacOS/Synthetic'),'helperSHA256':sha(app/'Contents/Helpers/crewd')}};dump(base/'build-provenance.json',proof)
validator=['python3',str(root/'scripts/dev/check-codex-first-provenance.py'),str(app)]
def run(label,want):
 p=subprocess.run(validator,capture_output=True,text=True);(base/(label+'.log')).write_text(p.stdout+p.stderr);assert (p.returncode==0)==want,(label,p.stdout,p.stderr)
run('valid-synthetic-structure',True)
for label,change in [('declared-only',lambda p:p['backend'].update(mode='declared-unverified')),('different-rust-input',lambda p:p['backend'].update(inputDigest='2'*64)),('wrong-input-manifest',lambda p:p.update(inputManifestSHA256='3'*64)),('wrong-gui-source',lambda p:p.update(sourceRef='4'*40)),('wrong-packaged-binary',lambda p:p['packaged'].update(helperSHA256='5'*64)),('wrong-presign-binary',lambda p:p['gui'].update(preSignSHA256='6'*64))]:
 bad=copy.deepcopy(proof);change(bad);dump(base/'build-provenance.json',bad);run(label,False)
for label,field,value in [('wrong-profile','profile','release'),('wrong-target','target','x86_64-apple-darwin'),('wrong-toolchain','toolchain','rustc incompatible'),('wrong-feature-set','features',['testkit']),('wrong-default-features','defaultFeatures',False),('wrong-effective-testkit','workspaceFeatures',{'crewd':[],'crewd-server':['testkit']})]:
 bad=copy.deepcopy(proof);bad['backend']['buildConfiguration'][field]=value;dump(base/'build-provenance.json',bad);run(label,False)
dump(base/'build-provenance.json',proof)
p=snapshot/paths[0];original=p.read_bytes();p.write_text('mutated source');run('changed-source',False);p.write_bytes(original)
extra=snapshot/'unrecorded';extra.write_text('extra');run('extra-source',False);extra.unlink()
log=base/'gui.log';original=log.read_bytes();log.write_text('sourceRef declaration only');run('fake-log',False);log.write_bytes(original)
p=base/'pre-backend';original=p.read_bytes();p.write_bytes(b'wrong binary');run('changed-presign-file',False);p.write_bytes(original)
run('restored-control',True)
evidence=base/'prebuilt-evidence';evidence.mkdir();rs=evidence/'source-rust';rs.mkdir()
for row in rust:
 p=rs/row['path'];p.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(snapshot/row['path'],p)
shutil.copy2(base/'pre-backend',evidence/'backend-binary')
(evidence/'rust.log').write_text('BuildConfiguration: '+json.dumps(configuration,sort_keys=True,separators=(',',':'))+'\n'+'Snapshot: source-rust\nInputDigest: '+rd+'\nCommand: cd crewd && cargo build --locked -p crewd\nExit: 0\nsynthetic validator-only control\n')
witness={'format':1,'sourceRef':'2'*40,'evidenceDirectory':evidence.name,'snapshotDirectory':rs.name,'files':rust,'rustInputDigest':rd,'buildConfiguration':configuration,'binaryFile':'backend-binary','binarySHA256':sha(evidence/'backend-binary'),'buildLog':'rust.log','buildLogSHA256':sha(evidence/'rust.log')}
dump(base/'backend-witness.json',witness)
prebuilt=copy.deepcopy(proof);prebuilt['backend'].update(mode='verified-prebuilt',witnessFile='backend-witness.json',witnessSHA256=sha(base/'backend-witness.json'));dump(base/'build-provenance.json',prebuilt)
run('different-ref-identical-complete-rust-inputs',True)
only_ref=copy.deepcopy(witness);only_ref['files']=[];dump(base/'backend-witness.json',only_ref)
prebuilt['backend']['witnessSHA256']=sha(base/'backend-witness.json');dump(base/'build-provenance.json',prebuilt);run('prebuilt-only-ref-no-source',False)
dump(base/'backend-witness.json',witness);prebuilt['backend']['witnessSHA256']=sha(base/'backend-witness.json');dump(base/'build-provenance.json',prebuilt)
(evidence/'backend-binary').write_bytes(b'wrong prebuilt binary');run('prebuilt-other-binary',False)
shutil.copy2(base/'pre-backend',evidence/'backend-binary');run('restored-prebuilt',True)
dump(base/'build-provenance.json',proof)
entry=subprocess.run(check,capture_output=True,text=True);(base/'full-entry-green.log').write_text(entry.stdout+entry.stderr);assert entry.returncode==0,(entry.stdout,entry.stderr)
print('PASS: small synthetic structure control plus 18 provenance/configuration mismatch refusals and independently matched prebuilt inputs; not real app build proof')
PY
