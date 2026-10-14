#!/bin/sh
# Actual normal helper hello; no model, GUI, LAN, personal data or Cargo.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
: "${PENDINGCREW_TEST_NORMAL_HELPER:?explicit recorded normal helper required}"
fixture=$(mktemp -d /private/tmp/pcw-session-start-capability.XXXXXX)
echo "Capability evidence: $fixture"
xcrun swiftc -module-cache-path /private/tmp/pcw-codex-first-xcode/ModuleCache.noindex -parse-as-library "$root/Sources/Mac/Crewd/CrewdClient.swift" "$root/scripts/dev/fixtures/crewd-session-start-capability.swift" -o "$fixture/probe"
python3 - "$PENDINGCREW_TEST_NORMAL_HELPER" "$fixture" <<'PY'
import hashlib,json,pathlib,subprocess,sys,time
helper=pathlib.Path(sys.argv[1]).resolve();base=pathlib.Path(sys.argv[2]);assert helper.is_file()
for d in ['data','home']:(base/d).mkdir(mode=0o700)
env={'PATH':'/usr/bin:/bin','HOME':str(base/'home'),'SHELL':'/usr/bin/false'}
args=[str(helper),'--data-dir',str(base/'data'),'serve','--codex','/usr/bin/false','--claude','/usr/bin/false','--codex-env','HOME='+str(base/'home'),'--claude-env','HOME='+str(base/'home')]
(base/'inputs.json').write_text(json.dumps({'helper':str(helper),'sha256':hashlib.sha256(helper.read_bytes()).hexdigest(),'command':args},indent=2)+'\n')
with (base/'daemon.log').open('w') as log:
 p=subprocess.Popen(args,env=env,stdout=log,stderr=log)
 try:
  for _ in range(200):
   assert p.poll() is None,'actual normal helper exited'
   if (base/'data/crewd.sock').exists():break
   time.sleep(.05)
  else:raise AssertionError('actual normal helper socket missing')
  probe=subprocess.run([str(base/'probe'),str(base/'data/crewd.sock'),str(base/'actual-hello.json')],env=env,capture_output=True,timeout=20)
  (base/'probe.log').write_bytes(probe.stdout+probe.stderr);assert probe.returncode==0,'see '+str(base/'probe.log')
 finally:
  if p.poll() is None:p.terminate()
  p.wait(timeout=20)
 assert p.returncode==0
print('PASS recorded normal production hello/shared client gate with no provider execution')
PY
