#!/usr/bin/env python3
"""Actual Linux: pinned ordinary21 -> new33, original UUID, tables/docs/search,
normal reopen and formal backup/restore. No service installation or providers.
Every start is our direct child. Every success ends by its original Hibernate.
"""
import argparse, contextlib, fcntl, hashlib, importlib.util, json, os, pathlib
import platform, shutil, socket, sqlite3, stat, struct, subprocess, sys, time, urllib.request, uuid
sys.dont_write_bytecode = True
PREDECESSOR = '48b3452ed554337f3ed41db116c095903f9f044b'
PREDECESSOR_TAR_SHA256 = '3abafc5e3e22dfa70fb365196079f1fdc4216788abc39c98d24bac0ad64a6d3b'

def require(ok, message):
    if not ok: raise ValueError(message)
def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def save(path, value):
    with path.open('x') as out:
        json.dump(value, out, indent=2); out.write('\n'); out.flush(); os.fsync(out.fileno())
def load_module(name, path):
    spec=importlib.util.spec_from_file_location(name, path)
    module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module); return module

def birth(pid):
    try:
        text=pathlib.Path(f'/proc/{pid}/stat').read_text(); fields=text[text.rfind(')')+2:].split()
        return {'pid':pid,'ppid':int(fields[1]),'startTime':int(fields[19]),
                'bootId':pathlib.Path('/proc/sys/kernel/random/boot_id').read_text().strip()}
    except FileNotFoundError: return None

def inventory(root):
    rows={}
    for path in sorted(root.rglob('*')):
        info=path.lstat(); require(not path.is_symlink(), 'cold authority alias')
        require(stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode), 'cold authority special inode')
        rows[str(path.relative_to(root))]={'mode':stat.S_IMODE(info.st_mode), **({'sha256':sha(path)} if path.is_file() else {'directory':True})}
    return rows

def facts(c):
    names=['organization_work_book','organization_work_events','messages','object_refs']
    return {name:hashlib.sha256(repr(sorted(c.execute('SELECT * FROM '+name).fetchall(),key=repr)).encode()).hexdigest() for name in names}

def cold(data, version):
    require(not any((data/name).exists() for name in ['crewd.sock','crewd.db-wal','crewd.db-shm','crewd.db-journal']), 'cold authority still has live or recovery sidecars')
    registry=data/'exec-registry.json'
    require(not registry.exists() or json.loads(registry.read_text())=={}, 'unresolved executor registry')
    with contextlib.closing(sqlite3.connect((data/'crewd.db').as_uri()+'?immutable=1',uri=True)) as c:
        require(c.execute('PRAGMA user_version').fetchone()[0]==version, 'actual Core schema differs')
        require(c.execute('PRAGMA application_id').fetchone()[0]==0x43525744, 'foreign Core identity')
        require(c.execute('PRAGMA integrity_check').fetchall()==[('ok',)] and not c.execute('PRAGMA foreign_key_check').fetchall(), 'actual Core integrity')
        require(c.execute("SELECT value FROM meta WHERE key='lifecycle.clean'").fetchone()==('true',), 'original clean shutdown missing')
        dbid=c.execute("SELECT value FROM meta WHERE key='dbId'").fetchone()[0]
        require((data/'crewd.db.id').read_text().splitlines()[0]==dbid, 'original DB identity differs')
        return dbid

