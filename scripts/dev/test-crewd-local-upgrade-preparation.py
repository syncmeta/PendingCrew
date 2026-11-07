#!/usr/bin/env python3
"""Private fixture for the ordinary local-upgrade preparation contract."""
import argparse
import contextlib
import fcntl
import hashlib
import importlib.util
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
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / 'scripts/release/prepare-local-upgrade.py'
REQUIREMENT = 'identifier "com.pendingname.pendingcrew"'
sys.dont_write_bytecode = True


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


def source_evidence(root):
    """Real isolated Git commits, not a mock schema number supplied to production."""
    repo = root / 'source-evidence'
    subprocess.run(['git', 'init', '--quiet', '--initial-branch', 'fixture', str(repo)], check=True)
    source = repo / 'crewd/crates/store/src/schema.rs'
    source.parent.mkdir(parents=True)
    env = dict(os.environ, GIT_AUTHOR_NAME='PendingCrew fixture',
               GIT_AUTHOR_EMAIL='fixture@example.invalid', GIT_COMMITTER_NAME='PendingCrew fixture',
               GIT_COMMITTER_EMAIL='fixture@example.invalid')
    refs = {}
    for version in (21, 22, 33):
        source.write_text('pub const SCHEMA_VERSION: i64 = ' + str(version) + ';\n')
        subprocess.run(['git', '-C', str(repo), 'add', 'crewd'], check=True, env=env)
        tree = subprocess.check_output(['git', '-C', str(repo), 'write-tree'], env=env, text=True).strip()
        argv = ['git', '-c', 'commit.gpgsign=false', '-C', str(repo), 'commit-tree', tree,
                '-m', 'synthetic schema ' + str(version)]
        if refs:
            argv += ['-p', refs[max(refs)]]
        refs[version] = subprocess.check_output(argv, env=env, text=True).strip()
    return refs


