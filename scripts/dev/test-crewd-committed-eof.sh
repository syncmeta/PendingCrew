#!/bin/sh
# Actual frozen normal backend commit -> physical proxy EOF -> shared typed recovery.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
: "${PENDINGCREW_TEST_CREWD_PROGRAM:?explicit packaged helper required}"
: "${FAKE_CODEX_PROGRAM:?explicit fake executable required}"
: "${PENDINGCREW_TEST_FAKE_SHA256:?explicit verified fake digest required}"
[ -x "$PENDINGCREW_TEST_CREWD_PROGRAM" ] && [ -x "$FAKE_CODEX_PROGRAM" ]
fixture=$(mktemp -d /private/tmp/pcw-committed-eof.XXXXXX)
echo "Committed EOF evidence: $fixture"
ref=$(git -C "$root" rev-parse "${PENDINGCREW_CLIENT_REF:-HEAD}")
printf '%s\n' "$ref" > "$fixture/source-ref.txt"
mkdir "$fixture/source"
git -C "$root" archive "$ref" Sources/Mac/Crewd Sources/Mac/Design/CrewColorHash.swift | tar -x -C "$fixture/source"
cp "$root/scripts/dev/fixtures/crewd-committed-eof.swift" "$fixture/fixture.swift"
find "$fixture/source" -type f -exec shasum -a 256 {} \; > "$fixture/source-sha256.txt"
shasum -a 256 "$fixture/fixture.swift" "$PENDINGCREW_TEST_CREWD_PROGRAM" "$FAKE_CODEX_PROGRAM" >> "$fixture/source-sha256.txt"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$fixture/source"/Sources/Mac/Crewd/*.swift "$fixture/source/Sources/Mac/Design/CrewColorHash.swift" "$fixture/fixture.swift" -o "$fixture/probe"
python3 - "$fixture" "$PENDINGCREW_TEST_CREWD_PROGRAM" "$FAKE_CODEX_PROGRAM" "$PENDINGCREW_TEST_FAKE_SHA256" <<'PY'
import pathlib,sys,os,subprocess,socket,threading,json,time,hashlib
p=pathlib.Path(sys.argv[1]);helper,fake,expected_fake=sys.argv[2:]
assert hashlib.sha256(pathlib.Path(fake).read_bytes()).hexdigest()==expected_fake,'unverified fake; no real executor fallback'
for name in ['home','work','data']: (p/name).mkdir()
env={'HOME':str(p/'home'),'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','SHELL':'/usr/bin/false','PENDINGCREW_DATA_DIR':str(p/'legacy')}
log=open(p/'server.log','w');server=subprocess.Popen([helper,'--data-dir',str(p/'data'),'serve','--codex',fake,'--claude','/usr/bin/false','--codex-env','HOME='+str(p/'home'),'--codex-env','SHELL=/usr/bin/false'],env=env,stdout=log,stderr=log)
listener=socket.socket(socket.AF_UNIX);listener.bind(str(p/'proxy.sock'));listener.listen(4);records=[];lock=threading.Lock();errors=[];dropped=[False]
def connection(client):
 try:
  with client,socket.socket(socket.AF_UNIX) as backend:
   backend.connect(str(p/'data/crewd.sock'));incoming=backend.makefile('rb')
   for raw in client.makefile('rb'):
    q=json.loads(raw)
    with lock:
     records.append(q);(p/'requests.json').write_text(json.dumps(records,ensure_ascii=False,indent=2))
     if (p/'readonly-complete').exists() and not (p/'readonly-methods.json').exists(): (p/'readonly-methods.json').write_text(json.dumps(records[:-1]))
    backend.sendall(raw)
    while True:
     response=incoming.readline();assert response,'backend unexpectedly closed'
     value=json.loads(response)
     if value.get('id')==q['id'] and q['method']=='todo.create' and not dropped[0]:
      assert 'result' in value,value
      (p/'actual-committed-withheld-response.json').write_bytes(response);dropped[0]=True
      client.shutdown(socket.SHUT_RDWR);return
     client.sendall(response)
     if value.get('id')==q['id']:break
 except BaseException as error: errors.append(repr(error))
def accept():
 try:
  while True:
   client,_=listener.accept();threading.Thread(target=connection,args=(client,),daemon=True).start()
 except OSError:pass
try:
 for _ in range(200):
  if (p/'data/crewd.sock').exists():break
  assert server.poll() is None,'actual server exited';time.sleep(.025)
 threading.Thread(target=accept,daemon=True).start()
 subprocess.run([str(p/'probe'),str(p)],env=env,check=True,timeout=45)
 assert dropped[0] and not errors,errors
 readonly=json.loads((p/'readonly-methods.json').read_text());assert sum(q['method']=='todo.create' for q in readonly)==1,readonly
 creates=[q for q in records if q['method']=='todo.create'];assert len(creates)==2 and creates[0]['params']==creates[1]['params'],creates
 (p/'RESULTS.json').write_text(json.dumps({'actualEOF':True,'initialCreates':1,'automaticReplays':0,'explicitSameRequestConfirmations':1,'serverReplyRetained':True}))
finally:
 listener.close();server.terminate()
 try:server.wait(timeout=15)
 except subprocess.TimeoutExpired:server.kill();server.wait()
 log.close()
PY
