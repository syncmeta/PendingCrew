"""Real normal schema12->13 CLI chain; isolated data, disabled providers only."""
import contextlib, hashlib, json, pathlib, shutil, socket, sqlite3, subprocess, sys, time, uuid
base=pathlib.Path(sys.argv[1]).resolve();old,new=map(pathlib.Path,sys.argv[2:4])
for name in ["home","work","data"]:(base/name).mkdir()
env={"PATH":"/usr/bin:/bin:/usr/sbin:/sbin","HOME":str(base/"home"),"SHELL":"/usr/bin/false"}
def args(program,data):return [str(program),"--data-dir",str(data),"serve","--codex","/usr/bin/false","--claude","/usr/bin/false","--codex-env","HOME="+str(base/"home"),"--claude-env","HOME="+str(base/"home")]
def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def read_db(path):
 wal=pathlib.Path(str(path)+"-wal")
 return sqlite3.connect(path.as_uri()+"?mode=ro"+("" if wal.exists() and wal.stat().st_size>32 else "&immutable=1"),uri=True)
def version(data):
 with read_db(data/"crewd.db") as conn:return conn.execute("PRAGMA user_version").fetchone()[0]
def facts(data):
 with read_db(data/"crewd.db") as conn:
  result={}
  for (name,) in conn.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"):
   assert all(c.isalnum() or c=="_" for c in name)
   result[name]=sorted(repr(row) for row in conn.execute('SELECT * FROM "'+name+'"'))
  return result
class RPC:
 def __init__(self,data):
  self.sock=socket.socket(socket.AF_UNIX);self.sock.settimeout(10);self.sock.connect(str(data/"crewd.sock"));self.file=self.sock.makefile("rb");self.seq=0
 def call(self,method,params):
  self.seq+=1;self.sock.sendall((json.dumps({"id":self.seq,"method":method,"params":params})+"\n").encode())
  while True:
   line=self.file.readline();assert line,"actual daemon closed before response";value=json.loads(line)
   if value.get("id")!=self.seq:continue
   assert "error" not in value,(method,value.get("error"))
   return value["result"]
 def close(self):self.file.close();self.sock.close()
@contextlib.contextmanager
def server(program,data,name):
 with (base/(name+".log")).open("w") as log:
  process=subprocess.Popen(args(program,data),env=env,stdout=log,stderr=log)
  try:
   for _ in range(200):
    assert process.poll() is None,"actual daemon exited: "+name
    try:rpc=RPC(data);break
    except OSError:time.sleep(.05)
   else:raise AssertionError("socket not ready")
   try:yield rpc
   finally:rpc.close()
  finally:
   if process.poll() is None:process.terminate()
   process.wait(timeout=20);assert process.returncode==0,(name,process.returncode)
with server(old,base/"data","actual-normal12") as rpc:
 crew=rpc.call("crew.create",{"title":"real schema12 preservation","workingDirectory":str(base/"work")})["id"]
 source=rpc.call("todo.create",{"crewId":crew,"requestId":str(uuid.uuid4()),"text":"pre13 source unchanged"})["requirementId"]
 plan=rpc.call("work.create",{"crewId":crew,"title":"pre13 real plan","sources":[source]})
 original={"crewId":crew,"requestId":str(uuid.uuid4()),"title":"immutable doc request","body":"original 🌙 body"}
 made=rpc.call("document.create",original);doc=made["document"]["id"]
 changed=rpc.call("document.update",{"id":doc,"expectedRevision":made["document"]["revision"],"title":"edited before schema13","body":"current exact 🌙 body"})
 tree=rpc.call("viewFolder.list",{"crewId":crew,"view":"documents"})
 tree=rpc.call("viewFolder.create",{"crewId":crew,"view":"documents","requestId":str(uuid.uuid4()),"expectedTreeRevision":tree["treeRevision"],"name":"pre13 category"})
 tree=rpc.call("viewFolder.assign",{"crewId":crew,"view":"documents","expectedTreeRevision":tree["treeRevision"],"object":{"kind":"document","id":doc},"folderId":tree["folders"][0]["id"]})
assert version(base/"data")==12,"old normal executable must produce real12; never PRAGMA downgrade"
pre_facts=facts(base/"data");pre_hash=digest(base/"data/crewd.db")
# Real filesystem protection failure; no migration attempt is permitted after backup failure.
shutil.copytree(base/"data",base/"backup-failure")
obstacle=base/"backup-failure/backups"
if obstacle.exists():shutil.rmtree(obstacle)
obstacle.write_text("intentional backup filesystem obstacle")
failed=subprocess.run(args(new,base/"backup-failure"),env=env,capture_output=True,timeout=20)
(base/"backup-failure.log").write_bytes(failed.stdout+failed.stderr)
assert failed.returncode and version(base/"backup-failure")==12 and facts(base/"backup-failure")==pre_facts
assert b"backups" in failed.stderr or "备份" in failed.stderr.decode(errors="replace"),"failed before the intended backup-protection seam"
assert digest(base/"backup-failure/crewd.db")==pre_hash
# A collision at the official V13 INDEX follows its preceding CREATE TABLE inside
# the real owning transaction; no fake IF NOT EXISTS or schema downgrade.
shutil.copytree(base/"data",base/"mid-ddl-failure")
collision=base/"mid-ddl-failure"
# A copied coherent root must first acquire its actual inode identity through
# the old production opener; never bypass the retained-WAL guard.
with server(old,collision,"claim-copy-before-collision") as rpc:
 assert rpc.call("todo.get",{"id":source})["text"]=="pre13 source unchanged"
