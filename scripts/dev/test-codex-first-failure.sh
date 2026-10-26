#!/bin/sh
# Real workspace/client + tiny Unix protocol peer; no Cargo, GUI or real runner.
set -eu
mode=${1:-all}
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-client-failure.XXXXXX)
echo "Failure evidence: $fixture"
# Reuse the normal built App module, not a partial replacement Workspace target.
: "${PENDINGCREW_TEST_APP_PRODUCTS:?Set the existing normal Debug products directory}"
: "${PENDINGCREW_TEST_APP_LIBRARY:?Set the exact actual App Debug dylib}"
: "${PENDINGCREW_TEST_DERIVED_DATA:?Set the existing warm DerivedData directory}"
/usr/bin/python3 - "$fixture" "$PENDINGCREW_TEST_APP_LIBRARY" "$root/scripts/dev/fixtures/crewd-client-failure.swift" <<'INPUTS'
import hashlib,json,pathlib,sys
out,library,source=map(pathlib.Path,sys.argv[1:])
(out/'actual-inputs.json').write_text(json.dumps({'library':str(library),'librarySHA256':hashlib.sha256(library.read_bytes()).hexdigest(),'fixture':str(source),'fixtureSHA256':hashlib.sha256(source.read_bytes()).hexdigest()},indent=2)+'\n')
INPUTS
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library -I "$PENDINGCREW_TEST_APP_PRODUCTS" \
    -Xcc "-fmodule-map-file=$PENDINGCREW_TEST_DERIVED_DATA/Build/Intermediates.noindex/GeneratedModuleMaps/CTOML.modulemap" \
    "$root/scripts/dev/fixtures/crewd-client-failure.swift" "$PENDINGCREW_TEST_APP_LIBRARY" \
    -Xlinker -rpath -Xlinker "$(dirname "$PENDINGCREW_TEST_APP_LIBRARY")" \
    -Xlinker -rpath -Xlinker "$(dirname "$(dirname "$PENDINGCREW_TEST_APP_LIBRARY")")/Frameworks" -o "$fixture/probe"
