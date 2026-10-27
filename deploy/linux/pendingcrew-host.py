#!/usr/bin/python3 -I
"""Typed local root administration and Linux installation. No provider inference/login."""
import argparse, base64, contextlib, fcntl, hashlib, json, os, pathlib, platform, pwd, re, secrets, shutil, socket, stat, subprocess, sys, tarfile, tempfile, time, urllib.parse, urllib.request, uuid
sys.dont_write_bytecode = True
DEFAULT_MANIFEST_URL = None  # Filled by the immutable release entry renderer.
DEFAULT_MANIFEST_SHA256 = None
PREFIX = pathlib.Path('/opt/pendingcrew')
STATE = pathlib.Path('/var/lib/pendingcrew-host')
ADMIN_SOCKET = pathlib.Path('/run/pendingcrew/admin.sock')
SERVICE = 'pendingcrew.service'
SOCKET_UNIT = 'pendingcrew.socket'
FRAME_LIMIT = 262144
# Also read as a literal by the assembler. These are installation prerequisites,
# not full protocol, model access, or runtime qualification.
PROVIDER_POLICY = {
    'codex': {'versionPattern': r'codex-cli [0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?',
              'interfaceArgs': ['app-server', '--help'],
              'interfaceUsage': r'^Usage:\s+codex app-server(?:\s|$)',
              'loginArgs': ['login', 'status']},
    'claude': {'exactVersion': '2.1.301 (Claude Code)', 'loginArgs': ['auth', 'status']},
}
GUIDANCE = '''Install and log in yourself as the selected ordinary service user, then rerun:
Codex: curl -fsSL https://chatgpt.com/codex/install.sh | sh
       codex login --device-auth
       https://learn.chatgpt.com/docs/auth
Claude: curl -fsSL https://claude.ai/install.sh | bash
        claude auth login
        https://code.claude.com/docs/en/authentication
Claude managed tasks require exactly 2.1.301; do not downgrade automatically.
These are installation prerequisites, not a model-access or full protocol test.
No tool installation, login, credential copy or task is performed by PendingCrew.'''
class Refused(Exception): pass

def require_root():
    if platform.system() != 'Linux' or os.getuid() != 0 or os.geteuid() != 0:
        raise Refused('Linux root service management is required; no work Human privilege is inferred')
def sha(data): return hashlib.sha256(data).hexdigest()
def checked_text(value, maximum=4096):
    if not isinstance(value, str) or not value or len(value.encode()) > maximum or value.strip() != value or any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise Refused('invalid bounded metadata')
    return value
def canonical_uuid(value):
    if not isinstance(value, str) or str(uuid.UUID(value)) != value:
        raise Refused('canonical original UUID required')
    return value

def private_root(path, create=False):
    path = pathlib.Path(path)
    if create: path.mkdir(mode=0o700, parents=False, exist_ok=True)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or stat.S_IMODE(info.st_mode) != 0o700:
        raise Refused('root-private directory required')
    return path
