#!/bin/sh
# Real Swift/Unix client + real crewd + synthetic app-server, with private HOME.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
target=${CARGO_TARGET_DIR:-$root/crewd/target}
# Explicit synthetic executables let acceptance target a frozen package without
# rebuilding unrelated sources or racing another worktree's Cargo cache.
if [ -z "${PENDINGCREW_TEST_CREWD_PROGRAM:-}" ] || [ -z "${PENDINGCREW_TEST_CODEX_PROGRAM:-}" ]; then
    (cd "$root/crewd" && cargo build --locked -p crewd -p crewd-exec --bins)
fi
server_program=${PENDINGCREW_TEST_CREWD_PROGRAM:-$target/debug/crewd}
fake_program=${PENDINGCREW_TEST_CODEX_PROGRAM:-$target/debug/fake_codex}
[ -x "$server_program" ] && [ -x "$fake_program" ] || { echo "synthetic programs are not executable" >&2; exit 2; }
fixture=$(mktemp -d /private/tmp/pcw-swift-client.XXXXXX)
mkdir -p "$fixture/home" "$fixture/work" "$fixture/cache" "$fixture/data"
echo "Full synthetic logs: $fixture"
xcrun swiftc -module-cache-path "$fixture/cache/swift" -parse-as-library "$root/Sources/Mac/Crewd/CrewdClient.swift" \
    "$root/Sources/Mac/Crewd/CrewdWorkStore.swift" "$root/Sources/Mac/Crewd/CrewdNavigation.swift" "$root/Sources/Mac/Crewd/CrewdRequirementDrafts.swift" "$root/Sources/Mac/Crewd/CrewdPlanActions.swift" "$root/Sources/Mac/Crewd/CrewdHumanActions.swift" "$root/Sources/Mac/Crewd/CrewdResources.swift" "$root/Sources/Mac/Crewd/CrewdResourceDrafts.swift" "$root/Sources/Mac/Crewd/CrewdWorkspace.swift" "$root/scripts/dev/fixtures/crewd-client-smoke.swift" -o "$fixture/client-test"
python3 - "$fixture" "$server_program" "$fake_program" <<'PY'
import os, pathlib, subprocess, sys, time
fixture = pathlib.Path(sys.argv[1])
server_program, fake_program = sys.argv[2:4]
env = {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'HOME': str(fixture/'home'),
       'SHELL':'/usr/bin/false', 'XDG_CACHE_HOME': str(fixture/'cache'), 'PENDINGCREW_DATA_DIR': str(fixture/'legacy')}
args = [server_program,'--data-dir',str(fixture/'data'),'serve',
        '--codex',fake_program,'--claude','/usr/bin/false',
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
        subprocess.run([str(fixture/'client-test'),str(fixture/'data'),str(fixture/'rpc.log')],env=env,check=True,timeout=60)
        server.wait(timeout=20)
        if server.returncode: raise RuntimeError('synthetic server exit '+str(server.returncode))
    finally:
        if server.poll() is None:
            server.terminate()
            try: server.wait(timeout=10)
            except subprocess.TimeoutExpired: server.kill(); server.wait()
PY
