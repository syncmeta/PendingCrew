#!/usr/bin/env python3
"""Author production-flow gate. Explicit ordinary daemon + existing test-only programs.
No native SDK, model, discovered executable, installation, user data or private route.
A diagnostic normal must not be relabelled as a frozen strong-provenance candidate.
"""
import argparse, base64, ctypes, hashlib, json, os, pathlib, signal, socket, stat, struct, subprocess, sys, tempfile, time, uuid

def sha(path): return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()
def save(path, value):
    with open(path, 'x') as f:
        os.chmod(path, 0o600); json.dump(value, f, indent=2); f.write('\n'); f.flush(); os.fsync(f.fileno())

def birth(pid):
    if sys.platform == 'darwin':
        class BSD(ctypes.Structure):
            _fields_=[(name,ctypes.c_uint32) for name in ['flags','status','xstatus','pid','ppid','uid','gid','ruid','rgid','svuid','svgid','rfu']]+[('comm',ctypes.c_char*16),('name',ctypes.c_char*32)]+[(name,ctypes.c_uint32) for name in ['nfiles','pgid','jobc','tdev','tpgid']]+[('nice',ctypes.c_int32),('sec',ctypes.c_uint64),('usec',ctypes.c_uint64)]
        info=BSD(); lib=ctypes.CDLL('/usr/lib/libproc.dylib',use_errno=True)
        n=lib.proc_pidinfo(pid,3,0,ctypes.byref(info),ctypes.sizeof(info))
        if n!=ctypes.sizeof(info):
            if n==0 and ctypes.get_errno() in [0,3]: return None
            raise RuntimeError('kernel birth unreadable')
        assert info.pid==pid
        return dict(pid=pid,ppid=info.ppid,uid=info.uid,pgid=info.pgid,startTime=info.sec*1000000+info.usec,bootId=subprocess.check_output(['/usr/sbin/sysctl','-n','kern.bootsessionuuid'],text=True).strip())
    try:
        text=pathlib.Path(f'/proc/{pid}/stat').read_text(); fields=text[text.rfind(')')+2:].split()
        uid=int(pathlib.Path(f'/proc/{pid}/status').read_text().split('Uid:')[1].split()[0])
        return dict(pid=pid,ppid=int(fields[1]),pgid=int(fields[2]),uid=uid,startTime=int(fields[19]),bootId=pathlib.Path('/proc/sys/kernel/random/boot_id').read_text().strip())
    except FileNotFoundError:return None

def same(a,b): return a is not None and b is not None and all(a[k]==b[k] for k in ['pid','pgid','startTime','bootId'])

def wait_for(fn, label, seconds=20):
    end=time.monotonic()+seconds
    while True:
        result=fn()
        if result:return result
        if time.monotonic()>=end:raise TimeoutError(label)
        time.sleep(.03)

