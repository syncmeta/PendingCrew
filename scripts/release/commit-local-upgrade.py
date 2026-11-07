#!/usr/bin/env python3
"""Explicit, resumable exchange of a prepared ordinary Mac App and its data authority."""
import argparse
import contextlib
import ctypes
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import uuid

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('local_upgrade_preparation', Path(__file__).with_name('prepare-local-upgrade.py'))
preparation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(preparation)
require = preparation.require


def checked_record(path, expected, kind):
    require(preparation.directory(path.parent) == path.parent, 'canonical record required')
    preparation.plain_file(path)
    raw = path.read_bytes()
    require(hashlib.sha256(raw).hexdigest() == expected, 'record digest differs')
    value = json.loads(raw)
    require(type(value.get('format')) is int and value['format'] in (1, 2, 3)
            and value.get('kind') == kind, 'unsupported transaction record')
    return value


def identity(path):
    st = preparation.directory(path).lstat()
    return [st.st_dev, st.st_ino]


def assert_app_stopped(*paths):
    processes = subprocess.check_output(['/bin/ps', '-axww', '-o', 'comm='], text=True).splitlines()
    require(processes and not any(line.strip().startswith(str(root) + '/')
                                  for root in paths for line in processes),
            'an affected App process is still running')


def exchange(active, staged):
    require(identity(active)[0] == identity(staged)[0], 'same-volume atomic exchange required')
    libc = ctypes.CDLL('/usr/lib/libSystem.B.dylib', use_errno=True)
    swap = libc.renamex_np
    swap.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
    swap.restype = ctypes.c_int
    if swap(os.fsencode(active), os.fsencode(staged), 2) != 0:  # Darwin RENAME_SWAP
        number = ctypes.get_errno()
        raise OSError(number, os.strerror(number))
    sync_parents(active, staged)


def sync_parents(*paths):
    for parent in {path.parent for path in paths}:
        preparation.sync(parent, os.O_DIRECTORY)


def publish_record(root, name, value):
    destination = root / name
    require(not os.path.lexists(destination), 'transaction record already exists')
    staged = root / ('.record-' + uuid.uuid4().hex)
    preparation.write_record(staged, value)
    staged.rename(destination)
    preparation.sync(root, os.O_DIRECTORY)


@contextlib.contextmanager
def journal_lease(root):
    with (root / 'intent.json').open('rb') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield


def paths_for(root, intent):
    return {
        'app': (Path(intent['app']), root / 'staged.app'),
        'data': (Path(intent['authority']), root / 'staged-data'),
    }


def location(paths, intent, name, side):
    pair = paths[name]
    original, staged = intent[name + 'Inodes']
    actual = [identity(path) for path in pair]
    require(actual in ([original, staged], [staged, original]), f'{name} directory identity is ambiguous')
    return pair[actual.index([original, staged][side])]


def verified(paths, intent, leases):
    preparation.intent_contract(intent)
    for side in range(2):
        app = location(paths, intent, 'app', side)
        data = location(paths, intent, 'data', side)
        leases[side](data)
        require(preparation.package(app, intent['requirement']) == intent['apps'][side],
                'App generation changed after intent')
        rows = preparation.inventory(data)
        require(preparation.digest(rows) == intent['dataDigests'][side]
                and preparation.inspect(data, rows, intent['coreVersions'][side]) == intent['dbId'],
                'data generation changed after intent')
    assert_app_stopped(*paths['app'])


def advance(root, intent, intent_sha, leases):
    paths = paths_for(root, intent)
    order = ['data', 'app'] if intent['operation'] == 'upgrade' else ['app', 'data']
    exchanged = [location(paths, intent, name, 1) == paths[name][0] for name in order]
    require(exchanged in ([False, False], [True, False], [True, True]), 'unsupported exchange order')
    verified(paths, intent, leases)
    if intent['operation'] == 'upgrade' and not all(exchanged):
        preparation.qualify_upgrade(location(paths, intent, 'app', 1), intent['fixtureRoot'])
    for name, swapped in zip(order, exchanged):
        if not swapped:
            exchange(*paths[name])
            verified(paths, intent, leases)
    sync_parents(*paths['app'], *paths['data'])
    publish_record(root, 'complete.json', {
        'format': intent['format'], 'kind': 'pendingcrew-local-cutover-completed',
        'intentSHA256': intent_sha, 'id': intent['id'], 'operation': intent['operation'],
        'activeApp': str(paths['app'][0]), 'activeAuthority': str(paths['data'][0]),
        'preservedApp': str(paths['app'][1]), 'preservedAuthority': str(paths['data'][1]),
        'openedApp': False,
    })
    print(f'Exchange complete: {root / "complete.json"}')


