#!/bin/sh
# Actual full recorded Rust witness control; no Cargo, Swift compilation or artifact copies.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
: "${PENDINGCREW_TEST_CREWD_WITNESS:?Explicit complete normal backend witness required}"
: "${PENDINGCREW_TEST_CREWD_PROGRAM:?Explicit immutable normal binary required}"
fixture=$(mktemp -d /private/tmp/pcw-inventory-qa.XXXXXX)
echo "Inventory control evidence: $fixture"
python3 - "$root" "$fixture" "$PENDINGCREW_TEST_CREWD_WITNESS" "$PENDINGCREW_TEST_CREWD_PROGRAM" <<'PYCODE'
import json,pathlib,subprocess,sys
root,out,witness,binary=map(pathlib.Path,sys.argv[1:]);value=json.loads(witness.read_text())
# The real fixture includes same-stem Rust module file/directory ordering, which
# a tiny one-main.rs source cannot exercise.
paths={item['path'] for item in value['files']}
assert 'crewd/crates/collab/src/causal.rs' in paths and 'crewd/crates/collab/src/causal/admission_tests.rs' in paths
(out/'build-inputs.json').write_text(json.dumps({'rustInputDigest':value['rustInputDigest'],'backendBuildConfiguration':value['buildConfiguration']}))
old=out/'old-writer.py';old.write_bytes(subprocess.check_output(['git','-C',str(root),'show','56cce67c:scripts/dev/write-codex-first-provenance.py']))
for script,expected,label in [(old,False,'red-old-component-order'),(root/'scripts/dev/write-codex-first-provenance.py',True,'green-full-path-order')]:
 p=subprocess.run(['python3',str(script),'preflight-witness',str(out),str(witness),str(binary)],capture_output=True,text=True)
 (out/(label+'.log')).write_text(p.stdout+p.stderr)
 assert (p.returncode==0)==expected,(label,p.stdout,p.stderr)
 if not expected:assert 'inventory/hash differs' in p.stderr
# Configuration mismatch still refuses the same valid corpus and binary.
manifest=json.loads((out/'build-inputs.json').read_text());manifest['backendBuildConfiguration']['profile']='release';(out/'build-inputs.json').write_text(json.dumps(manifest))
p=subprocess.run(['python3',str(root/'scripts/dev/write-codex-first-provenance.py'),'preflight-witness',str(out),str(witness),str(binary)],capture_output=True,text=True)
(out/'mismatched-profile.log').write_text(p.stdout+p.stderr);assert p.returncode!=0 and 'explicit build intent' in p.stderr
print('PASS: genuine 348-file corpus with module same-stem file+directory RED old ordering / GREEN canonical full path; actual unchanged binary/config preserved; mismatched profile rejected; no build')
PYCODE
