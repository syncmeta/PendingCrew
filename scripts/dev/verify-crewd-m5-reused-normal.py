#!/usr/bin/env python3
"""Bind remaining runtime qualification to the original compiled M5 artifacts."""
import argparse,hashlib,importlib.util,json,os,pathlib,shutil,subprocess,tarfile,tempfile
ORIGINAL='2bf1ce50129eb54c412496600c1f17945ed3f61d'
BASELINE='a76cae4804cb26c62f4e2a64c993dbe18822758f'
PINS={'Linux':{'target':'x86_64-unknown-linux-gnu','artifactID':11871714902,'13':'f5ec7ef6a4ffc07c0fdbdd890f1c0f1a43484ad0207b1797aac824a9846be7b7','12':'1e42aa91b45f89abb66f79c33135d01cee6dd1831383eac9bced040857ace6b2'},'macOS':{'target':'aarch64-apple-darwin','artifactID':11872685419,'13':'066a1da5954edcbef7942d8ff831840919291f5b7477f82f80c3f80479b701fd'}}
def main():
 p=argparse.ArgumentParser();p.add_argument('artifact_root',type=pathlib.Path);p.add_argument('repository',type=pathlib.Path);p.add_argument('output',type=pathlib.Path);p.add_argument('--platform',choices=PINS,required=True);a=p.parse_args()
 root=a.artifact_root.resolve();repo=a.repository.resolve();out=a.output.resolve();pin=PINS[a.platform]
 if out.exists():raise ValueError('new composition evidence directory required')
 out.mkdir(mode=0o700)
 current=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip()
 if subprocess.run(['git','diff','--quiet',ORIGINAL,current,'--','crewd'],cwd=repo).returncode!=0:raise ValueError('current complete Rust tree differs from original compiled inputs; no artifact reuse')
 spec=importlib.util.spec_from_file_location('pcw_proof',pathlib.Path(__file__).with_name('write-codex-first-provenance.py'));proof=importlib.util.module_from_spec(spec);spec.loader.exec_module(proof)
 records=[]
 for schema in ['13','12'] if a.platform=='Linux' else ['13']:
  witnesses=list(root.rglob('m5-normal'+schema+'-witness.json'))
  if len(witnesses)!=1:raise ValueError('unique complete original witness required: '+schema)
  witness=witnesses[0];value=json.loads(witness.read_text());ref=ORIGINAL if schema=='13' else BASELINE
  if value['sourceRef']!=ref or value['binarySHA256']!=pin[schema]:raise ValueError('original source/binary pin mismatch')
  evidence=witness.parent/value['evidenceDirectory'];binary=evidence/value['binaryFile']
  with tempfile.TemporaryDirectory(prefix='pcw-m5-input-check-') as temporary:
   temp=pathlib.Path(temporary);archive=temp/'source.tar'
   subprocess.run(['git','archive','--format=tar','-o',str(archive),ref,'crewd'],cwd=repo,check=True)
   snapshot=temp/'source';snapshot.mkdir()
   with tarfile.open(archive) as tar:
    for member in tar.getmembers():
     path=pathlib.PurePosixPath(member.name)
     if path.is_absolute() or '..' in path.parts or not (member.isdir() or member.isfile()):raise ValueError('unsafe Git source snapshot')
    tar.extractall(snapshot)
   rows=proof.inventory(snapshot)
   if rows!=value['files']:raise ValueError('complete original Git Rust inventory differs')
   proof.verify_witness(witness,binary,proof.digest(rows),{'profile':'debug','target':pin['target'],'toolchain':'rustc 1.94.0 (4a4ef493e 2026-03-02)','defaultFeatures':True,'features':[],'allFeatures':False},require_fresh=True)
  # Download-artifact loses executable mode. Preserve downloaded originals and
  # restore mode only on a new byte-identical executable for this fake run.
  used=out/('normal'+schema+'-used');used.mkdir(mode=0o700);saved=used/'crewd';shutil.copy2(binary,saved);original_mode=oct(binary.stat().st_mode&0o777);saved.chmod(0o700)
  if hashlib.sha256(saved.read_bytes()).hexdigest()!=value['binarySHA256']:raise ValueError('new runtime copy content changed')
  records.append({'schema':schema,'compiledSourceRef':ref,'originalWitness':str(witness),'originalEvidence':str(evidence),'binarySHA256':value['binarySHA256'],'originalDownloadedMode':original_mode,'runtimeCopy':str(saved),'runtimeMode':'0o700','rustInputDigest':value['rustInputDigest'],'originalBuildConfiguration':value['buildConfiguration']})
 result={'originalRun':38516501175,'originalRunSourceRef':ORIGINAL,'originalArtifactID':pin['artifactID'],'compositionSourceRef':current,'currentCompleteRustTreeMatchesOriginal':True,'newNormalCompiled':False,'freshOwningSourceRawGraphVerified':True,'expectedRuntimeTarget':pin['target'],'normalArtifacts':records}
 (out/'composition.json').write_text(json.dumps(result,indent=2)+'\n')
 print('PASS original compiled source/complete inputs/config/raw owning graph/binary; new ref is composition only')
if __name__=='__main__':main()
