#!/bin/sh
# Structural proof controls only; no Xcode/Cargo/bundle execution or build claim.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
python3 - "$root" <<'PY'
import copy,hashlib,json,pathlib,plistlib,shlex,subprocess,sys,tempfile
repo=pathlib.Path(sys.argv[1]);base=pathlib.Path(tempfile.mkdtemp(prefix='pcw-native-proof-controls-',dir='/private/tmp'));print(base,flush=True)
app=base/'Synthetic Native.app';(app/'Contents/MacOS').mkdir(parents=True);(app/'Contents/Helpers').mkdir();snapshot=base/'source';snapshot.mkdir();dep=base/'deps';dep.mkdir();wb=base/'backend';ws=wb/'source';ws.mkdir(parents=True)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def dump(p,v):p.write_text(json.dumps(v,sort_keys=True))
def put(root,name,text):p=root/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(text)
def inventory(root):return [{'path':p.relative_to(root).as_posix(),'sha256':sha(p)} for p in sorted(root.rglob('*')) if p.is_file()]
def digest(rows):return hashlib.sha256(''.join(x['path']+'\t'+x['sha256']+'\n' for x in sorted(rows,key=lambda x:x['path'])).encode()).hexdigest()
required=['Sources/CrewdRemote/CrewdNativeApp.swift','Sources/Mac/Crewd/CrewdClient.swift','Sources/Mac/Crewd/CrewdWorkspace.swift','PendingCrew.xcodeproj/project.pbxproj','scripts/dev/build-crewd-native-macos.py','Sources/PendingCrewEntry.swift','crewd/Cargo.toml','crewd/Cargo.lock','crewd/src/main.rs']
for name in required:put(snapshot,name,'synthetic structural control PENDINGCREW_NATIVE_CANDIDATE\n')
lockpath='PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved';pins=[{'identity':'synthetic-package','revision':'2'*40}];put(snapshot,lockpath,json.dumps({'pins':[{'identity':pins[0]['identity'],'state':{'revision':pins[0]['revision']}}]}));put(dep,'checkouts/synthetic-package/Package.swift','synthetic source');put(dep,'artifacts/synthetic/input.bin','synthetic artifact')
rows=inventory(snapshot);rust=[r for r in rows if r['path'].startswith('crewd/')];rd=digest(rust)
for r in rust:put(ws,r['path'],(snapshot/r['path']).read_text())
configuration={'toolchain':'synthetic-rustc','cargoVersion':'synthetic-cargo','target':'aarch64-apple-darwin','profile':'debug','defaultFeatures':True,'allFeatures':False,'features':[],'artifactFeatures':[],'artifactProfile':{'test':False},'workspaceFeatures':{'crewd':[]}}
manifest={'format':2,'sourceRef':'1'*40,'sourceSelection':'committed-archive','snapshotDirectory':'source','files':rows,'sourceInputDigest':digest(rows),'rustInputDigest':rd,'dependencyDirectory':'deps','dependencyFiles':inventory(dep),'dependencyInputDigest':digest(inventory(dep)),'packagePins':pins,'backendBuildConfiguration':configuration,'guiBuildConfiguration':{'buildSystem':'xcodebuild','scheme':'PendingCrew','configuration':'Debug','destination':'platform=macOS,arch=arm64','compileCondition':'PENDINGCREW_NATIVE_CANDIDATE','swiftActiveCompilationConditions':'$(inherited) PENDINGCREW_NATIVE_CANDIDATE','packageResolution':'locked-pre-resolved','xcodeVersion':'synthetic Xcode','sdkVersion':'26.5'}}
put(base,'gui.pre-sign','synthetic GUI bytes');put(base,'crewd.pre-sign','synthetic backend bytes');put(wb,'binary','synthetic backend bytes')
put(wb,'build.log','BuildConfiguration: '+json.dumps(configuration)+'\nSnapshot: source\nInputDigest: '+rd+'\nCommand: cargo build -p crewd --locked\nExit: 0\nsynthetic structural log, never claimed actual compiler execution\n')
witness={'format':1,'sourceRef':'3'*40,'evidenceDirectory':'backend','snapshotDirectory':'source','files':inventory(ws),'rustInputDigest':rd,'buildConfiguration':configuration,'binaryFile':'binary','binarySHA256':sha(wb/'binary'),'buildLog':'build.log','buildLogSHA256':sha(wb/'build.log')};dump(base/'witness.json',witness)
command=['xcodebuild','-project',str(snapshot/'PendingCrew.xcodeproj'),'-scheme','PendingCrew','-configuration','Debug','-destination','platform=macOS,arch=arm64','-derivedDataPath','/synthetic/dd','-clonedSourcePackagesDirPath','/synthetic/spm','-disableAutomaticPackageResolution','-skipPackageUpdates','ARCHS=arm64','CODE_SIGNING_ALLOWED=NO','SWIFT_ACTIVE_COMPILATION_CONDITIONS=$(inherited) PENDINGCREW_NATIVE_CANDIDATE','build']
put(base,'gui.log','Command: '+shlex.join(command)+'\n** BUILD SUCCEEDED **\nExit: 0\nsynthetic structural log, never claimed actual Xcode execution\n')
put(base,'settings.log','Command: '+shlex.join(command[:-1]+['-showBuildSettings','-json'])+'\nExit: 0\n')
dump(base/'settings.json',[{'target':'PendingCrew','buildSettings':{'CONFIGURATION':'Debug','ARCHS':'arm64','SWIFT_ACTIVE_COMPILATION_CONDITIONS':'DEBUG PENDINGCREW_NATIVE_CANDIDATE','SDKROOT':'/synthetic/MacOSX26.5.sdk'}}])
dump(base/'toolchain.json',{'xcodeVersion':'synthetic Xcode','sdkVersion':'26.5'});put(base,'toolchain.log','Command: xcodebuild -version\nsynthetic Xcode\nExit: 0\nCommand: xcrun --sdk macosx --show-sdk-version\n26.5\nExit: 0\n')
put(base,'settings.log','Command: '+shlex.join(command[:-1]+['-showBuildSettings','-json'])+'\n'+(base/'settings.json').read_text()+'\nExit: 0\n')
plist={'CFBundleExecutable':'Synthetic','CFBundleIdentifier':'com.pendingname.pendingcrew.native-candidate','CFBundleShortVersionString':'0.0.12','PendingCrewSourceRef':'1'*40,'BuildStampCommit':'1'*40,'PendingCrewNativeCandidate':True};(app/'Contents/Info.plist').write_bytes(plistlib.dumps(plist));put(app,'Contents/MacOS/Synthetic','synthetic packaged GUI');put(app,'Contents/Helpers/crewd','synthetic packaged backend');put(base,'source-head.txt','1'*40+'\n')
gui={'preSignFile':'gui.pre-sign','preSignSHA256':sha(base/'gui.pre-sign'),'buildLog':'gui.log','buildLogSHA256':sha(base/'gui.log'),'buildSettingsFile':'settings.json','buildSettingsSHA256':sha(base/'settings.json'),'buildSettingsLog':'settings.log','buildSettingsLogSHA256':sha(base/'settings.log'),'toolchainFile':'toolchain.json','toolchainSHA256':sha(base/'toolchain.json'),'toolchainLog':'toolchain.log','toolchainLogSHA256':sha(base/'toolchain.log')}
manifest['externalDependencyPath']='/synthetic/spm'
dump(base/'dependency-build-inventory.json',{'externalPath':'/synthetic/spm','before':manifest['dependencyFiles'],'after':manifest['dependencyFiles']})
proof={'dependencyInventoryFile':'dependency-build-inventory.json','dependencyInventorySHA256':sha(base/'dependency-build-inventory.json'),'format':2,'sourceRef':'1'*40,'inputManifestSHA256':'','appDirectory':app.name,'gui':gui,'backend':{'mode':'verified-prebuilt','buildConfiguration':configuration,'witnessFile':'witness.json','witnessSHA256':sha(base/'witness.json'),'preSignFile':'crewd.pre-sign','preSignSHA256':sha(base/'crewd.pre-sign')},'packaged':{'nativeSHA256':sha(app/'Contents/MacOS/Synthetic'),'helperSHA256':sha(app/'Contents/Helpers/crewd')},'candidateVersion':'0.0.12'}
def run(label,want,m=None,p=None):
 m=copy.deepcopy(manifest if m is None else m);p=copy.deepcopy(proof if p is None else p);dump(base/'build-inputs.json',m);p['inputManifestSHA256']=sha(base/'build-inputs.json');dump(base/'build-provenance.json',p)
 r=subprocess.run(['python3',str(repo/'scripts/dev/check-codex-first-provenance.py'),str(app)],capture_output=True,text=True);put(base,label+'.log',r.stdout+r.stderr);assert (r.returncode==0)==want,(label,r.stdout,r.stderr)
