#!/usr/bin/env python3
"""Crash-recovery fixture for ordinary App and data-generation exchanges."""
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
fixture_spec = importlib.util.spec_from_file_location(
    'preparation_fixture', Path(__file__).with_name('test-crewd-local-upgrade-preparation.py'))
fixture = importlib.util.module_from_spec(fixture_spec)
fixture_spec.loader.exec_module(fixture)
COMMIT = fixture.ROOT / 'scripts/release/commit-local-upgrade.py'


def run(*args, fail=None, crash=None):
    if crash is None:
        command = [sys.executable, str(COMMIT)]
    else:
        command = [sys.executable, __file__, 'fault-child', str(crash)]
    result = subprocess.run([*command, *map(str, args)], capture_output=True, text=True, timeout=30)
    if crash is not None:
        assert result.returncode == 79, (result.stdout, result.stderr)
    elif fail is not None:
        assert result.returncode != 0 and fail in result.stderr, result
    else:
        assert result.returncode == 0, result.stderr
    return result


def fault_child():
    module_spec = importlib.util.spec_from_file_location('local_cutover', COMMIT)
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    stop_at = int(sys.argv[2])
    real = module.exchange
    calls = 0

    def exchange(active, staged):
        nonlocal calls
        if calls == stop_at == 0:
            os._exit(79)
        real(active, staged)
        calls += 1
        if calls == stop_at:
            os._exit(79)

    module.exchange = exchange
    sys.argv = [str(COMMIT), *sys.argv[3:]]
    module.main()


