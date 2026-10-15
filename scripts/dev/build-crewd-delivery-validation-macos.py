#!/usr/bin/env python3
"""Exact-source DeliveryValidation preparation; default PLAN, no launch or install.
--execute --adhoc explicitly builds and ad-hoc seals only a private artifact.
Never selects a signing identity, accesses credentials, notarizes or publishes.
"""
import argparse,hashlib,importlib.util,json,os,pathlib,plistlib,re,shutil,stat,subprocess,sys,tarfile,xml.etree.ElementTree as ET
sys.dont_write_bytecode=True
SCHEME='PendingCrewDeliveryValidation'
CONFIG='DeliveryValidation'
IDENTIFIER='com.pendingname.pendingcrew.native-candidate'
CONDITION='PENDINGCREW_DELIVERY_VALIDATION'
DD='/private/tmp/pcw-codex-first-xcode'
SPM='/private/tmp/pcw-codex-first-spm'
TOOLS=['build-crewd-delivery-validation-macos.py','check-crewd-delivery-validation-macos.py','build-crewd-native-macos.py','write-codex-first-provenance.py','crewd-macho-seal-equivalence.py','crewd-validation-load-closure.py']

def sha(p):return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
def save(p,v):p.write_text(json.dumps(v,sort_keys=True,indent=2)+'\n')
def git(repo,*args):return subprocess.check_output(['git','-C',str(repo),*args])
def load(path,name):
 spec=importlib.util.spec_from_file_location(name,path);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module

def frozen_tools(repo,ref):
 if not re.fullmatch('[0-9a-f]{40}',ref) or git(repo,'rev-parse','--verify',ref+'^{commit}').decode().strip()!=ref:raise ValueError('complete reviewed source ref required')
 for name in TOOLS:
  path=repo/'scripts/dev'/name
  if path.read_bytes()!=git(repo,'show',ref+':scripts/dev/'+name):raise ValueError('recipe/consumer/primitives differ from selected source: '+name)
 return load(repo/'scripts/dev/build-crewd-native-macos.py','validation_native'),load(repo/'scripts/dev/write-codex-first-provenance.py','validation_proof')

def require_normal_source(value,ref):
 if value.get('sourceRef')!=ref or value.get('freshWorkspaceArtifacts') is not True:raise ValueError('normal helper must actually be compiled fresh from this complete source ref')
 cfg=value.get('buildConfiguration',{});workspace=cfg.get('workspaceFeatures')
 if cfg.get('defaultFeatures') is not True or cfg.get('allFeatures') is not False or cfg.get('features')!=[] or cfg.get('artifactFeatures')!=[] or not isinstance(workspace,dict) or not workspace or any(v!=[] for v in workspace.values()) or cfg.get('artifactProfile',{}).get('test') is not False or cfg.get('target')!='aarch64-apple-darwin':raise ValueError('Mac default non-test empty-feature normal required')

def configuration_source(read):
 cfg=read('Config/DeliveryValidation.xcconfig').decode()
 required=['CODE_SIGN_STYLE = Manual','CODE_SIGN_IDENTITY = -','DEVELOPMENT_TEAM =','CODE_SIGN_ENTITLEMENTS =','CODE_SIGNING_ALLOWED = NO','ENABLE_HARDENED_RUNTIME = YES',CONDITION]
 if not all(x in cfg for x in required):raise ValueError('explicit isolated validation configuration missing')
 info=plistlib.loads(read('Config/DeliveryValidation-Info.plist'))
 if info.get('PendingCrewDeliveryValidation') is not True:raise ValueError('validation marker missing')
 scheme=ET.fromstring(read('PendingCrew.xcodeproj/xcshareddata/xcschemes/PendingCrewDeliveryValidation.xcscheme'))
 if any(node.get('buildConfiguration')!=CONFIG for node in scheme if node.tag!='BuildAction'):raise ValueError('scheme action configuration mismatch')
 entry=read('Sources/PendingCrewEntry.swift').decode();vault=read('Sources/CrewdRemote/CrewdRemoteVault.swift').decode()
 if '#if PENDINGCREW_DELIVERY_VALIDATION' not in entry or 'CrewdDeliveryValidation.qualified()' not in entry or 'PendingCrewApp.main()' not in entry or 'CrewdValidationCredentials(scope: scope)' not in vault:raise ValueError('original validation entry/factory missing')

