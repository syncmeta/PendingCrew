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
        runner.proc=runner.owner=runner.human=runner.watch=None;runner._pending_tool=None
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


if __name__=='__main__':
    assert not sys.flags.optimize
    unittest.main(verbosity=2)
