#!/usr/bin/env python3
"""Prepare a stopped ordinary Mac upgrade bound to exact source generations."""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import sqlite3
import stat
import subprocess

BUNDLE_ID = 'com.pendingname.pendingcrew'
CORE_APPLICATION_ID = 0x43525744
SCHEMA_SOURCE = 'crewd/crates/store/src/schema.rs'
REPOSITORY = Path(__file__).resolve().parents[2]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def directory(raw):
    path = Path(raw)
    require(path.is_absolute() and path.resolve(strict=True) == path and path.is_dir(),
            'canonical existing directory required')
    return path


def plain_file(path):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1, f'unsupported file: {path.name}')
    return info


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def digest(rows):
    return hashlib.sha256(json.dumps(rows, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def inventory(root, *, bundle=False):
    rows = {}
    for path in sorted(root.rglob('*')):
        name = path.relative_to(root).as_posix()
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode):
            require(bundle and path.resolve(strict=True).is_relative_to(root),
                    f'unsupported symbolic link: {name}')
            rows[name] = {'link': os.readlink(path)}
        elif stat.S_ISDIR(info.st_mode):
            rows[name] = {'directoryMode': stat.S_IMODE(info.st_mode)}
        else:
            plain_file(path)
            rows[name] = {'sha256': sha(path), 'mode': stat.S_IMODE(info.st_mode)}
    return rows


def package(root, requirement, source=None, expected=None):
    info = root / 'Contents/Info.plist'
    helper = root / 'Contents/Helpers/crewd'
    for path in (info, helper):
        require(path.resolve(strict=True) == path, 'aliased App metadata or Core helper')
        plain_file(path)
    plist = plistlib.loads(info.read_bytes())
    ref = plist.get('BuildStampCommit')
    require(plist.get('CFBundleIdentifier') == BUNDLE_ID
            and plist.get('PendingCrewNativeCandidate') is not True
            and isinstance(ref, str) and re.fullmatch(r'[0-9a-f]{40}', ref)
            and plist.get('PendingCrewSourceRef') == ref
            and os.access(helper, os.X_OK), 'an ordinary source-stamped Mac App is required')
    if source is not None:
        require(ref == source, 'ordinary App source stamp changed')
    subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict',
                    '-R=' + requirement, str(root)], check=True, capture_output=True)
    rows = inventory(root, bundle=True)
    snapshot = {'source': ref, 'bundleDigest': digest(rows),
                'helperSHA256': rows['Contents/Helpers/crewd']['sha256']}
    if expected is not None:
        require(snapshot['bundleDigest'] == expected, 'ordinary App inventory changed')
    return snapshot


def isolated_root(args, *paths):
    if args.isolated_fixture is None:
        return None
    root = directory(args.isolated_fixture)
    info = root.stat()
    require(root.parent == Path('/private/tmp') and info.st_uid == os.getuid()
            and stat.S_IMODE(info.st_mode) == 0o700,
            'fixture root must be a canonical owner-private directory directly under /private/tmp')
    for raw in paths:
        path = Path(raw)
        require(path.is_absolute() and path.resolve() == path
                and path != root and path.is_relative_to(root), 'fixture path escapes private root')
    return str(root)


def require_mode(record, fixture):
    require('fixtureRoot' in record and record['fixtureRoot'] == fixture,
            'preparation mode or fixture root differs from invocation')