def syncdir(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try: os.fsync(fd)
    finally: os.close(fd)
def new_record(path, value):
    private_root(path.parent)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    with os.fdopen(fd, 'w') as stream:
        json.dump(value, stream, separators=(',', ':')); stream.write('\n'); stream.flush(); os.fsync(stream.fileno())
    syncdir(path.parent)
def read_record(path):
    private_root(path.parent)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(fd, 'r') as stream:
        info = os.fstat(stream.fileno())
        if info.st_uid != 0 or stat.S_IMODE(info.st_mode) != 0o600 or not stat.S_ISREG(info.st_mode) or info.st_size > FRAME_LIMIT:
            raise Refused('root0600 operation record required')
        return json.load(stream)
def rpc(method, params, socket_path=ADMIN_SOCKET, timeout=10):
    # Closed callers below choose method names; this function is not a user RPC CLI.
    info = socket_path.lstat()
    if not stat.S_ISSOCK(info.st_mode) or info.st_uid != 0 or stat.S_IMODE(info.st_mode) != 0o600:
        raise Refused('root0600 admin socket required')
    request = json.dumps({'id': 1, 'method': method, 'params': params}, separators=(',', ':')).encode() + b'\n'
    if len(request) > FRAME_LIMIT: raise Refused('admin frame bound')
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(timeout); connection.connect(str(socket_path)); connection.sendall(request)
        with connection.makefile('rb') as reader: line = reader.readline(FRAME_LIMIT + 1)
    if not line.endswith(b'\n') or len(line) > FRAME_LIMIT: raise Refused('result unknown; retain original operation and query it, never resend automatically')
    result = json.loads(line)
    if result.get('id') != 1 or ('result' in result) == ('error' in result): raise Refused('original result unknown')
    if 'error' in result: raise Refused('typed host operation refused: ' + str(result['error'].get('code', 'unknown')))
    return result['result']
def enrolled_link(record, created):
    p = record['params']
    if created['requestId'] != p['requestId'] or created['enrollmentId'] != p['enrollmentId'] or created['endpoint'] != p['endpoint']:
        raise Refused('original creation result identity mismatch')
    link = {'version': 1, 'enrollmentId': p['enrollmentId'], 'server': created['server'], 'endpoint': created['endpoint'], 'expiresAt': created['expiresAt'], 'computerLabel': created['computerLabel'], 'token': record['token']}
    encoded = base64.urlsafe_b64encode(json.dumps(link, separators=(',', ':'), ensure_ascii=False).encode()).decode().rstrip('=')
    uri = 'pendingcrew://enroll/v1/' + encoded
    if len(uri.encode()) > 8192: raise Refused('enrollment URI exceeds bound')
    return uri

def pair_record(label, host, port, request_id=None, enrollment_id=None):
    if not 1 <= port <= 65535: raise Refused('explicit valid TLS port required')
    token_bytes = secrets.token_bytes(32)
    return {'format': 1, 'kind': 'enrollmentCreation', 'token': base64.urlsafe_b64encode(token_bytes).decode().rstrip('='), 'params': {'requestId': request_id or str(uuid.uuid4()), 'enrollmentId': enrollment_id or str(uuid.uuid4()), 'tokenHash': sha(token_bytes), 'computerLabel': checked_text(label, 256), 'endpoint': {'host': checked_text(host, 255), 'port': port}}}

def pair(args):
    path = pathlib.Path(args.operation_file)
    record = pair_record(args.label, args.host, args.port)
    # Save secret and UUIDs before any request; an existing record is never replaced.
    new_record(path, record)
    created = rpc('host.enrollmentCreate', record['params'])
    print(enrolled_link(record, created))  # Deliberate terminal only; never argv/journal/cloud.

def recover(args):
    record = read_record(pathlib.Path(args.operation_file))
    if record.get('format') != 1 or record.get('kind') != 'enrollmentCreation': raise Refused('wrong operation record')
    p = record['params']; result = rpc('host.enrollmentLookup', {'requestId': p['requestId'], 'enrollmentId': p['enrollmentId']})
    if result.get('state') != 'created': raise Refused('original creation not recorded; no automatic replay or new token')
    print(enrolled_link(record, result['created']))

def transient(uid, gid, argv, timeout=20, runner=subprocess.run):
    # systemd itself supplies the real User login environment. No Environment=,
    # HOME/PATH/SHELL/CODEX_HOME override or credential copy is permitted here.
    command = ['/usr/bin/systemd-run', '--quiet', '--wait', '--pipe', '--collect', '--service-type=exec', '--uid=' + str(uid), '--gid=' + str(gid), '--', *argv]
    return runner(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout, check=False)
def service_user(name):
    if name is None:
        uid = os.environ.get('SUDO_UID')
        if uid and uid.isdecimal() and int(uid) != 0: account = pwd.getpwuid(int(uid))
        else: raise Refused('select the existing ordinary user with --user; install/login the selected tools as that user first\n' + GUIDANCE)
    else: account = pwd.getpwnam(name)
    if account.pw_uid == 0 or not pathlib.Path(account.pw_dir).is_dir() or account.pw_shell.endswith(('nologin', '/false')):
        raise Refused('an existing ordinary login user is required; no credentials are imported')
    return account

def provider_output(result):
    # Auth status and broken wrappers can contain credentials. Never echo output.
    if result.returncode != 0 or len(result.stdout) > 65536:
        raise Refused('command failed or returned excessive output')
    return result.stdout.decode('utf-8', errors='strict').strip()

def provider_preflight(account, supplied, runner=subprocess.run, disable_claude=False):
    if disable_claude and supplied.get('claude') is not None:
        raise Refused('--claude conflicts with --disable-claude')
    tools, failures = {}, []
    for name, policy in PROVIDER_POLICY.items():
        if name == 'claude' and disable_claude: continue
        try:
            path = supplied.get(name)
            if path is None:
                located = transient(account.pw_uid, account.pw_gid, ['/bin/sh', '-c', 'command -v "$1"', 'sh', name], runner=runner)
                if located.returncode == 0:
                    path = provider_output(located)
                else:
                    candidate = pathlib.Path(account.pw_dir) / '.local/bin' / name
                    tested = transient(account.pw_uid, account.pw_gid, ['/usr/bin/test', '-x', str(candidate)], runner=runner)
                    if tested.returncode == 0: path = str(candidate)
            if path is None or not pathlib.Path(path).is_absolute():
                raise Refused('not found; install it as the selected service user')
            path = str(pathlib.Path(checked_text(path)).resolve(strict=True))
            version = provider_output(transient(account.pw_uid, account.pw_gid, [path, '--version'], runner=runner))
            if 'exactVersion' in policy:
                if version != policy['exactVersion']:
                    raise Refused('managed task version must be ' + policy['exactVersion'])
            elif re.fullmatch(policy['versionPattern'], version) is None:
                raise Refused('unrecognized CLI identity/version')
            if 'interfaceArgs' in policy:
                help_text = provider_output(transient(account.pw_uid, account.pw_gid, [path, *policy['interfaceArgs']], runner=runner))
                if re.search(policy['interfaceUsage'], help_text, re.MULTILINE) is None:
                    raise Refused('app-server command help is not supported')
            status = transient(account.pw_uid, account.pw_gid, [path, *policy['loginArgs']], runner=runner)
            if status.returncode != 0:
                raise Refused('not logged in/configured for the selected service user')
            tools[name] = path
        except Refused as failure:
            failures.append(name + ': ' + str(failure))
        except (OSError, ValueError, subprocess.SubprocessError):
            failures.append(name + ': prerequisite check failed or timed out in the real service context')
    if failures:
        guidance = GUIDANCE if not disable_claude else ('Install and log in yourself as the selected ordinary service user, then rerun:\nCodex: curl -fsSL https://chatgpt.com/codex/install.sh | sh\n       codex login --device-auth\n       https://learn.chatgpt.com/docs/auth\nClaude is explicitly disabled; it need not be installed or logged in.\nNo installation, login, credential copy or task is performed. This is not a model-access test.')
        raise Refused('Installation prerequisites not met:\n' + '\n'.join(failures) + '\n' + guidance)
    return tools

def validate_provider_policy(manifest):
    if manifest.get('providerPrerequisites') != PROVIDER_POLICY:
        raise Refused('release installation prerequisites do not match this installer')


def download(url, expected, limit):
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != 'https' or not parsed.hostname or parsed.username or parsed.password or parsed.fragment or not re.fullmatch('[0-9a-f]{64}', expected or ''):
        raise Refused('immutable HTTPS input and exact SHA256 required')
    with urllib.request.urlopen(url, timeout=20) as response:
        if urllib.parse.urlsplit(response.url).scheme != 'https': raise Refused('insecure release redirect refused')
        data = response.read(limit + 1)
    if len(data) > limit or sha(data) != expected: raise Refused('release input digest or bound mismatch')
    return data

def extract_release(payload, root, expected):
    archive = root / 'release.tar'; archive.write_bytes(payload)
    unpacked = root / 'unpacked'; unpacked.mkdir(mode=0o755); unpacked.chmod(0o755)
    with tarfile.open(archive, 'r:*') as bundle:
        members = bundle.getmembers()
        if len(members) > 512 or sum(m.size for m in members) > 1024 * 1024 * 1024: raise Refused('release archive bound')
        for member in members:
            path = pathlib.PurePosixPath(member.name)
            if path.is_absolute() or '..' in path.parts or not path.parts or len(path.parts) > 8 or not (member.isfile() or member.isdir()): raise Refused('unsafe release archive member')
            target = unpacked.joinpath(*path.parts)
            if member.isdir(): target.mkdir(mode=0o755, parents=True, exist_ok=True)
            else:
                target.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
                with bundle.extractfile(member) as source, target.open('xb') as destination: shutil.copyfileobj(source, destination)
                target.chmod(0o555 if path.parts[0] == 'bin' else 0o444)
    for directory in unpacked.rglob('*'):
        if directory.is_dir(): directory.chmod(0o755)
    release = json.loads((unpacked / 'release.json').read_bytes())
    if release.get('format') != 1 or release.get('sourceCommit') != expected['sourceCommit'] or release.get('architecture') != expected['architecture'] or release.get('storeVersion') != 21 or release.get('apiVersion') != 3:
        raise Refused('wrong release source/architecture/schema')
    files = release.get('files', {})
    actual = {str(p.relative_to(unpacked)) for p in unpacked.rglob('*') if p.is_file()}
    if actual != set(files) | {'release.json'} or not {'bin/crewd', 'bin/pendingcrew-host.py', 'bin/managed-host.py', 'build-proof.json'} <= actual:
        raise Refused('release file manifest mismatch')
    for name, digest in files.items():
        if sha((unpacked / name).read_bytes()) != digest: raise Refused('release member digest mismatch')
    proof = json.loads((unpacked / 'build-proof.json').read_bytes())
    core_source = release.get('coreSourceCommit', release['sourceCommit'])
    if not isinstance(core_source, str) or not re.fullmatch('[0-9a-f]{40}', core_source) or core_source != expected.get('coreSourceCommit', expected['sourceCommit']):
        raise Refused('Core component source mismatch')
    if proof.get('sourceCommit') != core_source or proof.get('sourceClean') is not True or proof.get('defaultFeatures') is not True or proof.get('features') != [] or proof.get('test') is not False or proof.get('binarySha256') != files['bin/crewd']:
        raise Refused('ordinary binary build proof mismatch')
    header = (unpacked / 'bin/crewd').read_bytes()[:20]
    machine = {'x86_64': 62, 'aarch64': 183}.get(release['architecture'])
    if len(header) < 20 or header[:6] != b'\x7fELF\x02\x01' or int.from_bytes(header[18:20], 'little') != machine:
        raise Refused('wrong ELF architecture')
    return unpacked, release

def root_code_directory(path, create=False):
    if create:
        try: path.mkdir(mode=0o755, parents=False)
        except FileExistsError: pass
        else: path.chmod(0o755)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022 or path.is_symlink(): raise Refused('root-owned non-writable code directory required')

def pointer(prefix):
    link = prefix / 'current'
    if not link.exists() and not link.is_symlink(): return None
    if not link.is_symlink(): raise Refused('current must be a controlled activation link')
    value = os.readlink(link)
    if not re.fullmatch(r'activations/[0-9a-f-]{36}', value): raise Refused('activation pointer malformed')
    return value

def unit_quote(value):
    checked_text(value)
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%') + '"'

def units(account, data, tools, disable_claude=False):
    executable = PREFIX / 'current/bin/managed-host.py'
    command = ['/usr/bin/python3', '-I', str(executable), '--prefix', str(PREFIX), '--data-dir', str(data), '--codex', tools['codex'], '--system-admin-socket', str(ADMIN_SOCKET)]
    command += ['--disable-claude'] if disable_claude else ['--claude', tools['claude']]
    socket_unit = '''[Unit]
Description=PendingCrew root-only typed management socket
[Socket]
ListenStream=/run/pendingcrew/admin.sock
SocketUser=root
SocketGroup=root
SocketMode=0600
DirectoryMode=0700
FileDescriptorName=pcw-admin
RemoveOnStop=yes
[Install]
WantedBy=sockets.target
'''
    service_unit = '''[Unit]
Description=PendingCrew background Core
Requires=pendingcrew.socket
After=pendingcrew.socket network.target
[Service]
Type=exec
User={uid}
Group={gid}
WorkingDirectory={home}
UMask=0077
ExecStart={command}
Restart=on-failure
RestartSec=3
TimeoutStopSec=150
KillMode=process
SendSIGKILL=no
[Install]
WantedBy=multi-user.target
'''.format(uid=account.pw_uid, gid=account.pw_gid,
           # WorkingDirectory is a path directive, not an ExecStart argument:
           # systemd retains surrounding quotes here as literal path bytes.
           home=checked_text(account.pw_dir).replace('%', '%%'),
           command=' '.join(unit_quote(x) for x in command))
    return socket_unit, service_unit

def replace_root_file(path, content, mode=0o644):
    if path.exists() or path.is_symlink():
        previous = path.lstat()
        if not stat.S_ISREG(previous.st_mode) or previous.st_uid != 0 or previous.st_mode & 0o022: raise Refused('refuse replacing non-root-owned configuration')
    temporary = path.with_name('.' + path.name + '-' + str(uuid.uuid4()))
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    os.fchmod(fd, mode)
    with os.fdopen(fd, 'wb') as stream: stream.write(content); stream.flush(); os.fsync(stream.fileno())
    os.replace(temporary, path); syncdir(path.parent)

def active():
    return subprocess.run(['/usr/bin/systemctl', 'is-active', '--quiet', SERVICE], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0

def create_user_data(data, account):
    home = pathlib.Path(account.pw_dir)
    if not data.is_absolute() or '..' in data.parts or home not in data.parents:
        raise Refused('data must be below the selected real login home without traversal')
    # Walk anchored descriptors, never a root mkdir/chown through a mutable user
    # symlink. Renaming a checked parent cannot redirect a later operation.
    fd = os.open('/', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for index, component in enumerate(data.parts[1:]):
            final = index == len(data.parts[1:]) - 1
            created = False
            try:
                child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            except FileNotFoundError:
                os.mkdir(component, 0o700, dir_fd=fd)
                child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                created = True
            try:
                if created: os.fchown(child, account.pw_uid, account.pw_gid)
                info = os.fstat(child)
                if info.st_uid not in (0, account.pw_uid) or info.st_mode & 0o022:
                    raise Refused('unsafe data directory ancestry')
                if final and (info.st_uid != account.pw_uid or stat.S_IMODE(info.st_mode) != 0o700):
                    raise Refused('private0700 service-user data required')
                if created: os.fsync(fd)
            except BaseException:
                os.close(child); raise
            os.close(fd); fd = child
    finally:
        os.close(fd)


def data_lease(data, uid, gid):
    directory = os.open(data, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        info = os.fstat(directory)
        if info.st_uid != uid or stat.S_IMODE(info.st_mode) != 0o700:
            raise Refused('original private service-user data required')
        try:
            fd = os.open('crewd.lock', os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=directory)
        except FileExistsError:
            fd = os.open('crewd.lock', os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=directory)
        else:
            os.fchown(fd, uid, gid); os.fchmod(fd, 0o600); os.fsync(directory)
    finally: os.close(directory)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != uid or info.st_mode & 0o022:
            raise Refused('original Core lease ownership mismatch')
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BaseException:
        os.close(fd); raise
    return fd


def parse_preserved_status(status, listen_host, port):
    # Input is an original typed remote.status/host.status result obtained from
    # its still-live management entry, not a new authority/proof format.
    if not isinstance(status, dict) or set(status) != {'identity','configuration','endpoint','listening'} or status['listening'] is not True:
        raise Refused('original enabled/listening status without failure is required')
    identity, configuration, endpoint = status['identity'], status['configuration'], status['endpoint']
    if not isinstance(identity, dict) or set(identity) != {'authorityId','dbId','certificateDerBase64','spkiSha256'}:
        raise Refused('complete original public identity required')
    canonical_uuid(identity['authorityId']); checked_text(identity['dbId'],128)
    if not isinstance(identity['spkiSha256'], str) or not re.fullmatch('[0-9a-f]{64}',identity['spkiSha256']):
        raise Refused('original TLS pin invalid')
    certificate = checked_text(identity['certificateDerBase64'],32768)
    try:
        if not base64.b64decode(certificate, validate=True): raise ValueError()
    except ValueError:
        raise Refused('original public certificate encoding invalid') from None
    if not isinstance(configuration, dict) or set(configuration) != {'revision','enabled','listenHost','port'}:
        raise Refused('complete original configuration required')
    revision = configuration['revision']
    if not isinstance(revision,str) or not re.fullmatch('[1-9][0-9]{0,19}',revision) or configuration['enabled'] is not True or type(configuration['port']) is not int:
        raise Refused('original enabled configuration invalid')
    if configuration['listenHost'] != listen_host or configuration['port'] != port or not 1 <= port <= 65535 or not isinstance(endpoint,dict) or type(endpoint.get('port')) is not int or endpoint != {'host':listen_host,'port':port}:
        raise Refused('original listener differs from explicit bind; preservation never reconfigures it')
    return status

def check_preserved_connection(original, status):
    saved = original['preservedConnection']
    parse_preserved_status(saved, original['listenHost'], original['port'])
    if status != saved:
        raise Refused('original identity/configuration/endpoint changed or listener failed; no reconfiguration or new enrollment')


def install(args):
    # All installation prerequisite checks finish before any persistent PendingCrew path/unit write.
    account = service_user(args.user)
    status_file = getattr(args, 'preserve_connection_from', None)
    preserved = parse_preserved_status(read_record(pathlib.Path(status_file)), args.listen_host, args.port) if status_file else None
    disable_claude = getattr(args, 'disable_claude', False)
    supplied = {'codex': args.codex, 'claude': args.claude}
    tools = provider_preflight(account, supplied, disable_claude=True) if disable_claude else provider_preflight(account, supplied)
    if active(): raise Refused('service is running; stop it safely first, then install. No process is killed or replaced')
    manifest_url = args.manifest_url or DEFAULT_MANIFEST_URL
    manifest_sha = args.manifest_sha256 or DEFAULT_MANIFEST_SHA256
    if not manifest_url or not manifest_sha: raise Refused('this source requires an immutable release entry; no current public Linux package is inferred')
    manifest = json.loads(download(manifest_url, manifest_sha, 1024 * 1024))
    architecture = {'x86_64': 'x86_64', 'aarch64': 'aarch64'}.get(platform.machine())
    artifact = manifest.get('artifact', {})
    if manifest.get('format') != 1 or artifact.get('architecture') != architecture or not re.fullmatch('[0-9a-f]{40}', artifact.get('sourceCommit', '')) or artifact['sourceCommit'] not in urllib.parse.urlsplit(artifact.get('url', '')).path:
        raise Refused('wrong immutable release manifest')
    validate_provider_policy(manifest)
    data = pathlib.Path(args.data_dir or (pathlib.Path(account.pw_dir) / '.local/share/PendingCrew')).absolute()
    checked_text(str(data))
    if len(os.fsencode(data / 'crewd.sock')) >= 108: raise Refused('data directory exceeds Unix socket address limit')
    if data.exists():
        info = data.lstat()
        if not stat.S_ISDIR(info.st_mode) or data.is_symlink() or info.st_uid != account.pw_uid or stat.S_IMODE(info.st_mode) != 0o700: raise Refused('existing data must be owned private0700 service-user data')
    if not 1 <= args.port <= 65535: raise Refused('explicit valid TLS port required')
    checked_text(args.advertise_host, 255); checked_text(args.listen_host, 255)
    import ipaddress
    ipaddress.ip_address(args.listen_host)
    if preserved is not None:
        if not data.is_dir(): raise Refused('preservation requires an existing private data directory')
        database = (data / 'crewd.db').lstat()
        if not stat.S_ISREG(database.st_mode) or database.st_uid != account.pw_uid or stat.S_IMODE(database.st_mode) != 0o600:
            raise Refused('preservation requires an existing service-user0600 database')
    with tempfile.TemporaryDirectory(prefix='pendingcrew-install-') as staging_name:
        staging = pathlib.Path(staging_name); staging.chmod(0o755)
        unpacked, release = extract_release(download(artifact['url'], artifact['sha256'], 512 * 1024 * 1024), staging, artifact)
        schema = transient(account.pw_uid, account.pw_gid, [str(unpacked / 'bin/crewd'), 'schema'])
        if schema.returncode != 0: raise Refused('ordinary artifact offline schema check failed before installation')
        schema_value = json.loads(schema.stdout)
        if not {'remote.enrollmentCreate', 'remote.enrollmentLookup'} <= set(schema_value.get('methods', {})):
            raise Refused('artifact has no current enrollment contract; never substitute an old R2 package')
        root_code_directory(PREFIX, create=True)
        for name in ['releases', 'activations']: root_code_directory(PREFIX / name, create=True)
        private_root(STATE, create=True)
        installation = canonical_uuid(args.request_id) if args.request_id else str(uuid.uuid4())
        binding = STATE / 'installation.json'
        if binding.exists() or binding.is_symlink():
            existing = read_record(binding)
            if existing.get('uid') != account.pw_uid or existing.get('gid') != account.pw_gid or existing.get('dataDir') != str(data):
                raise Refused('existing service user/data binding cannot be replaced by installation')
        prior = pointer(PREFIX)
        operation = STATE / ('install-' + installation + '.json')
        new_record(operation, {'format': 1, 'kind': 'installation', 'requestId': installation, 'expectedCurrent': prior, 'sourceCommit': release['sourceCommit'], 'artifactSha256': artifact['sha256'], 'uid': account.pw_uid, 'dataDir': str(data), 'gid': account.pw_gid, 'providerPaths': tools, 'disableClaude': disable_claude, 'preservedConnection': preserved, 'label': checked_text(args.label, 256), 'advertiseHost': args.advertise_host, 'listenHost': args.listen_host, 'port': args.port, 'configureRequestId': str(uuid.uuid4()), 'enrollmentRequestId': str(uuid.uuid4()), 'enrollmentId': str(uuid.uuid4())})
        with contextlib.ExitStack() as leases:
            for name, mode in [('.install.lock', 0o600), ('.runtime.lock', 0o644)]:
                try:
                    fd = os.open(PREFIX / name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, mode)
                except FileExistsError:
                    fd = os.open(PREFIX / name, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
                else:
                    os.fchmod(fd, mode)
                leases.callback(os.close, fd)
                info = os.fstat(fd)
                if info.st_uid != 0 or not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode) != mode: raise Refused('root installation/runtime lease mode mismatch')
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            if active() or pointer(PREFIX) != prior: raise Refused('installation CAS or active-runtime conflict; inspect original installation, never replay blindly')
            create_user_data(data, account)
            leases.callback(os.close, data_lease(data, account.pw_uid, account.pw_gid))
            destination = PREFIX / 'releases' / release['sourceCommit']
            if destination.exists() or destination.is_symlink():
                root_code_directory(destination)
                root_code_directory(destination / 'bin')
                for member in [destination / 'release.json', *(destination / name for name in release['files'])]:
                    info = member.lstat()
                    if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
                        raise Refused('existing immutable release ownership mismatch')
                if sha((destination / 'release.json').read_bytes()) != sha((unpacked / 'release.json').read_bytes()): raise Refused('immutable release already differs')
                for name, digest in release['files'].items():
                    if sha((destination / name).read_bytes()) != digest: raise Refused('existing release member changed')
            else:
                shutil.copytree(unpacked, destination); syncdir(destination); syncdir(destination.parent)
            activation = PREFIX / 'activations' / installation
            activation.mkdir(mode=0o755); activation.chmod(0o755)
            (activation / 'release').symlink_to('../../releases/' + release['sourceCommit'])
            (activation / 'bin').symlink_to('release/bin')
            # Transaction file is public provenance only; all secret operations live in STATE.
            replace_root_file(activation / 'transaction.json', json.dumps({'requestId': installation, 'previous': prior, 'sourceCommit': release['sourceCommit'], 'storeVersion': 21}).encode(), 0o444)
            syncdir(activation); syncdir(activation.parent)
            pending = PREFIX / ('.current-' + installation); pending.symlink_to('activations/' + installation); os.replace(pending, PREFIX / 'current'); syncdir(PREFIX)
            if pointer(PREFIX) != 'activations/' + installation: raise Refused('activation result unknown; inspect original request only')
            socket_unit, service_unit = units(account, data, tools, disable_claude=disable_claude)
            replace_root_file(pathlib.Path('/etc/systemd/system/pendingcrew.socket'), socket_unit.encode())
            replace_root_file(pathlib.Path('/etc/systemd/system/pendingcrew.service'), service_unit.encode())
            # Only this same service user/data binding may be updated by later installs.
            replace_root_file(STATE / 'installation.json', json.dumps({'format': 1, 'requestId': installation, 'uid': account.pw_uid, 'gid': account.pw_gid, 'dataDir': str(data), 'sourceCommit': release['sourceCommit']}).encode(), 0o600)
        finish_installation(read_record(operation))


def finish_installation(original):
    # Separate durable records precede each first mutation. On recovery an
    # existing record authorizes lookup only, never a second send.
    for command in [['/usr/bin/systemctl', 'daemon-reload'], ['/usr/bin/systemctl', 'enable', '--now', SOCKET_UNIT], ['/usr/bin/systemctl', 'enable', '--now', SERVICE]]:
        subprocess.run(command, check=True, timeout=30, stdout=subprocess.DEVNULL)
    deadline = time.monotonic() + 15
    status = None
    while time.monotonic() < deadline:
        try: status = rpc('host.status', {}); break
        except (OSError, ValueError, Refused): time.sleep(0.1)
    if status is None: raise Refused('activation recorded but service readiness unknown; inspect original installation, no automatic rollback or new enrollment')
    if original.get('preservedConnection') is not None:
        check_preserved_connection(original, status)
        grants = rpc('host.grants', {})  # Current readback, not proof of a prior grant list.
        print(json.dumps({'preservedConnection': original['preservedConnection'], 'currentGrants': grants}, indent=2))
        return
    configure_id = original['configureRequestId']
    configuration_file = STATE / ('configure-' + configure_id + '.json')
    if configuration_file.exists() or configuration_file.is_symlink():
        config_record = read_record(configuration_file)
        if config_record.get('kind') != 'configuration': raise Refused('wrong original configuration record')
        found = rpc('host.configurationLookup', config_record['params'])
        if found.get('state') != 'recorded': raise Refused('original configuration result not recorded; no automatic resend')
        configuration = found['configuration']
    else:
        params = {'requestId': configure_id, 'expectedRevision': status['configuration']['revision'], 'enabled': True, 'listenHost': original['listenHost'], 'port': original['port']}
        new_record(configuration_file, {'format': 1, 'kind': 'configuration', 'params': params})
        configuration = rpc('host.configure', params)
    status = rpc('host.status', {})
    if status['configuration'] != configuration or not status.get('listening'):
        raise Refused('original configuration no longer current/listening; no new enrollment')
    enrollment_file = STATE / ('enroll-' + original['enrollmentRequestId'] + '.json')
    if enrollment_file.exists() or enrollment_file.is_symlink():
        recover(argparse.Namespace(operation_file=str(enrollment_file)))
    else:
        record = pair_record(original['label'], original['advertiseHost'], original['port'], original['enrollmentRequestId'], original['enrollmentId'])
        new_record(enrollment_file, record)
        print(enrolled_link(record, rpc('host.enrollmentCreate', record['params'])))


def resume_installation(args):
    request = canonical_uuid(args.request_id)
    original = read_record(STATE / ('install-' + request + '.json'))
    if original.get('format') != 1 or original.get('kind') != 'installation' or original.get('requestId') != request:
        raise Refused('wrong original installation record')
    if pointer(PREFIX) != 'activations/' + request:
        raise Refused('original activation is not current; inspect original UUID, no automatic activation/replay')
    account = service_user(pwd.getpwuid(original['uid']).pw_name)
    if account.pw_gid != original['gid']: raise Refused('original service group changed')
    disable_claude = original.get('disableClaude', False)
    if not isinstance(disable_claude, bool) or set(original['providerPaths']) != ({'codex'} if disable_claude else {'codex','claude'}):
        raise Refused('original provider selection is invalid; no rediscovery')
    tools = provider_preflight(account, original['providerPaths'], disable_claude=True) if disable_claude else provider_preflight(account, original['providerPaths'])
    data = pathlib.Path(original['dataDir'])
    socket_unit, service_unit = units(account, data, tools, disable_claude=disable_claude)
    expected = [(pathlib.Path('/etc/systemd/system/pendingcrew.socket'), socket_unit.encode()), (pathlib.Path('/etc/systemd/system/pendingcrew.service'), service_unit.encode())]
    binding = STATE / 'installation.json'
    if binding.exists() or binding.is_symlink():
        current = read_record(binding)
        if current.get('uid') != original['uid'] or current.get('gid') != original['gid'] or current.get('dataDir') != original['dataDir']:
            raise Refused('original service binding changed')
    # Only missing continuation metadata is repaired; no package extraction,
    # activation pointer change, provider login, or task operation is repeated.
    if active():
        if any(not path.exists() or path.read_bytes() != content for path,content in expected):
            raise Refused('active service has different units; no replacement')
        if not binding.exists(): raise Refused('active service binding missing')
    else:
        with contextlib.ExitStack() as leases:
            for name in ['.install.lock', '.runtime.lock']:
                fd=os.open(PREFIX/name,os.O_RDWR|os.O_NOFOLLOW|os.O_CLOEXEC); leases.callback(os.close,fd)
                info=os.fstat(fd)
                if info.st_uid != 0 or not stat.S_ISREG(info.st_mode) or info.st_mode & 0o022: raise Refused('root-owned lease required')
                fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
            if active() or pointer(PREFIX) != 'activations/'+request: raise Refused('original activation changed during recovery')
            leases.callback(os.close,data_lease(data,original['uid'],original['gid']))
            for path,content in expected: replace_root_file(path,content)
            replace_root_file(binding,json.dumps({'format':1,'requestId':request,'uid':original['uid'],'gid':original['gid'],'dataDir':str(data),'sourceCommit':original['sourceCommit']}).encode(),0o600)
    finish_installation(original)


def stopped_database_version(data):
    # Never checkpoint, restore or reinterpret pending WAL during code rollback.
    for suffix in ['-wal', '-journal']:
        path = data / ('crewd.db' + suffix)
        if path.exists() or path.is_symlink():
            fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
            try:
                if os.fstat(fd).st_size: raise Refused('pending database recovery prevents rollback')
            finally: os.close(fd)
    path = data / 'crewd.db'
    if not path.exists(): return None
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode): raise Refused('regular Core database required')
        header = os.read(fd, 100)
    finally: os.close(fd)
    if len(header) != 100 or header[:16] != b'SQLite format 3\0': raise Refused('unrecognized database header')
    if int.from_bytes(header[68:72], 'big') != 0x43525744: raise Refused('database belongs to another application')
    return int.from_bytes(header[60:64], 'big')

def rollback(args):
    selected = canonical_uuid(args.request_id)
    expected = 'activations/' + canonical_uuid(args.expected_current)
    operation = canonical_uuid(args.operation_id)
    original = read_record(STATE / ('install-' + selected + '.json'))
    binding = read_record(STATE / 'installation.json')
    if original.get('format') != 1 or original.get('kind') != 'installation' or original.get('uid') != binding.get('uid') or original.get('dataDir') != binding.get('dataDir'):
        raise Refused('original installation/user/data binding mismatch')
    if active(): raise Refused('stop the service safely before rollback')
    if subprocess.run(['/usr/bin/systemctl', 'is-active', '--quiet', SOCKET_UNIT], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
        raise Refused('stop the activation socket before rollback')
    with contextlib.ExitStack() as leases:
        for name in ['.install.lock', '.runtime.lock']:
            fd = os.open(PREFIX / name, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
            leases.callback(os.close, fd)
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022: raise Refused('root-owned lease required')
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if active() or pointer(PREFIX) != expected: raise Refused('rollback current UUID or runtime conflict')
        target = PREFIX / 'activations' / selected
        root_code_directory(target)
        if os.readlink(target / 'release') != '../../releases/' + original['sourceCommit'] or os.readlink(target / 'bin') != 'release/bin':
            raise Refused('original immutable activation changed')
        release_dir = PREFIX / 'releases' / original['sourceCommit']
        root_code_directory(release_dir)
        release = json.loads((release_dir / 'release.json').read_bytes())
        for name, digest in release['files'].items():
            member = release_dir / name; info = member.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022 or sha(member.read_bytes()) != digest:
                raise Refused('rollback immutable release changed')
        leases.callback(os.close, data_lease(pathlib.Path(binding['dataDir']), binding['uid'], binding['gid']))
        version = stopped_database_version(pathlib.Path(binding['dataDir']))
        # This candidate has exactly one writer qualification. No guessed ranges
        # or downgrade migrations; the old writer must match the stopped format.
        if release.get('storeVersion') != 21 or version not in (None, 21):
            raise Refused('rollback target is not qualified for the stopped database')
        new_record(STATE / ('rollback-' + operation + '.json'), {'format':1, 'kind':'rollback', 'requestId':operation, 'expectedCurrent':expected, 'target':'activations/' + selected, 'databaseVersion':version})
        pending = PREFIX / ('.current-' + operation)
        pending.symlink_to('activations/' + selected)
        os.replace(pending, PREFIX / 'current'); syncdir(PREFIX)
        binding.update(requestId=selected, sourceCommit=original['sourceCommit'])
        replace_root_file(STATE / 'installation.json', json.dumps(binding).encode(), 0o600)
    print(json.dumps({'requestId':operation, 'current':pointer(PREFIX), 'serviceStarted':False}))


def notification_configure(args):
    # Secret material is only bounded stdin or a root-owned0600 regular file.
    # The durable CLI witness stores digest/UUID only; it can never replay a key.
    if args.input_file:
        fd = os.open(args.input_file, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        with os.fdopen(fd, 'rb') as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or stat.S_IMODE(info.st_mode) != 0o600:
                raise Refused('notification input requires root-owned0600 regular file')
            raw = stream.read(16385)
    else: raw = sys.stdin.buffer.read(16385)
    if not raw or len(raw) > 16384: raise Refused('bounded notification configuration input required')
    params = json.loads(raw)
    if not isinstance(params, dict) or set(params) - {'requestId','expectedRevision','enabled','provider'}:
        raise Refused('invalid typed notification configuration')
    request = canonical_uuid(params.get('requestId'))
    private_root(STATE, create=True)
    record = {'format':1,'kind':'notificationConfiguration','requestId':request,'payloadSha256':sha(json.dumps(params,sort_keys=True,separators=(',',':')).encode())}
    new_record(STATE / ('notification-' + request + '.json'), record)
    # Response loss is Unknown. This function never retries or replaces its UUID.
    print(json.dumps(rpc('host.notificationConfigure', params), indent=2))

def notification_lookup(args):
    request = canonical_uuid(args.request_id)
    record = read_record(STATE / ('notification-' + request + '.json'))
    if record.get('format') != 1 or record.get('kind') != 'notificationConfiguration' or record.get('requestId') != request:
        raise Refused('original notification operation witness required')
    print(json.dumps(rpc('host.notificationRequestLookup', {'requestId':request}), indent=2))


def draft_control(args):
    params = read_record(pathlib.Path(args.operation_file))
    if not isinstance(params, dict) or set(params) != {'requestId','dbId','runtimeBindingId','params'} or not isinstance(params['params'], dict):
        raise Refused('exact typed personal draft owner envelope required')
    request = canonical_uuid(params['requestId']); canonical_uuid(params['runtimeBindingId'])
    checked_text(params['dbId'],128)
    method, operation, keys = {
        'owner': ('host.personalDraftOwnerConfigure','personalDraft.ownerConfigure',{'ownerEpoch','expectedSpaceId','expectedSpaceRevision','localEnabled','acknowledgeRestore'}),
        'member': ('host.personalDraftMemberSet','personalDraft.memberSet',{'spaceId','spaceEpoch','expectedSpaceRevision','grantId','grantRevision','clientSpkiSha256','enabled'})
    }[args.kind]
    if set(params['params']) != keys: raise Refused('exact personal draft owner params required')
    if args.kind == 'owner' and params['params']['localEnabled'] is not False:
        raise Refused('root administration never becomes an App draft writer')
    original = {key:params[key] for key in ['requestId','dbId','runtimeBindingId']}
    original['command'] = {'operation':operation,'params':params['params']}
    private_root(STATE, create=True)
    new_record(STATE / ('draft-' + request + '.json'), {'format':1,'kind':'personalDraftControl','original':original,'payloadSha256':sha(json.dumps(original,sort_keys=True,separators=(',',':')).encode())})
    # Same original witness survives response loss. Never automatically resend.
    print(json.dumps(rpc(method,params),indent=2))

def draft_original(args):
    request = canonical_uuid(args.request_id)
    record = read_record(STATE / ('draft-' + request + '.json'))
    if record.get('format') != 1 or record.get('kind') != 'personalDraftControl': raise Refused('original private draft control witness required')
    original = record['original']
    if original.get('requestId') != request or original.get('command',{}).get('operation') not in ('personalDraft.ownerConfigure','personalDraft.memberSet'):
        raise Refused('wrong exact draft owner control')
    if record.get('payloadSha256') != sha(json.dumps(original,sort_keys=True,separators=(',',':')).encode()):
        raise Refused('original immutable draft envelope changed')
    return original

def draft_lookup(args):
    original = draft_original(args)
    print(json.dumps(rpc('host.personalDraftRequestLookup',{'requestId':original['requestId'],'dbId':original['dbId']}),indent=2))

def draft_fence(args):
    original = draft_original(args)
    # This writes only an original notSubmitted terminal receipt if missing.
    # It never calls owner/member mutation or changes the original UUID/body.
    print(json.dumps(rpc('host.personalDraftRequestFence',{'original':original}),indent=2))

def parser():
    p = argparse.ArgumentParser(description='PendingCrew one-link host installation/administration; never provider login or inference')
    commands = p.add_subparsers(dest='command', required=True)
    setup = commands.add_parser('install', allow_abbrev=False)
    setup.add_argument('--user'); setup.add_argument('--codex'); setup.add_argument('--claude'); setup.add_argument('--data-dir'); setup.add_argument('--manifest-url'); setup.add_argument('--manifest-sha256'); setup.add_argument('--request-id')
    setup.add_argument('--disable-claude', action='store_true', help='explicitly disable Claude; check and enable only Codex')
    setup.add_argument('--preserve-connection-from', metavar='STATUS_JSON', help='retain the identity/listener in original root0600 typed status JSON; no new enrollment')
    setup.add_argument('--listen-host', default='127.0.0.1'); setup.add_argument('--advertise-host', required=True); setup.add_argument('--port', type=int, default=49381); setup.add_argument('--label', default=socket.gethostname())
    for name in ['pair', 'recover']:
        c = commands.add_parser(name); c.add_argument('--operation-file', required=True)
        if name == 'pair': c.add_argument('--host', required=True); c.add_argument('--port', type=int, required=True); c.add_argument('--label', default=socket.gethostname())
    resume = commands.add_parser('resume-installation'); resume.add_argument('--request-id', required=True)
    notification = commands.add_parser('notification-configure'); notification.add_argument('--input-file')
    notification = commands.add_parser('notification-lookup'); notification.add_argument('--request-id', required=True)
    commands.add_parser('status'); commands.add_parser('stop'); commands.add_parser('grants')
    commands.add_parser('draft-status')
    draft = commands.add_parser('draft-members'); draft.add_argument('--space-id',required=True); draft.add_argument('--space-epoch',required=True)
    draft = commands.add_parser('draft-control'); draft.add_argument('--kind',required=True,choices=['owner','member']); draft.add_argument('--operation-file',required=True)
    for name in ['draft-lookup','draft-fence']:
        draft = commands.add_parser(name); draft.add_argument('--request-id',required=True)

    configuration = commands.add_parser('configuration'); configuration.add_argument('--operation-file', required=True)
    revoke = commands.add_parser('revoke'); revoke.add_argument('--grant-id', required=True); revoke.add_argument('--expected-revision', required=True); revoke.add_argument('--note', required=True)
    lookup = commands.add_parser('installation'); lookup.add_argument('--request-id', required=True)
    back = commands.add_parser('rollback'); back.add_argument('--request-id', required=True); back.add_argument('--expected-current', required=True); back.add_argument('--operation-id', required=True)
    return p

def main():
    args = parser().parse_args(); require_root(); os.umask(0o077)
    if args.command == 'install': install(args)
    elif args.command == 'resume-installation': resume_installation(args)
    elif args.command == 'pair': pair(args)
    elif args.command == 'recover': recover(args)
    elif args.command == 'status':
        if not active(): print(json.dumps({'service': 'inactive', 'noCoreWasStarted': True}))
        else: print(json.dumps(rpc('host.status', {}), indent=2))
    elif args.command == 'notification-configure': notification_configure(args)
    elif args.command == 'notification-lookup': notification_lookup(args)
    elif args.command == 'draft-status': print(json.dumps(rpc('host.personalDraftStatus',{}),indent=2))
    elif args.command == 'draft-members': print(json.dumps(rpc('host.personalDraftMembers',{'spaceId':canonical_uuid(args.space_id),'spaceEpoch':checked_text(args.space_epoch,512)}),indent=2))
    elif args.command == 'draft-control': draft_control(args)
    elif args.command == 'draft-lookup': draft_lookup(args)
    elif args.command == 'draft-fence': draft_fence(args)
    elif args.command == 'grants': print(json.dumps(rpc('host.grants', {}), indent=2))
    elif args.command == 'configuration':
        record = read_record(pathlib.Path(args.operation_file))
        if record.get('format') != 1 or record.get('kind') != 'configuration': raise Refused('wrong original configuration record')
        print(json.dumps(rpc('host.configurationLookup', record['params']), indent=2))
    elif args.command == 'stop':
        result = rpc('host.stop', {}, timeout=150)
        if result.get('status') != 'stopped': raise Refused('original safe stop not confirmed; no forced termination')
        subprocess.run(['/usr/bin/systemctl', 'stop', SOCKET_UNIT], check=True, timeout=30, stdout=subprocess.DEVNULL)
        print(json.dumps(result, indent=2))
    elif args.command == 'rollback': rollback(args)
    elif args.command == 'revoke': print(json.dumps(rpc('host.revoke', {'grantId': args.grant_id, 'expectedRevision': args.expected_revision, 'note': checked_text(args.note)}), indent=2))
    elif args.command == 'installation':
        value = read_record(STATE / ('install-' + canonical_uuid(args.request_id) + '.json'))
        print(json.dumps({'original': value, 'current': pointer(PREFIX), 'serviceActive': active()}, indent=2))

if __name__ == '__main__':
    try: main()
    except (Refused, OSError, ValueError, KeyError, subprocess.SubprocessError) as failure:
        print('PendingCrew stopped: ' + str(failure), file=sys.stderr); raise SystemExit(1)
