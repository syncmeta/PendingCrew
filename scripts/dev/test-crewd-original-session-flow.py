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
    a.add_argument('--products', help='Existing warm built module/products directory; never builds an App')
    a.add_argument('--library', help='Existing actual App debug dylib, including the current HotReload product')
    a.add_argument('--module-name', choices=['PendingCrew','PendingCrewHotReload'], default='PendingCrew')
    a.add_argument('--configuration', choices=['Debug','NativeCandidate'], default='NativeCandidate')
    a.add_argument('--scenarios', nargs='+', help='Run only named affected scenarios; default keeps the complete suite')
    args = a.parse_args()
    root = pathlib.Path(__file__).resolve().parents[2]
    dd = pathlib.Path(args.derived_data).resolve()
    products = pathlib.Path(args.products).resolve() if args.products else dd/'Build/Products'/args.configuration
    library = pathlib.Path(args.library).resolve() if args.library else products/'PendingCrew.app/Contents/MacOS/PendingCrew.debug.dylib'
    assert library.is_file(), 'Build the selected actual original App configuration first'
    out = pathlib.Path(args.output) if args.output else pathlib.Path(tempfile.mkdtemp(prefix='pcw-original-flow-', dir='/private/tmp'))
    out.mkdir(parents=True, exist_ok=True)
    (out/'inputs.json').write_text(json.dumps({'appLibrary':str(library),'sha256':hashlib.sha256(library.read_bytes()).hexdigest()},indent=2)+'\n')
    probe = out/'probe'
    fixture = out/'actual-session-flow.swift'
    source = (root/'scripts/dev/fixtures/crewd-original-session-flow.swift').read_text()
    assert source.count('@testable import PendingCrew\n') == 1
    fixture.write_text(source.replace('@testable import PendingCrew\n', '@testable import '+args.module_name+'\n'))
    command = ['xcrun', 'swiftc', '-parse-as-library', '-I', str(products),
               '-module-cache-path', str(dd/'ModuleCache.noindex'), '-Xcc',
               '-fmodule-map-file='+str(dd/'Build/Intermediates.noindex/GeneratedModuleMaps/CTOML.modulemap'),
               str(fixture), str(library),
               '-Xlinker', '-rpath', '-Xlinker', str(library.parent),
               '-Xlinker', '-rpath', '-Xlinker', str(library.parent.parent/'Frameworks'), '-o', str(probe)]
    (out/'compile-argv.json').write_text(json.dumps(command,indent=2)+'\n')
    with (out/'compile.log').open('w') as log:
        subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=True)
    results = []
    scenarios = ['claude-recovery-switch','claude-recovery-covered','claude-recovery-uncovered','claude-recovery-no-cap','claude-recovery-target-changed','claude-recovery-unknown','captain-not-configured','captain-retry','initialization-once','receipt-timeout','receipt-unknown','receipt-historical','authority-error-scope','quiet','click-unavailable','claude-unavailable','resume-refused','profiles','profiles-missing','profiles-unavailable','profiles-late','profiles-switch','legacy-text','legacy-resume','stopped','running','malformed','switch','double-click','captain','captain-refused','captain-switch-return','switch-return','remote-quiet','remote-running','remote-stopped','remote-malformed','remote-switch']
    assert not args.scenarios or all(s in scenarios for s in args.scenarios), 'Unknown scenario'
    for scenario in args.scenarios or scenarios:
        path = out/(scenario+'.sock')
        listener = socket.socket(socket.AF_UNIX); listener.bind(str(path)); listener.listen(1)
        calls = []; problems = []; state = {'running': scenario == 'running' or scenario == 'remote-running', 'captain': False}
        def serve():
            try:
                conn, _ = listener.accept()
                with conn, conn.makefile('r') as stream:
                    for line in stream:
                        req = json.loads(line); method = req['method']; calls.append(req)
                        if method == 'system.hello':
                            result = {'server':'crewd','apiVersion':3,'minApiVersion':1,'version':'synthetic-route', 'capabilities':['orchestra','session.respondRunnerRequest']+(['command.receipts'] if (scenario.startswith('legacy-') or scenario.startswith('receipt-') or scenario.startswith('claude-recovery-')) else []), 'runtimeIdentity':{'dbId':'synthetic-db','runtimeBindingId':'229e24cf-f1c0-4867-9fdb-3293686f7d55'}}
                            if scenario.startswith('claude-recovery-') and scenario != 'claude-recovery-no-cap': result['capabilities'].append('session.claude.recovery')
                        elif method == 'executor.list':
                            assert req['params']['crewId'] in ['crew-a','crew-b']
                            if scenario in ['profiles-late','profiles-switch','captain-switch-return'] and req['params']['crewId']=='crew-a': time.sleep(.15)
                            def profile(id, executor, backend, features, available=True, reason=None):
                                return {'id':id,'executor':executor,'displayName':id,'backend':backend,'available':available,'features':dict(zip(['initialContext','manualTerminal','structuredDecisions','fullHistoryLoad'],features)),'unavailableReason':reason}
                            result = {'profiles':[
                                profile('codex','codex','codex',[True,False,True,False],scenario not in ['profiles-unavailable','click-unavailable','captain-not-configured'],'notConfigured' if scenario in ['profiles-unavailable','click-unavailable','captain-not-configured'] else None),
                                profile('claude','claude','existingClaudePty',[False,True,False,False]),
                                profile('claude-managed','acp:claude-managed','nativeAcp',[False]*4,False,'nativeVerificationPending')]}
                            if scenario=='profiles-missing': result = {'profiles':'invalid'}
                        elif method == 'session.controlTarget':
                            assert req['params']=={'sessionId':'session-a'}
                            result = {'sessionId':'session-a','generation':'synthetic-generation:0:0','controlRevision':'1','turnId':None}
                        elif method == 'session.claude.recoveryInspect':
                            if scenario=='claude-recovery-switch': time.sleep(.15)
                            result = {'sessionId':'session-a','originalAgentSessionId':'d7c6f7ce-6928-47de-a58d-06c8cd162dc4','target':{'sessionId':'session-a','generation':'synthetic-generation:0:0','controlRevision':'2' if scenario=='claude-recovery-target-changed' else '1','turnId':None},'eligibility':'evidenceUnavailable' if scenario=='claude-recovery-unknown' else 'noUnresolvedRequests','evidenceToken':None,'unresolved':[],'dispositions':[{'requestId':'a764c01e-ea15-43f4-9936-fd2830c1d105','originalRequestIds':['11673573-53dd-43a0-8dd5-a3e72962a521' if scenario=='claude-recovery-uncovered' else state['original']]}]}
                        elif method == 'command.submit':
                            e=req['params'];cmd=e['command']
                            assert scenario in ['legacy-text','legacy-resume'] or scenario.startswith('receipt-') or scenario.startswith('claude-recovery-')
                            if scenario=='legacy-text':
                                assert cmd['operation']=='session.start' and cmd['params']['executor']=='claude' and cmd['params']['brief']=='One explicit text without extra context'
                            else:
                                assert cmd['operation']=='session.resume' and cmd['params']=={'sessionId':'session-a','generation':'synthetic-generation:0:0','controlRevision':'1','turnId':None}
                            result={'requestId':e['requestId'],'dbId':e['dbId'],'runtimeBindingId':e['runtimeBindingId'],'command':cmd,'phase':'unknown','historical':False,'payloadSha256':'a'*64,'createdAt':'2026-10-22T00:00:00Z','updatedAt':'2026-10-22T00:00:00Z','result':{}}
                            if scenario.startswith('claude-recovery-'):
                                if 'original' in state: result.update(phase='launched',result={'sessionId':'session-a'})
                                else: state['original']=e['requestId']
                            if scenario.startswith('receipt-'):
                                state['receipt'] = dict(result)
                                if scenario == 'receipt-timeout': result['phase'] = 'dispatched'
                        elif method == 'command.lookup':
                            assert scenario.startswith('receipt-')
                            assert req['params']['requestId'] == state['receipt']['requestId']
                            result = dict(state['receipt'])
                            result['phase'] = 'dispatched' if scenario == 'receipt-timeout' and not (out/(scenario+'-data')/'receipt-ready').exists() else 'launched'
                            result['historical'] = scenario == 'receipt-historical'
                            result['result'] = {'sessionId':'session-a'}
                        elif method == 'crew.list': result = [{'id':'crew-a'}]
                        elif method == 'session.list':
                            result = [{'id':'session-a','crewId':'crew-a','role':'captain','executor':('claude' if scenario=='legacy-resume' or scenario.startswith('claude-recovery-') else 'acp:claude-managed' if scenario=='claude-unavailable' else 'codex')}] if state['captain'] or not scenario.startswith('captain') else []
                        elif method == 'session.live':
                            if scenario.startswith('remote-'):
                                conn.sendall((json.dumps({'id':req['id'],'error':{'code':-32003,'message':'设备授权或连接作用域已失效，请在电脑上重新配对。','refusal':'remote.permission_denied'}})+'\n').encode()); continue
                            if scenario in ['switch','switch-return','double-click']: time.sleep(.15)
                            result = {} if scenario == 'malformed' else {'sessionId':'session-a','running':state['running']}
                        elif method == 'session.inspect':
                            if scenario == 'remote-switch': time.sleep(.15)
                            result = {'live':{} if scenario == 'remote-malformed' else {'sessionId':'session-a','running':state['running']},'screen':'Original completed remote transcript'}
                        elif method == 'session.start':
                            assert req['params']['role'] == 'captain' and req['params']['crewId'] == 'crew-a'
                            if scenario == 'captain-refused' or (scenario == 'captain-retry' and sum(r['method']=='session.start' for r in calls)==1):
                                conn.sendall((json.dumps({'id':req['id'],'error':{'code':-32009,'message':'synthetic refusal'}})+'\n').encode()); continue
                            state['captain'] = True; result = {'id':'session-a'}
                        elif method == 'session.resume':
                            assert req['params'] == {'sessionId':'session-a'}
                            if scenario == 'resume-refused':
                                conn.sendall((json.dumps({'id':req['id'],'error':{'code':-32009,'message':'synthetic permission refusal'}})+'\n').encode()); continue
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
            assert methods.count('session.resume') == (1 if scenario in ['stopped','double-click','resume-refused','remote-stopped'] else 0), methods
            assert methods.count('command.submit') == (2 if scenario=='claude-recovery-covered' else 1 if scenario.startswith('legacy-') or scenario.startswith('receipt-') or scenario.startswith('claude-recovery-') else 0), methods
            if scenario.startswith('claude-recovery-'):
                assert methods.count('session.claude.recoveryInspect') == (0 if scenario=='claude-recovery-no-cap' else 2 if scenario=='claude-recovery-covered' else 1), methods
                submitted = [r['params']['requestId'] for r in calls if r['method']=='command.submit']
                assert len(submitted)==len(set(submitted)), 'must not replay original UUID'
            if scenario.startswith('receipt-'):
                assert methods.count('command.lookup') >= (2 if scenario == 'receipt-timeout' else 1), methods
            if scenario.startswith('remote-'): assert methods.count('session.live') == 0 and methods.count('session.inspect') >= 1, methods
            assert methods.count('session.start') == (2 if scenario == 'captain-retry' else 1 if scenario in ['captain','captain-refused'] else 0), methods
            (out/(scenario+'.json')).write_text(json.dumps(calls,indent=2)+'\n')
            results.append({'scenario':scenario,'actualProbeExitCode':0,'rawStartCount':methods.count('session.start'),'rawResumeCount':methods.count('session.resume'),'publicSubmitCount':methods.count('command.submit'),'profileReadCount':methods.count('executor.list')})
            print('PASS '+scenario)
        finally:
            (out/(scenario+'.json')).write_text(json.dumps(calls,indent=2)+'\n')
            (out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
            listener.close()
    print('Evidence: '+str(out))
if __name__ == '__main__': main()