def source_generation(source, fixture):
    """Read the single schema declaration from the pinned commit, never HEAD.

    The explicit fixture mode has its own synthetic Git object store. It does
    not replace source evidence with a caller-supplied numeric version.
    """
    require(isinstance(source, str) and re.fullmatch(r'[0-9a-f]{40}', source),
            'full frozen source generation required')
    repo = directory(Path(fixture) / 'source-evidence') if fixture is not None else REPOSITORY
    command = ['git', '--no-replace-objects', '-C', str(repo)]
    commit = subprocess.run([*command, 'rev-parse', '--verify', source + '^{commit}'],
                            capture_output=True, text=True, timeout=30)
    require(commit.returncode == 0 and commit.stdout.strip() == source,
            'frozen schema source commit is unavailable')
    result = subprocess.run([*command, 'cat-file', 'blob', source + ':' + SCHEMA_SOURCE],
                            capture_output=True, timeout=30)
    require(result.returncode == 0, 'frozen schema source blob is unavailable')
    try:
        text = result.stdout.decode('utf-8')
    except UnicodeDecodeError:
        raise ValueError('frozen schema source is not UTF-8') from None
    declarations = re.findall(r'^pub const SCHEMA_VERSION: i64 = ([0-9]+);$', text, re.MULTILINE)
    require(len(declarations) == 1 and 0 < int(declarations[0]) < 2 ** 31,
            'one exact positive source schema declaration required')
    return {'source': source, 'schemaVersion': int(declarations[0]),
            'schemaSourceSHA256': hashlib.sha256(result.stdout).hexdigest()}


def schema_evidence(old, new, fixture):
    evidence = {'old': source_generation(old['source'], fixture),
                'new': source_generation(new['source'], fixture)}
    require(evidence['old']['schemaVersion'] < evidence['new']['schemaVersion'],
            'target source must declare a newer Core generation')
    return evidence


def plan_contract(plan):
    """Keep original format-1 receipts exact; new plans carry complete evidence."""
    require(plan.get('kind') == 'pendingcrew-local-upgrade-preparation'
            and type(plan.get('format')) is int and plan['format'] in (1, 2),
            'unsupported preparation record')
    if plan['format'] == 1:
        return {'old': 21, 'new': 22, 'before': 'before21',
                'preserved': 'preserved22', 'restore': 'restore21', 'upgraded': 'upgraded22'}
    require(set(plan) == {'format', 'kind', 'fixtureRoot', 'app', 'authority', 'dbId',
                         'before', 'oldApp', 'newApp', 'requirement', 'schemaEvidence'},
            'closed source-bound preparation record required')
    actual = schema_evidence(plan['oldApp'], plan['newApp'], plan['fixtureRoot'])
    require(plan['schemaEvidence'] == actual, 'frozen source/schema evidence changed')
    return {'old': actual['old']['schemaVersion'], 'new': actual['new']['schemaVersion'],
            'before': 'before', 'preserved': 'preserved', 'restore': 'restore', 'upgraded': 'upgraded'}


def rollback_contract(record, plan):
    layout = plan_contract(plan)
    unmigrated = record.get('format') == 3
    require((record.get('format') == plan['format'] or (unmigrated and plan['format'] == 2))
            and record.get('kind') == 'pendingcrew-local-rollback-staging',
            'unsupported rollback staging record')
    if plan['format'] == 2:
        keys = {'format', 'kind', 'fixtureRoot', 'planSHA256', 'app', 'dbId',
                'upgraded', 'restore', 'oldApp', 'upgradedApp', 'schemaEvidence'}
        if unmigrated:
            keys.add('rollbackState')
        require(set(record) == keys
                and record['schemaEvidence'] == plan['schemaEvidence'],
                'rollback source/schema evidence differs from preparation')
    if unmigrated:
        require(record['rollbackState'] == 'unmigrated', 'explicit unmigrated rollback state required')
    layout['outgoing'] = layout['old'] if unmigrated else layout['new']
    return layout


def intent_contract(intent):
    require(type(intent.get('format')) is int and intent['format'] in (1, 2, 3)
            and intent.get('kind') == 'pendingcrew-local-cutover-intent'
            and intent.get('operation') in ('upgrade', 'rollback'), 'unsupported cutover intent')
    if intent['format'] == 1:
        require(intent.get('coreVersions') == ([21, 21] if intent['operation'] == 'upgrade' else [22, 21]),
                'legacy cutover schema generations changed')
        return
    keys = {'format', 'kind', 'fixtureRoot', 'id', 'operation', 'planSHA256',
            'app', 'authority', 'dbId', 'appInodes', 'dataInodes', 'apps',
            'requirement', 'coreVersions', 'dataDigests', 'schemaEvidence'}
    if intent['format'] == 3:
        keys.add('rollbackState')
        require(intent['operation'] == 'rollback' and intent.get('rollbackState') == 'unmigrated',
                'explicit unmigrated rollback intent required')
    require(set(intent) == keys,
            'closed source-bound cutover intent required')
    apps = intent['apps']
    require(isinstance(apps, list) and len(apps) == 2, 'two exact App generations required')
    old, new = apps if intent['operation'] == 'upgrade' else reversed(apps)
    actual = schema_evidence(old, new, intent['fixtureRoot'])
    old_version, new_version = actual['old']['schemaVersion'], actual['new']['schemaVersion']
    expected = ([old_version, old_version] if intent['operation'] == 'upgrade' or intent['format'] == 3
                else [new_version, old_version])
    require(intent['schemaEvidence'] == actual and intent['coreVersions'] == expected,
            'cutover source/schema evidence changed')


