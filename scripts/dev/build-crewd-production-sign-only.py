#!/usr/bin/env python3
"""Private, explicit-input macOS signing preparation. Never notarize or publish.
Default is read-only planning; --execute needs a separately authorized manifest.
"""
import argparse,hashlib,importlib.util,json,os,pathlib,plistlib,re,shutil,stat,subprocess,sys,time
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
    if v['derivedData']!='/Users/hey/Library/Developer/Xcode/DerivedData/PendingCrew-aehzthzfjzjnxdfaqjkozipivxpn' or v['sourcePackages']!='/Users/hey/Library/Developer/Xcode/DerivedData/PendingCrew-aehzthzfjzjnxdfaqjkozipivxpn/SourcePackages':raise ValueError('original warm caches required; no cold directories')
    repo=pathlib.Path(__file__).resolve().parents[2]
    ref=subprocess.check_output(['git','rev-parse','--verify',v['sourceRef']+'^{commit}'],cwd=repo,text=True).strip()
    if ref!=v['sourceRef']:raise ValueError('exact commit required')
    own=subprocess.check_output(['git','show',ref+':scripts/dev/build-crewd-production-sign-only.py'],cwd=repo)
    if own!=pathlib.Path(__file__).read_bytes():raise ValueError('sign-only recipe must match frozen source')
    return scope,v,repo

def distribution_entitlements(template,profile,team,bundle):
    """Expand only current source claims; a profile is an allowlist, not claims."""
    if not isinstance(template,dict) or set(template)!={'keychain-access-groups'}:raise ValueError('review current source entitlement keys explicitly')
    if not isinstance(profile,dict) or not re.fullmatch('[A-Z0-9]{10}',team) or bundle!='com.pendingname.pendingcrew':raise ValueError('explicit normal Team and bundle required')
    prefixes=profile.get('ApplicationIdentifierPrefix')
    allowed=profile.get('Entitlements',{})
    if not isinstance(allowed,dict) or any(allowed.get(k) is True for k in ['get-task-allow','com.apple.security.get-task-allow','com.apple.security.cs.disable-library-validation']):raise ValueError('distribution profile restrictions invalid')
    if not isinstance(prefixes,list) or len(prefixes)!=1 or not isinstance(prefixes[0],str) or not re.fullmatch('[A-Z0-9]{10}',prefixes[0]):raise ValueError('one actual profile AppIdentifierPrefix required')
    prefix=prefixes[0]+'.'
    def permits(value,pattern):
        return isinstance(pattern,str) and (value==pattern or (pattern.endswith('*') and pattern.count('*')==1 and len(pattern)>1 and value.startswith(pattern[:-1])))
    application=prefix+bundle
    if profile.get('TeamIdentifier')!=[team] or allowed.get('com.apple.developer.team-identifier')!=team or not permits(application,allowed.get('com.apple.application-identifier',allowed.get('application-identifier'))):raise ValueError('profile application/team mismatch')
    groups=template['keychain-access-groups'];allow_groups=allowed.get('keychain-access-groups')
    if not isinstance(groups,list) or not groups or not isinstance(allow_groups,list):raise ValueError('explicit source/profile keychain groups required')
    expanded=[]
    for group in groups:
        if not isinstance(group,str):raise ValueError('source group must be a string')
        claim=group.replace('$(AppIdentifierPrefix)',prefix)
        if not claim or '$' in claim or '*' in claim or any(ord(c)<32 for c in claim) or not any(permits(claim,pattern) for pattern in allow_groups):raise ValueError('unexpanded or unauthorized source entitlement')
        expanded.append(claim)
    if len(set(expanded))!=len(expanded):raise ValueError('duplicate source entitlement')
    return {'com.apple.application-identifier':application,'com.apple.developer.team-identifier':team,'keychain-access-groups':expanded}

def marketing_version(value):
    if not isinstance(value,str) or not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)',value):raise ValueError('numeric three-part marketing version required')
    return value

