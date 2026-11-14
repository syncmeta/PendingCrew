"""Immutable reads of stopped, copied private data. No SQL runs at import time."""
import contextlib,hashlib,json,os,re,sqlite3,stat,uuid
from pathlib import Path
from common import sha

ROOT='pendingcrew-welcome'
CREWS={ROOT:('PendingCrew怎么用',[]),'pendingcrew-welcome-demo':('演示',[ROOT]),
       'pendingcrew-welcome-questions':('问答',[ROOT]),
       'pendingcrew-welcome-child':('子机组',['pendingcrew-welcome-demo','pendingcrew-welcome-questions'])}
ASSETS=['SKILL.md','INDEX.md','INDEX-v2.md','README-source-v1.md','README-public-v2.md',
        'README_EN-public-v1.md','architecture-source-v1.md','guide-v1.md','CORE-guide-v1.md']
SOURCE=None # Set only by full source/witness qualification before a flow.

@contextlib.contextmanager
def immutable(path):
    assert path.is_absolute() and path.is_file() and not path.is_symlink()
    assert not Path(str(path)+'-wal').exists() and not Path(str(path)+'-shm').exists()
    before=sha(path);c=sqlite3.connect(path.as_uri()+'?immutable=1',uri=True)
    try:
        assert c.execute('PRAGMA integrity_check').fetchone()[0]=='ok'
        assert c.execute('PRAGMA foreign_key_check').fetchall()==[]
        yield c
    finally:c.close()
    assert sha(path)==before

def meta(c,key):
    row=c.execute('SELECT value FROM meta WHERE key=?',(key,)).fetchone()
    return row[0] if row else None

def rows(c,table):
    assert table in ['crews','organization_work_book','organization_work_events','object_refs','resource_mutation_requests']
    cur=c.execute('SELECT * FROM '+table);names=[x[0] for x in cur.description]
    return [dict(zip(names,[{'sqliteBlobHex':v.hex()} if isinstance(v,bytes) else v for v in row])) for row in cur.fetchall()]

def zero_business(c):
    counts={t:c.execute('SELECT count(*) FROM '+t).fetchone()[0] for t in ['sessions','messages','wakes','todos','ask_meta']}
    assert all(v==0 for v in counts.values()),counts
    assert c.execute("SELECT count(*) FROM object_refs WHERE kind IN ('plan','human_task','requirement')").fetchone()[0]==0
    books=rows(c,'organization_work_book');assert len(books)==1
    book=json.loads(books[0]['state']);assert book['items']=={} and book['asks']=={}
    return {'counts':counts,'currentOrganizationBook':books,'organizationEvents':rows(c,'organization_work_events')}

def core_copy(directory,version):
    with immutable(directory/'crewd.db') as c:
        assert c.execute('PRAGMA user_version').fetchone()[0]==version
        raw=meta(c,'onboarding.core.v1');projection=meta(c,'onboarding.export.v1')
        return {'dbId':meta(c,'dbId'),'fresh':meta(c,'onboarding.fresh.v1'),
                'facts':json.loads(raw) if raw else None,'projection':json.loads(projection) if projection else None,
                'crews':rows(c,'crews'),'business':zero_business(c),'resourceMutationReceipts':rows(c,'resource_mutation_requests')}

def initial(directory):
    result=core_copy(directory,34);facts=result['facts'];assert result['fresh']=='1'
    assert type(result['dbId']) is str and re.fullmatch('[0-9a-f]{32}',result['dbId']) is not None
    assert facts['dbId']==result['dbId'] and not facts['skipped'] and facts['pending'] is None and len(facts['ready'])==1
    publication=facts['ready'][0];assert publication['edition']==1 and publication['createdAt']
    assert len(publication['pages'])==9 and {x['asset']['file'] for x in publication['pages']}==set(ASSETS)
    ids=[publication['request']]+[page[key] for page in publication['pages'] for key in ['view','block','placement']]
    assert len(set(ids))==28 and all(str(uuid.UUID(key))==key for key in ids)
    origin=publication['origin'];assert origin['dbId']==origin['authorityId']==result['dbId'] and origin['crewId']==ROOT
    assert origin['generation']=='0' and origin['restoreEpoch']=='initial'
    ns=publication['origin']['namespace']
    with immutable(directory/'knowledge'/(ns+'.sqlite')) as k:
        assert k.execute('PRAGMA application_id').fetchone()[0]==0x50434b31 and k.execute('PRAGMA user_version').fetchone()[0]==1
        context=json.loads(k.execute("SELECT value FROM _pc_meta WHERE key='context'").fetchone()[0])
        receipt=json.loads(k.execute('SELECT body FROM _pc_receipts WHERE id=?',(publication['request'],)).fetchone()[0])
        assert receipt['origin']==publication['origin'] and receipt['principal']=='builtin:onboarding.core.v1'
        assert receipt['digest']==publication['payloadSha256']
        assert receipt['result']['phase']=='committed' and receipt['result']['requestId']==publication['request']
        assert receipt['result']['context']==context
        assert context['generation']==receipt['result']['afterGeneration']=='1' and receipt['result']['beforeGeneration']=='0'
        assert all(context[key]==origin[key] for key in ['dbId','authorityId','crewId','namespace','restoreEpoch'])
        objects={}
        for page in publication['pages']:
            body=(SOURCE/'crewd/crates/server/src/onboarding/assets'/page['asset']['file']).read_bytes()
            assert hashlib.sha256(body).hexdigest()==page['asset']['sha256'] and len(body)==page['asset']['bytes']
            for kind,key in [('view','view'),('block','block')]:
                stored=json.loads(k.execute('SELECT body FROM _pc_objects WHERE kind=? AND id=?',(kind,page[key])).fetchone()[0]);obj=stored['object']
                assert obj['id']==page[key] and obj['revision']=='1' and not obj['archived'];objects[(kind,page[key])]=obj
            assert objects['block',page['block']]['content'].encode()==body
            view=objects['view',page['view']];assert view['title']==page['asset']['file']
            placement=view['definition']['placements'];assert len(placement)==1 and placement[0]['id']==page['placement']
            assert placement[0]['block']['reference']=={'namespace':ns,'kind':'block','id':page['block']}
        assert k.execute('SELECT count(*) FROM _pc_objects').fetchone()[0]==18
    result.update(publication=publication,context=context,builtinReceipt=receipt['result'],objects={kind+':'+key:v for (kind,key),v in objects.items()})
    assert result['projection']['state']=='ready' and result['projection']['generation']==context['generation']
    return result