class Core:
    def __init__(self, program, source, out, data, home, label, report):
        self.program,self.source,self.out,self.data,self.home,self.label,self.report=program,source,out,data,home,label,report
        self.child=None;self.sock=None;self.pending=b'';self.counter=0;self.owner=None
    def __enter__(self):
        self.log=(self.out/(self.label+'-core.log')).open('xb')
        env={'PATH':'/usr/bin:/bin','HOME':str(self.home),'XDG_CACHE_HOME':str(self.home/'cache'),'SHELL':'/bin/false'}
        command=[str(self.program),'--data-dir',str(self.data),'serve','--disable-claude','--codex','/usr/bin/false','--codex-env','HOME='+str(self.home)]
        save(self.out/(self.label+'-command.json'),command)
        self.child=subprocess.Popen(command,env=env,cwd=self.out/'work',stdin=subprocess.DEVNULL,stdout=self.log,stderr=self.log)
        self.owner=birth(self.child.pid)
        require(self.owner and self.owner['ppid']==os.getpid(), 'exact direct owned child required')
        try:
            require(pathlib.Path(f'/proc/{self.child.pid}/exe').resolve()==self.program.resolve(), 'actual child executable differs')
            deadline=time.monotonic()+30
            while True:
                require(self.child.poll() is None, 'owned ordinary Core exited before readiness')
                sock=socket.socket(socket.AF_UNIX);sock.settimeout(10)
                try: sock.connect(str(self.data/'crewd.sock')); self.sock=sock; break
                except OSError:
                    sock.close()
                    if time.monotonic()>=deadline: raise TimeoutError('owned socket readiness')
                    time.sleep(.05)
            pid,uid,_=struct.unpack('3i',self.sock.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
            require(pid==self.child.pid and uid==os.getuid() and birth(pid)==self.owner, 'socket peer is not our exact child')
            self.hello=self.call('system.hello',{})
            require(self.hello['build']['gitSha']==self.source[:12] and self.hello['build']['dirty'] is False, 'embedded actual source mismatch')
            require('command.receipts' in self.hello['capabilities'], 'original command receipt capability required')
            self.identity=self.hello['runtimeIdentity']
            require(all(self.identity.values()), 'actual runtime identity absent')
            self.call('lifecycle.claim',{})
            status=self.call('remote.status',{})
            require(not status['listening'] and not status['configuration']['enabled'], 'remote was enabled')
            save(self.out/(self.label+'-hello.json'),self.hello)
            return self
        except BaseException:
            self.cleanup_failure(); raise
    def frame(self):
        while b'\n' not in self.pending:
            data=self.sock.recv(65536)
            require(bool(data), 'unknown RPC outcome; never resend')
            self.pending+=data; require(len(self.pending)<=1048576, 'response bound')
        raw,self.pending=self.pending.split(b'\n',1);return json.loads(raw)
    def call(self,method,params):
        self.counter+=1; rid=self.counter
        self.sock.sendall(json.dumps({'id':rid,'method':method,'params':params}).encode()+b'\n')
        for _ in range(256):
            value=self.frame()
            if value.get('id')==rid:
                with (self.out/'rpc.jsonl').open('a') as log: log.write(json.dumps({'phase':self.label,'method':method,'params':params,'response':value})+'\n')
                require('error' not in value,'RPC '+method+': '+json.dumps(value.get('error')))
                return value['result']
        raise ValueError('bounded response missing')
    def cleanup_failure(self):
        if self.sock: self.sock.close(); self.sock=None
        if self.child and self.child.poll() is None:
            require(birth(self.child.pid)==self.owner,'failed child birth changed; no signal')
            self.report['nonAcceptanceCleanup']='owned failed child terminated; gate is not accepted'
            self.child.terminate()
            try:self.child.wait(timeout=15)
            except subprocess.TimeoutExpired:self.child.kill();self.child.wait()
        self.log.close()
    def __exit__(self,kind,error,trace):
        if kind is not None: self.cleanup_failure(); return False
        try:
            require(self.call('session.list',{'crewId':self.report['crew']})==[], 'provider session was created')
            request={'requestId':str(uuid.uuid4()),**self.identity,'command':{'operation':'crewd.hibernate','params':{'deadlineSecs':0}}}
            save(self.out/(self.label+'-original-hibernate-request.json'),request)
            receipt=self.call('command.submit',request)
            deadline=time.monotonic()+30
            while receipt['phase']!='confirmed':
                require(receipt['phase'] not in ['unknown','suppressed','rejected'],'original Hibernate not confirmed')
                require(time.monotonic()<deadline,'original Hibernate deadline; no resend')
                receipt=self.call('command.lookup',{'requestId':request['requestId'],'dbId':request['dbId']})
                require(receipt is not None,'original Hibernate unknown; no resend')
                if receipt['phase']!='confirmed':time.sleep(.02)
            require(receipt['requestId']==request['requestId'],'original Hibernate changed')
            require(all(receipt.get('result',{}).get(k) is True for k in ['actorCompleted','hibernationRecordPersisted','physicalChildrenExited']),'original Hibernate physical completion unconfirmed')
            save(self.out/(self.label+'-original-hibernate.json'),receipt)
            self.sock.close();self.sock=None
            require(self.child.wait(timeout=30)==0 and not (self.data/'crewd.sock').exists(),'original child did not exit naturally')
            self.report['phases'].append({'phase':self.label,'pid':self.child.pid,'birth':self.owner,'originalHibernateUUID':request['requestId'],'confirmed':True,'exit':0,'socketGone':True})
            self.log.close()
        except BaseException:self.cleanup_failure();raise

def main():
    p=argparse.ArgumentParser(description=__doc__)
    for name in ['package','source','output']:p.add_argument('--'+name,required=True)
    args=p.parse_args();require(platform.system()=='Linux' and os.geteuid()!=0,'actual ordinary Linux user required')
    out=pathlib.Path(args.output).resolve();out.mkdir(mode=0o700,exist_ok=False)
    report={'scope':'Actual ordinary Linux21->33, original UUID, tables/docs/search and formal backup/restore; no systemd install, provider or GUI','phases':[],'passed':False}
    try:
        package=pathlib.Path(args.package).resolve();repo=pathlib.Path(__file__).resolve().parents[2]
        host=load_module('exact_host_installer',repo/'deploy/linux/pendingcrew-host.py')
        manifest=json.loads((package/'manifest.json').read_bytes());payload=(package/'host.tar.gz').read_bytes()
        require(host.sha(payload)==manifest['artifact']['sha256'] and manifest['artifact']['sourceCommit']==args.source,'new package binding differs')
        newroot=out/'new-package';newroot.mkdir();new,release=host.extract_release(payload,newroot,manifest['artifact'])
        require(release['storeVersion']==33 and release['format']==2,'actual new source33 package required')
        oldurl='https://github.com/syncmeta/PendingCrew/releases/download/host-'+PREDECESSOR+'/host.tar.gz'
        oldpayload=host.download(oldurl,PREDECESSOR_TAR_SHA256,268435456)
        (out/'original21-host.tar.gz').write_bytes(oldpayload)
        oldroot=out/'old-package';oldroot.mkdir();old,oldrelease=host.extract_release(oldpayload,oldroot,{'sourceCommit':PREDECESSOR,'architecture':'x86_64'})
        require(oldrelease['storeVersion']==21 and oldrelease['format']==1,'pinned original21 package differs')
        data=out/'data';home=out/'home'
        for d in [data,home,out/'work']:d.mkdir(mode=0o700)
        require(len(os.fsencode(data/'crewd.sock'))<104,'runtime endpoint too long')
        original=str(uuid.uuid4());text='ordinary Linux original21 UUID remains original';post={'requestId':original,'text':text}
        with Core(old/'bin/crewd',PREDECESSOR,out,data,home,'old21-seed',report) as c:
            crew=c.call('crew.create',{'title':'Linux33 actual protected migration','workingDirectory':str(out/'work')})['id'];report['crew']=crew
            post['crewId']=crew;made=c.call('todo.create',post);original_id=made['requirementId']
        identity=cold(data,21);prior=inventory(data)
        with (data/'crewd.lock').open('rb') as lock:
            fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB);shutil.copytree(data,out/'original21-cold');require(inventory(data)==inventory(out/'original21-cold')==prior,'complete old cold copy changed')
        original_cold_sha=sha(out/'original21-cold/crewd.db')
        with contextlib.closing(sqlite3.connect((out/'original21-cold/crewd.db').as_uri()+'?immutable=1',uri=True)) as old_db:original_facts=facts(old_db)
        save(out/'original21-facts.json',original_facts)
        lookup={'crewId':crew,'requestId':original,'textSha256':hashlib.sha256(text.encode()).hexdigest()}
        def check_original(c):
            found=c.call('todo.createLookup',lookup)
            require(found['status']=='committed' and found['requestId']==original and found['receipt']['requirementId']==original_id,'original21 UUID was not recovered')
        def official(label,*argv):
            env={'PATH':'/usr/bin:/bin','HOME':str(home),'XDG_CACHE_HOME':str(home/'cache')}
            result=subprocess.run([str(new/'bin/crewd'),'--data-dir',str(data),*argv],env=env,capture_output=True,text=True,timeout=60)
            save(out/(label+'.json'),{'exit':result.returncode,'stdout':result.stdout,'stderr':result.stderr});require(result.returncode==0,'formal '+label+' failed')
        with Core(new/'bin/crewd',args.source,out,data,home,'new33-migrated',report) as c:
            require(c.identity['dbId']==identity,'real migration changed original DB identity');check_original(c)
            document=c.call('document.create',{'crewId':crew,'requestId':str(uuid.uuid4()),'title':'Linux33 document','body':'Linux33 durable body'})['document']['id']
            scope=c.call('managedObject.scope',{})
            made=c.call('managedTable.v2.submit',{'scope':scope,'crewId':crew,'requestId':str(uuid.uuid4()),'operation':{'kind':'createTable','title':'Linux33 table','purpose':'Actual ordinary Linux history','fields':[{'name':'Name','kind':{'kind':'text'},'nullable':False}],'titleFieldIndex':0,'rowWritePolicy':'authorCaptainHuman'}})
            table=made['effect']['tableId']
            row=c.call('managedTable.v2.submit',{'scope':scope,'crewId':crew,'requestId':str(uuid.uuid4()),'operation':{'kind':'addRow','tableId':table,'expectedSchemaRevision':'1','patches':[{'fieldId':'p1','change':{'kind':'setScalar','value':{'type':'text','value':'linux33-runtime-needle'}}}]}})['effect']['rowId']
            require(c.call('managedTable.v3.row.get',{'id':row})['id']==row,'actual row not readable')
            require(c.call('managedTable.v3.history',{'source':{'kind':'row','id':row}})['items'],'real row action history absent')
            deadline=time.monotonic()+60
            while True:
                results=c.call('organization.search',{'query':'linux33-runtime-needle','tableId':table})
                if results['indexState']=='ready':break
                require(time.monotonic()<deadline,'actual Source search did not become Ready');time.sleep(.05)
            require(results['items'],'actual new row not found')
            require(c.call('adminAskGroups.list',{'crewId':crew})['availability']=='disabled','unconfigured real admin classifier unexpectedly enabled')
        require(cold(data,33)==identity,'new33 normal cold identity differs')
        protected=list((data/'backups').glob('crewd-v21-pre-v33-*.db'));require(len(protected)==1,'real21 protected migration snapshot absent')
        with contextlib.closing(sqlite3.connect(protected[0].as_uri()+'?immutable=1',uri=True)) as db:
            require(db.execute('PRAGMA user_version').fetchone()[0]==21,'protected snapshot is not actual21')
            require(db.execute("SELECT value FROM meta WHERE key='dbId'").fetchone()[0]==identity,'protected old identity differs')
            require(facts(db)==original_facts,'protected original21 business facts differ')
        backups=set((data/'backups').glob('crewd-manual-*.db'));official('formal-backup','backup')
        created=set((data/'backups').glob('crewd-manual-*.db'))-backups;require(len(created)==1,'formal backup not uniquely identified');backup=created.pop();backup_sha=sha(backup)
        after_text='Created strictly after formal snapshot'
        with Core(new/'bin/crewd',args.source,out,data,home,'new33-reopen',report) as c:
            check_original(c);require(c.call('document.get',{'id':document})['body']=='Linux33 durable body','document did not survive normal reopen')
            after=c.call('todo.create',{'crewId':crew,'requestId':str(uuid.uuid4()),'text':after_text})['id']
        require(cold(data,33)==identity,'reopen cold identity differs');official('formal-restore','restore','--from',str(backup));require(sha(backup)==backup_sha,'formal backup changed')
        with Core(new/'bin/crewd',args.source,out,data,home,'new33-restored',report) as c:
            check_original(c)
            require(c.call('document.get',{'id':document})['body']=='Linux33 durable body','formal restore lost original document')
            require(c.call('managedTable.v3.row.get',{'id':row})['id']==row,'formal restore lost actual row')
            require(all(t['id']!=after for t in c.call('todo.list',{'crewId':crew,'ledger':'agent'})),'post-backup object incorrectly survived restore')
        require(cold(data,33)==identity,'restored cold identity differs')
        require(sha(out/'original21-cold/crewd.db')==original_cold_sha and inventory(out/'original21-cold')==prior,'original cold predecessor changed')
        report.update(passed=True,source=args.source,original21Source=PREDECESSOR,original21TarSHA256=PREDECESSOR_TAR_SHA256,schemaVersion=33,dbId=identity,originalRequestId=original,originalRequirementId=original_id,normalBackupRestore=True,real21ProtectedMigration=True,providersExecuted=False,installedSystemd=False)
    except BaseException as error:
        report['error']=str(error);save(out/'result.json',report);raise
    save(out/'result.json',report);print(json.dumps({'passed':True,'result':str(out/'result.json')}))

if __name__=='__main__':
    os.umask(0o077)
    try:main()
    except Exception as error:raise SystemExit('FAIL retained Linux package runtime: '+str(error))