def qualify_product_info(info,source_ref,build_number,version='0.1.44'):
    version=marketing_version(version)
    if info.get('CFBundleIdentifier')!='com.pendingname.pendingcrew' or info.get('PendingCrewSourceRef')!=source_ref or info.get('BuildStampCommit')!=source_ref or info.get('CFBundleVersion')!=build_number or info.get('CFBundleShortVersionString')!=version:raise ValueError('actual archive source/version differs')

def production_archive_argv(v,snapshot,out,normal_sha,build_number,version='0.1.44'):
    """One production overlay over the protected original local-Core Release.
    Xcode always archives unsigned; the exact reviewed profile and expanded
    source claims are embedded and signed explicitly after archive qualification.
    """
    if not re.fullmatch('[0-9a-f]{64}',normal_sha) or not re.fullmatch(r'[0-9]+\.[0-9]{5}',build_number):raise ValueError('exact backend SHA and ordinary build version required')
    version=marketing_version(version)
    return ['xcodebuild','-project',str(snapshot/'PendingCrew.xcodeproj'),'-scheme','PendingCrew','-configuration','Release','-destination','platform=macOS,arch=arm64','-derivedDataPath',v['derivedData'],'-clonedSourcePackagesDirPath',v['sourcePackages'],'-disableAutomaticPackageResolution','-skipPackageUpdates','-jobs','2','ARCHS=arm64',
        'CODE_SIGNING_ALLOWED=NO','CODE_SIGNING_REQUIRED=NO','CODE_SIGN_IDENTITY=','DEVELOPMENT_TEAM=','PROVISIONING_PROFILE_SPECIFIER=','PROVISIONING_PROFILE=','CODE_SIGN_ENTITLEMENTS=',
        'ENABLE_HARDENED_RUNTIME=YES','ENABLE_DEBUG_DYLIB=NO','ENABLE_PREVIEWS=NO',
        'SWIFT_ACTIVE_COMPILATION_CONDITIONS=$(inherited) PENDINGCREW_LOCAL_MAC PENDINGCREW_REMOTE_PRODUCT',
        'PENDINGCREW_BACKEND_INPUT='+str(out/'crewd.pre-sign'),'PENDINGCREW_BACKEND_SHA256='+normal_sha,
        'PENDINGCREW_PRODUCT_SOURCE_REF='+v['sourceRef'],'MARKETING_VERSION='+version,'CURRENT_PROJECT_VERSION='+build_number,
        '-archivePath',str(out/'PendingCrew.xcarchive')]

def copy_backend_for_archive(source,destination,expected_sha):
    source=pathlib.Path(source);destination=pathlib.Path(destination)
    before=source.lstat()
    if not stat.S_ISREG(before.st_mode) or sha(source)!=expected_sha:raise ValueError('exact regular normal input required')
    if destination.exists() or destination.is_symlink():raise ValueError('fresh private normal copy required')
    shutil.copy2(source,destination);destination.chmod(0o755)
    if sha(destination)!=expected_sha or sha(source)!=expected_sha or source.lstat().st_mode!=before.st_mode:raise ValueError('normal copy changed input or bytes')
    return {'originalMode':oct(before.st_mode&0o777),'privateCopyMode':'0o755','sha256':expected_sha,'originalUnchanged':True}

def production_sign_argv(identity,artifact,timestamp_mode='none',entitlements=None):
    if timestamp_mode not in ['none','secure']:raise ValueError('explicit none/secure timestamp mode required')
    args=['codesign','--force','--sign',identity,'--options','runtime',
          '--timestamp' if timestamp_mode=='secure' else '--timestamp=none']
    if entitlements is not None:args+=['--entitlements',str(entitlements)]
    return args+[str(artifact)]

