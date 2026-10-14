#!/usr/bin/env python3
"""Verify the original accepted canonical Linux11 tar; never repair loose upload."""
import argparse,hashlib,json,pathlib,subprocess,sys,tarfile
RUN=38500948870
ARTIFACT=11866465185
REF='7b1849d97a813a1655ebafb66068a04a614e8787'
TAR_SHA='8a9cc7395f66c1fd60283c1aeceb26ef762b7e8205ac04ad496feb28c7742c94'
BIN_SHA='c5a65cc9472fa92bdea9bc60a80f049a869045cc57706feffb19416dd5e54cda'
def main():
 a=argparse.ArgumentParser();a.add_argument('download');a.add_argument('output');args=a.parse_args()
 source=pathlib.Path(args.download).resolve();out=pathlib.Path(args.output).resolve()
 if out.exists():raise ValueError('fresh baseline evidence output required')
 candidates=list(source.rglob('crewd-linux-x86_64-7b1849d9.tar.gz'))
 if len(candidates)!=1:raise ValueError('one original canonical archive required; do not reconstruct omitted hidden files')
 archive=candidates[0]
 if hashlib.sha256(archive.read_bytes()).hexdigest()!=TAR_SHA:raise ValueError('original tar digest mismatch')
 out.mkdir(mode=0o700)
 with tarfile.open(archive) as t:
  seen=set()
  for m in t.getmembers():
   p=pathlib.PurePosixPath(m.name)
   if p.is_absolute() or '..' in p.parts or not p.parts or p.parts[0]!='crewd-linux-x86_64' or not (m.isfile() or m.isdir()) or m.name in seen or m.mode&~0o777:raise ValueError('unsafe or duplicate original tar member')
   seen.add(m.name)
  t.extractall(out)
 package=out/'crewd-linux-x86_64';value=json.loads((package/'candidate.json').read_text())
 if value['sourceRef']!=REF or value['binarySHA256']!=BIN_SHA or hashlib.sha256((package/'bin/crewd').read_bytes()).hexdigest()!=BIN_SHA:raise ValueError('original source or ELF mismatch')
 subprocess.run([sys.executable,str(pathlib.Path(__file__).with_name('check-crewd-linux-candidate.py')),str(package)],check=True)
 (out/'original-baseline.json').write_text(json.dumps({'runId':RUN,'artifactId':ARTIFACT,'sourceRef':REF,'canonicalTarSHA256':TAR_SHA,'binarySHA256':BIN_SHA,'sourceArchive':str(archive),'canonicalPackage':str(package),'looseUploadRepaired':False,'sourceProofTier':'legacy-byte-inventory-only'},indent=2)+'\n')
 print('PASS immutable original Linux11 canonical package, original build/config proof retained; not a new build')
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,KeyError,subprocess.SubprocessError,tarfile.TarError) as e:raise SystemExit('REFUSED original baseline: '+str(e))
