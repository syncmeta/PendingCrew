#!/usr/bin/env python3
"""Affected installer controls; optional offline systemd parser, never provider execution."""
import argparse, hashlib, importlib.util, io, json, os, pathlib, shutil, subprocess, sys, tarfile, tempfile, types, unittest
from unittest import mock
sys.dont_write_bytecode = True
repo = pathlib.Path(__file__).resolve().parents[2]
# Source path is explicit for private author runs, otherwise this lives in scripts/dev.
source = pathlib.Path(os.environ.get('PCW_HOST_INSTALLER_SOURCE', repo / 'deploy/linux/pendingcrew-host.py'))
spec = importlib.util.spec_from_file_location('pendingcrew_host', source)
host = importlib.util.module_from_spec(spec); spec.loader.exec_module(host)

class InstallerControls(unittest.TestCase):
    def setUp(self):
        # Existing protocol fixtures never write a real machine command entry.
        patch = mock.patch.object(host, "command_link")
        patch.start(); self.addCleanup(patch.stop)

    def test_draft_unknown_preserves_exact_owner_packet_and_only_original_lookup_or_fence(self):
        request='11111111-1111-4111-8111-111111111111'
        params={'requestId':request,'dbId':'opaque-core-db','runtimeBindingId':'22222222-2222-4222-8222-222222222222','params':{'ownerEpoch':'private-stamp','expectedSpaceId':None,'expectedSpaceRevision':'0','localEnabled':False,'acknowledgeRestore':False}}
        records=[]; calls=[]
        def rpc(method,body): calls.append((method,body)); raise OSError('synthetic response loss')
        with mock.patch.object(host,'read_record',return_value=params),mock.patch.object(host,'private_root'),mock.patch.object(host,'new_record',side_effect=lambda path,value:records.append(value)),mock.patch.object(host,'rpc',side_effect=rpc):
            with self.assertRaises(OSError):host.draft_control(types.SimpleNamespace(operation_file='owned0600',kind='owner'))
        self.assertEqual(len(records),1);record=records[0];original=record['original']
        self.assertEqual(original['command'],{'operation':'personalDraft.ownerConfigure','params':params['params']})
        self.assertEqual(calls, [('host.personalDraftOwnerConfigure',params)])
        calls.clear()
        with mock.patch.object(host,'read_record',return_value=record),mock.patch.object(host,'rpc',side_effect=lambda method,body:calls.append((method,body))),mock.patch('sys.stdout',new_callable=io.StringIO):
            host.draft_lookup(types.SimpleNamespace(request_id=request));host.draft_fence(types.SimpleNamespace(request_id=request))
        self.assertEqual(calls,[('host.personalDraftRequestLookup',{'requestId':request,'dbId':params['dbId']}),('host.personalDraftRequestFence',{'original':original})])
        record['original']['runtimeBindingId']='33333333-3333-4333-8333-333333333333'
        with mock.patch.object(host,'read_record',return_value=record),mock.patch.object(host,'rpc') as send:
            with self.assertRaises(host.Refused):host.draft_fence(types.SimpleNamespace(request_id=request))
            send.assert_not_called()
    def test_draft_root_owner_never_creates_local_writer_or_general_command_proxy(self):
        params={'requestId':'11111111-1111-4111-8111-111111111111','dbId':'opaque-core-db','runtimeBindingId':'22222222-2222-4222-8222-222222222222','params':{'ownerEpoch':'private-stamp','expectedSpaceId':None,'expectedSpaceRevision':'0','localEnabled':True,'acknowledgeRestore':False}}
        with mock.patch.object(host,'read_record',return_value=params),mock.patch.object(host,'new_record') as write,mock.patch.object(host,'rpc') as send:
            with self.assertRaises(host.Refused):host.draft_control(types.SimpleNamespace(operation_file='owned0600',kind='owner'))
            write.assert_not_called();send.assert_not_called()
        args=host.parser().parse_args(['draft-control','--kind','member','--operation-file','owned0600'])
        self.assertEqual(args.kind,'member')
        record={'format':1,'kind':'personalDraftControl','original':{'requestId':params['requestId'],'command':{'operation':'session.resume'}},'payloadSha256':'invalid'}
        with mock.patch.object(host,'read_record',return_value=record),mock.patch.object(host,'rpc') as send:
            with self.assertRaises(host.Refused):host.draft_lookup(types.SimpleNamespace(request_id=params['requestId']))
            send.assert_not_called()
    def test_notification_unknown_saves_only_original_digest_and_lookup_never_resends(self):
        request='11111111-1111-4111-8111-111111111111'
        params={'requestId':request,'expectedRevision':'0','enabled':True,'provider':{'privateKeyPkcs8Base64':'DO-NOT-PERSIST-KEY'}}
        records=[]; calls=[]
        def write(path,value): records.append(value)
        def rpc(method,body): calls.append((method,body)); raise OSError('synthetic response loss')
        with mock.patch.object(host.sys,'stdin',types.SimpleNamespace(buffer=io.BytesIO(json.dumps(params).encode()))),mock.patch.object(host,'private_root'),mock.patch.object(host,'new_record',side_effect=write),mock.patch.object(host,'rpc',side_effect=rpc):
            with self.assertRaises(OSError): host.notification_configure(types.SimpleNamespace(input_file=None))
        self.assertEqual(len(records),1);self.assertEqual(records[0]['requestId'],request)
        self.assertNotIn('DO-NOT-PERSIST-KEY',json.dumps(records));self.assertNotIn('provider',records[0]);self.assertEqual(calls[0][0],'host.notificationConfigure')
        with mock.patch.object(host,'read_record',return_value=records[0]),mock.patch.object(host,'rpc',return_value={'kind':'unknown','operation':'notification.configure'}) as read,mock.patch('sys.stdout',new_callable=io.StringIO):
            host.notification_lookup(types.SimpleNamespace(request_id=request))
        read.assert_called_once_with('host.notificationRequestLookup',{'requestId':request})
    def test_notification_secret_never_has_an_argv_or_environment_option(self):
        parsed=host.parser().parse_args(['notification-configure'])
        self.assertIsNone(parsed.input_file)
        with mock.patch('sys.stderr',new_callable=io.StringIO):
            with self.assertRaises(SystemExit):host.parser().parse_args(['notification-configure','--private-key','DO-NOT-ACCEPT'])
        with mock.patch.object(host.sys,'stdin',types.SimpleNamespace(buffer=io.BytesIO(b'x'*16385))),mock.patch.object(host,'new_record') as write,mock.patch.object(host,'rpc') as request:
            with self.assertRaises(host.Refused):host.notification_configure(types.SimpleNamespace(input_file=None))
            write.assert_not_called();request.assert_not_called()

    def provider_runner(self, outcomes=None):
        outcomes = outcomes or {}
        calls = []
        def run(argv, **kwargs):
            self.assertNotIn('env', kwargs)
            self.assertEqual(argv[:2], ['/usr/bin/systemd-run', '--quiet'])
            command = argv[argv.index('--') + 1:]
            calls.append(command)
            name = command[-1] if command[0] == '/bin/sh' else pathlib.Path(command[0]).name
            if command[0] == '/bin/sh':
                code, output = (1, b'') if outcomes.get(name) == 'missing' else (0, str(paths[name]).encode())
            elif command[0] == '/usr/bin/test': code, output = 1, b''
            elif command[1:] == ['--version']:
                if outcomes.get(name) == 'timeout': raise subprocess.TimeoutExpired(argv, 20, output=b'SECRET')
                code, output = 0, (b'codex-cli 0.164.0' if name == 'codex' else b'2.1.301 (Claude Code)')
                if outcomes.get(name) == 'identity': output = b'other-cli 0.164.0 SECRET'
                if outcomes.get(name) == 'version': output = b'2.1.302 (Claude Code)'
            elif command[1:] == ['app-server', '--help']:
                code, output = 0, b'Usage: codex app-server [OPTIONS] [COMMAND]'
                if outcomes.get(name) == 'interface': output = b'Usage: codex [OPTIONS] [PROMPT] SECRET'
            elif command[1:] in (['login', 'status'], ['auth', 'status']):
                code, output = (1, b'SECRET') if outcomes.get(name) == 'login' else (0, b'SECRET')
            else: self.fail('unexpected provider operation: ' + repr(command))
            return subprocess.CompletedProcess(argv, code, output, b'SECRET')
        temporary = tempfile.TemporaryDirectory(); self.addCleanup(temporary.cleanup)
        paths = {name: pathlib.Path(temporary.name) / name for name in ['codex', 'claude']}
        for path in paths.values(): path.touch()
        return run, calls

    def test_codex_interface_prerequisite_accepts_new_version_without_inference(self):
        run, calls = self.provider_runner()
        account = types.SimpleNamespace(pw_uid=1000, pw_gid=1000, pw_dir='/home/ordinary')
        tools = host.provider_preflight(account, {}, runner=run)
        self.assertEqual(set(tools), {'codex', 'claude'})
        self.assertIn([tools['codex'], 'app-server', '--help'], calls)
        self.assertEqual(len(calls), 7)

    def test_preflight_collects_both_failures_without_echo_or_persistent_write(self):
        account = types.SimpleNamespace(pw_uid=1000, pw_gid=1000, pw_dir='/home/ordinary')
        for outcomes in [{'codex':'missing','claude':'missing'}, {'codex':'login','claude':'login'},
                         {'codex':'interface','claude':'version'}, {'codex':'identity','claude':'login'},
                         {'codex':'timeout','claude':'missing'}]:
            with self.subTest(outcomes=outcomes):
                run, calls = self.provider_runner(outcomes)
                original = host.provider_preflight
                with mock.patch.object(host,'service_user',return_value=account), mock.patch.object(host,'provider_preflight',side_effect=lambda a,s:original(a,s,runner=run)), mock.patch.object(host,'root_code_directory') as write, mock.patch.object(host,'download') as network:
                    with self.assertRaises(host.Refused) as refusal:
                        host.install(types.SimpleNamespace(user='ordinary',codex=None,claude=None))
                    message = str(refusal.exception)
                    self.assertIn('codex:', message); self.assertIn('claude:', message)
                    self.assertNotIn('SECRET', message)
                    self.assertIn('codex login --device-auth', message)
                    self.assertIn('claude auth login', message)
                    write.assert_not_called(); network.assert_not_called()
                self.assertTrue(any(command[-1] == 'claude' for command in calls if command[0] == '/bin/sh'))
    def test_policy_mismatch_refuses_before_persistent_installation(self):
        manifest = {'format':1,'artifact':{'architecture':'x86_64','sourceCommit':'a'*40,'url':'https://example.invalid/'+'a'*40+'/host.tar.gz'},'providerPrerequisites':host.PROVIDER_POLICY}
        host.validate_provider_policy(manifest)
        variants = [None, {}, {'codex': '0.164.0', 'claude': '2.1.301'}]
        weakened = json.loads(json.dumps(host.PROVIDER_POLICY)); weakened['claude']['exactVersion'] = '2.1.302 (Claude Code)'; variants.append(weakened)
        account = types.SimpleNamespace(pw_uid=1000,pw_gid=1000,pw_dir='/home/ordinary')
        for policy in variants:
            altered = dict(manifest, providerPrerequisites=policy)
            with self.subTest(policy=policy), mock.patch.object(host,'service_user',return_value=account), mock.patch.object(host,'provider_preflight',return_value={}), mock.patch.object(host,'active',return_value=False), mock.patch.object(host.platform,'machine',return_value='x86_64'), mock.patch.object(host,'download',return_value=json.dumps(altered).encode()), mock.patch.object(host,'root_code_directory') as write, mock.patch.object(host,'new_record') as record:
                with self.assertRaises(host.Refused):
                    host.install(types.SimpleNamespace(user='ordinary',codex=None,claude=None,manifest_url='https://example.invalid/'+'a'*40+'/manifest.json',manifest_sha256='0'*64))
                write.assert_not_called(); record.assert_not_called()

    def test_service_user_does_not_import_root_credentials_or_create_an_account(self):
        account = types.SimpleNamespace(pw_uid=0,pw_gid=0,pw_dir='/root',pw_shell='/bin/bash')
        with mock.patch.object(host.pwd,'getpwnam',return_value=account):
            with self.assertRaises(host.Refused): host.service_user('root')

    def original_status(self):
        return {'identity':{'authorityId':'11111111-1111-4111-8111-111111111111','dbId':'original-db','certificateDerBase64':'Y2VydA==','spkiSha256':'a'*64},'configuration':{'revision':'2','enabled':True,'listenHost':'127.0.0.1','port':58643},'endpoint':{'host':'127.0.0.1','port':58643},'listening':True}

    def test_codex_only_install_flags_are_explicit(self):
        args = host.parser().parse_args(['install','--user','ordinary','--advertise-host','127.0.0.1','--disable-claude','--preserve-connection-from','/root/private/status.json'])
        self.assertTrue(args.disable_claude); self.assertEqual(args.preserve_connection_from,'/root/private/status.json')
        defaults = host.parser().parse_args(['install','--advertise-host','example.invalid'])
        self.assertFalse(defaults.disable_claude); self.assertIsNone(defaults.preserve_connection_from)
        with mock.patch('sys.stderr',new_callable=io.StringIO):
            with self.assertRaises(SystemExit): host.parser().parse_args(['install','--advertise-host','127.0.0.1','--preserve-connection'])
            with self.assertRaises(SystemExit): host.parser().parse_args(['install','--advertise-host','127.0.0.1','--preserve-connection','/root/status.json'])

    def test_preserve_connection_never_configures_or_enrolls(self):
        saved = self.original_status()
        original = {'preservedConnection':saved,'listenHost':'127.0.0.1','port':58643,'configureRequestId':'MUST-NOT-USE','enrollmentRequestId':'MUST-NOT-USE'}
        calls = []
        def rpc(method,params):
            calls.append(method)
            if method == 'host.status': return saved
            if method == 'host.grants': return [{'id':'current-grant','revision':'1'}]
            self.fail('preservation attempted a mutation: '+method)
        with mock.patch.object(host,'rpc',side_effect=rpc),mock.patch.object(host.subprocess,'run'),mock.patch.object(host,'new_record') as write,mock.patch.object(host.secrets,'token_bytes') as token,mock.patch('sys.stdout',new_callable=io.StringIO) as output:
            host.finish_installation(original)
            write.assert_not_called(); token.assert_not_called(); self.assertNotIn('pendingcrew://',output.getvalue())
        self.assertEqual(calls,['host.status','host.grants'])

    def test_preserve_status_rejects_failure_incomplete_identity_or_changed_binding(self):
        saved = self.original_status()
        self.assertEqual(host.parse_preserved_status(saved,'127.0.0.1',58643),saved)
        for changed in ['failure','identity','pin','configuration','endpoint','listening']:
            with self.subTest(changed=changed):
                status = json.loads(json.dumps(saved))
                if changed == 'failure': status['failure']='private failure text'
                if changed == 'identity': del status['identity']['certificateDerBase64']
                if changed == 'pin': status['identity']['spkiSha256']='invalid'
                if changed == 'configuration': status['configuration']['enabled']=False
                if changed == 'endpoint': status['endpoint']['port']=58644
                if changed == 'listening': status['listening']=False
                with self.assertRaises(host.Refused): host.parse_preserved_status(status,'127.0.0.1',58643)
        changed = json.loads(json.dumps(saved)); changed['identity']['dbId']='another-db'
        def rpc(method,params):
            if method == 'host.status': return changed
            self.fail('mismatch caused more requests: '+method)
        with mock.patch.object(host,'rpc',side_effect=rpc),mock.patch.object(host.subprocess,'run'),mock.patch.object(host,'new_record') as write:
            with self.assertRaises(host.Refused): host.finish_installation({'preservedConnection':saved,'listenHost':'127.0.0.1','port':58643})
            write.assert_not_called()
        # An existing record is reused on continuation; no file is reinterpreted.
        for failed in [OSError('unknown status'),host.Refused('read denied')]:
            with mock.patch.object(host,'rpc',side_effect=failed),mock.patch.object(host.subprocess,'run'),mock.patch.object(host,'new_record') as write,mock.patch.object(host.time,'monotonic',side_effect=[0,0,16]),mock.patch.object(host.time,'sleep'):
                with self.assertRaises(host.Refused): host.finish_installation({'preservedConnection':saved})
                write.assert_not_called()

    def test_install_record_and_resume_keep_original_provider_and_status(self):
        with tempfile.TemporaryDirectory(prefix='pcwh-',dir='/private/tmp') as temporary:
            home=pathlib.Path(temporary);data=home/'data';data.mkdir(mode=0o700);(data/'crewd.db').touch(mode=0o600)
            account=types.SimpleNamespace(pw_uid=os.getuid(),pw_gid=os.getgid(),pw_dir=str(home),pw_name='ordinary')
            status=self.original_status()
            manifest={'format':1,'providerPrerequisites':host.PROVIDER_POLICY,'artifact':{'architecture':'x86_64','sourceCommit':'a'*40,'url':'https://example.invalid/'+'a'*40+'/host.tar.gz','sha256':'b'*64}}
            args=types.SimpleNamespace(user='ordinary',codex=None,claude=None,disable_claude=True,preserve_connection_from='/root/private/status.json',manifest_url='https://example.invalid/'+'a'*40+'/manifest.json',manifest_sha256='c'*64,data_dir=str(data),port=58643,listen_host='127.0.0.1',advertise_host='127.0.0.1',request_id=None,label='existing')
            captured=[]
            def record(path,value): captured.append(value); raise OSError('fixture stop before activation')
            with mock.patch.object(host,'service_user',return_value=account),mock.patch.object(host,'read_record',return_value=status),mock.patch.object(host,'provider_preflight',return_value={'codex':'/ordinary/codex'}),mock.patch.object(host,'active',return_value=False),mock.patch.object(host.platform,'machine',return_value='x86_64'),mock.patch.object(host,'download',return_value=json.dumps(manifest).encode()),mock.patch.object(host,'extract_release',return_value=(data,{'sourceCommit':'a'*40})),mock.patch.object(host,'transient',return_value=subprocess.CompletedProcess([],0,b'{"methods":{"remote.enrollmentCreate":{},"remote.enrollmentLookup":{}}}',b'')),mock.patch.object(host,'root_code_directory'),mock.patch.object(host,'private_root'),mock.patch.object(host,'pointer',return_value=None),mock.patch.object(host,'new_record',side_effect=record):
                with self.assertRaises(OSError): host.install(args)
            self.assertEqual(len(captured),1); original=captured[0]
            self.assertTrue(original['disableClaude']); self.assertEqual(set(original['providerPaths']),{'codex'})
            self.assertEqual(original['preservedConnection'],status)
            class BeforeUnits(Exception): pass
            with mock.patch.object(host,'read_record',return_value=original),mock.patch.object(host,'pointer',return_value='activations/'+original['requestId']),mock.patch.object(host.pwd,'getpwuid',return_value=account),mock.patch.object(host,'service_user',return_value=account),mock.patch.object(host,'provider_preflight',return_value={'codex':'/ordinary/codex'}) as preflight,mock.patch.object(host,'units',side_effect=BeforeUnits) as units:
                with self.assertRaises(BeforeUnits): host.resume_installation(types.SimpleNamespace(request_id=original['requestId']))
                preflight.assert_called_once_with(account,original['providerPaths'],disable_claude=True)
                self.assertTrue(units.call_args.kwargs['disable_claude'])
            original['providerPaths']['claude']='/injected/claude'
            with mock.patch.object(host,'read_record',return_value=original),mock.patch.object(host,'pointer',return_value='activations/'+original['requestId']),mock.patch.object(host.pwd,'getpwuid',return_value=account),mock.patch.object(host,'service_user',return_value=account),mock.patch.object(host,'provider_preflight') as preflight:
                with self.assertRaises(host.Refused): host.resume_installation(types.SimpleNamespace(request_id=original['requestId']))
                preflight.assert_not_called()

    def test_preserve_input_refusal_precedes_checks_and_persistent_install(self):
        account=types.SimpleNamespace(pw_uid=1000,pw_gid=1000,pw_dir='/home/ordinary')
        args=types.SimpleNamespace(user='ordinary',preserve_connection_from='/root/private/status.json',listen_host='127.0.0.1',port=58643)
        for failure in [host.Refused('root0600 input required'),OSError('input missing')]:
            with mock.patch.object(host,'service_user',return_value=account),mock.patch.object(host,'read_record',side_effect=failure),mock.patch.object(host,'provider_preflight') as provider,mock.patch.object(host,'download') as network,mock.patch.object(host,'root_code_directory') as write:
                with self.assertRaises(type(failure)): host.install(args)
                provider.assert_not_called(); network.assert_not_called(); write.assert_not_called()
        with mock.patch.object(host,'service_user',return_value=account),mock.patch.object(host,'read_record',return_value={'failure':'not ready'}),mock.patch.object(host,'provider_preflight') as provider,mock.patch.object(host,'root_code_directory') as write:
            with self.assertRaises(host.Refused): host.install(args)
            provider.assert_not_called();write.assert_not_called()

    def test_default_new_install_keeps_one_original_configuration_and_link(self):
        with tempfile.TemporaryDirectory() as temporary:
            configuration={'revision':'2','enabled':True,'listenHost':'127.0.0.1','port':49381}
            original={'configureRequestId':'config-original','enrollmentRequestId':'enroll-original','enrollmentId':'original-enrollment','listenHost':'127.0.0.1','advertiseHost':'host.invalid','port':49381,'label':'Host'}
            calls=[]
            def rpc(method,params):
                calls.append((method,params))
                if method=='host.status':return {'configuration':configuration,'listening':True}
                if method=='host.configure':return configuration
                if method=='host.enrollmentCreate':return {'original':'result'}
                self.fail('unexpected operation '+method)
            record={'params':{'requestId':'enroll-original','enrollmentId':'original-enrollment'},'token':'synthetic'}
            with mock.patch.object(host,'STATE',pathlib.Path(temporary)),mock.patch.object(host.subprocess,'run'),mock.patch.object(host,'rpc',side_effect=rpc),mock.patch.object(host,'new_record') as write,mock.patch.object(host,'pair_record',return_value=record),mock.patch.object(host,'enrolled_link',return_value='pendingcrew://synthetic'),mock.patch('sys.stdout',new_callable=io.StringIO) as output:
                host.finish_installation(original)
                self.assertEqual(output.getvalue().strip(),'pendingcrew://synthetic');self.assertEqual(write.call_count,2)
            self.assertEqual([method for method,_ in calls].count('host.configure'),1)
            self.assertEqual([method for method,_ in calls].count('host.enrollmentCreate'),1)
            self.assertEqual([params['requestId'] for method,params in calls if method=='host.enrollmentCreate'],['enroll-original'])

    def test_codex_only_skips_claude_and_preserves_explicit_disabled_argv(self):
        run, calls = self.provider_runner({'claude':'missing'})
        account = types.SimpleNamespace(pw_uid=1000,pw_gid=1000,pw_dir='/home/ordinary')
        tools = host.provider_preflight(account,{},runner=run,disable_claude=True)
        self.assertEqual(set(tools),{'codex'}); self.assertFalse(any(command[-1]=='claude' for command in calls))
        socket_unit, service = host.units(account,pathlib.Path('/home/ordinary/data'),tools,disable_claude=True)
        self.assertIn('"--disable-claude"',service); self.assertNotIn('"--claude"',service)
        self.assertIn('User=1000',service); self.assertIn('Restart=on-failure',service)
        with self.assertRaises(host.Refused): host.provider_preflight(account,{'claude':'/explicit/claude'},runner=run,disable_claude=True)

    def test_managed_wrapper_disabled_selection_and_child_environment(self):
        spec = importlib.util.spec_from_file_location('managed_runtime',source.with_name('managed-host.py'))
        runtime = importlib.util.module_from_spec(spec); spec.loader.exec_module(runtime)
        base=['managed-host','--prefix','/opt/pendingcrew','--data-dir','/home/ordinary/data','--system-admin-socket','/run/pendingcrew/admin.sock','--codex','/ordinary/codex']
        with mock.patch.object(runtime.sys,'argv',base+['--disable-claude']),mock.patch.object(runtime,'run') as run:
            runtime.main(); args=run.call_args.args[0]; self.assertTrue(args.disable_claude)
        with mock.patch.object(runtime.sys,'argv',base+['--disable-claude','--claude','/ordinary/claude']),mock.patch('sys.stderr',new_callable=io.StringIO):
            with self.assertRaises(SystemExit): runtime.main()
        env={'LISTEN_PID':str(os.getpid()),'LISTEN_FDS':'1','LISTEN_FDNAMES':'pcw-admin','CREWD_CLAUDE':'/should/not/enable','CREWD_CODEX':'/should/not/override','HOME':'/home/ordinary','PATH':'/ordinary/bin'}
        with mock.patch.dict(runtime.os.environ,env,clear=True),mock.patch.object(runtime.os,'getuid',return_value=1000),mock.patch.object(runtime.os,'geteuid',return_value=1000),mock.patch.object(runtime.os,'open',return_value=99),mock.patch.object(runtime.os,'fstat',return_value=types.SimpleNamespace(st_mode=0o100644,st_uid=0)),mock.patch.object(runtime.fcntl,'flock'),mock.patch.object(runtime,'core_path',return_value=pathlib.Path('/immutable/crewd')),mock.patch.object(runtime.os,'set_inheritable'),mock.patch.object(runtime.os,'execve') as execute:
            runtime.run(args)
            binary, argv, child = execute.call_args.args
            self.assertEqual(argv[-1],'--disable-claude'); self.assertNotIn('--claude',argv)
            self.assertNotIn('CREWD_CLAUDE',child); self.assertNotIn('CREWD_CODEX',child)
            self.assertEqual(child['HOME'],env['HOME']); self.assertEqual(child['PATH'],env['PATH']); self.assertEqual(child['LISTEN_PID'],env['LISTEN_PID'])
            self.assertEqual(runtime.os.environ['CREWD_CLAUDE'],env['CREWD_CLAUDE'])
        with mock.patch.object(runtime.os,'getuid',return_value=0):
            with self.assertRaises(ValueError): runtime.run(args)

    def test_canonical_operation_paths_refuse_traversal(self):
        for value in ['../../etc/passwd', '../' + '0'*36, 'A'*36, '0'*36, ' 11111111-1111-4111-8111-111111111111']:
            with self.assertRaises((host.Refused, ValueError)): host.canonical_uuid(value)
        value = '11111111-1111-4111-8111-111111111111'
        self.assertEqual(host.canonical_uuid(value), value)
    def test_archive_escape_and_link_never_extract(self):
        for name, kind in [('../escape', tarfile.REGTYPE), ('bin/core', tarfile.SYMTYPE), ('/absolute', tarfile.REGTYPE)]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                root = pathlib.Path(temporary); stream = io.BytesIO()
                with tarfile.open(fileobj=stream, mode='w') as bundle:
                    member = tarfile.TarInfo(name); member.type = kind; member.linkname = '/etc/passwd'; bundle.addfile(member)
                with self.assertRaises(host.Refused): host.extract_release(stream.getvalue(), root, {})
                self.assertFalse((root.parent/'escape').exists())
    def test_offline_rollback_header_never_writes_or_recovers_wal(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary); db = root/'crewd.db'; wal = root/'crewd.db-wal'
            header = bytearray(100); header[:16] = b'SQLite format 3\0'; header[60:64] = (18).to_bytes(4, 'big'); header[68:72] = (0x43525744).to_bytes(4, 'big'); db.write_bytes(header)
            before = db.read_bytes(); self.assertEqual(host.stopped_database_version(root), 18)
            wal.write_bytes(b'pending'); preserved = wal.read_bytes()
            with self.assertRaises(host.Refused): host.stopped_database_version(root)
            self.assertEqual(db.read_bytes(), before); self.assertEqual(wal.read_bytes(), preserved)
            wal.unlink(); db.unlink(); db.symlink_to('/etc/passwd')
            with self.assertRaises(OSError): host.stopped_database_version(root)
    def test_package_extract_exact_manifest_and_umask_preserve_runtime_access(self):
        package_spec = importlib.util.spec_from_file_location('host_package', source.with_name('package-host-release.py'))
        package_module = importlib.util.module_from_spec(package_spec); package_spec.loader.exec_module(package_module)
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary); binary = root/'synthetic-elf'; proof = root/'proof.json'
            # Synthetic header is package-shape input only, never executable/runtime proof.
            header = bytearray(64); header[:6] = b'\x7fELF\x02\x01'; header[18:20] = (62).to_bytes(2,'little'); binary.write_bytes(header)
            commit = 'a'*40
            proof.write_text(json.dumps({'sourceCommit':commit,'binarySha256':host.sha(header),'sourceClean':True,'defaultFeatures':True,'features':[],'test':False}))
            args = types.SimpleNamespace(source=commit,architecture='x86_64',artifact_url='https://example.invalid/'+commit+'/host.tar.gz',manifest_url='https://example.invalid/'+commit+'/manifest.json',core=str(binary),proof=str(proof),output=str(root/'package'))
            with mock.patch.object(package_module,'verify_component_sources') as verify, mock.patch('sys.stdout', new_callable=io.StringIO):
                package_module.package(args)
                verify.assert_called_once()
            output = root/'package'; manifest = json.loads((output/'manifest.json').read_bytes()); payload = (output/'host.tar.gz').read_bytes()
            self.assertEqual(host.sha(payload),manifest['artifact']['sha256'])
            self.assertNotIn('coreSourceCommit',manifest['artifact'])
            self.assertEqual(manifest['providerPrerequisites'],host.PROVIDER_POLICY)
            host.validate_provider_policy(manifest)
            self.assertNotIn('providerQualification',manifest)
            extraction = root/'extract'; extraction.mkdir()
            previous = os.umask(0o077)
            try: unpacked, release = host.extract_release(payload,extraction,manifest['artifact'])
            finally: os.umask(previous)
            self.assertEqual((unpacked.stat().st_mode & 0o777),0o755)
            self.assertEqual(((unpacked/'bin').stat().st_mode & 0o777),0o755)
            self.assertEqual(((unpacked/'bin/crewd').stat().st_mode & 0o777),0o555)
            self.assertNotIn('coreSourceCommit',release)
            self.assertEqual(release['storeVersion'],21)
            self.assertEqual(release['files']['bin/crewd'],host.sha(header))
            self.assertIn(repr(host.sha((output/'manifest.json').read_bytes())),(output/'install.py').read_text())
    def mixed_package_fixture(self):
        temporary=tempfile.TemporaryDirectory(prefix='pcw-components-',dir='/private/tmp');self.addCleanup(temporary.cleanup)
        root=pathlib.Path(temporary.name);here=root/'deploy/linux';here.mkdir(parents=True)
        for name in ['package-host-release.py','pendingcrew-host.py','managed-host.py','install.sh']:
            (here/name).write_bytes((source.parent/name).read_bytes())
        (root/'crewd').mkdir();(root/'crewd/Cargo.toml').write_text('# synthetic source tree; never compiled\n')
        def git(*args):return subprocess.check_output(['git','-C',str(root),*args],stderr=subprocess.PIPE).decode().strip()
        git('init','-q');git('add','.');git('-c','user.name=Offline fixture','-c','user.email=fixture@invalid','commit','-qm','original component')
        core_source=git('rev-parse','HEAD')
        with (here/'pendingcrew-host.py').open('a') as stream:stream.write('\n# Offline package component fixture.\n')
        git('add','.');git('-c','user.name=Offline fixture','-c','user.email=fixture@invalid','commit','-qm','new Python component')
        package_source=git('rev-parse','HEAD')
        binary=root/'synthetic-elf';header=bytearray(64);header[:6]=b'\x7fELF\x02\x01';header[18:20]=(62).to_bytes(2,'little');binary.write_bytes(header)
        proof=root/'proof.json';proof.write_text(json.dumps({'sourceCommit':core_source,'binarySha256':host.sha(header),'sourceClean':True,'defaultFeatures':True,'features':[],'test':False}))
        spec=importlib.util.spec_from_file_location('mixed_package',here/'package-host-release.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
        args=types.SimpleNamespace(source=package_source,core_source=core_source,architecture='x86_64',artifact_url='https://example.invalid/'+package_source+'/host.tar.gz',manifest_url='https://example.invalid/'+package_source+'/manifest.json',core=str(binary),proof=str(proof),output=str(root/'package'))
        return root,git,module,args

    def test_release_bootstrap_binds_short_urls_exact_digest_and_elf_glibc(self):
        root,git,module,args=self.mixed_package_fixture()
        binary=pathlib.Path(args.core);binary.write_bytes(binary.read_bytes()+b'GLIBC_2.17\0GLIBC_2.39\0')
        proof=json.loads(pathlib.Path(args.proof).read_text());proof['binarySha256']=host.sha(binary.read_bytes());pathlib.Path(args.proof).write_text(json.dumps(proof))
        base='https://github.com/syncmeta/PendingCrew/releases/download/v0.2.2/'
        args.artifact_url=base+'pendingcrew-0.2.2-linux-amd64.tar.gz';args.manifest_url=base+'pendingcrew-0.2.2-linux-amd64.manifest.json';args.installer_url=base+'installer.py'
        with mock.patch('sys.stdout',new_callable=io.StringIO):module.package(args)
        out=pathlib.Path(args.output);entry=(out/'install.sh').read_text()
        self.assertIn(repr(args.installer_url),entry);self.assertIn(repr(host.sha((out/'install.py').read_bytes())),entry)
        self.assertIn('requires glibc 2.39 or newer',entry);self.assertNotIn('@MIN_GLIBC@',entry)
        manifest=json.loads((out/'manifest.json').read_text())
        self.assertEqual(manifest['artifact']['sha256'],host.sha((out/'host.tar.gz').read_bytes()))
        self.assertEqual(manifest['artifact']['coreSourceCommit'],args.core_source)

    def test_mixed_package_preserves_core_proof_and_extracts_with_explicit_sources(self):
        root,git,module,args=self.mixed_package_fixture()
        before=pathlib.Path(args.proof).read_bytes();binary=pathlib.Path(args.core).read_bytes()
        with mock.patch('sys.stdout',new_callable=io.StringIO):module.package(args)
        output=pathlib.Path(args.output);manifest=json.loads((output/'manifest.json').read_bytes())
        self.assertEqual(manifest['artifact']['sourceCommit'],args.source)
        self.assertEqual(manifest['artifact']['coreSourceCommit'],args.core_source)
        extraction=root/'extracted';extraction.mkdir()
        unpacked,release=host.extract_release((output/'host.tar.gz').read_bytes(),extraction,manifest['artifact'])
        self.assertEqual(release['sourceCommit'],args.source);self.assertEqual(release['coreSourceCommit'],args.core_source)
        self.assertEqual((unpacked/'build-proof.json').read_bytes(),before)
        self.assertEqual((unpacked/'bin/crewd').read_bytes(),binary)
        self.assertEqual((unpacked/'bin/pendingcrew-host.py').read_bytes(),git('show',args.source+':deploy/linux/pendingcrew-host.py').encode()+b'\n')

    def test_mixed_package_rejects_changed_tree_actual_blobs_or_relabelled_proof(self):
        for failure in ['different-tree','dirty-rust','dirty-helper','proof-source']:
            with self.subTest(failure=failure):
                root,git,module,args=self.mixed_package_fixture()
                if failure=='different-tree':
                    (root/'crewd/Cargo.toml').write_text('# different Rust input\n')
                    git('add','crewd');git('-c','user.name=Offline fixture','-c','user.email=fixture@invalid','commit','-qm','different Rust')
                    args.source=git('rev-parse','HEAD')
                    args.artifact_url='https://example.invalid/'+args.source+'/host.tar.gz';args.manifest_url='https://example.invalid/'+args.source+'/manifest.json'
                elif failure=='dirty-rust':(root/'crewd/Cargo.toml').write_text('# uncommitted Rust input\n')
                elif failure=='dirty-helper':
                    with (root/'deploy/linux/managed-host.py').open('a') as stream:stream.write('\n# uncommitted helper\n')
                else:
                    proof=json.loads(pathlib.Path(args.proof).read_bytes());proof['sourceCommit']=args.source;pathlib.Path(args.proof).write_text(json.dumps(proof))
                with self.assertRaises(ValueError),mock.patch('sys.stdout',new_callable=io.StringIO):module.package(args)
                self.assertFalse(pathlib.Path(args.output).exists())

    def test_mixed_extract_rejects_component_metadata_or_binary_proof_relabel(self):
        root,git,module,args=self.mixed_package_fixture()
        with mock.patch('sys.stdout',new_callable=io.StringIO):module.package(args)
        output=pathlib.Path(args.output);manifest=json.loads((output/'manifest.json').read_bytes())
        with tarfile.open(output/'host.tar.gz') as archive:
            original={member.name:archive.extractfile(member).read() for member in archive.getmembers()}
        for failure in ['release-component','proof-source','manifest-component-missing']:
            with self.subTest(failure=failure):
                files=dict(original);release=json.loads(files['release.json']);expected=dict(manifest['artifact'])
                if failure=='release-component':release['coreSourceCommit']='f'*40
                elif failure=='proof-source':
                    proof=json.loads(files['build-proof.json']);proof['sourceCommit']=args.source
                    files['build-proof.json']=json.dumps(proof).encode();release['files']['build-proof.json']=host.sha(files['build-proof.json'])
                else:del expected['coreSourceCommit']
                files['release.json']=json.dumps(release).encode();stream=io.BytesIO()
                with tarfile.open(fileobj=stream,mode='w:gz') as archive:
                    for name,data in files.items():
                        member=tarfile.TarInfo(name);member.size=len(data);archive.addfile(member,io.BytesIO(data))
                extraction=root/failure;extraction.mkdir()
                with self.assertRaises(host.Refused):host.extract_release(stream.getvalue(),extraction,expected)

    def test_activation_pointer_is_bounded_and_never_follows_arbitrary_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=pathlib.Path(temporary); current=root/'current'
            self.assertIsNone(host.pointer(root))
            for target in ['../../etc','/opt/other','activations/../releases','activations/'+'x'*36]:
                current.symlink_to(target)
                with self.assertRaises(host.Refused): host.pointer(root)
                current.unlink()
            target='activations/11111111-1111-4111-8111-111111111111'; current.symlink_to(target)
            self.assertEqual(host.pointer(root),target)
    def test_install_continuation_unknown_uses_original_lookup_without_resend(self):
        original={'configureRequestId':'config-original','enrollmentRequestId':'enroll-original','enrollmentId':'token-original','label':'Host','advertiseHost':'host.invalid','listenHost':'127.0.0.1','port':49381}
        configuration={'revision':'2','enabled':True,'listenHost':'127.0.0.1','port':49381}
        status={'configuration':configuration,'listening':True}
        config_record={'format':1,'kind':'configuration','params':{'requestId':'config-original'}}
        enrollment_record={'format':1,'kind':'enrollmentCreation','token':'original-secret','params':{'requestId':'enroll-original','enrollmentId':'token-original'}}
        for missing in ['configuration','enrollment']:
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as temporary:
                state=pathlib.Path(temporary)
                (state/'configure-config-original.json').touch(); (state/'enroll-enroll-original.json').touch()
                calls=[]
                def rpc(method,params):
                    calls.append(method)
                    if method=='host.status': return status
                    if method=='host.configurationLookup': return {'state':'notFound'} if missing=='configuration' else {'state':'recorded','configuration':configuration}
                    if method=='host.enrollmentLookup': return {'state':'notFound'}
                    self.fail('unknown original result caused a mutation: '+method)
                def record(path): return config_record if path.name.startswith('configure-') else enrollment_record
                with mock.patch.object(host,'STATE',state),mock.patch.object(host,'read_record',side_effect=record),mock.patch.object(host,'rpc',side_effect=rpc),mock.patch.object(host.subprocess,'run'),mock.patch.object(host.secrets,'token_bytes') as token,mock.patch.object(host,'new_record') as write:
                    with self.assertRaises(host.Refused): host.finish_installation(original)
                    token.assert_not_called();write.assert_not_called()
                self.assertIn('host.configurationLookup',calls)
                self.assertNotIn('host.configure',calls); self.assertNotIn('host.enrollmentCreate',calls)
    def test_same_original_data_lease_excludes_concurrent_activation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=pathlib.Path(temporary); root.chmod(0o700)
            first=host.data_lease(root,os.getuid(),os.getgid())
            try:
                self.assertEqual({p.name for p in root.iterdir()},{'crewd.lock'})
                with self.assertRaises(BlockingIOError): host.data_lease(root,os.getuid(),os.getgid())
            finally: os.close(first)
            second=host.data_lease(root,os.getuid(),os.getgid()); os.close(second)
    def test_unit_preserves_environment_and_root_socket_floor(self):
        account = types.SimpleNamespace(pw_uid=1000,pw_gid=1000,pw_dir='/home/ordinary')
        socket_unit, service = host.units(account, pathlib.Path('/home/ordinary/.local/share/PendingCrew'), {'codex':'/home/ordinary/.local/bin/codex','claude':'/home/ordinary/.local/bin/claude'})
        self.assertIn('User=1000', service); self.assertIn('SocketMode=0600', socket_unit); self.assertIn('DirectoryMode=0700', socket_unit)
        self.assertNotIn('Environment=', service); self.assertNotIn('SetLoginEnvironment=', service)
        self.assertIn('SendSIGKILL=no', service); self.assertIn('KillMode=process', service)
        self.assertIn('WorkingDirectory=/home/ordinary\n', service)
    @unittest.skipUnless(shutil.which('systemd-analyze'), 'real offline systemd parser requires Linux')
    def test_working_directory_is_accepted_by_real_systemd_parser(self):
        # verify parses units without loading, starting, or enabling a service.
        for home in ['/home/pendingcrew', '/home/ordinary space%name']:
            with self.subTest(home=home), tempfile.TemporaryDirectory() as directory:
                account = types.SimpleNamespace(pw_uid=1000, pw_gid=1000, pw_dir=home)
                socket_unit, service = host.units(account, pathlib.Path(home)/'.local/share/PendingCrew', {'codex':'/usr/bin/true'}, disable_claude=True)
                root = pathlib.Path(directory)
                (root/'pendingcrew.socket').write_text(socket_unit)
                (root/'pendingcrew.service').write_text(service)
                result = subprocess.run(['systemd-analyze', '--man=no', 'verify', str(root/'pendingcrew.socket'), str(root/'pendingcrew.service')], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)
                self.assertEqual(result.returncode, 0, result.stderr.decode(errors='replace'))
    def test_original_configuration_lookup_never_applies_or_recreates_token(self):
        args = types.SimpleNamespace(operation_file='/root/original.json')
        original = {'format':1,'kind':'enrollmentCreation','token':'saved-secret','params':{'requestId':'r','enrollmentId':'e'}}
        with mock.patch.object(host,'read_record',return_value=original), mock.patch.object(host,'rpc',return_value={'state':'notFound'}) as call, mock.patch.object(host.secrets,'token_bytes') as fresh:
            with self.assertRaises(host.Refused): host.recover(args)
            call.assert_called_once_with('host.enrollmentLookup', {'requestId':'r','enrollmentId':'e'})
            fresh.assert_not_called()

class ReviewedBuildBindingControls(unittest.TestCase):
    """Offline controls with temporary Git inputs, never a real Actions receipt."""
    def setUp(self):
        spec = importlib.util.spec_from_file_location('reviewed_host_build', repo/'scripts/dev/build-crewd-host-release.py')
        self.build = importlib.util.module_from_spec(spec); spec.loader.exec_module(self.build)
        self.temporary = tempfile.TemporaryDirectory(); self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        for name in set(self.build.SOURCE_RECIPES + self.build.CONTROLLER_RECIPES + ['crewd/schema/crewd-api.schema.json','crewd/crates/store/src/schema.rs']):
            target = self.root/name; target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes((repo/name).read_bytes())
        for args in [['init','-q'],['add','.'],['-c','user.name=Offline fixture','-c','user.email=fixture@invalid','commit','-qm','offline binding fixture']]:
            subprocess.run(['git','-C',str(self.root),*args],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        self.ref = self.build.git(self.root,'rev-parse','HEAD').decode().strip()
        self.tree = self.build.git(self.root,'rev-parse','HEAD:crewd').decode().strip()
        self.environment = {'SOURCE_SHA':self.ref,'EXPECTED_RUST_TREE':self.tree,'GITHUB_SHA':self.ref,
                            'GITHUB_ACTIONS':'true','GITHUB_EVENT_NAME':'workflow_dispatch',
                            'GITHUB_RUN_ID':'123','GITHUB_RUN_ATTEMPT':'1','CARGO_INCREMENTAL':'0'}

    def test_exact_manual_source_tree_and_full_inputs_are_bound(self):
        with mock.patch.dict(os.environ,self.environment,clear=True):
            result = self.build.binding(self.root,self.root,self.ref,self.ref)
        self.assertEqual(result['sourceSha'],self.ref)
        self.assertEqual(result['rustTree'],self.tree)
        self.assertEqual(result['runId'],'123')
        self.assertEqual(result['sourceInputs'],result['controllerInputs'])

    def test_committed_symlink_binds_raw_target_without_following(self):
        name = 'design/prototypes/2026-10-24-native-team/schema-source'
        link = self.root/name; link.parent.mkdir(parents=True)
        target = os.readlink(repo/name)
        link.symlink_to(target)
        dangling = self.root/'unresolved-source'; dangling.symlink_to('missing-input')
        for args in [['add','.'],['-c','user.name=Offline fixture','-c','user.email=fixture@invalid','commit','-qm','committed link inputs']]:
            subprocess.run(['git','-C',str(self.root),*args],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        self.ref = self.build.git(self.root,'rev-parse','HEAD').decode().strip()
        self.environment.update(SOURCE_SHA=self.ref,GITHUB_SHA=self.ref)
        with mock.patch.dict(os.environ,self.environment,clear=True):
            result = self.build.binding(self.root,self.root,self.ref,self.ref)
        rows = {row['path']:row for row in result['sourceInputs']}
        for path, raw in [(name,os.fsencode(target)),('unresolved-source',b'missing-input')]:
            self.assertEqual(rows[path],{'path':path,'sha256':hashlib.sha256(raw).hexdigest(),'bytes':len(raw),'gitMode':'120000'})
        self.assertEqual(result['sourceInputs'],result['controllerInputs'])

    def test_inventory_rejects_changed_raw_target_type_or_executable_mode(self):
        link = self.root/'source-link'; link.symlink_to('missing-original')
        regular = self.root/'regular-input'; regular.write_bytes(b'ordinary input')
        executable = self.root/'executable-input'; executable.write_bytes(b'executable input'); executable.chmod(0o755)
        for args in [['add','.'],['-c','user.name=Offline fixture','-c','user.email=fixture@invalid','commit','-qm','input mode controls']]:
            subprocess.run(['git','-C',str(self.root),*args],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        ref = self.build.git(self.root,'rev-parse','HEAD').decode().strip()
        original_git = self.build.git
        def status_clean(root,*args):
            # Exercise the blob/type/mode guards independently of the earlier dirty-tree gate.
            return b'' if args == ('status','--porcelain') else original_git(root,*args)
        cases = [('link-target',link,lambda:link.symlink_to('missing-modified')),
                 ('link-to-file',link,lambda:link.write_bytes(b'missing-original')),
                 ('file-to-link',regular,lambda:regular.symlink_to('missing-original')),
                 ('file-bytes',regular,lambda:regular.write_bytes(b'changed input')),
                 ('executable-mode',executable,lambda:executable.chmod(0o644))]
        for name,path,change in cases:
            with self.subTest(name=name):
                subprocess.run(['git','-C',str(self.root),'restore','--worktree','.'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
                if name != 'executable-mode': path.unlink()
                change()
                with mock.patch.object(self.build,'git',side_effect=status_clean):
                    with self.assertRaises(AssertionError): self.build.inventory(self.root,ref)

    def test_missing_or_changed_review_and_execution_context_are_refused(self):
        for key, value in [('SOURCE_SHA',None),('SOURCE_SHA','f'*40),('EXPECTED_RUST_TREE',None),
                           ('EXPECTED_RUST_TREE','bad'),('EXPECTED_RUST_TREE','f'*40),
                           ('GITHUB_SHA','f'*40),('GITHUB_ACTIONS','false'),
                           ('GITHUB_EVENT_NAME','push'),('GITHUB_RUN_ID',''),
                           ('GITHUB_RUN_ATTEMPT','0'),('CARGO_INCREMENTAL','1'),
                           ('CC','/unexpected/compiler'),('CFLAGS','-unexpected-flag')]:
            with self.subTest(key=key,value=value):
                environment = dict(self.environment)
                if value is None: environment.pop(key)
                else: environment[key] = value
                with mock.patch.dict(os.environ,environment,clear=True):
                    with self.assertRaises((AssertionError,KeyError)):
                        self.build.binding(self.root,self.root,self.ref,self.ref)

    def test_changed_source_bytes_cannot_pass_matching_reviewed_ids(self):
        (self.root/'crewd/schema/crewd-api.schema.json').write_text('{}')
        with mock.patch.dict(os.environ,self.environment,clear=True):
            with self.assertRaises(AssertionError):
                self.build.binding(self.root,self.root,self.ref,self.ref)

if __name__ == '__main__': unittest.main()
