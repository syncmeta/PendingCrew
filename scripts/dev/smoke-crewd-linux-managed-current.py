#!/usr/bin/env python3
"""One current-version private managed-prefix lifecycle; no upgrade, service or model."""
import argparse,contextlib,fcntl,hashlib,importlib.util,json,os,pathlib,shutil,signal,socket,stat,subprocess,sys,time,uuid
sys.dont_write_bytecode=True
COMPILED_REF='c8d70482e42b0e9f777df11f9db720a910a91115'
INPUT_DIGEST='673cd2bff312b747ca71bff29aa0d3abe2ba04872ed68108f8966827f4f99d50'
ELF_SHA='72fdab4aeaf1ad256ea488d4aece836f0e2c220de417d49a4113bf75ff597f1a'
CANDIDATES={'c8':(COMPILED_REF,INPUT_DIGEST,ELF_SHA),'r2':('5821310a7a9e99daba1a784b8f7b4af0c2af5e45','6d16f90874edd4a577f27748ef9647148591fe72380412badadd6cf964e7e708','f0b5b7ac483c8fec80fe0455782f110b2b01999e22908f76e6db156818b4cffd')}
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def require(value,message):
 if not value:raise ValueError(message)
def main():
 a=argparse.ArgumentParser();a.add_argument('package');a.add_argument('output');a.add_argument('--development-macos',action='store_true');a.add_argument('--candidate-family',choices=sorted(CANDIDATES),default='c8');args=a.parse_args()
 compiled_ref,input_digest,elf_sha=CANDIDATES[args.candidate_family]
 dev=args.development_macos;package=pathlib.Path(args.package).resolve();out=pathlib.Path(args.output).resolve();scripts=pathlib.Path(__file__).resolve().parent
 require(sys.platform==('darwin' if dev else 'linux') and os.geteuid()!=0 and os.getuid()==os.geteuid(),'actual nonroot target host required')
 require(not out.exists(),'new private evidence root required');out.mkdir(mode=0o700)
 controller_files=['smoke-crewd-linux-managed-current.py','install-crewd-linux-candidate.py','crewd_managed_launch.py','check-crewd-linux-candidate.py','write-codex-first-provenance.py','archive-crewd-linux-lifecycle.py'];recipe={name:sha(scripts/name) for name in controller_files}
 manifest=json.loads((package/'candidate.json').read_text());w=json.loads((package/'normal-build-witness.json').read_text())
 require(w['rustInputDigest']==input_digest and len(w['files'])==443,'exact current Rust family required')
 require(w['sourceRef']==manifest['sourceRef'] and sha(package/'bin/crewd')==manifest['binarySHA256']==w['binarySHA256'] and sha(package/'normal-build-witness.json')==manifest['witnessSHA256'],'original compiled source/binary/proof pairing required')
 if not dev:require(manifest['sourceRef']==compiled_ref and manifest['binarySHA256']==elf_sha,'only the selected exact accepted Linux package is supported')
 (out/'source-action.json').write_text(json.dumps({'compiledRef':manifest['sourceRef'],'binarySHA256':manifest['binarySHA256'],'rustInputDigest':input_digest,'candidateFamily':args.candidate_family,'developmentMacOnly':dev,'controllerFiles':recipe},indent=2)+'\n')
 installer=scripts/'install-crewd-linux-candidate.py';installer_command=[sys.executable,str(installer)]
 if dev:
  # Private development consumer only: retain real Mac proof and real flock/CAS;
  # substitute only Linux host/ELF/proc observations, never product ACL or Store.
  adapter=out/'mac-development-installer.py'
  adapter.write_text('''import sys,pathlib,importlib.util,json,os,subprocess
sys.dont_write_bytecode=True
scripts=pathlib.Path(sys.argv.pop(1));spec=importlib.util.spec_from_file_location('installer',scripts/'install-crewd-linux-candidate.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
spec=importlib.util.spec_from_file_location('proof',scripts/'write-codex-first-provenance.py');p=importlib.util.module_from_spec(spec);spec.loader.exec_module(p)
def verify(package):
 c=json.loads((package/'candidate.json').read_text());w=json.loads((package/'normal-build-witness.json').read_text())
 assert c['sourceRef']==w['sourceRef'] and m.digest(package) and c['binarySHA256']==m.hashlib.sha256((package/'bin/crewd').read_bytes()).hexdigest() and c['witnessSHA256']==m.hashlib.sha256((package/'normal-build-witness.json').read_bytes()).hexdigest()
 assert w['rustInputDigest']=='673cd2bff312b747ca71bff29aa0d3abe2ba04872ed68108f8966827f4f99d50' and len(w['files'])==443
 p.verify_witness(package/'normal-build-witness.json',package/'bin/crewd',w['rustInputDigest'],{'target':'aarch64-apple-darwin','profile':'debug','defaultFeatures':True,'allFeatures':False,'features':[]},require_fresh=True)
 assert not any(w['buildConfiguration']['workspaceFeatures'].values()) and w['buildConfiguration']['artifactFeatures']==[]
 return c,m.digest(package)
def inactive(prefix):
 for line in subprocess.check_output(['ps','-axo','uid=,pid=,comm='],text=True).splitlines():
  fields=line.strip().split(None,2)
  if len(fields)==3 and int(fields[0])==os.geteuid() and fields[2].startswith(str(prefix/'releases')+'/'):raise ValueError('private Mac prefix runtime still running')
m.verify=verify;m.inactive=inactive;m.platform.system=lambda:'Linux'
try:m.main()
except (ValueError,OSError,KeyError,subprocess.SubprocessError) as e:raise SystemExit('REFUSED/UNKNOWN: '+str(e))
''');adapter.write_text(adapter.read_text().replace(repr(INPUT_DIGEST),repr(input_digest)));installer_command=[sys.executable,str(adapter),str(scripts)]
 prefix=out/'prefix';data=out/'data';home=out/'home';work=out/'work'
 for p in [data,home,work]:p.mkdir(mode=0o700)
 require(len(str(data/'crewd.sock').encode())<=103,'short private path required')
 env={'PATH':'/usr/bin:/bin','HOME':str(home),'XDG_CACHE_HOME':str(home/'cache'),'SHELL':'/usr/bin/false','PYTHONDONTWRITEBYTECODE':'1'};records=[];terminals=[]
 def run(label,argv,success=True):
  r=subprocess.run(argv,env=env,capture_output=True,text=True,timeout=60);(out/(label+'.log')).write_text(r.stdout+r.stderr);records.append({'phase':label,'argv':argv,'exitCode':r.returncode});require((r.returncode==0)==success,'command outcome differs: '+label);return r
 def install(label,rid,expected,success=True):
  r=run(label,installer_command+['install','--prefix',str(prefix),'--package',str(package),'--request-id',rid,'--expected-current',expected or 'none'],success);return json.loads(r.stdout.splitlines()[-1]) if success else None
 def status(rid):return json.loads(run('status-'+rid,installer_command+['status','--prefix',str(prefix),'--request-id',rid]).stdout)
 def physical():return {str(p.relative_to(data)):{'sha256':sha(p),'size':p.stat().st_size,'mode':p.stat().st_mode&0o777} for p in data.rglob('*') if p.is_file()}
 def prefix_metadata():return {str(p.relative_to(prefix)):sha(p) for p in (prefix/'activations').rglob('transaction.json')}
 def locked(path):
  fd=os.open(path,os.O_RDWR|os.O_CLOEXEC|os.O_NOFOLLOW)
  try:
   try:fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
   except BlockingIOError:return
   raise ValueError('expected live inherited lease missing: '+str(path))
  finally:os.close(fd)
 @contextlib.contextmanager
 def serve(label,crash=False):
  log=(out/(label+'.log')).open('w');cmd=[str(prefix/'bin/crewd'),'--data-dir',str(data),'serve','--codex','/usr/bin/false','--claude','/usr/bin/false','--codex-env','HOME='+str(home),'--claude-env','HOME='+str(home)];child=subprocess.Popen(cmd,env=env,stdout=log,stderr=log);s=None
  try:
   for _ in range(200):
    require(child.poll() is None,'managed daemon exited before readiness: '+label);s=socket.socket(socket.AF_UNIX);s.settimeout(10)
    try:s.connect(str(data/'crewd.sock'));break
    except OSError:s.close();s=None;time.sleep(.05)
   require(s is not None,'managed readiness timeout');locked(prefix/'.runtime.lock');locked(prefix/'.data-leases'/hashlib.sha256(str(data).encode()).hexdigest())
   if not dev:
    inode=(prefix/'.runtime.lock').stat().st_ino;require(any(p.stat().st_ino==inode for p in pathlib.Path('/proc',str(child.pid),'fd').iterdir()),'actual Linux daemon did not inherit runtime lease')
   f=s.makefile('rb');sequence=0
   def call(method,params):
    nonlocal sequence
    sequence+=1;s.sendall((json.dumps({'id':sequence,'method':method,'params':params})+'\n').encode())
    while True:
     line=f.readline();require(bool(line),'unknown RPC outcome; never resend');value=json.loads(line)
     if value.get('id')==sequence:
      with (out/'rpc.jsonl').open('a') as trace:trace.write(json.dumps({'phase':label,'method':method,'params':params,'response':value})+'\n')
      require('error' not in value,'RPC refusal: '+json.dumps(value.get('error')));return value['result']
   try:yield call
   finally:f.close();s.close();s=None
  finally:
   if s:s.close()
   if child.poll() is None:child.send_signal(signal.SIGKILL if crash else signal.SIGTERM)
   try:child.wait(timeout=20)
   except subprocess.TimeoutExpired:child.kill();child.wait();raise ValueError('owned managed child exceeded stop bound')
   log.close();terminals.append({'phase':label,'pid':child.pid,'waitExit':child.returncode});(out/'terminals.json').write_text(json.dumps(terminals,indent=2)+'\n');require(child.returncode==(-signal.SIGKILL if crash else 0),'managed termination result differs')
 first=str(uuid.uuid4());active=install('fresh-install',first,None)['currentActivation'];original=str(uuid.uuid4());write_id=str(uuid.uuid4())
 with serve('first-normal') as call:
  remote=call('remote.status',{});require(not remote['listening'] and not remote['configuration']['enabled'],'remote enabled')
  crew=call('crew.create',{'title':'Current managed private work','workingDirectory':str(work)})['id'];params={'crewId':crew,'requestId':original,'text':'managed same-version original UUID'};requirement=call('todo.create',params)['requirementId']
  context=call('data.workspace.get',{'crewId':crew})['context'];created=call('data.submit',{'requestId':write_id,'context':context,'change':{'kind':'apply','message':'managed durable knowledge and view','edits':[{'kind':'createBlock','alias':'shared','contentType':'text/plain','content':'before crash','sources':[],'attributes':{}},{'kind':'createView','alias':'document','title':'Managed document','definition':{'kind':'document','placements':[{'kind':'set','id':None,'alias':'position','block':{'kind':'created','alias':'shared'},'atRevision':None,'layout':{}}],'layout':{}},'sources':[],'attributes':{}}]}});initial_context=context;context=created['context'];block=created['created']['shared']['object']['reference'];view=created['created']['document']['object']['reference']
  require(call('session.list',{'crewId':crew})==[],'unexpected automatic agent session')
  before=physical();install('live-prefix-lease-refusal',str(uuid.uuid4()),active,False);require(physical()==before,'live activation refusal changed data')
  run('second-data-lease-refusal',[str(prefix/'bin/crewd'),'--data-dir',str(data),'check'],False)
 # A genuine durable preparation failure leaves its original UUID Unknown.
 before=physical();prepared=str(uuid.uuid4());(prefix/('.current-'+prepared)).mkdir();install('prepared-pointer-failure',prepared,active,False);require(status(prepared)['outcome']=='prepared-unknown' and status(first)['currentActivation']==active,'prepared Unknown was not retained');require(physical()==before,'failed activation changed data')
 dropped=str(uuid.uuid4());install('caller-dropped-receipt',dropped,active);active=status(dropped)['currentActivation'];require(status(dropped)['outcome']=='committed-current','metadata-only status failed');require(status(prepared)['outcome']=='prepared-unknown','unrelated commit changed Unknown')
 before=physical();count=prefix_metadata();install('stale-CAS-refusal',str(uuid.uuid4()),'activations/'+str(uuid.uuid4()),False);require(physical()==before and prefix_metadata()==count,'stale CAS changed data or receipts')
 children=[]
 for _ in range(2):
  rid=str(uuid.uuid4());cmd=installer_command+['install','--prefix',str(prefix),'--package',str(package),'--request-id',rid,'--expected-current',active];children.append((rid,subprocess.Popen(cmd,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)))
 outcomes=[]
 for i,(rid,p) in enumerate(children):
  stdout,stderr=p.communicate(timeout=60);(out/('concurrent-'+str(i)+'.log')).write_text(stdout+stderr);outcomes.append({'requestId':rid,'exitCode':p.returncode})
 require(sum(x['exitCode']==0 for x in outcomes)==1,'concurrent activation CAS needs one winner');winner=next(x['requestId'] for x in outcomes if x['exitCode']==0);active=status(winner)['currentActivation'];require(physical()==before,'CAS writers changed data');(out/'concurrent-results.json').write_text(json.dumps(outcomes,indent=2)+'\n')
 crash_id=str(uuid.uuid4());crash_params={'crewId':crew,'requestId':crash_id,'text':'committed immediately before owned SIGKILL'};knowledge_id=str(uuid.uuid4())
 with serve('actual-SIGKILL',True) as call:
  require(call('todo.create',params)['requirementId']==requirement,'normal reopen lost original UUID');require(call('session.list',{'crewId':crew})==[],'reopen auto-started tasks')
  crash_requirement=call('todo.create',crash_params)['requirementId']
  updated=call('data.submit',{'requestId':knowledge_id,'context':context,'change':{'kind':'apply','message':'committed knowledge before actual SIGKILL','edits':[{'kind':'updateBlock','id':block['id'],'expectedRevision':'1','contentType':'text/plain','content':'committed before crash','sources':None,'attributes':{}}]}});crash_context=updated['context']
 wal=data/'crewd.db-wal';require(wal.is_file() and wal.stat().st_size>32,'actual SIGKILL did not leave nonempty Core WAL');crash_files=physical();(out/'crash-files.json').write_text(json.dumps(crash_files,indent=2)+'\n')
 for command in ['backup','check']:run('pending-WAL-'+command+'-refused',[str(prefix/'bin/crewd'),'--data-dir',str(data),command],False)
 require(physical()==crash_files,'pending-WAL offline readers changed source files')
 for fault in ['bad-wal','future-main']:
  target=out/fault;shutil.copytree(data,target,ignore=lambda parent,names:[name for name in names if stat.S_ISSOCK((pathlib.Path(parent)/name).lstat().st_mode)])
  changed=target/('crewd.db-wal' if fault=='bad-wal' else 'crewd.db');raw=bytearray(changed.read_bytes())
  if fault=='bad-wal':raw[0]^=255
  else:raw[60:64]=(16).to_bytes(4,'big')
  changed.write_bytes(raw)
  def fault_files():return {str(p.relative_to(target)):{'sha256':sha(p),'mode':p.stat().st_mode&0o777} for p in target.rglob('*') if p.is_file()}
  before_fault=fault_files();run(fault+'-preserved-refusal',[str(prefix/'bin/crewd'),'--data-dir',str(target),'serve','--codex','/usr/bin/false','--claude','/usr/bin/false'],False);require(fault_files()==before_fault and not (target/'crewd.sock').exists(),'rejected fault changed original files')
  (out/(fault+'-unchanged.json')).write_text(json.dumps(before_fault,indent=2)+'\n')
 with serve('same-version-recovery') as call:
  require(call('todo.create',params)['requirementId']==requirement,'recovery lost original request identity');require(call('todo.create',crash_params)['requirementId']==crash_requirement,'crash committed request identity lost')
  require(call('data.workspace.get',{'crewId':crew})['context']==crash_context,'knowledge current context lost')
  recovered=call('data.object.get',{'context':crash_context,'reference':block,'atRevision':None})['object'];doc=call('data.object.get',{'context':crash_context,'reference':view,'atRevision':None})['object'];require(recovered['content']=='committed before crash' and doc['title']=='Managed document' and doc['definition']['placements'][0]['block']['reference']==block,'knowledge/view or placement lost')
  for rid,origin in [(write_id,initial_context),(knowledge_id,context)]:require(call('data.request.get',{'requestId':rid,'context':origin})['lookup']['status']=='known','current same-version request history lost')
  require(call('session.list',{'crewId':crew})==[],'recovery auto-started tasks')
 with serve('final-normal-reopen') as call:
  require(call('todo.create',params)['requirementId']==requirement and call('data.workspace.get',{'crewId':crew})['context']==crash_context,'final normal reopen lost state');require(call('session.list',{'crewId':crew})==[],'final reopen auto-started tasks')
  require(call('todo.create',crash_params)['requirementId']==crash_requirement,'final crash UUID lost')
  require(call('data.object.get',{'context':crash_context,'reference':block,'atRevision':None})['object']==recovered and call('data.object.get',{'context':crash_context,'reference':view,'atRevision':None})['object']==doc,'final normal reopen changed knowledge/view semantics')
 require(status(prepared)['outcome']=='prepared-unknown','Unknown was replayed or erased');require(recipe=={name:sha(scripts/name) for name in controller_files},'controller source changed during flow');require(sha(package/'bin/crewd')==manifest['binarySHA256'],'original binary changed')
 result={'passed':True,'compiledRef':manifest['sourceRef'],'binarySHA256':manifest['binarySHA256'],'controllerFiles':recipe,'developmentMacOnly':dev,'LinuxProcFdChecked':not dev,'sameVersionOnly':True,'ordinaryUID':os.geteuid(),'prefix':str(prefix),'liveRuntimeAndDataLeaseVerified':True,'preparedUnknownStatusOnlyNoReplay':prepared,'concurrentSingleCASWinner':winner,'realSIGKILLWithNonemptyWAL':True,'badWALAndFutureMainNoTouchRefused':True,'crashRequestId':crash_id,'crashRequirementId':crash_requirement,'knowledgeAndViewOriginalUUIDsRecovered':True,'zeroAgentSessionsThroughout':True,'originalRequestId':original,'knowledgeWriteIds':[write_id,knowledge_id],'terminals':terminals};(out/'result.json').write_text(json.dumps(result,indent=2)+'\n');print('PASS current managed prefix/install/lease/CAS/Unknown/crash/knowledge/reopen; '+('Mac development only' if dev else 'actual Linux'))
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,KeyError,AssertionError,subprocess.SubprocessError) as e:raise SystemExit('FAIL retained current managed lifecycle: '+str(e))
