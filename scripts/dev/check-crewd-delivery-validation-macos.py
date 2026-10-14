#!/usr/bin/env python3
"""Read-only independent DeliveryValidation artifact consumer, no bundle launch."""
import argparse,importlib.util,json,os,pathlib,plistlib,re,shlex,subprocess,sys
sys.dont_write_bytecode=True

def load_builder():
 path=pathlib.Path(__file__).with_name('build-crewd-delivery-validation-macos.py');spec=importlib.util.spec_from_file_location('validation_builder',path);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module

def logged(path,wanted,marker=None):
 text=path.read_text();lines=text.splitlines();commands=[shlex.split(x[9:]) for x in lines if x.startswith('Command: ')]
 exits=[x[6:] for x in lines if x.startswith('Exit: ')]
 if commands!=wanted or exits!=['0']*len(wanted) or (marker and marker not in text):raise ValueError('actual command/success mismatch: '+path.name)
 return text

def check(base,ref):
 b=load_builder();repo=pathlib.Path(__file__).resolve().parents[2];native,proof=b.frozen_tools(repo,ref);base=pathlib.Path(base)
 if not base.is_absolute() or base.resolve()!=base:raise ValueError('canonical artifact root required')
 manifest=json.loads((base/'build-inputs.json').read_text());receipt=json.loads((base/'build-provenance.json').read_text())
 if type(manifest.get('format')) is not int or manifest['format']!=1 or type(receipt.get('format')) is not int or receipt['format']!=1:raise ValueError('closed proof version required')
 if manifest.get('sourceRef')!=ref or receipt.get('sourceRef')!=ref or manifest.get('sourceSelection')!='committed-archive':raise ValueError('explicit source attribution mismatch')
 required={'build-inputs.json','backend-witness.json','original-backend-witness.json','crewd.pre-sign','gui.pre-sign','settings.json','settings.log','toolchain.json','toolchain.log','build.log','dependency-build-inventory.json','xcode-products.json','gui-pre-sign-products.json','packaged-products.json','adhoc-sign.log','signature-check.log','signature-display.log','entitlements.log','entitlements.plist'}
 if set(receipt.get('files',{}))!=required:raise ValueError('complete evidence file set required')
 for name,digest in receipt['files'].items():
  p=base/name
  if p.is_symlink() or not p.is_file() or b.sha(p)!=digest:raise ValueError('proof evidence bytes mismatch: '+name)
 snapshot=base/'source-inputs';sources=native.regular_inventory(snapshot)
 paths=b.git(repo,'ls-tree','-r','--name-only',ref).decode().splitlines()
 actual=[{'path':name,'sha256':__import__('hashlib').sha256(b.git(repo,'show',ref+':'+name)).hexdigest()} for name in sorted(paths)]
 if sources!=actual or manifest['files']!=sources or manifest['sourceInputDigest']!=proof.digest(sources):raise ValueError('all Git blobs/source snapshot mismatch')
 if (snapshot/'Config/Local.xcconfig').exists():raise ValueError('personal Local configuration entered snapshot')
 b.configuration_source(lambda path:(snapshot/path).read_bytes());rust=[row for row in sources if row['path'].startswith('crewd/')]
 if manifest['rustInputDigest']!=proof.digest(rust):raise ValueError('complete Rust inventory mismatch')
 if manifest.get('backendProfile') not in ['debug','release']:raise ValueError('literal normal profile missing')
 os.environ['PENDINGCREW_BUILD_BACKEND_PROFILE']=manifest['backendProfile']
 b.require_normal_source(json.loads((base/'backend-witness.json').read_text()),ref)
 witness,evidence=proof.verify_witness(base/'backend-witness.json',base/'crewd.pre-sign',proof.digest(rust),proof.requested_configuration(snapshot/'crewd'),require_fresh=True)
 original=json.loads((base/'original-backend-witness.json').read_text());expected=dict(original,evidenceDirectory='backend-evidence')
 if witness!=expected or witness['sourceRef']!=ref or witness['buildConfiguration']!=manifest['backendBuildConfiguration']:raise ValueError('helper compiled source/config or relocation mismatch')
 dependencies=native.regular_inventory(base/'dependency-inputs')
 if dependencies!=manifest['dependencyFiles'] or proof.digest(dependencies)!=manifest['dependencyInputDigest']:raise ValueError('complete SPM archive bytes differ')
 lock=json.loads((snapshot/'PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved').read_text());pins=[{'identity':row['identity'],'revision':row['state']['revision']} for row in lock['pins']]
 if pins!=manifest['packagePins'] or len({row['identity'] for row in pins})!=len(pins):raise ValueError('SPM lock/pins mismatch')
 for pin in pins:
  if not re.fullmatch('[0-9a-f]{40}',pin['revision']) or not any(row['path'].startswith('checkouts/'+pin['identity']+'/') for row in dependencies):raise ValueError('complete resolved package missing')
 if any(not row['path'].startswith(('checkouts/','artifacts/')) for row in dependencies):raise ValueError('unexpected dependency origin')
 if manifest.get('externalSPM')!=b.SPM or manifest.get('derivedData')!=b.DD:raise ValueError('warm cache configuration mismatch')
 if json.loads((base/'dependency-build-inventory.json').read_text())!={'before':dependencies,'after':dependencies,'externalPath':b.SPM}:raise ValueError('SPM changed during build')
 if manifest.get('scheme')!=b.SCHEME or manifest.get('configuration')!=b.CONFIG or manifest.get('compileCondition')!=b.CONDITION:raise ValueError('dedicated validation config missing')
 args=argparse.Namespace(ref=ref,derived_data=b.DD,spm=b.SPM);command=b.command(snapshot,args)
 if manifest['command']!=command+['build']:raise ValueError('actual original-target request mismatch')
 logged(base/'build.log',[command+['build']],'** BUILD SUCCEEDED **');text=logged(base/'settings.log',[command+['-showBuildSettings','-json']])
 if (base/'settings.json').read_text() not in text:raise ValueError('actual settings query content mismatch')
 settings=b.settings_check(json.loads((base/'settings.json').read_text()),ref)
 tool=json.loads((base/'toolchain.json').read_text())
 if tool!=manifest['toolchain'] or set(tool)!= {'xcodeVersion','sdkVersion'} or not all(tool.values()):raise ValueError('toolchain evidence mismatch')
 b.sdk_check(settings,tool)
 text=logged(base/'toolchain.log',[['xcodebuild','-version'],['xcrun','--sdk','macosx','--show-sdk-version']])
 if not all(value in text for value in tool.values()):raise ValueError('actual toolchain output missing')
 name=receipt.get('appDirectory')
 if name!='PendingCrew Delivery Validation.app':raise ValueError('dedicated package identity missing')
 app=base/name
 if app.is_symlink():raise ValueError('app alias forbidden')
 info=plistlib.loads((app/'Contents/Info.plist').read_bytes())
 if info.get('CFBundleIdentifier')!=b.IDENTIFIER or info.get('PendingCrewDeliveryValidation') is not True or info.get('PendingCrewSourceRef')!=ref or info.get('BuildStampCommit')!=ref:raise ValueError('actual validation marker/source/ID mismatch')
 executable=info.get('CFBundleExecutable')
 if not isinstance(executable,str) or pathlib.PurePath(executable).name!=executable:raise ValueError('invalid executable basename')
 presign=json.loads((base/'gui-pre-sign-products.json').read_text())
 if presign!=json.loads((base/'xcode-products.json').read_text()):raise ValueError('target product copied with changes before packaging')
 mainrow=next(row for row in presign if row['path']=='Contents/MacOS/'+executable)
 if mainrow.get('kind')!='file' or mainrow.get('sha256')!=b.sha(base/'gui.pre-sign'):raise ValueError('actual GUI pre-sign pairing mismatch')
 final=b.products(app)
 if final!=json.loads((base/'packaged-products.json').read_text()) or b.loaded_code(app,final)!=receipt.get('allLoadedCode'):raise ValueError('complete sealed product/loaded code inventory mismatch')
 loaded={row['path'] for row in receipt['allLoadedCode']}
 if not {'Contents/MacOS/'+executable,'Contents/Helpers/crewd','Contents/_CodeSignature/CodeResources'}.issubset(loaded):raise ValueError('complete GUI/helper/seal objects missing')
 logged(base/'adhoc-sign.log',[['codesign','--force','--deep','--sign','-','--options','runtime',str(app)]])
 logged(base/'signature-check.log',[['codesign','--verify','--deep','--strict',str(app)]])
 signature=logged(base/'signature-display.log',[['codesign','--display','--verbose=4',str(app)]])
 if 'Identifier='+b.IDENTIFIER not in signature or 'Signature=adhoc' not in signature or 'TeamIdentifier=not set' not in signature or 'runtime)' not in signature or not re.search(r'^CDHash=[0-9a-f]{40}$',signature,re.M):raise ValueError('actual ad-hoc identity/runtime/code hash missing')
 logged(base/'entitlements.log',[['codesign','--display','--entitlements',':-',str(app)]])
 content=(base/'entitlements.plist').read_bytes()
 if content.strip() and plistlib.loads(content):raise ValueError('production/debug/shared entitlements entered validation product')
 if receipt.get('actualArtifactBuilt') is not True or receipt.get('signatureMode')!='explicit-adhoc' or any(receipt.get(key) is not False for key in ['publisherEligible','actualKeychainUsed','AppLaunched','installed','published']):raise ValueError('evidence layer or execution boundary mismatch')
 subprocess.run(['codesign','--verify','--deep','--strict',str(app)],check=True)
 print('PASS complete DeliveryValidation source/config/SPM/strong normal/products/loaded code/ad-hoc seal; no persistent runtime or publisher claim')
 return {'sourceRef':ref,'sourceCount':len(sources),'signatureMode':'explicit-adhoc','persistentRuntimeProven':False,'publisherEligible':False}

def main():
 p=argparse.ArgumentParser();p.add_argument('artifact');p.add_argument('--ref',required=True);args=p.parse_args();check(pathlib.Path(args.artifact),args.ref)
if __name__=='__main__':
 try:main()
 except (ValueError,OSError,KeyError,StopIteration,subprocess.CalledProcessError) as e:raise SystemExit('FAIL: '+str(e))
