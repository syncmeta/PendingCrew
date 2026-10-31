#!/usr/bin/env python3
"""Real Codex driver, deterministic loopback Responses provider; no OAuth request.

Runs only the explicitly ignored native_case_profile test. The Rust side must
select the production restricted policy; this provider does NOT enforce it.
Evidence is retained under a unique /private/tmp directory, never global config.
"""
import argparse
import hashlib
import http.server
import json
import os
import pathlib
import shutil
import stat
import subprocess
import tempfile
import threading


def remove_probe_auth_copy(base):
    """Delete only this fixture's marked private copy, without reading auth bytes."""
    base = pathlib.Path(base)
    assert base.parent == pathlib.Path('/private/tmp') and base.name.startswith('pcw-codex-case-native-')
    for directory in (base, base / 'runtime'):
        if not directory.exists():
            return 'no-copy-created'
        info = directory.lstat()
        assert stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode)
        assert info.st_uid == os.getuid()
    home = base / 'runtime' / 'codex-case-home-native-case-profile-probe'
    if not home.exists():
        return 'no-copy-created'
    info = home.lstat()
    assert stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and info.st_mode & 0o077 == 0
    directory_fd = os.open(home, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        marker_fd = os.open('pcw-case-home.json', os.O_RDONLY | os.O_NOFOLLOW, dir_fd=directory_fd)
        with os.fdopen(marker_fd) as marker_file:
            marker_info = os.fstat(marker_file.fileno())
            assert stat.S_ISREG(marker_info.st_mode) and marker_info.st_uid == os.getuid()
            assert marker_info.st_mode & 0o077 == 0 and marker_info.st_size < 16384
            marker = json.load(marker_file)
        assert marker['version'] == 1 and marker['session'] == 'native-case-profile-probe'
        assert marker['cwd'] == str(base / 'project')
        assert pathlib.Path(marker['sourceHome']).is_absolute()
        assert not pathlib.Path(marker['sourceHome']).is_relative_to(base)
        try:
            copied = os.stat('auth.json', dir_fd=directory_fd, follow_symlinks=False)
        except FileNotFoundError:
            return 'copy-already-absent'
        assert stat.S_ISREG(copied.st_mode) and copied.st_uid == os.getuid()
        assert copied.st_mode & 0o077 == 0 and copied.st_nlink == 1
        os.unlink('auth.json', dir_fd=directory_fd)
        return 'marked-private-auth-copy-removed'
    finally:
        os.close(directory_fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', default=shutil.which('codex'))
    args = parser.parse_args()
    if not args.codex:
        parser.error('codex executable required')
    base = pathlib.Path(tempfile.mkdtemp(prefix='pcw-codex-case-native-', dir='/private/tmp'))
    project = base / 'project'
    project.mkdir()
    sentinel = project / 'sentinel.txt'
    sentinel.write_text('unchanged isolated native profile sentinel\n')
    outside = base / 'outside.txt'
    outside.write_text('PRIVATE_OUTSIDE_PROBE_DO_NOT_READ\n')
    (project / 'outside-link').symlink_to(outside)
    # Deliberately hostile *test-owned* project config. The production policy
    # must suppress startup itself, not merely hide outsider tools afterward.
    foreign = base / 'foreign-mcp'
    foreign.write_text('#!/usr/bin/env python3\nfrom pathlib import Path\nPath(' +
                       repr(str(base / 'foreign-started')) + ').write_text("UNEXPECTED")\n')
    foreign.chmod(0o700)
    (project / '.codex').mkdir()
    (project / '.codex' / 'config.toml').write_text(
        '[mcp_servers.outsider]\ncommand = ' + json.dumps(str(foreign)) + '\nenabled = true\n')
    expected = hashlib.sha256(sentinel.read_bytes()).hexdigest()
    state = {'requests': [], 'calls': [], 'sink': 0}

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            if self.path == '/sink':
                state['sink'] += 1
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'NETWORK_PROBE_REACHED' if self.path == '/sink' else b'{"data": []}')

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get('content-length', '0'))))
            state['requests'].append(body)
            # Each fresh/resumed turn starts with a user item; the harness uses
            # identical independent six-call sequences, tracked per process run.
            index = len(state['requests']) - 1
            cycle = index % 8
            commands = [
                f'cat {sentinel}',
                f'printf forbidden > {project / "shell-write"}',
                f'cat {outside}',
                f'cat {project / "outside-link"}',
                f'curl --max-time 2 http://127.0.0.1:{self.server.server_port}/sink',
            ]
            rid = f'resp_probe_{index}'
            if cycle < 5:
                item = {'type': 'function_call', 'id': f'fc_{index}',
                        'call_id': f'call_{index}', 'name': 'exec_command',
                        'arguments': json.dumps({'cmd': commands[cycle], 'max_output_tokens': 1000}),
                        'status': 'completed'}
            elif cycle == 5:
                item = {'type': 'custom_tool_call', 'id': f'fc_{index}',
                        'call_id': f'call_{index}', 'name': 'apply_patch',
                        'input': f'*** Begin Patch\n*** Update File: {sentinel}\n@@\n-unchanged isolated native profile sentinel\n+FORBIDDEN_EDIT\n*** End Patch',
                        'status': 'completed'}
            elif cycle == 6:
                item = {'type': 'function_call', 'id': f'fc_{index}',
                        'call_id': f'call_{index}', 'name': 'exec_command',
                        'arguments': json.dumps({'cmd': f'touch {project / "escalated-write"}',
                                               'sandbox_permissions': 'require_escalated',
                                               'justification': 'Isolated denial probe'}),
                        'status': 'completed'}
            else:
                item = {'type': 'message', 'id': f'msg_{index}', 'role': 'assistant',
                        'status': 'completed', 'content': [{'type': 'output_text', 'text': 'Probe finished.', 'annotations': []}]}
            state['calls'].append(item)
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()

            def event(kind, **payload):
                self.wfile.write(('event: ' + kind + '\ndata: ' + json.dumps({'type': kind, **payload}) + '\n\n').encode())
                self.wfile.flush()

            event('response.created', response={'id': rid, 'object': 'response', 'status': 'in_progress', 'output': []})
            event('response.output_item.added', output_index=0, item=item)
            event('response.output_item.done', output_index=0, item=item)
            event('response.completed', response={'id': rid, 'object': 'response', 'status': 'completed',
                                                  'output': [item], 'usage': {'input_tokens': 10, 'output_tokens': 10, 'total_tokens': 20}})

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    wrapper = base / 'codex-wrapper'
    overrides = ['model_provider="pcw_probe"', 'model="gpt-5.1-codex"',
                 'model_providers.pcw_probe.name="Isolated loopback fixture"',
                 f'model_providers.pcw_probe.base_url="http://127.0.0.1:{server.server_port}/v1"',
                 'model_providers.pcw_probe.wire_api="responses"',
                 'model_providers.pcw_probe.requires_openai_auth=false']
    argv = [str(pathlib.Path(args.codex).resolve())]
    for option in overrides:
        argv.extend(['-c', option])
    wrapper.write_text('#!/usr/bin/env python3\nimport os,sys\na=' + repr(argv) + '\nos.execv(a[0],a+sys.argv[1:])\n')
    wrapper.chmod(0o700)
    helper = base / 'crew-handshake-fixture'
    helper.write_text('''#!/usr/bin/env python3
import json,sys
for line in sys.stdin:
 try: q=json.loads(line)
 except ValueError: continue
 if 'id' not in q: continue
 if q.get('method')=='initialize':
  r={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'isolated-handshake-only','version':'1'}}
 elif q.get('method')=='tools/list': r={'tools':[]}
 else: r={}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':r}),flush=True)
''')
    helper.chmod(0o700)
    env = dict(os.environ, PCW_NATIVE_CASE_PROBE='loopback', PCW_NATIVE_CASE_PROGRAM=str(wrapper),
               PCW_NATIVE_CASE_PROJECT=str(project), PCW_NATIVE_CASE_EVIDENCE=str(base),
               PCW_NATIVE_CASE_HELPER=str(helper))
    repo = pathlib.Path(__file__).resolve().parents[2]
    try:
        result = subprocess.run(['cargo', 'test', '-p', 'crewd-exec', '--test', 'native_case_profile',
                                 '--', '--ignored', '--nocapture'], cwd=repo / 'crewd', env=env,
                                capture_output=True, text=True, timeout=240)
        (base / 'test.log').write_text(result.stdout + result.stderr)
        assert result.returncode == 0, (result.returncode, result.stderr[-3000:])
        assert len(state['requests']) == 16, ('fresh+resume forced calls incomplete', len(state['requests']))
        assert hashlib.sha256(sentinel.read_bytes()).hexdigest() == expected
        assert not (project / 'shell-write').exists()
        assert not (project / 'escalated-write').exists()
        assert not (base / 'foreign-started').exists(), 'inherited project MCP started'
        assert state['sink'] == 0, 'native network probe reached loopback sink'
        results = {entry['call_id']: entry.get('output', '') for request in state['requests']
                   for entry in request.get('input', []) if isinstance(entry, dict) and entry.get('type') in ('function_call_output', 'custom_tool_call_output')}
        for offset in (0, 8):
            assert 'unchanged isolated' in str(results.get(f'call_{offset}', '')), 'positive shell read not executed'
            for step in (1, 2, 3, 4, 5, 6):
                output = str(results.get(f'call_{offset+step}', ''))
                assert output, ('missing actual tool result', offset, step)
                assert 'PRIVATE_OUTSIDE_PROBE' not in output
                accepted_denials = ('denied', 'not permitted', 'reject', 'not allowed', 'prohibited', 'sandbox')
                if step == 4:
                    accepted_denials += ('couldn\'t connect', 'failed to connect')
                if step == 5:
                    accepted_denials += ('unsupported custom tool call: apply_patch',)
                assert any(s in output.lower() for s in accepted_denials), output
        print('PASS: real Codex fresh/resume forced native probes; fake provider only. PCW MCP is a separate acceptance gate.')
    finally:
        server.shutdown()
        (base / 'requests.json').write_text(json.dumps(state, indent=2))
        cleanup = remove_probe_auth_copy(base)
        (base / 'credential-cleanup.json').write_text(json.dumps({'status': cleanup}) + '\n')
        print('Evidence:', base, flush=True)


if __name__ == '__main__':
    main()