def settings_check(rows,ref):
 targets=[row['buildSettings'] for row in rows if row.get('target')=='PendingCrew']
 if len(targets)!=1:raise ValueError('one actual original App target required')
 v=targets[0];conditions=v.get('SWIFT_ACTIVE_COMPILATION_CONDITIONS','').split()
 required={'CONFIGURATION':CONFIG,'PRODUCT_BUNDLE_IDENTIFIER':IDENTIFIER,'ARCHS':'arm64','CODE_SIGNING_ALLOWED':'NO','CODE_SIGN_STYLE':'Manual','ENABLE_HARDENED_RUNTIME':'YES','ENABLE_DEBUG_DYLIB':'NO','ENABLE_PREVIEWS':'NO','PENDINGCREW_VALIDATION_SOURCE_REF':ref}
 if any(v.get(key)!=value for key,value in required.items()):raise ValueError('actual validation settings differ from reviewed request')
 if '-Wl,-dead_strip_dylibs' not in v.get('OTHER_LDFLAGS','').split():raise ValueError('private Mac unused updater linkage exclusion missing')
 if CONDITION not in conditions or any(x in conditions for x in ['DEBUG','PENDINGCREW_NATIVE_CANDIDATE']):raise ValueError('validation credential factory compile channel mismatch')
 if any(v.get(key,'') for key in ['DEVELOPMENT_TEAM','PROVISIONING_PROFILE_SPECIFIER','PROVISIONING_PROFILE','CODE_SIGN_ENTITLEMENTS']):raise ValueError('personal signing/entitlement inputs forbidden')
 if not v.get('INFOPLIST_FILE','').endswith('Config/DeliveryValidation-Info.plist') or v.get('CODE_SIGN_IDENTITY')!='-':raise ValueError('validation Info/signing identity mismatch')
 return v

def sdk_check(settings,tool):
 if not settings.get('SDKROOT','').endswith('MacOSX'+tool['sdkVersion']+'.sdk'):raise ValueError('actual SDK differs from recorded macOS toolchain')

def products(root):
 root=pathlib.Path(root);rows=[]
 for p in sorted(root.rglob('*')):
  name=p.relative_to(root).as_posix();s=p.lstat()
  if any(x in name for x in '\n\r\t'):raise ValueError('invalid product path')
  if p.is_symlink():
   if not p.resolve(strict=True).is_relative_to(root.resolve()):raise ValueError('product link escapes app')
   rows.append({'path':name,'kind':'symlink','target':os.readlink(p)})
  elif stat.S_ISREG(s.st_mode):rows.append({'path':name,'kind':'file','sha256':sha(p),'mode':stat.S_IMODE(s.st_mode)})
  elif not stat.S_ISDIR(s.st_mode):raise ValueError('special product file forbidden')
 return rows

def loaded_code(app,rows):
 result=[];magic={b'\xfe\xed\xfa\xce',b'\xce\xfa\xed\xfe',b'\xfe\xed\xfa\xcf',b'\xcf\xfa\xed\xfe',b'\xca\xfe\xba\xbe',b'\xbe\xba\xfe\xca',b'\xca\xfe\xba\xbf',b'\xbf\xba\xfe\xca'}
 for row in rows:
  if row['kind']!='file':continue
  p=app/row['path']
  with p.open('rb') as f:header=f.read(4)
  if header in magic or p.name=='CodeResources':result.append({'path':row['path'],'sha256':row['sha256']})
 return result

def arguments(argv=None):
 p=argparse.ArgumentParser();p.add_argument('output');p.add_argument('--ref',required=True);p.add_argument('--backend',required=True);p.add_argument('--witness',required=True);p.add_argument('--backend-profile',required=True,choices=['debug','release']);p.add_argument('--derived-data',default=DD);p.add_argument('--spm',default=SPM);p.add_argument('--execute',action='store_true');p.add_argument('--adhoc',action='store_true');return p.parse_args(argv)

