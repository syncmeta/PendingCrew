#!/bin/sh
# Focused actual CLI testkit difference; never all-features or a real provider.
# Run only in the owning CI job / after the shared Cargo target lease is granted.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
cd "$root/crewd"
evidence=${1:?explicit evidence directory required}
case "$evidence" in /*) ;; *) echo "absolute evidence directory required" >&2; exit 2 ;; esac
mkdir -p "$evidence"
if [ -e "$evidence/feature-result.json" ]; then echo "retain terminal evidence; choose fresh evidence directory" >&2; exit 2; fi
git -C "$root" rev-parse HEAD > "$evidence/source-ref.txt"
cargo --version > "$evidence/cargo-version.txt"
rustc -Vv > "$evidence/rustc-version.txt"
# Keep the existing runner target/cache. These two binaries are synthetic CLI
# test inputs; no normal candidate package is declared by this feature build.
cargo build --locked --offline -p crewd --bin crewd --features testkit --message-format=json > "$evidence/feature-cli-build.log" 2>&1
cargo build --locked --offline -p crewd-exec --bin fake_claude --features testkit --message-format=json > "$evidence/fake-child-build.log" 2>&1
python3 - "$evidence" <<'CHECK'
import hashlib,json,pathlib,sys
e=pathlib.Path(sys.argv[1]);matched={}
for name,filename in [('crewd','feature-cli-build.log'),('fake_claude','fake-child-build.log')]:
 for line in (e/filename).read_text().splitlines():
  try:v=json.loads(line)
  except json.JSONDecodeError:continue
  if v.get('reason')=='compiler-artifact' and v.get('target',{}).get('name')==name and v.get('executable'):
   if 'testkit' not in v.get('features',[]) or v.get('profile',{}).get('test') is not False:raise SystemExit('actual feature-enabled normal binary evidence missing: '+name)
   binary=pathlib.Path(v['executable'])
   matched[name]={'path':str(binary),'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'packageID':v['package_id'],'manifestPath':v['manifest_path'],'features':v['features'],'fresh':v.get('fresh')}
 if name not in matched:raise SystemExit('actual compiler-artifact missing: '+name)
(e/'feature-binaries.json').write_text(json.dumps(matched,indent=2,sort_keys=True)+'\n')
CHECK
# Reuse the existing literal CLI scenario. Its own fixture clears the env and
# uses fake_claude, private HOME/data, false SHELL, real registry enrollment,
# public RPC siblings and strict actual birth/registry/socket shutdown oracles.
cargo test --locked --offline -p crewd --features testkit --test s1_literal_enrollment s1_literal_rpc_sibling_waits_real_driver_enrollment_and_pending_managed_peer_stays_denied -- --exact --nocapture > "$evidence/actual-cli-enrollment.log" 2>&1
python3 - "$evidence" <<'RESULT'
import hashlib,json,pathlib,sys
e=pathlib.Path(sys.argv[1]);log=(e/'actual-cli-enrollment.log').read_text()
for name,binary in json.loads((e/'feature-binaries.json').read_text()).items():
 if hashlib.sha256(pathlib.Path(binary['path']).read_bytes()).hexdigest()!=binary['sha256']:raise SystemExit('runtime test replaced the observed feature binary: '+name)
if 'test result: ok. 1 passed; 0 failed;' not in log:raise SystemExit('exact actual CLI flow did not run once')
(e/'feature-result.json').write_text(json.dumps({'sourceRef':(e/'source-ref.txt').read_text().strip(),'scope':'crewd/testkit actual CLI enrollment difference only','exactTest':'s1_literal_rpc_sibling_waits_real_driver_enrollment_and_pending_managed_peer_stays_denied','actualAssertions':['true held driver enrollment','human sibling pending no effects','managed root and child Human routes denied','same original Human request commits once after release','generated helper attach/tools accepted','physical birth/socket/registry closed'],'realModel':False,'allFeaturesSuite':False,'outcome':'passed'},indent=2)+'\n')
print('PASS actual CLI testkit enrollment difference; not historical workspace all-features completion')
RESULT
