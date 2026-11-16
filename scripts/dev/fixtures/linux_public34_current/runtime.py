"""Owned ordinary-Core controller. Only Root calls these methods."""
import ctypes,hashlib,json,os,shutil,socket,stat,subprocess,sys,tempfile,threading,time,uuid
from pathlib import Path
from common import birth,same,save,sha,socket_owned,wait_for,procfs_gone,procfs_pid,ProcfsObservationError,WatchErrors
from lifecycle import Lifecycle
from transport import JournaledPublic
from cold import ROOT,CREWS,core_copy,initial,closure

def candidate_error(operation,pid,candidates,cause,tid=None):
    error=ProcfsObservationError(operation,Path('/proc',str(pid),'task'),pid,cause,tid)
    error.observation.update(observedCandidatePIDs=sorted(set(candidates)),
        lineageUnknown=True,candidatesNotProvenGone=True,signalsAllowed=False)
    return error

def executable(pid):
    procfs_pid(pid);path='/proc/'+str(pid)+'/exe'
    try:return os.readlink(path).removesuffix(' (deleted)')
    except OSError as error:
        if procfs_gone(error) and birth(pid) is None:
            # NativeRunner already handles FileNotFoundError + missing current
            # birth. Normalize only this exact gone PID's link, not arbitrary IO.
            raise FileNotFoundError(error.errno,error.strerror,path) from error
        raise ProcfsObservationError('readlink-pid-exe',path,pid,error) from error

def children(pid):
    result=[]
    try:return scan_children(pid,result)
    except BaseException as error:
        if result:
            if isinstance(error,ProcfsObservationError):
                error.observation.update(observedCandidatePIDs=sorted(set(result)),
                    lineageUnknown=True,candidatesNotProvenGone=True,signalsAllowed=False)
            else:
                raise candidate_error('children-scan-failed-after-observed-candidates',pid,result,error) from error
        raise

def scan_children(pid,result):
    procfs_pid(pid);directory=Path('/proc',str(pid),'task')
    try:tasks=list(directory.iterdir())
    except OSError as error:
        if procfs_gone(error) and birth(pid) is None:return []
        raise ProcfsObservationError('list-pid-task',directory,pid,error) from error
    for task in tasks:
        if not task.name.isdecimal() or int(task.name)<=0:raise ValueError('unexpected procfs TID name')
        tid=int(task.name);task=directory/str(tid);path=task/'children'
        try:text=path.read_text()
        except UnicodeError as error:raise ProcfsObservationError('read-task-children',path,pid,error,tid) from error
        except OSError as error:
            if not procfs_gone(error):raise ProcfsObservationError('read-task-children',path,pid,error,tid) from error
            # A live task with an unavailable children interface is Unknown.
            # Only a confirmed disappeared enumerated task or missing root can
            # be skipped. Never Path.exists(), which can hide permission/IO.
            try:task.stat()
            except OSError as changed:
                if procfs_gone(changed):continue
                raise ProcfsObservationError('recheck-task-directory',task,pid,changed,tid) from changed
            if birth(pid) is None:
                if result:raise candidate_error('root-disappeared-after-observed-children',pid,result,error,tid) from error
                return []
            raise ProcfsObservationError('read-live-task-children',path,pid,error,tid) from error
        try:result.extend(procfs_pid(int(x)) for x in text.split())
        except ValueError as error:raise ProcfsObservationError('parse-task-children',path,pid,error,tid) from error
    return sorted(set(result))

