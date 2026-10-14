#!/usr/bin/env python3
"""Archive one private fixture tree without dereferencing activation/release links."""
import argparse,hashlib,json,os,pathlib,stat,tarfile

def archive(root,output):
 root=pathlib.Path(root).absolute();output=pathlib.Path(output).absolute()
 if root.is_symlink() or root.resolve()!=root or not root.is_dir():raise ValueError('canonical private evidence root required')
 info=root.stat()
 if info.st_uid!=os.geteuid() or stat.S_IMODE(info.st_mode)!=0o700:raise ValueError('owned0700 evidence root required')
 if output.exists() or output.is_relative_to(root):raise ValueError('fresh archive outside fixture required')
 rows=[]
 for parent,dirs,files in os.walk(root,followlinks=False):
  for name in sorted(dirs+files):
   path=pathlib.Path(parent)/name;relative=path.relative_to(root).as_posix();s=path.lstat()
   if path.is_symlink():
    target=os.readlink(path)
    if pathlib.Path(target).is_absolute() or not path.resolve().is_relative_to(root):raise ValueError('evidence link escapes fixture')
    rows.append({'path':relative,'kind':'symlink','target':target})
   elif stat.S_ISREG(s.st_mode):rows.append({'path':relative,'kind':'file','size':s.st_size,'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
   elif not stat.S_ISDIR(s.st_mode):raise ValueError('special file cannot enter lifecycle archive')
 with tarfile.open(output,'w:gz',dereference=False) as t:t.add(root,arcname='flow',recursive=True)
 with tarfile.open(output,'r:gz') as t:
  members={m.name.removeprefix('flow/'):m for m in t.getmembers() if m.name!='flow'}
  for row in rows:
   m=members[row['path']]
   if row['kind']=='symlink':
    if not m.issym() or m.linkname!=row['target']:raise ValueError('archive link was dereferenced')
   else:
    if not m.isfile() or hashlib.sha256(t.extractfile(m).read()).hexdigest()!=row['sha256']:raise ValueError('archive actual file differs')
 result={'format':1,'sourceDirectory':str(root),'archive':output.name,'archiveSHA256':hashlib.sha256(output.read_bytes()).hexdigest(),'dereference':False,'members':rows}
 output.with_suffix(output.suffix+'.json').write_text(json.dumps(result,sort_keys=True,indent=2)+'\n')
 return result

def main():
 p=argparse.ArgumentParser();p.add_argument('root');p.add_argument('output');a=p.parse_args();r=archive(a.root,a.output);print('ARCHIVED private lifecycle without dereferencing references:',r['archiveSHA256'])
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,tarfile.TarError) as e:raise SystemExit('REFUSED lifecycle evidence archive: '+str(e))
