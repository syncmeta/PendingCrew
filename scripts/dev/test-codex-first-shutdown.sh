#!/bin/sh
# Synthetic protocol peer: shutdown stops new snapshot reads, not in-flight replies.
set -eu
stage=${1:-crew.list}
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-client-shutdown.XXXXXX)
echo "Full synthetic logs: $fixture"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library \
    "$root/Sources/Mac/Crewd/CrewdClient.swift" "$root/Sources/Mac/Crewd/CrewdWorkStore.swift" "$root/Sources/Mac/Crewd/CrewdNavigation.swift" "$root/Sources/Mac/Crewd/CrewdRequirementDrafts.swift" "$root/Sources/Mac/Crewd/CrewdPlanActions.swift" "$root/Sources/Mac/Crewd/CrewdWorkspace.swift" \
    "$root/scripts/dev/fixtures/crewd-client-shutdown.swift" -o "$fixture/probe"
python3 - "$fixture" "$stage" <<'PYTHON'
import json,pathlib,socket,subprocess,sys,time,threading
root=pathlib.Path(sys.argv[1]); path=str(root/'crewd.sock'); calls=[]; errors=[]
stage=sys.argv[2]
sequence=['system.hello','events.subscribe','crew.list','ask.list','restart.pending','session.list','channel.list']
assert stage in sequence[2:]
def serve():
    try:
        with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as listener:
            listener.bind(path); listener.listen(1)
            with listener.accept()[0] as conn:
                conn.settimeout(5)
                reader=conn.makefile('rb',buffering=0)
                while True:
                    line=reader.readline()
                    if not line: break
                    req=json.loads(line); method=req['method']; calls.append(method)
                    if method=='system.hello': result={'server':'crewd','apiVersion':3,'minApiVersion':3,'version':'synthetic','capabilities':['session.respondRunnerRequest']}
                    elif method=='events.subscribe': result={'topics':[]}
                    elif method=='crew.list': result=[{'id':'probe-crew','title':'synthetic'}]
                    elif method in ['session.list','channel.list']: result=[]
                    elif method=='ask.list': result=[]
                    elif method=='restart.pending': result=None
                    else: raise AssertionError('unexpected method '+method)
                    if method==stage:
                        conn.sendall((json.dumps({'event':'system.shutdown','data':{'reason':'hibernate'}})+'\n').encode())
                    conn.sendall((json.dumps({'id':req['id'],'result':result})+'\n').encode())
    except BaseException as e: errors.append(e)
thread=threading.Thread(target=serve); thread.start()
for _ in range(100):
    if pathlib.Path(path).exists(): break
    time.sleep(.01)
try: subprocess.run([str(root/'probe'),str(root)],check=True,timeout=10)
finally: thread.join(timeout=2)
(root/'calls.json').write_text(json.dumps(calls))
if errors: raise errors[0]
assert calls==sequence[:sequence.index(stage)+1], 'snapshot reads after shutdown: '+repr(calls)
print('PASS: shutdown event prevents further snapshot requests')
PYTHON
