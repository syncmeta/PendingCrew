#!/bin/sh
# Actual offline compiler success/failure in a tiny dependency-free synthetic repo.
# Run only after the Cargo owner hands off an execution window; not PendingCrew build.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-build-record-qa.XXXXXX)
echo "Actual build-record evidence: $fixture"
python3 - "$root" "$fixture" <<'PY'
import json,os,pathlib,subprocess,sys
root,base=map(pathlib.Path,sys.argv[1:]);repo=base/'repo';repo.mkdir()
env=dict(os.environ,GIT_CONFIG_GLOBAL='/dev/null',GIT_CONFIG_NOSYSTEM='1',RUSTUP_TOOLCHAIN='1.94.0',CARGO_INCREMENTAL='0',CARGO_TARGET_DIR=str(base/'tiny-target'))
def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],env=env)
git('init','-q','--initial-branch=codex/compiler-control');(repo/'crewd/src').mkdir(parents=True)
(repo/'crewd/Cargo.toml').write_text('[package]\nname = "crewd"\nversion = "0.0.0"\nedition = "2024"\n')
(repo/'crewd/Cargo.lock').write_text('version = 4\n\n[[package]]\nname = "crewd"\nversion = "0.0.0"\n')
source=repo/'crewd/src/main.rs';source.write_text('fn main() { println!("synthetic compiler control only"); }\n')
def commit(name):
 git('add','crewd');git('-c','user.name=QA fixture','-c','user.email=fixture@invalid','-c','commit.gpgSign=false','commit','-qm',name);return git('rev-parse','HEAD').decode().strip()
record=root/'scripts/dev/record-crewd-build.py'
def run(stage,out,ref,want):
 p=subprocess.run(['python3',str(record),'prepare',str(repo),str(out),ref],env=env,capture_output=True,text=True);assert p.returncode==0,(p.stdout,p.stderr)
 p=subprocess.run(['python3',str(record),'build',str(repo),str(out),str(base/'tiny-target/debug/crewd'),'cargo','build','-p','crewd','--locked','--offline','--message-format=json'],env=env,capture_output=True,text=True)
 (base/(stage+'.log')).write_text(p.stdout+p.stderr);assert (p.returncode==0)==want,(stage,p.stdout,p.stderr)
 if want:
  v=json.loads((out/'backend-witness.json').read_text());cfg=v['buildConfiguration'];assert cfg['profile']=='debug' and cfg['defaultFeatures'] and not cfg['allFeatures'];assert cfg['artifactProfile']['test'] is False and cfg['workspaceFeatures']=={'crewd':[]};assert (out/v['binaryFile']).is_file();assert 'Exit: 0' in (out/v['buildLog']).read_text()
 else:
  assert not (out/'backend-witness.json').exists(),'failed compiler must not emit a completed build witness';assert 'Exit: 101' in (out/'cargo-build.log').read_text()
run('actual-success',base/'success-evidence',commit('valid synthetic source'),True)
source.write_text('fn main() { deliberate invalid syntax }\n')
run('actual-failure',base/'failure-evidence',commit('real compiler failure control'),False)
print('PASS: actual tiny offline compiler success records JSON features/profile/binary/input; actual failure retains error and no completed witness')
PY
