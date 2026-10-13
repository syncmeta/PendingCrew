#!/bin/sh
# Real workspace/client + tiny Unix protocol peer; no Cargo, GUI or real runner.
set -eu
mode=${1:-all}
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-client-failure.XXXXXX)
echo "Failure evidence: $fixture"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library \
    "$root/Sources/Mac/Crewd/CrewdClient.swift" "$root/Sources/Mac/Crewd/CrewdWorkStore.swift" "$root/Sources/Mac/Crewd/CrewdNavigation.swift" "$root/Sources/Mac/Crewd/CrewdWorkspace.swift" \
    "$root/scripts/dev/fixtures/crewd-client-failure.swift" -o "$fixture/probe"
python3 - "$fixture" "$mode" <<'PYTHON'
import json,os,pathlib,socket,subprocess,sys,threading,time
base=pathlib.Path(sys.argv[1]); terminal=['final_reply','eof','truncated']; available=['startup','incompatible','retry']+terminal
modes=available if sys.argv[2]=='all' else terminal if sys.argv[2]=='transport' else [sys.argv[2]]
assert all(m in available for m in modes)
failures=[]
for mode in modes:
    root=base/mode;root.mkdir();path=str(root/'crewd.sock');calls=[];errors=[];ready=threading.Event();active=[];second_dropped=False;second_serving=False
    def peer(incompatible=False,drop=False):
        try:
            with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as listener:
                listener.bind(path);listener.listen(1);listener.settimeout(70);ready.set()
                with listener.accept()[0] as conn:
                    active[:]=[conn]
                    conn.settimeout(65);reader=conn.makefile('rb',buffering=0)
                    while True:
                        line=reader.readline()
                        if not line:break
                        req=json.loads(line);method=req['method'];calls.append(req)
                        if method=='system.hello':result={'server':'crewd','apiVersion':4 if incompatible else 3,'minApiVersion':4 if incompatible else 3,'version':'synthetic','capabilities':['session.respondRunnerRequest']}
                        elif method=='events.subscribe':result={'topics':[]}
                        elif method in ['crew.list','ask.list']:result=[]
                        elif method=='restart.pending':result=None
                        elif method=='message.post':
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
        thread=threading.Thread(target=peer,kwargs={'incompatible':mode=='incompatible','drop':mode=='retry'},daemon=True);thread.start();assert ready.wait(3)
    env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(root),'SHELL':'/usr/bin/false','PENDINGCREW_CREWD_PROGRAM':'/usr/bin/false','PENDINGCREW_CODEX_PROGRAM':'/usr/bin/true','PENDINGCREW_DATA_DIR':str(base/'legacy')}
    with open(root/'probe.log','w') as log:
        process=subprocess.Popen([str(base/'probe'),str(root),mode],env=env,stdout=log,stderr=log)
        try:
            deadline=time.monotonic()+75
            while process.poll() is None and time.monotonic()<deadline:
                if mode=='retry' and (root/'explicit-reconnect').exists() and not (root/'peer-ready').exists():
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
    if thread:thread.join(timeout=2)
    (root/'calls.json').write_text(json.dumps(calls,indent=2));(root/'peer-errors.json').write_text(json.dumps(errors))
    methods=[r['method'] for r in calls]
    mutation_count=methods.count('message.post')
    ok=code==0 and not errors and mutation_count==(1 if mode in ['retry']+terminal else 0)
    if mode=='incompatible':ok=ok and methods==['system.hello']
    if mode=='retry':ok=ok and methods.count('system.hello')==3 and second_dropped and second_serving
    print(mode,'PASS' if ok else 'FAIL','exit',code,'mutations',mutation_count,'calls',methods,flush=True)
    if not ok:failures.append(mode)
if failures:raise AssertionError('failed scenarios: '+', '.join(failures))
PYTHON