def prepare(args,repo):
 if args.execute!=args.adhoc:raise ValueError('artifact build needs both explicit --execute and --adhoc; default is PLAN')
 native,proof=frozen_tools(repo,args.ref)
 out=pathlib.Path(args.output)
 if not out.is_absolute() or out.exists() or out.parent.resolve(strict=True)!=out.parent:raise ValueError('fresh canonical private output required')
 s=out.parent.stat()
 if s.st_uid!=os.geteuid() or stat.S_IMODE(s.st_mode)!=0o700:raise ValueError('owned0700 output parent required')
 for actual,want in [(args.derived_data,DD),(args.spm,SPM)]:
  p=pathlib.Path(actual)
  if actual!=want or not p.is_dir() or p.resolve()!=p:raise ValueError('original canonical warm DD/SPM required')
 for path in [args.backend,args.witness]:
  p=pathlib.Path(path)
  if not p.is_absolute() or p.resolve(strict=True)!=p or not p.is_file():raise ValueError('explicit canonical recorded normal inputs required')
 configuration_source(lambda path:git(repo,'show',args.ref+':'+path))
 if git(repo,'ls-tree','--name-only',args.ref,'--','Config/Local.xcconfig').strip():raise ValueError('personal Local configuration cannot enter validation archive')
 return out,native,proof

def command(snapshot,args):
 return ['/usr/bin/xcodebuild','-project',str(snapshot/'PendingCrew.xcodeproj'),'-scheme',SCHEME,'-configuration',CONFIG,'-destination','platform=macOS,arch=arm64','-derivedDataPath',args.derived_data,'-clonedSourcePackagesDirPath',args.spm,'-disableAutomaticPackageResolution','-skipPackageUpdates','-jobs','2','ARCHS=arm64','CODE_SIGNING_ALLOWED=NO','CODE_SIGN_IDENTITY=-','CODE_SIGN_STYLE=Manual','DEVELOPMENT_TEAM=','PROVISIONING_PROFILE_SPECIFIER=','PROVISIONING_PROFILE=','CODE_SIGN_ENTITLEMENTS=','ENABLE_HARDENED_RUNTIME=YES','ENABLE_DEBUG_DYLIB=NO','ENABLE_PREVIEWS=NO','PENDINGCREW_VALIDATION_SOURCE_REF='+args.ref]

