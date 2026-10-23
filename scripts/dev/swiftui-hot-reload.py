#!/usr/bin/env python3
"""Prepare/build the existing macOS target for optional InjectionNext development.
Never installs or launches any application. No normal project/config is rewritten.
"""
import argparse
import json
import pathlib
import shutil
import shlex
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
INJECT_REVISION = 'f53380e986a8b056e37d7ce9ca68de610b8415ef'  # official 1.6.0
INJECTIONNEXT_REVISION = '2bb9fd51d7afd1f58e3000f09071bd3bdad9c527'  # official 2.0.1
DEFAULT_DD = pathlib.Path.home() / 'Library/Developer/Xcode/DerivedData/PendingCrew-aehzthzfjzjnxdfaqjkozipivxpn'


def prepare():
    output = ROOT / '.hotreload'
    if output.is_symlink():
        raise SystemExit('Refusing a symlinked .hotreload directory.')
    output.mkdir(exist_ok=True)
    spec = json.loads(subprocess.check_output(
        ['xcodegen', 'dump', '--type', 'json', '--spec', str(ROOT / 'project.yml'), '--no-env'], cwd=ROOT))
    # A local generated variant of the EXISTING target, with original Sources.
    # No validation target, alternative UI, or production recipe changes.
    spec['configs'] = {'Debug': 'debug'}
    spec['configFiles'] = {'Debug': spec['configFiles']['Debug']}
    target = spec['targets']['PendingCrew']
    spec['targets'] = {'PendingCrew': target}
    spec['schemes'] = {'PendingCrew': {
        'build': {'targets': {'PendingCrew': 'all'}},
        'run': {'config': 'Debug', 'environmentVariables': {}},
        'test': {'config': 'Debug', 'targets': []},
        'profile': {'config': 'Debug'}, 'analyze': {'config': 'Debug'},
        'archive': {'config': 'Debug'},
    }}
    target['supportedDestinations'] = ['macOS']
    target.pop('scheme', None)
    target['settings']['configs'] = {'Debug': target['settings']['configs']['Debug']}
    debug = target['settings']['configs']['Debug']
    debug.update({
        'PRODUCT_BUNDLE_IDENTIFIER': 'com.pendingname.pendingcrew.hotreload',
        'PRODUCT_NAME': 'PendingCrewHotReload',
        'PRODUCT_MODULE_NAME': 'PendingCrew',
        'INFOPLIST_FILE[sdk=macosx*]': str(ROOT / 'Config/Mac-Info.plist'),
        'CODE_SIGN_STYLE': 'Manual', 'CODE_SIGN_IDENTITY': '-',
        'CODE_SIGN_IDENTITY[sdk=macosx*]': '-', 'DEVELOPMENT_TEAM': '',
        'CODE_SIGN_ENTITLEMENTS': '', 'CODE_SIGN_ENTITLEMENTS[sdk=macosx*]': '',
        'ENABLE_HARDENED_RUNTIME': False,
        'COMPILATION_CACHE_ENABLE_CACHING': False,
        'EMIT_FRONTEND_COMMAND_LINES': True,
        'SWIFT_OPTIMIZATION_LEVEL': '-Onone',
        'SWIFT_ACTIVE_COMPILATION_CONDITIONS[sdk=macosx*]':
            '$(inherited) DEBUG PENDINGCREW_LOCAL_MAC PENDINGCREW_REMOTE_PRODUCT PENDINGCREW_HOTRELOAD',
        'OTHER_LDFLAGS[sdk=macosx*]': '$(inherited) -Wl,-dead_strip_dylibs -Xlinker -interposable',
    })
    # Compile frontend only. A private real-Core runtime is a separate unfinished
    # qualification; do not copy an existing daily helper into this product.
    target['postBuildScripts'] = [phase for phase in target.get('postBuildScripts', [])
                                  if phase['name'] == 'Stamp build info']
    for phase in target['postBuildScripts']:
        phase['script'] = 'export SRCROOT=' + shlex.quote(str(ROOT)) + '\n' + phase['script']
    spec['packages']['Inject'] = {'url': 'https://github.com/krzysztofzablocki/Inject.git', 'revision': INJECT_REVISION}
    target['dependencies'].append({'package': 'Inject', 'product': 'Inject'})
    local_spec = output / 'project.json'
    local_spec.write_text(json.dumps(spec, ensure_ascii=False, indent=2) + '\n')
    subprocess.run(['xcodegen', 'generate', '--spec', str(local_spec), '--project-root', str(ROOT),
                    '--project', str(output), '--quiet'], cwd=ROOT, check=True)
    project = output / 'PendingCrew.xcodeproj'
    resolved = project / 'project.xcworkspace/xcshareddata/swiftpm/Package.resolved'
    existing = ROOT / 'PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved'
    pins = json.loads(existing.read_text())
    pins['pins'] = [pin for pin in pins['pins'] if pin['identity'] != 'inject']
    pins['pins'].append({'identity': 'inject', 'kind': 'remoteSourceControl',
                         'location': 'https://github.com/krzysztofzablocki/Inject.git',
                         'state': {'revision': INJECT_REVISION, 'version': None}})
    # The generated project has a different package graph; Xcode regenerates its
    # originHash. All existing dependency revision pins remain exact.
    pins.pop('originHash', None)
    resolved.parent.mkdir(parents=True, exist_ok=True)
    resolved.write_text(json.dumps(pins, indent=2) + '\n')
    return project


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'build'])
    parser.add_argument('--derived-data', type=pathlib.Path, default=DEFAULT_DD)
    args = parser.parse_args()
    project = prepare()
    dd = args.derived_data.expanduser().resolve()
    if not dd.is_dir() or not (dd / 'SourcePackages').is_dir():
        raise SystemExit('Use an existing coordinated warm DerivedData with SourcePackages; no cold cache is created.')
    print('Project:', project)
    print('Warm DerivedData:', dd)
    print('InjectionNext 2.0.1 source:', INJECTIONNEXT_REVISION)
    print('Inject 1.6.0 source:', INJECT_REVISION)
    print('No application was installed or launched. Runtime is disabled in the generated scheme.')
    if args.action == 'build':
        if shutil.disk_usage(dd).free < 4 * 1024**3:
            raise SystemExit('Less than 4 GiB free: coordinate disk space; this entry never deletes caches.')
        subprocess.run(['xcodebuild', '-project', str(project), '-scheme', 'PendingCrew',
                        '-configuration', 'Debug', '-destination', 'platform=macOS',
                        '-derivedDataPath', str(dd), '-clonedSourcePackagesDirPath', str(dd / 'SourcePackages'),
                        '-skipPackageUpdates', '-onlyUsePackageVersionsFromResolvedFile',
                        'build'], cwd=ROOT, check=True)


if __name__ == '__main__':
    main()
