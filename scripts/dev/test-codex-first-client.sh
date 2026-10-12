#!/bin/sh
# Real Swift/Unix client + real crewd + synthetic app-server, with private HOME.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
target=${CARGO_TARGET_DIR:-$root/crewd/target}
(cd "$root/crewd" && cargo build --locked -p crewd -p crewd-exec --bins)
fixture=$(mktemp -d /private/tmp/pcw-swift-client.XXXXXX)
mkdir -p "$fixture/home" "$fixture/work" "$fixture/cache" "$fixture/data"
xcrun swiftc -parse-as-library "$root/Sources/Mac/Crewd/CrewdClient.swift" \
    "$root/Sources/Mac/Crewd/CrewdWorkspace.swift" "$root/scripts/dev/fixtures/crewd-client-smoke.swift" -o "$fixture/client-test"
python3 - "$target" "$fixture" <<'PY'
import os, pathlib, subprocess, sys, time
target, fixture = map(pathlib.Path, sys.argv[1:])
env = {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'HOME': str(fixture/'home'),
       'SHELL':'/usr/bin/false', 'XDG_CACHE_HOME': str(fixture/'cache'), 'PENDINGCREW_DATA_DIR': str(fixture/'legacy')}
args = [str(target/'debug/crewd'),'--data-dir',str(fixture/'data'),'serve',
        '--codex',str(target/'debug/fake_codex'),'--claude','/usr/bin/false',
        '--codex-env','HOME='+str(fixture/'home'),'--codex-env','SHELL=/usr/bin/false',
        '--codex-env','FAKE_CODEX_MODE=human_requests',
        '--codex-env','FAKE_CODEX_LOG='+str(fixture/'rpc.log')]
with open(fixture/'server.log','w') as log:
    server=subprocess.Popen(args,env=env,stdout=log,stderr=log)
    try:
        for _ in range(100):
            if (fixture/'data/crewd.sock').exists(): break
            if server.poll() is not None: raise RuntimeError('synthetic server exited')
            time.sleep(.05)
        subprocess.run([str(fixture/'client-test'),str(fixture/'data')],env=env,check=True,timeout=60)
        server.wait(timeout=20)
        if server.returncode: raise RuntimeError('synthetic server exit '+str(server.returncode))
    finally:
        if server.poll() is None:
            server.terminate()
            try: server.wait(timeout=10)
            except subprocess.TimeoutExpired: server.kill(); server.wait()
print('Full synthetic logs: '+str(fixture))
PY
