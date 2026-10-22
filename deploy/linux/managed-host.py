#!/usr/bin/python3 -I
"""Keep the installed prefix leased while the exact ordinary Core owns its data."""
import argparse
import fcntl
import os
import pathlib
import re
import stat
import sys


def root_directory(path):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
        raise ValueError('root-owned non-writable code ancestry required')


def core_path(prefix):
    prefix = pathlib.Path(prefix)
    if not prefix.is_absolute():
        raise ValueError('absolute installation prefix required')
    for parent in [*reversed(prefix.parents), prefix]:
        root_directory(parent)
    activation = os.readlink(prefix / 'current')
    if not re.fullmatch(r'activations/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}', activation):
        raise ValueError('invalid immutable activation')
    for path in [prefix / 'activations', prefix / activation]:
        root_directory(path)
    release = os.readlink(prefix / activation / 'release')
    if not re.fullmatch(r'../../releases/[0-9a-f]{40}', release):
        raise ValueError('invalid immutable release')
    directory = prefix / 'releases' / release.rsplit('/', 1)[1]
    for path in [prefix / 'releases', directory, directory / 'bin']:
        root_directory(path)
    binary = directory / 'bin/crewd'
    info = binary.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022 or not info.st_mode & 0o111:
        raise ValueError('ordinary Core must be immutable root-owned executable')
    return binary


def run(args):
    if os.getuid() == 0 or os.geteuid() == 0:
        raise ValueError('Core must run as the selected ordinary login user')
    # Socket activation owns fd3. exec preserves this PID; no fake LISTEN_PID or
    # environment assignment. Core validates the root listener and seals fd3.
    if os.environ.get('LISTEN_PID') != str(os.getpid()) or os.environ.get('LISTEN_FDS') != '1' or os.environ.get('LISTEN_FDNAMES') != 'pcw-admin':
        raise ValueError('the original systemd root management listener is required')
    prefix = pathlib.Path(args.prefix)
    fd = os.open(prefix / '.runtime.lock', os.O_RDONLY | os.O_NOFOLLOW)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or stat.S_IMODE(info.st_mode) != 0o644:
        raise ValueError('root-owned runtime prefix lock required')
    fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
    binary = core_path(prefix)
    # Resolve only after the shared lease: installer holds the exclusive side
    # across current-pointer activation. No second data lease: Core crewd.lock
    # remains its sole existing authority for this data directory.
    os.set_inheritable(fd, True)
    os.execv(str(binary), [str(binary), '--data-dir', args.data_dir, 'serve', '--system-admin-socket', args.system_admin_socket, '--codex', args.codex, '--claude', args.claude])


def main():
    parser = argparse.ArgumentParser()
    for name in ['prefix', 'data-dir', 'system-admin-socket', 'codex', 'claude']:
        parser.add_argument('--' + name, required=True)
    run(parser.parse_args())


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError) as failure:
        print('PendingCrew runtime refused: ' + str(failure), file=sys.stderr)
        raise SystemExit(1)
