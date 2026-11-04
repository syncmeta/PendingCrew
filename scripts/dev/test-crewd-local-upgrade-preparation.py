#!/usr/bin/env python3
"""Private fixture for the ordinary local-upgrade preparation contract."""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / 'scripts/release/prepare-local-upgrade.py'
REQUIREMENT = 'identifier "com.pendingname.pendingcrew"'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(*args, refusal=None):
    result = subprocess.run([sys.executable, str(HELPER), *map(str, args)],
                            capture_output=True, text=True, timeout=30)
    if refusal is None:
        assert result.returncode == 0, result.stderr
    else:
        assert result.returncode != 0 and refusal in result.stderr, result
    return result.stdout


def ordinary(path, source, native=False):
    contents = path / 'Contents'
    for name in ('Helpers', 'MacOS'):
        (contents / name).mkdir(parents=True)
    for tool, destination in (('true', 'MacOS/PendingCrew'), ('false', 'Helpers/crewd')):
        target = contents / destination
        shutil.copyfile('/usr/bin/' + tool, target)
        target.chmod(0o755)
    (contents / 'Info.plist').write_bytes(plistlib.dumps({
        'CFBundleExecutable': 'PendingCrew', 'CFBundleIdentifier': 'com.pendingname.pendingcrew',
        'CFBundlePackageType': 'APPL', 'BuildStampCommit': source, 'PendingCrewSourceRef': source,
        'PendingCrewNativeCandidate': native,
    }))
    subprocess.run(['/usr/bin/codesign', '--force', '--deep', '--sign', '-', str(path)],
                   check=True, capture_output=True)
    return path


def sqlite(path, *statements):
    with contextlib.closing(sqlite3.connect(path)) as connection:
        for statement in statements:
            connection.execute(statement)
        connection.commit()


def contents(root):
    return {p.relative_to(root).as_posix(): sha(p) for p in root.rglob('*') if p.is_file()}


