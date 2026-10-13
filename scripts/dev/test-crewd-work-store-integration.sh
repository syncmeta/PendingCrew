#!/bin/sh
# Explicit synthetic executables only; never builds Cargo or discovers a real Codex.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-work-integration.XXXXXX)
echo "Integration evidence: $fixture"
xcrun swiftc -module-cache-path "$fixture/cache/swift" -parse-as-library \
 "$root/Sources/Mac/Crewd/CrewdClient.swift" "$root/Sources/Mac/Crewd/CrewdWorkStore.swift" \
 "$root/scripts/dev/fixtures/crewd-work-store-integration.swift" -o "$fixture/client-test"
if [ "${1:-}" = "--compile-only" ]; then echo "PASS: integration Swift fixture compiled only"; exit 0; fi
: "${PENDINGCREW_TEST_CREWD_PROGRAM:?Set the absolute gated crewd binary path}"
: "${FAKE_CODEX_PROGRAM:?Set the absolute fake_codex binary path; real accounts are forbidden}"
case "$PENDINGCREW_TEST_CREWD_PROGRAM" in /*) ;; *) echo "crewd path must be absolute" >&2; exit 2;; esac
case "$FAKE_CODEX_PROGRAM" in /*/fake_codex) ;; *) echo "fake path must be absolute and named fake_codex" >&2; exit 2;; esac
[ -x "$PENDINGCREW_TEST_CREWD_PROGRAM" ] && [ -x "$FAKE_CODEX_PROGRAM" ] || { echo "fixture binaries not executable" >&2; exit 2; }
python3 - "$fixture" "$PENDINGCREW_TEST_CREWD_PROGRAM" "$FAKE_CODEX_PROGRAM" <<'PYCODE'
import os, pathlib, subprocess, sys, time, json
p=pathlib.Path(sys.argv[1]); server_program,fake_program=sys.argv[2:]
for name in ['home','work','cache','data','legacy/whiteboards']: (p/name).mkdir(parents=True,exist_ok=True)
env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(p/'home'),'SHELL':'/usr/bin/false','XDG_CACHE_HOME':str(p/'cache'),'PENDINGCREW_DATA_DIR':str(p/'legacy')}
stamp='2026-10-13T00:00:00.000Z'; crew='synthetic-source-crew'; owner='worker-synthetic-source'
# Synthetic legacy source imported through the real CLI before daemon startup.
(p/'legacy/local-crews.json').write_text(json.dumps({'version':1,'nextCrewNumber':2,'crews':[{'id':crew,'title':'Synthetic source crew','createdAt':stamp,'updatedAt':stamp,'crewNumber':1,'workingDirectory':str(p/'work'),'sessionMembers':[{'sessionId':owner,'displayName':'Paused synthetic owner','createdAt':stamp}]}]}))
(p/'legacy/whiteboards'/f'{crew}.todos.json').write_text(json.dumps([{'id':'synthetic-original-demand','number':1,'text':'synthetic original demand','status':'pending','createdAt':stamp}]))
with open(p/'import.log','w') as log:
 subprocess.run([server_program,'--data-dir',str(p/'data'),'import','--from',str(p/'legacy')],env=env,stdout=log,stderr=log,check=True,timeout=30)
args=[server_program,'--data-dir',str(p/'data'),'serve','--codex',fake_program,'--claude','/usr/bin/false','--codex-env','HOME='+str(p/'home'),'--codex-env','SHELL=/usr/bin/false','--codex-env','FAKE_CODEX_MODE=human_requests','--codex-env','FAKE_CODEX_LOG='+str(p/'fake-rpc.log')]
with open(p/'server.log','w') as log:
 server=subprocess.Popen(args,env=env,stdout=log,stderr=log)
 try:
  for _ in range(200):
   if (p/'data/crewd.sock').exists(): break
   if server.poll() is not None: raise RuntimeError('synthetic daemon exited')
   time.sleep(.05)
  subprocess.run([str(p/'client-test'),str(p/'data')],env=env,check=True,timeout=60)
  # Imported owner is stopped/unknown, so no runner launch is required for this data workflow.
  assert not (p/'fake-rpc.log').exists() or not (p/'fake-rpc.log').read_text().strip(),'unexpected fake runner traffic'
 finally:
  if server.poll() is None:
   server.terminate()
   try: server.wait(timeout=20)
   except subprocess.TimeoutExpired: server.kill(); server.wait()
PYCODE
