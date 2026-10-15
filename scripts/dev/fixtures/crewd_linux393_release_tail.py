#!/usr/bin/env python3
"""Linux Release393 narrow managed startup tail; source-only until approved CI.
No installation/CAS, backup-DDL rollback, SDK, model, App, or credential access.
The workflow must strong-verify both original canonical packages before this run.
"""
import sys
sys.dont_write_bytecode = True
import argparse, hashlib, json, os, pathlib, re, shutil, signal, socket, stat, struct, subprocess, time, uuid

DIGEST393 = '530de912ebc0fb615f36ed444e7ed1867bcf72510b639f6467e5633195e09a06'
OLD_REF = '4e80134b9b5ff5a9b759aed1446f61da489efc4c'
OLD_BINARY = 'd891b4d84b5e8b35c9f502504ae5da5569ee330c7f23ba489c746cc8f6a5b8e5'
MAX_FRAME = 1048576

def require(value, message):
    if not value: raise ValueError(message)

def sha(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(1048576), b''): h.update(chunk)
    return h.hexdigest()

def write_json(path, value):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as f:
        json.dump(value, f, sort_keys=True, indent=2); f.write('\n'); f.flush(); os.fsync(f.fileno())

def birth(pid):
    root = pathlib.Path('/proc') / str(pid)
    raw = (root / 'stat').read_text(); fields = raw[raw.rfind(')') + 2:].split()
    uid = [line for line in (root / 'status').read_text().splitlines() if line.startswith('Uid:')]
    require(len(uid) == 1, 'kernel UID unreadable')
    return {'pid': pid, 'ppid': int(fields[1]), 'pgid': int(fields[2]),
            'startTime': int(fields[19]), 'bootId': pathlib.Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
            'uid': int(uid[0].split()[2])}

def same(left, right):
    return all(left[k] == right[k] for k in ('pid', 'pgid', 'startTime', 'bootId', 'uid'))

def registry(data):
    path = data / 'exec-registry.json'
    if not os.path.lexists(path): return {'absent': True}
    st = path.lstat()
    require(stat.S_ISREG(st.st_mode) and st.st_uid == os.geteuid() and path.resolve() == path and st.st_size <= MAX_FRAME, 'registry unknown/alias/special')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        before = os.fstat(fd); raw = os.read(fd, MAX_FRAME + 1); after = os.fstat(fd)
        require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) ==
                (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns), 'registry unstable')
        value = json.loads(raw)
        require(isinstance(value, dict), 'registry malformed')
        return {'absent': False, 'entries': value, 'sha256': hashlib.sha256(raw).hexdigest()}
    finally: os.close(fd)

def physical(data):
    rows = {}
    for path in sorted(data.iterdir()):
        if not (path.name.startswith('crewd.db') or path.name == 'exec-registry.json'): continue
        st = path.lstat(); require(stat.S_ISREG(st.st_mode) and path.resolve() == path, 'physical input not regular')
        rows[path.name] = {'sha256': sha(path), 'size': st.st_size, 'mode': stat.S_IMODE(st.st_mode)}
    return rows

def copy_data(source, destination):
    destination.mkdir(mode=0o700)
    for path in source.iterdir():
        if path.name.startswith('crewd.db') and stat.S_ISREG(path.lstat().st_mode): shutil.copy2(path, destination / path.name)