def qualify_upgrade(app, fixture):
    if fixture is not None:
        return
    signed = subprocess.run(['/usr/bin/codesign', '-d', '--verbose=4', str(app)],
                            capture_output=True, text=True, timeout=30)
    lines = (signed.stdout + signed.stderr).splitlines()
    require(signed.returncode == 0
            and any(line.startswith('Authority=Developer ID Application: ') for line in lines)
            and any(re.fullmatch(r'TeamIdentifier=[0-9A-Z]{10}', line) for line in lines)
            and any(line.startswith('Timestamp=') and line != 'Timestamp=' for line in lines),
            'ordinary upgrade requires timestamped Developer ID Application signing')
    ticket = subprocess.run(['/usr/bin/xcrun', 'stapler', 'validate', str(app)],
                            capture_output=True, text=True, timeout=60)
    require(ticket.returncode == 0, 'ordinary upgrade requires a valid stapled notarization ticket')
    policy = subprocess.run(['/usr/sbin/spctl', '--assess', '--type', 'execute', '--ignore-cache',
                             '--no-cache', '--verbose=4', str(app)],
                            capture_output=True, text=True, timeout=60)
    require(policy.returncode == 0 and 'source=Notarized Developer ID' in
            (policy.stdout + policy.stderr).splitlines(), 'Gatekeeper did not accept a notarized Developer ID App')


