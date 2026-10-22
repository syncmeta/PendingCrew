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

if __name__ == '__main__': unittest.main()
