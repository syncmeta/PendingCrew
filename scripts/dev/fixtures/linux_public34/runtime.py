"""Owned ordinary-Core controller. Only Root calls these methods."""
import ctypes,json,os,shutil,socket,stat,subprocess,sys,tempfile,threading,time,uuid
from pathlib import Path
from common import birth,same,save,sha,socket_owned,wait_for
from lifecycle import Lifecycle
from transport import JournaledPublic
from cold import ROOT,CREWS,core_copy,initial,closure

def executable(pid):
    return os.readlink('/proc/'+str(pid)+'/exe').removesuffix(' (deleted)')

def children(pid):
    result=[]
    try:tasks=list(Path('/proc',str(pid),'task').iterdir())
    except FileNotFoundError:
        if birth(pid) is None:return []
        raise
    for task in tasks:
        try:result.extend(int(x) for x in (task/'children').read_text().split())
        except FileNotFoundError:continue
    return sorted(set(result))

class Runner(Lifecycle):
    def __init__(self,args,out,label,qualified,inputs):
        self.a=args;self.out=out;self.label=label;self.qualified=qualified;self.input_hashes=inputs.copy()
        self.out.mkdir(mode=0o700);self.owns_output=True
        for n in ['observations','logs','cold']:(out/n).mkdir(mode=0o700)
        self.phase=self.seq=0;self.proc=self.owner=self.human=self.hibernate_request=None
        self.commands=[];self.pending=[];self.streams=[];self.tools=[];self.births=[];self.watch_errors=[]
        self.watch_stop=threading.Event();self.watch_lock=threading.Lock()
        self.watch=None
        self.report={'exit':1,'scope':label,'phases':[],'intendedOrdinaryIndependentCore':True,
                     'actualModel':False,'actualNativeExecutor':False,'productCompiledByRunner':False,
                     'unknownRequestsNeverRetried':True,'noSignalsOrForcedCleanup':True}
        self.runtime=Path(tempfile.mkdtemp(prefix='pcl34-',dir='/tmp'))
        for n in ['data','home','tmp','codex','work']:(self.runtime/n).mkdir(mode=0o700)
        self.data=self.runtime/'data';self.endpoint=self.data/'crewd.sock';assert len(str(self.endpoint).encode())<=103
        self.env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(self.runtime/'home'),
                  'TMPDIR':str(self.runtime/'tmp'),'CODEX_HOME':str(self.runtime/'codex'),'SHELL':'/usr/bin/false'}
        self.policy=Path(__file__).parent/'offline.py'
        self.input_hashes[str(self.policy)]=sha(self.policy)
        self.record('private-runtime',{'path':str(self.runtime),'retained':True,'privateHOME':self.env['HOME'],
            'noCodexArgumentOrDiscovery':True,'ClaudeExplicitlyDisabled':True,'policySHA256':sha(self.policy),
            'policy':'Linux inherited seccomp: socket/socketpair domains restricted to AF_UNIX'})
    def wrap(self,argv):
        return [sys.executable,'-I',str(self.policy),*map(str,argv)]

    def prepare(self):
        self.tool(self.wrap(['/usr/bin/true']),'policy-load',expected=0)
        with socket.socket(socket.AF_INET,socket.SOCK_STREAM) as tcp:
            tcp.bind(('127.0.0.1',0));tcp.listen(4);port=tcp.getsockname()[1]
            listener=socket.socket(socket.AF_UNIX);listener.bind(str(self.endpoint));listener.listen(4);owned=self.endpoint.lstat()
            try:
                self.tool(self.wrap([sys.executable,str(Path(__file__).parent/'network_probe.py'),
                    '--mode','client','--endpoint',str(self.endpoint),'--tcp-port',str(port)]),'actual-local-client-filter')
            finally:
                listener.close();now=self.endpoint.lstat()
                assert (owned.st_dev,owned.st_ino,os.getuid())==(now.st_dev,now.st_ino,now.st_uid)
                self.endpoint.unlink()
            self.tool(self.wrap([sys.executable,str(Path(__file__).parent/'network_probe.py'),
                '--mode','bind','--endpoint',str(self.endpoint),'--tcp-port',str(port)]),'actual-local-bind-filter')
            assert not os.path.lexists(self.endpoint)
        self.record('actual-private-offline-policy-qualified',{'UnixBindConnectAllowed':True,'NonUnixSocketCreationPermissionDenied':True,'externalDialAttempts':0})
        self.watch=threading.Thread(target=self.observe,daemon=True);self.watch.start()
    def native_args(self):return []
    def observe(self):
        while not self.watch_stop.wait(.03):
            try:
                with self.watch_lock:roots=list(self.births)
                for root in roots:
                    if same(root,birth(root['pid'])):
                        found=children(root['pid'])
                        assert not found,('unexpected owned Core child with both executors disabled',root,found)
            except BaseException as error:self.watch_errors.append(type(error).__name__+': '+str(error))
    def tool(self,argv,label,expected=0,contains=None):
        index=len(self.tools);out=self.out/'logs'/f'cli-{index}-{label}.stdout';err=out.with_suffix('.stderr')
        stdout=out.open('xb');stderr=err.open('xb');self.streams.extend([stdout,stderr])
        self.record('owned-tool-before-launch',{'argv':argv,'label':label,'attempts':1})
        proc=subprocess.Popen(argv,cwd=self.runtime/'work',env=self.env,stdin=subprocess.DEVNULL,stdout=stdout,stderr=stderr)
        owned=birth(proc.pid);assert owned and owned['ppid']==os.getpid();entry={'proc':proc,'birth':owned,'argv':argv};self.tools.append(entry)
        self.record('original-owned-tool-birth',{'birth':owned,'argv':argv,'label':label})
        code=proc.wait(timeout=45);stdout.flush();stderr.flush();stdout.close();stderr.close()
        assert not same(owned,birth(proc.pid))
        value={'argv':argv,'originalBirth':owned,'exit':code,'stdout':out.read_text(),'stderr':err.read_text(),'physicalExit':True}
        self.record('owned-tool-terminal',value)
        assert (code==expected if expected is not None else code!=0),value
        if contains:assert contains in value['stderr'],value
        return value
    def cli(self,operation,*args,expected=0,contains=None):
        assert self.proc is None and self.human is None,'backup/restore CLI only while original Core is stopped'
        return self.tool(self.wrap([str(self.a.core),'--data-dir',str(self.data),operation,*map(str,args)]),operation,expected,contains)
    def boot(self,legacy=False):
        assert self.proc is None;self.phase+=1;self.hibernate_request=None
        q=self.qualified['legacy33' if legacy else 'current'];binary=Path(q['binary'])
        assert sha(binary)==q['sha256']
        log=(self.out/'logs'/f'phase-{self.phase}-core.log').open('xb');self.streams.append(log)
        argv=self.wrap([str(getattr(self,'launcher',binary)),'--data-dir',str(self.data),'serve','--disable-claude',*self.native_args()])
        self.record('owned-Core-before-launch',{'argv':argv,'sourceRef':q['sourceRef'],'attempts':1})
        self.proc=subprocess.Popen(argv,cwd=self.runtime/'work',env=self.env,stdin=subprocess.DEVNULL,stdout=log,stderr=log)
        self.owner=birth(self.proc.pid);assert self.owner and self.owner['ppid']==os.getpid() and self.owner['uid']==os.getuid()
        with self.watch_lock:self.births.append(self.owner.copy())
        self.record('original-Core-birth',{'phase':self.phase,'birth':self.owner,'expectedExecutable':str(binary)})
        wait_for(lambda:self.proc.poll() is None and socket_owned(self.endpoint,self.owner),'original owned Core endpoint',seconds=30)
        assert executable(self.proc.pid)==str(binary)
        self.report['actualCoreExecutableQualified']=True
        self.human=JournaledPublic(self.endpoint,self.owner,self.record)
        assert self.human.hello['build']['gitSha']==q['sourceRef'][:12] and self.human.hello['build']['dirty'] is False
        self.record('qualified-original-hello',self.human.hello);self.rpc('lifecycle.claim',{})
        assert not self.rpc('remote.status',{})['listening'];self.quiet()
    def quiet(self):
        assert self.rpc('session.list',{'includeRemoved':True})==[]
        assert self.rpc('ask.list',{})==[]
        work=self.rpc('work.list',{'liveOnly':False,'includeRemoved':True});assert work['items']==[]
        settings=self.rpc('judgment.settings.get',{})['view'];assert settings['settings']['config'] is None
        assert settings['credentialState']=='missing' and settings['pendingRequestId'] is None
    def stop(self):
        self.quiet();super().stop()
        assert not os.path.lexists(self.endpoint) and not self.watch_errors
    def archive_cold(self,label):
        assert self.proc is None and self.human is None and not os.path.lexists(self.endpoint)
        assert all(x['proc'].poll() is not None for x in self.tools)
        assert not Path(str(self.data/'crewd.db')+'-wal').exists()
        super().archive_cold(label)
        contract=self.a.host_contract
        proof=contract.qualify_stopped_public34(self.data)
        self.record('full-stopped-H33-public34-format-authority-qualification',proof)
    def cold(self,version=34):
        assert self.proc is None and self.human is None
        directory=self.out/'cold'/f'phase-{self.phase}'
        result=core_copy(directory,version);self.record('immutable-Core-copy-read',result);return result
    def fresh(self):
        self.boot();self.dag();self.stop()
        self.initial=initial(self.out/'cold'/f'phase-{self.phase}');self.record('original-fresh-A-B-C-and-immutable-receipt',self.initial)
        self.boot();self.dag();objects,context=self.public_pages(self.initial)
        closure(self.data,self.initial['projection'],context,objects,self.initial['publication']['pages'])
        receipt=self.lookup(self.initial['publication']['request'],self.initial['publication']['origin'])
        assert receipt['lookup']['status']=='known' and receipt['lookup']['result']==self.initial['builtinReceipt']
        self.record('public-current-complete-closure',{'context':context,'objects':objects,'originalBuiltinReceipt':receipt})
    def dag(self):
        crews=self.rpc('crew.list',{'includeRemoved':True});assert len(crews)==5
        assert len([x for x in crews if x['management']])==1
        for key,(title,parents) in CREWS.items():
            c=next(c for c in crews if c['id']==key);assert c['title']==title and c['parentIds']==parents and not c.get('removedAt')
            assert c.get('captainSessionId') is None and c.get('workingDirectory') is None
        identity={c['id']:{key:c.get(key) for key in ['id','title','number','management','parentIds','childIds']} for c in crews}
        if hasattr(self,'original_dag'):assert identity==self.original_dag
        else:self.original_dag=identity
        self.record('fixed-original-DAG-and-management',crews)
    def workspace(self):return self.rpc('data.workspace.get',{'crewId':ROOT})
    def object(self,context,kind,key):
        return self.rpc('data.object.get',{'context':context,'reference':{'namespace':context['namespace'],'kind':kind,'id':key}})['object']
    def public_pages(self,initial_state):
        context=self.workspace()['context'];assert context==initial_state['context'];objects=self.all_objects(context)
        assert objects==initial_state['objects']
        links=self.rpc('channel.knowledgeList',{'channelId':ROOT+'/lobby','limit':100})
        assert links.get('next') is None and len(links['items'])==9
        assert {v['source']['reference']['id'] for v in links['items']}=={p['view'] for p in initial_state['publication']['pages']}
        return objects,context
    def all_objects(self,context):
        objects={}
        for kind in ['view','block']:
            page=self.rpc('data.object.list',{'context':context,'objectType':kind,'limit':100,'includeArchived':True})
            assert page.get('next') is None and len(page['items'])==9
            for obj in page['items']:objects[kind+':'+obj['id']]=obj
        return objects
    def lookup(self,request,origin):return self.rpc('data.request.get',{'requestId':request,'context':origin})
    def submit(self,origin,edits,message):
        request=str(uuid.uuid4());params={'requestId':request,'context':origin,'change':{'kind':'apply','message':message,'edits':edits}}
        self.record('original-Knowledge-intent',{'requestId':request,'params':params,'attempts':1,'neverResubmit':True})
        result=self.rpc('data.submit',params);assert result['phase']=='committed' and result['requestId']==request
        lookup=self.lookup(request,origin);assert lookup['lookup']['status']=='known' and lookup['lookup']['result']==result
        return {'request':request,'origin':origin,'params':params,'receipt':result}
    def refusal(self,method,params,code,contains):
        identity=self.human.request(method,params)
        for _ in range(256):
            frame=self.human.frame();self.record('expected-original-refusal-frame',frame)
            if frame.get('id')!=identity:self.human.events.append(frame);continue
            error=frame['error'];assert error['code']==code and contains in error['message'];return error
        raise RuntimeError('original expected refusal unknown; no retry')
    def finish(self):
        self.watch_stop.set()
        if self.watch:self.watch.join(timeout=3)
        if self.watch_errors or (self.watch and self.watch.is_alive()):self.report.update(exit=1,processObservationUnqualified=True)
        active=self.proc is not None or any(x['proc'].poll() is None for x in self.tools)
        if self.human:self.human.close()
        if active:self.report.update(exit=1,completeArchive=False,mutableFailureSnapshot=True,privateRuntimeRetained=str(self.runtime))
        else:
            for stream in self.streams:
                if not stream.closed:stream.flush();stream.close()
        complete=not active and self.report['exit']==0
        if complete:
            try:
                special=[]
                def ignore(directory,names):
                    skipped=[]
                    for name in names:
                        q=Path(directory)/name;info=q.lstat()
                        if not (stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):
                            special.append({'path':str(q.relative_to(self.runtime)),'mode':info.st_mode,'device':info.st_dev,'inode':info.st_ino});skipped.append(name)
                    return skipped
                shutil.copytree(self.runtime,self.out/'cold/runtime-final',ignore=ignore,symlinks=False)
                self.record('complete-stopped-private-runtime-archive',{'specialInodesMetadataOnly':special,'allOwnedProcessesGone':True})
            except BaseException as error:
                complete=False;self.report.update(exit=1,archiveFailed=type(error).__name__+': '+str(error),privateRuntimeRetained=str(self.runtime))
        self.report.update(ownedCoreBirths=self.births,watchErrors=self.watch_errors,pendingOriginals=self.pending,
            ownedToolObligations=[{'birth':x['birth'],'argv':x['argv'],'exit':x['proc'].poll(),'currentBirth':birth(x['birth']['pid'])} for x in self.tools])
        save(self.out/'result.json',self.report)
        save(self.out/('archive-manifest.json' if complete else 'failure-snapshot-manifest.json'),
             {'completeArchive':complete,'mutableFailureSnapshot':active,'privateRuntime':str(self.runtime),
              'files':{str(q.relative_to(self.out)):sha(q) for q in self.out.rglob('*') if q.is_file()}})
    def succeed(self):
        assert self.proc is None and self.human is None and not self.pending and not self.watch_errors
        assert all(not same(b,birth(b['pid'])) for b in self.births)
        assert all(x['proc'].poll() is not None and not same(x['birth'],birth(x['birth']['pid'])) for x in self.tools)
        assert all(sha(Path(q))==digest for q,digest in self.input_hashes.items())
        self.report.update(exit=0,allOriginalCoreWait0=True,inputsUnchanged=True,modelsOrExecutorsStarted=False)
