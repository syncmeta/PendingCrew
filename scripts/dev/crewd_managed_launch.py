"""Conservative managed-prefix SQLite launch gate. Never opens SQLite."""
import fcntl, hashlib, json, os, pathlib, re, stat, sys
sys.dont_write_bytecode = True
DB_COMMANDS = {'serve', 'backup', 'check', 'restore', 'import'}

def sha(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(1024 * 1024), b''): h.update(block)
    return h.hexdigest()

def private(path, directory=False):
    info = path.lstat()
    if path.resolve() != path or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != (0o700 if directory else 0o600):
        raise ValueError('canonical owner-private path required')
    if not (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)):
        raise ValueError('regular private file/directory required')
    return info

def classify(argv):
    if argv in [['--help'], ['--version'], ['-h'], ['-V'], ['schema']]: return None
    if len(argv) == 2 and argv[0] in DB_COMMANDS and argv[1] in ['--help', '-h']: return None
    if len(argv) < 3 or argv[0] != '--data-dir':
        raise ValueError('managed launch requires explicit --data-dir before a supported command')
    data = pathlib.Path(argv[1]); command = argv[2]; rest = argv[3:]
    if not data.is_absolute() or str(data) != argv[1] or any(c in argv[1] for c in ['\n', '\0']):
        raise ValueError('explicit canonical data path required')
    private(data, True)
    if command == 'rpc':
        if len(rest) not in [1, 2] or not rest[0] or rest[0].startswith('-'):
            raise ValueError('unsupported managed RPC arguments')
        return None
    if command not in DB_COMMANDS:
        # Standalone claude/codex can open StoreLedger. They cannot bypass this
        # gate merely because their argv does not look like `serve`.
        raise ValueError('unsupported managed command; no unguarded SQLite launch')
    values = {'serve': {'--codex', '--claude', '--codex-env', '--claude-env'}, 'import': {'--from'}, 'restore': {'--from'}}.get(command, set())
    seen = set(); i = 0
    while i < len(rest):
        option = rest[i]
        if command == 'check' and option == '--quick' and option not in seen:
            seen.add(option); i += 1; continue
        if option not in values or i + 1 == len(rest) or not rest[i + 1] or rest[i + 1].startswith('-'):
            raise ValueError('unknown managed database argument; no launch')
        if option in seen and option not in {'--codex-env', '--claude-env'}:
            raise ValueError('duplicate managed database argument')
        seen.add(option); i += 2
    if command == 'restore' and '--from' not in seen: raise ValueError('restore requires explicit source')
    return data

def supported_schema(package):
    candidate = json.loads((package / 'candidate.json').read_text())
    witness_path = package / 'normal-build-witness.json'
    if sha(witness_path) != candidate['witnessSHA256'] or sha(package / 'bin/crewd') != candidate['binarySHA256']:
        raise ValueError('managed retained package provenance changed')
    witness = json.loads(witness_path.read_text())
    if witness['sourceRef'] != candidate['sourceRef']: raise ValueError('managed package source mismatch')
    for field in ['evidenceDirectory', 'snapshotDirectory']:
        value = pathlib.PurePosixPath(witness[field])
        if value.is_absolute() or '..' in value.parts: raise ValueError('proof path escape')
    relative = 'crewd/crates/store/src/schema.rs'
    row = [r for r in witness['files'] if r['path'] == relative]
    source = package / witness['evidenceDirectory'] / witness['snapshotDirectory'] / relative
    if len(row) != 1 or source.is_symlink() or sha(source) != row[0]['sha256']:
        raise ValueError('supported schema lacks frozen package source proof')
    matches = re.findall(r'^pub const SCHEMA_VERSION: i64 = ([0-9]+);$', source.read_text(), re.M)
    if len(matches) != 1 or int(matches[0]) < 1: raise ValueError('unknown package schema authority')
    return int(matches[0])

def readonly_header(data, maximum):
    private(data, True)
    for suffix in ['.id', '-shm']:
        path = data / ('crewd.db' + suffix)
        if path.exists() or path.is_symlink(): private(path)
    for suffix in ['-wal', '-journal']:
        path = data / ('crewd.db' + suffix)
        if path.exists() or path.is_symlink():
            info = private(path)
            if info.st_size: raise ValueError('managed database has pending WAL/journal; preserve files and use authoritative newer recovery')
    db = data / 'crewd.db'
    if not db.exists() and not db.is_symlink(): return
    private(db)
    fd = os.open(db, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        before = os.fstat(fd); header = os.read(fd, 100); after = os.fstat(fd)
        if (before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_ino, after.st_size, after.st_mtime_ns):
            raise ValueError('database changed during managed preflight')
    finally: os.close(fd)
    if len(header) != 100 or header[:16] != b'SQLite format 3\0' or header[18] not in [1, 2] or header[19] not in [1, 2]:
        raise ValueError('unknown database header; preserve original files')
    size = int.from_bytes(header[16:18], 'big'); size = 65536 if size == 1 else size
    if size < 512 or size > 65536 or size & (size - 1) or before.st_size % size:
        raise ValueError('unknown database page layout')
    version = int.from_bytes(header[60:64], 'big')
    if version > maximum:
        raise ValueError('managed.future_schema: database version %d exceeds supported %d; old program not launched' % (version, maximum))
    if int.from_bytes(header[68:72], 'big') != 1129469764 or version < 1:
        raise ValueError('unknown database identity/version; preserve original files')

def main(prefix, argv):
    if os.getuid() != os.geteuid(): raise ValueError('elevated mixed-UID launch unsupported')
    private(prefix, True)
    private(prefix / '.runtime.lock')
    runtime = os.open(prefix / '.runtime.lock', os.O_RDWR | os.O_NOFOLLOW)
    if os.fstat(runtime).st_uid != os.geteuid() or stat.S_IMODE(os.fstat(runtime).st_mode) != 0o600:
        raise ValueError('private runtime lease required')
    fcntl.flock(runtime, fcntl.LOCK_SH | fcntl.LOCK_NB); os.set_inheritable(runtime, True)
    package = (prefix / 'current/release').resolve()
    if package.parent != prefix / 'releases': raise ValueError('managed activation escape')
    data = classify(argv)
    if data is not None:
        locks = prefix / '.data-leases'; private(locks, True)
        name = hashlib.sha256(str(data).encode()).hexdigest()
        fd = os.open(locks / name, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        private(locks / name)
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB); os.set_inheritable(fd, True)
        readonly_header(data, supported_schema(package))
    os.execv(str(package / 'bin/crewd'), [str(package / 'bin/crewd'), *argv])