def exercise(root, fail_at, refs, new_version=33, legacy=False, unmigrated=False,
             old_version=21, new_source=None):
    root.mkdir()
    mode = ('--isolated-fixture', root.parent)
    new_source = new_source or refs[new_version]
    old = fixture.ordinary(root / 'Current.app', refs[old_version])
    incoming = fixture.ordinary(root / 'New.app', new_source)
    probe = old / 'Contents/Helpers/activity-probe'
    if fail_at == 0:
        for app in (old, incoming):
            target = app / 'Contents/Helpers/activity-probe'
            shutil.copyfile('/bin/sleep', target)
            target.chmod(0o755)
            subprocess.run(['/usr/bin/codesign', '--force', '--deep', '--sign', '-', str(app)],
                           check=True, capture_output=True)
    old_package, new_package = [
        json.loads(fixture.run('inspect-package', '--app', app, '--requirement', fixture.REQUIREMENT))
        for app in (old, incoming)
    ]
    data = root / 'authority'
    data.mkdir(mode=0o700)
    (data / 'crewd.lock').touch()
    (data / 'crewd.db.id').write_text('fixture-generation-id\n')
    fixture.sqlite(data / 'crewd.db', 'PRAGMA application_id=0x43525744',
                   'PRAGMA user_version=' + str(old_version),
                   'CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT)',
                   "INSERT INTO meta VALUES('dbId','fixture-generation-id'),('lifecycle.clean','true')")
    (data / 'attachments').mkdir()
    (data / 'attachments/original').write_text('pre-cutover content')
    before = fixture.contents(data)
    app_before = fixture.contents(old)
    app_after = fixture.contents(incoming)
    setup = root / 'prepared'
    prepare_args = ('prepare', '--authority', data, '--old-app', old, '--new-app', incoming,
                    '--old-source', old_package['source'], '--new-source', new_package['source'],
                    '--old-digest', old_package['bundleDigest'], '--new-digest', new_package['bundleDigest'],
                    '--requirement', fixture.REQUIREMENT, '--out', setup, *mode)
    if old_version == new_version and fail_at == 0:
        for label, ref, expected in [('same-source', refs[old_version], 'distinct source generations required'),
                                     ('downgrade', refs[22], 'target source must not downgrade the Core generation')]:
            refused_app = fixture.ordinary(root / (label + '.app'), ref)
            package = json.loads(fixture.run('inspect-package', '--app', refused_app,
                                             '--requirement', fixture.REQUIREMENT))
            output = root / (label + '-preparation')
            fixture.run(*prepare_args, '--new-app', refused_app, '--new-source', ref,
                        '--new-digest', package['bundleDigest'], '--out', output, refusal=expected)
            assert not output.exists() and fixture.contents(data) == before
    fixture.run(*prepare_args)
    plan = setup / 'plan.json'
    if legacy:
        # Materialize the exact original format-1 receipt layout as a separate
        # fixture. Production never rewrites or upgrades existing receipts.
        assert new_version == 22
        record = json.loads(plan.read_text())
        record['format'] = 1
        record['before21'] = record.pop('before')
        del record['schemaEvidence']
        (setup / 'before').rename(setup / 'before21')
        plan.write_text(json.dumps(record))
    identity_args = ('--plan', plan, '--plan-sha', fixture.sha(plan), '--authority', data, *mode)
    common = (*identity_args, '--app', old)

    def refuse_wrong_app_path(operation, *args):
        alternate = root / (operation + '-alternate.app')
        shutil.copytree(old, alternate, symlinks=True)
        assert fixture.contents(alternate) == fixture.contents(old)
        output = root / (operation + '-wrong-path')
        current = fixture.contents(data)
        run('start', operation, *identity_args, '--app', alternate, *args, '--out', output,
            fail='active App path differs from preparation')
        assert not output.exists() and fixture.contents(data) == current

    def check_transaction(operation, output, *args):
        run('start', operation, *common, *args, '--out', output, crash=fail_at)
        intent = output / 'intent.json'
        assert intent.exists() and not (output / 'complete.json').exists()
        record = json.loads(intent.read_text())
        expected_format = 3 if unmigrated and operation == 'rollback' else (1 if legacy else 2)
        assert record['format'] == expected_format
        if expected_format == 3:
            assert record['rollbackState'] == 'unmigrated' and record['coreVersions'] == [old_version, old_version]
        if old_version == new_version and operation == 'rollback':
            original_record = intent.read_bytes()
            altered = json.loads(original_record)
            altered['format'] = 3
            altered['rollbackState'] = 'unmigrated'
            intent.write_text(json.dumps(altered))
            locations = (old.stat().st_ino, data.stat().st_ino)
            run('resume', '--transaction', output, '--intent-sha', fixture.sha(intent), *mode,
                fail='unmigrated rollback requires distinct schema generations')
            assert (old.stat().st_ino, data.stat().st_ino) == locations
            intent.write_bytes(original_record)
        if not legacy:
            assert record['schemaEvidence']['new']['source'] == new_source
            assert record['schemaEvidence']['new']['schemaVersion'] == new_version
            # A freshly hashed but internally inconsistent intent must still
            # fail closed before either generation moves.
            for field, value in (('schemaVersion', 999), ('schemaSourceSHA256', '0' * 64),
                                 ('source', refs[22]), ('coreVersions', [999, 21])):
                original_record = intent.read_bytes()
                altered = json.loads(original_record)
                if field == 'coreVersions':
                    altered[field] = value
                else:
                    altered['schemaEvidence']['new'][field] = value
                intent.write_text(json.dumps(altered))
                locations = (old.stat().st_ino, data.stat().st_ino)
                run('resume', '--transaction', output, '--intent-sha', fixture.sha(intent), *mode,
                    fail='cutover source/schema evidence changed')
                assert (old.stat().st_ino, data.stat().st_ino) == locations
                intent.write_bytes(original_record)
        order = ['data', 'app'] if operation == 'upgrade' else ['app', 'data']
        for index, name in enumerate(order):
            live = old if name == 'app' else data
            assert live.stat().st_ino == record[name + 'Inodes'][index < fail_at][1]
        retry = ('resume', '--transaction', output, '--intent-sha', fixture.sha(intent))
        run(*retry, fail='preparation mode or fixture root differs from invocation')
        retry += mode
        run('resume', '--transaction', output, '--intent-sha', '0' * 64, *mode, fail='record digest differs')
        with (data / 'crewd.lock').open('rb') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            run(*retry, fail='Core authority is still in use')
        with (output / 'staged-data/crewd.lock').open('rb') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            run(*retry, fail='Core authority is still in use')
        marker = data / 'attachments/original'
        original = marker.read_bytes()
        marker.write_bytes(b'external change after interruption')
        run(*retry, fail='data generation changed after intent')
        marker.write_bytes(original)
        (output / '.record-interrupted').write_text('partial unpublished record')
        run(*retry)
        assert (output / 'complete.json').exists()
        assert 'Already completed' in run(*retry).stdout

    upgrade = root / 'upgrade'
    if fail_at == 0:
        run('start', 'upgrade', '--plan', plan, '--plan-sha', fixture.sha(plan), '--authority', data,
            '--app', old, '--new-app', incoming, '--out', upgrade,
            fail='preparation mode or fixture root differs from invocation')
        assert not upgrade.exists()
        refuse_wrong_app_path('upgrade', '--new-app', incoming)
        process = subprocess.Popen([str(probe), '30'])
        try:
            run('start', 'upgrade', *common, '--new-app', incoming, '--out', upgrade,
                fail='an affected App process is still running')
            assert not upgrade.exists()
        finally:
            process.terminate()
            process.wait(timeout=5)
    check_transaction('upgrade', upgrade, '--new-app', incoming)
    assert fixture.contents(data) == fixture.contents(upgrade / 'staged-data') == before
    assert fixture.contents(old) == app_after
    assert fixture.contents(upgrade / 'staged.app') == app_before
    if not unmigrated:
        fixture.sqlite(data / 'crewd.db', 'PRAGMA user_version=' + str(new_version))
    (data / 'attachments/after').write_text('post-cutover content')
    upgraded = fixture.contents(data)
    # A known completed original intent is historical evidence, never a fresh
    # exchange after the upgraded authority has changed or migrated.
    assert 'Already completed' in run('resume', '--transaction', upgrade,
        '--intent-sha', fixture.sha(upgrade / 'intent.json'), *mode).stdout
    assert fixture.contents(data) == upgraded
    rollback_stage = root / 'rollback-stage'
    if old_version == new_version:
        fixture.run('stage-rollback', '--authority', data, '--upgraded-app', old,
                    '--plan', plan, '--plan-sha', fixture.sha(plan), '--out', rollback_stage,
                    '--unmigrated', *mode,
                    refusal='unmigrated rollback requires distinct schema generations')
        assert not rollback_stage.exists() and fixture.contents(data) == upgraded
    if unmigrated:
        # The original migrated-only verb still refuses actual old-schema
        # data. The new branch is explicit, never guessed from the new App.
        fixture.run('stage-rollback', '--authority', data, '--upgraded-app', old,
                    '--plan', plan, '--plan-sha', fixture.sha(plan), '--out', rollback_stage,
                    *mode, refusal='unsupported Core database generation')
        assert not rollback_stage.exists() and fixture.contents(data) == upgraded
        if fail_at == 0:
            stage_args = ('stage-rollback', '--authority', data, '--upgraded-app', old,
                          '--plan', plan, '--plan-sha', fixture.sha(plan), '--unmigrated', *mode)

            def refused(label, expected):
                output = root / ('refused-unmigrated-' + label)
                current, package = fixture.contents(data), fixture.contents(old)
                fixture.run(*stage_args, '--out', output, refusal=expected)
                assert not output.exists()
                assert fixture.contents(data) == current and fixture.contents(old) == package

            database = data / 'crewd.db'
            original_database = database.read_bytes()
            for version in (22, 32, 33, 34):
                fixture.sqlite(database, 'PRAGMA user_version=' + str(version))
                refused('schema-' + str(version), 'unsupported Core database generation')
                database.write_bytes(original_database)
            for key, value in (('lifecycle.clean', 'false'), ('dbId', 'another-database')):
                fixture.sqlite(database, "UPDATE meta SET value='" + value + "' WHERE key='" + key + "'")
                refused(key, 'Core identity or clean shutdown is unconfirmed')
                database.write_bytes(original_database)
            for suffix in ('-wal', '-shm', '-journal'):
                sidecar = data / ('crewd.db' + suffix)
                sidecar.write_bytes(b'fixture unfinished state')
                refused(suffix, 'SQLite sidecars')
                sidecar.unlink()
            registry = data / 'exec-registry.json'
            registry.write_text('{"unresolved":{}}')
            refused('registry', 'unresolved executor process obligations')
            registry.unlink()
            with (data / 'crewd.lock').open('rb') as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                refused('live-lock', 'Core authority is still in use')
            lock = data / 'crewd.lock'
            lock.rename(data / 'real-lock')
            lock.symlink_to(data / 'real-lock')
            refused('aliased-lock', 'crewd.lock')
            lock.unlink()
            (data / 'real-lock').rename(lock)
            sealed_marker = setup / 'before/attachments/original'
            original_marker = sealed_marker.read_bytes()
            sealed_marker.write_text('changed sealed old authority')
            refused('sealed-snapshot', 'sealed pre-cut authority changed')
            sealed_marker.write_bytes(original_marker)
            assert fixture.contents(data) == upgraded
    fixture.run('stage-rollback', '--authority', data, '--upgraded-app', old,
                '--plan', plan, '--plan-sha', fixture.sha(plan), '--out', rollback_stage, *mode,
                *(('--unmigrated',) if unmigrated else ()))
    rollback_record = rollback_stage / 'rollback.json'
    rollback = root / 'rollback'
    if fail_at == 0:
        if old_version == new_version:
            original_record = rollback_record.read_bytes()
            altered = json.loads(original_record)
            altered['format'] = 3
            altered['rollbackState'] = 'unmigrated'
            rollback_record.write_text(json.dumps(altered))
            run('start', 'rollback', *common, '--rollback', rollback_record,
                '--rollback-sha', fixture.sha(rollback_record), '--out', rollback,
                fail='unmigrated rollback requires distinct schema generations')
            assert not rollback.exists() and fixture.contents(data) == upgraded
            rollback_record.write_bytes(original_record)
        refuse_wrong_app_path('rollback', '--rollback', rollback_record, '--rollback-sha', fixture.sha(rollback_record))
        process = subprocess.Popen([str(probe), '30'])
        try:
            run('start', 'rollback', *common, '--rollback', rollback_record,
                '--rollback-sha', fixture.sha(rollback_record), '--out', rollback,
                fail='an affected App process is still running')
            assert not rollback.exists()
        finally:
            process.terminate()
            process.wait(timeout=5)
    check_transaction('rollback', rollback, '--rollback', rollback_record, '--rollback-sha', fixture.sha(rollback_record))
    assert fixture.contents(data) == before
    assert fixture.contents(old) == app_before
    assert fixture.contents(rollback / 'staged-data') == upgraded
    assert fixture.contents(rollback / 'staged.app') == app_after
    print(f'PASS format{1 if legacy else 2} Core{old_version}→{new_version} upgrade and '
          f'{"unmigrated21" if unmigrated else "migrated"} rollback recover after {fail_at} exchanges')


