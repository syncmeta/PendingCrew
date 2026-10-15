#!/usr/bin/python3
"""Owned real old14 -> Jev15 mechanism gate; never a normal-build witness."""
import sys
sys.dont_write_bytecode = True
import argparse, hashlib, importlib.util, json, os, pathlib, signal, sqlite3, stat, subprocess, time, uuid

def sha(p):
    h=hashlib.sha256()
    with open(p,'rb') as f:
        for part in iter(lambda:f.read(65536),b''): h.update(part)
    return h.hexdigest()
def require(v,why):
    if not v: raise RuntimeError(why)
def save(p,v):
    fd=os.open(str(p),os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
    with os.fdopen(fd,'w') as f: json.dump(v,f,indent=2);f.write('\n');f.flush();os.fsync(f.fileno())
def regular(p):
    st=p.lstat();require(stat.S_ISREG(st.st_mode) and not p.is_symlink(),'regular input required')
    require(p.resolve()==p,'canonical input required')

a=argparse.ArgumentParser()
for name in ['old-binary','old-witness','new-binary','new-binary-sha','new-source-manifest','common','common-sha','out']:a.add_argument('--'+name,required=True)
a=a.parse_args()
old=pathlib.Path(a.old_binary);new=pathlib.Path(a.new_binary);wpath=pathlib.Path(a.old_witness);common=pathlib.Path(a.common);source=pathlib.Path(a.new_source_manifest)
for p in [old,new,wpath,common,source]: regular(p)
require(sys.platform=='darwin','this old artifact is the reviewed arm64 Darwin Core14')
w=json.loads(wpath.read_text())
require(w['sourceRef']=='31f0b512b7e54bb6d7a34dc715a1aca0db1caad6' and w.get('freshWorkspaceArtifacts') is True,'sealed old14 source')
require(sha(old)==w['binarySHA256']=='48e83f900d82ef6519880c3fe5a4055a5e187ed7e2594360ea4dd7fa00e22fc2','sealed old14 binary')
require(sha(wpath)=='4706786a893fe1cb5c0429d006512844ee2a57f0ad010ae3da55e298c5a3f781','sealed old14 witness')
require(sha(new)==a.new_binary_sha and sha(common)==a.common_sha,'new diagnostic/source dependency changed')
out=pathlib.Path(a.out);require(out.is_absolute() and not os.path.lexists(str(out)) and out.parent.resolve()==out.parent,'fresh canonical private output')
require(len(str(out/'data/crewd.sock').encode())<=103,'short owned socket required')
out.mkdir(mode=0o700)
for name in ['data','home','config','cache','work']:(out/name).mkdir(mode=0o700)
spec=importlib.util.spec_from_file_location('owned_recovery_common',common);f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)
data=out/'data';db=data/'crewd.db';processes=[];connections=[];events=[]
env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(out/'home'),'XDG_CACHE_HOME':str(out/'cache'),'CLAUDE_CONFIG_DIR':str(out/'config'),'TMPDIR':str(out),'SHELL':'/usr/bin/false','LANG':'C.UTF-8','GIT_CONFIG_NOSYSTEM':'1','GIT_CONFIG_GLOBAL':'/dev/null'}
result={'layer':'real old14 sealed binary and new Jev15 diagnostic; private false-provider DB migration only','oldRef':w['sourceRef'],'oldBinarySHA':sha(old),'newBinarySHA':sha(new),'newSourceManifestSHA':sha(source),'commonSHA':sha(common),'fixtureSHA':sha(__file__),'events':events,'SDKModelKeychainAppInvocations':0}
initialInputs={'old':sha(old),'oldWitness':sha(wpath),'new':sha(new),'common':sha(common),'source':sha(source),'fixture':sha(__file__)}
def logopen(name):return os.fdopen(os.open(str(out/name),os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600),'wb')
def census():
    rows={}
    for name in ['crewd.db','crewd.db.id','crewd.db-wal','crewd.db-shm','crewd.db-journal']:
        p=data/name
        if not os.path.lexists(str(p)):rows[name]=None;continue
        regular(p);st=p.stat();rows[name]={'sha256':sha(p),'mode':stat.S_IMODE(st.st_mode),'size':st.st_size,'inode':st.st_ino,'device':st.st_dev}
    return rows
