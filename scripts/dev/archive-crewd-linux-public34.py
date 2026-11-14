#!/usr/bin/env python3
"""Preserve diagnostics; never signal a process or label live files cold."""
import hashlib,io,json,os,pathlib,stat,sys,tarfile


def main():
    out=pathlib.Path(sys.argv[1]).resolve()
    if not out.is_dir():return
    assert not (out/'delivery-evidence-manifest.json').exists()
    failures=[]
    for scope in sorted(p for p in (out/'runtime').glob('*') if p.is_dir()):
        record=scope/'result.json'
        value=json.loads(record.read_bytes()) if record.exists() else {'exit':1,'terminalResultMissing':True}
        if value.get('exit')==0:continue
        directory=value.get('privateRuntimeRetained')
        if not directory:
            witnesses=list(scope.glob('observations/*-private-runtime.json'))
            if len(witnesses)==1:directory=json.loads(witnesses[0].read_bytes())['path']
        if not directory:continue
        root=pathlib.Path(directory)
        assert root.parent==pathlib.Path('/tmp') and root.name.startswith('pcl34-')
        info=root.lstat();assert stat.S_ISDIR(info.st_mode) and info.st_uid==os.getuid() and stat.S_IMODE(info.st_mode)==0o700
        rows=[];target=record.parent/'mutable-failure-files.tar.gz'
        # Every read uses O_NOFOLLOW on the observed regular inode. Socket/FIFO
        # and links are metadata only. A changing file stays explicitly unstable.
        with tarfile.open(target,'x:gz') as archive:
            for parent,dirs,files in os.walk(root,followlinks=False):
                for name in sorted(dirs+files):
                    path=pathlib.Path(parent)/name;relative=path.relative_to(root).as_posix();before=path.lstat()
                    row={'path':relative,'device':before.st_dev,'inode':before.st_ino,'mode':before.st_mode,'uid':before.st_uid}
                    if stat.S_ISREG(before.st_mode):
                        fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
                        with os.fdopen(fd,'rb') as stream:
                            opened=os.fstat(stream.fileno());assert (opened.st_dev,opened.st_ino)==(before.st_dev,before.st_ino)
                            data=stream.read();after=os.fstat(stream.fileno())
                        meta=tarfile.TarInfo(relative);meta.size=len(data);meta.mode=0o600
                        archive.addfile(meta,io.BytesIO(data))
                        row.update(sha256=hashlib.sha256(data).hexdigest(),bytes=len(data),
                            stableObservedRead=(before.st_size,before.st_mtime_ns)==(after.st_size,after.st_mtime_ns))
                    elif stat.S_ISLNK(before.st_mode):row['linkTargetMetadataOnly']=os.readlink(path)
                    else:row['contentsCaptured']=False
                    rows.append(row)
        manifest={'mutableFailureSnapshot':True,'coldOrConsistentOrRestorable':False,'originalRuntime':str(root),
            'processObligations':value.get('ownedToolObligations',[]),'files':rows,'forcedCleanup':False}
        (record.parent/'mutable-failure-files.json').write_text(json.dumps(manifest,indent=2)+'\n');failures.append(manifest)
    rows=[]
    for path in sorted(out.rglob('*')):
        info=path.lstat()
        if stat.S_ISREG(info.st_mode):
            rows.append({'path':path.relative_to(out).as_posix(),'bytes':info.st_size,'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
        elif not stat.S_ISDIR(info.st_mode):raise ValueError('special/link inode cannot be uploaded as an ordinary artifact')
    (out/'delivery-evidence-manifest.json').write_text(json.dumps({'files':rows,'mutableFailures':len(failures),
        'newAcceptanceClaim':False,'published':False,'systemdInstalled':False},indent=2)+'\n')


if __name__=='__main__':main()