def delivery_policy(app):
    spec = importlib.util.spec_from_file_location('delivery_preparation', HELPER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    signed = 'Authority=Developer ID Application: Fixture\nTeamIdentifier=ABCDEFGHIJ\nTimestamp=Fixture\n'
    accepted = 'accepted\nsource=Notarized Developer ID\n'
    def reply(output, code=0):
        return subprocess.CompletedProcess([], code, '', output)

    with mock.patch.object(module.subprocess, 'run', side_effect=[reply(signed), reply('ticket'), reply(accepted)]) as calls:
        module.qualify_upgrade(app, None)
        assert [call.args[0][0] for call in calls.call_args_list] == [
            '/usr/bin/codesign', '/usr/bin/xcrun', '/usr/sbin/spctl',
        ]
        assert calls.call_args_list[1].args[0][1:3] == ['stapler', 'validate']
        assert '--no-cache' in calls.call_args_list[2].args[0]
    for replies, expected in [
        ([reply(signed.replace('Timestamp=', 'Signed Time='))], 'timestamped Developer ID'),
        ([reply(signed), reply('missing ticket', 1)], 'valid stapled notarization'),
        ([reply(signed), reply('ticket'), reply('source=Unnotarized Developer ID', 3)], 'Gatekeeper did not accept'),
    ]:
        with mock.patch.object(module.subprocess, 'run', side_effect=replies) as calls:
            try:
                module.qualify_upgrade(app, None)
            except ValueError as error:
                assert expected in str(error) and calls.call_count == len(replies)
            else:
                raise AssertionError('unqualified distribution accepted')
    print('PASS distribution gate checks timestamp, staple and Gatekeeper in order')


def legacy_guard(root):
    installed = root / 'InstalledSwift.app'
    incoming = root / 'IncomingSwift.app'
    for app in (installed, incoming):
        (app / 'Contents/Helpers').mkdir(parents=True)
    (incoming / 'Contents/Info.plist').write_bytes(plistlib.dumps({
        'CFBundleShortVersionString': '0.1.0', 'CFBundleVersion': '1',
    }))
    script = root / 'legacy-install.sh'
    original = (ROOT / 'scripts/release/install-local-update.sh').read_text()
    script.write_text(original.replace('old="/Applications/PendingCrew.app"', f'old="{installed}"')
                      .replace('/tmp/pendingcrew-local-install.XXXXXX', str(root / 'extract.XXXXXX')))
    commands = root / 'bin'
    commands.mkdir()
    spctl = commands / 'spctl'
    spctl.write_text('#!/bin/sh\necho "fixture signature gate reached" >&2\nexit 1\n')
    spctl.chmod(0o755)
    env = {'PATH': str(commands) + ':/usr/bin:/bin:/usr/sbin:/sbin', 'HOME': str(root / 'home')}
    logs = root / 'home/Library/Logs/PendingCrew'

    def invoke(source):
        result = subprocess.run(['/bin/sh', str(script), str(source)], env=env,
                                capture_output=True, text=True, timeout=10)
        assert result.returncode != 0
        return result

    helper = installed / 'Contents/Helpers/crewd'
    helper.symlink_to(root / 'missing')
    assert '保护流程' in invoke(incoming).stderr and not logs.exists()
    helper.unlink()
    helper = incoming / 'Contents/Helpers/crewd'
    helper.touch()
    assert '保护流程' in invoke(incoming).stderr and not logs.exists()
    zip_source = root / 'zip-source/PendingCrew.app'
    zip_source.parent.mkdir()
    shutil.copytree(incoming, zip_source)
    archive = root / 'incoming.zip'
    subprocess.run(['/usr/bin/ditto', '-c', '-k', '--keepParent', str(zip_source), str(archive)],
                   check=True, capture_output=True)
    invoke(archive)
    assert any('保护流程' in log.read_text() for log in logs.iterdir())
    helper.unlink()
    invoke(incoming)
    assert any('fixture signature gate reached' in log.read_text() for log in logs.iterdir())
    print('PASS legacy installer refuses installed/incoming embedded Core before live operations')


def native_lock_probe(core, expected, root):
    assert core.is_absolute() and core.resolve() == core and sha(core) == expected
    data = root / 'real-core'
    home = root / 'real-core-home'
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


def regression_21_to33():
    """Run unchanged CLI verbs first, so old code fails on real schema 33.

    This changes a synthetic SQLite header as an installer-contract fixture;
    it does not claim that a real Core migration or original RPC was exercised.
    """
    with tempfile.TemporaryDirectory(prefix='pcw-upgrade-33-red-', dir='/private/tmp') as temporary:
        root = Path(temporary)
        refs = source_evidence(root)
        old = ordinary(root / 'Current.app', refs[21])
        incoming = ordinary(root / 'Incoming.app', refs[33])
        packages = [json.loads(run('inspect-package', '--app', app, '--requirement', REQUIREMENT))
                    for app in (old, incoming)]
        data = root / 'authority'
        data.mkdir(mode=0o700)
        (data / 'crewd.lock').touch()
        (data / 'crewd.db.id').write_text('original-generation-id\n')
        sqlite(data / 'crewd.db', 'PRAGMA application_id=0x43525744', 'PRAGMA user_version=21',
               'CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT)',
               "INSERT INTO meta VALUES('dbId','original-generation-id'),('lifecycle.clean','true')")
        before = contents(data)
        setup = root / 'prepared'
        mode = ('--isolated-fixture', root)
        run('prepare', '--authority', data, '--old-app', old, '--new-app', incoming,
            '--old-source', packages[0]['source'], '--new-source', packages[1]['source'],
            '--old-digest', packages[0]['bundleDigest'], '--new-digest', packages[1]['bundleDigest'],
            '--requirement', REQUIREMENT, '--out', setup, *mode)
        plan = setup / 'plan.json'
        plan_sha = sha(plan)
        old.rename(root / 'PreservedOld.app')
        shutil.copytree(incoming, old, symlinks=True)
        sqlite(data / 'crewd.db', 'PRAGMA user_version=33')
        (data / 'new-generation-only').write_text('must be preserved separately')
        upgraded = contents(data)
        stage = root / 'rollback-stage'
        # The original production helper reaches its hard-coded inspect(...,22)
        # and refuses this actual 33 header, rather than failing on a new flag.
        run('stage-rollback', '--authority', data, '--upgraded-app', old,
            '--plan', plan, '--plan-sha', plan_sha, '--out', stage, *mode)
        record = json.loads((stage / 'rollback.json').read_text())
        assert record['format'] == 2 and record['schemaEvidence']['new']['schemaVersion'] == 33
        assert contents(stage / 'restore') == before
        assert contents(stage / 'preserved') == contents(data) == upgraded
        assert sha(plan) == plan_sha
        print('PASS original CLI verbs preserve exact 21 and 33 generations for rollback')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--core', type=Path)
    parser.add_argument('--core-sha')
    parser.add_argument('--red-21-to33', action='store_true', help='focused old-production RED before source changes')
    args = parser.parse_args()
    if bool(args.core) != bool(args.core_sha):
        parser.error('--core and --core-sha are required together')
    if args.red_21_to33:
        regression_21_to33()
        return
    with tempfile.TemporaryDirectory(prefix='pcw-local-stage-', dir='/private/tmp') as temporary:
        root = Path(temporary)
        refs = source_evidence(root)
        old = ordinary(root / 'OldSource.app', refs[21])
        new = ordinary(root / 'NewSource.app', refs[33])
        native = ordinary(root / 'Native.app', '3' * 40, native=True)
        delivery_policy(new)
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
        run(*base, '--new-digest', new_package['bundleDigest'], '--out', root / 'unqualified-distribution',
            refusal='ordinary upgrade requires timestamped Developer ID Application signing')
        assert not (root / 'unqualified-distribution').exists()
        mode = ('--isolated-fixture', root)
        base += mode
        prepare = (*base, '--new-digest', new_package['bundleDigest'])
        original = contents(data)
        for side, value in (('old', '22'), ('new', '22'), ('new', '999')):
            out = root / ('wrong-' + side + '-schema-' + value)
            run(*prepare, '--' + side + '-schema', value, '--out', out,
                refusal='explicit schema assertion differs from frozen source')
            assert not out.exists() and contents(data) == original
        run(*prepare, '--new-source', 'f' * 40, '--out', root / 'wrong-source',
            refusal='ordinary App source stamp changed')
        assert not (root / 'wrong-source').exists() and contents(data) == original
        escaped = root.parent / (root.name + '-outside')
        run(*prepare, '--out', escaped, refusal='fixture path escapes private root')
        assert not escaped.exists()
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
        assert contents(data) == contents(preparation / 'before') == before
        plan = preparation / 'plan.json'
        recorded = json.loads(plan.read_text())
        assert recorded['format'] == 2 and recorded['schemaEvidence']['old']['schemaVersion'] == 21
        assert recorded['schemaEvidence']['new']['schemaVersion'] == 33
        assert recorded['schemaEvidence']['old']['source'] == refs[21]
        assert recorded['schemaEvidence']['new']['source'] == refs[33]
        for field, value in (('schemaVersion', 999), ('schemaSourceSHA256', '0' * 64), ('source', refs[22])):
            altered = json.loads(plan.read_text())
            altered['schemaEvidence']['new'][field] = value
            bad = root / ('changed-' + field)
            bad.mkdir()
            bad_plan = bad / 'plan.json'
            bad_plan.write_text(json.dumps(altered))
            out = bad / 'rollback'
            run('stage-rollback', '--authority', data, '--plan', bad_plan, '--plan-sha', sha(bad_plan),
                '--upgraded-app', old, '--out', out, *mode,
                refusal='frozen source/schema evidence changed')
            assert not out.exists() and contents(data) == before
        rollback = ('stage-rollback', '--authority', data, '--plan', plan, '--plan-sha', sha(plan))
        run(*rollback, '--upgraded-app', old, '--out', root / 'fixture-as-production',
            refusal='preparation mode or fixture root differs from invocation')
        rollback += mode
        run(*rollback, '--upgraded-app', old, '--out', root / 'wrong-rollback-app',
            refusal='upgraded App differs from preparation')
        old.rename(root / 'FormerActive.app')
        shutil.copytree(new, old, symlinks=True)
        assert contents(old) == contents(new)
        rollback += ('--upgraded-app', old)
        run(*rollback, '--out', root / 'old-only', refusal='unsupported Core database generation')
        sqlite(data / 'crewd.db', 'PRAGMA user_version=33', 'CREATE TABLE newer(text TEXT)')
        sqlite(data / 'knowledge/example.sqlite', "UPDATE content SET text='after cutoff'")
        (data / 'attachments/new').write_bytes(b'added after cutoff')
        upgraded = contents(data)
        run('stage-rollback', '--authority', data, '--plan', plan, '--plan-sha', sha(plan),
            '--upgraded-app', new, '--out', root / 'wrong-rollback-path', *mode,
            refusal='upgraded App path differs from preparation')
        assert not (root / 'wrong-rollback-path').exists()
        staged = root / 'staged-rollback'
        run(*rollback, '--out', staged)
        assert contents(staged / 'restore') == before
        assert contents(staged / 'preserved') == contents(data) == upgraded
        assert contents(preparation / 'before') == before
        (preparation / 'before/attachments/example').write_bytes(b'modified evidence')
        run(*rollback, '--out', root / 'tampered', refusal='sealed pre-cut authority changed')
        assert contents(data) == upgraded
        print('PASS source-bound Core21→33 preparation, schema/blob refusals, lock/sidecar and rollback staging')
        legacy_guard(root)
        if args.core:
            native_lock_probe(args.core, args.core_sha, root)


if __name__ == '__main__':
    main()
