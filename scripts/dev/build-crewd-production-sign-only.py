#!/usr/bin/env python3
"""Private, explicit-input macOS signing preparation. Never notarize or publish.
Default is read-only planning; --execute needs a separately authorized manifest.
"""
import argparse,hashlib,importlib.util,json,os,pathlib,plistlib,re,shutil,subprocess,sys,tarfile
sys.dont_write_bytecode=True

def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def private(path,directory=False):
    path=pathlib.Path(path)
    if not path.is_absolute() or path.resolve(strict=True)!=path:raise ValueError('canonical absolute input required')
    s=path.lstat()
    if s.st_uid!=os.geteuid() or (s.st_mode & 0o777)!=(0o700 if directory else 0o600) or not (path.is_dir() if directory else path.is_file()):raise ValueError('owner-private input required')
    return path

def prepare(manifest):
    manifest=private(manifest);scope=private(manifest.parent,True)
    v=json.loads(manifest.read_text())
    required={'format','sourceRef','team','identitySHA1','profileUUID','profilePath','profileSHA256','backendPath','backendWitness','backendCompiledRef','backendProfile','derivedData','sourcePackages'}
    if not isinstance(v,dict) or set(v)!=required or type(v['format']) is not int or v['format']!=1:raise ValueError('closed signing manifest required')
    if any(not isinstance(v[k],str) for k in required-{'format'}):raise ValueError('typed signing manifest required')
    for key in ['sourceRef','backendCompiledRef']:
        if not re.fullmatch('[0-9a-f]{40}',v[key]):raise ValueError('full reviewed source ref required')
    if not re.fullmatch('[A-Z0-9]{10}',v['team']) or not re.fullmatch('[0-9A-Fa-f]{40}',v['identitySHA1']):raise ValueError('explicit Team and certificate SHA1 required; no default identity')
    import uuid
    if str(uuid.UUID(v['profileUUID']))!=v['profileUUID']:raise ValueError('explicit canonical profile UUID required')
    profile=private(v['profilePath'])
    if profile.parent!=scope or sha(profile)!=v['profileSHA256']:raise ValueError('reviewed private profile copy/hash required')
    if v['backendProfile'] not in ['debug','release']:raise ValueError('explicit normal helper profile required')
    for key in ['backendPath','backendWitness','derivedData','sourcePackages']:
        p=pathlib.Path(v[key]);
        if not p.is_absolute() or p.resolve(strict=True)!=p:raise ValueError('canonical existing build input required')
    if v['derivedData']!='/private/tmp/pcw-codex-first-xcode' or v['sourcePackages']!='/private/tmp/pcw-codex-first-spm':raise ValueError('original warm caches required; no cold directories')
    repo=pathlib.Path(__file__).resolve().parents[2]
    ref=subprocess.check_output(['git','rev-parse','--verify',v['sourceRef']+'^{commit}'],cwd=repo,text=True).strip()
    if ref!=v['sourceRef']:raise ValueError('exact commit required')
    own=subprocess.check_output(['git','show',ref+':scripts/dev/build-crewd-production-sign-only.py'],cwd=repo)
    if own!=pathlib.Path(__file__).read_bytes():raise ValueError('sign-only recipe must match frozen source')
    return scope,v,repo