def start(args):
    extra = [path for path in (args.new_app, args.rollback) if path is not None]
    fixture = preparation.isolated_root(args, args.app, args.authority, args.plan, args.out, *extra)
    plan_path = Path(args.plan)
    plan = checked_record(plan_path, args.plan_sha, 'pendingcrew-local-upgrade-preparation')
    preparation.require_mode(plan, fixture)
    layout = preparation.plan_contract(plan)
    app = preparation.directory(args.app)
    data = preparation.directory(args.authority)
    require(str(app) == plan.get('app'), 'active App path differs from preparation')
    require(str(data) == plan['authority'] and not (app.is_relative_to(data) or data.is_relative_to(app)),
            'cutover paths differ from the prepared authority')
    old = preparation.directory(plan_path.parent / 'Old.app')
    sealed = preparation.directory(plan_path.parent / layout['before'])
    before = plan[layout['before']]
    require(preparation.package(old, plan['requirement']) == plan['oldApp']
            and preparation.inventory(sealed) == before
            and preparation.inspect(sealed, before, layout['old']) == plan['dbId'],
            'original preparation evidence changed')
    extra_inputs = [plan_path.parent]
    if args.operation == 'upgrade':
        require(args.new_app and not args.rollback and not args.rollback_sha, 'explicit new ordinary App required')
        incoming = preparation.directory(args.new_app)
        active_package, incoming_package = plan['oldApp'], plan['newApp']
        active_rows = staged_rows = before
        # Both stopped copies remain in the old generation until first launch.
        versions = [layout['old'], layout['old']]
    else:
        require(args.rollback and args.rollback_sha and not args.new_app, 'explicit rollback staging evidence required')
        rollback_path = Path(args.rollback)
        rollback = checked_record(rollback_path, args.rollback_sha, 'pendingcrew-local-rollback-staging')
        preparation.require_mode(rollback, fixture)
        rollback_layout = preparation.rollback_contract(rollback, plan)
        require(rollback['planSHA256'] == args.plan_sha and rollback.get('app') == plan['app']
                and rollback['dbId'] == plan['dbId']
                and rollback[layout['restore']] == before and rollback['oldApp'] == plan['oldApp']
                and rollback['upgradedApp'] == plan['newApp'], 'rollback staging has another generation')
        old = preparation.directory(rollback_path.parent / 'Old.app')
        sealed = preparation.directory(rollback_path.parent / layout['restore'])
        preserved = preparation.directory(rollback_path.parent / layout['preserved'])
        require(preparation.inventory(preserved) == rollback[layout['upgraded']]
                and preparation.inspect(preserved, rollback[layout['upgraded']], rollback_layout['outgoing']) == plan['dbId'],
                'preserved upgraded authority changed')
        incoming = old
        active_package, incoming_package = plan['newApp'], plan['oldApp']
        active_rows, staged_rows = rollback[layout['upgraded']], rollback[layout['restore']]
        versions = [rollback_layout['outgoing'], layout['old']]
        extra_inputs.append(rollback_path.parent)
    require(app != incoming and preparation.package(app, plan['requirement']) == active_package
            and preparation.package(incoming, plan['requirement']) == incoming_package,
            'active or incoming ordinary App does not match preparation')
    if args.operation == 'upgrade':
        preparation.qualify_upgrade(incoming, fixture)
    assert_app_stopped(app, incoming)
    with preparation.authority_lease(data) as check_original:
        require(preparation.inventory(data) == active_rows
                and preparation.inspect(data, active_rows, versions[0]) == plan['dbId'],
                'active authority changed after preparation')
        out = preparation.disjoint_out(args.out, app, data, incoming, *extra_inputs)
        staged_app, staged_data = out / 'staged.app', out / 'staged-data'
        preparation.copy_verified(incoming, staged_app, preparation.inventory(incoming, bundle=True), bundle=True)
        preparation.copy_verified(sealed, staged_data, staged_rows)
        with preparation.authority_lease(staged_data) as check_staged:
            intent = {
                'format': rollback['format'] if args.operation == 'rollback' else plan['format'],
                'kind': 'pendingcrew-local-cutover-intent',
                'fixtureRoot': fixture,
                'id': str(uuid.uuid4()), 'operation': args.operation, 'planSHA256': args.plan_sha,
                'app': str(app), 'authority': str(data), 'dbId': plan['dbId'],
                'appInodes': [identity(app), identity(staged_app)],
                'dataInodes': [identity(data), identity(staged_data)],
                'apps': [active_package, incoming_package], 'requirement': plan['requirement'],
                'coreVersions': versions, 'dataDigests': list(map(preparation.digest, [active_rows, staged_rows])),
            }
            if plan['format'] == 2:
                intent['schemaEvidence'] = plan['schemaEvidence']
            if intent['format'] == 3:
                intent['rollbackState'] = rollback['rollbackState']
            require(all(pair[0][0] == pair[1][0] for pair in (intent['appInodes'], intent['dataInodes'])),
                    'same-volume atomic exchange required for both generations')
            leases = (check_original, check_staged)
            verified(paths_for(out, intent), intent, leases)
            publish_record(out, 'intent.json', intent)
            preparation.sync(out.parent, os.O_DIRECTORY)
            intent_sha = preparation.sha(out / 'intent.json')
            print(f'Durable intent {out / "intent.json"} SHA256 {intent_sha}', flush=True)
            with journal_lease(out):
                advance(out, intent, intent_sha, leases)


