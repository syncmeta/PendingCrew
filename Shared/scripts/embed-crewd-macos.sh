#!/bin/sh
# Ordinary Mac target only; never build Rust, run code or access signing identities.
set -eu
[ "${PLATFORM_NAME:-}" = "macosx" ] || exit 0
case "${CONFIGURATION:-}" in Debug|Release) ;; *) exit 0 ;; esac
/usr/bin/python3 - <<'PY'
import hashlib,os,pathlib,re,stat
source=os.environ.get('PENDINGCREW_BACKEND_INPUT','');expected=os.environ.get('PENDINGCREW_BACKEND_SHA256','')
if not source.startswith('/') or not re.fullmatch('[0-9a-f]{64}',expected):raise SystemExit('error: Supply exact PENDINGCREW_BACKEND_INPUT and PENDINGCREW_BACKEND_SHA256')
with os.fdopen(os.open(source,os.O_RDONLY|os.O_NOFOLLOW),'rb') as f:
 if not stat.S_ISREG(os.fstat(f.fileno()).st_mode) or not os.access(source,os.X_OK):raise SystemExit('error: Core input must be an executable regular file')
 payload=f.read()
if hashlib.sha256(payload).hexdigest()!=expected:raise SystemExit('error: Core input hash differs')
raw=os.environ['TARGET_BUILD_DIR'];canonical='/private'+raw if raw=='/tmp' or raw.startswith('/tmp/') else raw
root=pathlib.Path(canonical)
if not root.is_absolute() or root.resolve(strict=True)!=root:raise SystemExit('error: Target product directory alias')
parts=pathlib.PurePosixPath(os.environ['CONTENTS_FOLDER_PATH']).parts
if len(parts)!=2 or not parts[0].endswith('.app') or parts[1]!='Contents' or parts[0] in ['.app','..']:raise SystemExit('error: Unexpected product contents path')
directories=[]
try:
 fd=os.open(root,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW);directories.append(fd)
 for part in [parts[0],parts[1],'Helpers']:
  if part=='Helpers':
   try:os.mkdir(part,0o755,dir_fd=fd)
   except FileExistsError:pass
  fd=os.open(part,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd);directories.append(fd)
  if os.fstat(fd).st_uid!=os.getuid():raise SystemExit('error: Target directories must belong to the builder')
 try:os.stat('crewd-data-worker',dir_fd=fd,follow_symlinks=False)
 except FileNotFoundError:pass
 else:raise SystemExit('error: Retired data worker in target')
 output=os.open('crewd',os.O_WRONLY|os.O_CREAT|os.O_NOFOLLOW,0o755,dir_fd=fd)
 try:
  inode=os.fstat(output)
  if not stat.S_ISREG(inode.st_mode) or inode.st_uid!=os.getuid() or inode.st_nlink!=1:raise SystemExit('error: Core output must be an owned regular single-link file')
  os.ftruncate(output,0)
  view=memoryview(payload)
  while view:view=view[os.write(output,view[:1048576]):]
  os.fchmod(output,0o755)
  current=os.stat('crewd',dir_fd=fd,follow_symlinks=False)
  if (current.st_dev,current.st_ino)!=(inode.st_dev,inode.st_ino):raise SystemExit('error: Core output path changed')
 finally:os.close(output)
finally:
 for fd in reversed(directories):os.close(fd)
print('note: Embedded coordinated normal crewd')
PY
