#!/usr/bin/env python3
"""Fail-closed static dyld dependency policy for the private Validation channel.
Does not launch, sign, strip code, access credentials, or relax library validation.
Only prune-target mutates the newly built, unsigned Validation target (never an
artifact); it removes an unused updater payload after checking every other image.
"""
import argparse,hashlib,json,os,pathlib,plistlib,re,shutil,stat,struct,subprocess
MAGIC={bytes.fromhex(x) for x in ['cffaedfe','feedfacf','cafebabe','bebafeca','cafebabf','bfbafeca']}
LOAD={0xc:'load',0x80000018:'weak',0x8000001f:'reexport',0x80000023:'upward',0x20:'lazy'}
POLICY='validation-system-libraries-only-v1'

def fail(reason):raise ValueError('Validation load closure: '+reason)
def sha(data):return hashlib.sha256(data).hexdigest()
def slices(data):
 if data[:4] in [bytes.fromhex('cffaedfe'),bytes.fromhex('feedfacf')]:return [(None,data)]
 fmts={bytes.fromhex('cafebabe'):('>',False),bytes.fromhex('bebafeca'):('<',False),bytes.fromhex('cafebabf'):('>',True),bytes.fromhex('bfbafeca'):('<',True)}
 if data[:4] not in fmts or len(data)<8:fail('unknown or truncated Mach-O container')
 endian,wide=fmts[data[:4]];count=struct.unpack_from(endian+'I',data,4)[0];size=32 if wide else 20;table=8+size*count
 if not 1<=count<=16 or table>len(data):fail('invalid fat table')
 ranges=[];rows=[];identities=set()
 for index in range(count):
  fields=struct.unpack_from(endian+('IIQQII' if wide else 'IIIII'),data,8+size*index);cpu,sub,offset,length,align=fields[:5]
  if (cpu,sub) in identities or align>30 or offset<table or offset%(1<<align) or not length or offset+length>len(data) or (wide and fields[5]):fail('invalid fat identity/bounds')
  if any(offset<end and offset+length>start for start,end in ranges):fail('overlapping fat slices')
  identities.add((cpu,sub));ranges.append((offset,offset+length));rows.append(((cpu,sub),data[offset:offset+length]))
 cursor=table
 for start,end in sorted(ranges):
  if any(data[cursor:start]):fail('nonzero unclassified fat padding')
  cursor=end
 if any(data[cursor:]):fail('nonzero trailing fat payload')
 return rows

def string(raw,minimum,endian):
 if len(raw)<minimum:fail('truncated string command')
 offset=struct.unpack_from(endian+'I',raw,8)[0]
 if not minimum<=offset<len(raw):fail('invalid command string offset')
 end=raw.find(b'\0',offset)
 if end<0 or any(raw[end+1:]):fail('unterminated/nonzero string padding')
 try:value=raw[offset:end].decode('utf-8')
 except UnicodeDecodeError:fail('invalid command UTF-8')
 if not value or any(ord(x)<32 or ord(x)==127 for x in value):fail('invalid command name')
 return value

def platform_library(name):
 # No @rpath, relative, arbitrary absolute, traversal or environment-selected image.
 return name.startswith(('/System/Library/Frameworks/','/usr/lib/')) and name==str(pathlib.PurePosixPath(name)) and '..' not in pathlib.PurePosixPath(name).parts

def image(data):
 result=[]
 for identity,part in slices(data):
  endian='<' if part[:4]==bytes.fromhex('cffaedfe') else '>' if part[:4]==bytes.fromhex('feedfacf') else None
  if endian is None or len(part)<32:fail('only explicit 64-bit images supported')
  h=struct.unpack_from(endian+'8I',part);cpu,sub,kind,ncmds,total=h[1:6];end=32+total
  if cpu not in [0x100000c,0x1000007] or kind not in [2,6,8] or (identity and identity!=(cpu,sub)) or not 0<ncmds<=4096 or end>len(part):fail('invalid architecture/load-command header')
  cursor=32;imports=[];rpaths=[];dylinkers=[]
  for _ in range(ncmds):
   if cursor+8>end:fail('truncated load command')
   command,size=struct.unpack_from(endian+'II',part,cursor)
   if size<8 or size%8 or cursor+size>end:fail('invalid load-command extent')
   raw=part[cursor:cursor+size]
   if command not in set(LOAD)|{0x19,0x1b,0x1d,0x2,0x26,0x29,0x2a,0x32,0x80000022,0x80000028,0x80000033,0x80000034,0xb,0xe,0xd,0x24,0x8000001c,0x27}:fail('unreviewed load command: '+hex(command))
   if command in LOAD:
    name=string(raw,24,endian)
    if not platform_library(name):fail('non-platform dependency: '+name)
    imports.append({'kind':LOAD[command],'name':name})
   elif command==0x8000001c:
    name=string(raw,12,endian)
    if name not in ['@executable_path/../Frameworks','@executable_path/Frameworks','@loader_path/../Frameworks','/usr/lib/swift']:fail('unreviewed rpath: '+name)
    rpaths.append(name)
   elif command==0xe:
    name=string(raw,12,endian)
    if name!='/usr/lib/dyld':fail('unreviewed dynamic linker')
    dylinkers.append(name)
   elif command==0x27:fail('embedded dyld environment forbidden')
   cursor+=size
  if cursor!=end:fail('load-command count/size mismatch')
  result.append({'cpu':cpu,'subtype':sub,'fileType':kind,'imports':imports,'rpaths':rpaths,'dynamicLinkers':dylinkers})
 return result

