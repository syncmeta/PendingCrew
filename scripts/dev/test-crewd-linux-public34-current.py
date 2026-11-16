#!/usr/bin/env python3
"""Local pure controls for the new named cohort, never a product/Linux proof.

The original public34 controls still execute separately with all old assertions.
Only constants and pure functions are extracted from the formal host AST.
"""
import ast,copy,hashlib,importlib.util,json,os,pathlib,re,stat,sys,tempfile,unittest,uuid
sys.dont_write_bytecode=True
ROOT=pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'scripts/dev/fixtures/linux_public34_current'))
import patrol_contract as original_small
import patrol_n6_contract as fixed


def formal_pure():
    path=ROOT/'deploy/linux/pendingcrew-host.py';tree=ast.parse(path.read_bytes())
    constants={'PUBLIC34','PUBLIC34_CURRENT'}
    functions={'release_generation','qualify_public34_release','qualify_release_manifest',
               'public34_descriptor','qualify_public34_source','qualify_original_cohort'}
    class Refused(Exception):pass
    scope={'Refused':Refused,'re':re}
    for node in tree.body:
        if isinstance(node,ast.Assign) and len(node.targets)==1 and isinstance(node.targets[0],ast.Name) and node.targets[0].id in constants:
            scope[node.targets[0].id]=ast.literal_eval(node.value)
    nodes=[node for node in tree.body if isinstance(node,ast.FunctionDef) and node.name in functions]
    assert len(nodes)==len(functions) and constants<=set(scope)
    exec(compile(ast.Module(body=nodes,type_ignores=[]),str(path),'exec'),scope)
    return scope


class CohortControls(unittest.TestCase):
    def setUp(self):self.h=formal_pure()
    def release(self,current):
        d=self.h['PUBLIC34_CURRENT' if current else 'PUBLIC34']
        e={k:d[k] for k in ['schemaSourceSHA256','h33SourceSHA256','secretarySourceSHA256','apiSchemaSHA256','rustInputDigest']}
        e.update(sourceCommit='a'*40,storeVersion=34)
        return {'format':3,'storeVersion':34,'sourceCommit':'a'*40,'cohort':d['cohort'],
                'architecture':'x86_64','apiVersion':3,'schemaEvidence':e}
    def test_two_exact_profiles_and_original_fieldless_binding(self):
        old=self.h['PUBLIC34'];new=self.h['PUBLIC34_CURRENT'];qualify=self.h['qualify_public34_source']
        self.assertEqual(qualify(old['rustInputDigest'],680),old)
        self.assertEqual(qualify(old['rustInputDigest'],680,old['cohort']),old)
        self.assertEqual(qualify(new['rustInputDigest'],681,new['cohort']),new)
        for digest,count,cohort in [(new['rustInputDigest'],681,None),(new['rustInputDigest'],680,new['cohort']),
            (old['rustInputDigest'],681,old['cohort']),(old['rustInputDigest'],680,new['cohort']),
            ('f'*64,681,new['cohort']),(new['rustInputDigest'],681,'private34'),
            (old['rustInputDigest'],True,None),(new['rustInputDigest'],681,'')]:
            with self.subTest(digest=digest,count=count,cohort=cohort),self.assertRaises(self.h['Refused']):qualify(digest,count,cohort)
    def test_release_and_manifest_profiles_cannot_cross_or_acquire_private_evidence(self):
        for current in [False,True]:
            release=self.release(current);self.assertEqual(self.h['release_generation'](release),34)
            self.h['qualify_release_manifest']({'format':2,'cohort':release['cohort'],
                'artifact':{'rustInputDigest':release['schemaEvidence']['rustInputDigest']}})
            bad=copy.deepcopy(release);bad['cohort']=self.release(not current)['cohort']
            with self.assertRaises(self.h['Refused']):self.h['release_generation'](bad)
            bad=copy.deepcopy(release);bad['schemaEvidence']['privateQueryFormat']='private34'
            with self.assertRaises(self.h['Refused']):self.h['release_generation'](bad)
            with self.assertRaises(self.h['Refused']):self.h['qualify_release_manifest']({'format':2,
                'cohort':release['cohort'],'artifact':{'rustInputDigest':self.release(not current)['schemaEvidence']['rustInputDigest']}})
    def test_immutable_original_binding_and_historical_status_identity(self):
        old=self.release(False);new=self.release(True);check=self.h['qualify_original_cohort']
        check({},old);check({'cohort':old['cohort']},old);check({'cohort':new['cohort']},new)
        for original,release in [({},new),({'cohort':old['cohort']},new),({'cohort':new['cohort']},old),
                                 ({'cohort':new['cohort']},{'format':1,'storeVersion':21})]:
            with self.assertRaises(self.h['Refused']):check(original,release)
        check({},{'format':1,'storeVersion':21})
    def test_no_duplicate_profile_authority_in_private_helpers(self):
        for name in ['install-crewd-linux-candidate.py','crewd_managed_launch.py']:
            body=(ROOT/'scripts/dev'/name).read_text();ast.parse(body)
            self.assertNotIn(self.h['PUBLIC34_CURRENT']['rustInputDigest'],body)
            self.assertNotIn(self.h['PUBLIC34']['rustInputDigest'],body)
            self.assertIn('public34_profile(package',body)
            self.assertIn('formalAuthoritySHA256',body)
        body=(ROOT/'scripts/dev/install-crewd-linux-candidate.py').read_text()
        self.assertLess(body.index("if args.action=='status':"),body.index('import crewd_managed_launch as launch'))
        self.assertLess(body.index('if transaction.exists():'),body.index('inactive(prefix)'))
        self.assertIn('if base!=payload',body)
        self.assertLess(body.index('if transaction.exists():'),body.index('import crewd_managed_launch as launch'))
    def test_current_recipe_is_fixed_and_preserves_no_bytecode_source_closure(self):
        path=ROOT/'scripts/dev/build-crewd-linux-public34-current.py';tree=ast.parse(path.read_bytes())
        values={n.targets[0].id:ast.literal_eval(n.value) for n in tree.body if isinstance(n,ast.Assign)
                and len(n.targets)==1 and isinstance(n.targets[0],ast.Name) and n.targets[0].id in ['INPUT','COHORT','TREE']}
        self.assertEqual(values['INPUT'],self.h['PUBLIC34_CURRENT']['rustInputDigest'])
        self.assertEqual(values['COHORT'],self.h['PUBLIC34_CURRENT']['cohort'])
        self.assertEqual(values['TREE'],'b68ff54fa2b51d6f5f2cf20515cc1756b8837488')
        isolated=[]
        for node in ast.walk(tree):
            if not (isinstance(node,ast.Call) and isinstance(node.func,ast.Name) and node.func.id=='command'
                    and node.args and isinstance(node.args[0],ast.List)):continue
            argv=node.args[0].elts
            if argv and isinstance(argv[0],ast.Attribute) and argv[0].attr=='executable' and any(isinstance(x,ast.Constant) and x.value=='-I' for x in argv):isolated.append(argv)
        self.assertEqual(len(isolated),3)
        self.assertTrue(all(isinstance(x[1],ast.Constant) and x[1].value=='-B' and isinstance(x[2],ast.Constant) and x[2].value=='-I' for x in isolated))
    def test_installed_authority_requires_original_sha_before_any_source_execution(self):
        path=ROOT/'scripts/dev/crewd_managed_launch.py';tree=ast.parse(path.read_bytes())
        nodes=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='formal_public34_contract']
        self.assertEqual(len(nodes),1)
        with tempfile.TemporaryDirectory(prefix='pcl34-authority-') as temporary:
            directory=pathlib.Path(temporary).resolve()/'bin';directory.mkdir(mode=0o700)
            authority=directory/'pendingcrew-host.py'
            scope={'__file__':str(directory/'crewd_managed_launch.py'),'pathlib':pathlib,'os':os,
                   'stat':stat,'re':re,'hashlib':hashlib,'importlib':importlib}
            exec(compile(ast.Module(body=nodes,type_ignores=[]),str(path),'exec'),scope)
            load=scope['formal_public34_contract']
            # A malformed SHA must reject even bytes that would raise if run.
            raw=b"raise RuntimeError('unverified source was executed')\n";authority.write_bytes(raw);authority.chmod(0o400)
            before=(authority.read_bytes(),authority.stat().st_mode,authority.stat().st_ino)
            for digest in [None,'f'*64,'not-a-sha']:
                with self.subTest(digest=digest),self.assertRaises(ValueError):load(digest)
            self.assertEqual((authority.read_bytes(),authority.stat().st_mode,authority.stat().st_ino),before)
            authority.unlink();raw=b"VERIFIED_LOCAL_CONTROL = True\n";authority.write_bytes(raw);authority.chmod(0o400)
            digest=hashlib.sha256(raw).hexdigest();self.assertIs(load(digest).VERIFIED_LOCAL_CONTROL,True)
            authority.chmod(0o600)
            with self.assertRaises(ValueError):load(digest)
            authority.chmod(0o400);other=directory/'hardlink';os.link(authority,other)
            with self.assertRaises(ValueError):load(digest)
            other.unlink();authority.unlink();authority.symlink_to(directory/'missing')
            with self.assertRaises(ValueError):load(digest)


