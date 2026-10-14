#!/usr/bin/env python3
"""Stage original iPhone/iPad candidate products from a verified frozen Mac input.
No device boot, installation, signing identity discovery or App execution.
"""
import argparse, importlib.util, json, pathlib, shutil, subprocess, sys
sys.dont_write_bytecode = True

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('candidate_root')
    parser.add_argument('--platform', choices=['iPhoneOS', 'iOSSimulator'], required=True)
    parser.add_argument('--derived-data', required=True)
    parser.add_argument('--spm', required=True)
    parser.add_argument('--execute', action='store_true')
    args = parser.parse_args()
    root = pathlib.Path(args.candidate_root).resolve(strict=True)
    snapshot = root / 'source-inputs'
    manifest = json.loads((root / 'build-inputs.json').read_text())
    ref = manifest['sourceRef']
    if len(ref) != 40 or any(c not in '0123456789abcdef' for c in ref):
        raise ValueError('committed full source ref required')
    own = snapshot / 'scripts/dev/build-crewd-native-mobile.py'
    if own.read_bytes() != pathlib.Path(__file__).read_bytes():
        raise ValueError('mobile recipe must match the selected complete input')
    spec = importlib.util.spec_from_file_location('native_builder', snapshot / 'scripts/dev/build-crewd-native-macos.py')
    builder = importlib.util.module_from_spec(spec); spec.loader.exec_module(builder)
    builder.verify_candidate_entry(snapshot)
    if builder.regular_inventory(snapshot) != manifest['files']:
        raise ValueError('complete committed input inventory changed')
    dd, spm = pathlib.Path(args.derived_data).resolve(strict=True), pathlib.Path(args.spm).resolve(strict=True)
    if dd.is_relative_to(root) or spm.is_relative_to(root):
        raise ValueError('existing external warm caches required')
    before = builder.external_dependency_inventory(spm, manifest['packagePins'])
    if before != manifest['dependencyFiles']:
        raise ValueError('actual complete SPM input changed')
    sdk = 'iphoneos' if args.platform == 'iPhoneOS' else 'iphonesimulator'
    destination = 'generic/platform=iOS' if sdk == 'iphoneos' else 'generic/platform=iOS Simulator'
    argv = ['xcodebuild', '-project', str(snapshot / 'PendingCrew.xcodeproj'), '-scheme', 'PendingCrewNativeCandidate', '-configuration', 'NativeCandidate', '-destination', destination, '-derivedDataPath', str(dd), '-clonedSourcePackagesDirPath', str(spm), '-disableAutomaticPackageResolution', '-skipPackageUpdates', '-jobs', '2', 'CODE_SIGNING_ALLOWED=NO', 'ARCHS=arm64', 'PENDINGCREW_CANDIDATE_SOURCE_REF=' + ref]
    if not args.execute:
        print(json.dumps({'sourceRef': ref, 'platform': args.platform, 'actualBuildPlanned': argv + ['build'], 'executed': False, 'unsigned': True, 'installReady': False}, indent=2)); return
    free = shutil.disk_usage(root).free
    if free < 2684354560: raise ValueError('actual free capacity below 2.5GiB; no build started')
    out = root / ('apple-' + args.platform)
    out.mkdir()
    builder.save(out / 'capacity-before.json', {'freeBytes': free, 'derivedData': str(dd), 'sourcePackages': str(spm), 'coldCacheCreated': False})
    builder.query(argv + ['-showBuildSettings', '-json'], snapshot, out / 'settings.log', out / 'settings.json')
    settings = json.loads((out / 'settings.json').read_text())
    actual = next(x['buildSettings'] for x in settings if x.get('target') == 'PendingCrew')
    if actual.get('PRODUCT_BUNDLE_IDENTIFIER') != 'com.pendingname.pendingcrew.native-candidate' or actual.get('CONFIGURATION') != 'NativeCandidate' or 'PENDINGCREW_NATIVE_CANDIDATE' not in actual.get('SWIFT_ACTIVE_COMPILATION_CONDITIONS', '').split() or actual.get('CODE_SIGNING_ALLOWED') != 'NO' or actual.get('CODE_SIGN_ENTITLEMENTS') or actual.get('DEVELOPMENT_TEAM') or actual.get('PROVISIONING_PROFILE_SPECIFIER'):
        raise ValueError('isolated unsigned staging settings required')
    builder.run(argv + ['build'], snapshot, out / 'build.log')
    after = builder.external_dependency_inventory(spm, manifest['packagePins'])
    if before != after or builder.regular_inventory(snapshot) != manifest['files']:
        raise ValueError('full source/dependency inputs changed during actual build')
    app = dd / 'Build/Products' / ('NativeCandidate-' + sdk) / 'PendingCrew.app'
    before_copy = [{'path': p.relative_to(app).as_posix(), 'sha256': builder.sha(p)} for p in sorted(app.rglob('*')) if p.is_file()]
    shutil.copytree(app, out / 'PendingCrew.app', symlinks=True)
    import plistlib
    info = plistlib.loads((out / 'PendingCrew.app/Info.plist').read_bytes())
    if info.get('CFBundleIdentifier') != 'com.pendingname.pendingcrew.native-candidate' or info.get('PendingCrewSourceRef') != ref or info.get('PendingCrewNativeCandidate') is not True:
        raise ValueError('actual mobile product source/namespace missing')
    products = [{'path': p.relative_to(out).as_posix(), 'sha256': builder.sha(p)} for p in sorted((out / 'PendingCrew.app').rglob('*')) if p.is_file()]
    if [{'path': row['path'].removeprefix('PendingCrew.app/'), 'sha256': row['sha256']} for row in products] != before_copy:
        raise ValueError('staged copy differs from actual unsigned product')
    signature = subprocess.run(['codesign', '--display', '--verbose=4', str(out / 'PendingCrew.app')], capture_output=True, text=True)
    (out / 'actual-signature-display.log').write_text(signature.stdout + signature.stderr)
    if signature.returncode and 'code object is not signed at all' not in signature.stderr:
        raise ValueError('actual signature status is not safely classified')
    if signature.returncode == 0 and 'Signature=adhoc' not in signature.stderr:
        raise ValueError('unsigned staging unexpectedly acquired a signing identity')
    builder.save(out / 'full-build-proof.json', {'sourceRef': ref, 'sourceInputDigest': manifest['sourceInputDigest'], 'inputManifestSHA256': builder.sha(root / 'build-inputs.json'), 'destination': destination, 'sdk': sdk, 'sdkVersion': builder.query(['xcrun', '--sdk', sdk, '--show-sdk-version'], snapshot, out / 'toolchain.log'), 'configuration': 'NativeCandidate', 'condition': 'PENDINGCREW_NATIVE_CANDIDATE', 'codeSigningAllowed': False, 'signingActionPerformed': False, 'actualSignatureDisplayExit': signature.returncode, 'signatureDisplaySHA256': builder.sha(out / 'actual-signature-display.log'), 'actualUnsignedProductBeforeCopy': before_copy, 'stagedCopyBytesMatch': True, 'actualCommand': argv + ['build'], 'buildLogSHA256': builder.sha(out / 'build.log'), 'settingsSHA256': builder.sha(out / 'settings.json'), 'products': products, 'dependencyBefore': before, 'dependencyAfter': after, 'deviceInstalled': False, 'deviceConnected': False, 'published': False, 'freeBytesBefore': free, 'freeBytesAfter': shutil.disk_usage(root).free, 'installReady': False})
    print('PASS isolated mobile candidate full build; unsigned, not installed')

if __name__ == '__main__':
    try: main()
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error: raise SystemExit('FAIL: ' + str(error))