/usr/bin/python3 - "$fixture" "$mode" <<'PYTHON'
import json,os,pathlib,socket,subprocess,sys,threading,time
base=pathlib.Path(sys.argv[1]); terminal=['final_reply','eof','truncated']; remotes=['remote-foreground','remote-manual','remote-retired','remote-refused','remote-grant-refused','remote-business-refused','remote-cancel-pending','remote-cancel-initial'];available=['startup','incompatible','retry']+terminal+remotes
modes=available if sys.argv[2]=='all' else remotes if sys.argv[2]=='remote' else terminal if sys.argv[2]=='transport' else [sys.argv[2]]
assert all(m in available for m in modes)
failures=[]
for mode in modes:
    root=base/mode;root.mkdir();path=str(root/'crewd.sock');calls=[];errors=[];accepts=[];ready=threading.Event();stopping=threading.Event();active=[];second_dropped=False;second_serving=False
    def peer(incompatible=False,drop=False):
        try:
            with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as listener:
                listener.bind(path);listener.listen(1);listener.settimeout(.2);ready.set()
                while not stopping.is_set():
                    try:conn=listener.accept()[0];break
                    except socket.timeout:continue
                else:return
                accepts.append(True)
                with conn:
                    active[:]=[conn]
                    conn.settimeout(65);reader=conn.makefile('rb',buffering=0)
                    while True:
                        line=reader.readline()
                        if not line:break
                        req=json.loads(line);method=req['method'];calls.append(req)
                        if method=='system.hello':
                            if mode=='remote-grant-refused':
                                conn.sendall((json.dumps({'id':req['id'],'error':{'code':-32003,'message':'grant revoked','data':{'refusal':'remote.permission_denied'}}})+'\n').encode());continue
                            result={'server':'crewd','apiVersion':4 if incompatible else 3,'minApiVersion':4 if incompatible else 3,'version':'synthetic','capabilities':['session.respondRunnerRequest']}
                        elif method=='events.subscribe':result={'topics':[]}
                        elif method in ['crew.list','ask.list']:result=[]
                        elif method=='restart.pending':result=None
                        elif method=='message.post':
                            if mode=='remote-business-refused':
                                conn.sendall((json.dumps({'id':req['id'],'error':{'code':-32003,'message':'method not allowed','data':{'refusal':'remote.permission_denied'}}})+'\n').encode());continue
                            wire=json.dumps({'id':req['id'],'result':{'id':'synthetic-post'}})
                            if drop or mode=='eof':break # received once, result deliberately lost
                            if mode=='truncated':conn.sendall(wire[:len(wire)//2].encode());break
                            if mode=='final_reply':
                                shutdown=json.dumps({'event':'system.shutdown','data':{'reason':'hibernate'}})
                                conn.sendall((wire+'\n'+shutdown+'\n').encode());break
                            raise AssertionError('unexpected mutation')
                        else:raise AssertionError('unexpected request '+method)
                        conn.sendall((json.dumps({'id':req['id'],'result':result})+'\n').encode())
        except BaseException as e:errors.append(repr(e))
        finally:pathlib.Path(path).unlink(missing_ok=True)
    thread=None
    if mode!='startup':
        thread=threading.Thread(target=peer,kwargs={'incompatible':mode in ['incompatible','remote-refused'],'drop':mode=='retry' or mode in remotes},daemon=True);thread.start();assert ready.wait(3)
    env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(root),'SHELL':'/usr/bin/false','PENDINGCREW_CREWD_PROGRAM':'/usr/bin/false','PENDINGCREW_CODEX_PROGRAM':'/usr/bin/true','PENDINGCREW_DATA_DIR':str(base/'legacy')}
    with open(root/'probe.log','w') as log:
        process=subprocess.Popen([str(base/'probe'),str(root),mode],env=env,stdout=log,stderr=log)
        try:
            deadline=time.monotonic()+75
            while process.poll() is None and time.monotonic()<deadline:
                if (mode=='retry' or mode in remotes) and (root/'explicit-reconnect').exists() and not (root/'peer-ready').exists():
                    if thread:thread.join(timeout=2)
                    ready.clear();thread=threading.Thread(target=peer,daemon=True);thread.start();assert ready.wait(3)
                    (root/'peer-ready').write_text('ready')
                if mode=='retry' and (root/'second-drop').exists() and not second_dropped:
                    active[0].shutdown(socket.SHUT_RDWR);second_dropped=True
                if mode=='retry' and (root/'second-reconnect').exists() and not second_serving:
                    if thread:thread.join(timeout=2)
                    ready.clear();thread=threading.Thread(target=peer,daemon=True);thread.start();assert ready.wait(3)
                    second_serving=True
                time.sleep(.03)
            code=process.wait(timeout=2)
        except BaseException:
            process.kill();process.wait();raise
    stopping.set()
    if thread:thread.join(timeout=2)
    (root/'calls.json').write_text(json.dumps(calls,indent=2));(root/'peer-errors.json').write_text(json.dumps(errors))
    methods=[r['method'] for r in calls]
    mutation_count=methods.count('message.post')
    ok=code==0 and not errors and mutation_count==(1 if mode in ['retry']+terminal+['remote-foreground','remote-manual','remote-retired','remote-business-refused','remote-cancel-pending'] else 0)
    if mode in ['incompatible','remote-refused','remote-grant-refused']:ok=ok and methods==['system.hello']
    if mode=='remote-foreground':ok=ok and methods.count('system.hello')==2
    if mode in ['remote-manual','remote-retired']:ok=ok and methods.count('system.hello')==1
    if mode=='remote-cancel-pending':ok=ok and len(accepts)==1
    if mode=='remote-cancel-initial':ok=ok and len(accepts)==0
    (root/'physical-accepts.json').write_text(json.dumps({'count':len(accepts)}))
    if mode=='retry':ok=ok and methods.count('system.hello')==3 and second_dropped and second_serving
    print(mode,'PASS' if ok else 'FAIL','exit',code,'mutations',mutation_count,'calls',methods,flush=True)
    if not ok:failures.append(mode)
if failures:raise AssertionError('failed scenarios: '+', '.join(failures))
PYTHON
