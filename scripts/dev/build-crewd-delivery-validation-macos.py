#!/usr/bin/env python3
"""Build the original Mac App with one SQLite crewd, using the existing warm caches.
Default prints the concrete command. --execute creates a private ad-hoc candidate;
it never launches, installs, uses a personal signing identity, or publishes.
"""
import argparse, hashlib, json, pathlib, plistlib, shutil, subprocess, sys

DD = '/private/tmp/pcw-codex-first-xcode'
SPM = '/private/tmp/pcw-codex-first-spm'

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
    command = ['/usr/bin/xcodebuild','-project',str(repo/'PendingCrew.xcodeproj'),
        '-scheme','PendingCrewKnowledgeValidation','-configuration','KnowledgeValidation',
        '-destination','platform=macOS,arch=arm64','-derivedDataPath',DD,
        '-clonedSourcePackagesDirPath',SPM,'-disableAutomaticPackageResolution','-skipPackageUpdates',
        '-jobs','2','ARCHS=arm64','CODE_SIGNING_ALLOWED=NO','CODE_SIGN_IDENTITY=-',
        'DEVELOPMENT_TEAM=','PROVISIONING_PROFILE_SPECIFIER=','PROVISIONING_PROFILE=',
        'CODE_SIGN_ENTITLEMENTS=','ENABLE_DEBUG_DYLIB=NO','ENABLE_PREVIEWS=NO',
        'PENDINGCREW_VALIDATION_SOURCE_REF='+args.ref,'build']
    if not args.execute:
        print(json.dumps({'sourceRef':args.ref,'backendSHA256':args.backend_sha256,'buildCommand':command}, indent=2))
        return
    out = args.output
    if not out.is_absolute() or out.exists():
        raise ValueError('Use a fresh absolute output directory')
    if not pathlib.Path(DD).is_dir() or not pathlib.Path(SPM).is_dir():
        raise ValueError('Existing warm caches are required; do not start a cold build')
    out.mkdir(mode=0o700)
    run(command, repo, out/'build.log')
    built = pathlib.Path(DD)/'Build/Products/KnowledgeValidation/PendingCrew.app'
    app = out/'PendingCrew SQLite Candidate.app'
    shutil.copytree(built, app, symlinks=True)
    helpers = app/'Contents/Helpers'
    helpers.mkdir(exist_ok=True)
    shutil.copy2(backend, helpers/'crewd')
    if (helpers/'crewd-data-worker').exists():
        raise ValueError('Unexpected retired worker in candidate')
    info = plistlib.loads((app/'Contents/Info.plist').read_bytes())
    if info.get('PendingCrewSourceRef') != args.ref or info.get('PendingCrewKnowledgeValidation') is not True:
        raise ValueError('Built source/configuration marker mismatch')
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
    # Reuse the existing dependency loader inspection; no compiler-chain proof machinery.
    sys.dont_write_bytecode = True
    import importlib.util
    spec = importlib.util.spec_from_file_location('candidate_closure',repo/'scripts/dev/crewd-validation-load-closure.py')
    closure = importlib.util.module_from_spec(spec); spec.loader.exec_module(closure)
    loaded = closure.audit(app)
    facts = {'sourceRef':args.ref,'configuration':'KnowledgeValidation','app':str(app),
        'backendInputSHA256':args.backend_sha256,'backendSHA256':digest(helpers/'crewd'),
        'binaryHashes':{str(path.relative_to(app)):digest(path) for path in code},
        'buildLogSHA256':digest(out/'build.log'),'loadClosure':loaded,'signing':'ad-hoc',
        'buildCommand':command,'configurationHashes':{name:digest(repo/name) for name in ['Config/KnowledgeValidation.xcconfig','Config/KnowledgeValidation-Info.plist','project.yml','PendingCrew.xcodeproj/project.pbxproj']},
        'launched':False,'installed':False,'published':False}
    (out/'candidate.json').write_text(json.dumps(facts,ensure_ascii=False,indent=2)+'\n')
    print(app)

if __name__ == '__main__':
    main()