class NaturalN6Controls(unittest.TestCase):
    def state(self,large=True):
        active=['target','management','welcome','demo','questions','child']
        if large:return fixed.start_large('target',active,[],1000,10,'W1',[f'W{i}' for i in range(2,62)])
        state=fixed.start_small('target',active,[],1000,10);state['fixedWorkIds']=['W1'];return state
    def status(self,n,complete=None):
        complete=(n==61) if complete is None else complete
        run={'runId':'11111111-1111-4111-8111-111111111111','startedAtMs':1001,
             'completedAtMs':2000 if complete else None,'scan':'channels' if complete else 'work',
             'after':None if complete else f'W{n}','capturedSeq':7,
             'scannedCounts':{'work':n,'entries':0,'channels':0} if complete else {'work':n},
             'findings':{'unowned':[f'W{i}' for i in range(1,min(n,50)+1)]},
             'counts':{'unowned':n},'truncated':n>50}
        return {'crewId':'target','run':run,'error':None}
    def test_large_three_real_pages_and_two_equal_complete_reads(self):
        state=self.state();self.assertEqual(state['totalBudgetSeconds'],660);self.assertEqual(state['noProgressBudgetSeconds'],240)
        self.assertEqual(fixed.observe_large(state,self.status(25),11),'producer-progress')
        self.assertEqual(fixed.observe_large(state,self.status(50),12),'producer-progress')
        self.assertEqual(fixed.observe_large(state,self.status(61),13),'first-complete-read')
        self.assertEqual(fixed.observe_large(state,self.status(61),14),'completed-confirmed')
        self.assertEqual(state['lastRun']['counts']['unowned'],61)
        self.assertEqual(len(state['lastRun']['findings']['unowned']),50)
    def test_small_stays_exact_one_work_and_after_none(self):
        state=self.state(False);good=self.status(1,True)
        self.assertEqual(fixed.observe_small(state,good,11),'first-complete-read')
        self.assertEqual(fixed.observe_small(state,good,12),'completed-confirmed')
        for bad in [self.status(25),self.status(61)]:
            with self.assertRaises(ValueError):fixed.observe_small(self.state(False),bad,11)
            with self.assertRaises(ValueError):original_small.validate_run(bad['run'],self.state(False))
        for key,value in [('after','W1'),('truncated',True)]:
            bad=copy.deepcopy(good);bad['run'][key]=value
            with self.assertRaises(ValueError):fixed.observe_small(self.state(False),bad,11)
    def test_large_rejects_missing_page_replaced_uuid_cursor_counts_and_false_truncation(self):
        state=self.state();fixed.observe_large(state,self.status(25),11)
        with self.assertRaises(ValueError):fixed.observe_large(state,self.status(61),12)
        for label,change in [('UUID',lambda x:x['run'].update(runId=str(uuid.uuid4()))),
            ('cursor',lambda x:x['run'].update(after='W49')),
            ('count',lambda x:x['run']['counts'].update(unowned=49)),
            ('duplicate',lambda x:x['run']['findings']['unowned'].__setitem__(0,'W2')),
            ('foreign',lambda x:x['run']['findings']['unowned'].__setitem__(0,'W999')),
            ('partial-truncated',lambda x:x['run'].update(truncated=True))]:
            state=self.state();fixed.observe_large(state,self.status(25),11);bad=self.status(50);change(bad)
            with self.subTest(label=label),self.assertRaises(ValueError):fixed.observe_large(state,bad,12)
        state=self.state();fixed.observe_large(state,self.status(25),11);fixed.observe_large(state,self.status(50),12)
        bad=self.status(61);bad['run']['truncated']=False
        with self.assertRaises(ValueError):fixed.observe_large(state,bad,13)
        with self.assertRaises(ValueError):fixed.observe_large(state,self.status(61),14)
    def test_large_rejects_first_cut_50_duplicate_fixture_and_deadlines(self):
        with self.assertRaises(ValueError):fixed.observe_large(self.state(),self.status(50),11)
        with self.assertRaises(ValueError):fixed.work_ids('W1',['W2']*60)
        with self.assertRaises(ValueError):fixed.observe_large(self.state(),self.status(25),250)
        with self.assertRaises(ValueError):fixed.observe_large(self.state(),self.status(25),670)
        bad=self.status(25);bad['run']['startedAtMs']=1000
        with self.assertRaises(ValueError):fixed.observe_large(self.state(),bad,11)
    def test_other_five_empty_progress_requires_real_coverage_and_original_identity(self):
        good=self.status(0,True);good['crewId']='welcome';good['run']['findings']={};good['run']['counts']={}
        saved=fixed.empty_other(good,'welcome',None);self.assertEqual(saved,good['run'])
        bad=copy.deepcopy(good);bad['run']['scannedCounts'].pop('entries')
        with self.assertRaises(ValueError):fixed.empty_other(bad,'welcome',None)
        bad=copy.deepcopy(good);bad['run']['runId']=str(uuid.uuid4())
        with self.assertRaises(ValueError):fixed.empty_other(bad,'welcome',saved)
    def test_checksum_binds_scope_epoch_original_run_and_field_order(self):
        run=self.status(61)['run'];scope={'dbId':'a'*32,'restoreEpoch':'b'*36};epoch='unwitnessed:::1'
        ordered={k:run[k] for k in fixed.RUN_ORDER}
        for key in ['scannedCounts','findings','counts']:ordered[key]={k:run[key][k] for k in sorted(run[key])}
        material=[1,'target',scope,epoch,ordered]
        digest=hashlib.sha256(json.dumps(material,ensure_ascii=False,separators=(',',':')).encode()).hexdigest()
        value={'version':1,'crew':'target','scope':scope,'epoch':epoch,'run':run,'checksum':digest}
        self.assertEqual(fixed.completed_checksum(value,'target',scope,epoch,run),digest)
        with self.assertRaises(ValueError):fixed.completed_checksum(value,'target',dict(scope,restoreEpoch='foreign'),epoch,run)
        with self.assertRaises(ValueError):fixed.completed_checksum(value,'target',scope,'foreign',run)


import subprocess,types


