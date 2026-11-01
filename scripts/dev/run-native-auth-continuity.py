#!/usr/bin/env python3
"""Real local Codex 401/refresh protocol with synthetic JWTs and broker only.

Never reads user credentials. HTTP Authorization is recorded only as a digest.
Does not prove the production source-manager broker (separate Core acceptance).
"""
import argparse
import hashlib
import http.server
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import threading
import time


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--codex', default=shutil.which('codex'))
    ap.add_argument('--mode', choices=['all', 'success', 'reject', 'mismatch'], default='all')
    args = ap.parse_args()
    assert args.codex, 'codex executable required'
    repo = pathlib.Path(__file__).resolve().parents[2]
    for mode in (['success', 'reject', 'mismatch'] if args.mode == 'all' else [args.mode]):
        base = pathlib.Path(tempfile.mkdtemp(prefix='pcw-native-auth-' + mode + '-', dir='/private/tmp'))
        (base / 'project').mkdir()
        requests = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b'{"data":[]}')

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers.get('content-length', '0'))))
                authorization = self.headers.get('Authorization', '')
                requests.append({'path': self.path, 'authHash': hashlib.sha256(authorization.encode()).hexdigest(),
                                 'hasAuthorization': bool(authorization), 'model': body.get('model')})
                if self.path != '/v1/responses':
                    self.send_response(200)
                    self.end_headers()
                    self.wfile.write(b'{}')
                    return
                response_requests = [r for r in requests if r['path'] == '/v1/responses']
                if len(response_requests) == 1 or mode != 'success':
                    self.send_response(401)
                    self.send_header('Content-Type', 'application/json')
                    self.end_headers()
                    self.wfile.write(b'{"error":{"type":"invalid_request_error","code":"invalid_api_key","message":"Synthetic authorization expired"}}')
                    return
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.end_headers()
                item = {'type': 'message', 'id': 'probe_message', 'role': 'assistant', 'status': 'completed',
                        'content': [{'type': 'output_text', 'text': 'AUTH_PROBE_COMPLETED', 'annotations': []}]}
                def event(kind, **data):
                    self.wfile.write(('event: ' + kind + '\ndata: ' + json.dumps({'type': kind, **data}) + '\n\n').encode())
                    self.wfile.flush()
                event('response.created', response={'id': 'probe_response', 'object': 'response', 'status': 'in_progress', 'output': []})
                event('response.output_item.added', output_index=0, item=item)
                event('response.output_item.done', output_index=0, item=item)
                event('response.completed', response={'id': 'probe_response', 'object': 'response', 'status': 'completed', 'output': [item],
                                                       'usage': {'input_tokens': 1, 'output_tokens': 1, 'total_tokens': 2}})

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        wrapper = base / 'codex-wrapper'
        argv = [str(pathlib.Path(args.codex).resolve())]
        overrides = ['model_provider="pcw_auth_probe"', 'model="gpt-5.1-codex"',
                     f'chatgpt_base_url="http://127.0.0.1:{server.server_port}/chatgpt"',
                     'model_providers.pcw_auth_probe.name="Synthetic auth acceptance"',
                     f'model_providers.pcw_auth_probe.base_url="http://127.0.0.1:{server.server_port}/v1"',
                     'model_providers.pcw_auth_probe.wire_api="responses"',
                     'model_providers.pcw_auth_probe.requires_openai_auth=true']
        for v in overrides:
            argv.extend(['-c', v])
        wrapper.write_text('#!/usr/bin/env python3\nimport os,sys\na=' + repr(argv) + '\nos.execv(a[0],a+sys.argv[1:])\n')
        wrapper.chmod(0o700)
        helper = base / 'crew-handshake-only'
        helper.write_text('''#!/usr/bin/env python3
import json,sys
for line in sys.stdin:
 try: q=json.loads(line)
 except ValueError: continue
 if 'id' not in q: continue
 if q.get('method')=='initialize': r={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'handshake-only','version':'1'}}
 elif q.get('method')=='tools/list': r={'tools':[]}
 else: r={}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':r}),flush=True)
''')
        helper.chmod(0o700)
        env = dict(os.environ, PCW_AUTH_PROBE='loopback', PCW_AUTH_MODE=mode, PCW_AUTH_EVIDENCE=str(base),
                   PCW_AUTH_PROGRAM=str(wrapper), PCW_AUTH_HELPER=str(helper))
        try:
            result = subprocess.run(['cargo', 'test', '-p', 'crewd-exec', '--test', 'native_auth_continuity', '--', '--ignored', '--nocapture'],
                                    cwd=repo / 'crewd', env=env, capture_output=True, text=True, timeout=180)
            (base / 'test.log').write_text(result.stdout + result.stderr)
            assert result.returncode == 0, (mode, result.stderr[-1800:])
            count_after_exit = len(requests)
            time.sleep(2)
            assert len(requests) == count_after_exit, 'no native request may continue after actual exit'
            assert requests and all(r['hasAuthorization'] for r in requests), 'must exercise authenticated actual HTTP requests'
            response_requests = [r for r in requests if r['path'] == '/v1/responses']
            if mode == 'success':
                assert len(response_requests) == 2 and response_requests[0]['authHash'] != response_requests[1]['authHash'], requests
            else:
                assert len(response_requests) == 1, 'rejected/mismatched auth must not retry provider with another identity'
            assert not list((base / 'runtime').rglob('auth.json')), 'case must not persist auth file'
            print(mode, 'PASS', 'actual HTTP requests', len(requests))
        finally:
            server.shutdown()
            (base / 'http-evidence.json').write_text(json.dumps(requests, indent=2))
            print('Evidence:', base, flush=True)


if __name__ == '__main__':
    main()
