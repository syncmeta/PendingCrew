#!/bin/sh
# Explicit fake executor + actual daemon + frozen Swift source. No real account discovery.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-channel-integration.XXXXXX)
echo "Channel actual-daemon evidence: $fixture"
ref=$(git -C "$root" rev-parse --verify "${PENDINGCREW_CLIENT_REF:-HEAD}^{commit}")
mkdir -p "$fixture/source" "$fixture/captures"
echo "$ref" > "$fixture/client-source-ref.txt"
git -C "$root" ls-tree -r --name-only "$ref" -- Sources/Mac/Crewd > "$fixture/source-paths.txt"
while IFS= read -r source_file; do
 case "$source_file" in *.swift) git -C "$root" show "$ref:$source_file" > "$fixture/source/$(basename "$source_file")";; esac
done < "$fixture/source-paths.txt"
git -C "$root" show "$ref:Sources/Mac/Design/CrewColorHash.swift" > "$fixture/source/CrewColorHash.swift"
cp "$root/scripts/dev/fixtures/crewd-channel-integration.swift" "$fixture/source/fixture.swift"
shasum -a 256 "$fixture"/source/*.swift > "$fixture/client-source-sha256.txt"
xcrun swiftc -module-cache-path "$fixture/cache/swift" -parse-as-library "$fixture"/source/*.swift -o "$fixture/client-test"
if [ "${1:-}" = "--compile-only" ]; then echo "PASS: frozen channel Swift fixture compiled only"; exit 0; fi
: "${PENDINGCREW_TEST_CREWD_PROGRAM:?Explicit fresh crewd binary required}"
: "${FAKE_CODEX_PROGRAM:?Explicit fake_codex binary required}"
case "$PENDINGCREW_TEST_CREWD_PROGRAM" in /*) ;; *) exit 2;; esac
case "$FAKE_CODEX_PROGRAM" in /*/fake_codex) ;; *) exit 2;; esac
[ -x "$PENDINGCREW_TEST_CREWD_PROGRAM" ] && [ -x "$FAKE_CODEX_PROGRAM" ]
cp "$root/scripts/dev/fixtures/crewd-fake-codex-capture.py" "$fixture/fake_codex"
chmod 700 "$fixture/fake_codex"
mkdir -p "$fixture/bin"
cp "$PENDINGCREW_TEST_CREWD_PROGRAM" "$fixture/bin/crewd"
cp "$FAKE_CODEX_PROGRAM" "$fixture/bin/fake_codex"
printf '%s\n' "${PENDINGCREW_BINARY_SOURCE_REF:-unprovided}" > "$fixture/backend-source-ref.txt"
shasum -a 256 "$fixture/bin/crewd" "$fixture/bin/fake_codex" "$fixture/fake_codex" > "$fixture/binary-sha256.txt"
python3 - "$fixture" "$fixture/bin/crewd" "$fixture/bin/fake_codex" <<'PYCODE'
import os,pathlib,subprocess,sys,time
p=pathlib.Path(sys.argv[1]);server_program,fake_program=sys.argv[2:]
for name in ['home','work','cache','data']: (p/name).mkdir(parents=True,exist_ok=True)
env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(p/'home'),'SHELL':'/usr/bin/false','XDG_CACHE_HOME':str(p/'cache')}
args=[server_program,'--data-dir',str(p/'data'),'serve','--codex',str(p/'fake_codex'),'--claude','/usr/bin/false','--codex-env','HOME='+str(p/'home'),'--codex-env','SHELL=/usr/bin/false','--codex-env','FAKE_CODEX_MODE=crew_mcp','--codex-env','FAKE_CODEX_QA_REAL_PROGRAM='+fake_program,'--codex-env','FAKE_CODEX_QA_CAPTURE_DIR='+str(p/'captures')]
with open(p/'server.log','w') as log:
 server=subprocess.Popen(args,env=env,stdout=log,stderr=log)
 try:
  for _ in range(200):
   if (p/'data/crewd.sock').exists(): break
   if server.poll() is not None: raise RuntimeError('synthetic daemon exited')
   time.sleep(.05)
  subprocess.run([str(p/'client-test'),str(p/'data'),str(p/'captures'),str(p/'work')],env=env,check=True,timeout=90)
 finally:
  if server.poll() is None:
   server.terminate()
   try: server.wait(timeout=20)
   except subprocess.TimeoutExpired: server.kill(); server.wait()
PYCODE