def inspect():
    require(not (data/'crewd.db-wal').exists() and not (data/'crewd.db-shm').exists() and not (data/'crewd.db-journal').exists(),'immutable observer requires proven stopped sidecar-free cut')
    c=sqlite3.connect(db.as_uri()+'?immutable=1',uri=True)
    try:
        return {'version':c.execute('PRAGMA user_version').fetchone()[0],'dbId':c.execute("SELECT value FROM meta WHERE key='dbId'").fetchone()[0],'nativeTables':c.execute("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE 'native_history_%'").fetchone()[0],'marker':c.execute("SELECT value FROM meta WHERE key='judgment.format.v1'").fetchone(),'requirements':c.execute("SELECT id,text FROM todos WHERE ledger='agent' ORDER BY id").fetchall()}
    finally:c.close()
def spawn(binary,tag):
    with logopen(tag+'.log') as log:p=subprocess.Popen([str(binary),'--data-dir',str(data),'serve','--codex','/usr/bin/false','--claude','/usr/bin/false'],env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=log)
    processes.append(p);events.append({'tag':tag,'pid':p.pid,'argvKind':'serve false/false'})
    return p
def start(binary,tag):
    p=spawn(binary,tag);deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        require(p.poll() is None,tag+' exited before qualified socket')
        b=f.birth(p.pid)
        if b and f.socket_owned(data/'crewd.sock',b):
            require(b['ppid']==os.getpid() and b['uid']==os.geteuid() and bool(b['bootId']),'owned creator/birth')
            c=f.Public(data/'crewd.sock',b);connections.append(c);events[-1]['birth']=b;return p,c
        time.sleep(.02)
    raise TimeoutError(tag+' socket')
def stop(p,c,tag):
    q=c.prepared('crewd.hibernate',{'deadlineSecs':0});save(out/(tag+'-quit.json'),q);reply=c.submit(q);require(reply['requestId']==q['requestId'],'quit request identity');require(p.wait(10)==0,'owned daemon normal exit');c.close();events.append({'tag':tag,'terminal':p.returncode})
def refuse(binary,tag):
    p=spawn(binary,tag);require(p.wait(10)!=0,tag+' unexpectedly admitted');require(not (data/'crewd.sock').exists(),tag+' exposed socket');events.append({'tag':tag,'terminal':p.returncode})
def quiesce_old(tag):
    p,c=start(old,tag)
    require(c.rpc('todo.get',{'id':todo['requirementId']})['text']==todo['text'],'fault preparation changed source')
    stop(p,c,tag)
    inspect() # Require the genuine old writer's stopped sidecar-free cut.
def sql_fault(enable):
    c=sqlite3.connect(str(db))
    try:
        c.execute("CREATE TRIGGER jev_marker_fault BEFORE INSERT ON meta WHEN new.key='judgment.format.v1' BEGIN SELECT RAISE(ABORT,'owned injected format marker fault'); END" if enable else 'DROP TRIGGER jev_marker_fault');c.commit()
    finally:c.close()
