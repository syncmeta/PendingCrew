#!/usr/bin/env python3
"""Prove Debug-only RPATH removal retains raw commands and signature bytes."""
import importlib.util
import pathlib
import struct
import sys

sys.dont_write_bytecode = True
repo = pathlib.Path(__file__).resolve().parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, repo / 'scripts/dev' / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


builder = load('build-crewd-delivery-validation-macos')
closure = load('crewd-validation-load-closure')
seal = load('crewd-macho-seal-equivalence')
transient = ('@loader_path', '/private/tmp/pcw-codex-first-xcode/Build/Products/Debug/PackageFrameworks')
retained_path = '@executable_path/../Frameworks'
payload = b'nonzero linkedit payload'
signature = struct.pack('>III', 0xfade0cc0, 20, 0) + b'raw seal'
content_start = 512


def rpath(path):
    value = path.encode() + b'\0'
    size = (12 + len(value) + 7) // 8 * 8
    return struct.pack('<III', 0x8000001c, size, 12) + value.ljust(size - 12, b'\0')


def fixture(paths, *, signed=True, linkedit_vm_size=16384):
    blob = signature if signed else b''
    linkedit = struct.pack('<II16s4Q4I', 0x19, 72, b'__LINKEDIT', 0x100004000,
                           linkedit_vm_size, content_start, len(payload) + len(blob), 1, 1, 0, 0)
    uuid = struct.pack('<II16s', 0x1b, 24, b'test uuid bytes!')
    code_signature = struct.pack('<4I', 0x1d, 16, content_start + len(payload), len(blob)) if signed else b''
    commands = [linkedit, rpath(paths[0]), uuid, *(rpath(path) for path in paths[1:])]
    if signed:
        commands.append(code_signature)
    body = b''.join(commands)
    header = struct.pack('<8I', 0xfeedfacf, 0x100000c, 0, 2, len(commands), len(body), 0, 0)
    return (header + body).ljust(content_start, b'\0') + payload + blob, commands


def rejects(original, expected):
    try:
        builder.compact_debug_rpaths(original, transient, closure, seal)
    except ValueError as error:
        assert expected in str(error), str(error)
    else:
        raise AssertionError(f'must refuse {expected}')


for signed in (False, True):
    original, commands = fixture([transient[0], retained_path, transient[1]], signed=signed)
    edited, removed, observed = builder.compact_debug_rpaths(original, transient, closure, seal)
    assert removed == list(transient)
    assert observed == closure.sha(original[seal.thin(original)['commandEnd']:seal.thin(original)['contentEnd']])
    retained = commands[0] + commands[2] + commands[3] + (commands[5] if signed else b'')
    assert edited[:16] == original[:16] and edited[24:32] == original[24:32]
    assert struct.unpack_from('<II', edited, 16) == (len(commands) - 2, len(retained))
    assert edited[32:32 + len(retained)] == retained
    old_end = seal.thin(original)['commandEnd']
    assert edited[32 + len(retained):old_end] == bytes(old_end - 32 - len(retained))
    assert edited[old_end:] == original[old_end:]
    assert closure.image(edited)[0]['rpaths'] == [retained_path]
    assert seal.thin(edited)['rounding'] == {16384}

already_clean, _ = fixture([retained_path])
assert builder.compact_debug_rpaths(already_clean, transient, closure, seal)[0] == already_clean
rejects(fixture([retained_path, '/unreviewed/PackageFrameworks'])[0], 'unreviewed rpath')
rejects(fixture([transient[0], transient[0]])[0], 'duplicate transient paths')
rejects(fixture([transient[0]], linkedit_vm_size=32768)[0], 'not exact signature-bound page rounding')
print('PASS Debug-only RPATH compaction retains raw LINKEDIT, code payload and linker signature')
