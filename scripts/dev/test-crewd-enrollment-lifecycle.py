#!/usr/bin/env python3
"""Offline production begin/commit slice; no App, network, vault or keys."""
import hashlib, json, pathlib, subprocess, sys

root = pathlib.Path(__file__).resolve().parents[2]
out = pathlib.Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
if (out / 'result.json').exists(): raise SystemExit('terminal evidence exists; choose a fresh directory')
enrollment = (root / 'Sources/CrewdRemote/CrewdEnrollment.swift').read_text()
transport = (root / 'Sources/CrewdRemote/CrewdEnrollmentTransport.swift').read_text()
fixture = (root / 'scripts/dev/fixtures/crewd-enrollment-lifecycle.swift').read_text()
begin = enrollment[enrollment.index('    func begin('):enrollment.index('    /// Only an explicit confirmation')]
commit = enrollment[enrollment.index('    func commit('):enrollment.index('    func lookup(')]
has_failure = 'struct CrewdEnrollmentTransportFailure:' in transport
if has_failure:
    types = transport[transport.index('struct CrewdEnrollmentTransportFailure:'):transport.index('/// A fresh, bounded enrollment connection')]
else:
    types = '''struct CrewdEnrollmentTransportFailure: Error {
        enum Request { case begin, commit, lookup }; enum Stage { case connecting, handedToSend }
        enum Cause { case timeout }; let request: Request; let stage: Stage; let cause: Cause
    }'''
source = fixture.replace('/* FAILURE_TYPES */', types).replace('/* PRODUCTION_METHODS */', begin + commit)
source = source.replace(' /* CAN_RESTORE */', ', canRestore: { scope }' if 'canRestore:' in begin else '')
path = out / 'production-slice.swift'; path.write_text(source)
argv = ['/usr/bin/xcrun', 'swiftc', '-swift-version', '5', '-parse-as-library', '-module-cache-path', '/private/tmp/pcw-enrollment-lifecycle-module-cache']
if has_failure: argv += ['-D', 'PRODUCTION_ENROLLMENT_FAILURE']
argv += [str(path), '-o', str(out / 'production-slice')]
compiled = subprocess.run(argv, capture_output=True, text=True)
(out / 'compile.log').write_text(compiled.stdout + compiled.stderr)
run = subprocess.run([str(out / 'production-slice')], capture_output=True, text=True) if compiled.returncode == 0 else None
(out / 'run.log').write_text('' if run is None else run.stdout + run.stderr)
result = {'compileExit': compiled.returncode, 'runExit': None if run is None else run.returncode,
          'productionEnrollmentSHA256': hashlib.sha256(enrollment.encode()).hexdigest(),
          'productionTransportSHA256': hashlib.sha256(transport.encode()).hexdigest(),
          'exactBeginCommitBodies': True, 'networkOrRealCredentials': False, 'argv': argv}
(out / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
# The actual presentation owner must fence both recovery and late UI results.
pairing = (root / 'Sources/CrewdRemote/CrewdRemotePairing.swift').read_text()
operation = pairing[pairing.index('@MainActor final class CrewdRemoteOperation:'):]
operation_source = 'import Foundation\nimport Combine\n' + operation + r"""
@main struct OperationScopeFixture {
    enum Stop: Error { case stopped }
    @MainActor static func main() async {
        let owner=CrewdRemoteOperation()
        let original=owner.recoveryScope()
        precondition(await original(), "current scope")
        owner.invalidate()
        precondition(!(await original()), "closed scope must not recover")
        var applied=false, failed=false
        let a=owner.run({ owner.invalidate();return 1 },apply:{_ in applied=true},failed:{_ in failed=true})!
        await a.value
        precondition(!applied && !failed && !owner.busy, "late success is fenced")
        let b=owner.run({ () async throws -> Int in owner.invalidate();throw Stop.stopped },apply:{_ in applied=true},failed:{_ in failed=true})!
        await b.value
        precondition(!applied && !failed && !owner.busy, "late failure is fenced")
        let current=owner.recoveryScope()
        precondition(await current(), "new current scope")
        print("PASS 5 actual presentation lifetime assertions")
    }
}
"""
# Swift precondition's autoclosure cannot await.
operation_source=operation_source.replace('precondition(await original(),', 'let initial=await original();precondition(initial,').replace('precondition(!(await original()),', 'let invalidated=await original();precondition(!invalidated,').replace('precondition(await current(),','let latest=await current();precondition(latest,')
(out / 'operation-scope.swift').write_text(operation_source)
operation_argv=argv[:argv.index('-D')] if '-D' in argv else argv[:-3]
operation_argv += [str(out / 'operation-scope.swift'), '-o', str(out / 'operation-scope')]
oc=subprocess.run(operation_argv,capture_output=True,text=True)
(out / 'operation-compile.log').write_text(oc.stdout+oc.stderr)
orun=subprocess.run([str(out / 'operation-scope')],capture_output=True,text=True) if oc.returncode==0 else None
(out / 'operation-run.log').write_text('' if orun is None else orun.stdout+orun.stderr)
result['operationCompileExit']=oc.returncode
result['operationRunExit']=None if orun is None else orun.returncode
result['operationSourceSHA256']=hashlib.sha256(operation.encode()).hexdigest()
(out / 'result.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result, indent=2)); print('' if run is None else run.stdout); print('' if orun is None else orun.stdout)
if oc.returncode or orun is None or orun.returncode: sys.exit(1)
sys.exit(compiled.returncode or (1 if run is None else run.returncode))
