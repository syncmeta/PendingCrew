#!/usr/bin/env python3
"""Extract only the already-accepted original13 canonical tar from the smaller
accepted Linux14 artifact. Preserve original4e compiled identity; no Core runs.
"""
import argparse,hashlib,json,os,pathlib,subprocess,sys,zipfile
sys.dont_write_bytecode=True
ZIP_SHA='bbeec7ab04c8be1b5484eb4d57b8d3ce7ca3e0ea2b87e93a26e211803afd18aa'
TAR_SHA='cc73c54b606181f7f2c2ea500cbe0665f2aeb07138ee3c74db5553dec1ad129a'
MEMBER='linux14-baseline13/original4e-normal13.tar.gz'
def sha(p):
 h=hashlib.sha256()
 with p.open('rb') as f:
  for b in iter(lambda:f.read(1048576),b''):h.update(b)
 return h.hexdigest()
def main():
 a=argparse.ArgumentParser();a.add_argument('zip');a.add_argument('output');v=a.parse_args();source=pathlib.Path(v.zip);out=pathlib.Path(v.output)
 if not source.is_absolute() or source.resolve()!=source or source.is_symlink() or sha(source)!=ZIP_SHA:raise ValueError('exact accepted1533 artifact ZIP required')
 if not out.is_absolute() or out.parent.resolve()!=out.parent or out.exists() or out.is_symlink():raise ValueError('fresh absolute baseline output required')
 out.mkdir(mode=0o700);canonical=out/'canonical';canonical.mkdir(mode=0o700);tar=canonical/'crewd-linux-x86_64-4e80134b.tar.gz'
 with zipfile.ZipFile(source) as z:
  members=[m for m in z.infolist() if m.filename==MEMBER]
  if len(members)!=1 or members[0].file_size!=28364798:raise ValueError('unique exact canonical baseline member required')
  with z.open(members[0]) as inp,tar.open('xb') as target:
   for b in iter(lambda:inp.read(1048576),b''):target.write(b)
 if sha(tar)!=TAR_SHA:raise ValueError('accepted canonical13 tar content mismatch')
 subprocess.run([sys.executable,'-B',str(pathlib.Path(__file__).with_name('prepare-crewd-linux-install-reuse.py')),str(canonical),str(out/'verified'),'--baseline13'],check=True)
 value={'originalCompiledRef':'4e80134b9b5ff5a9b759aed1446f61da489efc4c','retrievedFromRun':38602193470,'retrievedFromArtifact':11913938011,'retrievedArtifactRef':'1533e69a901dd6c6c2be27cac11293cfd78117a0','retrievedArtifactZIP_SHA256':ZIP_SHA,'canonicalOriginal13SHA256':TAR_SHA,'baselineOnly':True,'currentProductionCovered':False,'normalRebuilt':False}
 (out/'baseline-composition.json').write_text(json.dumps(value,indent=2)+'\n');print('PASS original13 strong canonical baseline only; no current393 claim')
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,KeyError,zipfile.BadZipFile,subprocess.SubprocessError) as e:raise SystemExit('REFUSED original13 baseline: '+str(e))
