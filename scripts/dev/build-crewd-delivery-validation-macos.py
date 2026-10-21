#!/usr/bin/env python3
"""Build the original Mac App with one SQLite crewd, using the existing warm caches.
Default prints the concrete command. --execute creates a private ad-hoc candidate;
it never launches, installs, uses a personal signing identity, or publishes.
"""
import argparse, hashlib, json, pathlib, plistlib, shutil, subprocess, sys, time

DD = str(pathlib.Path.home() / 'Library/Developer/Xcode/DerivedData/PendingCrew-aehzthzfjzjnxdfaqjkozipivxpn')
SPM = str(pathlib.Path(DD) / 'SourcePackages')

def digest(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()

def run(argv, cwd, log):
    with log.open('ab') as output:
        subprocess.run(argv, cwd=cwd, stdout=output, stderr=subprocess.STDOUT, check=True)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=pathlib.Path)
    parser.add_argument('--ref', required=True)
    parser.add_argument('--backend', required=True, type=pathlib.Path)
    parser.add_argument('--backend-sha256', required=True)
    parser.add_argument('--configuration', choices=['KnowledgeValidation', 'Debug', 'Release'], default='KnowledgeValidation')
    parser.add_argument('--purpose', choices=['product', 'ui-preview', 'remote-preview'], default='product', help='Isolated UI and normal remote previews use fixed distinct bundle identities')
    parser.add_argument('--execute', action='store_true')
    args = parser.parse_args()
    repo = pathlib.Path(__file__).resolve().parents[2]
    actual = subprocess.check_output(['git','rev-parse','HEAD'], cwd=repo, text=True).strip()
    if actual != args.ref or len(actual) != 40:
        raise ValueError('Freeze and select the actual complete source HEAD before packaging')
    dirty = subprocess.check_output(['git','status','--porcelain','--untracked-files=all'], cwd=repo, text=True)
    if dirty:
        raise ValueError('Source must be frozen and clean; existing WIP must be committed, never discarded')
    backend = args.backend.resolve(strict=True)
    if digest(backend) != args.backend_sha256:
        raise ValueError('Backend differs from the coordinated build hash')
    ordinary = args.configuration in ['Debug', 'Release']
    if args.purpose in ['ui-preview', 'remote-preview'] and not ordinary:
        raise ValueError('Preview requires ordinary Debug or Release')
    epoch = int(time.time())
    build_number = str(epoch // 86400) + '.' + format(epoch % 86400, '05d')
    command = ['/usr/bin/xcodebuild','-project',str(repo/'PendingCrew.xcodeproj'),
        '-scheme','PendingCrew' if ordinary else 'PendingCrewKnowledgeValidation','-configuration',args.configuration,
        '-destination','platform=macOS,arch=arm64','-derivedDataPath',DD,
        '-clonedSourcePackagesDirPath',SPM,'-disableAutomaticPackageResolution','-skipPackageUpdates',
        '-jobs','2','ARCHS=arm64','CODE_SIGNING_ALLOWED=NO','CODE_SIGN_IDENTITY=-',
        'DEVELOPMENT_TEAM=','PROVISIONING_PROFILE_SPECIFIER=','PROVISIONING_PROFILE=',
        'CODE_SIGN_ENTITLEMENTS=','ENABLE_HARDENED_RUNTIME=YES','ENABLE_DEBUG_DYLIB=NO','ENABLE_PREVIEWS=NO',
        ('PENDINGCREW_PRODUCT_SOURCE_REF=' if ordinary else 'PENDINGCREW_VALIDATION_SOURCE_REF=')+args.ref,
        'PENDINGCREW_BACKEND_INPUT='+str(backend),'PENDINGCREW_BACKEND_SHA256='+args.backend_sha256,'build']
    if ordinary: command[-1:-1] = ['MARKETING_VERSION=0.1.44', 'CURRENT_PROJECT_VERSION='+build_number]
    if args.purpose == 'ui-preview': command[-1:-1] = ['PRODUCT_BUNDLE_IDENTIFIER=com.pendingname.pendingcrew.ui-preview']
    if args.purpose == 'remote-preview': command[-1:-1] = ['PRODUCT_BUNDLE_IDENTIFIER=com.pendingname.pendingcrew.remote-preview', 'SWIFT_ACTIVE_COMPILATION_CONDITIONS=$(inherited) PENDINGCREW_LOCAL_MAC PENDINGCREW_REMOTE_PRODUCT']
    if not args.execute:
        print(json.dumps({'purpose':args.purpose,'sourceRef':args.ref,'backendSHA256':args.backend_sha256,'buildCommand':command}, indent=2))
        return
    out = args.output
    if not out.is_absolute() or out.exists():
        raise ValueError('Use a fresh absolute output directory')
    if not pathlib.Path(DD).is_dir() or not pathlib.Path(SPM).is_dir():
        raise ValueError('Existing warm caches are required; do not start a cold build')
    out.mkdir(mode=0o700)
    run(command, repo, out/'build.log')
    if subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip() != args.ref or subprocess.check_output(['git','status','--porcelain','--untracked-files=all'],cwd=repo,text=True).strip():
        raise ValueError('Source changed during the coordinated build; preserve output and freeze before retrying')
    if digest(backend) != args.backend_sha256:
        raise ValueError('Coordinated backend input changed during GUI compilation')
    # Xcode may copy package frameworks after post-build scripts in an incremental build.
    # Finalize the actual unsigned target after all Xcode tasks, before capture/sealing.
    sys.dont_write_bytecode = True
    import importlib.util
    spec = importlib.util.spec_from_file_location('candidate_closure',repo/'scripts/dev/crewd-validation-load-closure.py')
    closure = importlib.util.module_from_spec(spec); spec.loader.exec_module(closure)
    built = pathlib.Path(DD)/'Build/Products'/args.configuration/'PendingCrew.app'
    if ordinary and digest(built/'Contents/Helpers/crewd') != args.backend_sha256:
        raise ValueError('Ordinary target did not embed the coordinated normal Core')
    finalization = closure.finalize_unsigned_target(built, args.ref, args.configuration, args.purpose)
    app = out/('PendingCrew Remote Preview.app' if args.purpose == 'remote-preview' else 'PendingCrew UI Preview.app' if args.purpose == 'ui-preview' else 'PendingCrew.app' if ordinary else 'PendingCrew SQLite Candidate.app')
    shutil.copytree(built, app, symlinks=True)
    helpers = app/'Contents/Helpers'
    helpers.mkdir(exist_ok=True)
    shutil.copy2(backend, helpers/'crewd')
    if (helpers/'crewd-data-worker').exists():
        raise ValueError('Unexpected retired worker in candidate')
    info = plistlib.loads((app/'Contents/Info.plist').read_bytes())
    if info.get('PendingCrewSourceRef') != args.ref or info.get('PendingCrewLocalMac' if ordinary else 'PendingCrewKnowledgeValidation') is not True:
        raise ValueError('Built source/configuration marker mismatch')
    if ordinary and (info.get('CFBundleShortVersionString') != '0.1.44' or info.get('CFBundleVersion') != build_number):
        raise ValueError('Ordinary version/build marker mismatch')
    # Sign embedded code inside out, then seal the application. No keychain identity.
    magic = {b'\xfe\xed\xfa\xce',b'\xce\xfa\xed\xfe',b'\xfe\xed\xfa\xcf',b'\xcf\xfa\xed\xfe',b'\xca\xfe\xba\xbe',b'\xbe\xba\xfe\xca',b'\xca\xfe\xba\xbf',b'\xbf\xba\xfe\xca'}
    code = []
    for path in app.rglob('*'):
        if path.is_file() and not path.is_symlink():
            with path.open('rb') as handle:
                if handle.read(4) in magic: code.append(path)
    for path in sorted(code, key=lambda path: -len(path.parts)):
        run(['/usr/bin/codesign','--force','--sign','-','--options','runtime',str(path)], repo, out/'sign.log')
    run(['/usr/bin/codesign','--force','--sign','-','--options','runtime',str(app)], repo, out/'sign.log')
    run(['/usr/bin/codesign','--verify','--deep','--strict',str(app)], repo, out/'signature-check.log')
    loaded = closure.audit(app)
    facts = {'purpose':args.purpose,'uiPreview':args.purpose == 'ui-preview','artifactRole':'ui-preview-only-not-installable' if args.purpose == 'ui-preview' else 'normal-remote-preview-isolated-keychain' if args.purpose == 'remote-preview' else 'mac-candidate','bundleIdentifier':info.get('CFBundleIdentifier'),'sourceRef':args.ref,'configuration':args.configuration,'app':str(app),
        'version':info.get('CFBundleShortVersionString'),'buildNumber':info.get('CFBundleVersion'),
        'backendInputSHA256':args.backend_sha256,'backendSHA256':digest(helpers/'crewd'),
        'binaryHashes':{str(path.relative_to(app)):digest(path) for path in code},
        'buildLogSHA256':digest(out/'build.log'),'loadClosure':loaded,'unsignedTargetFinalization':finalization,'signing':'ad-hoc',
        'buildCommand':command,'configurationHashes':{name:digest(repo/name) for name in (['Config/Signing.xcconfig','Config/Mac-Info.plist'] if ordinary else ['Config/KnowledgeValidation.xcconfig','Config/KnowledgeValidation-Info.plist']) + ['project.yml','PendingCrew.xcodeproj/project.pbxproj']},
        'launched':False,'installed':False,'published':False}
    (out/'candidate.json').write_text(json.dumps(facts,ensure_ascii=False,indent=2)+'\n')
    print(app)

if __name__ == '__main__':
    main()