def owned(root,entry):
    # Export receipts use clean relative paths, including version-*/manifest.json.
    # Anchor every component to an owned directory FD; leaf O_NOFOLLOW alone
    # would still follow a substituted parent directory.
    assert type(entry) is dict and type(entry.get('path')) is str
    parts=entry['path'].split('/')
    assert all(p not in ('','.','..') and re.fullmatch('[A-Za-z0-9_.-]+',p) for p in parts)
    assert all(type(entry.get(k)) is int and entry[k]>=0 for k in ('device','inode','bytes'))
    assert type(entry.get('sha256')) is str and re.fullmatch('[0-9a-f]{64}',entry['sha256'])
    assert root.is_absolute() and root.resolve(strict=True)==root
    flags=os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC
    def identity(s):return (s.st_dev,s.st_ino)
    def directory(s):
        assert stat.S_ISDIR(s.st_mode) and s.st_uid==os.getuid() and stat.S_IMODE(s.st_mode)&0o077==0
    def leaf(s):
        assert stat.S_ISREG(s.st_mode) and s.st_uid==os.getuid() and s.st_nlink==1
        assert identity(s)==(entry['device'],entry['inode'])
        assert stat.S_IMODE(s.st_mode)==0o400 and s.st_size==entry['bytes']
    def chain():
        fds=[]
        try:
            before=root.lstat();directory(before)
            fds.append(os.open(root,flags));opened=os.fstat(fds[-1]);directory(opened)
            assert identity(before)==identity(opened) and root.resolve(strict=True)==root
            ids=[identity(opened)]
            for part in parts[:-1]:
                before=os.stat(part,dir_fd=fds[-1],follow_symlinks=False);directory(before)
                child=os.open(part,flags,dir_fd=fds[-1]);fds.append(child)
                opened=os.fstat(child);directory(opened)
                assert identity(before)==identity(opened);ids.append(identity(opened))
            return fds,ids
        except BaseException:
            for fd in reversed(fds):os.close(fd)
            raise
    fds,ids=chain();fd=None
    try:
        before=os.stat(parts[-1],dir_fd=fds[-1],follow_symlinks=False);leaf(before)
        fd=os.open(parts[-1],os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC|os.O_NONBLOCK,dir_fd=fds[-1])
        a=os.fstat(fd);leaf(a);assert identity(a)==identity(before)
        chunks=[];remaining=entry['bytes']+1
        while remaining:
            block=os.read(fd,min(remaining,65536))
            if not block:break
            chunks.append(block);remaining-=len(block)
        data=b''.join(chunks);b=os.fstat(fd);leaf(b)
        assert (a.st_dev,a.st_ino,a.st_size,a.st_mtime_ns,a.st_ctime_ns)==(b.st_dev,b.st_ino,b.st_size,b.st_mtime_ns,b.st_ctime_ns)
        assert len(data)==entry['bytes'] and hashlib.sha256(data).hexdigest()==entry['sha256']
        fresh,fresh_ids=chain()
        try:
            assert fresh_ids==ids
            leaf(os.stat(parts[-1],dir_fd=fresh[-1],follow_symlinks=False))
        finally:
            for current in reversed(fresh):os.close(current)
        return data
    finally:
        if fd is not None:os.close(fd)
        for current in reversed(fds):os.close(current)

def closure(actual_data,projection,context,objects,pages):
    root=actual_data/'welcome-crew';d=root.lstat();assert stat.S_ISDIR(d.st_mode) and not root.is_symlink() and d.st_uid==os.getuid()
    assert projection['state']=='ready' and projection['current'] and not projection['reservedCurrent'] and not projection['retiredCurrent']
    pointer=json.loads(owned(root,projection['current']));assert pointer['format']==1 and pointer['readOnlyProjection'] is True
    assert pointer['generation']==projection['generation']==context['generation']
    entries=[m for m in projection['manifests'] if m['path']==pointer['manifest'] and m['sha256']==pointer['manifestSHA256']];assert len(entries)==1
    manifest=json.loads(owned(root,entries[0]));assert all(manifest[k]==context[k] for k in ['dbId','crewId','namespace','restoreEpoch','generation'])
    assert len(manifest['pages'])==len(pages)==9
    for page,export in zip(pages,manifest['pages']):
        obj=objects['block:'+page['block']];view=objects['view:'+page['view']]
        assert export['file']==page['asset']['file'] and export['viewId']==page['view'] and export['viewRevision']==view['revision']
        assert export['title']==view['title'] and export['blocks']==[[page['block'],obj['revision']]]
        entry=[x for x in projection['owned'] if x['path']==export['exportPath'] and x['sha256']==export['sha256'] and x['bytes']==export['bytes']];assert len(entry)==1
        assert owned(root,entry[0])==obj['content'].encode()
    return manifest