run('valid-structural-control',True)
bad=copy.deepcopy(manifest);bad['externalDependencyPath']='/different/spm';run('different-external-package-path',False,m=bad)
original_inventory=(base/'dependency-build-inventory.json').read_text();value=json.loads(original_inventory);value['after'].append({'path':'artifacts/new.bin','sha256':'a'*64});dump(base/'dependency-build-inventory.json',value);bad=copy.deepcopy(proof);bad['dependencyInventorySHA256']=sha(base/'dependency-build-inventory.json');run('added-external-dependency-after-build',False,p=bad);put(base,'dependency-build-inventory.json',original_inventory)
for label,path,value in [('wrong-profile','profile','release'),('wrong-target','target','wrong-target'),('wrong-features','features',['testkit'])]:
 bad=copy.deepcopy(proof);bad['backend']['buildConfiguration'][path]=value;run(label,False,p=bad)
for label,path,value in [('wrong-sdk','sdkVersion','27.0'),('wrong-condition','compileCondition','OTHER'),('wrong-dependency-digest','dependencyInputDigest','4'*64),('wrong-source-digest','sourceInputDigest','5'*64),('wrong-locked-pin','packagePins',[{'identity':'synthetic-package','revision':'6'*40}])]:
 bad=copy.deepcopy(manifest)
 if path in ['sdkVersion','compileCondition']:bad['guiBuildConfiguration'][path]=value
 else:bad[path]=value
 run(label,False,m=bad)