class Public:
    def __init__(self, sock, proc, captured, record):
        self.sock, self.proc, self.captured, self.record = sock, proc, captured, record
        self.sequence, self.buffer = 0, b''
        pid, uid, gid = struct.unpack('3i', sock.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
        require(pid == proc.pid and uid == os.geteuid() and same(captured, birth(pid)), 'actual socket peer not owned daemon')
        record['peer'] = {'pid': pid, 'uid': uid, 'gid': gid, 'birth': birth(pid)}
        self.hello = self.call('system.hello', {})
    def call(self, method, params):
        self.sequence += 1
        request = {'id': self.sequence, 'method': method, 'params': params}
        self.record.setdefault('requests', []).append(request)
        deadline = min(time.monotonic() + 8, self.record['runtimeDeadline'])
        raw = (json.dumps(request, separators=(',', ':')) + '\n').encode()
        require(len(raw) <= MAX_FRAME, 'request oversized')
        write_json(pathlib.Path(self.record['preparedDirectory']) / (self.record['label'] + '-request-%03d.json' % self.sequence), request)
        self.sock.settimeout(max(.001, deadline - time.monotonic())); self.sock.sendall(raw)
        for _ in range(256):
            while b'\n' not in self.buffer:
                require(time.monotonic() < deadline, 'whole RPC deadline')
                self.sock.settimeout(max(.001, deadline - time.monotonic()))
                part = self.sock.recv(65536); require(part, 'EOF with request result unknown; never replay')
                self.buffer += part; require(len(self.buffer) <= MAX_FRAME, 'bounded response frame')
            line, self.buffer = self.buffer.split(b'\n', 1); response = json.loads(line)
            self.record.setdefault('responses', []).append(response)
            if response.get('id') == self.sequence:
                require('error' not in response, 'typed RPC rejection: ' + str(response.get('error')))
                return response['result']
        raise ValueError('bounded frame budget')
    def close(self): self.sock.close()

def package(path, expected_ref, new):
    require(path.is_absolute() and path.resolve() == path and path.is_dir(), 'canonical verified package required')
    manifest = json.loads((path / 'candidate.json').read_text())
    witness = json.loads((path / 'normal-build-witness.json').read_text())
    binary = path / 'bin/crewd'
    require(manifest['sourceRef'] == witness['sourceRef'] == expected_ref, 'original package sourceRef mismatch')
    require(manifest['binarySHA256'] == witness['binarySHA256'] == sha(binary), 'package binary mismatch')
    require(manifest['witnessSHA256'] == sha(path / 'normal-build-witness.json'), 'package witness mismatch')
    config = witness['buildConfiguration']
    require(config['target'] == 'x86_64-unknown-linux-gnu' and config['profile'] == 'release' and config['artifactProfile']['test'] is False, 'actual Linux normal Release required')
    require(config['defaultFeatures'] is True and config['allFeatures'] is False and config['features'] == [] and config['artifactFeatures'] == [] and not any(config['workspaceFeatures'].values()), 'default non-test configuration required')
    with binary.open('rb') as f: header = f.read(20)
    require(header[:6] == b'\x7fELF\x02\x01' and int.from_bytes(header[18:20], 'little') == 62, 'Linux x86_64 ELF required')
    rows = witness['files']; require(len(rows) == (393 if new else 374), 'complete input count mismatch')
    require(len({row['path'] for row in rows}) == len(rows), 'duplicate source rows')
    evidence = pathlib.PurePosixPath(witness['evidenceDirectory'])
    snapshot = pathlib.PurePosixPath(witness['snapshotDirectory'])
    require(not evidence.is_absolute() and '..' not in evidence.parts and not snapshot.is_absolute() and '..' not in snapshot.parts, 'proof path escape')
    for row in rows:
        relative = pathlib.PurePosixPath(row['path'])
        require(not relative.is_absolute() and '..' not in relative.parts and row['path'].startswith('crewd/'), 'input path escape')
        source = path / evidence / snapshot / relative
        require(source.resolve() == source and stat.S_ISREG(source.lstat().st_mode) and sha(source) == row['sha256'], 'original package source changed')
    source = path / evidence / snapshot / 'crewd/crates/store/src/schema.rs'
    versions = re.findall(r'^pub const SCHEMA_VERSION: i64 = ([0-9]+);$', source.read_text(), re.M)
    require(versions == [str(14 if new else 13)], 'actual source schema mismatch')
    digest = hashlib.sha256(''.join(row['path'] + '\t' + row['sha256'] + '\n' for row in sorted(rows, key=lambda x: x['path'])).encode()).hexdigest()
    require(digest == witness['rustInputDigest'], 'complete Rust input digest mismatch')
    if new: require(digest == DIGEST393 and witness.get('freshWorkspaceArtifacts') is True, 'certified393 fresh normal required')
    else: require(sha(binary) == OLD_BINARY and digest == 'ddecfaf231a2f141e95c6feedc8adb8bd3fba183953eb92a0b8254d621ef643c', 'accepted actual old13 binary/full input required')
    return {'path': str(path), 'sourceRef': expected_ref, 'binarySHA256': sha(binary), 'witnessSHA256': sha(path / 'normal-build-witness.json'), 'candidateSHA256': sha(path / 'candidate.json'), 'rustInputDigest': digest}

def main():
    parser = argparse.ArgumentParser()
    for name in ('old-package', 'new-package', 'expected-source-ref', 'managed-module', 'managed-module-sha256', 'out'): parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    require(sys.platform == 'linux' and os.getuid() == os.geteuid(), 'same UID Linux execution only')
    require(re.fullmatch('[0-9a-f]{40}', args.expected_source_ref), 'explicit immutable sourceRef')
    out = pathlib.Path(args.out)
    require(out.is_absolute() and out.parent.resolve() == out.parent and not os.path.lexists(out), 'fresh canonical short root required')
    require(re.fullmatch(r'/[A-Za-z0-9_./-]+', str(out)) and '..' not in out.parts, 'controlled shell-safe explicit root')
    parent = out.parent.lstat()
    require(stat.S_ISDIR(parent.st_mode) and parent.st_uid == os.geteuid() and stat.S_IMODE(parent.st_mode) == 0o700, 'owned0700 parent required')
    require(len(str(out / 'flow/unknown/crewd.sock').encode()) < 104, 'Unix socket path budget')
    module = pathlib.Path(args.managed_module)
    require(module.resolve() == module and stat.S_ISREG(module.lstat().st_mode) and sha(module) == args.managed_module_sha256, 'reviewed managed module mismatch')
    old = package(pathlib.Path(args.old_package), OLD_REF, False)
    new = package(pathlib.Path(args.new_package), args.expected_source_ref, True)
    out.mkdir(mode=0o700); flow = out / 'flow'; flow.mkdir(mode=0o700)
    result = {'pass': False, 'sourceRef': args.expected_source_ref, 'fixtureSHA256': sha(pathlib.Path(__file__).resolve()), 'managedModuleSHA256': sha(module), 'oldPackage': old, 'newPackage': new, 'events': [], 'cleanup': [], 'claims': {'stableBackupDDLRollbackRerun': False, 'installerCAS': False, 'AppSDKModel': False, 'sourceCompilation': False}}
    owned = []; sockets = []; started = time.monotonic()
    def event(label, **values):
        row = {'label': label, **values}; result['events'].append(row)
        write_json(flow / ('%02d-%s.json' % (len(result['events']), label)), row)
    prefix = out / 'runtime-prefix'
    prefix.mkdir(mode=0o700)
    for name in ('releases', 'activations', '.data-leases'): (prefix / name).mkdir(mode=0o700)
    (prefix / '.runtime.lock').touch(mode=0o600)
    for name, info in (('old', old), ('new', new)):
        shutil.copytree(info['path'], prefix / 'releases' / name)
        activation = prefix / 'activations' / name; activation.mkdir(mode=0o700)
        (activation / 'release').symlink_to('../../releases/' + name)
    def activate(name):
        current = prefix / 'current'
        if current.is_symlink(): current.unlink()
        current.symlink_to('activations/' + name)
        event('activate-' + name, prefix=str(prefix), actualPackage=str(current.resolve() / 'release'), QASelectionOnly=True)
    for name in ('home', 'tmp', 'work'): (flow / name).mkdir(mode=0o700)
    env = {'PATH': '/usr/bin:/bin', 'HOME': str(flow / 'home'), 'TMPDIR': str(flow / 'tmp'), 'XDG_CONFIG_HOME': str(flow / 'home'), 'XDG_CACHE_HOME': str(flow / 'tmp'), 'SHELL': '/usr/bin/false', 'PYTHONDONTWRITEBYTECODE': '1'}
    marker = flow / 'provider-executions.log'
    provider = flow / 'deny-provider'
    provider.write_text('#!/bin/sh\nprintf "owned-deny-provider\\n" >> ' + str(marker) + '\nexit 111\n'); provider.chmod(0o700)
    launcher = flow / 'managed-launcher.py'
    launcher.write_text('import sys\nsys.dont_write_bytecode=True\nimport importlib.util,pathlib\ns=importlib.util.spec_from_file_location("managed",sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)\ntry:m.main(pathlib.Path(sys.argv[2]),sys.argv[3:])\nexcept Exception as e:raise SystemExit("REFUSED managed launch: "+str(e))\n')
    def command(data): return [sys.executable, '-B', str(launcher), str(module), str(prefix), '--data-dir', str(data), 'serve', '--codex', str(provider), '--claude', str(provider)]
    def spawn(label, data):
        require(time.monotonic() - started < 150, 'whole runtime budget')
        log = (flow / (label + '.log')).open('wb')
        p = subprocess.Popen(command(data), env=env, stdout=log, stderr=log, start_new_session=True)
        owned.append((p, log)); captured = birth(p.pid)
        require(captured['ppid'] == os.getpid() and captured['pgid'] == p.pid and captured['uid'] == os.geteuid(), 'actual child ownership unknown')
        record = {'label': label, 'birth': captured, 'argv': command(data), 'preparedDirectory': str(flow), 'runtimeDeadline': started + 150}; result['events'].append(record)
        return p, captured, record
    def start(label, data):
        p, captured, record = spawn(label, data); deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            require(p.poll() is None, 'managed startup refused unexpectedly ' + label)
            sock = socket.socket(socket.AF_UNIX); sock.settimeout(.5)
            try: sock.connect(str(data / 'crewd.sock'))
            except OSError: sock.close(); time.sleep(.025); continue
            public = Public(sock, p, captured, record); sockets.append(public); return p, public, record
        raise ValueError('startup finite readiness budget')
    def stop(handle, crash=False):
        p, public, record = handle; public.close()
        require(p.poll() is None and same(record['birth'], birth(p.pid)), 'owned daemon changed before signal')
        p.send_signal(signal.SIGKILL if crash else signal.SIGTERM); p.wait(timeout=10)
        require(p.returncode == (-9 if crash else 0), 'actual daemon termination mismatch')
        record['exit'] = p.returncode; record['reaped'] = True
    def refusal(label, data):
        before = physical(data); p, captured, record = spawn(label, data)
        p.wait(timeout=10); record.update(exit=p.returncode, reaped=True)
        require(p.returncode != 0 and not os.path.lexists(data / 'crewd.sock'), 'Unknown/guard admitted public socket')
        require(physical(data) == before, 'refusal changed protected original physical data')
        require(marker.read_bytes() == marker_baseline, 'refusal dispatched provider')
        event(label + '-protected', physical=before, noSocket=True, calibratedProviderUnchanged=True)
    try:
        calibration = subprocess.run([str(provider)], env=env, capture_output=True, timeout=3)
        require(calibration.returncode == 111, 'own deny-provider calibration')
        marker_baseline = marker.read_bytes(); require(marker_baseline == b'owned-deny-provider\n', 'capture baseline mismatch')
        data = flow / 'data'; data.mkdir(mode=0o700); activate('old')
        h = start('real13-original', data)
        crew = h[1].call('crew.create', {'title': 'Release393 narrow migration', 'workingDirectory': str(flow / 'work')})['id']
        params = {'crewId': crew, 'requestId': str(uuid.uuid4()), 'text': 'durable real13 witness'}
        write_json(flow / 'prepared-resource-request.json', params)
        requirement = h[1].call('todo.create', params)['requirementId']; stop(h)
        reg = registry(data); require(reg.get('absent') is True or reg.get('entries') == {}, 'real13 registry not explicitly empty')
        require(int.from_bytes((data / 'crewd.db').read_bytes()[60:64], 'big') == 13, 'actual old normal did not create schema13')
        event('stopped-real13', requirementId=requirement, requestId=params['requestId'], registry=reg, physical=physical(data))
        activate('new'); h = start('real13-through-current393-startup', data)
        require(h[1].call('todo.create', params)['requirementId'] == requirement, 'migration lost original resource identity'); stop(h)
        require(int.from_bytes((data / 'crewd.db').read_bytes()[60:64], 'big') == 14, 'actual14 migration absent')
        h = start('actual393-duplicate-reopen', data)
        require(h[1].call('todo.create', params)['requirementId'] == requirement, 'reopen duplicate changed identity')
        crash_params = {'crewId': crew, 'requestId': str(uuid.uuid4()), 'text': 'committed before actual SIGKILL'}
        write_json(flow / 'prepared-crash-request.json', crash_params)
        crash_id = h[1].call('todo.create', crash_params)['requirementId']; stop(h, True)
        crash = physical(data); require(crash.get('crewd.db-wal', {}).get('size', 0) > 32, 'real nonempty crash WAL absent')
        saved = flow / 'crash-copy'; copy_data(data, saved)
        event('actual-nonempty-WAL', physical=crash, committedRequirementId=crash_id)
        h = start('certified393-managed-WAL-recovery', data)
        require(h[1].call('todo.create', crash_params)['requirementId'] == crash_id, 'crash committed identity lost'); stop(h)
        for name in ('bad-wal', 'future-main'):
            target = flow / name; copy_data(saved, target)
            path = target / ('crewd.db-wal' if name == 'bad-wal' else 'crewd.db')
            body = bytearray(path.read_bytes())
            if name == 'bad-wal': body[0] ^= 255
            else: body[60:64] = (15).to_bytes(4, 'big')
            path.write_bytes(body); refusal(name, target)
        unknown = flow / 'unknown'; copy_data(data, unknown)
        child = subprocess.Popen(['/bin/sleep', '45'], env=env, start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        owned.append((child, None)); identity = birth(child.pid)
        require(identity['ppid'] == os.getpid() and identity['pgid'] == child.pid, 'unknown control child ownership')
        write_json(unknown / 'exec-registry.json', {'isolated-legacy': {'pid': child.pid, 'pgid': identity['pgid'], 'startTime': identity['startTime'], 'bootId': '', 'command': 'isolated-control'}})
        refusal('legacy-empty-boot-Unknown', unknown)
        require(child.poll() is None and same(identity, birth(child.pid)), 'Unknown obligation was signalled/retired')
        require(marker.read_bytes() == marker_baseline, 'unexpected provider dispatch/replay')
        event('Unknown-original-birth-preserved', birth=identity, registry=registry(unknown), noProviderReplay=True)
        result['pass'] = True
    except Exception as error: result['error'] = {'type': type(error).__name__, 'message': str(error)}
    finally:
        for public in sockets:
            try: public.close()
            except Exception: pass
        for p, log in reversed(owned):
            row = {'pid': p.pid}
            try:
                if p.poll() is None:
                    p.terminate()
                    try: p.wait(timeout=5)
                    except subprocess.TimeoutExpired: p.kill(); p.wait(timeout=5)
                row.update(exit=p.returncode, reaped=p.poll() is not None)
            except Exception as error: row['error'] = str(error); result['pass'] = False
            finally:
                if log: log.close()
            result['cleanup'].append(row)
        try:
            require(sha(module) == args.managed_module_sha256, 'managed source changed during run')
            require(package(pathlib.Path(args.old_package), OLD_REF, False) == old and package(pathlib.Path(args.new_package), args.expected_source_ref, True) == new, 'original package input changed')
            result['specialInodes'] = []
            for path in flow.rglob('*'):
                st = path.lstat()
                if not stat.S_ISREG(st.st_mode) and not stat.S_ISDIR(st.st_mode): result['specialInodes'].append({'path': str(path.relative_to(flow)), 'mode': st.st_mode, 'inode': st.st_ino, 'device': st.st_dev, 'mtime_ns': st.st_mtime_ns, 'ctime_ns': st.st_ctime_ns})
            result['regularEvidence'] = {str(p.relative_to(flow)): sha(p) for p in flow.rglob('*') if stat.S_ISREG(p.lstat().st_mode)}
        except Exception as error: result['completionError'] = str(error); result['pass'] = False
        result['durationSeconds'] = time.monotonic() - started
        result['runtimePrefixRetainedNotArchived'] = str(prefix)
        result['socketNodesNotDeleted'] = True
        write_json(flow / 'result.json', result)
    print(json.dumps({'pass': result['pass'], 'result': str(flow / 'result.json')}))
    return 0 if result['pass'] else 1

if __name__ == '__main__':
    try: raise SystemExit(main())
    except Exception as error: raise SystemExit('REFUSED before runtime: ' + str(error))
