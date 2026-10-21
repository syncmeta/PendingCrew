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
    a.add_argument('--configuration', choices=['Debug','NativeCandidate'], default='NativeCandidate')
    args = a.parse_args()
    root = pathlib.Path(__file__).resolve().parents[2]
    dd = pathlib.Path(args.derived_data).resolve()
    products = dd/'Build/Products'/args.configuration
    library = products/'PendingCrew.app/Contents/MacOS/PendingCrew.debug.dylib'
    assert library.is_file(), 'Build the selected actual original App configuration first'
    out = pathlib.Path(args.output) if args.output else pathlib.Path(tempfile.mkdtemp(prefix='pcw-original-flow-', dir='/private/tmp'))
    out.mkdir(parents=True, exist_ok=True)
    (out/'inputs.json').write_text(json.dumps({'appLibrary':str(library),'sha256':hashlib.sha256(library.read_bytes()).hexdigest()},indent=2)+'\n')
    probe = out/'probe'
    command = ['xcrun', 'swiftc', '-parse-as-library', '-I', str(products),
               '-module-cache-path', str(dd/'ModuleCache.noindex'), '-Xcc',
               '-fmodule-map-file='+str(dd/'Build/Intermediates.noindex/GeneratedModuleMaps/CTOML.modulemap'),
               str(root/'scripts/dev/fixtures/crewd-original-session-flow.swift'), str(library),
               '-Xlinker', '-rpath', '-Xlinker', str(library.parent),
               '-Xlinker', '-rpath', '-Xlinker', str(library.parent.parent/'Frameworks'), '-o', str(probe)]
    (out/'compile-argv.json').write_text(json.dumps(command,indent=2)+'\n')
    with (out/'compile.log').open('w') as log:
        subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=True)
    for scenario in ['profiles','profiles-missing','profiles-unavailable','profiles-late','profiles-switch','legacy-text','legacy-resume','stopped','running','malformed','switch','double-click','captain','captain-refused']:
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
                            result = {'server':'crewd','apiVersion':3,'minApiVersion':1,'version':'synthetic-route', 'capabilities':['orchestra','session.respondRunnerRequest']+(['command.receipts'] if scenario.startswith('legacy-') else []), 'runtimeIdentity':{'dbId':'synthetic-db','runtimeBindingId':'229e24cf-f1c0-4867-9fdb-3293686f7d55'}}
                        elif method == 'executor.list':
                            assert req['params']['crewId'] in ['crew-a','crew-b']
                            if scenario in ['profiles-late','profiles-switch'] and req['params']['crewId']=='crew-a': time.sleep(.15)
                            def profile(id, executor, backend, features, available=True, reason=None):
                                return {'id':id,'executor':executor,'displayName':id,'backend':backend,'available':available,'features':dict(zip(['initialContext','manualTerminal','structuredDecisions','fullHistoryLoad'],features)),'unavailableReason':reason}
                            result = {'profiles':[
                                profile('codex','codex','codex',[True,False,True,False],scenario!='profiles-unavailable','notConfigured' if scenario=='profiles-unavailable' else None),
                                profile('claude','claude','existingClaudePty',[False,True,False,False]),
                                profile('claude-managed','acp:claude-managed','nativeAcp',[False]*4,False,'nativeVerificationPending')]}
                            if scenario=='profiles-missing': result = {'profiles':'invalid'}
                        elif method == 'session.controlTarget':
                            assert req['params']=={'sessionId':'session-a'}
                            result = {'sessionId':'session-a','generation':'synthetic-generation:0:0','controlRevision':'1','turnId':None}
                        elif method == 'command.submit':
                            e=req['params'];cmd=e['command']
                            assert scenario in ['legacy-text','legacy-resume']
                            if scenario=='legacy-text':
                                assert cmd['operation']=='session.start' and cmd['params']['executor']=='claude' and cmd['params']['brief']=='One explicit text without extra context'
                            else:
                                assert cmd['operation']=='session.resume' and cmd['params']=={'sessionId':'session-a','generation':'synthetic-generation:0:0','controlRevision':'1','turnId':None}
                            result={'requestId':e['requestId'],'dbId':e['dbId'],'runtimeBindingId':e['runtimeBindingId'],'command':cmd,'phase':'unknown','historical':False,'payloadSha256':'a'*64,'createdAt':'2026-10-22T00:00:00Z','updatedAt':'2026-10-22T00:00:00Z','result':{}}
                        elif method == 'crew.list': result = [{'id':'crew-a'}]
                        elif method == 'session.list':
                            result = [{'id':'session-a','crewId':'crew-a','role':'captain','executor':'claude' if scenario=='legacy-resume' else 'codex'}] if state['captain'] or scenario not in ['captain','captain-refused'] else []
                        elif method == 'session.live':
                            if scenario in ['switch','double-click']: time.sleep(.15)
                            result = {} if scenario == 'malformed' else {'sessionId':'session-a','running':state['running']}
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
            assert methods.count('command.submit') == (1 if scenario.startswith('legacy-') else 0), methods
            assert methods.count('session.start') == (1 if scenario.startswith('captain') else 0), methods
            (out/(scenario+'.json')).write_text(json.dumps(calls,indent=2)+'\n')
            print('PASS '+scenario)
        finally: listener.close()
    print('Evidence: '+str(out))
if __name__ == '__main__': main()