def deadline(_signum,_frame):raise TimeoutError('whole migration cooperative 120s budget')
signal.signal(signal.SIGALRM,deadline);signal.alarm(120)
try:
    p,c=start(old,'old14-create')
    req=c.prepared('crew.createFirstMessage',{'crew':{'title':'format15 owned source','workingDirectory':str(out/'work')},'text':'FORMAT15_ORIGINAL_MESSAGE'})
    save(out/'original-command.json',req);c.submit(req);crew=c.phase(req,['committed'])['result']['crew']['id']
    todo=c.rpc('todo.create',{'requestId':str(uuid.uuid4()),'crewId':crew,'text':'FORMAT15_IMMUTABLE_REQUIREMENT','channelId':None});save(out/'original-requirement.json',todo)
    stop(p,c,'old14-create');before=inspect();require(before['version']==14 and before['marker'] is None,'genuine old14 created database')
    save(out/'old14-before.json',before)
    require(bool(before['requirements']) and before['nativeTables']==0,'old source facts')
    obstruction=data/'backups';obstruction.write_bytes(b'owned protective backup obstacle');os.chmod(obstruction,0o600);physical=census();save(out/'backup-before.json',physical);refuse(new,'new15-backup-refusal');save(out/'backup-after.json',census());require(census()==physical,'backup failure touched originals');require(inspect()==before,'backup failure changed logical source');obstruction.rename(out/'backup-obstacle-retained')
    sql_fault(True);quiesce_old('old14-fault-quiesce');physical=census();save(out/'marker-before.json',physical);fault=inspect();refuse(new,'new15-marker-rollback');save(out/'marker-after.json',census());require(census()==physical,'marker transaction failure changed original bytes/mode');require(inspect()==fault and fault['version']==14 and fault['marker'] is None,'PRAGMA/marker transaction did not rollback');sql_fault(False);quiesce_old('old14-retry-quiesce')
    p,c=start(new,'new15-upgrade');require(c.rpc('todo.get',{'id':todo['requirementId']})['text']==todo['text'],'requirement changed');require(c.lookup(req)['phase']=='committed','original SQL receipt lost');stop(p,c,'new15-upgrade')
    after=inspect();require(after['version']==15 and after['marker']==('1',) and after['nativeTables']==0,'Jev-only format15, no Native activation');require(after['dbId']==before['dbId'] and after['requirements']==before['requirements'],'immutable business identity')
    backups=list((data/'backups').glob('crewd-v14-pre-v15-*.db'));require(bool(backups),'no real protective old14 backup');result['protectedBackups']=[{'path':str(x),'sha256':sha(x)} for x in backups]
    physical=census();save(out/'old14-refusal-before.json',physical);refuse(old,'old14-future-refusal');save(out/'old14-refusal-after.json',census());require(census()==physical,'old14 future refusal touched original files');stderr=(out/'old14-future-refusal.log').read_text();require('15' in stderr and '14' in stderr and '拒绝' in stderr,'not a version refusal')
    p,c=start(new,'new15-reopen');require(c.lookup(req)['phase']=='committed','reopen SQL result changed');require(c.rpc('todo.get',{'id':todo['requirementId']})['text']==todo['text'],'reopen source changed');stop(p,c,'new15-reopen');require(inspect()==after,'reopen changed stored business cut')
    result.update(status='PASS',before=before,after=after,old14PhysicalRefusal=physical,newVersionReopened=True)
except BaseException as e:result.update(status='FAIL',error=repr(e))
finally:
    signal.alarm(0);cleanup=[]
    for c in connections:
        try:c.close()
        except OSError:pass
    for p in processes:
        if p.poll() is None:
            try:
                p.terminate()
                try:p.wait(5)
                except subprocess.TimeoutExpired:p.kill();p.wait(5)
            except BaseException as e:cleanup.append({'pid':p.pid,'error':repr(e)})
        if p.poll() is None:cleanup.append({'pid':p.pid,'unreaped':True})
    if cleanup:result.update(status='FAIL',cleanupErrors=cleanup)
    result['processTerminals']=[{'pid':p.pid,'terminal':p.returncode} for p in processes]
    result['inputBeforeSHA']=initialInputs;result['inputAfterSHA']={}
    for key,path in [('old',old),('oldWitness',wpath),('new',new),('common',common),('source',source),('fixture',pathlib.Path(__file__))]:
        try:result['inputAfterSHA'][key]=sha(path)
        except BaseException as e:result.update(status='FAIL');result.setdefault('inputReadErrors',[]).append({'input':key,'error':repr(e)})
    if result['inputBeforeSHA']!=result['inputAfterSHA']:result.update(status='FAIL',inputChanged=True)
    save(out/'result.json',result);print(json.dumps({'status':result['status'],'error':result.get('error'),'result':str(out/'result.json')}),flush=True)
sys.exit(0 if result['status']=='PASS' else 1)
