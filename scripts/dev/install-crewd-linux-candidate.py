#!/usr/bin/env python3
"""Explicit Linux candidate prefix only. Never access DBs or manage services."""
import argparse,fcntl,hashlib,json,os,pathlib,platform,re,shutil,subprocess,sys,tempfile,uuid

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
 with path.open('x') as f:json.dump(value,f,sort_keys=True);f.write('\n');f.flush();os.fsync(f.fileno())
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
def inactive(prefix):
 release=(prefix/'releases').resolve()
 for path in pathlib.Path('/proc').iterdir():
  if not path.name.isdecimal():continue
  try:
   # Kernel threads have no executable and are not runtime identities.
   stat=(path/'stat').read_text();fields=stat[stat.rfind(')')+2:].split();flags=int(fields[6])
   if flags & 0x00200000 or fields[0]=='Z':continue
   executable=os.readlink(path/'exe')
  except FileNotFoundError:
   if path.exists():raise ValueError('process executable identity missing; refuse activation: '+path.name)
   continue # Process vanished during the observation.
  except (OSError,ValueError) as e:raise ValueError('process identity unreadable; refuse activation: '+path.name) from e
  executable=executable.removesuffix(' (deleted)')
  if pathlib.Path(executable).is_relative_to(release):raise ValueError('installed release runtime still running: '+path.name)
 # This is a bounded observation, never a claim that processes stay quiescent.
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
 if not prefix.is_absolute() or prefix==pathlib.Path('/') or prefix.is_symlink():raise ValueError('explicit non-root, non-link prefix required')
 if args.request_id:request=str(uuid.UUID(args.request_id))
 else:request=None
 if args.action=='status':print(json.dumps(status(prefix,request),sort_keys=True));return
 if platform.system()!='Linux':raise ValueError('actual Linux activation required')
 if request is None or args.expected_current is None:raise ValueError('explicit durable request UUID and expected-current CAS required')
 expected=None if args.expected_current=='none' else args.expected_current
 prefix.mkdir(mode=0o700,parents=True,exist_ok=True)
 if prefix.stat().st_mode & 0o022:raise ValueError('prefix writable by another identity; refuse')
 for name in ['.install.lock','.runtime.lock']:
  if (prefix/name).is_symlink():raise ValueError('prefix locks cannot be links')
 for name in ['releases','activations']:
  p=prefix/name
  if p.is_symlink():raise ValueError('prefix control directories cannot be links')
  p.mkdir(mode=0o700,exist_ok=True)
 with (prefix/'.install.lock').open('a') as writer,(prefix/'.runtime.lock').open('a') as runtime:
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
  bindir=prefix/'bin';bindir.mkdir(mode=0o700,exist_ok=True)
  launcher=bindir/'crewd'
  launcher_source='#!/usr/bin/env python3\nimport fcntl,os,pathlib,sys\nprefix=pathlib.Path(__file__).resolve().parents[1]\nlock=open(prefix/".runtime.lock","a")\nfcntl.flock(lock,fcntl.LOCK_SH)\nos.set_inheritable(lock.fileno(),True)\nprogram=prefix/"current/bin/crewd"\nos.execv(str(program),[str(program),*sys.argv[1:]])\n'
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
try:main()
except (ValueError,OSError,KeyError,subprocess.SubprocessError) as e:raise SystemExit('REFUSED/UNKNOWN: '+str(e)+'; inspect explicit prefix status before deciding')