class ToolSupervisionControls(unittest.TestCase):
    """Execute the actual runtime methods with local fake Popen/birth only.

    These controls use temporary ordinary log files. There is no real child,
    kernel birth probe, Core, command RPC, network, database or credential.
    """
    def fixture(self,birth_fault=None,record_fault=None,append_fault=None,waits=None,save_fault=None,bad_sink=False,hash_fault=False):
        temporary=tempfile.TemporaryDirectory(prefix='pcl34-fake-supervision-');self.addCleanup(temporary.cleanup)
        directory=pathlib.Path(temporary.name).resolve();out=directory/'evidence';out.mkdir()
        for n in ['logs','cold','observations']:(out/n).mkdir()
        runtime=directory/'runtime';runtime.mkdir();(runtime/'work').mkdir()
        records=[];saved=[];save_attempts=[];archives=[];birth_calls=[];sleep_calls=[];popen_calls=[];live_wait_observations=[]
        state={'record_fault_used':False,'save_fault_used':False}
        class FakeProcess:
            pid=123456789
            def __init__(self):self.returncode=None;self.waits=[];self.plan=list(waits or [0]);self.signals=[]
            def poll(self):return self.returncode
            def wait(self,timeout=None):
                self.waits.append(timeout)
                live_wait_observations.append({'logsOpen':all(not s.closed for s in runner.streams),
                    'pendingRetained':runner._pending_tool is not None,
                    'registeredHandles':[x['proc'] is self for x in runner.tool_entries()],
                    'beforeWaitSnapshotAttempted':any(name=='tool-supervision-before-wait.json' for name,_ in save_attempts)})
                value=self.plan.pop(0) if self.plan else (self.returncode if self.returncode is not None else 0)
                if isinstance(value,BaseException):raise value
                self.returncode=value;return value
            def kill(self):self.signals.append('kill');raise AssertionError('no fake signal permitted')
            def terminate(self):self.signals.append('terminate');raise AssertionError('no fake signal permitted')
            def send_signal(self,value):self.signals.append(value);raise AssertionError('no fake signal permitted')
        proc=FakeProcess()
        def fake_birth(pid):
            birth_calls.append(pid)
            if birth_fault=='raise':raise OSError('fake identity metadata error')
            if birth_fault=='missing' or proc.returncode is not None:return None
            return {'pid':pid,'ppid':os.getpid()+(1 if birth_fault=='parent' else 0),'uid':os.getuid(),
                    'pgid':pid,'startTime':99,'bootId':'fake-control-only'}
        def save(path,value):
            captured=copy.deepcopy(value);save_attempts.append((path.name,captured))
            if path.name==save_fault and not state['save_fault_used']:
                state['save_fault_used']=True;raise OSError('fake evidence metadata error')
            saved.append((path.name,captured))
        def record(name,value):
            records.append((name,copy.deepcopy(value)))
            if name==record_fault and not state['record_fault_used']:
                state['record_fault_used']=True;raise OSError('fake post-Popen metadata error')
        class BadSink:
            def __init__(self,wrapped):self.wrapped=wrapped;self.flush_calls=0;self.close_calls=0
            @property
            def closed(self):return self.wrapped.closed
            def flush(self):self.flush_calls+=1;raise OSError('fake original flush outcome unknown')
            def close(self):self.close_calls+=1;self.wrapped.close()
        bad=[]
        def popen(argv,**kwargs):
            popen_calls.append((argv,kwargs))
            # The pending slot and the exact sink references must predate the
            # Popen return, not depend on any birth/metadata operation.
            self.assertIsNotNone(runner._pending_tool)
            self.assertIs(runner._pending_tool['sinks'][0],kwargs['stdout'])
            self.assertIs(runner._pending_tool['sinks'][1],kwargs['stderr'])
            if bad_sink:
                wrapper=BadSink(kwargs['stderr']);bad.append(wrapper)
                runner._pending_tool['sinks']=(kwargs['stdout'],wrapper)
            return proc
        def file_sha(path):
            if hash_fault:raise OSError('fake file metadata hash error')
            return hashlib.sha256(path.read_bytes()).hexdigest()
        namespace={'Path':pathlib.Path,'os':os,'stat':stat,'json':json,'hashlib':hashlib,'same':lambda a,b:a is not None and b is not None and all(a[k]==b[k] for k in ['pid','pgid','startTime','bootId']),
            'birth':fake_birth,'save':save,'sha':file_sha,'Lifecycle':type('NoProductLifecycle',(),{}),
            'subprocess':types.SimpleNamespace(Popen=popen,DEVNULL=subprocess.DEVNULL,TimeoutExpired=subprocess.TimeoutExpired),
            'time':types.SimpleNamespace(sleep=lambda delay:sleep_calls.append(delay)),
            'shutil':types.SimpleNamespace(copytree=lambda *a,**k:archives.append((a,k)))}
        source=ROOT/'scripts/dev/fixtures/linux_public34_current/runtime.py';tree=ast.parse(source.read_bytes())
        nodes=[n for n in tree.body if isinstance(n,ast.ClassDef) and n.name=='Runner'];self.assertEqual(len(nodes),1)
        exec(compile(ast.Module(body=nodes,type_ignores=[]),str(source),'exec'),namespace)
        runner=namespace['Runner'].__new__(namespace['Runner']);runner.out=out;runner.runtime=runtime;runner.env={}
        runner.proc=runner.owner=runner.human=None;runner._pending_tool=None
        class FakeWatch:
            alive=True
            def is_alive(self):return self.alive
            def join(self,timeout):self.alive=False
        runner.watch=FakeWatch();runner._watch_loop_completed=True
        runner.commands=[];runner.pending=[];runner.streams=[];runner.tools=[];runner.births=[];runner.watch_errors=[];runner._finish_errors=[]
        runner.input_hashes={};runner.report={'exit':1,'scope':'fake-local-control'}
        runner.watch_stop=types.SimpleNamespace(set=lambda:None);runner.record=record
        if append_fault:
            class FailingAppend(list):
                def append(self,value):
                    if append_fault=='after':super().append(value)
                    raise MemoryError('fake registry append failure')
            runner.tools=FailingAppend()
        # Cleanup runs only after each test has checked retained open handles.
        def close_test_files():
            for stream in runner.streams:
                if not stream.closed:stream.close()
        self.addCleanup(close_test_files)
        return types.SimpleNamespace(r=runner,proc=proc,records=records,saved=saved,save_attempts=save_attempts,
            archives=archives,birth_calls=birth_calls,sleep_calls=sleep_calls,popen_calls=popen_calls,
            live_wait_observations=live_wait_observations,bad=bad)

    def failure_is_not_green(self,f,raw=0):
        self.assertEqual(f.r.report['exit'],1);self.assertFalse(f.r.report['completeArchive'])
        self.assertEqual(len(f.popen_calls),1);self.assertEqual(f.proc.signals,[]);self.assertEqual(f.archives,[])
        rows=f.r.report['ownedToolObligations'];self.assertEqual(len(rows),1)
        self.assertEqual(rows[0]['pid'],f.proc.pid);self.assertTrue(rows[0]['originalPopenRetained'])
        self.assertTrue(rows[0]['waitKnown']);self.assertEqual(rows[0]['rawOriginalWait'],raw)
        self.assertTrue(rows[0]['metadataFailed']);self.assertIn('privateRuntimeRetained',f.r.report)
        self.assertFalse(any(name=='archive-manifest.json' for name,_ in f.saved))
        return rows[0]

    def test_missing_birth_keeps_original_handle_and_logs_until_real_fake_wait(self):
        f=self.fixture(birth_fault='missing')
        with self.assertRaises(AssertionError):f.r.tool(['/fake-never-executed'],'missing-birth')
        entry=f.r.tools[0];self.assertIs(entry['proc'],f.proc);self.assertFalse(entry['waitKnown'])
        self.assertTrue(all(not s.closed for s in f.r.streams));self.assertEqual(f.proc.waits,[])
        f.r.finish();row=self.failure_is_not_green(f)
        self.assertFalse(row['identityQualified']);self.assertIsNone(row['originalBirthGone'])
        self.assertTrue(row['sinksClosed']);self.assertTrue(f.r.report['mutableFailureSnapshot'])
        self.assertTrue(f.live_wait_observations[0]['logsOpen']);self.assertTrue(f.live_wait_observations[0]['beforeWaitSnapshotAttempted'])
        initial=next(v for n,v in f.saved if n=='tool-supervision-before-wait.json')
        self.assertTrue(initial['mutableFailureSnapshot']);self.assertFalse(initial['ownedToolObligations'][0]['waitKnown'])

    def test_throwing_birth_and_wrong_parent_do_not_fabricate_identity_or_relaunch(self):
        for mode,error in [('raise',OSError),('parent',AssertionError)]:
            with self.subTest(mode=mode):
                f=self.fixture(birth_fault=mode)
                with self.assertRaises(error):f.r.tool(['/fake-never-executed'],'bad-identity')
                f.r.finish();row=self.failure_is_not_green(f)
                self.assertFalse(row['identityQualified']);self.assertIsNone(row['originalBirthGone'])
                self.assertTrue(f.r.report['mutableFailureSnapshot'])

    def test_registry_append_failure_before_or_after_mutation_retains_emergency_slot(self):
        for mode in ['before','after']:
            with self.subTest(mode=mode):
                f=self.fixture(append_fault=mode,waits=[7])
                with self.assertRaises(MemoryError):f.r.tool(['/fake-never-executed'],'append-error')
                self.assertIs(f.r._pending_tool['proc'],f.proc)
                self.assertEqual(len(list(f.r.tool_entries())),1);self.assertEqual(f.birth_calls,[])
                self.assertTrue(all(not s.closed for s in f.r.streams))
                f.r.finish();row=self.failure_is_not_green(f,7)
                self.assertFalse(row['identityQualified']);self.assertIsNone(row['originalBirthGone'])
                self.assertTrue(f.live_wait_observations[0]['pendingRetained'])
                self.assertEqual(f.live_wait_observations[0]['registeredHandles'],[True])

    def test_post_popen_birth_record_failure_preserves_failed_business_after_wait_zero(self):
        f=self.fixture(record_fault='original-owned-tool-birth')
        with self.assertRaises(OSError):f.r.tool(['/fake-never-executed'],'record-error')
        self.assertIs(f.r.tools[0]['proc'],f.proc);self.assertEqual(f.proc.waits,[])
        f.r.finish();row=self.failure_is_not_green(f)
        self.assertTrue(row['identityQualified']);self.assertEqual(row['errorType'],'OSError')
        self.assertEqual(f.proc.waits,[.25]);self.assertTrue(row['originalBirthGone'])

    def test_original_45_second_timeout_only_supervises_same_popen_to_its_terminal(self):
        expired=subprocess.TimeoutExpired(['/fake-never-executed'],45)
        f=self.fixture(waits=[expired,subprocess.TimeoutExpired(['/fake-never-executed'],.25),0])
        with self.assertRaises(subprocess.TimeoutExpired):f.r.tool(['/fake-never-executed'],'original-timeout')
        f.r.finish();self.failure_is_not_green(f)
        self.assertEqual(f.proc.waits,[45,.25,.25]);self.assertEqual(len(f.popen_calls),1)
        self.assertTrue(all(x['logsOpen'] for x in f.live_wait_observations))

    def test_terminal_metadata_error_keeps_observed_original_wait_and_never_waits_again(self):
        f=self.fixture(record_fault='owned-tool-terminal')
        with self.assertRaises(OSError):f.r.tool(['/fake-never-executed'],'terminal-record-error')
        self.assertTrue(f.r.tools[0]['waitKnown']);self.assertEqual(f.r.tools[0]['rawWait'],0)
        f.r.finish();self.failure_is_not_green(f);self.assertEqual(f.proc.waits,[45])

    def test_failed_mutable_snapshot_write_cannot_skip_original_child_supervision(self):
        f=self.fixture(record_fault='original-owned-tool-birth',save_fault='tool-supervision-before-wait.json')
        with self.assertRaises(OSError):f.r.tool(['/fake-never-executed'],'snapshot-error')
        f.r.finish();self.failure_is_not_green(f)
        self.assertTrue(f.r.report['finishMetadataUnknown']);self.assertTrue(f.r.report['mutableFailureSnapshot'])
        self.assertEqual(f.proc.waits,[.25]);self.assertTrue(f.live_wait_observations[0]['logsOpen'])
        self.assertTrue(any(x['path'].endswith('tool-supervision-before-wait.json') for x in f.r.report['finishErrors']))

    def test_unknown_log_flush_is_not_retried_or_closed_by_generic_finish(self):
        f=self.fixture(bad_sink=True)
        with self.assertRaises(RuntimeError):f.r.tool(['/fake-never-executed'],'flush-error')
        self.assertTrue(f.r.tools[0]['waitKnown']);self.assertEqual(f.r.tools[0]['rawWait'],0)
        f.r.finish();row=self.failure_is_not_green(f)
        self.assertTrue(row['logIOUnknown']);self.assertFalse(row['sinksClosed'])
        self.assertEqual(f.bad[0].flush_calls,1);self.assertEqual(f.bad[0].close_calls,0)
        self.assertTrue(f.r.report['mutableFailureSnapshot']);self.assertEqual(f.proc.waits,[45])

    def test_final_result_write_failure_remains_failed_after_the_known_child_exit(self):
        f=self.fixture(record_fault='owned-tool-terminal',save_fault='result.json')
        with self.assertRaises(OSError):f.r.tool(['/fake-never-executed'],'final-evidence-error')
        f.r.finish();self.failure_is_not_green(f)
        self.assertTrue(f.r.report['finishMetadataUnknown']);self.assertTrue(f.r.report['mutableFailureSnapshot'])
        self.assertEqual(f.proc.waits,[45])

    def test_normal_local_fake_tool_keeps_qualified_wait_and_positive_archive_path(self):
        f=self.fixture();value=f.r.tool(['/fake-never-executed'],'positive')
        self.assertEqual(value['exit'],0);self.assertTrue(value['physicalExit'])
        self.assertTrue(all(s.closed for s in f.r.streams));self.assertEqual(f.proc.waits,[45])
        f.r.succeed();f.r.finish()
        self.assertEqual(f.r.report['exit'],0);self.assertTrue(f.r.report['completeArchive'])
        self.assertFalse(f.r.report['mutableFailureSnapshot']);self.assertEqual(len(f.archives),1)
        self.assertEqual(f.proc.signals,[]);self.assertEqual(len(f.popen_calls),1)
        row=f.r.report['ownedToolObligations'][0]
        self.assertTrue(row['identityQualified']);self.assertTrue(row['waitKnown']);self.assertEqual(row['rawOriginalWait'],0)

    def test_finish_hash_metadata_failure_is_recorded_before_terminal_result_not_green(self):
        f=self.fixture(hash_fault=True);f.r.tool(['/fake-never-executed'],'hash-evidence-error');f.r.succeed();f.r.finish()
        self.assertEqual(f.r.report['exit'],1);self.assertFalse(f.r.report['completeArchive'])
        self.assertTrue(f.r.report['mutableFailureSnapshot']);self.assertEqual(f.proc.waits,[45])
        self.assertEqual(f.proc.signals,[]);self.assertEqual(len(f.popen_calls),1)
        result=next(value for name,value in f.saved if name=='result.json')
        self.assertEqual(result['exit'],1);self.assertFalse(result['completeArchive'])
        self.assertTrue(any(x.get('stage')=='finish-file-hash' for x in result['finishErrors']))
        self.assertFalse(any(name=='archive-manifest.json' for name,_ in f.saved))