def main():
 args=arguments();repo=pathlib.Path(__file__).resolve().parents[2];out,native,proof=prepare(args,repo)
 planned={'format':1,'mode':'PLAN','sourceRef':args.ref,'scheme':SCHEME,'configuration':CONFIG,'identifier':IDENTIFIER,'compileCondition':CONDITION,'backendProfile':args.backend_profile,'normalProofVerified':False,'actualSettingsVerified':False,'buildCommand':command(out/'source-inputs',args)+['build'],'actualBuild':False,'adhocSeal':False,'usesAppleSigningIdentity':False,'KeychainAccess':False,'AppLaunched':False,'installed':False,'published':False}
 if not args.execute:print(json.dumps(planned,indent=2));return
 require_normal_source(json.loads(pathlib.Path(args.witness).read_text()),args.ref)
 if shutil.disk_usage(out.parent).free<2684354560:raise ValueError('2.5GiB minimum free before artifact build')
 out.mkdir(mode=0o700);snapshot=out/'source-inputs';snapshot.mkdir();archive=out/'source.tar';git(repo,'archive','--format=tar','-o',str(archive),args.ref)
 with tarfile.open(archive) as t:
  if any(pathlib.PurePosixPath(m.name).is_absolute() or '..' in pathlib.PurePosixPath(m.name).parts or not(m.isfile() or m.isdir()) for m in t):raise ValueError('unsafe committed input')
  t.extractall(snapshot)
 archive.unlink();sources=native.regular_inventory(snapshot);rust=[r for r in sources if r['path'].startswith('crewd/')]
 os.environ['PENDINGCREW_BUILD_BACKEND_PROFILE']=args.backend_profile
 witness,evidence=proof.verify_witness(pathlib.Path(args.witness),pathlib.Path(args.backend),proof.digest(rust),proof.requested_configuration(snapshot/'crewd'),require_fresh=True)
 require_normal_source(witness,args.ref)
 shutil.copytree(evidence,out/'backend-evidence');shutil.copy2(args.witness,out/'original-backend-witness.json');witness['evidenceDirectory']='backend-evidence';save(out/'backend-witness.json',witness);shutil.copy2(args.backend,out/'crewd.pre-sign')
 lock=json.loads((snapshot/'PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved').read_text());pins=[];deps=out/'dependency-inputs';deps.mkdir();spm=pathlib.Path(args.spm)
 for row in lock['pins']:
  name=row['identity'];checkout=spm/'checkouts'/name;revision=git(checkout,'rev-parse','HEAD').decode().strip()
  if revision!=row['state']['revision'] or git(checkout,'status','--porcelain').strip():raise ValueError('locked package differs: '+name)
  pins.append({'identity':name,'revision':revision});native.copied_inputs(checkout,deps/'checkouts'/name)
 if (spm/'artifacts').exists():native.copied_inputs(spm/'artifacts',deps/'artifacts')
 dependencies=native.regular_inventory(deps)
 if native.external_dependency_inventory(spm,pins)!=dependencies:raise ValueError('SPM differs before build')
 argv=command(snapshot,args);native.query(argv+['-showBuildSettings','-json'],snapshot,out/'settings.log',out/'settings.json');settings=settings_check(json.loads((out/'settings.json').read_text()),args.ref)
 tool={'xcodeVersion':native.query(['/usr/bin/xcodebuild','-version'],snapshot,out/'toolchain.log'),'sdkVersion':native.query(['/usr/bin/xcrun','--sdk','macosx','--show-sdk-version'],snapshot,out/'toolchain.log')};sdk_check(settings,tool);save(out/'toolchain.json',tool)
 manifest={'format':1,'sourceRef':args.ref,'sourceSelection':'committed-archive','files':sources,'sourceInputDigest':proof.digest(sources),'rustInputDigest':proof.digest(rust),'dependencyFiles':dependencies,'dependencyInputDigest':proof.digest(dependencies),'packagePins':pins,'externalSPM':args.spm,'derivedData':args.derived_data,'scheme':SCHEME,'configuration':CONFIG,'compileCondition':CONDITION,'backendProfile':args.backend_profile,'backendBuildConfiguration':witness['buildConfiguration'],'command':argv+['build'],'toolchain':tool};save(out/'build-inputs.json',manifest)
 native.run(argv+['build'],snapshot,out/'build.log')
 after=native.external_dependency_inventory(spm,pins)
 if sources!=native.regular_inventory(snapshot) or dependencies!=after:raise ValueError('source or complete SPM inputs changed')
 save(out/'dependency-build-inventory.json',{'before':dependencies,'after':after,'externalPath':args.spm})
 closure=load(repo/'scripts/dev/crewd-validation-load-closure.py','validation_load_policy')
 built=closure.reviewed_target_root(settings['TARGET_BUILD_DIR'],existing=True)/settings['FULL_PRODUCT_NAME']
 if not built.is_dir() or not built.resolve().is_relative_to(pathlib.Path(args.derived_data)):raise ValueError('actual original target product outside warm DD')
 # Explicit unsigned-target finalization also handles XCBuild's deferred package
 # embedding order; it precedes every actual-target inventory/copy, never repairs
 # a sealed/staged artifact. The same full guard is used by the project phase.
 env=dict(settings,PLATFORM_NAME='macosx')
 finalization=closure.prune_target(env);save(out/'unsigned-target-finalization.json',finalization)
 if (built/'Contents/Helpers/crewd').exists():raise ValueError('unexpected helper in raw Xcode target; normal is injected only after capture')
 target_closure=closure.audit(built);save(out/'target-load-closure.json',target_closure)
 builtrows=products(built);save(out/'xcode-products.json',builtrows);app=out/'PendingCrew Delivery Validation.app';shutil.copytree(built,app,symlinks=True)
 info=plistlib.loads((app/'Contents/Info.plist').read_bytes())
 if info.get('CFBundleIdentifier')!=IDENTIFIER or info.get('PendingCrewDeliveryValidation') is not True or info.get('PendingCrewSourceRef')!=args.ref or info.get('BuildStampCommit')!=args.ref:raise ValueError('actual compiled Info marker/ID/source differs; do not repair it after build')
 exe=app/'Contents/MacOS'/info['CFBundleExecutable'];shutil.copy2(exe,out/'gui.pre-sign');save(out/'gui-pre-sign-products.json',products(app));helper=app/'Contents/Helpers/crewd';helper.parent.mkdir(exist_ok=True);shutil.copy2(out/'crewd.pre-sign',helper)
 preseal=out/'seal-pre-sign-code';preseal.mkdir(mode=0o700);code_sources=[]
 for row in loaded_code(app,products(app)):
  if pathlib.PurePosixPath(row['path']).name=='CodeResources':continue
  target=preseal/row['path'];target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(app/row['path'],target)
  if sha(target)!=row['sha256']:raise ValueError('pre-seal code capture changed')
  code_sources.append(row)
 save(out/'seal-code-inputs.json',code_sources)
 # Explicit inside-out code sealing. --deep is verification only, not signing.
 for row in sorted(code_sources,key=lambda row:(-len(pathlib.PurePosixPath(row['path']).parts),row['path'])):
  native.run(['/usr/bin/codesign','--force','--sign','-','--options','runtime',str(app/row['path'])],snapshot,out/'adhoc-sign.log')
 native.run(['/usr/bin/codesign','--force','--sign','-','--options','runtime',str(app)],snapshot,out/'adhoc-sign.log')
 root_queries=[]
 for options,logname in [(['--verify','--deep','--strict'],'signature-check.log'),(['--display','--verbose=4'],'signature-display.log'),(['--display','--entitlements','-','--xml'],'entitlements.log')]:
  query=closure.readonly_query(['/usr/bin/codesign',*options,str(app)]);root_queries.append(query);(out/logname).write_text(closure.query_log(query))
  if query['exitCode']:raise ValueError('actual root signature read failed; preserve '+logname)
 save(out/'root-signature-queries.json',root_queries)
 (out/'entitlements.plist').write_text(root_queries[-1]['stdout'])
 ent=plistlib.loads((out/'entitlements.plist').read_bytes()) if (out/'entitlements.plist').read_bytes().strip() else {}
 if ent:raise ValueError('validation bundle must not carry production/debug/shared entitlements')
 seal=load(repo/'scripts/dev/crewd-macho-seal-equivalence.py','validation_seal');equivalence=[]
 for row in code_sources:equivalence.append({'path':row['path'],'comparison':seal.equivalent_files(preseal/row['path'],app/row['path'])})
 save(out/'seal-code-equivalence.json',equivalence)
 final=products(app);save(out/'packaged-products.json',final)
 sealed_closure=closure.audit(app);save(out/'sealed-load-closure.json',sealed_closure)
 save(out/'loaded-code-signatures.json',closure.signature_audit(app,sealed_closure))
 files=['build-inputs.json','backend-witness.json','original-backend-witness.json','crewd.pre-sign','gui.pre-sign','settings.json','settings.log','toolchain.json','toolchain.log','build.log','dependency-build-inventory.json','xcode-products.json','gui-pre-sign-products.json','packaged-products.json','adhoc-sign.log','signature-check.log','signature-display.log','entitlements.log','entitlements.plist','seal-code-inputs.json','seal-code-equivalence.json','target-load-closure.json','sealed-load-closure.json','loaded-code-signatures.json','unsigned-target-finalization.json','root-signature-queries.json']
 save(out/'build-provenance.json',{'format':1,'sourceRef':args.ref,'appDirectory':app.name,'files':{name:sha(out/name) for name in files},'allLoadedCode':loaded_code(app,final),'actualArtifactBuilt':True,'signatureMode':'explicit-adhoc','publisherEligible':False,'actualKeychainUsed':False,'AppLaunched':False,'installed':False,'published':False})
 checker=load(repo/'scripts/dev/check-crewd-delivery-validation-macos.py','validation_consumer');checker.check(out,args.ref)
 print('BUILT private DeliveryValidation artifact; no Keychain/App launch/install/publisher action:',app)
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,KeyError,subprocess.CalledProcessError,tarfile.TarError) as e:raise SystemExit('FAIL: '+str(e))