def same_schema_source(root, refs):
    repo = root / 'source-evidence'
    tree = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', refs[33] + '^{tree}'],
                                   text=True).strip()
    env = dict(os.environ, GIT_AUTHOR_NAME='PendingCrew fixture',
               GIT_AUTHOR_EMAIL='fixture@example.invalid', GIT_COMMITTER_NAME='PendingCrew fixture',
               GIT_COMMITTER_EMAIL='fixture@example.invalid')
    return subprocess.check_output(['git', '-c', 'commit.gpgsign=false', '-C', str(repo),
                                    'commit-tree', tree, '-p', refs[33], '-m',
                                    'synthetic UI successor with unchanged schema33'],
                                   env=env, text=True).strip()


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == 'fault-child':
        fault_child()
    else:
        with tempfile.TemporaryDirectory(prefix='pcw-cutover-', dir='/private/tmp') as temporary:
            refs = fixture.source_evidence(Path(temporary))
            same_source = same_schema_source(Path(temporary), refs)
            if len(sys.argv) > 1 and sys.argv[1] == '--red-same-schema':
                exercise(Path(temporary) / 'same-schema-red', 0, refs,
                         old_version=33, new_source=same_source)
                sys.exit(0)
            for fail_at in range(3):
                exercise(Path(temporary) / ('current-' + str(fail_at)), fail_at, refs)
                exercise(Path(temporary) / ('legacy-' + str(fail_at)), fail_at, refs, 22, legacy=True)
                exercise(Path(temporary) / ('unmigrated-' + str(fail_at)), fail_at, refs, unmigrated=True)
                exercise(Path(temporary) / ('same-schema-' + str(fail_at)), fail_at, refs,
                         old_version=33, new_source=same_source)
