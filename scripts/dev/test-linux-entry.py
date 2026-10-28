#!/usr/bin/env python3
"""Local entry tests; no provider, service or installation is executed."""
import importlib.util, pathlib, tempfile, unittest, types, json, io
from unittest import mock
root=pathlib.Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('host',root/'deploy/linux/pendingcrew-host.py')
host=importlib.util.module_from_spec(spec);spec.loader.exec_module(host)
class Entry(unittest.TestCase):
    def args(self,*extra):
        return host.parser().parse_args(['setup','--non-interactive','--user','ordinary','--advertise-host','example.test','--listen-host','127.0.0.1','--port','49381','--disable-claude',*extra])
    def test_setup_preserves_selected_identity_and_explicit_provider(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(host,'STATE',pathlib.Path(tmp)),mock.patch.object(host,'service_user',return_value=types.SimpleNamespace(pw_name='ordinary')),mock.patch.object(host,'install') as install,mock.patch.object(host,'ask',side_effect=AssertionError('unexpected prompt')),mock.patch('sys.stdout',new_callable=io.StringIO):
            host.setup(self.args())
            value=install.call_args.args[0]
            self.assertEqual(value.user,'ordinary');self.assertTrue(value.disable_claude);self.assertIsNone(value.preserve_connection_from)
            host.canonical_uuid(value.request_id)
    def test_setup_existing_or_partial_record_never_replays(self):
        for name in ['installation.json','install-original.json']:
            with tempfile.TemporaryDirectory() as tmp,mock.patch.object(host,'STATE',pathlib.Path(tmp)),mock.patch.object(host,'install') as install:
                (pathlib.Path(tmp)/name).write_text('{}')
                with self.assertRaises(host.Refused):host.setup(self.args())
                install.assert_not_called()
    def test_noninteractive_requires_connection_and_loopback_ack(self):
        with tempfile.TemporaryDirectory() as tmp,mock.patch.object(host,'STATE',pathlib.Path(tmp)),mock.patch.object(host,'service_user',return_value=types.SimpleNamespace(pw_name='ordinary')),mock.patch.object(host,'install') as install:
            for args in [host.parser().parse_args(['setup','--non-interactive']),self.args('--advertise-host','127.0.0.1')]:
                with self.assertRaises(host.Refused):host.setup(args)
            install.assert_not_called()
    def test_unknown_pair_keeps_record_and_next_call_refuses_new_token(self):
        with tempfile.TemporaryDirectory() as tmp,mock.patch.object(host,'STATE',pathlib.Path(tmp)),mock.patch.object(host,'read_record',side_effect=[{'requestId':'11111111-1111-4111-8111-111111111111'},{'advertiseHost':'example.test','port':49381}]):
            def lost(args):
                pathlib.Path(args.operation_file).write_text('original')
                raise OSError('unknown')
            with mock.patch.object(host,'pair',side_effect=lost) as pair,mock.patch('sys.stderr',new_callable=io.StringIO):
                with self.assertRaises(OSError):host.easy_pair(host.parser().parse_args(['pair']))
                with self.assertRaises(host.Refused):host.easy_pair(host.parser().parse_args(['pair']))
                self.assertEqual(pair.call_count,1)
    def test_command_entry_never_overwrites_unrelated_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=pathlib.Path(tmp)/'pendingcrew';path.write_text('other')
            with mock.patch.object(host,'COMMAND_LINK',path):
                with self.assertRaises(host.Refused):host.command_link(create=True)
            self.assertEqual(path.read_text(),'other')
    def test_command_link_tracks_current_managed_member(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=pathlib.Path(tmp)/'pendingcrew'
            with mock.patch.object(host,'COMMAND_LINK',path),mock.patch.object(host,'root_code_directory'):
                host.command_link(create=True)
                self.assertEqual(path.readlink(),host.PREFIX/'current/bin/pendingcrew-host.py')

class URLs(unittest.TestCase):
    def test_fixed_release_and_historical_source_inputs_only(self):
        spec=importlib.util.spec_from_file_location('package',root/'deploy/linux/package-host-release.py')
        package=importlib.util.module_from_spec(spec);spec.loader.exec_module(package)
        source='a'*40
        valid=['https://github.com/syncmeta/PendingCrew/releases/download/v0.2.2/pendingcrew-0.2.2-linux-amd64.tar.gz','https://example.test/'+source+'/host.tar.gz']
        invalid=['https://github.com/syncmeta/PendingCrew/releases/latest/download/host.tar.gz','https://github.com/other/repo/releases/download/v0.2.2/host.tar.gz','http://github.com/syncmeta/PendingCrew/releases/download/v0.2.2/a','https://github.com/syncmeta/PendingCrew/releases/download/v0.2.2/a?x=1','https://github.com/syncmeta/PendingCrew/releases/download/main/a','https://user@github.com/syncmeta/PendingCrew/releases/download/v0.2.2/a']
        for value in valid:
            self.assertEqual(package.url(value,source),value)
            self.assertTrue(host.immutable_release_url(value,source))
        for value in invalid:
            with self.assertRaises(ValueError):package.url(value,source)
            self.assertFalse(host.immutable_release_url(value,source))
    def test_digest_mismatch_never_becomes_a_download_result(self):
        response=mock.MagicMock()
        response.__enter__.return_value=response
        response.url='https://example.test/original'
        response.read.return_value=b'changed'
        with mock.patch.object(host.urllib.request,'urlopen',return_value=response):
            with self.assertRaises(host.Refused):host.download(response.url,'a'*64,1024)
    def test_bootstrap_validates_platform_glibc_and_digest_before_exec(self):
        template=(root/'deploy/linux/install.sh').read_text()
        self.assertIn('Linux:x86_64',template)
        code=template.split("<<'PY'\n",1)[1].rsplit('\nPY',1)[0]
        code=code.replace('@INSTALLER_URL@',repr('https://example.test/installer.py')).replace('@INSTALLER_SHA256@',repr('0'*64)).replace('@MIN_GLIBC@','2.39')
        response=mock.MagicMock();response.__enter__.return_value=response
        response.geturl.return_value='https://example.test/installer.py';response.read.return_value=b'not installer'
        import urllib.request,subprocess,os,shutil
        for libc in ['glibc 2.38','musl 1.2','glibc 2.39']:
            with mock.patch.object(os,'geteuid',return_value=0),mock.patch.object(os,'confstr',return_value=libc),mock.patch.object(pathlib.Path,'is_dir',return_value=True),mock.patch.object(shutil,'which',return_value='/bin/tool'),mock.patch.object(urllib.request,'urlopen',return_value=response) as fetch,mock.patch.object(subprocess,'run') as execute:
                with self.assertRaises(SystemExit):exec(compile(code,'bootstrap','exec'),{})
                execute.assert_not_called()
                self.assertEqual(fetch.call_count,1 if libc=='glibc 2.39' else 0)

if __name__=='__main__': unittest.main()
