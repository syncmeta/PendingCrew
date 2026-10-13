#!/bin/sh
# Private synthetic backend restart: real Mac client, no user accounts or providers.
set -eu
mode=${1:-graceful}
case "$mode" in graceful|crash) ;; *) echo "expected graceful or crash" >&2; exit 2 ;; esac
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
target=${CARGO_TARGET_DIR:-$root/crewd/target}
(cd "$root/crewd" && cargo build --locked -p crewd -p crewd-exec --bins)
fixture=$(mktemp -d /private/tmp/pcw-swift-restart.XXXXXX)
mkdir -p "$fixture/home" "$fixture/work" "$fixture/cache" "$fixture/data"
echo "Full synthetic logs: $fixture"
xcrun swiftc -module-cache-path "$fixture/cache/swift" -parse-as-library \
    "$root/Sources/Mac/Crewd/CrewdClient.swift" "$root/Sources/Mac/Crewd/CrewdWorkStore.swift" "$root/Sources/Mac/Crewd/CrewdNavigation.swift" "$root/Sources/Mac/Crewd/CrewdRequirementDrafts.swift" "$root/Sources/Mac/Crewd/CrewdPlanActions.swift" "$root/Sources/Mac/Crewd/CrewdWorkspace.swift" \
    "$root/scripts/dev/fixtures/crewd-client-restart.swift" -o "$fixture/client-test"
python3 - "$target" "$fixture" "$mode" <<'PYTHON'
import os, pathlib, socket, subprocess, sys, time
target, fixture = map(pathlib.Path, sys.argv[1:3])
mode = sys.argv[3]
env = {'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(fixture/'home'),
       'SHELL':'/usr/bin/false','PENDINGCREW_DATA_DIR':str(fixture/'legacy')}
args = [str(target/'debug/crewd'),'--data-dir',str(fixture/'data'),'serve',
        '--codex',str(target/'debug/fake_codex'),'--claude','/usr/bin/false',
        '--codex-env','HOME='+str(fixture/'home'),'--codex-env','SHELL=/usr/bin/false',
        '--codex-env','FAKE_CODEX_LOG='+str(fixture/'rpc.log')]
for phase in ['before','after']:
    with (fixture/(phase+'-server.log')).open('w') as log:
        server=subprocess.Popen(args,env=env,stdout=log,stderr=log)
        try:
            for _ in range(100):
                if server.poll() is not None: raise RuntimeError('synthetic server exited')
                try:
                    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
                        probe.connect(str(fixture/'data/crewd.sock'))
                    break
                except OSError:
                    pass
                time.sleep(.05)
            subprocess.run([str(fixture/'client-test'),str(fixture/'data'),str(fixture/'rpc.log'),phase],env=env,check=True,timeout=30)
        finally:
            if server.poll() is None:
                if phase == "before" and mode == "crash": server.kill()
                else: server.terminate()
            try: server.wait(timeout=15)
            except subprocess.TimeoutExpired: server.kill(); server.wait()
        expected = -9 if phase == 'before' and mode == 'crash' else 0
        if server.returncode != expected: raise RuntimeError('synthetic backend exit '+str(server.returncode))
print('PASS: native Mac client across backend process restart')
PYTHON