def main():
    p=argparse.ArgumentParser();p.add_argument('manifest');p.add_argument('--execute',action='store_true');a=p.parse_args()
    scope,v,repo=prepare(a.manifest);manifest_hash=sha(pathlib.Path(a.manifest));out=scope/'signed-output'
    if out.exists():raise ValueError('immutable fresh output required')
    if not a.execute:
        print(json.dumps({'sourceRef':v['sourceRef'],'backendCompiledRef':v['backendCompiledRef'],'configuration':'Release','identifier':'com.pendingname.pendingcrew','hardenedRuntime':True,'explicitInputsPresent':True,'profileCompatibilityValidated':False,'identityAvailabilityValidated':False,'output':str(out),'actualSigning':False,'notarize':False,'upload':False,'install':False},indent=2));return
    if shutil.disk_usage(scope).free<2684354560:raise ValueError('2.5GiB capacity required before build')
    out.mkdir(mode=0o700);snapshot=out/'source-inputs';snapshot.mkdir();archive=out/'source.tar'
    subprocess.run(['git','archive','--format=tar','-o',str(archive),v['sourceRef']],cwd=repo,check=True)
    with tarfile.open(archive) as t:
        if any(pathlib.PurePosixPath(m.name).is_absolute() or '..' in pathlib.PurePosixPath(m.name).parts or not(m.isfile() or m.isdir()) for m in t):raise ValueError('unsafe committed archive')
        t.extractall(snapshot)
    archive.unlink()
    def load(name,path):
        spec=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
    builder=load('production_inventory',snapshot/'scripts/dev/build-crewd-native-macos.py');proof=load('production_normal',snapshot/'scripts/dev/write-codex-first-provenance.py')
    inputs=builder.regular_inventory(snapshot);rust=[x for x in inputs if x['path'].startswith('crewd/')]
    os.environ['PENDINGCREW_BUILD_BACKEND_PROFILE']=v['backendProfile']
    witness,evidence=proof.verify_witness(pathlib.Path(v['backendWitness']),pathlib.Path(v['backendPath']),proof.digest(rust),proof.requested_configuration(snapshot/'crewd'),require_fresh=True)
    if witness['sourceRef']!=v['backendCompiledRef']:raise ValueError('actual helper compiled-ref mismatch')
    shutil.copytree(evidence,out/'backend-evidence');witness['evidenceDirectory']='backend-evidence';builder.save(out/'backend-witness.json',witness)
    shutil.copy2(v['backendPath'],out/'crewd.pre-sign')
    if sha(out/'crewd.pre-sign')!=witness['binarySHA256']:raise ValueError('frozen helper copy does not match actual normal proof')
    lock=json.loads((snapshot/'PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved').read_text());pins=[{'identity':x['identity'],'revision':x['state']['revision']} for x in lock['pins']]
    spm=pathlib.Path(v['sourcePackages']);before=builder.external_dependency_inventory(spm,pins)
    if sha(pathlib.Path(v['profilePath']))!=v['profileSHA256']:raise ValueError('profile changed before decoding')
    decoded=subprocess.run(['security','cms','-D','-i',v['profilePath']],capture_output=True,check=True)
    (out/'profile-decoded.private.plist').write_bytes(decoded.stdout);profile=plistlib.loads(decoded.stdout)
    from datetime import datetime,timezone
    expiry=profile.get('ExpirationDate')
    if not isinstance(expiry,datetime) or expiry.replace(tzinfo=expiry.tzinfo or timezone.utc)<=datetime.now(timezone.utc):raise ValueError('reviewed profile expired or lacks expiry')
    ent=profile.get('Entitlements',{})
    if str(__import__('uuid').UUID(profile.get('UUID','')))!=v['profileUUID'] or profile.get('TeamIdentifier')!=[v['team']] or (ent.get('get-task-allow') is True or ent.get('com.apple.security.get-task-allow') is True) or ent.get('com.apple.developer.team-identifier')!=v['team'] or ent.get('com.apple.application-identifier',ent.get('application-identifier')) not in [v['team']+'.com.pendingname.pendingcrew',v['team']+'.*']:raise ValueError('actual reviewed distribution profile does not qualify')
    local=snapshot/'Config/Local.xcconfig'
    local.write_text('CODE_SIGN_STYLE = Manual\nDEVELOPMENT_TEAM = '+v['team']+'\nCODE_SIGN_IDENTITY = '+v['identitySHA1']+'\nCODE_SIGN_IDENTITY[sdk=macosx*] = '+v['identitySHA1']+'\nPROVISIONING_PROFILE_SPECIFIER = '+v['profileUUID']+'\nCODE_SIGN_ENTITLEMENTS = Resources/PendingCrew.entitlements\n')
    argv=['xcodebuild','-project',str(snapshot/'PendingCrew.xcodeproj'),'-scheme','PendingCrew','-configuration','Release','-destination','generic/platform=macOS','-derivedDataPath',v['derivedData'],'-clonedSourcePackagesDirPath',v['sourcePackages'],'-disableAutomaticPackageResolution','-skipPackageUpdates','-jobs','2','ENABLE_HARDENED_RUNTIME=YES','ENABLE_DEBUG_DYLIB=NO','-archivePath',str(out/'PendingCrew.xcarchive')]
    builder.query(argv+['-showBuildSettings','-json'],snapshot,out/'settings.log',out/'settings.json')
    settings=next(x['buildSettings'] for x in json.loads((out/'settings.json').read_text()) if x.get('target')=='PendingCrew')
    if settings.get('PRODUCT_BUNDLE_IDENTIFIER')!='com.pendingname.pendingcrew' or settings.get('DEVELOPMENT_TEAM')!=v['team'] or settings.get('ENABLE_HARDENED_RUNTIME')!='YES' or any(x in settings.get('SWIFT_ACTIVE_COMPILATION_CONDITIONS','').split() for x in ['DEBUG','PENDINGCREW_NATIVE_CANDIDATE','PENDINGCREW_DELIVERY_VALIDATION']):raise ValueError('actual Release production configuration mismatch')
    builder.run(argv+['archive'],snapshot,out/'archive.log')
    app=out/'PendingCrew.xcarchive/Products/Applications/PendingCrew.app'
    if not (app/'Contents/embedded.provisionprofile').is_file() or sha(app/'Contents/embedded.provisionprofile')!=v['profileSHA256']:raise ValueError('archive did not embed the exact reviewed profile')
    helper=app/'Contents/Helpers/crewd';helper.parent.mkdir(exist_ok=True);shutil.copy2(out/'crewd.pre-sign',helper)
    builder.run(['codesign','--force','--sign',v['identitySHA1'],'--options','runtime','--timestamp=none',str(helper)],snapshot,out/'helper-sign.log')
    info=app/'Contents/Info.plist';info_value=plistlib.loads(info.read_bytes());info_value.update(PendingCrewSourceRef=v['sourceRef'],BuildStampCommit=v['sourceRef']);info.write_bytes(plistlib.dumps(info_value))
    builder.run(['codesign','--force','--sign',v['identitySHA1'],'--options','runtime','--timestamp=none','--preserve-metadata=entitlements,requirements',str(app)],snapshot,out/'app-sign.log')
    builder.run(['codesign','--verify','--deep','--strict',str(app)],snapshot,out/'signature-check.log')
    requirement='anchor apple generic and identifier "com.pendingname.pendingcrew" and certificate leaf[subject.OU] = "'+v['team']+'" and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists'
    builder.run(['codesign','--verify','--strict','--test-requirement','='+requirement,str(app)],snapshot,out/'apple-publisher-requirement.log')
    display=subprocess.run(['codesign','--display','--verbose=4',str(app)],capture_output=True,check=True);(out/'signature-display.log').write_bytes(display.stdout+display.stderr)
    signed_ent=subprocess.run(['codesign','--display','--entitlements',':-',str(app)],capture_output=True,check=True);(out/'signed-entitlements.plist').write_bytes(signed_ent.stdout)
    actual_ent=plistlib.loads(signed_ent.stdout);text=display.stderr.decode()
    if 'Identifier=com.pendingname.pendingcrew' not in text or 'TeamIdentifier='+v['team'] not in text or 'runtime)' not in text or 'Authority=Developer ID Application:' not in text or 'Authority=Developer ID Certification Authority' not in text or (actual_ent.get('get-task-allow') is True or actual_ent.get('com.apple.security.get-task-allow') is True):raise ValueError('actual final publisher package signature mismatch')
    after=builder.external_dependency_inventory(spm,pins)
    observed=builder.regular_inventory(snapshot);expected=[x for x in inputs if x['path']!='Config/Local.xcconfig'];actual=[x for x in observed if x['path']!='Config/Local.xcconfig']
    if sha(pathlib.Path(a.manifest))!=manifest_hash or sha(pathlib.Path(v['profilePath']))!=v['profileSHA256']:raise ValueError('reviewed signing input changed')
    if expected!=actual or before!=after:raise ValueError('committed inputs or dependencies changed')
    builder.save(out/'sign-only-proof.json',{'sourceRef':v['sourceRef'],'fullCommittedInputs':inputs,'explicitLocalOverrideSHA256':sha(local),'actualHelperCompiledRef':witness['sourceRef'],'helperOriginalSHA256':witness['binarySHA256'],'buildSettingsSHA256':sha(out/'settings.json'),'manifestSHA256':manifest_hash,'products':builder.regular_inventory(app),'dependencyBefore':before,'dependencyAfter':after,'profileSHA256':v['profileSHA256'],'signingAction':True,'staticAppleRequirement':requirement,'staticAppleRequirementLogSHA256':sha(out/'apple-publisher-requirement.log'),'staticAppleRequirementVerified':True,'publisherRuntimeAcceptance':False,'AppLaunched':False,'installed':False,'notarized':False,'published':False})
    print('BUILT private sign-only product; no launch/install/notarize/publication')
if __name__=='__main__':
    try:main()
    except (ValueError,OSError,KeyError,subprocess.CalledProcessError) as e:raise SystemExit('FAIL: '+str(e))
