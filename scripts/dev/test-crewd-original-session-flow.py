#!/usr/bin/python3
"""Test actual compiled original-App session actions over disposable Unix RPC.
No provider, credentials, daily data, installation or product authentication override.
Usage: python3 SCRIPT --derived-data DIR [--output FRESH_DIR]
"""
import argparse, hashlib, json, pathlib, socket, subprocess, tempfile, threading, time

def main():
    a = argparse.ArgumentParser()
    a.add_argument('--derived-data', required=True)
    a.add_argument('--output')
    a.add_argument('--module-cache')
    args = a.parse_args()
    root = pathlib.Path(__file__).resolve().parents[2]
    dd = pathlib.Path(args.derived_data).resolve()
    products = dd/'Build/Products/NativeCandidate'
    library = products/'PendingCrew.app/Contents/MacOS/PendingCrew.debug.dylib'
    assert library.is_file(), 'Build the original NativeCandidate App first'
    out = pathlib.Path(args.output) if args.output else pathlib.Path(tempfile.mkdtemp(prefix='pcw-original-flow-', dir='/private/tmp'))
    out.mkdir(parents=True, exist_ok=True)
    (out/'inputs.json').write_text(json.dumps({'appLibrary':str(library),'sha256':hashlib.sha256(library.read_bytes()).hexdigest()},indent=2)+'\n')
    probe = out/'probe'
    command = ['xcrun', 'swiftc', '-parse-as-library', '-I', str(products),
               '-module-cache-path', str(pathlib.Path(args.module_cache) if args.module_cache else out/'cache'), '-Xcc',
               '-fmodule-map-file='+str(dd/'Build/Intermediates.noindex/GeneratedModuleMaps/CTOML.modulemap'),
               str(root/'scripts/dev/fixtures/crewd-original-session-flow.swift'), str(library),
               '-Xlinker', '-rpath', '-Xlinker', str(library.parent),
               '-Xlinker', '-rpath', '-Xlinker', str(library.parent.parent/'Frameworks'), '-o', str(probe)]
    with (out/'compile.log').open('w') as log:
        subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=True)
    for scenario in ['captain-before-selection', 'stopped', 'running', 'malformed', 'switch', 'double-click', 'captain', 'captain-refused', 'captain-unavailable', 'captain-connection-switch']:
        path = out/(scenario+'.sock')
        listener = socket.socket(socket.AF_UNIX); listener.bind(str(path)); listener.listen(1)
        calls = []; problems = []; state = {'running': scenario == 'running', 'captain': False}
        def serve():
            try:
                conn, _ = listener.accept()
                with conn, conn.makefile('r') as stream:
                    for line in stream:
                        req = json.loads(line); method = req['method']; calls.append(req)
                        if method == 'system.hello':
                            result = {'server':'crewd','apiVersion':3,'minApiVersion':1,'version':'synthetic-route', 'capabilities':['orchestra','session.respondRunnerRequest']}
                        elif method == 'crew.list': result = [{'id':'crew-a'}]
                        elif method == 'session.list':
                            result = [{'id':'session-a','crewId':'crew-a','role':'captain','executor':'codex'}] if state['captain'] or not scenario.startswith('captain') else []
                        elif method == 'session.live':
                            if scenario in ['switch','double-click']: time.sleep(.15)
                            result = {} if scenario == 'malformed' else {'sessionId':'session-a','running':state['running']}
                        elif method == 'executor.list':
                            assert req['params'] == {'crewId':'crew-a'}
                            if scenario == 'captain-connection-switch': time.sleep(.2)
                            available = scenario != 'captain-unavailable'
                            result = {'profiles':[{'id':'codex','executor':'codex','displayName':'Codex','backend':'codex','available':available,'features':{'initialContext':available,'manualTerminal':False,'structuredDecisions':False,'fullHistoryLoad':False},'unavailableReason':None if available else 'notConfigured'}]}
                        elif method == 'session.start':
                            assert req['params']['role'] == 'captain' and req['params']['crewId'] == 'crew-a'
                            if scenario == 'captain-refused':
                                conn.sendall((json.dumps({'id':req['id'],'error':{'code':-32009,'message':'synthetic refusal'}})+'\n').encode()); continue
                            state['captain'] = True; result = {'id':'session-a'}
                        elif method == 'session.resume':
                            assert req['params'] == {'sessionId':'session-a'}
                            state['running'] = True; result = {'sessionId':'session-a'}
                        elif method.endswith('.list'): result = []
                        else: result = {}
                        conn.sendall((json.dumps({'id':req['id'],'result':result})+'\n').encode())
            except (BrokenPipeError, ConnectionResetError): pass
            except Exception as error: problems.append(repr(error))
        thread = threading.Thread(target=serve, daemon=True); thread.start()
        try:
            with (out/(scenario+'.log')).open('w') as log:
                subprocess.run([str(probe),str(path),scenario],stdout=log,stderr=subprocess.STDOUT,check=True,timeout=20)
            thread.join(timeout=3)
            assert not problems, problems
            methods = [r['method'] for r in calls]
            assert methods.count('session.resume') == (1 if scenario in ['stopped','double-click'] else 0), methods
            assert methods.count('session.start') == (1 if scenario in ['captain','captain-refused','captain-before-selection'] else 0), methods
            (out/(scenario+'.json')).write_text(json.dumps(calls,indent=2)+'\n')
            print('PASS '+scenario)
        finally:
            (out/(scenario+'.json')).write_text(json.dumps(calls,indent=2)+'\n')
            listener.close()
    print('Evidence: '+str(out))
if __name__ == '__main__': main()
