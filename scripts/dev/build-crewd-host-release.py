#!/usr/bin/env python3
"""One reviewed Linux host build; no publish, service, provider, or retry action."""
import hashlib, importlib.util, json, os, pathlib, re, subprocess, sys
sys.dont_write_bytecode = True

# This controller admits only the exact frozen Codex/Store20 source tree.
# Independent source acceptance and dispatch authorization remain separate gates.
QUALIFIED_RUST_TREE = '232239060a0851997f1863e2877b1af2629fab63'
SOURCE_RECIPES = [
    'scripts/dev/record-crewd-build.py',
    'scripts/dev/write-codex-first-provenance.py',
    'deploy/linux/package-host-release.py',
    'deploy/linux/pendingcrew-host.py',
    'deploy/linux/managed-host.py',
]
CONTROLLER_RECIPES = ['.github/workflows/crewd-linux-current.yml',
                      'scripts/dev/build-crewd-host-release.py']


def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def git(root, *args): return subprocess.check_output(['git', '-C', str(root), *args])
def save(path, value): path.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')


def inventory(root, ref):
    assert git(root, 'rev-parse', 'HEAD').decode().strip() == ref, 'checkout differs from reviewed SHA'
    assert not git(root, 'status', '--porcelain').strip(), 'clean tracked and untracked checkout required'
    rows = []
    for raw in git(root, 'ls-tree', '-rz', '--full-tree', ref).split(b'\0'):
        if not raw: continue
        metadata, name = raw.split(b'\t', 1)
        mode, kind, blob = metadata.decode().split()
        name = name.decode(); path = root / name
        assert kind == 'blob' and mode in ['100644', '100755'], 'ordinary tracked files only'
        assert not path.is_symlink() and path.is_file(), name
        data = git(root, 'cat-file', 'blob', blob)
        assert path.read_bytes() == data, 'source differs from committed blob: ' + name
        rows.append({'path': name, 'sha256': sha(path), 'bytes': len(data), 'gitMode': mode})
    return rows


def binding(controller, source, controller_sha, source_sha):
    assert all(re.fullmatch('[0-9a-f]{40}', ref) for ref in [controller_sha, source_sha])
    assert controller_sha == os.environ['GITHUB_SHA'], 'controller input must equal actual run SHA'
    before = {'controllerSha': controller_sha, 'sourceSha': source_sha,
              'controllerInputs': inventory(controller, controller_sha),
              'sourceInputs': inventory(source, source_sha)}
    assert git(source, 'rev-parse', source_sha + ':crewd').decode().strip() == QUALIFIED_RUST_TREE, 'qualified Rust tree changed; review a new controller before dispatch'
    assert git(controller, 'rev-parse', controller_sha + ':crewd').decode().strip() == QUALIFIED_RUST_TREE
    for path in SOURCE_RECIPES:
        assert (source / path).read_bytes() == (controller / path).read_bytes(), 'reviewed source recipe changed: ' + path
    for key in ['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
                'CARGO_BUILD_TARGET', 'CREWD_GIT_SHA', 'CREWD_GIT_DIRTY']:
        assert not os.environ.get(key), 'undeclared compiler override: ' + key
    before.update(rustTree=QUALIFIED_RUST_TREE,
                  controllerRecipeHashes={p: sha(controller/p) for p in CONTROLLER_RECIPES},
                  sourceRecipeHashes={p: sha(source/p) for p in SOURCE_RECIPES},
                  runId=os.environ['GITHUB_RUN_ID'], runAttempt=os.environ['GITHUB_RUN_ATTEMPT'])
    return before


def command(argv, cwd, log):
    with log.open('w') as stream:
        stream.write('Argv: ' + json.dumps(argv) + '\n'); stream.flush()
        code = subprocess.run(argv, cwd=cwd, stdout=stream, stderr=subprocess.STDOUT).returncode
        stream.write('\nExit: ' + str(code) + '\n')
    assert code == 0, 'original command failed; retain log, do not retry: ' + str(log)