class Runner(Lifecycle):
    def __init__(self,args,out,label,qualified,inputs):
        self.a=args;self.out=out;self.label=label;self.qualified=qualified;self.input_hashes=inputs.copy()
        self.out.mkdir(mode=0o700);self.owns_output=True
        for n in ['observations','logs','cold']:(out/n).mkdir(mode=0o700)
        self.phase=self.seq=0;self.proc=self.owner=self.human=self.hibernate_request=None
        self.commands=[];self.pending=[];self.streams=[];self.tools=[];self.births=[];self.watch_errors=WatchErrors()
        self._pending_tool=None;self._finish_errors=[]
        self.watch_stop=threading.Event();self.watch_lock=threading.Lock()
        self.watch=None;self._watch_loop_completed=False
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
        self.watch=threading.Thread(target=self.watch_loop,daemon=True);self.watch.start()
    def native_args(self):return []
    def watch_loop(self):
        try:
            self.observe()  # retain NativeRunner's actual override
            self._watch_loop_completed=self.watch_stop.is_set()
        except BaseException as error:
            self._watch_loop_completed=False
            self.watch_errors.append(type(error).__name__+': '+str(error))
    def observe(self):
        while not self.watch_stop.wait(.03):
            try:
                with self.watch_lock:roots=list(self.births)
                for root in roots:
                    if same(root,birth(root['pid'])):
                        found=children(root['pid'])
                        try:current=birth(root['pid'])
                        except BaseException as error:
                            if found:
                                raise candidate_error('root-recheck-failed-after-observed-children',root['pid'],found,error) from error
                            raise
                        if not same(root,current):
                            if found:
                                raise candidate_error('root-changed-after-observed-children',root['pid'],found,
                                    RuntimeError('parent identity changed after nonempty child enumeration'))
                            continue
                        assert not found,('unexpected owned Core child with both executors disabled',root,found)
            except BaseException as error:self.watch_errors.append(type(error).__name__+': '+str(error))
    def tool_entries(self):
        # Append can raise before OR after mutation. The prelaunch slot remains
        # a strong reference until successful registration, with identity dedup.
        yield from self.tools
        pending=self._pending_tool
        if pending is not None and not any(x is pending for x in self.tools):yield pending
    def tool_wait(self,entry,timeout):
        code=entry['proc'].wait(timeout=timeout)
        if type(code) is not int:raise RuntimeError('original tool wait result unknown')
        entry['waitKnown']=True;entry['rawWait']=code
        return code
    def close_tool_sinks(self,entry):
        assert entry['waitKnown'],'never close an original tool log before its actual wait'
        if entry['logIOUnknown']:return False # Unknown flush/write is never retried.
        try:
            for stream in entry['sinks']:
                if not stream.closed:stream.flush();stream.close()
            return True
        except BaseException as error:
            entry['logIOUnknown']=True;entry['logErrorType']=type(error).__name__
            entry['failed']=True;self.report['exit']=1
            return False
    def tool_obligation(self,entry):
        proc=entry['proc'];pid=entry['pid'];current=None;probe_error=None;observed_exit=None;probe_details=[]
        def probe_detail(stage,error):
            import traceback
            probe_details.append({'stage':stage,'errorType':type(error).__name__,'errno':getattr(error,'errno',None),
                'procfs':getattr(error,'observation',None),
                'traceback':''.join(traceback.format_exception(type(error),error,error.__traceback__))})
        if proc is not None:
            try:observed_exit=proc.poll()
            except BaseException as error:probe_error='poll:'+type(error).__name__;probe_detail('original-popen-poll',error)
        if pid is not None:
            try:current=birth(pid)
            except BaseException as error:probe_error='birth:'+type(error).__name__;probe_detail('current-tool-birth',error)
        # A missing original birth is Unknown, never evidence that it is gone.
        gone=(not same(entry['birth'],current)) if entry['identityQualified'] and probe_error is None else None
        return {'pid':pid,'birth':entry['birth'],'identityQualified':entry['identityQualified'],
            'argv':entry['argv'],'label':entry['label'],'originalPopenRetained':proc is not None,
            'waitKnown':entry['waitKnown'],'rawOriginalWait':entry['rawWait'],'exit':entry['rawWait'] if entry['waitKnown'] else None,'pollExit':observed_exit,
            'currentBirth':current,'originalBirthGone':gone,'observationError':probe_error,'observationErrorDetails':probe_details,
            'sinksClosed':all(x.closed for x in entry['sinks']),'logIOUnknown':entry['logIOUnknown'],
            'metadataFailed':entry['failed'],'errorType':entry['errorType'],'logErrorType':entry['logErrorType'],
            'state':'original-child-terminal' if entry['waitKnown'] else 'unknown-original-child-needs-supervision'}
    def tool(self,argv,label,expected=0,contains=None):
        assert self._pending_tool is None,'retain previous unregistered original; never replace it'
        index=len(self.tools);out=self.out/'logs'/f'cli-{index}-{label}.stdout';err=out.with_suffix('.stderr')
        stdout=out.open('xb');stderr=err.open('xb');self.streams.extend([stdout,stderr])
        self.record('owned-tool-before-launch',{'argv':argv,'label':label,'attempts':1})
        # Allocate every ledger field BEFORE Popen. Its returned handle lands in
        # this retained slot before birth, assertions, appends or metadata IO.
        entry={'proc':None,'pid':None,'birth':None,'identityQualified':False,'argv':argv,'label':label,
               'sinks':(stdout,stderr),'waitKnown':False,'rawWait':None,'logIOUnknown':False,
               'logErrorType':None,'failed':False,'errorType':None}
        self._pending_tool=entry
        try:
            entry['proc']=subprocess.Popen(argv,cwd=self.runtime/'work',env=self.env,stdin=subprocess.DEVNULL,stdout=stdout,stderr=stderr)
            proc=entry['proc'];entry['pid']=proc.pid
            self.tools.append(entry);self._pending_tool=None
            owned=birth(proc.pid);entry['birth']=owned
            assert owned and owned['ppid']==os.getpid();entry['identityQualified']=True
            self.record('original-owned-tool-birth',{'birth':owned,'argv':argv,'label':label})
            code=self.tool_wait(entry,45)
            if not self.close_tool_sinks(entry):raise RuntimeError('original tool log closure unknown')
            assert not same(owned,birth(proc.pid))
            value={'argv':argv,'originalBirth':owned,'exit':code,'stdout':out.read_text(),'stderr':err.read_text(),'physicalExit':True}
            self.record('owned-tool-terminal',value)
            assert (code==expected if expected is not None else code!=0),value
            if contains:assert contains in value['stderr'],value
            return value
        except BaseException as error:
            entry['failed']=True;entry['errorType']=type(error).__name__;self.report['exit']=1
            raise
        finally:
            # A registry append error deliberately keeps _pending_tool alive.
            # Popen raising before returning supplies no original handle to own.
            if entry['proc'] is None:self._pending_tool=None
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
        assert not self.watch_errors,'process observation Unknown: no cold-success copy'
        assert self.watch is not None and self.watch.is_alive(),'observer ended unexpectedly: no cold-success copy'
        assert self._pending_tool is None
        assert all(x['waitKnown'] and x['proc'].poll() is not None and not x['failed'] and not x['logIOUnknown'] for x in self.tool_entries())
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
    def finish_save(self,path,value):
        # A reporting error cannot abandon a still-owned original Popen.
        try:save(path,value);return True
        except BaseException as error:
            self._finish_errors.append({'path':str(path),'errorType':type(error).__name__})
            self.report['finishErrors']=list(self._finish_errors)
            self.report.update(exit=1,finishMetadataUnknown=True,completeArchive=False,
                mutableFailureSnapshot=True,privateRuntimeRetained=str(self.runtime))
            return False
    def supervise_failed_tool(self,entry):
        proc=entry['proc']
        if proc is None:return
        # Business has already failed. This is only the original Popen owner,
        # not a longer operation budget, retry, provider call or replacement.
        while not entry['waitKnown']:
            try:self.tool_wait(entry,.25)
            except subprocess.TimeoutExpired:continue
            except BaseException as error:
                entry['failed']=True;entry['errorType']=type(error).__name__
                self.report.update(exit=1,originalToolWaitUnknown=True)
                time.sleep(.25)
        self.close_tool_sinks(entry)
    def finish(self):
        try:
            self.watch_stop.set()
            if self.watch:self.watch.join(timeout=3)
        except BaseException as error:
            self._finish_errors.append({'stage':'watch-close','errorType':type(error).__name__});self.report['exit']=1
        watch_unqualified=bool(self.watch_errors) or bool(self.watch and (self.watch.is_alive() or not getattr(self,'_watch_loop_completed',False)))
        if watch_unqualified:self.report.update(exit=1,processObservationUnqualified=True)
        if self.human:
            try:self.human.close()
            except BaseException as error:
                self._finish_errors.append({'stage':'public-close','errorType':type(error).__name__});self.report['exit']=1
        before=[self.tool_obligation(x) for x in self.tool_entries()]
        if any(not x['waitKnown'] for x in before):
            self.report.update(exit=1,completeArchive=False,mutableFailureSnapshot=True,
                privateRuntimeRetained=str(self.runtime),ownedToolObligations=before)
            self.finish_save(self.out/'tool-supervision-before-wait.json',self.report)
        # Keep each original handle/sink, including the emergency append slot,
        # until its actual wait becomes known. Never signal an unknown identity.
        for entry in self.tool_entries():self.supervise_failed_tool(entry)
        obligations=[self.tool_obligation(x) for x in self.tool_entries()]
        unqualified=watch_unqualified or any(not x['identityQualified'] or x['observationError'] or x['logIOUnknown'] or not x['sinksClosed'] for x in obligations)
        active=self.proc is not None or any(not x['waitKnown'] for x in obligations)
        if any(x['metadataFailed'] for x in obligations):self.report['exit']=1
        if active or unqualified or self._finish_errors:
            self.report.update(exit=1,completeArchive=False,mutableFailureSnapshot=True,privateRuntimeRetained=str(self.runtime))
        else:
            # Tool sinks were closed only after the exact original wait. Do not
            # retry an IO-unknown sink through this generic Core-log loop.
            tool_sinks={id(s) for x in self.tool_entries() for s in x['sinks']}
            for stream in self.streams:
                if id(stream) not in tool_sinks and not stream.closed:
                    try:stream.flush();stream.close()
                    except BaseException as error:
                        self._finish_errors.append({'stage':'Core-log-close','errorType':type(error).__name__});self.report['exit']=1
        complete=not active and not unqualified and not self._finish_errors and self.report['exit']==0
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
        if self.report['exit']!=0:self.report['privateRuntimeRetained']=str(self.runtime)
        try:files={str(q.relative_to(self.out)):sha(q) for q in self.out.rglob('*') if q.is_file()}
        except BaseException as error:
            self._finish_errors.append({'stage':'finish-file-hash','errorType':type(error).__name__})
            self.report.update(exit=1,completeArchive=False,mutableFailureSnapshot=True,privateRuntimeRetained=str(self.runtime))
            complete=False;files={}
        self.report.update(ownedCoreBirths=self.births,watchErrors=self.watch_errors,
            watchErrorDetails=list(getattr(self.watch_errors,'details',[])),
            watchErrorDetailsDropped=getattr(self.watch_errors,'details_dropped',0),
            watchLoopCompleted=getattr(self,'_watch_loop_completed',False),pendingOriginals=self.pending,
            ownedToolObligations=obligations,finishErrors=list(self._finish_errors),completeArchive=complete,
            mutableFailureSnapshot=active or unqualified or bool(self._finish_errors))
        # Hash the exact common.save serialization in memory, not a fallible
        # post-success reread. Independent archive verification checks its bytes.
        try:result_sha=hashlib.sha256((json.dumps(self.report,indent=2)+'\n').encode()).hexdigest()
        except BaseException as error:
            result_sha=None;self._finish_errors.append({'stage':'finish-result-serialization','errorType':type(error).__name__})
            self.report.update(exit=1,completeArchive=False,mutableFailureSnapshot=True,finishErrors=list(self._finish_errors))
        saved=self.finish_save(self.out/'result.json',self.report)
        if saved and result_sha is not None:files['result.json']=result_sha
        # The success manifest cannot exist if final metadata was not saved.
        complete=complete and saved and result_sha is not None and not self._finish_errors
        self.finish_save(self.out/('archive-manifest.json' if complete else 'failure-snapshot-manifest.json'),
             {'completeArchive':complete,'mutableFailureSnapshot':active or unqualified or bool(self._finish_errors),
              'privateRuntime':str(self.runtime),'finishErrors':list(self._finish_errors),'files':files})
    def succeed(self):
        assert self.proc is None and self.human is None and not self.pending and not self.watch_errors
        assert self.watch is not None and self.watch.is_alive(),'observer stopped before requested closure'
        assert self._pending_tool is None and not self._finish_errors
        assert all(x['waitKnown'] and x['identityQualified'] and not x['failed'] and not x['logIOUnknown'] for x in self.tool_entries())
        assert all(not same(b,birth(b['pid'])) for b in self.births)
        assert all(x['proc'].poll() is not None and not same(x['birth'],birth(x['birth']['pid'])) for x in self.tools)
        assert all(sha(Path(q))==digest for q,digest in self.input_hashes.items())
        self.report.update(exit=0,allOriginalCoreWait0=True,inputsUnchanged=True,modelsOrExecutorsStarted=False)
