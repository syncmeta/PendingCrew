#!/usr/bin/env python3
"""Exercise a packaged candidate in a fresh private workspace; no model or LAN."""
import argparse,contextlib,hashlib,json,os,pathlib,platform,signal,socket,subprocess,time,uuid

def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def main():
 a=argparse.ArgumentParser();a.add_argument('package');a.add_argument('output');a.add_argument('--development-macos',action='store_true',help='fixture development only against an explicitly retained Mac binary; never Linux qualification');args=a.parse_args()
 development=args.development_macos
 if platform.system()!=('Darwin' if development else 'Linux'):raise ValueError('actual target platform required')
 package=pathlib.Path(args.package).resolve();out=pathlib.Path(args.output).resolve()
 if out.exists():raise ValueError('fresh retained runtime evidence directory required')
 out.mkdir();program=package/'bin/crewd';expected=sha(program);manifest=json.loads((package/'candidate.json').read_text())
 if expected!=manifest['binarySHA256']:raise ValueError('explicit package binary identity mismatch')
 for d in ['home','data','work']:(out/d).mkdir(mode=0o700)
 env={'PATH':'/usr/bin:/bin','HOME':str(out/'home'),'XDG_CACHE_HOME':str(out/'home/cache'),'SHELL':'/usr/bin/false'}
 trace=out/'rpc.jsonl';shutdowns=[]
 def command(*argv,refusal=False):
  run=subprocess.run([str(program),'--data-dir',str(out/'data'),*argv],env=env,capture_output=True,timeout=30)
  (out/(argv[0]+'-'+str(time.time_ns())+'.log')).write_bytes(run.stdout+run.stderr)
  if refusal:
   if run.returncode!=1 or run.stderr.strip()!=b'crewd: Core-only restore cannot replace a knowledge workspace; restore a knowledge snapshot under current ACL':raise ValueError('expected explicit Core-only knowledge restore refusal')
  elif run.returncode:raise ValueError('packaged CLI failed: '+argv[0])
  return run
 @contextlib.contextmanager
 def serve(label):
  argv=[str(program),'--data-dir',str(out/'data'),'serve','--codex','/usr/bin/false','--claude','/usr/bin/false','--codex-env','HOME='+str(out/'home'),'--claude-env','HOME='+str(out/'home')]
  with (out/(label+'.log')).open('w') as log:
   process=subprocess.Popen(argv,env=env,stdout=log,stderr=log);s=None
   try:
    for _ in range(200):
     if process.poll() is not None:raise ValueError('packaged daemon exited before readiness')
     s=socket.socket(socket.AF_UNIX);s.settimeout(10)
     try:s.connect(str(out/'data/crewd.sock'));break
     except OSError:s.close();s=None;time.sleep(.05)
    if s is None:raise ValueError('packaged daemon socket readiness timed out')
    f=s.makefile('rb');sequence=0
    def call(method,params,refusal=False):
     nonlocal sequence
     sequence+=1;s.sendall((json.dumps({'id':sequence,'method':method,'params':params})+'\n').encode())
     while True:
      line=f.readline()
      if not line:raise ValueError('unexpected result loss; no retry permitted')
      value=json.loads(line)
      if value.get('id')==sequence:
       with trace.open('a') as t:t.write(json.dumps({'phase':label,'method':method,'params':params,'response':value})+'\n')
       if refusal:
        if 'error' not in value:raise ValueError('expected dangerous operation refusal')
        reason='destructive SQL requires data.preview and real Human data.confirm' if refusal is True else refusal
        if value['error'].get('code')!=-32009 or reason not in value['error'].get('message',''):raise ValueError('unexpected RPC refusal type')
        return value['error']
       if 'error' in value:raise ValueError('RPC failed '+method+': '+json.dumps(value['error']))
       return value['result']
    try:yield call
    finally:f.close();s.close();s=None
   finally:
    if s:s.close()
    if process.poll() is None:process.send_signal(signal.SIGTERM)
    try:process.wait(timeout=20)
    except subprocess.TimeoutExpired:
     process.kill();process.wait();raise ValueError('packaged shutdown exceeded deadline')
    shutdowns.append({'phase':label,'pid':process.pid,'exitCode':process.returncode})
    (out/'shutdowns.json').write_text(json.dumps(shutdowns,indent=2)+'\n')
    if process.returncode!=0:raise ValueError('packaged daemon shutdown failure')
 def query(call,context,sql):return call('data.query',{'context':context,'statement':{'sql':sql,'params':[]},'atRevision':None,'cursor':None,'limit':10})
 def objects(call,context):
  block_value=call('data.object.get',{'context':context,'reference':block,'atRevision':None})
  view_value=call('data.object.get',{'context':context,'reference':view,'atRevision':None})
  if block_value['object']['content']!='durable shared block' or view_value['object']['definition']['kind']!='document':raise ValueError('knowledge block/view lost')
  return {'block':block_value['object'],'view':view_value['object']}
 request=str(uuid.uuid4());submit_id=str(uuid.uuid4());confirm_id=str(uuid.uuid4());restore_id=str(uuid.uuid4())
 with serve('first-open') as call:
  remote=call('remote.status',{})
  if remote['listening'] or remote['configuration']['enabled']:raise ValueError('remote unexpectedly enabled')
  crew=call('crew.create',{'title':'Linux distribution synthetic work','workingDirectory':str(out/'work')})['id']
  params={'crewId':crew,'requestId':request,'text':'packaged release persists across knowledge recovery'};made=call('todo.create',params)
  initial=call('data.workspace.get',{'crewId':crew})['context']
  edits=[{'kind':'sql','statements':[{'sql':'CREATE TABLE items(id INTEGER PRIMARY KEY,value TEXT)','params':[]},{'sql':"INSERT INTO items VALUES(1,'one'),(2,'two')",'params':[]}]},
   {'kind':'createBlock','alias':'shared','contentType':'text/plain','content':'durable shared block','sources':[],'attributes':{}},
   {'kind':'createView','alias':'document','title':'Durable shared document','definition':{'kind':'document','placements':[{'kind':'set','id':None,'alias':'position','block':{'kind':'created','alias':'shared'},'atRevision':None,'layout':{}}],'layout':{}},'sources':[],'attributes':{}}]
  committed=call('data.submit',{'requestId':submit_id,'context':initial,'change':{'kind':'apply','message':'packaged knowledge durability','edits':edits}})
  if committed['phase']!='committed':raise ValueError('knowledge submit not committed')
  context=committed['context'];block=committed['created']['shared']['object']['reference'];view=committed['created']['document']['object']['reference'];before_objects=objects(call,context)
  original_rows=query(call,context,'SELECT id,value FROM items ORDER BY id')['rows']
  if len(original_rows)!=2:raise ValueError('business rows missing')
  call('data.submit',{'requestId':str(uuid.uuid4()),'context':context,'change':{'kind':'apply','message':'unconfirmed destructive SQL must fail','edits':[{'kind':'sql','statements':[{'sql':'DROP TABLE items','params':[]}]}]}},refusal=True)
  if call('data.workspace.get',{'crewId':crew})['context']!=context:raise ValueError('refused SQL changed context')
  preview=call('data.preview',{'context':context,'statements':[{'sql':'DROP TABLE items','params':[]}]})
  if preview['affectedRows']!=2 or not preview['schemaChanged']:raise ValueError('dangerous preview facts missing')
  if query(call,context,'SELECT id,value FROM items ORDER BY id')['rows']!=original_rows:raise ValueError('preview mutated data')
  confirmed=call('data.confirm',{'requestId':confirm_id,'context':context,'token':preview['token']});before_restore=confirmed['context'];snapshot=confirmed['snapshotId']
  if query(call,before_restore,"SELECT name FROM sqlite_master WHERE type='table' AND name='items'")['rows']:raise ValueError('confirmed DROP did not execute')
  placement=before_objects['view']['definition']['placements'][0]['id']
  changed=call('data.submit',{'requestId':str(uuid.uuid4()),'context':before_restore,'change':{'kind':'apply','message':'change content and view after recovery snapshot','edits':[
   {'kind':'updateBlock','id':block['id'],'expectedRevision':before_objects['block']['revision'],'contentType':'text/plain','content':'changed after snapshot','sources':None,'attributes':{}},
   {'kind':'updateView','id':view['id'],'expectedRevision':before_objects['view']['revision'],'title':'Changed after snapshot','definition':{'kind':'document','placements':[{'kind':'keep','id':placement,'layout':{'changed':True}}],'layout':{'changed':True}},'sources':None,'attributes':{}}
  ]}})
  before_restore=changed['context'];changed_objects={}
  for kind,reference in [('block',block),('view',view)]:changed_objects[kind]=call('data.object.get',{'context':before_restore,'reference':reference,'atRevision':None})['object']
  if changed_objects['block']['content']!='changed after snapshot' or changed_objects['view']['title']!='Changed after snapshot' or changed_objects['view']['definition']['layout']!={'changed':True}:raise ValueError('post-snapshot mutations did not execute')
  snapshots=call('data.snapshots',{'context':before_restore})
  if not any(x['id']==snapshot for x in snapshots['items']):raise ValueError('recovery snapshot missing')
  restored=call('data.restore',{'requestId':restore_id,'context':before_restore,'snapshotId':snapshot});context=restored['context']
  if context['restoreEpoch']==before_restore['restoreEpoch']:raise ValueError('restore authority epoch unchanged')
  call('data.query',{'context':before_restore,'statement':{'sql':'SELECT id,value FROM items ORDER BY id','params':[]},'atRevision':None,'cursor':None,'limit':10},refusal='knowledge generation or restore context changed')
  if query(call,context,'SELECT id,value FROM items ORDER BY id')['rows']!=original_rows:raise ValueError('knowledge rows not restored')
  restored_objects=objects(call,context)
  for kind,old in before_objects.items():
   new=restored_objects[kind]
   if int(new['revision'])!=int(changed_objects[kind]['revision'])+1 or new['provenance']['updatedBy']!={'kind':'human'} or new['provenance']['updatedAt']<old['provenance']['updatedAt']:raise ValueError('restore did not advance revision with current Human provenance')
   def semantic(value):
    value=json.loads(json.dumps(value));value.pop('revision');value['provenance'].pop('updatedAt');value['provenance'].pop('updatedBy');return value
   if semantic(new)!=semantic(old):raise ValueError('shared content/view/placement identity changed across restore')
  for identity,original_context in [(submit_id,initial),(restore_id,before_restore)]:
   if call('data.request.get',{'requestId':identity,'context':original_context})['lookup']['status']!='historical':raise ValueError('old request history not retained')
 command('backup');backups=list((out/'data/backups').glob('crewd-manual-*.db'))
 if len(backups)!=1:raise ValueError('actual protective backup missing')
 before={str(p.relative_to(out/'data')):sha(p) for p in (out/'data').rglob('*') if p.is_file()}
 command('restore','--from',str(backups[0]),refusal=True)
 after={str(p.relative_to(out/'data')):sha(p) for p in (out/'data').rglob('*') if p.is_file()}
 if before!=after:raise ValueError('refused Core-only restore changed private data')
 command('check')
 with serve('reopened') as call:
  recovered=call('todo.create',params)
  if recovered['requirementId']!=made['requirementId']:raise ValueError('reopened request identity duplicated')
  current=call('data.workspace.get',{'crewId':crew})['context']
  if current!=context or query(call,current,'SELECT id,value FROM items ORDER BY id')['rows']!=original_rows:raise ValueError('knowledge reopen lost state')
  if objects(call,current)!=restored_objects:raise ValueError('restored object revisions/content lost after reopen')
  for identity,original_context in [(submit_id,initial),(restore_id,before_restore)]:
   if call('data.request.get',{'requestId':identity,'context':original_context})['lookup']['status']!='historical':raise ValueError('reopened old UUID history lost')
  remote=call('remote.status',{})
  if remote['listening'] or remote['configuration']['enabled']:raise ValueError('reopen unexpectedly activated remote')
 if sha(program)!=expected:raise ValueError('packaged executable changed during smoke')
 result={'passed':True,'binarySHA256':expected,'sourceRef':manifest['sourceRef'],'developmentMacFixtureOnly':development,'providerProcesses':'disabled /usr/bin/false','remoteEnabled':False,'freshData':True,'originalRequestId':request,'requirementId':made['requirementId'],'knowledgeSubmitId':submit_id,'knowledgeRestoreId':restore_id,'knowledgeSnapshotId':snapshot,'knowledgeRestoreEpochChanged':True,'knowledgeRowsAndViewReopened':True,'postSnapshotBlockAndViewChangesActuallyReverted':True,'coreRestoreRefusedUnchanged':True,'shutdowns':shutdowns}
 (out/'result.json').write_text(json.dumps(result,indent=2)+'\n');print('PASS packaged runtime/knowledge snapshot restore/guarded Core restore/reopen/original UUID')
try:main()
except (ValueError,OSError,KeyError,subprocess.SubprocessError) as e:raise SystemExit('FAIL: '+str(e))
