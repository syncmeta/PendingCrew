#!/bin/sh
# Offline Swift data layer and real NDJSON error decoding; no app, account or runner.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-work-store.XXXXXX)
echo "Evidence: $fixture"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library \
 "$root/Sources/Mac/Crewd/CrewdClient.swift" "$root/Sources/Mac/Crewd/CrewdWorkStore.swift" \
 "$root/Sources/Mac/Crewd/CrewdNavigation.swift" "$root/Sources/Mac/Crewd/CrewdRequirementDrafts.swift" "$root/Sources/Mac/Crewd/CrewdPlanActions.swift" "$root/Sources/Mac/Crewd/CrewdHumanActions.swift" "$root/Sources/Mac/Crewd/CrewdWorkspace.swift" "$root/scripts/dev/fixtures/crewd-work-store.swift" -o "$fixture/test"
python3 - "$fixture" <<'PYCODE'
import pathlib, socket, json, subprocess, threading, sys
p=pathlib.Path(sys.argv[1]); listener=socket.socket(socket.AF_UNIX); listener.bind(str(p/'wire.sock')); listener.listen(1)
seen=[]
def serve():
 conn,_=listener.accept()
 with conn, conn.makefile('r') as f:
  for line in f:
   req=json.loads(line); seen.append(req['method'])
   if req['method']=='system.hello': result={'server':'crewd','apiVersion':3,'minApiVersion':1,'version':'fixture','capabilities':['session.respondRunnerRequest']}
   elif req['method']=='events.subscribe': result={}
   else:
    conn.sendall((json.dumps({'id':req['id'],'error':{'code':-32009,'message':'conflict','data':{'refusal':'work.revision_conflict','currentWork':{'revision':'18446744073709551615'},'unknown':True}}})+'\n').encode()); continue
   conn.sendall((json.dumps({'id':req['id'],'result':result})+'\n').encode())
t=threading.Thread(target=serve,daemon=True); t.start()
try:
 subprocess.run([str(p/'test'),str(p/'wire.sock')],check=True,timeout=30)
 t.join(timeout=5)
 assert seen.count('work.update')==1,seen
 (p/'wire-methods.json').write_text(json.dumps(seen))
finally: listener.close()
PYCODE