conn=sqlite3.connect(str(collision/"crewd.db"))
conn.execute("CREATE TABLE qa_followup_index_barrier(witness TEXT)");conn.execute("CREATE INDEX work_followup_wake ON qa_followup_index_barrier(witness)");conn.execute("INSERT INTO qa_followup_index_barrier VALUES('retain collision witness')")
conn.commit();conn.execute("PRAGMA wal_checkpoint(TRUNCATE)");conn.close()
before=facts(collision)
failed=subprocess.run(args(new,collision),env=env,capture_output=True,timeout=20)
(base/"mid-ddl-failure.log").write_bytes(failed.stdout+failed.stderr)
assert failed.returncode and version(collision)==12 and facts(collision)==before
assert b"work_followup_wake" in failed.stderr and b"already exists" in failed.stderr,"did not reach the intended second migration DDL"
assert "work_followup_facts" not in facts(collision),"new work_followup_facts TABLE escaped INDEX rollback"
# Preserve failed original and backup before explicitly removing only the fixture obstacle.
retry_root=base/"retry-after-collision";shutil.copytree(collision,retry_root)
with server(old,retry_root,"claim-retry-copy") as rpc:
 assert rpc.call("todo.get",{"id":source})["text"]=="pre13 source unchanged"
conn=sqlite3.connect(str(retry_root/"crewd.db"))
conn.execute("DROP TABLE qa_followup_index_barrier");conn.commit();conn.execute("PRAGMA wal_checkpoint(TRUNCATE)");conn.close()
with server(new,retry_root,"retry-after-mid-ddl") as rpc:assert rpc.call("todo.get",{"id":source})["text"]=="pre13 source unchanged"
assert version(retry_root)==13 and version(collision)==12 and facts(collision)==before
with server(new,base/"data","protected-upgrade13") as rpc:
 assert rpc.call("todo.get",{"id":source})["text"]=="pre13 source unchanged"
 assert rpc.call("work.get",{"id":plan["id"]})["sources"]==[source]
 assert rpc.call("document.get",{"id":doc})["body"]=="current exact 🌙 body"
 assert rpc.call("viewFolder.list",{"crewId":crew,"view":"documents"})==tree
 reused=rpc.call("document.create",original)
 assert reused["reused"] and reused["document"]["id"]==doc and reused["document"]["revision"]==changed["revision"]
 assert reused["document"]["title"]=="edited before schema13"
 status=rpc.call("remote.status",{});assert not status["listening"] and not status["configuration"]["enabled"] and status.get("identity") is None
 assert rpc.call("remote.grants",{})==[]
assert version(base/"data")==13 and not (base/"data/remote/identity.json").exists()
backups=list((base/"data/backups").glob("crewd-v12-pre-v13-*.db"));assert len(backups)==1
with read_db(backups[0]) as conn:assert conn.execute("PRAGMA user_version").fetchone()[0]==12 and conn.execute("PRAGMA integrity_check").fetchone()[0]=="ok"
backup_copy=base/"backup-facts";backup_copy.mkdir();shutil.copy2(backups[0],backup_copy/"crewd.db")
assert facts(backup_copy)==pre_facts,"protected backup did not preserve pre13 authority"
before=digest(base/"data/crewd.db")
refused=subprocess.run(args(old,base/"data"),env=env,capture_output=True,timeout=20)
(base/"old-normal12-refuses13.log").write_bytes(refused.stdout+refused.stderr)
assert refused.returncode and version(base/"data")==13 and digest(base/"data/crewd.db")==before
with server(new,base/"data","reopen13") as rpc:
 assert rpc.call("document.get",{"id":doc})["body"]=="current exact 🌙 body"
 assert rpc.call("viewFolder.list",{"crewId":crew,"view":"documents"})==tree
 assert rpc.call("document.create",original)["reused"]
(base/"migration-result.json").write_text(json.dumps({"oldVersion":12,"newVersion":13,"source":source,"plan":plan["id"],"document":doc,"protectedBackup":str(backups[0]),"preUpgradeSHA256":pre_hash},indent=2))
print("PASS actual followup normal12→13 protected migration, unchanged facts/witness, failed backup and mid-DDL rollback/retry, old-normal refusal and reopen; providers disabled")