def main():
    workspace = pathlib.Path(os.environ['GITHUB_WORKSPACE']).resolve()
    controller = workspace/'controller'; source = workspace/'source'
    out = pathlib.Path(os.environ['HOST_EVIDENCE']).resolve()
    c = os.environ['CONTROLLER_SHA']; s = os.environ['SOURCE_SHA']
    mode = sys.argv[1]
    current = binding(controller, source, c, s)
    if mode == 'bind':
        out.mkdir(mode=0o700, exist_ok=False)
        save(out/'source-before.json', current)
        # Same source toolchain applies to fetch as well as the recorded build.
        assert (source/'crewd/rust-toolchain.toml').is_file()
        print('Bound controller and complete source; no build or deployment executed')
        return
    assert mode == 'package'
    assert current == json.loads((out/'source-before.json').read_text()), 'checkout/config changed after binding'
    target = pathlib.Path(os.environ['CARGO_TARGET_DIR']).resolve()
    assert target == source/'crewd/target'
    normal = out/'normal-build'; recorder = source/SOURCE_RECIPES[0]
    command([sys.executable, '-I', str(recorder), 'prepare', str(source), str(normal), s], source, out/'prepare.log')
    binary = target/'release/crewd'
    command([sys.executable, '-I', str(recorder), 'build', str(source), str(normal), str(binary),
             'cargo', 'build', '--offline', '--locked', '--release', '-p', 'crewd', '--message-format=json'], source, out/'build.log')
    witness = out/'normal-build-witness.json'
    spec = importlib.util.spec_from_file_location('original_provenance', source/SOURCE_RECIPES[1])
    proof = importlib.util.module_from_spec(spec); spec.loader.exec_module(proof)
    w = json.loads(witness.read_text())
    expected = {'target':'x86_64-unknown-linux-gnu', 'profile':'release',
                'defaultFeatures':True, 'allFeatures':False, 'features':[]}
    verified, evidence = proof.verify_witness(witness, binary, w['rustInputDigest'], expected, require_fresh=True)
    assert verified['sourceRef'] == s
    bound_rust = [{'path':r['path'], 'sha256':r['sha256']} for r in current['sourceInputs'] if r['path'].startswith('crewd/')]
    assert sorted(bound_rust, key=lambda r:r['path']) == verified['files']
    assert proof.digest(bound_rust) == verified['rustInputDigest']
    cfg = verified['buildConfiguration']
    assert cfg['artifactFeatures'] == [] and not any(cfg['workspaceFeatures'].values()) and cfg['artifactProfile']['test'] is False
    assert binary.read_bytes()[:6] == b'\x7fELF\x02\x01'
    build_env = []
    for line in (normal/'cargo-build.log').read_text().splitlines():
        try: item = json.loads(line)
        except json.JSONDecodeError: continue
        if isinstance(item, dict) and item.get('reason') == 'build-script-executed':
            env = dict(item.get('env', []))
            if 'CREWD_GIT_SHA' in env: build_env.append(env)
    assert len(build_env) == 1 and build_env[0]['CREWD_GIT_SHA'] == s[:12] and build_env[0]['CREWD_GIT_DIRTY'] == 'false'
    save(out/'embedded-build-env.json', build_env)
    digest = sha(binary)
    with (out/'schema.json').open('wb') as stdout, (out/'schema.stderr').open('wb') as stderr:
        code = subprocess.run([str(binary), 'schema'], cwd=source, stdout=stdout, stderr=stderr).returncode
    checked = source/'crewd/schema/crewd-api.schema.json'
    schema = json.loads((out/'schema.json').read_bytes())
    assert code == 0 and not (out/'schema.stderr').read_bytes() and schema == json.loads(checked.read_bytes())
    assert len(schema['methods']) == 141 and all(p in schema['methods'] for p in ['remote.enrollmentCreate', 'remote.enrollmentLookup',
        'notification.deliveryGet', 'session.codex.inputPrepare', 'session.codex.inputLookup',
        'session.codex.inputDiscard', 'session.codex.permissionProfiles', 'session.codex.archive',
        'session.codex.unarchive', 'session.codex.delete', 'session.codex.revert'])
    assert sha(binary) == digest
    # Project only after original graph/fresh-source verification; keep original witness.
    save(out/'build-proof.json', {'sourceCommit':s, 'sourceClean':True, 'defaultFeatures':True,
                                 'features':[], 'test':False, 'binarySha256':digest,
                                 'originalWitnessSha256':sha(witness), 'controllerCommit':c})
    base = 'https://github.com/syncmeta/PendingCrew/releases/download/host-' + s
    command([sys.executable, '-I', str(source/SOURCE_RECIPES[2]), '--core', str(binary),
             '--proof', str(out/'build-proof.json'), '--source', s, '--architecture', 'x86_64',
             '--artifact-url', base+'/host.tar.gz', '--manifest-url', base+'/manifest.json',
             '--output', str(out/'install')], source, out/'package.log')
    after = binding(controller, source, c, s)
    assert current == after
    save(out/'source-after.json', after)
    save(out/'result.json', {'controllerSha':c, 'sourceSha':s, 'rustTree':QUALIFIED_RUST_TREE,
                            'binarySha256':digest, 'originalWitnessSha256':sha(witness),
                            'schemaExitCode':code, 'schemaEqualsChecked':True, 'methodCount':141,
                            'sourceBeforeAfterEqual':True, 'published':False, 'installed':False,
                            'providerExecuted':False, 'runtimeQualified':False,
                            'plannedReleaseBase':base,
                            'packageFiles':{p.name:sha(p) for p in sorted((out/'install').iterdir())}})
    # The normal witness already retains the exact ELF; no second standalone ELF.
    save(out/'artifact-manifest.json', {p.relative_to(out).as_posix():{'sha256':sha(p), 'bytes':p.stat().st_size}
         for p in sorted(out.rglob('*')) if p.is_file() and p.name != 'artifact-manifest.json'})
    print('One normal Linux package prepared; publication, systemd and clients remain unqualified')


if __name__ == '__main__':
    main()
