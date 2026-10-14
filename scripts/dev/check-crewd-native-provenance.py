#!/usr/bin/python3
"""Read-only Native Xcode candidate evidence checks; never launch bundle code."""
import importlib.util,json,pathlib,plistlib,shlex,sys
sys.dont_write_bytecode = True
spec=importlib.util.spec_from_file_location('base_proof',pathlib.Path(__file__).with_name('check-codex-first-provenance.py'))
p=importlib.util.module_from_spec(spec);spec.loader.exec_module(p)
def load(path):
 value=json.loads(path.read_text())
 if not isinstance(value,dict) or type(value.get('format')) is not int or value['format']!=2:p.fail('Native proof format must be2')
 return value
def check(app):
 base=app.parent;mp=p.regular(base,'build-inputs.json');manifest=load(mp);proof=load(p.regular(base,'build-provenance.json'))
 source=p.hexsha(manifest.get('sourceRef'),40)
 if manifest.get('sourceSelection')!='committed-archive' or proof.get('sourceRef')!=source or (base/'source-head.txt').read_text()!=source+'\n':p.fail('Native frozen source binding mismatch')
 if proof.get('inputManifestSHA256')!=p.sha(mp) or proof.get('appDirectory')!=app.name:p.fail('Native manifest/app binding mismatch')
 snapshot=base/p.basename(manifest.get('snapshotDirectory'));entries=p.files(snapshot,manifest.get('files'));paths={x for x,h in entries}
 required={'Sources/CrewdRemote/CrewdNativeApp.swift','Sources/Mac/Crewd/CrewdClient.swift','Sources/Mac/Crewd/CrewdWorkspace.swift','PendingCrew.xcodeproj/project.pbxproj','PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved','scripts/dev/build-crewd-native-macos.py'}
 if not required.issubset(paths):p.fail('Native complete project inputs missing')
 entry=[x for x in paths if x.endswith('/PendingCrewEntry.swift')]
 if len(entry)!=1:p.fail('unique original candidate entry missing')
 entry_spec=importlib.util.spec_from_file_location('native_entry_contract',pathlib.Path(__file__).with_name('build-crewd-native-macos.py'))
 entry_module=importlib.util.module_from_spec(entry_spec);entry_spec.loader.exec_module(entry_module)
 entry_module.verify_candidate_entry(snapshot)
 rust=[x for x in entries if x[0].startswith('crewd/')];rd=p.digest(rust)
 if manifest.get('sourceInputDigest')!=p.digest(entries) or manifest.get('rustInputDigest')!=rd:p.fail('Native complete source digest mismatch')
 dep=base/p.basename(manifest.get('dependencyDirectory'));deps=p.files(dep,manifest.get('dependencyFiles'))
 if manifest.get('dependencyInputDigest')!=p.digest(deps):p.fail('Native dependency inventory mismatch')
 lock=json.loads((snapshot/'PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved').read_text())
 pins=[{'identity':x['identity'],'revision':x['state']['revision']} for x in lock['pins']]
 if manifest.get('packagePins')!=pins or len({x['identity'] for x in pins})!=len(pins):p.fail('Native package lock/pins mismatch')
 for pin in pins:
  p.hexsha(pin['revision'],40)
  if not any(x.startswith('checkouts/'+pin['identity']+'/') for x,h in deps):p.fail('Native full dependency checkout absent')
 for x,h in deps:
  if not x.startswith(('checkouts/','artifacts/')):p.fail('Native unexpected dependency origin')
 cfg=manifest.get('guiBuildConfiguration',{})
 expected={'buildSystem':'xcodebuild','scheme':'PendingCrew','configuration':'Debug','destination':'platform=macOS,arch=arm64','compileCondition':'PENDINGCREW_NATIVE_CANDIDATE','swiftActiveCompilationConditions':'$(inherited) PENDINGCREW_NATIVE_CANDIDATE','packageResolution':'locked-pre-resolved'}
 if any(cfg.get(k)!=v for k,v in expected.items()) or not all(isinstance(cfg.get(k),str) and cfg[k] for k in ['xcodeVersion','sdkVersion']):p.fail('Native GUI configuration mismatch')
 gui=proof.get('gui',{});backend=proof.get('backend',{});bc=p.build_configuration(manifest.get('backendBuildConfiguration'))
 if backend.get('mode')!='verified-prebuilt' or p.build_configuration(backend.get('buildConfiguration'))!=bc:p.fail('Native backend configuration/origin mismatch')
 for item in [gui,backend]:
  if p.sha(p.regular(base,item.get('preSignFile')))!=p.hexsha(item.get('preSignSHA256')):p.fail('Native pre-sign binary mismatch')
 lp=p.regular(base,gui.get('buildLog'))
 if p.sha(lp)!=p.hexsha(gui.get('buildLogSHA256')):p.fail('Native build log mismatch')
 lines=lp.read_text().splitlines();commands=[shlex.split(x[9:]) for x in lines if x.startswith('Command: ')]
 if len(commands)!=1 or [x[6:] for x in lines if x.startswith('Exit: ')]!=['0'] or '** BUILD SUCCEEDED **' not in lines:p.fail('Native actual Xcode success missing')
 command=commands[0]
 if not command or pathlib.Path(command[0]).name!='xcodebuild' or command[-1]!='build':p.fail('Native actual build command missing')
 for key,want in [('-project',str(snapshot/'PendingCrew.xcodeproj')),('-scheme','PendingCrew'),('-configuration','Debug'),('-destination','platform=macOS,arch=arm64')]:
  if command.count(key)!=1 or command[command.index(key)+1]!=want:p.fail('Native command '+key+' mismatch')
 for token in ['ARCHS=arm64','-disableAutomaticPackageResolution','-skipPackageUpdates','CODE_SIGNING_ALLOWED=NO','SWIFT_ACTIVE_COMPILATION_CONDITIONS=$(inherited) PENDINGCREW_NATIVE_CANDIDATE']:
  if command.count(token)!=1:p.fail('Native locked build command missing '+token)
 external=manifest.get('externalDependencyPath')
 if not isinstance(external,str) or not pathlib.Path(external).is_absolute() or command.count('-clonedSourcePackagesDirPath')!=1 or command[command.index('-clonedSourcePackagesDirPath')+1]!=external:p.fail('Native external dependency build path mismatch')
 di=p.regular(base,proof.get('dependencyInventoryFile'))
 if p.sha(di)!=p.hexsha(proof.get('dependencyInventorySHA256')):p.fail('Native dependency build inventory hash mismatch')
 dv=json.loads(di.read_text())
 if dv!={'externalPath':external,'before':manifest['dependencyFiles'],'after':manifest['dependencyFiles']}:p.fail('Native complete dependencies changed during actual build')
 # Actual settings evidence is required rather than trusting requested options.
 settings=p.regular(base,gui.get('buildSettingsFile'))
 if p.sha(settings)!=p.hexsha(gui.get('buildSettingsSHA256')):p.fail('Native actual build-settings hash mismatch')
 settings_log=p.regular(base,gui.get('buildSettingsLog'))
 if p.sha(settings_log)!=p.hexsha(gui.get('buildSettingsLogSHA256')):p.fail('Native actual settings log hash mismatch')
 settings_text=settings_log.read_text()
 if settings.read_text() not in settings_text:p.fail('Native settings output not present in actual query log')
 slines=settings_text.splitlines();sc=[shlex.split(x[9:]) for x in slines if x.startswith('Command: ')]
 if sc!=[command[:-1]+['-showBuildSettings','-json']] or [x[6:] for x in slines if x.startswith('Exit: ')]!=['0']:p.fail('Native settings were not queried from identical actual build inputs')
 tool=p.regular(base,gui.get('toolchainFile'))
 if p.sha(tool)!=p.hexsha(gui.get('toolchainSHA256')) or json.loads(tool.read_text())!={'xcodeVersion':cfg['xcodeVersion'],'sdkVersion':cfg['sdkVersion']}:p.fail('Native actual SDK/toolchain output mismatch')
 tlog=p.regular(base,gui.get('toolchainLog'))
 if p.sha(tlog)!=p.hexsha(gui.get('toolchainLogSHA256')):p.fail('Native toolchain query log mismatch')
 tool_text=tlog.read_text()
 if not all(v in tool_text for v in [cfg['xcodeVersion'],cfg['sdkVersion']]):p.fail('Native SDK/toolchain output not present in actual query log')
 tlines=tool_text.splitlines();tc=[shlex.split(x[9:]) for x in tlines if x.startswith('Command: ')]
 if tc!=[['xcodebuild','-version'],['xcrun','--sdk','macosx','--show-sdk-version']] or [x[6:] for x in tlines if x.startswith('Exit: ')]!=['0','0']:p.fail('Native actual toolchain query commands missing')
 rows=json.loads(settings.read_text());targets=[x.get('buildSettings',{}) for x in rows if x.get('target')=='PendingCrew']
 if len(targets)!=1:p.fail('Native actual target settings missing')
 actual=targets[0]
 if actual.get('CONFIGURATION')!='Debug' or actual.get('ARCHS')!='arm64' or 'PENDINGCREW_NATIVE_CANDIDATE' not in actual.get('SWIFT_ACTIVE_COMPILATION_CONDITIONS','').split() or not actual.get('SDKROOT','').endswith('MacOSX'+cfg['sdkVersion']+'.sdk'):p.fail('Native actual settings differ from candidate inputs')
 wp=p.regular(base,backend.get('witnessFile'))
 if p.sha(wp)!=p.hexsha(backend.get('witnessSHA256')):p.fail('Native backend witness hash mismatch')
 witness=p.load(wp);p.hexsha(witness.get('sourceRef'),40)
 if p.build_configuration(witness.get('buildConfiguration'))!=bc:p.fail('Native witness configuration mismatch')
 wb=base/p.basename(witness.get('evidenceDirectory'));ws=p.basename(witness.get('snapshotDirectory'));we=p.files(wb/ws,witness.get('files'))
 if sorted(we)!=sorted(rust) or witness.get('rustInputDigest')!=rd:p.fail('Native actual complete backend source differs')
 if p.sha(p.regular(wb,witness.get('binaryFile')))!=p.hexsha(witness.get('binarySHA256')) or witness['binarySHA256']!=backend['preSignSHA256']:p.fail('Native backend pre-sign pairing mismatch')
 p.log(wb,witness,ws,rd,'rust')
 fresh_spec=importlib.util.spec_from_file_location('fresh_proof',pathlib.Path(__file__).with_name('write-codex-first-provenance.py'));fresh=importlib.util.module_from_spec(fresh_spec);fresh_spec.loader.exec_module(fresh)
 tier=fresh.verify_fresh_source(wb,witness)
 if backend.get("sourceProofTier") and backend["sourceProofTier"]!=tier:p.fail("Native backend source-proof tier mismatch")
 plist=plistlib.loads((app/'Contents/Info.plist').read_bytes());version=proof.get('candidateVersion')
 if not isinstance(version,str) or not __import__('re').fullmatch(r'0\.0\.[0-9]+',version) or plist.get('CFBundleShortVersionString')!=version or plist.get('PendingCrewSourceRef')!=source or plist.get('BuildStampCommit')!=source or plist.get('PendingCrewNativeCandidate') is not True or plist.get('CFBundleIdentifier')!='com.pendingname.pendingcrew.native-candidate':p.fail('Native candidate plist/source isolation mismatch')
 exe=p.basename(plist.get('CFBundleExecutable'));packaged=proof.get('packaged',{})
 if packaged.get('nativeSHA256')!=p.sha(app/'Contents/MacOS'/exe) or packaged.get('helperSHA256')!=p.sha(app/'Contents/Helpers/crewd'):p.fail('Native packaged hash mismatch')
 print('PASS: full Native project/dependencies, actual Xcode settings, normal backend and packaged binaries pair; backend source tier: '+tier)