for label,item,field,value in [('missing-settings','gui','buildSettingsFile','absent.json'),('missing-toolchain','gui','toolchainFile','absent.json'),('wrong-packaged','packaged','helperSHA256','7'*64),('wrong-presign','gui','preSignSHA256','8'*64),('declared-origin','backend','mode','declared-unverified')]:
 bad=copy.deepcopy(proof);bad[item][field]=value;run(label,False,p=bad)
put(dep,'extra-unlisted.bin','extra');run('unlisted-dependency',False);(dep/'extra-unlisted.bin').unlink()
put(snapshot,'extra-unlisted.swift','extra');run('unlisted-source',False);(snapshot/'extra-unlisted.swift').unlink()
realsettings=json.loads((base/'settings.json').read_text());bad=copy.deepcopy(realsettings);bad[0]['buildSettings']['SWIFT_ACTIVE_COMPILATION_CONDITIONS']='DEBUG';dump(base/'settings.json',bad);changed=copy.deepcopy(proof);changed['gui']['buildSettingsSHA256']=sha(base/'settings.json');old_query=(base/'settings.log').read_text();put(base,'settings.log','Command: '+shlex.join(command[:-1]+['-showBuildSettings','-json'])+'\n'+(base/'settings.json').read_text()+'\nExit: 0\n');changed['gui']['buildSettingsLogSHA256']=sha(base/'settings.log');run('actual-settings-lack-condition',False,p=changed);dump(base/'settings.json',realsettings);put(base,'settings.log',old_query)
original_settings_log=(base/'settings.log').read_text();put(base,'settings.log',original_settings_log.replace('ARCHS=arm64','ARCHS=x86_64'));changed=copy.deepcopy(proof);changed['gui']['buildSettingsLogSHA256']=sha(base/'settings.log');run('actual-settings-query-different-inputs',False,p=changed);put(base,'settings.log',original_settings_log)
original_toolchain_log=(base/'toolchain.log').read_text();put(base,'toolchain.log',original_toolchain_log.replace('Exit: 0','Exit: 1',1));changed=copy.deepcopy(proof);changed['gui']['toolchainLogSHA256']=sha(base/'toolchain.log');run('toolchain-command-failed',False,p=changed);put(base,'toolchain.log',original_toolchain_log)
original_query=(base/'settings.log').read_text();put(base,'settings.log','Command: '+shlex.join(command[:-1]+['-showBuildSettings','-json'])+'\nExit: 0\n');changed=copy.deepcopy(proof);changed['gui']['buildSettingsLogSHA256']=sha(base/'settings.log');run('settings-json-not-in-query-output',False,p=changed);put(base,'settings.log',original_query)
put(base,'toolchain.log',original_toolchain_log.replace('synthetic Xcode','different Xcode'));changed=copy.deepcopy(proof);changed['gui']['toolchainLogSHA256']=sha(base/'toolchain.log');run('toolchain-json-not-in-query-output',False,p=changed);put(base,'toolchain.log',original_toolchain_log)
link=dep/'checkouts/synthetic-package/escape';link.symlink_to('/etc/hosts');run('dependency-symlink',False);link.unlink()
run('restored-structural-control',True)
print('PASS Native v2 structural controls; no build/source attestation or signature claim')
PY
