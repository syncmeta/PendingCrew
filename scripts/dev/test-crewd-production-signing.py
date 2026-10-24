#!/usr/bin/env python3
"""Recipe construction and local archive/inode checks. No Xcode/security/Apple calls."""
import contextlib,copy,hashlib,importlib.util,io,os,pathlib,plistlib,shutil,subprocess,sys,tarfile,tempfile
sys.dont_write_bytecode=True
root=pathlib.Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('sign_recipe',root/'scripts/dev/build-crewd-production-sign-only.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
team='M42BKJN82S';bundle='com.pendingname.pendingcrew'
template=plistlib.loads((root/'Resources/PendingCrew.entitlements').read_bytes())
profile={'ApplicationIdentifierPrefix':[team],'TeamIdentifier':[team],'Entitlements':{'com.apple.application-identifier':team+'.'+bundle,'com.apple.developer.team-identifier':team,'keychain-access-groups':[team+'.*'],'unused-allowlist-capability':True}}
expected={'com.apple.application-identifier':team+'.'+bundle,'com.apple.developer.team-identifier':team,'keychain-access-groups':[team+'.com.pendingname.pendingcrew',team+'.com.pendingname.shared']}
assert m.distribution_entitlements(template,profile,team,bundle)==expected
print('PASS actual source groups expanded exactly; unused profile capability not copied')
def reject(label,change_template=lambda x:None,change_profile=lambda x:None):
 t,p=copy.deepcopy(template),copy.deepcopy(profile);change_template(t);change_profile(p)
 try:m.distribution_entitlements(t,p,team,bundle)
 except ValueError:print('PASS reject '+label);return
 raise AssertionError('accepted '+label)
reject('missing prefix',change_profile=lambda p:p.pop('ApplicationIdentifierPrefix'))
reject('ambiguous prefix',change_profile=lambda p:p.update(ApplicationIdentifierPrefix=[team,'OTHER00000']))
reject('wrong Team',change_profile=lambda p:p.update(TeamIdentifier=['OTHER00000']))
reject('wrong AppID',change_profile=lambda p:p['Entitlements'].update({'com.apple.application-identifier':team+'.other.app'}))
reject('unauthorized shared group',change_profile=lambda p:p['Entitlements'].update({'keychain-access-groups':[team+'.'+bundle]}))
reject('wildcard source claim',lambda t:t.update({'keychain-access-groups':[team+'.*']}))
reject('unresolved variable',lambda t:t.update({'keychain-access-groups':['$(Unknown).app']}))
reject('extra privileged source claim',lambda t:t.update({'com.apple.security.cs.disable-library-validation':True}))
reject('debug profile',change_profile=lambda p:p['Entitlements'].update({'get-task-allow':True}))
reject('duplicate source groups',lambda t:t['keychain-access-groups'].append(t['keychain-access-groups'][0]))
args=m.production_archive_argv({'derivedData':'/existing/warm','sourcePackages':'/existing/spm','sourceRef':'a'*40},pathlib.Path('/snapshot'),pathlib.Path('/out'),'b'*64,'20749.00001')
for token in ['CODE_SIGNING_ALLOWED=NO','CODE_SIGNING_REQUIRED=NO','CODE_SIGN_IDENTITY=','DEVELOPMENT_TEAM=','PROVISIONING_PROFILE_SPECIFIER=','PROVISIONING_PROFILE=','CODE_SIGN_ENTITLEMENTS=','ENABLE_HARDENED_RUNTIME=YES','ENABLE_DEBUG_DYLIB=NO']:
 assert token in args
assert not any('[sdk=' in token for token in args)
assert '-allowProvisioningUpdates' not in args and '-configuration' in args and args[args.index('-configuration')+1]=='Release'
print('PASS unsigned ordinary Release construction clears Xcode signing/profile selection; no provisioning updates')
assert 'MARKETING_VERSION=0.1.44' in args
version_args=m.production_archive_argv({'derivedData':'/existing/warm','sourcePackages':'/existing/spm','sourceRef':'a'*40},pathlib.Path('/snapshot'),pathlib.Path('/out'),'b'*64,'20749.00001','0.2.0')
assert [x for x in version_args if x.startswith('MARKETING_VERSION=')]==['MARKETING_VERSION=0.2.0']
assert [x for x in args if not x.startswith('MARKETING_VERSION=')]==[x for x in version_args if not x.startswith('MARKETING_VERSION=')]
print('PASS explicit0.2.0 archive version; default0.1.44 and all other original archive arguments unchanged')

def reject_call(label,operation):
 try:operation()
 except ValueError:print('PASS reject '+label);return
 raise AssertionError('accepted '+label)

info={'CFBundleIdentifier':bundle,'PendingCrewSourceRef':'a'*40,'BuildStampCommit':'a'*40,'CFBundleVersion':'20749.00001','CFBundleShortVersionString':'0.2.0'}
m.qualify_product_info(info,'a'*40,'20749.00001','0.2.0')
legacy_info=dict(info,CFBundleShortVersionString='0.1.44')
m.qualify_product_info(legacy_info,'a'*40,'20749.00001')
reject_call('old actual metadata under requested0.2.0',lambda:m.qualify_product_info(legacy_info,'a'*40,'20749.00001','0.2.0'))
reject_call('new actual metadata under default0.1.44',lambda:m.qualify_product_info(info,'a'*40,'20749.00001'))
for key in ['PendingCrewSourceRef','BuildStampCommit','CFBundleVersion','CFBundleIdentifier']:
 reject_call('version gate preserves '+key,lambda key=key:m.qualify_product_info(dict(info,**{key:'wrong'}),'a'*40,'20749.00001','0.2.0'))
for value in ['', '0.2', '0.2.0-preview', '00.2.0', '0.2.0\nOTHER=1']:
 reject_call('invalid marketing version '+repr(value),lambda value=value:m.production_archive_argv({'derivedData':'/existing/warm','sourcePackages':'/existing/spm','sourceRef':'a'*40},pathlib.Path('/snapshot'),pathlib.Path('/out'),'b'*64,'20749.00001',value))
print('PASS actual metadata gate binds requested version and retains every source/build/identifier check')

# Exercise the actual CLI parser and read-only plan. Only the separately tested
# source/profile preparation boundary is replaced; no execute flag is passed.
with tempfile.TemporaryDirectory(prefix='pcw-version-plan-fixture-') as temporary:
 scope=pathlib.Path(temporary);manifest=scope/'fixture-manifest';manifest.write_text('{}');original_prepare=m.prepare;original_argv=sys.argv
 try:
  m.prepare=lambda manifest:(scope,{'sourceRef':'a'*40,'backendCompiledRef':'b'*40},root)
  for flags,version in [([], '0.1.44'),(['--marketing-version','0.2.0'], '0.2.0')]:
   sys.argv=['build-crewd-production-sign-only.py',str(manifest)]+flags;output=io.StringIO()
   with contextlib.redirect_stdout(output):m.main()
   plan=__import__('json').loads(output.getvalue())
   assert plan['marketingVersion']==version and plan['timestampMode']=='none'
   assert plan['actualSigning'] is False and plan['notarize'] is False and plan['upload'] is False
   assert not (scope/'signed-output').exists()
 finally:m.prepare=original_prepare;sys.argv=original_argv
print('PASS actual CLI default/0.2.0 read-only plans preserve original local timestamp/signing semantics')

assert m.production_sign_argv('identity','/app')==['codesign','--force','--sign','identity','--options','runtime','--timestamp=none','/app']
for artifact in ['/app','/app/Contents/Helpers/crewd']:
 signed=m.production_sign_argv('identity',artifact,'secure')
 assert '--timestamp' in signed and '--timestamp=none' not in signed and signed[-1]==artifact
assert m.production_sign_argv('identity','/app','secure','/claims')[-3:]==['--entitlements','/claims','/app']
reject_call('arbitrary timestamp service',lambda:m.production_sign_argv('identity','/app','https://other'))
print('PASS secure timestamp construction for App and helper; original local default unchanged')

spec=importlib.util.spec_from_file_location('source_inventory',root/'scripts/dev/build-crewd-native-macos.py')
b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
with tempfile.TemporaryDirectory(prefix='pcw-production-recipe-fixture-') as temporary:
 scope=pathlib.Path(temporary);repo=scope/'repo';repo.mkdir();(repo/'design').mkdir()
 payload=repo/'payload';payload.write_bytes(b'ordinary executable input\n');payload.chmod(0o755)
 target='../never-present';os.symlink(target,repo/'design/schema-source')
 def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],stderr=subprocess.DEVNULL)
 git('init','-q');git('add','payload','design/schema-source')
 git('-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-qm','inputs')
 ref=git('rev-parse','HEAD').decode().strip();links=b.committed_symlink_inputs(repo,ref)
 archive=scope/'source.tar';git('archive','--format=tar','-o',str(archive),ref)
 snapshot=scope/'snapshot';snapshot.mkdir();b.extract_source_archive(archive,snapshot,links)
 rows=b.regular_inventory(snapshot,links);link=next(r for r in rows if r['path']=='design/schema-source')
 assert link['gitMode']=='120000' and link['gitBlob']==links['design/schema-source']['blob']
 assert link['sha256']==hashlib.sha256(os.fsencode(target)).hexdigest()
 assert (snapshot/'payload').stat().st_mode&0o111 and not (snapshot/'design/schema-source').exists()
 assert (snapshot/'design/schema-source').is_symlink() and len(rows)==2
 print('PASS real Git120000 archive/blob and lstat/readlink inventory; dangling target never followed; executable preserved')
 reject_call('default unreviewed symlink',lambda:b.regular_inventory(snapshot))
 linkpath=snapshot/'design/schema-source';linkpath.unlink();os.symlink('changed-target',linkpath)
 reject_call('changed symlink bytes',lambda:b.regular_inventory(snapshot,links))
 linkpath.unlink();linkpath.write_bytes(os.fsencode(target))
 reject_call('symlink replaced by regular file',lambda:b.regular_inventory(snapshot,links))
 linkpath.unlink();reject_call('missing committed symlink',lambda:b.regular_inventory(snapshot,links))
 for label,name,kind,value in [
     ('unreviewed archive link','other',tarfile.SYMTYPE,target),
     ('changed archive link','design/schema-source',tarfile.SYMTYPE,'changed'),
     ('archive hardlink','other',tarfile.LNKTYPE,'payload'),
     ('archive path traversal','../escape',tarfile.REGTYPE,''),
     ('member below reviewed link','design/schema-source/escape',tarfile.REGTYPE,'')]:
  bad=scope/'bad.tar'
  with tarfile.open(bad,'w') as t:
   if label=='member below reviewed link':
    original=tarfile.TarInfo('design/schema-source');original.type=tarfile.SYMTYPE;original.linkname=target;t.addfile(original)
   item=tarfile.TarInfo(name);item.type=kind;item.linkname=value;t.addfile(item,io.BytesIO(b''))
  dest=scope/'bad-output';dest.mkdir()
  reject_call(label,lambda:b.extract_source_archive(bad,dest,links))
  assert not list(dest.iterdir());dest.rmdir()
 original=scope/'normal';original.write_bytes(b'normal payload');original.chmod(0o444)
 digest=hashlib.sha256(original.read_bytes()).hexdigest();private=scope/'private-normal'
 proof=m.copy_backend_for_archive(original,private,digest)
 assert original.stat().st_mode&0o777==0o444 and private.stat().st_mode&0o777==0o755
 assert original.read_bytes()==private.read_bytes() and proof['originalUnchanged']
 reject_call('overwrite prior private normal',lambda:m.copy_backend_for_archive(original,private,digest))
 reject_call('wrong normal hash',lambda:m.copy_backend_for_archive(original,scope/'wrong','0'*64))
 print('PASS actual0444 normal -> private0755 exact bytes; immutable input stays0444')
 products=scope.resolve()/'Products';(products/'Fixture.app/Contents').mkdir(parents=True)
 legacy_copy=scope/'legacy-copy';shutil.copy2(original,legacy_copy)
 env=dict(os.environ,PLATFORM_NAME='macosx',CONFIGURATION='Release',PENDINGCREW_BACKEND_INPUT=str(legacy_copy.resolve()),PENDINGCREW_BACKEND_SHA256=digest,TARGET_BUILD_DIR=str(products),CONTENTS_FOLDER_PATH='Fixture.app/Contents')
 red=subprocess.run(['/bin/sh',str(root/'Shared/scripts/embed-crewd-macos.sh')],env=env,capture_output=True,text=True)
 assert red.returncode!=0 and 'executable regular file' in red.stderr
 assert not (products/'Fixture.app/Contents/Helpers/crewd').exists()
 env['PENDINGCREW_BACKEND_INPUT']=str(private.resolve())
 green=subprocess.run(['/bin/sh',str(root/'Shared/scripts/embed-crewd-macos.sh')],env=env,capture_output=True,text=True)
 assert green.returncode==0,green.stderr
 assert (products/'Fixture.app/Contents/Helpers/crewd').read_bytes()==original.read_bytes()
 assert original.stat().st_mode&0o777==0o444
 print('PASS real existing embed entry RED with copy2/0444 then GREEN private0755; original guard unchanged')
