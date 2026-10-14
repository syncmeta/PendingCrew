#!/usr/bin/env python3
"""Build a inspectable development Linux backend candidate. Never install or publish."""
import argparse,hashlib,json,os,pathlib,platform,shutil,subprocess,tarfile

def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 a=argparse.ArgumentParser();a.add_argument('output');args=a.parse_args()
 if platform.system()!='Linux' or platform.machine()!='x86_64':raise ValueError('build on actual Linux x86_64; cross compile is not runtime acceptance')
 repo=pathlib.Path(__file__).resolve().parents[2];out=pathlib.Path(args.output).resolve()
 if out.exists():raise ValueError('fresh output required; retain previous evidence')
 if subprocess.check_output(['git','-C',str(repo),'status','--porcelain']).strip():raise ValueError('clean committed input required')
 ref=subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip();out.mkdir()
 target=pathlib.Path(os.environ.get('CARGO_TARGET_DIR',str(repo/'crewd/target'))).resolve()
 record=repo/'scripts/dev/record-crewd-build.py';evidence=out/'normal-build'
 subprocess.run(['python3',str(record),'prepare',str(repo),str(evidence),ref],check=True)
 subprocess.run(['python3',str(record),'build',str(repo),str(evidence),str(target/'release/crewd'),'cargo','build','--offline','--locked','--release','-p','crewd','--message-format=json'],env=dict(os.environ,CARGO_TARGET_DIR=str(target)),check=True)
 package=out/'crewd-linux-x86_64';(package/'bin').mkdir(parents=True);shutil.copy2(evidence/'crewd-presign',package/'bin/crewd')
 if json.loads((out/'normal-build-witness.json').read_text()).get('freshWorkspaceArtifacts') is not True:raise ValueError('new Linux candidate requires fresh owning-workspace source proof')
 shutil.copytree(evidence,package/'normal-build');shutil.copy2(out/'normal-build-witness.json',package/'normal-build-witness.json')
 (package/'README.txt').write_text('PendingCrew development Linux backend candidate\nSource: '+ref+'\nUnpacking never installs a service or migrates data.\nRun bin/crewd --help to inspect commands. For explicit isolated use:\n  bin/crewd --data-dir /absolute/fresh-directory serve\nRemote access is disabled by default. Pairing requires an explicit authenticated local invitation and configuration.\nKeep normal-build/ and normal-build-witness.json with the binary.\nDo not replace a daily database or start a system service without separate authorization.\n')
 manifest={'format':1,'sourceProofTier':'fresh-owning-workspace-production-graph','sourceRef':ref,'platform':'linux-x86_64','binary':'bin/crewd','binarySHA256':sha(package/'bin/crewd'),'witness':'normal-build-witness.json','witnessSHA256':sha(package/'normal-build-witness.json'),'distribution':'development-candidate','installed':False,'published':False}
 (package/'candidate.json').write_text(json.dumps(manifest,sort_keys=True,indent=2)+'\n')
 subprocess.run(['python3',str(repo/'scripts/dev/check-crewd-linux-candidate.py'),str(package)],check=True)
 subprocess.run(['python3',str(repo/'scripts/dev/smoke-crewd-linux-candidate.py'),str(package),str(out/'runtime-smoke')],check=True)
 archive=out/('crewd-linux-x86_64-'+ref[:8]+'.tar.gz')
 with tarfile.open(archive,'w:gz') as t:t.add(package,arcname=package.name)
 (out/'archive-sha256.txt').write_text(sha(archive)+'  '+archive.name+'\n')
 print('BUILT development Linux candidate with actual isolated packaged-runtime evidence:',archive)
try:main()
except (ValueError,OSError,KeyError,subprocess.SubprocessError) as e:raise SystemExit('FAIL: '+str(e))
