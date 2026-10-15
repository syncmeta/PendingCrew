#!/usr/bin/env python3
"""Reuse accepted e630 Core14 Release as baseline only; never relabel as15.
No Core execution or installation. ZIP and canonical tar are exact immutable
accepted historical inputs. Full original Git/source/fresh proof is rechecked.
"""
import argparse,hashlib,importlib.util,json,pathlib,subprocess,sys,tarfile,zipfile
sys.dont_write_bytecode=True
ZIP_SHA='64f270a19834a041e9b3fbde602f933325ca96d4732083e795e96d0ec5929713'
TAR_SHA='997c851720ae03657c7dad040083b1c16e862b26f545aee6dee610a5a5e97160'
BIN_SHA='b791de5f1497cea877858b52d9b3ef1556349d0285b8a6e7670841839548f18f'
REF='e630d3e25dab2c67a39057b3c61ab54f258894b6'
DIGEST='530de912ebc0fb615f36ed444e7ed1867bcf72510b639f6467e5633195e09a06'
MEMBER='linux393-release/crewd-linux-x86_64-e630d3e2.tar.gz'
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def load(name):
 spec=importlib.util.spec_from_file_location(name,pathlib.Path(__file__).with_name(name+'.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
def main():
 parser=argparse.ArgumentParser();parser.add_argument('zip');parser.add_argument('output');a=parser.parse_args();source=pathlib.Path(a.zip);out=pathlib.Path(a.output)
 if not source.is_absolute() or source.resolve()!=source or source.is_symlink() or sha(source)!=ZIP_SHA:raise ValueError('exact accepted e630 artifact ZIP required')
 if not out.is_absolute() or out.parent.resolve()!=out.parent or out.exists() or out.is_symlink():raise ValueError('fresh absolute baseline output required')
 out.mkdir(mode=0o700);canonical=out/'canonical';canonical.mkdir(mode=0o700);archive=canonical/'crewd-linux-x86_64-e630d3e2.tar.gz'
 with zipfile.ZipFile(source) as z:
  members=[x for x in z.infolist() if x.filename==MEMBER]
  if len(members)!=1 or members[0].file_size!=30822282:raise ValueError('unique exact canonical14 member required')
  with z.open(members[0]) as inp,archive.open('xb') as target:
   for block in iter(lambda:inp.read(1048576),b''):target.write(block)
 if sha(archive)!=TAR_SHA:raise ValueError('accepted canonical14 content mismatch')
 verified=out/'verified';verified.mkdir(mode=0o700)
 baseline=load('prepare-crewd-linux-install-baseline')
 with tarfile.open(archive) as t:baseline.validate_members(t.getmembers());t.extractall(verified)
 package=verified/'crewd-linux-x86_64';candidate=json.loads((package/'candidate.json').read_text());witness=json.loads((package/'normal-build-witness.json').read_text())
 if candidate['sourceRef']!=REF or candidate['binarySHA256']!=BIN_SHA or sha(package/'bin/crewd')!=BIN_SHA:raise ValueError('accepted14 source/binary mismatch')
 repo=pathlib.Path(__file__).resolve().parents[2];rows=[]
 paths=subprocess.check_output(['git','-C',str(repo),'ls-tree','-r','--name-only',REF,'crewd'],text=True).splitlines()
 for path in sorted(paths):rows.append({'path':path,'sha256':hashlib.sha256(subprocess.check_output(['git','-C',str(repo),'show',REF+':'+path])).hexdigest()})
 if len(rows)!=393 or rows!=witness['files']:raise ValueError('complete original393 Git inputs differ')
 proof=load('write-codex-first-provenance')
 if proof.digest(rows)!=DIGEST:raise ValueError('original393 digest mismatch')
 proof.verify_witness(package/'normal-build-witness.json',package/'bin/crewd',DIGEST,{'target':'x86_64-unknown-linux-gnu','profile':'release','defaultFeatures':True,'allFeatures':False,'features':[]},require_fresh=True)
 subprocess.run([sys.executable,'-B',str(pathlib.Path(__file__).with_name('check-crewd-linux-candidate.py')),str(package)],check=True)
 value={'runId':38670397185,'artifactId':11943682299,'compiledSourceRef':REF,'binarySHA256':BIN_SHA,'canonicalTarSHA256':TAR_SHA,'artifactZIP_SHA256':ZIP_SHA,'rustInputs':393,'rustInputDigest':DIGEST,'role':'historical14-baseline-only','normalRebuilt':False,'current15Covered':False}
 (out/'baseline-composition.json').write_text(json.dumps(value,indent=2)+'\n');print('PASS accepted original e630 defaultRelease393 baseline only; no15 claim')
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,KeyError,subprocess.SubprocessError,tarfile.TarError,zipfile.BadZipFile) as e:raise SystemExit('REFUSED baseline14: '+str(e))
