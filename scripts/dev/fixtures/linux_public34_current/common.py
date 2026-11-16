"""Selected peer/transport plus bounded procfs disappearance/error contracts."""
import ctypes,errno,hashlib,json,os,pathlib,socket,stat,struct,subprocess,sys,time,traceback,uuid

def sha(path): return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def save(path, value):
    with open(path, 'x') as f:
        os.chmod(path, 0o600); json.dump(value, f, indent=2); f.write('\n'); f.flush(); os.fsync(f.fileno())


def procfs_gone(error):
    # errno, not exception subclass/message. Only callers at exact generated
    # /proc/<positive-pid>[/task/<positive-tid>] sites may use this predicate.
    return isinstance(error,OSError) and error.errno in (errno.ENOENT,errno.ESRCH)


def procfs_pid(pid):
    if type(pid) is not int or pid<=0:raise ValueError('positive integer procfs PID required')
    return pid


class ProcfsObservationError(RuntimeError):
    def __init__(self,operation,path,pid,error,tid=None):
        self.observation={'operation':operation,'path':str(path),'pid':pid,'tid':tid,
            'errorType':type(error).__name__,'errno':getattr(error,'errno',None),
            'filename':getattr(error,'filename',None),'missingWasNotAccepted':True}
        self.errno=getattr(error,'errno',None)
        super().__init__('procfs observation unknown: '+operation+' '+str(path)+': '+type(error).__name__)


def pid_text(pid,field):
    procfs_pid(pid)
    if field not in ('stat','status'):raise ValueError('only named PID stat/status may disappear')
    path=pathlib.Path('/proc',str(pid),field)
    try:return path.read_text()
    except (OSError,UnicodeError) as error:
        if procfs_gone(error):return None
        raise ProcfsObservationError('read-pid-'+field,path,pid,error) from error


def observation_error(error):
    # Capture while still inside the ORIGINAL except, including NativeRunner's
    # unchanged override. Do not guess a callsite from a free-form error string.
    original={};tb=error.__traceback__
    while tb is not None:
        value=tb.tb_frame.f_locals.get('root')
        if isinstance(value,dict) and all(k in value for k in ('pid','pgid','startTime','bootId')):
            original={k:value.get(k) for k in ('pid','ppid','pgid','uid','startTime','bootId')}
        tb=tb.tb_next
    return {'errorType':type(error).__name__,'errno':getattr(error,'errno',None),
        'filename':getattr(error,'filename',None),'message':str(error),
        'procfs':getattr(error,'observation',None),'originalRootBirth':original or None,
        'traceback':''.join(traceback.format_exception(type(error),error,error.__traceback__)),
        'callsiteInferredFromMessage':False}


class WatchErrors(list):
    """Keep original fatal strings; add first64 exact structured error stacks."""
    def __init__(self):super().__init__();self.details=[];self.details_dropped=0
    def append(self,value):
        super().append(value)  # preserve fatal state even if diagnostics fail
        if len(self.details)>=64:self.details_dropped+=1;return
        error=sys.exc_info()[1]
        try:
            detail=observation_error(error) if error is not None else {
                'errorType':'MissingActiveException','message':str(value),'traceback':''.join(traceback.format_stack()),
                'callsiteInferredFromMessage':False,'diagnosticIncomplete':True}
        except BaseException as capture:
            detail={'errorType':type(error).__name__ if error is not None else None,
                'diagnosticIncomplete':True,'diagnosticErrorType':type(capture).__name__}
        self.details.append(detail)


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
    text=pid_text(pid,'stat')
    if text is None:return None
    try:
        fields=text[text.rfind(')')+2:].split()
        if int(text.split('(',1)[0].strip())!=pid:raise ValueError('PID stat identity differs')
        ppid,pgid,start=int(fields[1]),int(fields[2]),int(fields[19])
    except (ValueError,IndexError) as error:
        raise ProcfsObservationError('parse-pid-stat',pathlib.Path('/proc',str(pid),'stat'),pid,error) from error
    status=pid_text(pid,'status')
    if status is None:return None
    try:uid=int(status.split('Uid:')[1].split()[0])
    except (ValueError,IndexError) as error:
        raise ProcfsObservationError('parse-pid-status',pathlib.Path('/proc',str(pid),'status'),pid,error) from error
    # Global boot metadata is never PID disappearance, even ENOENT/ESRCH.
    boot_path=pathlib.Path('/proc/sys/kernel/random/boot_id')
    try:
        boot=boot_path.read_text().strip()
        if str(uuid.UUID(boot))!=boot:raise ValueError('kernel boot UUID invalid')
    except (OSError,ValueError) as error:
        raise ProcfsObservationError('read-global-boot-id',boot_path,pid,error) from error
    return dict(pid=pid,ppid=ppid,pgid=pgid,uid=uid,startTime=start,bootId=boot)


def same(a,b): return a is not None and b is not None and all(a[k]==b[k] for k in ['pid','pgid','startTime','bootId'])


def wait_for(fn, label, seconds=20):
    end=time.monotonic()+seconds
    while True:
        result=fn()
        if result:return result
        if time.monotonic()>=end:raise TimeoutError(label)
        time.sleep(.03)


class Public:
    def __init__(self,path,owned,required_capabilities=('command.receipts','session.terminal')):
        self.socket=socket.socket(socket.AF_UNIX); self.socket.settimeout(10); self.socket.connect(str(path)); self.counter=0; self.pending=b''; self.events=[]
        if sys.platform=='darwin':
            pid=struct.unpack('i',self.socket.getsockopt(0,2,4))[0]
            cred=self.socket.getsockopt(0,1,80);uid=struct.unpack_from('I',cred,4)[0]
        else:pid,uid,_=struct.unpack('3i',self.socket.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
        assert pid==owned['pid'] and uid==os.getuid() and same(birth(pid),owned)
        self.hello=self.rpc('system.hello',{})
        assert set(required_capabilities) <= set(self.hello['capabilities']), self.hello['capabilities']
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

