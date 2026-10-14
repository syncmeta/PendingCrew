#!/usr/bin/env python3
"""Reuse only the actual 4e normal13 release; preserve its original compiled source."""
import argparse,hashlib,importlib.util,json,pathlib,subprocess,sys,tarfile,zipfile
sys.dont_write_bytecode=True
RUN=38541835255
ARTIFACT=11882857926
REF='4e80134b9b5ff5a9b759aed1446f61da489efc4c'
TAR_SHA='cc73c54b606181f7f2c2ea500cbe0665f2aeb07138ee3c74db5553dec1ad129a'
ZIP_SHA='eaeb78c4130bb61556cc79041e4aa20d4d94bae8085f110854857b02945825cb'
BIN_SHA='d891b4d84b5e8b35c9f502504ae5da5569ee330c7f23ba489c746cc8f6a5b8e5'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def load(name):
 spec=importlib.util.spec_from_file_location(name,pathlib.Path(__file__).with_name(name+'.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m

def main():
 p=argparse.ArgumentParser();p.add_argument('download');p.add_argument('output');a=p.parse_args()
 source=pathlib.Path(a.download).resolve();out=pathlib.Path(a.output).resolve();repo=pathlib.Path(__file__).resolve().parents[2]
 if out.exists():raise ValueError('fresh reused release directory required')
 if source.is_file():
  if sha(source)!=ZIP_SHA:raise ValueError('original4e artifact ZIP digest mismatch')
  with zipfile.ZipFile(source) as z:
   matches=[m for m in z.infolist() if m.filename.endswith('/linux-install-new13/crewd-linux-x86_64-4e80134b.tar.gz')]
   if len(matches)!=1:raise ValueError('one unique original normal13 tar member required')
   content=z.read(matches[0])
   if hashlib.sha256(content).hexdigest()!=TAR_SHA:raise ValueError('original canonical tar content differs')
  out.mkdir(mode=0o700);archive=out/'original4e-normal13.tar.gz';archive.write_bytes(content);archives=[archive]
 else:archives=list(source.rglob('crewd-linux-x86_64-4e80134b.tar.gz'))
 if len(archives)!=1 or sha(archives[0])!=TAR_SHA:raise ValueError('one exact original actual4e canonical tar required')
 baseline=load('prepare-crewd-linux-install-baseline')
 if not out.exists():out.mkdir(mode=0o700)
 with tarfile.open(archives[0]) as t:baseline.validate_members(t.getmembers());t.extractall(out)
 package=out/'crewd-linux-x86_64';manifest=json.loads((package/'candidate.json').read_text());witness=json.loads((package/'normal-build-witness.json').read_text())
 if manifest['sourceRef']!=REF or manifest['binarySHA256']!=BIN_SHA or sha(package/'bin/crewd')!=BIN_SHA:raise ValueError('original normal source/binary mismatch')
 paths=subprocess.check_output(['git','-C',str(repo),'ls-tree','-r','--name-only','HEAD','crewd']).decode().splitlines()
 rows=[]
 for path in paths:
  committed=subprocess.check_output(['git','-C',str(repo),'show','HEAD:'+path]);h=hashlib.sha256(committed).hexdigest()
  if sha(repo/path)!=h:raise ValueError('current owning Rust working tree differs from committed input')
  rows.append({'path':path,'sha256':h})
 if sorted(rows,key=lambda r:r['path'])!=witness['files']:raise ValueError('complete current Rust inputs differ; original normal cannot cover new production')
 proof=load('write-codex-first-provenance');cfg=proof.complete_configuration(witness['buildConfiguration'])
 expected={'target':'x86_64-unknown-linux-gnu','profile':'release','defaultFeatures':True,'allFeatures':False,'features':[]}
 if cfg.get('artifactProfile',{}).get('test') is not False:raise ValueError('normal must be non-test production')
 proof.verify_witness(package/'normal-build-witness.json',package/'bin/crewd',proof.digest(rows),expected,require_fresh=True)
 subprocess.run([sys.executable,str(pathlib.Path(__file__).with_name('check-crewd-linux-candidate.py')),str(package)],check=True)
 result={'runId':RUN,'artifactId':ARTIFACT,'compiledSourceRef':REF,'currentFixtureSourceRef':subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD']).decode().strip(),'normalRebuilt':False,'originalZipSHA256':ZIP_SHA,'canonicalTarSHA256':TAR_SHA,'binarySHA256':BIN_SHA,'fullRustInputDigest':proof.digest(rows),'sourceProofTier':'fresh-owning-workspace-production-graph','originalRunConclusion':'failure; install lifecycle closure remains unverified'}
 (out/'reused-normal13.json').write_text(json.dumps(result,sort_keys=True,indent=2)+'\n');print('VERIFIED original4e normal13 release, no new compile or source relabel')
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,KeyError,subprocess.SubprocessError,tarfile.TarError) as e:raise SystemExit('REFUSED reused normal13: '+str(e))
