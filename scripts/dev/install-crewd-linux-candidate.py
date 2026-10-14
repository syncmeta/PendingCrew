#!/usr/bin/env python3
"""Explicit Linux candidate prefix only. Never access DBs or manage services."""
import argparse,fcntl,hashlib,json,os,pathlib,platform,re,shutil,subprocess,sys,tempfile,uuid,stat

def digest(root):
 rows=[]
 for p in sorted(root.rglob('*')):
  if p.is_symlink():raise ValueError('candidate package links forbidden')
  if p.is_file():rows.append((p.relative_to(root).as_posix(),hashlib.sha256(p.read_bytes()).hexdigest()))
 return hashlib.sha256(json.dumps(rows,separators=(',',':')).encode()).hexdigest()
def syncdir(path):
 fd=os.open(path,os.O_RDONLY|os.O_DIRECTORY)
 try:os.fsync(fd)
 finally:os.close(fd)
def save(path,value):
 fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC,0o600)
 with os.fdopen(fd,'w') as f:json.dump(value,f,sort_keys=True);f.write('\n');f.flush();os.fsync(f.fileno())
 syncdir(path.parent)
def pointer(prefix):
 p=prefix/'current'
 if not p.exists() and not p.is_symlink():return None
 if not p.is_symlink():raise ValueError('current must be a controlled activation link')
 value=os.readlink(p)
 if not re.fullmatch(r'activations/[0-9a-f-]{36}',value):raise ValueError('current activation link malformed')
 if not (prefix/value/'transaction.json').is_file():raise ValueError('current activation provenance missing')
 return value
def verify(package):
 subprocess.run([sys.executable,str(pathlib.Path(__file__).with_name('check-crewd-linux-candidate.py')),str(package)],check=True)
 value=json.loads((package/'candidate.json').read_text())
 if not os.access(package/'bin/crewd',os.X_OK):raise ValueError('packaged ELF lost executable mode')
 return value,digest(package)
def private_directory(path,create=False):
 if path.is_symlink() or path.resolve()!=path:raise ValueError('owner-private control directory must be canonical, without aliases')
 if create:path.mkdir(mode=0o700,exist_ok=True)
 info=path.stat()
 if not stat.S_ISDIR(info.st_mode) or info.st_uid!=os.geteuid() or stat.S_IMODE(info.st_mode)!=0o700:raise ValueError('control directory must be owner-private 0700')
def private_lock(path):
 fd=os.open(path,os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW|os.O_CLOEXEC,0o600)
 info=os.fstat(fd)
 if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.geteuid() or stat.S_IMODE(info.st_mode)!=0o600:
  os.close(fd);raise ValueError('control lock must be an owned 0600 regular file')
 return os.fdopen(fd,'a+')
def inactive(prefix,proc_root=pathlib.Path('/proc')):
 release=prefix/'releases'
 # Before any release exists no supported helper can have been launched.
 if not any(release.iterdir()):return
 for path in proc_root.iterdir():
  if not path.name.isdecimal():continue
  try:
   # Other UIDs cannot execute this 0700 private prefix. Host root/elevated
   # execution and raw bypasses are outside the managed-lease guarantee.
   # /proc directory ownership may become root for a non-dumpable process;
   # use the kernel's effective UID row, not inode ownership as process identity.
   uid_row=next(line for line in (path/'status').read_text().splitlines() if line.startswith('Uid:'))
   if int(uid_row.split()[2])!=os.geteuid():continue
   before=(path/'stat').read_text();fields=before[before.rfind(')')+2:].split()
   if int(fields[6]) & 0x00200000 or fields[0]=='Z':continue
   birth=fields[19]
   executable=os.readlink(path/'exe').removesuffix(' (deleted)')
  except (OSError,ValueError,IndexError,StopIteration) as e:
   if not path.exists():continue
   # Relevant unreadable same-UID prefix processes must fail closed. An
   # unrelated process with no prefix hint is not a permanent host-wide veto;
   # the managed runtime's shared flock is the lasting switching authority.
   try:hint=(path/'cmdline').read_bytes().split(b'\0')[0].decode(errors='replace')
   except OSError:hint=''
   if hint.startswith(str(prefix)+'/'):raise ValueError('related private-prefix runtime identity unreadable: '+path.name) from e
   continue
  if not pathlib.Path(executable).is_relative_to(release):continue
  try:
   after=(path/'stat').read_text();after_fields=after[after.rfind(')')+2:].split()
   if after_fields[19]!=birth or os.readlink(path/'exe').removesuffix(' (deleted)')!=executable:raise ValueError('related process birth/executable changed')
  except (OSError,ValueError,IndexError) as e:raise ValueError('related runtime identity unknown: '+path.name) from e
  raise ValueError('installed release runtime still running: '+path.name)
 # Only a bounded same-UID observation. Managed shared leases, not this scan,
 # prevent new supported processes throughout writer-exclusive activation.