def native_lock_probe(core, expected, root):
    assert core.is_absolute() and core.resolve() == core and sha(core) == expected
    data = root / 'real-core'
    home = root / 'home'
    data.mkdir(mode=0o700)
    home.mkdir(mode=0o700)
    env = {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'HOME': str(home),
           'CLAUDE_CONFIG_DIR': str(home / 'claude'), 'TMPDIR': str(root)}
    with (root / 'real-core.log').open('w') as log:
        process = subprocess.Popen([str(core), '--data-dir', str(data), 'serve',
                                    '--codex', '/usr/bin/false', '--claude', '/usr/bin/false'],
                                   env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        try:
            end = time.monotonic() + 10
            while not (data / 'crewd.sock').exists():
                if process.poll() is not None or time.monotonic() >= end:
                    raise AssertionError((root / 'real-core.log').read_text())
                time.sleep(.02)
            with (data / 'crewd.lock').open('rb') as lock:
                try:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    pass
                else:
                    raise AssertionError('Python lease did not contend with real Core')
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
    assert process.returncode == 0
    print('PASS Python lock contends with actual packaged Core lease')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--core', type=Path)
    parser.add_argument('--core-sha')
    args = parser.parse_args()
    if bool(args.core) != bool(args.core_sha):
        parser.error('--core and --core-sha are required together')
    with tempfile.TemporaryDirectory(prefix='pcw-local-stage-', dir='/private/tmp') as temporary:
        root = Path(temporary)
        old = ordinary(root / 'OldSource.app', '1' * 40)
        new = ordinary(root / 'NewSource.app', '2' * 40)
        native = ordinary(root / 'Native.app', '3' * 40, native=True)
        old_package, new_package = [
            json.loads(run('inspect-package', '--app', app, '--requirement', REQUIREMENT))
            for app in (old, new)
        ]
        run('inspect-package', '--app', native, '--requirement', REQUIREMENT, refusal='ordinary')
        data = root / 'data'
        data.mkdir(mode=0o700)
        (data / 'crewd.lock').touch()
        (data / 'crewd.db.id').write_text('private-test-id\n')
        (data / 'exec-registry.json').write_text('{}')
        sqlite(data / 'crewd.db', 'PRAGMA application_id=0x43525744', 'PRAGMA user_version=21',
               'CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT)',
               "INSERT INTO meta VALUES('dbId','private-test-id'),('lifecycle.clean','true')")
        (data / 'knowledge').mkdir()
        sqlite(data / 'knowledge/example.sqlite', 'CREATE TABLE content(text TEXT)',
               "INSERT INTO content VALUES('retained original content')")
        (data / 'attachments').mkdir()
        (data / 'attachments/example').write_bytes(b'retained original attachment')
        base = ('prepare', '--authority', data, '--old-app', old, '--new-app', new,
                '--old-source', old_package['source'], '--new-source', new_package['source'],
                '--old-digest', old_package['bundleDigest'], '--requirement', REQUIREMENT)
        prepare = (*base, '--new-digest', new_package['bundleDigest'])
        run(*base, '--new-digest', '0' * 64, '--out', root / 'wrong-app',
            refusal='ordinary App inventory changed')
        with (data / 'crewd.lock').open('rb') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            run(*prepare, '--out', root / 'locked', refusal='Core authority is still in use')
        for name in ('locked', 'wrong-app'):
            assert not (root / name).exists()
        (data / 'crewd.db-wal').touch()
        run(*prepare, '--out', root / 'wal', refusal='SQLite sidecars')
        (data / 'crewd.db-wal').unlink()
        (data / 'exec-registry.json').write_text('{"unresolved": {}}')
        run(*prepare, '--out', root / 'registry', refusal='unresolved executor process obligations')
        (data / 'exec-registry.json').write_text('{}')
        path = data / 'crewd.lock'
        path.rename(data / 'other')
        path.symlink_to(data / 'other')
        run(*prepare, '--out', root / 'aliased-lock', refusal='crewd.lock')
        path.unlink()
        (data / 'other').rename(path)
        before = contents(data)
        preparation = root / 'preparation'
        run(*prepare, '--out', preparation)
        assert contents(data) == contents(preparation / 'before21') == before
        plan = preparation / 'plan.json'
        rollback = ('stage-rollback', '--authority', data, '--plan', plan, '--plan-sha', sha(plan))
        run(*rollback, '--upgraded-app', old, '--out', root / 'wrong-rollback-app',
            refusal='upgraded App differs from preparation')
        old.rename(root / 'FormerActive.app')
        shutil.copytree(new, old, symlinks=True)
        assert contents(old) == contents(new)
        rollback += ('--upgraded-app', old)
        run(*rollback, '--out', root / 'old-only', refusal='unsupported Core database generation')
        sqlite(data / 'crewd.db', 'PRAGMA user_version=22', 'CREATE TABLE newer(text TEXT)')
        sqlite(data / 'knowledge/example.sqlite', "UPDATE content SET text='after cutoff'")
        (data / 'attachments/new').write_bytes(b'added after cutoff')
        upgraded = contents(data)
        run('stage-rollback', '--authority', data, '--plan', plan, '--plan-sha', sha(plan),
            '--upgraded-app', new, '--out', root / 'wrong-rollback-path',
            refusal='upgraded App path differs from preparation')
        assert not (root / 'wrong-rollback-path').exists()
        staged = root / 'staged-rollback'
        run(*rollback, '--out', staged)
        assert contents(staged / 'restore21') == before
        assert contents(staged / 'preserved22') == contents(data) == upgraded
        assert contents(preparation / 'before21') == before
        (preparation / 'before21/attachments/example').write_bytes(b'modified evidence')
        run(*rollback, '--out', root / 'tampered', refusal='sealed pre-cut authority changed')
        assert contents(data) == upgraded
        print('PASS ordinary package checks, lock/sidecar refusal, sealed Core21 and preserved Core22')
        if args.core:
            native_lock_probe(args.core, args.core_sha, root)


if __name__ == '__main__':
    main()
