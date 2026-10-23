#!/usr/bin/env python3
"""Affected installer controls only; no systemd/provider/network execution."""
import argparse, importlib.util, io, json, os, pathlib, subprocess, sys, tarfile, tempfile, types, unittest
from unittest import mock
sys.dont_write_bytecode = True
repo = pathlib.Path(__file__).resolve().parents[2]
# Source path is explicit for private author runs, otherwise this lives in scripts/dev.
source = pathlib.Path(os.environ.get('PCW_HOST_INSTALLER_SOURCE', repo / 'deploy/linux/pendingcrew-host.py'))
spec = importlib.util.spec_from_file_location('pendingcrew_host', source)
host = importlib.util.module_from_spec(spec); spec.loader.exec_module(host)

class InstallerControls(unittest.TestCase):
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

    def test_preflight_refusal_performs_no_persistent_install_or_inference(self):
        account = types.SimpleNamespace(pw_uid=1000, pw_gid=1000, pw_dir='/home/ordinary')
        for failure in ['missing', 'version', 'login']:
            with self.subTest(failure=failure):
                calls = []
                def run(argv, **kwargs):
                    calls.append(argv)
                    self.assertNotIn('env', kwargs)
                    self.assertTrue(argv[:2] == ['/usr/bin/systemd-run', '--quiet'])
                    command = argv[argv.index('--') + 1:]
                    if command[0] == '/bin/sh':
                        self.assertEqual(command[1:], ['-c', 'command -v "$1"', 'sh', 'codex'])
                        return subprocess.CompletedProcess(argv, 1 if failure == 'missing' else 0, b'/usr/bin/python3\n', b'')
                    if command[0] == '/usr/bin/test': return subprocess.CompletedProcess(argv, 1, b'', b'')
                    if command[-1] == '--version': return subprocess.CompletedProcess(argv, 0, b'bad\n' if failure == 'version' else b'codex-cli 0.163.0\n', b'')
                    self.assertEqual(command[-2:], ['login', 'status'])
                    self.assertEqual(failure, 'login', 'unsupported version reached auth status')
                    return subprocess.CompletedProcess(argv, 1, b'', b'')
                with mock.patch.object(host, 'service_user', return_value=account), mock.patch.object(host.subprocess, 'run', side_effect=run), mock.patch.object(host, 'root_code_directory') as write, mock.patch.object(host, 'download') as network:
                    # provider_preflight default runner is bound at definition time.
                    with self.assertRaises(host.Refused): host.provider_preflight(account, {}, runner=run)
                    original_preflight = host.provider_preflight
                    with mock.patch.object(host, 'provider_preflight', side_effect=lambda account, supplied: original_preflight(account, supplied, runner=run)):
                        with self.assertRaises(host.Refused): host.install(types.SimpleNamespace(user='ordinary', codex=None, claude=None))
                    write.assert_not_called(); network.assert_not_called()
                self.assertTrue(calls)
                self.assertTrue(all(c[c.index('--')+2:] in (['--version'], ['login','status']) for c in calls if c[c.index('--')+1] not in ('/bin/sh','/usr/bin/test')))
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
            with mock.patch('sys.stdout', new_callable=io.StringIO): package_module.package(args)
            output = root/'package'; manifest = json.loads((output/'manifest.json').read_bytes()); payload = (output/'host.tar.gz').read_bytes()
            self.assertEqual(host.sha(payload),manifest['artifact']['sha256'])
            extraction = root/'extract'; extraction.mkdir()
            previous = os.umask(0o077)
            try: unpacked, release = host.extract_release(payload,extraction,manifest['artifact'])
            finally: os.umask(previous)
            self.assertEqual((unpacked.stat().st_mode & 0o777),0o755)
            self.assertEqual(((unpacked/'bin').stat().st_mode & 0o777),0o755)
            self.assertEqual(((unpacked/'bin/crewd').stat().st_mode & 0o777),0o555)
            self.assertEqual(release['storeVersion'],21)
            self.assertEqual(release['files']['bin/crewd'],host.sha(header))
            self.assertIn(repr(host.sha((output/'manifest.json').read_bytes())),(output/'install.py').read_text())
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