def status(prefix,request=None):
 current=pointer(prefix) if prefix.exists() else None
 result={'currentActivation':current,'request':request,'outcome':'absent'}
 if request:
  transaction=prefix/'activations'/request/'transaction.json'
  if transaction.exists():
   value=json.loads(transaction.read_text());result.update(sourceRef=value['sourceRef'],outcome='prepared-unknown')
   cursor=current;seen=set()
   while cursor:
    if not re.fullmatch(r'activations/[0-9a-f-]{36}',cursor):raise ValueError('activation history pointer malformed')
    if cursor in seen:raise ValueError('activation history cycle')
    seen.add(cursor)
    if cursor=='activations/'+request:result['outcome']='committed-current' if cursor==current else 'committed-history';break
    cursor=json.loads((prefix/cursor/'transaction.json').read_text())['previousActivation']
 return result
def main():
 a=argparse.ArgumentParser();a.add_argument('action',choices=['status','install','rollback']);a.add_argument('--prefix',required=True);a.add_argument('--package');a.add_argument('--source-ref');a.add_argument('--request-id');a.add_argument('--expected-current',help='Exact prior activation from status; use none for initial install');args=a.parse_args()
 raw_prefix=pathlib.Path(args.prefix).expanduser()
 if not raw_prefix.is_absolute():raise ValueError('prefix must be an explicit absolute path')
 prefix=raw_prefix
 if prefix==pathlib.Path('/') or prefix.is_symlink() or prefix.resolve()!=prefix:raise ValueError('explicit canonical owner-private prefix required')
 if os.getuid()!=os.geteuid():raise ValueError('elevated mixed-UID invocation is unsupported')
 if prefix.exists():private_directory(prefix)
 if args.request_id:request=str(uuid.UUID(args.request_id))
 else:request=None
 if args.action=='status':print(json.dumps(status(prefix,request),sort_keys=True));return
 if platform.system()!='Linux':raise ValueError('actual Linux activation required')
 if request is None or args.expected_current is None:raise ValueError('explicit durable request UUID and expected-current CAS required')
 expected=None if args.expected_current=='none' else args.expected_current
 private_directory(prefix,create=True)
 for name in ['.install.lock','.runtime.lock']:
  if (prefix/name).is_symlink():raise ValueError('prefix locks cannot be links')
 for name in ['releases','activations']:
  p=prefix/name
  if p.is_symlink():raise ValueError('prefix control directories cannot be links')
  private_directory(p,create=True)
 with private_lock(prefix/'.install.lock') as writer,private_lock(prefix/'.runtime.lock') as runtime:
  fcntl.flock(writer,fcntl.LOCK_EX|fcntl.LOCK_NB);fcntl.flock(runtime,fcntl.LOCK_EX|fcntl.LOCK_NB)
  if args.action=='install':
   if not args.package:raise ValueError('explicit verified package required')
   package=pathlib.Path(args.package).resolve();value,inventory=verify(package);ref=value['sourceRef']
  else:
   ref=args.source_ref
   if not ref or not re.fullmatch('[0-9a-f]{40}',ref):raise ValueError('rollback requires an exact retained source ref')
   package=prefix/'releases'/ref;value,inventory=verify(package)
  if not re.fullmatch('[0-9a-f]{40}',ref):raise ValueError('source ref malformed')
  activation=prefix/'activations'/request;transaction=activation/'transaction.json'
  payload={'action':args.action,'sourceRef':ref,'packageInventoryDigest':inventory,'binarySHA256':value['binarySHA256'],'previousActivation':expected}
  if transaction.exists():
   if json.loads(transaction.read_text())!=payload:raise ValueError('request UUID payload conflict; do not replay')
   print(json.dumps(status(prefix,request),sort_keys=True));return # Readback only after an unknown result.
  if pointer(prefix)!=expected:raise ValueError('activation CAS conflict; refresh status before deciding')
  inactive(prefix)
  destination=prefix/'releases'/ref
  if destination.is_symlink():raise ValueError('retained release cannot be an external link')
  if destination.exists():
   _,retained=verify(destination)
   if retained!=inventory:raise ValueError('immutable retained release conflict')
  else:
   stage=pathlib.Path(tempfile.mkdtemp(prefix='.stage-',dir=prefix/'releases'))
   try:
    shutil.copytree(package,stage/'package');copied=stage/'package'
    if digest(copied)!=inventory:raise ValueError('staged full package changed')
    for file in copied.rglob('*'):
     if file.is_file():
      fd=os.open(file,os.O_RDONLY)
      try:os.fsync(fd)
      finally:os.close(fd)
    for folder in sorted([p for p in copied.rglob('*') if p.is_dir()],key=lambda p:-len(p.parts)):syncdir(folder)
    syncdir(copied);os.rename(copied,destination);syncdir(destination.parent)
   finally:shutil.rmtree(stage)
  activation.mkdir(mode=0o700);save(transaction,payload)
  (activation/'release').symlink_to('../../releases/'+ref);(activation/'bin').symlink_to('release/bin');syncdir(activation);syncdir(activation.parent)
  # A controlled launcher holds a shared runtime lock before resolving current.
  bindir=prefix/'bin';private_directory(bindir,create=True)
  launcher=bindir/'crewd'
  launcher_source='''#!/usr/bin/env python3
import fcntl,os,pathlib,stat,sys
prefix=pathlib.Path(__file__).resolve().parents[1]
info=prefix.stat()
if os.getuid()!=os.geteuid() or info.st_uid!=os.geteuid() or stat.S_IMODE(info.st_mode)!=0o700:raise SystemExit("owner-private prefix required")
fd=os.open(prefix/".runtime.lock",os.O_RDWR|os.O_NOFOLLOW)
info=os.fstat(fd)
if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.geteuid() or stat.S_IMODE(info.st_mode)!=0o600:raise SystemExit("private runtime lease required")
fcntl.flock(fd,fcntl.LOCK_SH|fcntl.LOCK_NB)
os.set_inheritable(fd,True)
program=prefix/"current/bin/crewd"
os.execv(str(program),[str(program),*sys.argv[1:]])
'''
  if launcher.exists():
   if launcher.is_symlink() or launcher.read_text()!=launcher_source:raise ValueError('controlled launcher differs; refuse overwrite')
  else:
   with launcher.open('x') as f:f.write(launcher_source);f.flush();os.fsync(f.fileno())
   launcher.chmod(0o700);syncdir(bindir)
  if pointer(prefix)!=expected:raise ValueError('activation changed before commit; refuse')
  pending=prefix/('.current-'+request)
  pending.symlink_to('activations/'+request);os.replace(pending,prefix/'current');syncdir(prefix)
  result=status(prefix,request)
  if result['outcome']!='committed-current':raise ValueError('activation readback is unknown; inspect status, never blind replay')
  print(json.dumps(result,sort_keys=True))
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,KeyError,subprocess.SubprocessError) as e:raise SystemExit('REFUSED/UNKNOWN: '+str(e)+'; inspect explicit prefix status before deciding')
