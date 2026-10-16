#!/usr/bin/python3
"""Pinned native CC/AR provenance wrapper. Development build input, never runtime RPC."""
import sys
sys.dont_write_bytecode = True
import os, json, hashlib, stat, pathlib, subprocess, time, uuid, datetime, selectors
MAX_FILE = 512 * 1024 * 1024
MAX_OUTPUT = 16 * 1024 * 1024
CHILD_KEYS = {"PATH", "LANG", "LC_ALL", "TZ", "TMPDIR", "SDKROOT", "DEVELOPER_DIR", "MACOSX_DEPLOYMENT_TARGET"}
INJECT = {"CCC_OVERRIDE_OPTIONS", "CLANG_CONFIG_FILE_USER_DIR", "CLANG_CONFIG_FILE_SYSTEM_DIR", "CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "OBJC_INCLUDE_PATH", "LIBRARY_PATH", "COMPILER_PATH", "GCC_EXEC_PREFIX", "LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT", "LLVM_AR_LIBRARY_PATH"}

def write_all(fd, data):
    remaining=memoryview(data)
    while remaining:
        n=os.write(fd,remaining)
        if n<=0:raise OSError("short native evidence write")
        remaining=remaining[n:]

def require(ok, message):
    if not ok: raise RuntimeError(message)

def stamp(m):
    return (m.st_dev,m.st_ino,m.st_size,m.st_mtime_ns,m.st_ctime_ns,m.st_mode,m.st_uid)

def read_regular(path, bound, executable=False):
    p=pathlib.Path(path)
    require(p.is_absolute() and str(p.resolve(strict=True))==str(p), "noncanonical native input")
    m=p.lstat(); require(stat.S_ISREG(m.st_mode) and m.st_size<=bound and m.st_uid in (0,os.geteuid()) and not m.st_mode & 0o022,"native input qualification failed")
    if executable: require(m.st_mode & 0o111 and not m.st_mode & 0o6000,"native tool qualification failed")
    fd=os.open(str(p),os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC|os.O_NONBLOCK)
    try:
        require(stamp(os.fstat(fd))==stamp(m),"native input changed before open")
        h=hashlib.sha256(); parts=[]; total=0
        while True:
            b=os.read(fd,65536)
            if not b: break
            total+=len(b);require(total<=bound,"native input exceeded bound");h.update(b)
            if not executable: parts.append(b)
        require(stamp(os.fstat(fd))==stamp(m) and stamp(p.lstat())==stamp(m),"native input changed while reading")
        return h.hexdigest(),total,b"".join(parts)
    finally: os.close(fd)

def private_dir(path):
    p=pathlib.Path(path);m=p.lstat();require(str(p.resolve(strict=True))==str(p) and stat.S_ISDIR(m.st_mode) and m.st_uid==os.geteuid() and stat.S_IMODE(m.st_mode)==0o700,"private recorder directory unavailable");return p

def proof(path):
    h,n,_=read_regular(path,MAX_OUTPUT);return {"path":str(path),"sha256":h,"bytes":n}

def frozen_config():
    path=os.environ.get("PCW_NATIVE_C_RECORD_CONFIG","");expected=os.environ.get("PCW_NATIVE_C_RECORD_CONFIG_SHA256","")
    h,_,data=read_regular(path,256*1024);require(h==expected,"native recorder config hash mismatch")
    c=json.loads(data);require(set(c)=={"version","recorderSHA256","policyFile","policySHA256","recordRoot","programs","childEnvironment","timeoutSeconds"} and c["version"]==1,"native recorder config shape")
    require(read_regular(pathlib.Path(__file__).resolve(),1024*1024)[0]==c["recorderSHA256"],"native recorder source changed")
    require(read_regular(c["policyFile"],1024*1024)[0]==c["policySHA256"],"native policy source changed")
    require(set(c["programs"])=={"compiler","archiver"},"native tool role mismatch")
    for tool in c["programs"].values():
        require(set(tool)=={"path","sha256"},"native tool config fields")
        require(read_regular(tool["path"],MAX_FILE,True)[0]==tool["sha256"],"native tool hash mismatch")
    root=private_dir(c["recordRoot"]);private_dir(root/"tmp")
    env=c["childEnvironment"]; require(set(env)<=CHILD_KEYS and {"PATH","LANG","LC_ALL","TZ","TMPDIR"}<=set(env) and all(isinstance(x,str) for x in env.values()),"native child environment shape")
    require(env["LANG"]==env["LC_ALL"]=="C" and env["TZ"]=="UTC" and env["TMPDIR"]==str(root/"tmp") and env["PATH"]=="/usr/bin:/bin","native child environment constants")
    require(1<=c["timeoutSeconds"]<=300,"native compiler deadline bound")
    return c,h

def run():
    c,config_sha=frozen_config();role={"native-cc":"compiler","native-ar":"archiver"}.get(pathlib.Path(sys.argv[0]).name);require(role is not None,"unknown native wrapper role")
    rejected=sorted(k for k,v in os.environ.items() if v and (k in INJECT or k.startswith("DYLD_") or k.startswith("CCC_") or k.startswith("CLANG_CONFIG")))
    require(not rejected,"ambient native injection refused: "+",".join(rejected))
    args=sys.argv[1:];require(len(args)<=1024 and sum(len(a.encode()) for a in args)<=1024*1024,"native argv bound")
    forbidden=("--config","--config-user-dir","--config-system-dir","-fplugin","-fpass-plugin","-Xclang","-ivfsoverlay","-B","--gcc-toolchain","-resource-dir","-fno-integrated-as","-Xassembler","-Xpreprocessor","-load","-include","-imacros","-specs","--sysroot")
    require(not any(a.startswith("@") or any(a.startswith(v) for v in forbidden) for a in args),"unreviewed compiler config/plugin/response argument")
    program=c["programs"][role];tool_before=read_regular(program["path"],MAX_FILE,True)[0]
    argv=[program["path"]]+(["--no-default-config"] if role=="compiler" else [])+args
    root=private_dir(c["recordRoot"]);ident=uuid.uuid4().hex;stdout_path=root/(ident+".stdout");stderr_path=root/(ident+".stderr");record_path=root/(ident+".json")
    flags=os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC
    outfd=os.open(str(stdout_path),flags,0o600);errfd=os.open(str(stderr_path),flags,0o600)
    started=datetime.datetime.now(datetime.timezone.utc).isoformat();outcome="spawnFailure";code=None;sig=None;child=None;error=None
    output_complete=True;captured={outfd:0,errfd:0}
    try:
        child=subprocess.Popen(argv,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=dict(c["childEnvironment"]),close_fds=True)
        end=time.monotonic()+c["timeoutSeconds"]
        with selectors.DefaultSelector() as selector:
            for pipe,fd in ((child.stdout,outfd),(child.stderr,errfd)):
                os.set_blocking(pipe.fileno(),False);selector.register(pipe,selectors.EVENT_READ,fd)
            outcome="exited"
            while selector.get_map():
                if time.monotonic()>=end:
                    outcome="timeout";output_complete=False;break
                for key,_ in selector.select(min(0.1,max(0,end-time.monotonic()))):
                    data=os.read(key.fileobj.fileno(),65536)
                    if not data:selector.unregister(key.fileobj);key.fileobj.close();continue
                    fd=key.data;remaining=MAX_OUTPUT-captured[fd]
                    write_all(fd,data[:remaining]);captured[fd]+=min(len(data),remaining)
                    if len(data)>remaining:
                        outcome="timeout";error="output bound exceeded";output_complete=False;break
                if not output_complete:break
            if output_complete:
                try:child.wait(timeout=max(0.001,end-time.monotonic()))
                except subprocess.TimeoutExpired:outcome="timeout";output_complete=False
    except Exception as e:error=type(e).__name__;output_complete=False
    finally:
        # Only this exact unreaped Popen may be killed. A terminal observed
        # process is never signalled again, even if a descendant retains a pipe.
        if child is not None and child.poll() is None:
            child.kill()
            try:child.wait(timeout=5)
            except subprocess.TimeoutExpired:error="original compiler terminal unknown"
        if child is not None:
            for pipe in (child.stdout,child.stderr):
                if pipe is not None and not pipe.closed:pipe.close()
            if child.returncode is not None:
                if child.returncode<0:sig=-child.returncode
                else:code=child.returncode
        os.fsync(outfd);os.fsync(errfd);os.close(outfd);os.close(errfd)
    ended=datetime.datetime.now(datetime.timezone.utc).isoformat();tool_after=read_regular(program["path"],MAX_FILE,True)[0]
    # Raw per-command file retains the exact child environment independently of
    # the ambient environment map in the normal witness native sidecar.
    record={"schemaVersion":1,"role":role,"packageName":os.environ.get("CARGO_PKG_NAME"),"argv":argv,"cwd":os.getcwd(),"startedAtUtc":started,"endedAtUtc":ended,"outcome":outcome,"exitCode":code,"signal":sig,"originalChildPID":child.pid if child else None,"childTerminalObserved":child is not None and child.returncode is not None,"outputComplete":output_complete,"childEnvironment":c["childEnvironment"],"toolBeforeSHA256":tool_before,"toolAfterSHA256":tool_after,"configSHA256":config_sha,"policySHA256":c["policySHA256"],"recorderSHA256":c["recorderSHA256"],"stdout":proof(stdout_path),"stderr":proof(stderr_path),"error":error}
    fd=os.open(str(record_path),flags,0o600)
    try:write_all(fd,(json.dumps(record,sort_keys=True)+"\n").encode());os.fsync(fd)
    finally:os.close(fd)
    fd=os.open(str(root),os.O_RDONLY|os.O_DIRECTORY|os.O_CLOEXEC)
    try:os.fsync(fd)
    finally:os.close(fd)
    # Preserve compiler stdout/stderr semantics for cc probes and Cargo raw logs.
    sys.stdout.buffer.write(read_regular(stdout_path,MAX_OUTPUT)[2]);sys.stderr.buffer.write(read_regular(stderr_path,MAX_OUTPUT)[2])
    require(tool_before==tool_after==program["sha256"],"native tool changed during invocation")
    if outcome!="exited" or error is not None or not output_complete:return 125
    return 128+sig if sig else code

if __name__=="__main__":
    try:sys.exit(run())
    except Exception as e:
        sys.stderr.write("native recorder refused: "+str(e)+"\n");sys.exit(125)
