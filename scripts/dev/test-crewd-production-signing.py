#!/usr/bin/env python3
"""Pure recipe construction/entitlement rejection checks. No Xcode/security calls."""
import copy,importlib.util,pathlib,plistlib,sys
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
for token in ['CODE_SIGNING_ALLOWED=NO','CODE_SIGNING_REQUIRED=NO','CODE_SIGN_IDENTITY=','CODE_SIGN_IDENTITY[sdk=macosx*]=','DEVELOPMENT_TEAM=','PROVISIONING_PROFILE_SPECIFIER=','PROVISIONING_PROFILE=','CODE_SIGN_ENTITLEMENTS=','ENABLE_HARDENED_RUNTIME=YES','ENABLE_DEBUG_DYLIB=NO']:
 assert token in args
assert '-allowProvisioningUpdates' not in args and '-configuration' in args and args[args.index('-configuration')+1]=='Release'
print('PASS unsigned ordinary Release construction clears Xcode signing/profile selection; no provisioning updates')