def audit(app,exclude_updater=False):
 app=pathlib.Path(app)
 if not app.is_absolute() or app.is_symlink() or app.resolve(strict=True)!=app or not app.is_dir():fail('canonical App root required')
 rows=[];updater=app/'Contents/Frameworks/Sparkle.framework'
 for path in sorted(app.rglob('*')):
  rel=path.relative_to(app).as_posix()
  if exclude_updater and (path==updater or updater in path.parents):continue
  s=path.lstat()
  if stat.S_ISLNK(s.st_mode):
   if not path.resolve(strict=True).is_relative_to(app):fail('external product alias')
  elif stat.S_ISREG(s.st_mode):
   with path.open('rb') as f:magic=f.read(4)
   if magic in MAGIC:
    data=path.read_bytes();rows.append({'path':rel,'sha256':sha(data),'architectures':image(data)})
  elif not stat.S_ISDIR(s.st_mode):fail('special product inode')
 if not rows:fail('no executable code')
 if not exclude_updater and updater.exists():fail('updater payload retained in validation product')
 return {'policy':POLICY,'images':rows,'libraryValidationRelaxed':False,'runtimeLaunchProven':False,'updaterCovered':False}

def signature_audit(app,closure):
 # OS verification is read-only; no identity/key query and no signature mutation.
 rows=[]
 for image_row in closure['images']:
  path=pathlib.Path(app)/image_row['path'];queries=[]
  for options in [['--verify','--strict'],['--display','--verbose=4'],['--display','--entitlements',':-']]:
   argv=['/usr/bin/codesign',*options,str(path)];p=subprocess.run(argv,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,timeout=10)
   if p.returncode:fail('actual code signature query failed: '+image_row['path'])
   queries.append({'command':argv,'exitCode':p.returncode,'stdout':p.stdout,'stderr':p.stderr})
  display=queries[1]['stdout']+queries[1]['stderr']
  if 'Signature=adhoc' not in display or 'TeamIdentifier=not set' not in display or 'runtime)' not in display:fail('every actual code image must have explicit adhoc/runtime/noTeam seal')
  if not re.search(r'^CDHash=[0-9a-f]{40}$',display,re.M):fail('missing actual code-directory hash')
  ent=queries[2]['stdout'].strip()
  if ent and plistlib.loads(ent.encode()):fail('entitlements on actual loaded code forbidden')
  rows.append({'path':image_row['path'],'queries':queries})
 return rows

def main_build_signature(path):
 path=pathlib.Path(path);before=path.read_bytes();image(before)
 argv=['/usr/bin/codesign','--display','--verbose=4',str(path)]
 p=subprocess.run(argv,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,timeout=10)
 q={'command':argv,'exitCode':p.returncode,'stdout':p.stdout,'stderr':p.stderr}
 text=p.stdout+p.stderr
 if p.returncode==1 and 'code object is not signed at all' in text:
  # A malformed/unreadable embedded signature is Unknown, never unsigned.
  for _,part in slices(before):
   endian='<' if part[:4]==bytes.fromhex('cffaedfe') else '>';h=struct.unpack_from(endian+'8I',part);cursor=32
   for _ in range(h[4]):
    command,size=struct.unpack_from(endian+'II',part,cursor)
    if command==0x1d and (size!=16 or struct.unpack_from(endian+'II',part,cursor+8)!=(0,0)):fail('unsigned query contradicts embedded signature range')
    cursor+=size
  state='unsigned'
 elif p.returncode==0:
  flags=re.findall(r'flags=0x([0-9a-fA-F]+)\(',text)
  if len(flags)!=1 or int(flags[0],16)!=0x20002 or 'Signature=adhoc' not in text or 'TeamIdentifier=not set' not in text:fail('main is not a known linker-only adhoc build image')
  state='linker-only-adhoc'
 else:fail('main build signature unreadable/unknown')
 if path.read_bytes()!=before:fail('main changed during signature qualification')
 return {'state':state,'sha256':sha(before),'query':q}

