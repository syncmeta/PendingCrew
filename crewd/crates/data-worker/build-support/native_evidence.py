#!/usr/bin/python3
"""Seal native sidecar proof bytes. Called by the authorized build producer only."""
import sys
sys.dont_write_bytecode = True
import hashlib,json,os,pathlib,stat
MAX_FILE=512*1024*1024
MAX_TOTAL=2*1024*1024*1024

def require(ok,message):
    if not ok:raise ValueError(message)

def stamp(m):
    return (m.st_dev,m.st_ino,m.st_size,m.st_mtime_ns,m.st_ctime_ns,m.st_mode,m.st_uid)

def pointer(part):return str(part).replace('~','~0').replace('/','~1')

def proofs(value,at=''):
    if isinstance(value,dict):
        if {'path','sha256','bytes'}<=set(value):yield at,value
        for key,child in value.items():yield from proofs(child,at+'/'+pointer(key))
    elif isinstance(value,list):
        for i,child in enumerate(value):yield from proofs(child,at+'/'+str(i))

def guarded_bytes(path,bound):
    path=pathlib.Path(path)
    require(path.is_absolute() and path.resolve(strict=True)==path,'noncanonical record input')
    m=path.lstat();require(stat.S_ISREG(m.st_mode) and m.st_uid==os.geteuid() and not m.st_mode&0o022 and m.st_size<=bound,'unqualified record input')
    fd=os.open(str(path),os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC|os.O_NONBLOCK)
    try:
        require(stamp(os.fstat(fd))==stamp(m),'record changed before open')
        parts=[];n=0
        while True:
            b=os.read(fd,65536)
            if not b:break
            n+=len(b);require(n<=bound,'record size bound');parts.append(b)
        require(stamp(os.fstat(fd))==stamp(m) and stamp(path.lstat())==stamp(m),'record changed while reading')
        return b''.join(parts)
    finally:os.close(fd)

def write_all(fd,data):
    data=memoryview(data)
    while data:
        n=os.write(fd,data)
        if n<=0:raise OSError('short native snapshot write')
        data=data[n:]

class Sealer:
    def __init__(self,root):
        self.root=pathlib.Path(root);m=self.root.lstat()
        require(self.root.is_absolute() and self.root.resolve(strict=True)==self.root and stat.S_ISDIR(m.st_mode) and m.st_uid==os.geteuid() and stat.S_IMODE(m.st_mode)==0o700,'unqualified evidence root')
        self.objects=self.root/'objects';self.objects.mkdir(mode=0o700)
        self.seen={};self.total=0

    def snapshot(self,proof):
        path=pathlib.Path(proof['path']);expected=proof['sha256'];size=proof['bytes']
        require(isinstance(expected,str) and len(expected)==64 and all(c in '0123456789abcdef' for c in expected),'invalid snapshot hash')
        require(type(size) is int and 0<=size<=MAX_FILE,'snapshot size bound')
        require(path.is_absolute() and path.resolve(strict=True)==path,'noncanonical native snapshot input')
        m=path.lstat();require(stat.S_ISREG(m.st_mode) and m.st_uid in (0,os.geteuid()) and not m.st_mode&0o022 and m.st_size==size,'unqualified native snapshot input')
        source=os.open(str(path),os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC|os.O_NONBLOCK);target=None;new=expected not in self.seen
        try:
            require(stamp(os.fstat(source))==stamp(m),'native snapshot changed before open')
            if new:
                require(self.total+size<=MAX_TOTAL,'native unique evidence size bound')
                target=os.open(str(self.objects/expected),os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC,0o600)
            h=hashlib.sha256();n=0
            while True:
                data=os.read(source,65536)
                if not data:break
                n+=len(data);require(n<=size,'native snapshot grew');h.update(data)
                if target is not None:write_all(target,data)
            require(n==size and h.hexdigest()==expected,'native snapshot bytes mismatch')
            require(stamp(os.fstat(source))==stamp(m) and stamp(path.lstat())==stamp(m),'native snapshot changed during capture')
            if target is not None:os.fsync(target)
        finally:
            os.close(source)
            if target is not None:os.close(target)
        if new:self.seen[expected]=size;self.total+=size
        else:require(self.seen[expected]==size,'hash reused with different size')
        return {'originalPath':str(path),'snapshotPath':'objects/'+expected,'sha256':expected,'bytes':size}

    def mapping(self,value,excluded=()):
        return [dict(pointer=at,**self.snapshot(proof)) for at,proof in proofs(value) if at not in excluded]

    def seal(self,sidecar,raw_records,policy_sha):
        require(sidecar['artifactRole']=='crewd-data-worker' and sidecar['sourceInputsBefore']==sidecar['sourceInputsAfter'],'native source before/after mismatch')
        require(len(raw_records)==len(sidecar['commands']) and 1<=len(raw_records)<=128,'native command count mismatch')
        index={'schemaVersion':1,'policySHA256':policy_sha,'sourceRef':sidecar['build']['sourceRef'],'artifactRole':'crewd-data-worker','evidence':self.mapping(sidecar,('/commandIndex',)),'records':[]}
        for i,(raw_path,raw) in enumerate(raw_records):
            summary=sidecar['commands'][i]
            require(raw['role']==summary['tool'],'native raw role mismatch')
            for key in ('argv','cwd','startedAtUtc','endedAtUtc','outcome','exitCode','signal','toolBeforeSHA256','toolAfterSHA256','stdout','stderr'):
                require(raw[key]==summary[key],'native raw summary mismatch: '+key)
            require(raw['policySHA256']==policy_sha,'native raw policy mismatch')
            path=pathlib.Path(raw_path);data=guarded_bytes(path,1024*1024)
            require(json.loads(data)==raw,'native raw record mismatch')
            snap=self.snapshot({'path':str(path),'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)})
            index['records'].append({'commandIndex':i,'snapshotPath':snap['snapshotPath'],'sha256':snap['sha256'],'bytes':snap['bytes'],'evidence':self.mapping(raw)})
        data=(json.dumps(index,sort_keys=True,indent=2)+'\n').encode();out=self.root/'command-index.json'
        fd=os.open(str(out),os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC,0o600)
        try:write_all(fd,data);os.fsync(fd)
        finally:os.close(fd)
        for directory in (self.objects,self.root):
            fd=os.open(str(directory),os.O_RDONLY|os.O_DIRECTORY|os.O_CLOEXEC)
            try:os.fsync(fd)
            finally:os.close(fd)
        return {'path':'command-index.json','sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)}
