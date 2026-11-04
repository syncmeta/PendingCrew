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


def exercise(root, fail_at):
    root.mkdir()
    old = fixture.ordinary(root / 'Current.app', '1' * 40)
    incoming = fixture.ordinary(root / 'New.app', '2' * 40)
    probe = old / 'Contents/Helpers/activity-probe'
    if fail_at == 0:
        shutil.copyfile('/bin/sleep', probe)
        probe.chmod(0o755)
        subprocess.run(['/usr/bin/codesign', '--force', '--deep', '--sign', '-', str(old)],
                       check=True, capture_output=True)
    old_package, new_package = [
        json.loads(fixture.run('inspect-package', '--app', app, '--requirement', fixture.REQUIREMENT))
        for app in (old, incoming)
    ]
    data = root / 'authority'
    data.mkdir(mode=0o700)
    (data / 'crewd.lock').touch()
    (data / 'crewd.db.id').write_text('fixture-generation-id\n')
    fixture.sqlite(data / 'crewd.db', 'PRAGMA application_id=0x43525744', 'PRAGMA user_version=21',
                   'CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT)',
                   "INSERT INTO meta VALUES('dbId','fixture-generation-id'),('lifecycle.clean','true')")
    (data / 'attachments').mkdir()
    (data / 'attachments/original').write_text('pre-cutover content')
    before = fixture.contents(data)
    app_before = fixture.contents(old)
    app_after = fixture.contents(incoming)
    setup = root / 'prepared'
    fixture.run('prepare', '--authority', data, '--old-app', old, '--new-app', incoming,
                '--old-source', old_package['source'], '--new-source', new_package['source'],
                '--old-digest', old_package['bundleDigest'], '--new-digest', new_package['bundleDigest'],
                '--requirement', fixture.REQUIREMENT, '--out', setup)
    plan = setup / 'plan.json'
    common = ('--plan', plan, '--plan-sha', fixture.sha(plan), '--app', old, '--authority', data)

    def check_transaction(operation, output, *args):
        run('start', operation, *common, *args, '--out', output, crash=fail_at)
        intent = output / 'intent.json'
        assert intent.exists() and not (output / 'complete.json').exists()
        record = json.loads(intent.read_text())
        order = ['data', 'app'] if operation == 'upgrade' else ['app', 'data']
        for index, name in enumerate(order):
            live = old if name == 'app' else data
            assert live.stat().st_ino == record[name + 'Inodes'][index < fail_at][1]
        retry = ('resume', '--transaction', output, '--intent-sha', fixture.sha(intent))
        run('resume', '--transaction', output, '--intent-sha', '0' * 64, fail='record digest differs')
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
    fixture.sqlite(data / 'crewd.db', 'PRAGMA user_version=22')
    (data / 'attachments/after').write_text('post-cutover content')
    upgraded = fixture.contents(data)
    rollback_stage = root / 'rollback-stage'
    fixture.run('stage-rollback', '--authority', data, '--upgraded-app', old,
                '--plan', plan, '--plan-sha', fixture.sha(plan), '--out', rollback_stage)
    rollback_record = rollback_stage / 'rollback.json'
    rollback = root / 'rollback'
    check_transaction('rollback', rollback, '--rollback', rollback_record, '--rollback-sha', fixture.sha(rollback_record))
    assert fixture.contents(data) == before
    assert fixture.contents(old) == app_before
    assert fixture.contents(rollback / 'staged-data') == upgraded
    assert fixture.contents(rollback / 'staged.app') == app_after
    print(f'PASS upgrade and rollback recover after {fail_at} completed exchanges')


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == 'fault-child':
        fault_child()
    else:
        with tempfile.TemporaryDirectory(prefix='pcw-cutover-', dir='/private/tmp') as temporary:
            for fail_at in range(3):
                exercise(Path(temporary) / str(fail_at), fail_at)
