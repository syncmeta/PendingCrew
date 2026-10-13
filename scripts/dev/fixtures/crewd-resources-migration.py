"""Isolated actual v9->v10 daemon upgrade/reopen/backward refusal, never daily data."""
import contextlib,hashlib,json,pathlib,shutil,socket,sqlite3,subprocess,sys,time,uuid
base=pathlib.Path(sys.argv[1]);old,new,fake=map(pathlib.Path,sys.argv[2:5])
for name in ['home','work','data']:(base/name).mkdir()
env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(base/'home'),'SHELL':'/usr/bin/false'}
def args(program,data):return [str(program),'--data-dir',str(data),'serve','--codex',str(fake),'--claude','/usr/bin/false','--codex-env','HOME='+str(base/'home'),'--codex-env','SHELL=/usr/bin/false']
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def version(data):
 with sqlite3.connect('file:'+str(data/'crewd.db')+'?mode=ro',uri=True) as c:return c.execute('pragma user_version').fetchone()[0]
class RPC:
 def __init__(self,data):
  self.sock=socket.socket(socket.AF_UNIX);self.sock.settimeout(10);self.sock.connect(str(data/'crewd.sock'));self.file=self.sock.makefile('rb');self.seq=0
 def call(self,method,params):
  self.seq+=1;self.sock.sendall((json.dumps({'id':self.seq,'method':method,'params':params})+'\n').encode())
  while True:
   line=self.file.readline();assert line,'daemon closed before reply';reply=json.loads(line)
   if reply.get('id')!=self.seq:continue
   if 'error' in reply:raise AssertionError((method,reply['error']))
   return reply['result']
 def close(self):self.file.close();self.sock.close()
@contextlib.contextmanager
def server(program,data,name,crash=False):
 with (base/(name+'.log')).open('w') as log:
  p=subprocess.Popen(args(program,data),env=env,stdout=log,stderr=log)
  try:
   for _ in range(200):
    assert p.poll() is None,'daemon exited: '+name
    try:rpc=RPC(data);break
    except OSError:time.sleep(.05)
   else:raise AssertionError('socket timeout')
   try:yield rpc
   finally:rpc.close()
  finally:
   if p.poll() is None:
    if crash:p.kill()
    else:p.terminate()
   try:p.wait(timeout=20)
   except subprocess.TimeoutExpired:p.kill();p.wait();raise
   assert p.returncode==(-9 if crash else 0),(name,p.returncode)
with server(old,base/'data','old-v9') as rpc:
 crew=rpc.call('crew.create',{'title':'synthetic real v9 preservation','workingDirectory':str(base/'work')})['id']
 source=rpc.call('todo.create',{'crewId':crew,'requestId':str(uuid.uuid4()),'text':'immutable pre-upgrade original'})['requirementId']
 plan=rpc.call('work.create',{'crewId':crew,'title':'pre-upgrade preserved plan','sources':[source]})
assert version(base/'data')==9,'explicit old binary must really produce v9, not fake PRAGMA downgrade'
pre=digest(base/'data/crewd.db');shutil.copytree(base/'data',base/'backup-failure')
blocked=base/'backup-failure/backups'
if blocked.exists():shutil.rmtree(blocked)
blocked.write_text('deliberate real filesystem backup obstacle')
p=subprocess.run(args(new,base/'backup-failure'),env=env,capture_output=True,timeout=20)
(base/'backup-failure.log').write_bytes(p.stdout+p.stderr)
assert p.returncode!=0 and version(base/'backup-failure')==9
assert digest(base/'backup-failure/crewd.db')==pre,'failed backup must leave original v9 database unchanged'
request=str(uuid.uuid4());original={'crewId':crew,'requestId':request,'title':'immutable create witness','body':'exact original 🌙 body'}
with server(new,base/'data','upgrade-v10') as rpc:
 hello=rpc.call('system.hello',{});assert {'documents','viewFolders'}.issubset(hello['capabilities'])
 assert rpc.call('todo.get',{'id':source})['text']=='immutable pre-upgrade original'
 assert rpc.call('work.get',{'id':plan['id']})['sources']==[source]
 made=rpc.call('document.create',original);doc=made['document']['id']
 changed=rpc.call('document.update',{'id':doc,'expectedRevision':made['document']['revision'],'title':'edited collaboratively','body':'current preserved 🌙 body'})
 tree=rpc.call('viewFolder.list',{'crewId':crew,'view':'documents'})
 tree=rpc.call('viewFolder.create',{'crewId':crew,'view':'documents','requestId':str(uuid.uuid4()),'expectedTreeRevision':tree['treeRevision'],'name':'retained classification'})
 tree=rpc.call('viewFolder.assign',{'crewId':crew,'view':'documents','expectedTreeRevision':tree['treeRevision'],'object':{'kind':'document','id':doc},'folderId':tree['folders'][0]['id']})
assert version(base/'data')==10
backups=list((base/'data/backups').glob('crewd-v9-pre-v10-*.db'));assert len(backups)==1
with sqlite3.connect('file:'+str(backups[0])+'?mode=ro',uri=True) as c:
 assert c.execute('pragma user_version').fetchone()[0]==9
 assert c.execute('pragma integrity_check').fetchone()[0]=='ok'
 assert c.execute('select text from todos where ledger="agent"').fetchone()[0]=='immutable pre-upgrade original'