def qualify_unsigned_target(app,ref):
 app=pathlib.Path(app)
 if not re.fullmatch('[0-9a-f]{40}',ref):fail('complete source ref required')
 if not app.is_absolute() or app.is_symlink() or app.resolve(strict=True)!=app:fail('target App alias')
 # A vendor framework's own seal is not this root App bundle seal. Never touch
 # a root bundle that was already sealed, even if its Info claims this source.
 seal=app/'Contents/_CodeSignature'
 if seal.exists() or seal.is_symlink():fail('already sealed root App target')
 info_path=app/'Contents/Info.plist'
 if info_path.is_symlink() or not info_path.resolve(strict=True).is_relative_to(app):fail('target Info alias')
 raw=info_path.read_bytes();info=plistlib.loads(raw)
 if info.get('CFBundleIdentifier')!='com.pendingname.pendingcrew.native-candidate' or info.get('PendingCrewDeliveryValidation') is not True or info.get('PendingCrewSourceRef')!=ref or info.get('BuildStampCommit')!=ref:fail('actual target Info identity/source stamps differ')
 name=info.get('CFBundleExecutable')
 if not isinstance(name,str) or pathlib.PurePosixPath(name).name!=name or not name:fail('invalid target executable')
 main=app/'Contents/MacOS'/name
 if main.is_symlink() or main.resolve(strict=True)!=main or not main.is_file() or main.stat().st_uid!=os.getuid():fail('actual main missing/aliased/not-owned')
 signature=main_build_signature(main)
 return {'sourceRef':ref,'infoSHA256':sha(raw),'mainPath':main.relative_to(app).as_posix(),'mainBuildSignature':signature,'rootAppSealAbsent':True}

def prune_target(env):
 # Ordinary configs/platforms are byte-preserving no-ops, including iPhone/iPad.
 if env.get('CONFIGURATION')!='DeliveryValidation' or env.get('PLATFORM_NAME')!='macosx':return {'mode':'NOT_APPLICABLE','mutated':False}
 ref=env.get('PENDINGCREW_VALIDATION_SOURCE_REF','')
 if not re.fullmatch('[0-9a-f]{40}',ref):fail('reviewed full source ref missing')
 if env.get('CODE_SIGNING_ALLOWED')!='NO' or env.get('ENABLE_HARDENED_RUNTIME')!='YES' or env.get('CODE_SIGN_ENTITLEMENTS','') or env.get('DEVELOPMENT_TEAM',''):fail('unsigned hardened/empty-entitlement target required')
 root=pathlib.Path(env.get('TARGET_BUILD_DIR',''));name=env.get('FULL_PRODUCT_NAME','')
 if not root.is_absolute() or root.resolve(strict=True)!=root or not root.is_relative_to('/private/tmp/pcw-codex-first-xcode/Build/Products') or pathlib.PurePosixPath(name).name!=name or not name.endswith('.app'):fail('only reviewed warm target product may be changed')
 app=root/name
 return dict(finalize_unsigned_target(app,ref),mode='BUILD_TARGET_ONLY',sourceRef=ref)

def finalize_unsigned_target(app,ref):
 app=pathlib.Path(app);qualified=qualify_unsigned_target(app,ref)
 report=audit(app,exclude_updater=True);updater=app/'Contents/Frameworks/Sparkle.framework'
 if updater.is_symlink():fail('updater root alias')
 if updater.exists():
  for p in updater.rglob('*'):
   inode=p.lstat()
   if p.is_symlink():
    if not p.resolve(strict=True).is_relative_to(updater):fail('updater payload alias escapes')
   elif not(stat.S_ISREG(inode.st_mode) or stat.S_ISDIR(inode.st_mode)):fail('special updater payload inode')
  if sha((app/'Contents/Info.plist').read_bytes())!=qualified['infoSHA256'] or sha((app/qualified['mainPath']).read_bytes())!=qualified['mainBuildSignature']['sha256']:fail('target changed before payload exclusion')
  shutil.rmtree(updater)
 return dict(audit(app),qualification=qualified)

def main():
 p=argparse.ArgumentParser();p.add_argument('operation',choices=['audit','prune-target']);p.add_argument('app',nargs='?');args=p.parse_args()
 if args.operation=='audit':value=audit(pathlib.Path(args.app))
 else:value=prune_target(os.environ)
 print(json.dumps(value,sort_keys=True,indent=2))
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,KeyError,struct.error) as e:raise SystemExit('FAIL: '+str(e))