def resume(args):
    preparation.isolated_root(args, args.transaction)
    root = preparation.directory(args.transaction)
    intent = checked_record(root / 'intent.json', args.intent_sha, 'pendingcrew-local-cutover-intent')
    fixture = preparation.isolated_root(args, args.transaction, intent['app'], intent['authority'])
    preparation.require_mode(intent, fixture)
    preparation.intent_contract(intent)
    require(intent['operation'] in ('upgrade', 'rollback'), 'unsupported recorded operation')
    paths = paths_for(root, intent)
    with journal_lease(root):
        if (root / 'complete.json').exists():
            preparation.plain_file(root / 'complete.json')
            complete = json.loads((root / 'complete.json').read_text())
            require(complete.get('kind') == 'pendingcrew-local-cutover-completed'
                    and complete.get('intentSHA256') == args.intent_sha
                    and complete.get('id') == intent['id'], 'completion record differs from intent')
            if intent['format'] in (2, 3):
                require(complete.get('format') == intent['format']
                        and set(complete) == {'format', 'kind', 'intentSHA256', 'id', 'operation',
                                             'activeApp', 'activeAuthority', 'preservedApp',
                                             'preservedAuthority', 'openedApp'},
                        'closed source-bound completion record required')
            print(f'Already completed: {root / "complete.json"}')
            return
        with contextlib.ExitStack() as stack:
            leases = tuple(stack.enter_context(preparation.authority_lease(location(paths, intent, 'data', side)))
                           for side in range(2))
            advance(root, intent, args.intent_sha, leases)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(required=True)
    begin = commands.add_parser('start')
    begin.add_argument('operation', choices=('upgrade', 'rollback'))
    for name in ('plan', 'plan-sha', 'app', 'authority', 'out'):
        begin.add_argument('--' + name, required=True)
    for name in ('new-app', 'rollback', 'rollback-sha'):
        begin.add_argument('--' + name)
    begin.add_argument('--isolated-fixture')
    begin.set_defaults(action=start)
    retry = commands.add_parser('resume')
    retry.add_argument('--transaction', required=True)
    retry.add_argument('--intent-sha', required=True)
    retry.add_argument('--isolated-fixture')
    retry.set_defaults(action=resume)
    args = parser.parse_args()
    args.action(args)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, sqlite3.Error, subprocess.SubprocessError) as error:
        raise SystemExit(f'FAIL: {error}')