class Public:
    def __init__(self,path,owned):
        self.socket=socket.socket(socket.AF_UNIX); self.socket.settimeout(10); self.socket.connect(str(path)); self.counter=0; self.pending=b''; self.events=[]
        if sys.platform=='darwin':
            pid=struct.unpack('i',self.socket.getsockopt(0,2,4))[0]
            cred=self.socket.getsockopt(0,1,80);uid=struct.unpack_from('I',cred,4)[0]
        else:pid,uid,_=struct.unpack('3i',self.socket.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
        assert pid==owned['pid'] and uid==os.getuid() and same(birth(pid),owned)
        self.hello=self.rpc('system.hello',{})
        assert 'command.receipts' in self.hello['capabilities'] and 'session.terminal' in self.hello['capabilities']
        self.identity=self.hello['runtimeIdentity']; assert all(self.identity.values())
    def frame(self):
        while b'\n' not in self.pending:
            data=self.socket.recv(65536)
            if not data:raise EOFError('public connection closed')
            self.pending+=data
            assert len(self.pending)<=1048576
        line,self.pending=self.pending.split(b'\n',1);return json.loads(line)
    def request(self,method,params):
        self.counter+=1;value={'id':self.counter,'method':method,'params':params};self.socket.sendall(json.dumps(value).encode()+b'\n');return self.counter
    def rpc(self,method,params):
        identity=self.request(method,params)
        for _ in range(256):
            frame=self.frame()
            if frame.get('id')==identity:
                if 'error' in frame:raise RuntimeError(json.dumps(frame['error']))
                return frame['result']
            self.events.append(frame)
        raise RuntimeError('bounded public response not observed')
    def prepared(self,operation,params):return dict(requestId=str(uuid.uuid4()),dbId=self.identity['dbId'],runtimeBindingId=self.identity['runtimeBindingId'],command=dict(operation=operation,params=params))
    def submit(self,request):return self.rpc('command.submit',request)
    def lookup(self,request):return self.rpc('command.lookup',dict(requestId=request['requestId'],dbId=request['dbId']))
    def phase(self,request,want):
        def observe():
            result=self.lookup(request)
            if result and result['phase'] in ['unknown','suppressed','rejected'] and result['phase'] not in want:raise RuntimeError(json.dumps(result))
            return result if result and result['phase'] in want else None
        return wait_for(observe,'receipt '+str(want))
    def close(self):self.socket.close()


def qualified_args():
 p=argparse.ArgumentParser();p.add_argument('--binary',required=True);p.add_argument('--witness',required=True);p.add_argument('--expected-source-ref',required=True);p.add_argument('--fake-codex',required=True);p.add_argument('--fake-codex-sha',required=True);p.add_argument('--fake-claude',required=True);p.add_argument('--fake-claude-sha',required=True);p.add_argument('--out',required=True);a=p.parse_args()
 witness=json.loads(pathlib.Path(a.witness).read_text())
 if witness['sourceRef']!=a.expected_source_ref or not witness.get('freshWorkspaceArtifacts') or sha(a.binary)!=witness['binarySHA256']:raise ValueError('exact strong normal source/binary proof missing')
 for path,digest,name in [(a.fake_codex,a.fake_codex_sha,'fake_codex'),(a.fake_claude,a.fake_claude_sha,'fake_claude')]:
  q=pathlib.Path(path)
  if not q.is_absolute() or q.is_symlink() or not q.is_file() or q.name!=name or sha(q)!=digest:raise ValueError('explicit fake driver source/hash qualification missing')
 q=pathlib.Path(a.out)
 if not q.is_absolute() or q.exists() or len(str(q/'data/crewd.sock').encode())>103:raise ValueError('fresh private short endpoint required')
 return a,witness

def socket_owned(path,owned):
 s=socket.socket(socket.AF_UNIX);s.settimeout(.2)
 try:
  s.connect(str(path))
  if sys.platform=='darwin':
   pid=struct.unpack('i',s.getsockopt(0,2,4))[0];uid=struct.unpack_from('I',s.getsockopt(0,1,80),4)[0]
  else:pid,uid,_=struct.unpack('3i',s.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
  return pid==owned['pid'] and uid==os.getuid() and same(birth(pid),owned)
 except (FileNotFoundError,ConnectionRefusedError,TimeoutError):return False
 finally:s.close()

def physical_observation(original):
 if original is None:return None
 report={'original':original}
 try:report['currentIdentity']=birth(original['pid'])
 except Exception as e:report['identityError']=repr(e)
 try:os.kill(-original['pgid'],0);report['groupState']='present'
 except ProcessLookupError:report['groupState']='absent'
 except OSError as e:report.update(groupState='unknown',groupErrno=e.errno)
 return report

def retained_registry(data):
 path=pathlib.Path(data)/'exec-registry.json'
 if not path.exists():return {'exists':False}
 report={'exists':True,'sha256':sha(path)}
 try:report['entries']=json.loads(path.read_text())
 except Exception as e:report['readError']=repr(e)
 return report

def special_inode_evidence(root):
 root=pathlib.Path(root);records=[]
 for current,dirs,files in os.walk(root,followlinks=False):
  for name in dirs+files:
   path=pathlib.Path(current)/name;info=path.lstat()
   if stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode):continue
   row={'path':str(path.relative_to(root)),'mode':info.st_mode,'uid':info.st_uid,'gid':info.st_gid,'device':info.st_dev,'inode':info.st_ino,'size':info.st_size,'type':'socket' if stat.S_ISSOCK(info.st_mode) else 'fifo' if stat.S_ISFIFO(info.st_mode) else 'symlink' if stat.S_ISLNK(info.st_mode) else 'other'}
   if stat.S_ISLNK(info.st_mode):row['linkText']=os.readlink(path)
   records.append(row)
 return {'metadataOnlyNotRestorableFilesystem':True,'specialInodes':records,'originalSpecialInodesNotDeleted':True,'externalLinksNeverFollowed':True}