class NamedTerminalParserControls(unittest.TestCase):
    """Actual archived libtest bytes, no compiler/test/Core/network execution."""
    LISTED = ('client::tests::backoff_grows_by_four_is_capped_and_a_long_retry_after_gives_up', 'client::tests::endpoint_accepts_https_and_local_http_only', 'client::tests::key_is_trimmed_validated_and_never_printed', 'client::tests::retry_after_reads_integer_seconds_only', 'decisions::attribute::tests::candidates_plus_none_capped_and_described_with_vocab_words', 'decisions::attribute::tests::none_is_reserved_and_empty_candidate_list_is_refused', 'decisions::attribute::tests::none_maps_to_none', 'decisions::route::tests::answers_map_to_targets_and_low_confidence_defers', 'decisions::route::tests::bad_inputs_are_refused', 'decisions::route::tests::invented_or_missing_answers_are_malformed', 'decisions::route::tests::options_are_the_three_fixed_ones_plus_members_shuffled_by_seed', 'decisions::route::tests::roster_is_capped_at_fifteen_most_recent', 'decisions::route::tests::wire_order_matches_options', 'decisions::tests::clip_counts_characters_not_bytes', 'decisions::tests::defaults_are_pinned', 'decisions::tests::keys_must_be_ascii_slugs_unique_and_not_reserved', 'decisions::tests::member_view_has_no_raw_numbers', 'decisions::tests::quantities_become_words_without_digits', 'decisions::tests::roster_is_cut_to_the_most_recent_and_unknown_rank_goes_last', 'decisions::urgency::tests::argmax_level_and_gate', 'decisions::urgency::tests::levels_are_in_order_and_quantities_are_words', 'decisions::urgency::tests::out_of_range_levels_or_scores_are_malformed', 'decisions::urgency::tests::state_never_carries_the_durations_or_counts_as_digits', 'decisions::wake::tests::a_missing_member_answer_voids_the_whole_judgement', 'decisions::wake::tests::empty_set_goes_to_the_captain', 'decisions::wake::tests::no_members_or_empty_message_is_refused', 'decisions::wake::tests::one_noul_per_member_and_only_the_last_five_recent_messages', 'decisions::wake::tests::wake_set_is_everyone_at_or_above_the_line', 'diag::tests::existing_loose_file_is_tightened', 'diag::tests::one_json_object_per_line_in_a_private_file', 'diag::tests::rotates_into_a_single_old_file', 'error::tests::safe_projection_uses_types_and_never_error_text', 'error::tests::statuses_are_classified_like_the_docs_say', 'eval::tests::calibration_buckets_include_one_in_the_top_bucket', 'eval::tests::summary_counts_consistency_and_first_pick_bias', 'eval::tests::the_dataset_is_well_formed_synthetic_and_balanced', 'eval::tests::threshold_is_the_lowest_that_meets_the_target', 'eval::tests::validation_catches_bad_expectations', 'eval::tests::wake_threshold_maximises_member_accuracy_ties_toward_half', 'key::tests::a_fifo_at_the_key_path_is_refused_without_hanging', 'key::tests::a_symlinked_key_file_is_refused', 'key::tests::file_store_roundtrips_with_0600_and_clear_is_idempotent', 'key::tests::garbage_in_the_file_is_invalid_not_a_key', 'key::tests::keychain_roundtrip', 'key::tests::loose_permissions_are_refused_not_used', 'key::tests::missing_parent_directories_are_created_private', 'key::tests::owner_type_and_mode_are_all_vetted', 'key::tests::platform_default_matches_the_spec', 'model::tests::choice_criteria_keep_their_order_on_the_wire', 'model::tests::documented_answer_shapes_parse', 'model::tests::noul_and_score_questions_serialize_like_the_docs', 'model::tests::noul_confidence_is_distance_from_a_coin_flip', 'model::tests::unknown_answer_type_is_an_error_not_a_default', 'prompts::tests::every_prompt_file_has_its_sections', 'prompts::tests::missing_section_names_itself', 'prompts::tests::parser_rejects_empty_and_duplicate_sections', 'prompts::tests::prompt_texts_carry_no_digits_that_could_be_mistaken_for_data', 'scrub::tests::a_rejected_candidate_does_not_hide_the_real_value', 'scrub::tests::any_amount_of_whitespace_between_label_and_value', 'scrub::tests::assignments_keep_the_name_and_hide_the_value', 'scrub::tests::bare_key_and_plain_words_are_not_credentials', 'scrub::tests::chinese_label_plus_space_needs_a_mixed_value', 'scrub::tests::chinese_labelled_credentials_keep_the_label_and_hide_the_value', 'scrub::tests::chinese_prose_and_counts_stay_untouched', 'scrub::tests::env_style_assignments_keep_the_name_and_hide_the_value', 'scrub::tests::fuzzed_inputs_scrub_idempotently', 'scrub::tests::glued_vendor_keys_are_all_redacted_in_one_pass', 'scrub::tests::identifier_prefixes_do_not_make_ordinary_words_secrets', 'scrub::tests::is_takes_the_strict_rule', 'scrub::tests::long_values_are_redacted_to_the_end_of_the_run', 'scrub::tests::more_vendor_shapes_webhooks_pgp_and_basic_auth', 'scrub::tests::numeric_passwords_after_a_space_are_redacted', 'scrub::tests::ordinary_chat_is_left_alone', 'scrub::tests::quoted_fullwidth_json_traditional_and_suffixed_forms', 'scrub::tests::realistic_double_scrub_is_a_no_op', 'scrub::tests::request_is_scrubbed_in_place_without_reordering_options', 'scrub::tests::scrubbing_twice_finds_nothing_new', 'scrub::tests::urls_private_keys_and_jwts', 'scrub::tests::values_keep_non_cjk_unicode', 'scrub::tests::vendor_keys_and_tokens', 'scrub::tests::vendor_lookalikes_in_chat_stay_untouched', 'scrub::tests::verification_code_needs_a_code_not_a_year_or_a_count', 'scrub::tests::verification_codes_followed_by_punctuation', 'shuffle::tests::every_item_leads_about_equally_often', 'shuffle::tests::same_seed_same_order_and_it_is_a_permutation', 'shuffle::tests::seed_separates_segments', 'shuffle::tests::tiny_inputs_are_fine')
    IGNORED = ('key::tests::keychain_roundtrip',)
    SHOULD_PANIC = 'prompts::tests::missing_section_names_itself'
    FIXTURE_SHA256 = '4ca53159d6b1b1809c842037b0ffd862b2c3cea24e27b6bf22145aac081e3d8b'
    def helpers(self):
        path=ROOT/'scripts/dev/build-crewd-linux-public34-current.py';tree=ast.parse(path.read_bytes())
        wanted={'affected_output_audit','require_affected_output'}
        nodes=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in wanted]
        self.assertEqual({n.name for n in nodes},wanted)
        scope={'re':re};exec(compile(ast.Module(body=nodes,type_ignores=[]),str(path),'exec'),scope)
        return scope['affected_output_audit'],scope['require_affected_output']
    def raw(self):
        data=(ROOT/'scripts/dev/fixtures/linux_public34_current/cargo_passed_names_fixture.log').read_bytes()
        self.assertEqual(hashlib.sha256(data).hexdigest(),self.FIXTURE_SHA256)
        return data.decode('utf-8')
    def audit(self,text):
        parse,_=self.helpers();return parse(text,self.LISTED,self.IGNORED)
    def refuses(self,text):
        audit=self.audit(text);_,qualify=self.helpers()
        with self.assertRaises(AssertionError):qualify(audit)
        return audit
    def test_actual_original87_fixture_exposes_old85_parser_and_qualifies_real86(self):
        raw=self.raw();old=re.findall(r'^test (\S+) \.\.\. ok$',raw,re.M)
        self.assertEqual(len(old),85)
        self.assertEqual(set(self.LISTED)-set(self.IGNORED)-set(old),{self.SHOULD_PANIC})
        audit=self.audit(raw);_,qualify=self.helpers();qualify(audit)
        self.assertEqual(len(audit['passedNames']),86);self.assertEqual(audit['summaries'],[['86','1']])
        self.assertEqual(audit['runIgnoredNames'],list(self.IGNORED));self.assertEqual(audit['failedNames'],[])
        self.assertEqual(audit['missingNames'],[]);self.assertEqual(audit['extraNames'],[])
        self.assertIn(self.SHOULD_PANIC,audit['passedNames']);self.assertNotIn(self.SHOULD_PANIC+' - should panic',audit['passedNames'])
    def test_missing_should_panic_or_ordinary_actual_terminal_is_not_inferred_from_summary(self):
        raw=self.raw()
        for line in ['test '+self.SHOULD_PANIC+' - should panic ... ok',
                     next(x for x in raw.splitlines() if x.startswith('test ') and x.endswith(' ... ok') and '- should panic' not in x)]:
            with self.subTest(line=line):
                audit=self.refuses(raw.replace(line+'\n','',1))
                self.assertEqual(audit['summaries'],[['86','1']]);self.assertEqual(len(audit['missingNames']),1)
    def test_extra_and_duplicate_actual_ok_terminals_fail_even_with_original_summary(self):
        raw=self.raw();line='test '+self.SHOULD_PANIC+' - should panic ... ok'
        extra=self.refuses(raw+'test foreign::extra ... ok\n')
        self.assertEqual(extra['extraNames'],['foreign::extra'])
        duplicate=self.refuses(raw+line+'\n')
        self.assertEqual(duplicate['missingNames'],[]);self.assertEqual(duplicate['extraNames'],[])
        self.assertEqual(duplicate['duplicatePassedNames'],[self.SHOULD_PANIC])
    def test_true_failed_terminal_rejects_even_when_all_expected_ok_names_are_present(self):
        raw=self.raw()
        audit=self.refuses(raw+'test foreign::actually_failed ... FAILED\n')
        self.assertEqual(audit['missingNames'],[]);self.assertEqual(audit['extraNames'],[])
        self.assertEqual(audit['failedNames'],['foreign::actually_failed'])
        line='test '+self.SHOULD_PANIC+' - should panic ... ok'
        audit=self.refuses(raw.replace(line,line[:-2]+'FAILED',1))
        self.assertEqual(audit['failedNames'],[self.SHOULD_PANIC]);self.assertEqual(audit['missingNames'],[self.SHOULD_PANIC])
    def test_actual_ignored_name_missing_extra_or_duplicate_never_weakens_original_ignore_gate(self):
        raw=self.raw();line=next(x for x in raw.splitlines() if x.startswith('test key::tests::keychain_roundtrip ... ignored'))
        missing=self.refuses(raw.replace(line+'\n','',1));self.assertEqual(missing['missingIgnoredNames'],list(self.IGNORED))
        extra=self.refuses(raw.replace(line,line.replace('key::tests::keychain_roundtrip','foreign::ignored'),1))
        self.assertEqual(extra['extraIgnoredNames'],['foreign::ignored'])
        duplicate=self.refuses(raw+line+'\n');self.assertEqual(duplicate['duplicateRunIgnoredNames'],list(self.IGNORED))
    def test_wrong_missing_duplicate_or_failed_summary_is_not_hidden_by_good_named_lines(self):
        raw=self.raw();summary=next(x for x in raw.splitlines() if x.startswith('test result: ok.'))
        variants=[raw.replace('86 passed; 0 failed; 1 ignored;','85 passed; 0 failed; 1 ignored;',1),
                  raw.replace('86 passed; 0 failed; 1 ignored;','86 passed; 1 failed; 1 ignored;',1),
                  raw.replace('86 passed; 0 failed; 1 ignored;','86 passed; 0 failed; 0 ignored;',1),
                  raw.replace(summary+'\n','',1),raw+summary+'\n']
        for text in variants:
            with self.subTest(summary=text[-150:]):self.refuses(text)
    def test_only_literal_standard_should_panic_display_suffix_is_supported(self):
        raw=self.raw();line='test '+self.SHOULD_PANIC+' - should panic ... ok'
        for suffix in [' - expected panic',' - should panic eventually',' - SHOULD PANIC']:
            with self.subTest(suffix=suffix):
                audit=self.refuses(raw.replace(line,'test '+self.SHOULD_PANIC+suffix+' ... ok',1))
                self.assertEqual(audit['missingNames'],[self.SHOULD_PANIC])
    def test_each_selection_persists_output_audit_before_the_actual_qualifier_call(self):
        tree=ast.parse((ROOT/'scripts/dev/build-crewd-linux-public34-current.py').read_bytes())
        execute=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='execute')
        loops=[n for n in ast.walk(execute) if isinstance(n,ast.For) and isinstance(n.iter,ast.Call)
            and isinstance(n.iter.func,ast.Name) and n.iter.func.id=='enumerate' and n.iter.args
            and isinstance(n.iter.args[0],ast.Name) and n.iter.args[0].id=='additional']
        self.assertEqual(len(loops),1);body=loops[0].body
        def named_call(n,name):return isinstance(n,ast.Expr) and isinstance(n.value,ast.Call) and isinstance(n.value.func,ast.Name) and n.value.func.id==name
        saved=next(i for i,n in enumerate(body) if named_call(n,'save') and len(n.value.args)==2
            and isinstance(n.value.args[1],ast.Name) and n.value.args[1].id=='audit')
        qualified=next(i for i,n in enumerate(body) if named_call(n,'require_affected_output'))
        self.assertLess(saved,qualified)


