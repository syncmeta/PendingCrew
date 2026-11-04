#!/usr/bin/env python3
"""Verify the normal Mac Core21/22 cutover in a fresh private authority."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import subprocess
import sys
import time
import uuid

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent / 'fixtures'))
from crewd_recovery_common import Public, birth, save, wait_for


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inventory(root):
    rows = {}
    for path in sorted(root.rglob('*')):
        mode = path.lstat().st_mode
        if stat.S_ISREG(mode):
            rows[str(path.relative_to(root))] = (sha(path), stat.S_IMODE(mode))
        elif not stat.S_ISDIR(mode):
            raise ValueError(f'unexpected file type in stopped authority: {path}')
    return rows


def database(path):
    for suffix in ('-wal', '-shm', '-journal'):
        assert not Path(str(path) + suffix).exists(), 'inspection requires a clean database cut'
    with contextlib.closing(sqlite3.connect(path.as_uri() + '?immutable=1', uri=True)) as db:
        version = db.execute('PRAGMA user_version').fetchone()[0]
        identity = db.execute("SELECT value FROM meta WHERE key='dbId'").fetchone()[0]
        assert db.execute('PRAGMA integrity_check').fetchall() == [('ok',)]
        assert not db.execute('PRAGMA foreign_key_check').fetchall()
        index = db.execute(
            "SELECT sql FROM sqlite_master WHERE name='messages_stop_escalations_seq' AND type='index'"
        ).fetchone()
        return version, identity, index


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ('old', 'old-sha', 'out'):
        parser.add_argument('--' + option, required=True)
    parser.add_argument('--new')
    parser.add_argument('--new-sha')
    args = parser.parse_args()
    if bool(args.new) != bool(args.new_sha):
        parser.error('--new and --new-sha are required together')

    def binary(raw, expected):
        path = Path(raw)
        if not path.is_absolute() or path.is_symlink() or path.resolve() != path or not path.is_file():
            parser.error('an absolute, canonical normal executable is required')
        if not os.access(path, os.X_OK) or sha(path) != expected:
            parser.error('normal executable hash or execute mode mismatch')
        return path

    old = binary(args.old, args.old_sha)
    new = binary(args.new, args.new_sha) if args.new else None
    out = Path(args.out)
    if not out.is_absolute() or out.parent.resolve() != Path('/private/tmp'):
        parser.error('use a fresh evidence directory directly under /private/tmp')
    if len(os.fsencode(out / 'backup-obstacle/crewd.sock')) > 103:
        parser.error('isolated socket path is too long')
    out.mkdir(mode=0o700)
    data, home, work = (out / name for name in ('data', 'home', 'work'))
    for folder in (data, home, work):
        folder.mkdir(mode=0o700)
    db = data / 'crewd.db'
    env = {
        'PATH': '/usr/bin:/bin:/usr/sbin:/sbin',
        'HOME': str(home),
        'XDG_CACHE_HOME': str(home / 'cache'),
        'CLAUDE_CONFIG_DIR': str(home / 'claude'),
        'TMPDIR': str(out),
        'SHELL': '/usr/bin/false',
    }

    def argv(program, authority, *command):
        return [str(program), '--data-dir', str(authority), *command]

    def serve_argv(program, authority):
        return argv(program, authority, 'serve', '--codex', '/usr/bin/false', '--claude', '/usr/bin/false')

    def cli(program, authority, label, *command, refusal=None):
        result = subprocess.run(argv(program, authority, *command), env=env, capture_output=True, timeout=20)
        (out / (label + '.log')).write_bytes(result.stdout + result.stderr)
        if refusal:
            assert result.returncode and refusal in result.stderr.decode(), label
        else:
            assert result.returncode == 0, label
        return result.stdout.decode().strip()

    def rejected_serve(program, authority, label, *reasons):
        result = subprocess.run(serve_argv(program, authority), env=env, capture_output=True, timeout=20)
        output = (result.stdout + result.stderr).decode()
        (out / (label + '.log')).write_text(output)
        assert result.returncode and all(reason in output for reason in reasons), label
        assert not (authority / 'crewd.sock').exists(), label

    @contextlib.contextmanager
    def serve(program, authority, label):
        endpoint = authority / 'crewd.sock'
        assert not endpoint.exists(), label
        with (out / (label + '.log')).open('wb') as log:
            process = subprocess.Popen(
                serve_argv(program, authority), env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log
            )
        client = None
        try:
            def ready():
                assert process.poll() is None, label
                owner = birth(process.pid)
                if endpoint.exists() and owner is not None:
                    assert owner['ppid'] == os.getpid() and owner['uid'] == os.getuid()
                    return owner
                return None

            client = Public(endpoint, wait_for(ready, label))
            yield client
            quit_request = client.prepared('crewd.hibernate', {'deadlineSecs': 0})
            assert client.submit(quit_request)['requestId'] == quit_request['requestId']
            assert process.wait(timeout=15) == 0, label
            assert not endpoint.exists(), label
        finally:
            if client is not None:
                client.close()
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)

    with serve(old, data, 'old21-create') as client:
        first = client.prepared('crew.createFirstMessage', {
            'crew': {'title': 'Core21 protected cutover', 'workingDirectory': str(work)},
            'text': 'Original Core21 first message',
        })
        client.submit(first)
        committed = client.phase(first, ['committed'])
        crew = committed['result']['crew']['id']
        requirement = client.rpc('todo.create', {
            'requestId': str(uuid.uuid4()), 'crewId': crew, 'text': 'Original Core21 requirement',
        })['requirementId']
        work_id = client.rpc('work.create', {
            'crewId': crew, 'title': 'Core21 plan', 'sources': [requirement],
        })['id']
        context = client.rpc('data.workspace.get', {'crewId': crew})['context']
        knowledge = client.rpc('data.submit', {
            'requestId': str(uuid.uuid4()), 'context': context, 'change': {
                'kind': 'apply', 'message': 'Original Core21 knowledge', 'edits': [{
                    'kind': 'createBlock', 'alias': 'original', 'contentType': 'text/plain',
                    'content': 'Original Core21 durable content', 'sources': [], 'attributes': {},
                }],
            },
        })
        reference = knowledge['created']['original']['object']['reference']
    old_version, identity, index = database(db)
    assert old_version == 21 and index is None
    manual = Path(cli(old, data, 'old21-backup', 'backup'))
    assert manual.parent == data / 'backups' and database(manual) == (21, identity, None)
    sealed = out / 'before21'
    shutil.copytree(data, sealed)
    baseline = inventory(data)
    assert inventory(sealed) == baseline
    report = {
        'oldSHA256': args.old_sha, 'newSHA256': args.new_sha, 'dbId': identity, 'crewId': crew,
        'requestId': first['requestId'], 'requirementId': requirement, 'workId': work_id,
        'knowledgeReference': reference, 'old21Backup': str(manual), 'before21': baseline,
        'providers': 'disabled with /usr/bin/false', 'applicationLaunched': False,
    }
    if new is None:
        save(out / 'baseline.json', report)
        print('PASS actual old Core21 private authority, stopped and sealed:', out)
        return

    def verify(client):
        assert client.identity['dbId'] == identity
        receipt = client.lookup(first)
        assert receipt['phase'] == 'committed' and receipt['result'] == committed['result']
        assert client.rpc('todo.get', {'id': requirement})['text'] == 'Original Core21 requirement'
        assert client.rpc('work.get', {'id': work_id})['sources'] == [requirement]
        current = client.rpc('data.workspace.get', {'crewId': crew})['context']
        block = client.rpc('data.object.get', {'context': current, 'reference': reference})['object']
        assert block['content'] == 'Original Core21 durable content'

    failure = out / 'backup-obstacle'
    shutil.copytree(sealed, failure)
    (failure / 'backups').rename(out / 'obstacle-backups-retained')
    (failure / 'backups').write_text('intentional protective backup obstruction')
    before_failure = inventory(failure)
    rejected_serve(new, failure, 'new22-backup-refusal', 'backups')
    assert inventory(failure) == before_failure and database(failure / 'crewd.db') == (21, identity, None)

    with serve(new, data, 'new22-upgrade') as client:
        verify(client)
        client.rpc('todo.create', {'requestId': str(uuid.uuid4()), 'crewId': crew, 'text': 'Post-cutover evidence'})
    version, restored_identity, index = database(db)
    assert (version, restored_identity) == (22, identity) and index and "category='work_stop_escalation'" in index[0]
    protected, = (data / 'backups').glob('crewd-v21-pre-v22-*.db')
    assert database(protected) == (21, identity, None)
    cli(new, data, 'new22-integrity', 'check')
    physical = inventory(data)
    rejected_serve(old, data, 'old21-refuses22', '22', '21', '拒绝')
    assert inventory(data) == physical
    cli(new, data, 'knowledge-refuses-core-only-restore', 'restore', '--from', str(protected),
        refusal='Core-only restore cannot replace a knowledge workspace')
    assert inventory(data) == physical
    with serve(new, data, 'new22-reopen') as client:
        verify(client)

    assert inventory(sealed) == baseline
    data.rename(out / 'upgraded22-retained')
    shutil.copytree(sealed, data)
    assert inventory(data) == baseline
    with serve(old, data, 'old21-whole-authority-reopen') as client:
        verify(client)
    assert database(db) == (21, identity, None)
    assert database(out / 'upgraded22-retained/crewd.db')[0] == 22
    assert inventory(sealed) == baseline
    assert sha(old) == args.old_sha and sha(new) == args.new_sha
    report.update(protectedBackup=str(out / 'upgraded22-retained' / protected.relative_to(data)),
                  protectedBackupSHA256=sha(out / 'upgraded22-retained' / protected.relative_to(data)),
                  oldHelperRefused22=True, coreOnlyRestoreRefused=True, wholeAuthorityOld21Reopened=True)
    save(out / 'result.json', report)
    print('PASS isolated Core21→22 migration and whole-authority Core21 reopen:', out)


if __name__ == '__main__':
    main()
