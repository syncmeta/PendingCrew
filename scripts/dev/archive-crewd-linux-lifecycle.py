#!/usr/bin/env python3
"""Archive one private fixture tree without dereferencing activation/release links."""
import argparse,hashlib,json,os,pathlib,stat,tarfile

def archive(root,output,diagnostic_special_inodes=False):
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
   elif not stat.S_ISDIR(s.st_mode):
    if not diagnostic_special_inodes:raise ValueError('special file cannot enter lifecycle archive')
    if not (stat.S_ISSOCK(s.st_mode) or stat.S_ISFIFO(s.st_mode)):raise ValueError('only socket/FIFO diagnostic metadata permitted')
    kind='socket' if stat.S_ISSOCK(s.st_mode) else 'fifo'
    rows.append({'path':relative,'kind':'special-metadata','inodeType':kind,'mode':stat.S_IMODE(s.st_mode),'uid':s.st_uid,'gid':s.st_gid,'device':s.st_dev,'inode':s.st_ino,'size':s.st_size,'mtimeNS':s.st_mtime_ns,'ctimeNS':s.st_ctime_ns,'contentsCaptured':False,'restored':False})
 links={row['path']:row for row in rows if row['kind']=='symlink'}
 special={row['path'] for row in rows if row['kind']=='special-metadata'}
 def preserve_reference(info):
  if info.name.removeprefix('flow/') in special:return None
  row=links.get(info.name.removeprefix('flow/'))
  if row is not None:info.linkname=row['target']
  return info
 with tarfile.open(output,'w:gz',dereference=False) as t:t.add(root,arcname='flow',recursive=True,filter=preserve_reference)
 with tarfile.open(output,'r:gz') as t:
  members={m.name.removeprefix('flow/'):m for m in t.getmembers() if m.name!='flow'}
  for row in rows:
   if row['kind']=='special-metadata':
    if row['path'] in members:raise ValueError('diagnostic inode became a restorable archive member')
    continue
   m=members[row['path']]
   if row['kind']=='symlink':
    if not m.issym() or m.linkname!=row['target']:raise ValueError('archive link was dereferenced')
   else:
    if not m.isfile() or hashlib.sha256(t.extractfile(m).read()).hexdigest()!=row['sha256']:raise ValueError('archive actual file differs')
 result={'format':1,'sourceDirectory':str(root),'archive':output.name,'archiveSHA256':hashlib.sha256(output.read_bytes()).hexdigest(),'dereference':False,'members':rows}
 if diagnostic_special_inodes:result.update(diagnosticSpecialInodes=True,restorableCompleteFilesystem=False,specialContentsCaptured=False)
 for row in rows:
  if row['kind']=='special-metadata':
   current=(root/row['path']).lstat()
   values={'inodeType':'socket' if stat.S_ISSOCK(current.st_mode) else 'fifo' if stat.S_ISFIFO(current.st_mode) else 'unsupported','mode':stat.S_IMODE(current.st_mode),'uid':current.st_uid,'gid':current.st_gid,'device':current.st_dev,'inode':current.st_ino,'size':current.st_size,'mtimeNS':current.st_mtime_ns,'ctimeNS':current.st_ctime_ns}
   if any(row[key]!=value for key,value in values.items()):raise ValueError('diagnostic inode changed during archive')
 output.with_suffix(output.suffix+'.json').write_text(json.dumps(result,sort_keys=True,indent=2)+'\n')
 return result

def main():
 p=argparse.ArgumentParser();p.add_argument('root');p.add_argument('output');p.add_argument('--diagnostic-special-inodes',action='store_true',help='Retain special inode metadata only; never a complete restorable filesystem');a=p.parse_args();r=archive(a.root,a.output,a.diagnostic_special_inodes);print('ARCHIVED private lifecycle without dereferencing references:',r['archiveSHA256'])
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,RuntimeError,tarfile.TarError) as e:raise SystemExit('REFUSED lifecycle evidence archive: '+str(e))
