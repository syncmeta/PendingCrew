#!/usr/bin/env python3
"""Prepare/build the existing macOS target for optional InjectionNext development.
prepare/build never launch an application; run explicitly starts the private developer scope.
No normal project/config is rewritten.
"""
import argparse
import hashlib
import os
import plistlib
import re
import time
import uuid
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
    # Keep developer-only entry code outside the normal Sources auto-scan.
    target['sources'].append({'path': 'Development/PendingCrewHotReload.swift'})
    target['sources'].append({'path': 'Development/PendingCrewHotReloadRuntime.swift'})
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



def running(executable):
    result = subprocess.run(['pgrep', '-f', '^' + re.escape(str(executable)) + '( |$)'],
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode == 1:
        return None
    if result.returncode != 0:
        raise SystemExit('Process ownership query failed; no build or launch was attempted.')
    pids = result.stdout.split()
    if len(pids) != 1 or not pids[0].isdigit():
        raise SystemExit('Multiple matching developer processes; inspect owners before running again.')
    return int(pids[0])


def private_runtime_root(path):
    path = path.expanduser()
    if not str(path).startswith('/private/tmp/') or os.path.realpath(path) != str(path):
        raise SystemExit('Runtime root must be canonical under /private/tmp, without parent traversal or symlinks.')
    for name in ('frontend', 'authority'):
        if os.path.realpath(path / name) != str(path / name):
            raise SystemExit('Runtime child directories must be canonical and not symlinked.')
    return path


def run_private(args, dd):
    runtime = private_runtime_root(args.runtime_root)
    app = dd / 'Build/Products/Debug/PendingCrewHotReload.app'
    executable = app / 'Contents/MacOS/PendingCrewHotReload'
    if pid := running(executable):
        try:
            owner = json.loads((runtime / 'run.json').read_text())
        except (OSError, ValueError):
            owner = {}
        if owner.get('appPID') != pid or owner.get('app') != str(app) or owner.get('sourceRoot') != str(ROOT):
            raise SystemExit('A developer App is running but its source checkout is not verified for this command. No build or launch was attempted; use its original checkout or normally quit it first.')
        print('Developer App already running, PID', pid, '; no build or duplicate launch.')
        print('Recorded production source checkout:', ROOT / 'Sources')
        print('For structure/configuration changes, normally quit this developer App first, then run again.')
        return
    tool = args.injection_app.expanduser().resolve()
    info = plistlib.loads((tool / 'Contents/Info.plist').read_bytes())
    if info.get('CFBundleShortVersionString') != '2.0.1':
        raise SystemExit('Use the verified official InjectionNext 2.0.1 App.')
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(tool)], check=True)
    signature = subprocess.run(['codesign', '-dv', str(tool)], text=True, capture_output=True, check=True).stderr
    if 'TeamIdentifier=9V5A8WE85E' not in signature:
        raise SystemExit('InjectionNext developer identity differs from the verified official release.')
    # Normal build action is reused; never rebuild/overwrite a live developer App.
    subprocess.run([sys.executable, str(pathlib.Path(__file__).resolve()), 'build', '--derived-data', str(dd)], check=True)
    backend = args.backend_input.expanduser().resolve()
    subprocess.run(['codesign', '--verify', '--strict', str(backend)], check=True)
    core_bytes = backend.read_bytes()
    helper = app / 'Contents/Helpers/crewd'
    helper.parent.mkdir(exist_ok=True)
    if helper.is_symlink():
        raise SystemExit('Refusing a symlinked developer helper.')
    shutil.copy2(backend, helper)
    subprocess.run(['codesign', '--force', '--sign', '-', '--timestamp=none', str(app)], check=True)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
    assert helper.read_bytes() == core_bytes  # Outer signing must preserve the original Core signature.
    runtime.mkdir(mode=0o700, exist_ok=True)
    for name in ('frontend', 'authority'):
        directory = runtime / name
        if directory.is_symlink():
            raise SystemExit('Refusing a symlinked runtime directory.')
        directory.mkdir(mode=0o700, exist_ok=True)
    device_file = runtime / 'developer-device-id'
    if not device_file.exists():
        device_file.write_text(str(uuid.uuid4()) + '\n')
    device_id = str(uuid.UUID(device_file.read_text().strip()))
    tool_exe = tool / 'Contents/MacOS/InjectionNext'
    tool_pid = running(tool_exe)
    if tool_pid is None:
        existing_tool = subprocess.run(['pgrep', '-x', 'InjectionNext'], stdout=subprocess.DEVNULL)
        if existing_tool.returncode == 0:
            raise SystemExit('Another InjectionNext is running; select its verified App path with --injection-app. Nothing was stopped.')
        if existing_tool.returncode != 1:
            raise SystemExit('Tool process query failed; no new process was launched.')
        env = os.environ.copy()
        env.update(INJECTION_DIRECTORIES=str(ROOT / 'Sources'), NSUnbufferedIO='YES')
        logs = dd / 'Logs/Build'
        recent = max(logs.glob('*.xcactivitylog'), key=lambda path: path.stat().st_mtime)
        with (runtime / 'tool-run.log').open('ab') as log:
            child = subprocess.Popen([str(tool_exe), '-devicesEnabled', 'NO', '-mcpServer', 'NO',
                                      '-autoLaunchXcode', 'NO', '-hideXcodeAlert', 'YES',
                                      '-HotReloadingBuildLogsDir', str(recent)],
                                     env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        time.sleep(1)
        if child.poll() is not None:
            raise SystemExit('Official tool exited; inspect tool-run.log before continuing.')
        tool_pid = child.pid
    env = os.environ.copy()
    env.update(PENDINGCREW_HOTRELOAD_PRIVATE_RUNTIME_REVIEWED='1',
               PENDINGCREW_DATA_DIR=str(runtime / 'frontend'), PENDINGCREW_CREWD_DATA_DIR=str(runtime / 'authority'),
               PENDINGCREW_HOTRELOAD_INJECTION_RESOURCES=str(tool / 'Contents/Resources'),
               PENDINGCREW_HOTRELOAD_DEVICE_ID=device_id, PENDINGCREW_ISOLATED_CREDENTIALS='memory',
               PENDINGCREW_CODEX_PROGRAM='/usr/bin/false', PENDINGCREW_CLAUDE_PROGRAM='/usr/bin/false',
               INJECTION_PROJECT_ROOT=str(ROOT / 'Sources'), INJECTION_HOST='127.0.0.1', NSUnbufferedIO='YES')
    with (runtime / 'app-run.log').open('ab') as log:
        child = subprocess.Popen([str(executable)], env=env, stdout=log,
                                 stderr=subprocess.STDOUT, start_new_session=True)
    receipt = dict(appPID=child.pid, toolPID=tool_pid, sourceRoot=str(ROOT), app=str(app),
                   runtimeRoot=str(runtime), coreSHA256=hashlib.sha256(core_bytes).hexdigest())
    (runtime / 'run.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))
    print('Edit production SwiftUI under', ROOT / 'Sources', 'and save. This scope cannot call real model providers.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'build', 'run'])
    parser.add_argument('--derived-data', type=pathlib.Path, default=DEFAULT_DD)
    parser.add_argument('--injection-app', type=pathlib.Path, default=pathlib.Path('/private/tmp/pcw-injectionnext-runtime-2.0.1-20261023/InjectionNext.app'))
    parser.add_argument('--backend-input', type=pathlib.Path, default=pathlib.Path('/Applications/PendingCrew.app/Contents/Helpers/crewd'))
    parser.add_argument('--runtime-root', type=pathlib.Path, default=pathlib.Path('/private/tmp/pendingcrew-hotreload-runtime-20261023'))
    args = parser.parse_args()
    project = prepare()
    dd = args.derived_data.expanduser().resolve()
    if not dd.is_dir() or not (dd / 'SourcePackages').is_dir():
        raise SystemExit('Use an existing coordinated warm DerivedData with SourcePackages; no cold cache is created.')
    if args.action == 'run':
        run_private(args, dd)
        return
    print('Project:', project)
    print('Warm DerivedData:', dd)
    print('InjectionNext 2.0.1 source:', INJECTIONNEXT_REVISION)
    print('Inject 1.6.0 source:', INJECT_REVISION)
    print('No application was installed or launched. Runtime is disabled in the generated scheme.')
    if args.action == 'build':
        if shutil.disk_usage(dd).free < 4 * 1024**3:
            raise SystemExit('Less than 4 GiB free: coordinate disk space; this entry never deletes caches.')
        build_env = os.environ.copy()
        config_index = int(build_env.get('GIT_CONFIG_COUNT', '0'))
        build_env.update({'GIT_CONFIG_COUNT': str(config_index + 1),
                          'GIT_CONFIG_KEY_' + str(config_index): 'http.version',
                          'GIT_CONFIG_VALUE_' + str(config_index): 'HTTP/1.1'})
        subprocess.run(['xcodebuild', '-project', str(project), '-scheme', 'PendingCrew',
                        '-configuration', 'Debug', '-destination', 'platform=macOS',
                        '-derivedDataPath', str(dd), '-clonedSourcePackagesDirPath', str(dd / 'SourcePackages'),
                        '-skipPackageUpdates', '-onlyUsePackageVersionsFromResolvedFile',
                        'build'], cwd=ROOT, env=build_env, check=True)


if __name__ == '__main__':
    main()