@contextlib.contextmanager
def authority_lease(root):
    initial = root.stat()
    require(initial.st_uid == os.getuid() and initial.st_mode & 0o077 == 0,
            'authority directory must be owner-private')
    lock = root / 'crewd.lock'
    fd = os.open(lock, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        current = os.fstat(fd)
        require(stat.S_ISREG(current.st_mode) and current.st_uid == os.getuid() and current.st_nlink == 1,
                'unsupported Core lock inode')
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ValueError('Core authority is still in use') from None

        def recheck(location):
            attached = (location / 'crewd.lock').lstat()
            require(os.path.samestat(location.lstat(), initial) and os.path.samestat(attached, current)
                    and attached.st_nlink == 1, 'authority or Core lock was replaced')

        recheck(root)
        yield recheck
    finally:
        os.close(fd)


@contextlib.contextmanager
def lease(root):
    with authority_lease(root) as recheck:
        def check_here():
            recheck(root)
        yield check_here
        check_here()


def inspect(root, rows, version):
    require('crewd.sock' not in rows and 'exec-registry.json.tmp' not in rows,
            'socket or unfinished executor registry still exists')
    if 'exec-registry.json' in rows:
        require(json.loads((root / 'exec-registry.json').read_text()) == {},
                'unresolved executor process obligations')
    require(not any(name.endswith(('-wal', '-shm', '-journal')) for name in rows),
            'stopped authority still has SQLite sidecars')
    database_paths = [root / name for name, row in rows.items() if 'sha256' in row
                      and name.endswith(('.db', '.sqlite', '.sqlite3'))]
    core = root / 'crewd.db'
    require(core in database_paths and 'crewd.db.id' in rows, 'Core database or identity missing')
    for path in database_paths:
        with contextlib.closing(sqlite3.connect(path.as_uri() + '?immutable=1', uri=True)) as db:
            require(db.execute('PRAGMA integrity_check').fetchall() == [('ok',)]
                    and not db.execute('PRAGMA foreign_key_check').fetchall(),
                    f'SQLite integrity check failed: {path.name}')
            if path == core:
                require(db.execute('PRAGMA application_id').fetchone()[0] == CORE_APPLICATION_ID
                        and db.execute('PRAGMA user_version').fetchone()[0] == version,
                        'unsupported Core database generation')
                meta = dict(db.execute("SELECT key,value FROM meta WHERE key IN ('dbId','lifecycle.clean')"))
                identity = (root / 'crewd.db.id').read_text().splitlines()
                require(identity and meta.get('dbId') == identity[0] and meta.get('lifecycle.clean') == 'true',
                        'Core identity or clean shutdown is unconfirmed')
    return identity[0]


def disjoint_out(raw, *inputs):
    out = Path(raw)
    require(out.is_absolute() and directory(out.parent) == out.parent, 'canonical output parent required')
    require(not any(out.is_relative_to(path) or path.is_relative_to(out) for path in inputs),
            'evidence output overlaps an input')
    out.mkdir(mode=0o700)
    return out


def copy_verified(source, destination, rows, *, bundle=False):
    subprocess.run(['/usr/bin/ditto', '--rsrc', '--extattr', '--acl',
                    str(source), str(destination)], check=True, capture_output=True)
    require(inventory(destination, bundle=bundle) == rows and inventory(source, bundle=bundle) == rows,
            'source changed or complete copy did not verify')
    for name, row in rows.items():
        if 'sha256' in row:
            sync(destination / name)
    for name, row in reversed(rows.items()):
        if 'directoryMode' in row:
            sync(destination / name, os.O_DIRECTORY)
    sync(destination, os.O_DIRECTORY)


def sync(path, flags=0):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | flags)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write_record(path, value):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'w') as stream:
        json.dump(value, stream, sort_keys=True, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    sync(path.parent, os.O_DIRECTORY)


def prepare(args):
    fixture = isolated_root(args, args.authority, args.old_app, args.new_app, args.out)
    data = directory(args.authority)
    old = directory(args.old_app)
    new = directory(args.new_app)
    require(old != new, 'old and new App must be distinct')
    old_package = package(old, args.requirement, args.old_source, args.old_digest)
    new_package = package(new, args.requirement, args.new_source, args.new_digest)
    require(old_package['source'] != new_package['source'], 'distinct source generations required')
    qualify_upgrade(new, fixture)
    evidence = schema_evidence(old_package, new_package, fixture)
    for side in ('old', 'new'):
        assertion = getattr(args, side + '_schema')
        require(assertion is None or assertion == evidence[side]['schemaVersion'],
                'explicit schema assertion differs from frozen source')
    old_version = evidence['old']['schemaVersion']
    with lease(data) as recheck:
        rows = inventory(data)
        identity = inspect(data, rows, old_version)
        old_rows = inventory(old, bundle=True)
        require(digest(old_rows) == old_package['bundleDigest'], 'old App changed before staging')
        out = disjoint_out(args.out, data, old, new)
        copy_verified(data, out / 'before', rows)
        copy_verified(old, out / 'Old.app', old_rows, bundle=True)
        require(package(out / 'Old.app', args.requirement) == old_package
                and package(new, args.requirement) == new_package
                and inspect(out / 'before', rows, old_version) == identity
                and inventory(data) == rows, 'staged generations did not verify')
        recheck()
        write_record(out / 'plan.json', {
            'format': 2, 'kind': 'pendingcrew-local-upgrade-preparation',
            'fixtureRoot': fixture,
            'app': str(old), 'authority': str(data), 'dbId': identity, 'before': rows,
            'oldApp': old_package, 'newApp': new_package, 'requirement': args.requirement,
            'schemaEvidence': evidence,
        })
    print(f'Prepared evidence only: {out / "plan.json"} (SHA256 {sha(out / "plan.json")})')


def stage_rollback(args):
    fixture = isolated_root(args, args.authority, args.upgraded_app, args.plan, args.out)
    plan_path = Path(args.plan)
    require(plan_path.name == 'plan.json' and directory(plan_path.parent) == plan_path.parent,
            'canonical preparation record required')
    plain_file(plan_path)
    record = plan_path.read_bytes()
    require(hashlib.sha256(record).hexdigest() == args.plan_sha, 'preparation record digest differs')
    plan = json.loads(record)
    require_mode(plan, fixture)
    layout = plan_contract(plan)
    unmigrated = args.unmigrated
    require(not unmigrated or plan['format'] == 2,
            'unmigrated rollback requires a source-bound preparation')
    outgoing_version = layout['old'] if unmigrated else layout['new']
    data = directory(args.authority)
    require(str(data) == plan['authority'], 'rollback authority differs from preparation')
    upgraded_app = directory(args.upgraded_app)
    require(str(upgraded_app) == plan.get('app'), 'upgraded App path differs from preparation')
    old = directory(plan_path.parent / 'Old.app')
    sealed = directory(plan_path.parent / layout['before'])
    require(package(upgraded_app, plan['requirement']) == plan['newApp'], 'upgraded App differs from preparation')
    require(package(old, plan['requirement']) == plan['oldApp'], 'sealed old App changed')
    before = plan[layout['before']]
    require(inventory(sealed) == before and inspect(sealed, before, layout['old']) == plan['dbId'],
            'sealed pre-cut authority changed')
    with lease(data) as recheck:
        after = inventory(data)
        require(inspect(data, after, outgoing_version) == plan['dbId'], 'upgraded authority identity changed')
        out = disjoint_out(args.out, data, plan_path.parent, upgraded_app)
        copy_verified(data, out / layout['preserved'], after)
        copy_verified(sealed, out / layout['restore'], before)
        copy_verified(old, out / 'Old.app', inventory(old, bundle=True), bundle=True)
        require(inspect(out / layout['preserved'], after, outgoing_version) == plan['dbId']
                and inspect(out / layout['restore'], before, layout['old']) == plan['dbId']
                and package(out / 'Old.app', plan['requirement']) == plan['oldApp']
                and package(upgraded_app, plan['requirement']) == plan['newApp']
                and inventory(data) == after, 'rollback staging did not verify')
        recheck()
        record = {
            'format': 3 if unmigrated else plan['format'], 'kind': 'pendingcrew-local-rollback-staging',
            'fixtureRoot': fixture,
            'planSHA256': args.plan_sha, 'app': str(upgraded_app), 'dbId': plan['dbId'], layout['upgraded']: after,
            layout['restore']: before, 'oldApp': plan['oldApp'], 'upgradedApp': plan['newApp'],
        }
        if plan['format'] == 2:
            record['schemaEvidence'] = plan['schemaEvidence']
        if unmigrated:
            record['rollbackState'] = 'unmigrated'
        write_record(out / 'rollback.json', record)
    print(f'Rollback staged without replacing either generation: {out / "rollback.json"}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(required=True)
    audit = commands.add_parser('inspect-package', help='print the ordinary App inventory and Core digest')
    audit.add_argument('--app', required=True)
    audit.add_argument('--requirement', required=True)
    audit.set_defaults(action=lambda args: print(json.dumps(package(directory(args.app), args.requirement), indent=2)))
    cut = commands.add_parser('prepare')
    for name in ('authority', 'old-app', 'old-source', 'old-digest', 'new-app', 'new-source',
                 'new-digest', 'requirement', 'out'):
        cut.add_argument('--' + name, required=True)
    cut.add_argument('--isolated-fixture')
    cut.add_argument('--old-schema', type=int, help='optional assertion; actual generation comes from frozen source')
    cut.add_argument('--new-schema', type=int, help='optional assertion; actual generation comes from frozen source')
    cut.set_defaults(action=prepare)
    restore = commands.add_parser('stage-rollback')
    for name in ('authority', 'upgraded-app', 'plan', 'plan-sha', 'out'):
        restore.add_argument('--' + name, required=True)
    restore.add_argument('--isolated-fixture')
    restore.add_argument('--unmigrated', action='store_true',
                         help='explicit rollback of a stopped new App whose clean authority is still at the old source schema')
    restore.set_defaults(action=stage_rollback)
    args = parser.parse_args()
    args.action(args)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, sqlite3.Error, subprocess.SubprocessError) as error:
        raise SystemExit(f'FAIL: {error}')