before=digest(base/'data/crewd.db')
p=subprocess.run(args(old,base/'data'),env=env,capture_output=True,timeout=20);(base/'old-refusal.log').write_bytes(p.stdout+p.stderr)
assert p.returncode!=0 and version(base/'data')==10 and digest(base/'data/crewd.db')==before,'old binary must not write upgraded database'
with server(new,base/'data','reopen-v10') as rpc:
 detail=rpc.call('document.get',{'id':doc});assert detail['body']=='current preserved 🌙 body'
 assert rpc.call('viewFolder.list',{'crewId':crew,'view':'documents'})==tree
 retry=rpc.call('document.create',original);assert retry['reused'] and retry['document']['id']==doc and retry['document']['title']=='edited collaboratively'
 assert retry['document']['revision']==changed['revision']
# Real CLI backup/restore is a separate entry from automatic version upgrade.
with server(new,base/'data','manual-backup-v10') as rpc:
 p=subprocess.run([str(new),'--data-dir',str(base/'data'),'backup'],env=env,capture_output=True,text=True,timeout=20);assert p.returncode==0,(p.stdout,p.stderr)
 manual=pathlib.Path(p.stdout.strip());assert manual.is_file()
 refused=subprocess.run([str(new),'--data-dir',str(base/'data'),'restore','--from',str(manual)],env=env,capture_output=True,timeout=20);assert refused.returncode!=0,'live directory lock must refuse restore'
shutil.copytree(base/'data',base/'restore-target')
restore_root=base/'restore-target'
with sqlite3.connect('file:'+str(manual)+'?mode=ro',uri=True) as c:
 backup_id=c.execute("SELECT value FROM meta WHERE key='dbId'").fetchone()[0]
with server(new,restore_root,'before-crash-restore',crash=True) as rpc:
 current=rpc.call('document.get',{'id':doc})
 rpc.call('document.update',{'id':doc,'expectedRevision':current['revision'],'title':'after backup mutation','body':'this newer body must be replaced'})
 assert (restore_root/'crewd.db-wal').is_file() and (restore_root/'crewd.db-shm').is_file()
# Record original crash evidence before restore. The production restore may legally
# checkpoint WAL into the main file; preservation means recoverable original facts,
# not identical pre-checkpoint DB/WAL bytes.
crash_hashes={name:digest(restore_root/name) for name in ['crewd.db','crewd.db-wal','crewd.db-shm']}
(base/'pre-restore-crash-hashes.json').write_text(json.dumps(crash_hashes,indent=2))
p=subprocess.run([str(new),'--data-dir',str(restore_root),'restore','--from',str(manual)],env=env,capture_output=True,timeout=20);(base/'actual-restore.log').write_bytes(p.stdout+p.stderr);assert p.returncode==0
replaced=[p for p in restore_root.glob('crewd.db.replaced-*') if not p.name.endswith(('-wal','-shm'))]
assert len(replaced)==1,'restore must retain the original database authority'
with sqlite3.connect('file:'+str(replaced[0])+'?mode=ro',uri=True) as c:
 assert c.execute('pragma integrity_check').fetchone()[0]=='ok'
 assert c.execute('SELECT body FROM documents WHERE id=?',(doc,)).fetchone()[0]=='this newer body must be replaced','replaced database plus any renamed companions must retain crash-time facts'
# Any retained companions use <replaced database>-wal/-shm, not a new authority's names.
assert not (restore_root/'crewd.db-wal').exists() and not (restore_root/'crewd.db-shm').exists()
assert (restore_root/'crewd.db.id').read_text().splitlines()[0]==backup_id,'restore identity must bind the source backup; same-db restore need not invent a new UUID'
with server(new,restore_root,'after-actual-restore') as rpc:
 assert rpc.call('document.get',{'id':doc})['body']=='current preserved 🌙 body'
 assert rpc.call('viewFolder.list',{'crewId':crew,'view':'documents'})==tree
 assert rpc.call('document.create',original)['reused'],'immutable request witness survives actual restore'
 assert (restore_root/'crewd.db.id').read_text().splitlines()==[backup_id,'ino='+str((restore_root/'crewd.db').stat().st_ino)],'reopen must bind identity to the restored inode'
# Legacy Swift JSON import is its own public CLI path; source is a synthetic copy.
legacy=base/'legacy-source';shutil.copytree(pathlib.Path(sys.argv[5]),legacy)
legacy_hash={p.relative_to(legacy).as_posix():digest(p) for p in legacy.rglob('*') if p.is_file()}
legacy_dest=base/'legacy-imported'
p=subprocess.run([str(new),'--data-dir',str(legacy_dest),'import','--from',str(legacy)],env=env,capture_output=True,timeout=20);(base/'actual-legacy-import.log').write_bytes(p.stdout+p.stderr);assert p.returncode==0
assert version(legacy_dest)==10
assert {p.relative_to(legacy).as_posix():digest(p) for p in legacy.rglob('*') if p.is_file()}==legacy_hash,'legacy source must remain byte-for-byte read only'
with server(new,legacy_dest,'legacy-imported-v10') as rpc:
 assert len(rpc.call('message.list',{'channelId':'local-aaaa/lobby'}))==3
 duplicate=subprocess.run([str(new),'--data-dir',str(legacy_dest),'import','--from',str(legacy)],env=env,capture_output=True,timeout=20);assert duplicate.returncode!=0,'live lock rejects importing over active authority'
 registry=legacy_dest/'exec-registry.json';assert not registry.exists() or json.loads(registry.read_text())=={},'import/reopen must not automatically start a model'
(base/'result.json').write_text(json.dumps({'preUpgradeHash':pre,'backup':str(backups[0]),'document':doc,'request':request,'source':source,'plan':plan['id']},indent=2))
print('PASS: actual v9 binary creation/backup failure unchanged/v10 backup+upgrade/reopen persisted docs+folders+immutable witness/old binary refusal/real backup+restore+dbId+lock/read-only legacy import; synthetic only')
