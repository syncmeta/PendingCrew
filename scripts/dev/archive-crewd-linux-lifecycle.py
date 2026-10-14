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
    resolved=path.resolve()
    if not resolved.is_relative_to(root):raise ValueError('evidence link escapes fixture')
    # Controlled synthetic /proc entries retain absolute targets in the live
    # fixture. Preserve that fact, but serialize an equivalent portable in-root
    # reference; never dereference it or change the original evidence tree.
    serialized=os.path.relpath(resolved,path.parent) if pathlib.Path(target).is_absolute() else target
    rows.append({'path':relative,'kind':'symlink','target':serialized,'originalTarget':target})
   elif stat.S_ISREG(s.st_mode):rows.append({'path':relative,'kind':'file','size':s.st_size,'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
   elif not stat.S_ISDIR(s.st_mode):raise ValueError('special file cannot enter lifecycle archive')
 links={row['path']:row for row in rows if row['kind']=='symlink'}
 def preserve_reference(info):
  row=links.get(info.name.removeprefix('flow/'))
  if row is not None:info.linkname=row['target']
  return info
 with tarfile.open(output,'w:gz',dereference=False) as t:t.add(root,arcname='flow',recursive=True,filter=preserve_reference)
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
 except (ValueError,OSError,RuntimeError,tarfile.TarError) as e:raise SystemExit('REFUSED lifecycle evidence archive: '+str(e))
