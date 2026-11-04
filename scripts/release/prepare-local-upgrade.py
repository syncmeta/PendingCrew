#!/usr/bin/env python3
"""Prepare evidence for a stopped ordinary Mac Core21→22 update or its rollback."""
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


@contextlib.contextmanager
def lease(root):
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

        def recheck():
            attached = lock.lstat()
            require(os.path.samestat(root.stat(), initial) and os.path.samestat(attached, current)
                    and attached.st_nlink == 1, 'authority or Core lock was replaced')

        recheck()
        yield recheck
        recheck()
    finally:
        os.close(fd)


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
    data = directory(args.authority)
    old = directory(args.old_app)
    new = directory(args.new_app)
    require(old != new, 'old and new App must be distinct')
    old_package = package(old, args.requirement, args.old_source, args.old_digest)
    new_package = package(new, args.requirement, args.new_source, args.new_digest)
    require(old_package['source'] != new_package['source'], 'distinct source generations required')
    with lease(data) as recheck:
        rows = inventory(data)
        identity = inspect(data, rows, 21)
        old_rows = inventory(old, bundle=True)
        require(digest(old_rows) == old_package['bundleDigest'], 'old App changed before staging')
        out = disjoint_out(args.out, data, old, new)
        copy_verified(data, out / 'before21', rows)
        copy_verified(old, out / 'Old.app', old_rows, bundle=True)
        require(package(out / 'Old.app', args.requirement) == old_package
                and package(new, args.requirement) == new_package
                and inspect(out / 'before21', rows, 21) == identity
                and inventory(data) == rows, 'staged generations did not verify')
        recheck()
        write_record(out / 'plan.json', {
            'format': 1, 'kind': 'pendingcrew-local-upgrade-preparation',
            'authority': str(data), 'dbId': identity, 'before21': rows,
            'oldApp': old_package, 'newApp': new_package, 'requirement': args.requirement,
        })
    print(f'Prepared evidence only: {out / "plan.json"} (SHA256 {sha(out / "plan.json")})')


def stage_rollback(args):
    plan_path = Path(args.plan)
    require(plan_path.name == 'plan.json' and directory(plan_path.parent) == plan_path.parent,
            'canonical preparation record required')
    plain_file(plan_path)
    record = plan_path.read_bytes()
    require(hashlib.sha256(record).hexdigest() == args.plan_sha, 'preparation record digest differs')
    plan = json.loads(record)
    require(plan.get('format') == 1 and plan.get('kind') == 'pendingcrew-local-upgrade-preparation',
            'unsupported preparation record')
    data = directory(args.authority)
    require(str(data) == plan['authority'], 'rollback authority differs from preparation')
    upgraded_app = directory(args.upgraded_app)
    old = directory(plan_path.parent / 'Old.app')
    sealed = directory(plan_path.parent / 'before21')
    require(package(upgraded_app, plan['requirement']) == plan['newApp'], 'upgraded App differs from preparation')
    require(package(old, plan['requirement']) == plan['oldApp'], 'sealed old App changed')
    require(inventory(sealed) == plan['before21'] and inspect(sealed, plan['before21'], 21) == plan['dbId'],
            'sealed pre-cut authority changed')
    with lease(data) as recheck:
        after = inventory(data)
        require(inspect(data, after, 22) == plan['dbId'], 'upgraded authority identity changed')
        out = disjoint_out(args.out, data, plan_path.parent, upgraded_app)
        copy_verified(data, out / 'preserved22', after)
        copy_verified(sealed, out / 'restore21', plan['before21'])
        copy_verified(old, out / 'Old.app', inventory(old, bundle=True), bundle=True)
        require(inspect(out / 'preserved22', after, 22) == plan['dbId']
                and inspect(out / 'restore21', plan['before21'], 21) == plan['dbId']
                and package(out / 'Old.app', plan['requirement']) == plan['oldApp']
                and package(upgraded_app, plan['requirement']) == plan['newApp']
                and inventory(data) == after, 'rollback staging did not verify')
        recheck()
        write_record(out / 'rollback.json', {
            'format': 1, 'kind': 'pendingcrew-local-rollback-staging',
            'planSHA256': args.plan_sha, 'dbId': plan['dbId'], 'upgraded22': after,
            'restore21': plan['before21'], 'oldApp': plan['oldApp'], 'upgradedApp': plan['newApp'],
        })
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
    cut.set_defaults(action=prepare)
    restore = commands.add_parser('stage-rollback')
    for name in ('authority', 'upgraded-app', 'plan', 'plan-sha', 'out'):
        restore.add_argument('--' + name, required=True)
    restore.set_defaults(action=stage_rollback)
    args = parser.parse_args()
    args.action(args)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, sqlite3.Error, subprocess.CalledProcessError) as error:
        raise SystemExit(f'FAIL: {error}')
