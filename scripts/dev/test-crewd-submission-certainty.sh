#!/bin/sh
# Offline actual Unix transport certainty; no model/app/Cargo.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-submission-certainty.XXXXXX)
echo "Submission evidence: $fixture"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$root/Sources/Mac/Crewd/CrewdClient.swift" "$root/Sources/Mac/Crewd/CrewdWorkStore.swift" "$root/scripts/dev/fixtures/crewd-submission-certainty.swift" -o "$fixture/test"
python3 - "$fixture" <<'PYCODE'
import json,pathlib,socket,subprocess,sys,threading
p=pathlib.Path(sys.argv[1]);listener=socket.socket(socket.AF_UNIX);listener.bind(str(p/'wire.sock'));listener.listen(1);seen=[]
def serve():
 conn,_=listener.accept()
 with conn,conn.makefile('r') as stream:
  for line in stream:
   request=json.loads(line);seen.append(request['method'])
   if request['method']=='system.hello': result={'server':'crewd','apiVersion':3,'minApiVersion':1,'version':'fixture','capabilities':['session.respondRunnerRequest']}
   elif request['method']=='events.subscribe': result={}
   else: continue # The write really arrived, but the receipt is intentionally withheld.
   conn.sendall((json.dumps({'id':request['id'],'result':result})+'\n').encode())
t=threading.Thread(target=serve,daemon=True);t.start()
try:
 result=subprocess.run([str(p/'test'),str(p/'wire.sock')],timeout=30)
 t.join(timeout=5)
 assert seen.count('work.update')==1,seen
 (p/'wire-methods.json').write_text(json.dumps(seen))
 sys.exit(result.returncode)
finally: listener.close()
PYCODE