def main():
    p=argparse.ArgumentParser();p.add_argument('manifest');p.add_argument('--execute',action='store_true');p.add_argument('--timestamp-mode',choices=['none','secure'],default='none');p.add_argument('--marketing-version',type=marketing_version,default='0.1.44');a=p.parse_args()
    scope,v,repo=prepare(a.manifest);manifest_hash=sha(pathlib.Path(a.manifest));out=scope/'signed-output'
    if out.exists():raise ValueError('immutable fresh output required')
    if not a.execute:
        print(json.dumps({'sourceRef':v['sourceRef'],'backendCompiledRef':v['backendCompiledRef'],'configuration':'Release','marketingVersion':a.marketing_version,'remoteProductOverlay':True,'preservesLocalCoreGuards':True,'identifier':'com.pendingname.pendingcrew','hardenedRuntime':True,'timestampMode':a.timestamp_mode,'explicitInputsPresent':True,'profileCompatibilityValidated':False,'identityAvailabilityValidated':False,'output':str(out),'actualSigning':False,'notarize':False,'upload':False,'install':False},indent=2));return
    if shutil.disk_usage(scope).free<3221225472:raise ValueError('3GiB capacity required before build')
    out.mkdir(mode=0o700);snapshot=out/'source-inputs';snapshot.mkdir();archive=out/'source.tar'
    subprocess.run(['git','archive','--format=tar','-o',str(archive),v['sourceRef']],cwd=repo,check=True)
    def load(name,path):
        spec=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
    builder_path=repo/'scripts/dev/build-crewd-native-macos.py'
    if subprocess.check_output(['git','show',v['sourceRef']+':scripts/dev/build-crewd-native-macos.py'],cwd=repo)!=builder_path.read_bytes():raise ValueError('builder must match frozen source')
    source_builder=load('production_source_extract',builder_path)
    symlink_inputs=source_builder.committed_symlink_inputs(repo,v['sourceRef'])
    source_builder.extract_source_archive(archive,snapshot,symlink_inputs);archive.unlink()
    builder=load('production_inventory',snapshot/'scripts/dev/build-crewd-native-macos.py')
    proof=load('production_normal',snapshot/'scripts/dev/write-codex-first-provenance.py')
    closure=load('production_closure',snapshot/'scripts/dev/crewd-validation-load-closure.py');seal=load('production_seal',snapshot/'scripts/dev/crewd-macho-seal-equivalence.py')
    inputs=builder.regular_inventory(snapshot,symlink_inputs);rust=[x for x in inputs if x['path'].startswith('crewd/')]
    os.environ['PENDINGCREW_BUILD_BACKEND_PROFILE']=v['backendProfile']
    witness,evidence=proof.verify_witness(pathlib.Path(v['backendWitness']),pathlib.Path(v['backendPath']),proof.digest(rust),proof.requested_configuration(snapshot/'crewd'),require_fresh=True)
    if witness['sourceRef']!=v['backendCompiledRef']:raise ValueError('actual helper compiled-ref mismatch')
    shutil.copytree(evidence,out/'backend-evidence');witness['evidenceDirectory']='backend-evidence';builder.save(out/'backend-witness.json',witness)
    builder.save(out/'backend-copy.json',copy_backend_for_archive(v['backendPath'],out/'crewd.pre-sign',witness['binarySHA256']))
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
    expected_ent=distribution_entitlements(plistlib.loads((snapshot/'Resources/PendingCrew.entitlements').read_bytes()),profile,v['team'],'com.pendingname.pendingcrew')
    expected_ent_path=out/'expected-distribution-entitlements.plist'
    expected_ent_path.write_bytes(plistlib.dumps(expected_ent))
    local=snapshot/'Config/Local.xcconfig'
    local.write_text('CODE_SIGNING_ALLOWED = NO\nCODE_SIGNING_REQUIRED = NO\nCODE_SIGN_IDENTITY =\nCODE_SIGN_IDENTITY[sdk=macosx*] =\nDEVELOPMENT_TEAM =\nPROVISIONING_PROFILE_SPECIFIER =\nPROVISIONING_PROFILE =\nCODE_SIGN_ENTITLEMENTS =\n')
    epoch=int(time.time());build_number=str(epoch//86400)+'.'+format(epoch%86400,'05d')
    argv=production_archive_argv(v,snapshot,out,witness['binarySHA256'],build_number,a.marketing_version)
    builder.query(argv+['-showBuildSettings','-json'],snapshot,out/'settings.log',out/'settings.json')
    settings=next(x['buildSettings'] for x in json.loads((out/'settings.json').read_text()) if x.get('target')=='PendingCrew')
    if settings.get('PRODUCT_BUNDLE_IDENTIFIER')!='com.pendingname.pendingcrew' or settings.get('DEVELOPMENT_TEAM','')!='' or settings.get('CODE_SIGNING_ALLOWED')!='NO' or settings.get('CODE_SIGNING_REQUIRED')!='NO' or any(settings.get(k,'') for k in ['CODE_SIGN_IDENTITY','CODE_SIGN_ENTITLEMENTS','PROVISIONING_PROFILE_SPECIFIER','PROVISIONING_PROFILE']) or settings.get('ENABLE_HARDENED_RUNTIME')!='YES' or settings.get('ARCHS')!='arm64' or settings.get('ENABLE_DEBUG_DYLIB')!='NO' or not {'PENDINGCREW_LOCAL_MAC','PENDINGCREW_REMOTE_PRODUCT'}.issubset(settings.get('SWIFT_ACTIVE_COMPILATION_CONDITIONS','').split()) or any(x in settings.get('SWIFT_ACTIVE_COMPILATION_CONDITIONS','').split() for x in ['DEBUG','PENDINGCREW_NATIVE_CANDIDATE','PENDINGCREW_DELIVERY_VALIDATION','PENDINGCREW_KNOWLEDGE_VALIDATION']):raise ValueError('actual Release production configuration mismatch')
    builder.run(argv+['archive'],snapshot,out/'archive.log')
    app=out/'PendingCrew.xcarchive/Products/Applications/PendingCrew.app'
    unsigned_qualification=closure.qualify_unsigned_target(app,v['sourceRef'],'Release','product')
    embedded=app/'Contents/embedded.provisionprofile'
    if embedded.exists() or embedded.is_symlink():raise ValueError('unsigned archive unexpectedly embeds a profile')
    if sha(pathlib.Path(v['profilePath']))!=v['profileSHA256']:raise ValueError('profile changed before explicit embedding')
    shutil.copy2(v['profilePath'],embedded)
    if sha(embedded)!=v['profileSHA256']:raise ValueError('explicit embedded reviewed profile differs')
    helper=app/'Contents/Helpers/crewd'
    if sha(helper)!=witness['binarySHA256']:raise ValueError('archive did not embed the exact normal Core input')
    builder.run(production_sign_argv(v['identitySHA1'],helper,a.timestamp_mode),snapshot,out/'helper-sign.log')
    info=app/'Contents/Info.plist';info_value=plistlib.loads(info.read_bytes())
    qualify_product_info(info_value,v['sourceRef'],build_number,a.marketing_version)
    # Archive is a new artifact created by this exact invocation. Preserve its
    # actual main before resealing; never mutate an installed or earlier bundle.
    shutil.copy2(app/'Contents/MacOS'/info_value['CFBundleExecutable'],out/'gui.archive-image')
    before_closure=closure.audit(app,exclude_updater=True)
    updater=app/'Contents/Frameworks/Sparkle.framework'
    if updater.is_symlink():raise ValueError('archive updater root alias')
    if updater.exists():
        for entry in updater.rglob('*'):
            if entry.is_symlink() and not entry.resolve(strict=True).is_relative_to(updater):raise ValueError('archive updater alias escapes')
            if not(entry.is_symlink() or entry.is_file() or entry.is_dir()):raise ValueError('archive updater special file')
        shutil.rmtree(updater)
    builder.run(production_sign_argv(v['identitySHA1'],app,a.timestamp_mode,expected_ent_path),snapshot,out/'app-sign.log')
    builder.run(['codesign','--verify','--deep','--strict',str(app)],snapshot,out/'signature-check.log')
    requirement='anchor apple generic and identifier "com.pendingname.pendingcrew" and certificate leaf[subject.OU] = "'+v['team']+'" and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists'
    builder.run(['codesign','--verify','--strict','--test-requirement','='+requirement,str(app)],snapshot,out/'apple-publisher-requirement.log')
    display=closure.readonly_query(['/usr/bin/codesign','--display','--verbose=4',str(app)])
    signed_ent=closure.readonly_query(['/usr/bin/codesign','--display','--entitlements','-','--xml',str(app)])
    if display['exitCode'] or signed_ent['exitCode']:raise ValueError('actual signature query failed')
    if a.timestamp_mode=='secure':
        helper_display=closure.readonly_query(['/usr/bin/codesign','--display','--verbose=4',str(helper)])
        builder.save(out/'helper-timestamp-query.json',helper_display)
        if helper_display['exitCode'] or not any(line.startswith('Timestamp=') for line in helper_display['stderr'].splitlines()) or not any(line.startswith('Timestamp=') for line in display['stderr'].splitlines()):raise ValueError('secure timestamp missing from helper or App')
    (out/'signature-display.log').write_text(display['stdout']+display['stderr']);(out/'signed-entitlements.plist').write_text(signed_ent['stdout'])
    builder.save(out/'actual-signature-queries.json',{'display':display,'entitlements':signed_ent})
    actual_ent=plistlib.loads(signed_ent['stdout'].encode('utf-8'));text=display['stderr']
    if 'Identifier=com.pendingname.pendingcrew' not in text or 'TeamIdentifier='+v['team'] not in text or 'runtime)' not in text or 'Authority=Developer ID Application:' not in text or 'Authority=Developer ID Certification Authority' not in text or (actual_ent.get('get-task-allow') is True or actual_ent.get('com.apple.security.get-task-allow') is True):raise ValueError('actual final publisher package signature mismatch')
    if actual_ent!=expected_ent:raise ValueError('actual signed entitlements differ from exact expanded source claims')
    if actual_ent.get('com.apple.security.cs.disable-library-validation') is True:raise ValueError('library validation cannot be relaxed')
    final_closure=closure.audit(app)
    equivalence={'GUI':seal.equivalent_files(out/'gui.archive-image',app/'Contents/MacOS'/info_value['CFBundleExecutable']),'backend':seal.equivalent_files(out/'crewd.pre-sign',helper)}
    builder.save(out/'seal-equivalence.json',equivalence)
    after=builder.external_dependency_inventory(spm,pins)
    observed=builder.regular_inventory(snapshot,symlink_inputs);expected=[x for x in inputs if x['path']!='Config/Local.xcconfig'];actual=[x for x in observed if x['path']!='Config/Local.xcconfig']
    if sha(pathlib.Path(a.manifest))!=manifest_hash or sha(pathlib.Path(v['profilePath']))!=v['profileSHA256']:raise ValueError('reviewed signing input changed')
    if expected!=actual or before!=after:raise ValueError('committed inputs or dependencies changed')
    builder.save(out/'sign-only-proof.json',{'sourceRef':v['sourceRef'],'remoteProductOverlay':True,'preservesLocalCoreGuards':True,'version':info_value['CFBundleShortVersionString'],'buildNumber':build_number,'actualArchiveArgv':argv+['archive'],'fullCommittedInputs':inputs,'explicitLocalOverrideSHA256':sha(local),'actualHelperCompiledRef':witness['sourceRef'],'helperOriginalSHA256':witness['binarySHA256'],'buildSettingsSHA256':sha(out/'settings.json'),'manifestSHA256':manifest_hash,'products':builder.regular_inventory(app),'dependencyBefore':before,'dependencyAfter':after,'profileSHA256':v['profileSHA256'],'unsignedArchive':True,'unsignedArchiveQualification':unsigned_qualification,'explicitProfileEmbedding':True,'expectedEntitlements':expected_ent,'expectedEntitlementsSHA256':sha(expected_ent_path),'actualEntitlementsMatchExpected':actual_ent==expected_ent,'signingAction':True,'timestampMode':a.timestamp_mode,'staticAppleRequirement':requirement,'staticAppleRequirementLogSHA256':sha(out/'apple-publisher-requirement.log'),'staticAppleRequirementVerified':True,'loadClosure':final_closure,'archiveLoadClosureBeforeUpdaterExclusion':before_closure,'sealEquivalenceSHA256':sha(out/'seal-equivalence.json'),'publisherRuntimeAcceptance':False,'AppLaunched':False,'installed':False,'notarized':False,'published':False})
    print('BUILT private sign-only product; no launch/install/notarize/publication')
if __name__=='__main__':
    try:main()
    except (ValueError,OSError,KeyError,subprocess.CalledProcessError) as e:raise SystemExit('FAIL: '+str(e))