class ProcfsObservationControls(unittest.TestCase):
    """Actual common/runtime AST with fake procfs only; never a kernel/Popen."""
    def fixture(self,changes=None,birth_override=None):
        import errno,traceback
        fields=['0']*20;fields[0]='S';fields[1]='7';fields[2]='8';fields[19]='99'
        boot='11111111-1111-4111-8111-111111111111'
        values={('read','/proc/42/stat'):'42 (owned core) '+' '.join(fields),
            ('read','/proc/42/status'):'Name:\towned\nUid:\t1001\t1001\t1001\t1001\n',
            ('read','/proc/sys/kernel/random/boot_id'):boot,('iter','/proc/42/task'):['42'],
            ('read','/proc/42/task/42/children'):'',('stat','/proc/42/task/42'):True,
            ('link','/proc/42/exe'):'/owned/crewd'}
        values.update(changes or {});calls=[]
        def operation(kind,path):
            path=str(path);calls.append((kind,path));key=(kind,path)
            value=values.get(key,FileNotFoundError(errno.ENOENT,'fake procfs absent',path))
            if isinstance(value,list) and kind!='iter':
                value=value.pop(0)
            if isinstance(value,BaseException):raise value
            return value
        class FakePath:
            def __init__(self,*parts):self.value=str(pathlib.PurePosixPath(*map(str,parts)))
            def __str__(self):return self.value
            @property
            def name(self):return pathlib.PurePosixPath(self.value).name
            def __truediv__(self,part):return FakePath(self.value,str(part))
            def read_text(self):return operation('read',self)
            def iterdir(self):
                result=operation('iter',self)
                if isinstance(result,types.GeneratorType):return result
                return iter([FakePath(self.value,v) for v in result])
            def stat(self):return operation('stat',self)
        common_path=ROOT/'scripts/dev/fixtures/linux_public34_current/common.py'
        tree=ast.parse(common_path.read_bytes())
        selected={'procfs_gone','procfs_pid','ProcfsObservationError','pid_text','observation_error','WatchErrors','birth','same'}
        nodes=[n for n in tree.body if isinstance(n,(ast.FunctionDef,ast.ClassDef)) and n.name in selected]
        self.assertEqual(len(nodes),len(selected))
        scope={'__name__':'pure_procfs_control','errno':errno,'traceback':traceback,'uuid':uuid,
            'pathlib':types.SimpleNamespace(Path=FakePath),'sys':types.SimpleNamespace(platform='linux',exc_info=sys.exc_info)}
        exec(compile(ast.Module(body=nodes,type_ignores=[]),str(common_path),'exec'),scope)
        real_fake_birth=scope['birth']
        if birth_override is not None:scope['birth']=birth_override
        runtime_path=ROOT/'scripts/dev/fixtures/linux_public34_current/runtime.py';tree=ast.parse(runtime_path.read_bytes())
        nodes=[n for n in tree.body if (isinstance(n,ast.FunctionDef) and n.name in ('children','scan_children','executable','candidate_error'))
            or (isinstance(n,ast.ClassDef) and n.name=='Runner')]
        scope.update(Path=FakePath,Lifecycle=type('NoProductLifecycle',(),{}),
            os=types.SimpleNamespace(readlink=lambda path:operation('link',path)))
        exec(compile(ast.Module(body=nodes,type_ignores=[]),str(runtime_path),'exec'),scope)
        return types.SimpleNamespace(scope=scope,values=values,calls=calls,birth=real_fake_birth,boot=boot,errno=errno,FakePath=FakePath)
    def run_watch_once(self,f,native=False):
        import threading
        r=f.scope['Runner'].__new__(f.scope['Runner']);r.births=[{'pid':42,'ppid':7,'pgid':8,'uid':1001,'startTime':99,'bootId':f.boot}]
        r.watch_lock=threading.Lock();r.watch_errors=f.scope['WatchErrors']();r.helper_births={}
        class Stop:
            count=0
            def wait(self,delay):self.count+=1;return self.count>1
            def is_set(self):return True
        r.watch_stop=Stop()
        if native:
            p=ROOT/'scripts/dev/fixtures/linux_public34_current/native.py';t=ast.parse(p.read_bytes())
            c=next(n for n in t.body if isinstance(n,ast.ClassDef) and n.name=='NativeRunner')
            fn=next(n for n in c.body if isinstance(n,ast.FunctionDef) and n.name=='observe')
            exec(compile(ast.Module(body=[fn],type_ignores=[]),str(p),'exec'),f.scope)
            r.observe=types.MethodType(f.scope['observe'],r)
        r.observe();return r
    def test_real_linux_birth_fields_and_only_generated_positive_pid_paths(self):
        f=self.fixture();self.assertEqual(f.birth(42),{'pid':42,'ppid':7,'pgid':8,'uid':1001,'startTime':99,'bootId':f.boot})
        self.assertEqual(f.calls,[('read','/proc/42/stat'),('read','/proc/42/status'),('read','/proc/sys/kernel/random/boot_id')])
        for pid in (True,0,-1,'42','../sys'):
            with self.subTest(pid=pid),self.assertRaises(ValueError):f.birth(pid)
        with self.assertRaises(ValueError):f.scope['pid_text'](42,'../sys/kernel/random/boot_id')
    def test_stat_and_status_disappearance_both_enoent_and_esrch_are_current_absence_only(self):
        for field in ('stat','status'):
            for number in (2,3):
                path='/proc/42/'+field;f=self.fixture({('read',path):OSError(number,'fake PID disappeared',path)})
                self.assertIsNone(f.birth(42));self.assertNotIn(('read','/proc/sys/kernel/random/boot_id'),f.calls)
                self.assertIs(f.scope['same'](None,None),False) # no original identity is never gone proof
    def test_permission_other_io_and_wrong_errno_subclasses_never_become_missing(self):
        for field in ('stat','status'):
            for number in (1,5,11,13,22,None):
                error=ProcessLookupError(number,'wrong errno is not disappearance')
                f=self.fixture({('read','/proc/42/'+field):error})
                with self.subTest(field=field,errno=number),self.assertRaises(f.scope['ProcfsObservationError']) as got:f.birth(42)
                self.assertEqual(got.exception.observation['operation'],'read-pid-'+field)
                self.assertIs(got.exception.__cause__,error)
    def test_global_boot_missing_or_unreadable_is_unknown_even_with_disappearance_errno(self):
        for number in (2,3,5,13):
            f=self.fixture({('read','/proc/sys/kernel/random/boot_id'):OSError(number,'global unavailable')})
            with self.subTest(errno=number),self.assertRaises(f.scope['ProcfsObservationError']) as got:f.birth(42)
            self.assertEqual(got.exception.observation['operation'],'read-global-boot-id')
    def test_malformed_stat_uid_and_boot_are_never_a_gone_identity(self):
        for path,value,stage in [('/proc/42/stat','42 (owned) S','parse-pid-stat'),
            ('/proc/42/stat','different','parse-pid-stat'),('/proc/42/status','Uid: not-integer','parse-pid-status'),
            ('/proc/sys/kernel/random/boot_id','','read-global-boot-id')]:
            f=self.fixture({('read',path):value})
            with self.subTest(path=path),self.assertRaises(f.scope['ProcfsObservationError']) as got:f.birth(42)
            self.assertEqual(got.exception.observation['operation'],stage)
    def test_task_enumeration_disappearance_requires_root_absence_not_just_errno(self):
        for number in (2,3):
            f=self.fixture({('iter','/proc/42/task'):OSError(number,'task disappeared')},birth_override=lambda pid:None)
            self.assertEqual(f.scope['children'](42),[])
            f=self.fixture({('iter','/proc/42/task'):OSError(number,'task missing but root alive')})
            with self.assertRaises(f.scope['ProcfsObservationError']) as got:f.scope['children'](42)
            self.assertEqual(got.exception.observation['operation'],'list-pid-task')
        f=self.fixture({('iter','/proc/42/task'):PermissionError(13,'denied')},birth_override=lambda pid:None)
        with self.assertRaises(f.scope['ProcfsObservationError']):f.scope['children'](42)
    def test_disappeared_enumerated_tid_skips_only_that_tid_and_keeps_other_children(self):
        for number in (2,3):
            f=self.fixture({('iter','/proc/42/task'):['42','43'],('read','/proc/42/task/42/children'):OSError(number,'old TID disappeared'),
                ('stat','/proc/42/task/42'):OSError(number,'TID absent'),('read','/proc/42/task/43/children'):'100 101 100'})
            self.assertEqual(f.scope['children'](42),[100,101])
            self.assertIn(('stat','/proc/42/task/42'),f.calls)
    def test_live_task_missing_children_is_unknown_and_real_root_exit_is_narrowly_allowed(self):
        for number in (2,3):
            changes={('read','/proc/42/task/42/children'):OSError(number,'interface unavailable')}
            f=self.fixture(changes)
            with self.assertRaises(f.scope['ProcfsObservationError']) as got:f.scope['children'](42)
            self.assertEqual(got.exception.observation['operation'],'read-live-task-children')
            f=self.fixture(changes,birth_override=lambda pid:None);self.assertEqual(f.scope['children'](42),[])
    def test_task_read_recheck_and_parse_other_errors_do_not_suppress_unknown(self):
        cases=[{('read','/proc/42/task/42/children'):PermissionError(13,'denied')},
            {('read','/proc/42/task/42/children'):OSError(3,'gone?'),('stat','/proc/42/task/42'):OSError(5,'IO unknown')},
            {('read','/proc/42/task/42/children'):'100 broken'},
            {('iter','/proc/42/task'):['unexpected']},
            {('read','/proc/42/task/42/children'):'0'}]
        for changes in cases:
            f=self.fixture(changes)
            with self.subTest(changes=list(changes)),self.assertRaises((f.scope['ProcfsObservationError'],ValueError)):f.scope['children'](42)
    def test_partial_task_iterator_error_is_not_an_empty_live_snapshot(self):
        def broken(path):
            yield path
            raise ProcessLookupError(3,'enumeration changed')
        f=self.fixture();f.values[('iter','/proc/42/task')]=broken(f.FakePath('/proc/42/task/42'))
        with self.assertRaises(f.scope['ProcfsObservationError']):f.scope['children'](42)
        f=self.fixture(birth_override=lambda pid:None);f.values[('iter','/proc/42/task')]=broken(f.FakePath('/proc/42/task/42'))
        self.assertEqual(f.scope['children'](42),[])
    def test_exe_link_normalizes_only_named_gone_pid_enoent_esrch_for_unchanged_native_caller(self):
        for number in (2,3):
            error=OSError(number,'gone link')
            f=self.fixture({('link','/proc/42/exe'):error},birth_override=lambda pid:None)
            with self.assertRaises(FileNotFoundError) as got:f.scope['executable'](42)
            self.assertIs(got.exception.__cause__,error);self.assertEqual(got.exception.filename,'/proc/42/exe')
            f=self.fixture({('link','/proc/42/exe'):error})
            with self.assertRaises(f.scope['ProcfsObservationError']):f.scope['executable'](42)
        f=self.fixture({('link','/proc/42/exe'):PermissionError(13,'denied')},birth_override=lambda pid:None)
        with self.assertRaises(f.scope['ProcfsObservationError']):f.scope['executable'](42)
    def test_regular_watch_errors_have_real_operation_errno_path_and_original_stack(self):
        f=self.fixture({('read','/proc/42/status'):PermissionError(13,'fake status denied')});r=self.run_watch_once(f)
        self.assertEqual(len(r.watch_errors),1);detail=r.watch_errors.details[0]
        self.assertEqual(detail['errno'],13);self.assertEqual(detail['procfs']['path'],'/proc/42/status')
        self.assertEqual(detail['procfs']['operation'],'read-pid-status');self.assertIn('pid_text',detail['traceback'])
        self.assertIn('PermissionError',detail['traceback']);self.assertIn('runtime.py',detail['traceback'])
        self.assertEqual(detail['originalRootBirth']['pid'],42);self.assertIs(detail['callsiteInferredFromMessage'],False)
    def test_unchanged_native_override_also_keeps_active_exception_stack(self):
        f=self.fixture({('read','/proc/42/status'):OSError(5,'status IO unknown')});r=self.run_watch_once(f,native=True)
        self.assertEqual(len(r.watch_errors),1);detail=r.watch_errors.details[0]
        self.assertEqual(detail['errno'],5);self.assertIn('native.py',detail['traceback'])
        self.assertEqual(detail['procfs']['operation'],'read-pid-status')
    def test_no_child_guard_stays_fatal_and_original_pid_post_fence_prevents_replacement_mix(self):
        f=self.fixture({('read','/proc/42/task/42/children'):'100'});r=self.run_watch_once(f)
        self.assertEqual(len(r.watch_errors),1);self.assertEqual(r.watch_errors.details[0]['errorType'],'AssertionError')
        f=self.fixture({('read','/proc/42/task/42/children'):'100'})
        original=f.values[('read','/proc/42/stat')];f.values[('read','/proc/42/stat')]=[original,OSError(3,'original exited')]
        r=self.run_watch_once(f);self.assertEqual(len(r.watch_errors),1)
        detail=r.watch_errors.details[0]['procfs']
        self.assertEqual(detail['observedCandidatePIDs'],[100]);self.assertTrue(detail['lineageUnknown'])
        self.assertTrue(detail['candidatesNotProvenGone']);self.assertFalse(detail['signalsAllowed'])
        f=self.fixture();original=f.values[('read','/proc/42/stat')]
        f.values[('read','/proc/42/stat')]=[original,OSError(3,'original exited with no observed child')]
        r=self.run_watch_once(f);self.assertEqual(r.watch_errors,[]) # empty observation never invents children
    def test_root_exit_during_later_tid_read_preserves_previously_observed_candidates(self):
        changes={('iter','/proc/42/task'):['42','43'],('read','/proc/42/task/42/children'):'100 101 100',
            ('read','/proc/42/task/43/children'):OSError(3,'later children interface disappeared'),
            ('stat','/proc/42/task/43'):True}
        f=self.fixture(changes,birth_override=lambda pid:None)
        with self.assertRaises(f.scope['ProcfsObservationError']) as got:f.scope['children'](42)
        detail=got.exception.observation
        self.assertEqual(detail['observedCandidatePIDs'],[100,101]);self.assertTrue(detail['lineageUnknown'])
        self.assertEqual(detail['operation'],'root-disappeared-after-observed-children')
        self.assertEqual(detail['tid'],43);self.assertTrue(detail['candidatesNotProvenGone'])
        f=self.fixture({**changes,('read','/proc/42/task/43/children'):PermissionError(13,'later TID unreadable')})
        with self.assertRaises(f.scope['ProcfsObservationError']) as got:f.scope['children'](42)
        self.assertEqual(got.exception.observation['observedCandidatePIDs'],[100,101])
        self.assertEqual(got.exception.observation['errno'],13)
        self.assertTrue(got.exception.observation['candidatesNotProvenGone'])
        f=self.fixture({**changes,('read','/proc/42/task/42/children'):''},birth_override=lambda pid:None)
        self.assertEqual(f.scope['children'](42),[]) # no known PID is being discarded
    def test_post_fence_unknown_retains_candidates_and_original_error_chain(self):
        for path,stage in [('/proc/42/status','read-pid-status'),
            ('/proc/sys/kernel/random/boot_id','read-global-boot-id')]:
            f=self.fixture({('read','/proc/42/task/42/children'):'100'})
            original=f.values[('read',path)];f.values[('read',path)]=[original,PermissionError(13,'fake post-fence observation unknown')]
            r=self.run_watch_once(f);self.assertEqual(len(r.watch_errors),1)
            detail=r.watch_errors.details[0]
            self.assertEqual(detail['procfs']['observedCandidatePIDs'],[100])
            self.assertEqual(detail['procfs']['operation'],'root-recheck-failed-after-observed-children')
            self.assertTrue(detail['procfs']['lineageUnknown']);self.assertTrue(detail['procfs']['candidatesNotProvenGone'])
            self.assertEqual(detail['errno'],13);self.assertIn(stage,detail['traceback'])
            self.assertIn('PermissionError',detail['traceback']);self.assertFalse(detail['procfs']['signalsAllowed'])
            f=self.fixture();original=f.values[('read',path)]
            f.values[('read',path)]=[original,PermissionError(13,'unknown with genuinely empty child observation')]
            r=self.run_watch_once(f);self.assertEqual(len(r.watch_errors),1)
            self.assertEqual(r.watch_errors.details[0]['procfs']['operation'],stage)
    def test_error_detail_bound_and_formatter_failure_never_clear_fatal_strings(self):
        f=self.fixture();errors=f.scope['WatchErrors']()
        for _ in range(65):
            try:raise OSError(5,'fake observer unknown')
            except OSError as error:errors.append(type(error).__name__+': '+str(error))
        self.assertEqual(len(errors),65);self.assertEqual(len(errors.details),64);self.assertEqual(errors.details_dropped,1)
        def broken(error):raise RuntimeError('fake diagnostic formatter failed')
        f.scope['observation_error']=broken;errors=f.scope['WatchErrors']()
        try:raise PermissionError(13,'fake denied')
        except PermissionError as error:errors.append(str(error))
        self.assertEqual(len(errors),1);self.assertTrue(errors.details[0]['diagnosticIncomplete'])
    def test_unexpected_watch_return_or_error_cannot_qualify_finish_or_cold(self):
        pf=self.fixture();r=pf.scope['Runner'].__new__(pf.scope['Runner'])
        r.watch_stop=types.SimpleNamespace(is_set=lambda:False);r.watch_errors=pf.scope['WatchErrors']()
        r.observe=lambda:None;r.watch_loop()
        self.assertFalse(r._watch_loop_completed);self.assertEqual(r.watch_errors,[])
        r.watch_stop=types.SimpleNamespace(is_set=lambda:True);r.watch_loop();self.assertTrue(r._watch_loop_completed)
        def broken():raise PermissionError(13,'fake observer terminated before requested closure')
        r.observe=broken;r.watch_loop();self.assertFalse(r._watch_loop_completed)
        self.assertEqual(len(r.watch_errors),1);self.assertEqual(r.watch_errors.details[0]['errno'],13)
        f=ToolSupervisionControls.fixture(self)
        f.r.watch=types.SimpleNamespace(join=lambda timeout:None,is_alive=lambda:False)
        f.r._watch_loop_completed=False;f.r.finish()
        self.assertEqual(f.r.report['exit'],1);self.assertTrue(f.r.report['processObservationUnqualified'])
        self.assertFalse(f.r.report['completeArchive']);self.assertTrue(f.r.report['mutableFailureSnapshot']);self.assertEqual(f.archives,[])
        f=ToolSupervisionControls.fixture(self);f.r.endpoint=f.r.runtime/'never-a-socket';f.r.watch_errors=['original observation Unknown']
        with self.assertRaisesRegex(AssertionError,'process observation Unknown'):f.r.archive_cold('must-not-copy')


if __name__=='__main__':
    assert not sys.flags.optimize
    unittest.main(verbosity=2)
