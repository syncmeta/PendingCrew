#!/usr/bin/env python3
"""Small real gate compile/run + generated-project invariants; no App/Core/Inject execution."""
import json
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
GATE = ROOT / 'Development/PendingCrewHotReload.swift'
with tempfile.TemporaryDirectory(prefix='pcw-hotreload-test-', dir='/private/tmp') as temporary:
    work = pathlib.Path(temporary)
    runner = work / 'main.swift'
    runner.write_text('''import Foundation
@main struct GateTests {
    static func main() {
        let id = "com.pendingname.pendingcrew.hotreload"
        let base = CommandLine.arguments[1]
        var env = ["PENDINGCREW_HOTRELOAD_PRIVATE_RUNTIME_REVIEWED": "1",
                   "PENDINGCREW_DATA_DIR": base + "/frontend",
                   "PENDINGCREW_CREWD_DATA_DIR": base + "/authority"]
        precondition(!PendingCrewHotReload.allowEntry(environment: [:], bundleID: id))
        precondition(!PendingCrewHotReload.allowEntry(environment: env, bundleID: "com.pendingname.pendingcrew"))
        precondition(PendingCrewHotReload.allowEntry(environment: env, bundleID: id))
        env["PENDINGCREW_DATA_DIR"] = NSHomeDirectory() + "/Library/Application Support/PendingCrew"
        precondition(!PendingCrewHotReload.allowEntry(environment: env, bundleID: id))
        env["PENDINGCREW_DATA_DIR"] = base + "/../elsewhere"
        precondition(!PendingCrewHotReload.allowEntry(environment: env, bundleID: id))
        env["PENDINGCREW_DATA_DIR"] = base + "/alias"
        precondition(!PendingCrewHotReload.allowEntry(environment: env, bundleID: id))
        if CommandLine.arguments.count > 2 {
            let actual = CommandLine.arguments[2]
            for name in ["frontend", "authority"] {
                let path = actual + "/" + name
                print("Actual root raw=\(path) standardized=\(URL(fileURLWithPath:path).standardizedFileURL.path) resolved=\(URL(fileURLWithPath:path).resolvingSymlinksInPath().path)")
            }
            env["PENDINGCREW_DATA_DIR"] = actual + "/frontend"
            env["PENDINGCREW_CREWD_DATA_DIR"] = actual + "/authority"
            precondition(PendingCrewHotReload.allowEntry(environment: env, bundleID: id))
            print("Existing actual private runtime roots accepted.")
        }
        print("Runtime gate: absent opt-in, daily identity/path, traversal and symlink rejected; explicit private roots accepted.")
    }
}
''')
    (work / 'frontend').mkdir(mode=0o700)
    (work / 'authority').mkdir(mode=0o700)
    (work / 'alias').symlink_to(pathlib.Path.home(), target_is_directory=True)
    executable = work / 'gate-tests'
    subprocess.run(['xcrun', 'swiftc', '-parse-as-library', '-D', 'DEBUG', '-D', 'PENDINGCREW_HOTRELOAD',
                    str(GATE), str(runner), '-o', str(executable)], check=True)
    subprocess.run([str(executable), str(work)] + sys.argv[1:], check=True)
    ordinary = work / 'ordinary.o'
    subprocess.run(['xcrun', 'swiftc', '-parse-as-library', '-emit-object', str(GATE), '-o', str(ordinary)], check=True)
    symbols = subprocess.check_output(['nm', str(ordinary)], text=True)
    assert 'PendingCrewHotReload' not in symbols
    print('Ordinary/Release gate object: no hot-reload symbols.')

subprocess.run(['python3', str(ROOT / 'scripts/dev/swiftui-hot-reload.py'), 'prepare'], check=True)
spec = json.loads((ROOT / '.hotreload/project.json').read_text())
assert spec['configs'] == {'Debug': 'debug'}
assert list(spec['targets']) == ['PendingCrew']
target = spec['targets']['PendingCrew']
assert {'path': 'Development/PendingCrewHotReload.swift'} in target['sources']
settings = target['settings']['configs']['Debug']
assert settings['PRODUCT_BUNDLE_IDENTIFIER'] == 'com.pendingname.pendingcrew.hotreload'
assert settings['CODE_SIGN_ENTITLEMENTS'] == ''
assert settings['CODE_SIGN_ENTITLEMENTS[sdk=macosx*]'] == ''
assert settings['COMPILATION_CACHE_ENABLE_CACHING'] is False
assert '-interposable' in settings['OTHER_LDFLAGS[sdk=macosx*]']
assert spec['packages']['Inject']['revision'] == 'f53380e986a8b056e37d7ce9ca68de610b8415ef'
assert not spec['schemes']['PendingCrew']['run']['environmentVariables']
assert all('Embed' not in phase['name'] for phase in target['postBuildScripts'])
original_pins = json.loads((ROOT / 'PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved').read_text())['pins']
generated_pins = json.loads((ROOT / '.hotreload/PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved').read_text())['pins']
assert all(pin in generated_pins for pin in original_pins)
subprocess.run(['git', 'diff', '--exit-code', '--', 'project.yml', 'PendingCrew.xcodeproj', 'Config/Signing.xcconfig',
                'Config/NativeCandidate.xcconfig', 'Config/DeliveryValidation.xcconfig', 'Config/KnowledgeValidation.xcconfig'], cwd=ROOT, check=True)
entry = (ROOT / 'Sources/PendingCrewEntry.swift').read_text()
assert entry.index('guard PendingCrewHotReload.allowEntry()') < entry.index('CrewdFrontendIsolation.prepare()')
print('Generated variant: existing target/sources, Debug only, exact pin, no runtime opt-in/Core copy; normal project/signing unchanged.')
print('These checks do not prove full App compilation, injection, repaint latency, navigation or input preservation.')
