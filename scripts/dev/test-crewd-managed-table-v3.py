#!/usr/bin/env python3
"""Actual Native V3 Wire/Store + isolated mock/journal proof; no Core, UI, or daily data."""
import hashlib,json,os,pathlib,shutil,subprocess,sys,tempfile
ROOT=pathlib.Path(__file__).resolve().parents[2]
FILES=[
 'Sources/Mac/Crewd/CrewdClient.swift','Sources/Mac/Crewd/CrewdLocalIPC.swift',
 'Sources/Mac/Crewd/CrewdPublicCommandWire.swift','Sources/Mac/Crewd/CrewdCodexWire.swift',
 'Sources/Mac/Crewd/CrewdClaudeHistoryStore.swift','Sources/Mac/Crewd/CrewdPresentationJournal.swift',
 'Sources/Mac/Crewd/CrewdWorkStore.swift','Sources/Mac/Crewd/CrewdPublicCommands.swift',
 'Sources/PersonalSync/CrewPersonalCoreWire.swift','Sources/PersonalSync/AppleBranchStore.swift',
 'Sources/Chat/Adapter/CrewComposerDraftValue.swift','Sources/Chat/Adapter/CrewComposerDrafts.swift',
 'Sources/CrewdRemote/CrewdKnowledgeWire.swift','Sources/CrewdRemote/CrewdKnowledgeViewRuntime.swift',
 'Sources/CrewdRemote/CrewdKnowledgeStore.swift','Sources/CrewdRemote/CrewdManagedTableV2Wire.swift',
 'Sources/CrewdRemote/CrewdManagedTableV2Store.swift','Sources/CrewdRemote/CrewdOrganizationSearchWire.swift',
 'Sources/CrewdRemote/CrewdOrganizationSearchStore.swift','Sources/CrewdRemote/CrewdManagedTableV3Wire.swift',
 'Sources/CrewdRemote/CrewdManagedTableV3Store.swift','scripts/dev/fixtures/crewd-managed-table-v3.swift']
DATA=['scripts/dev/fixtures/managed-table-v3-intent-goldens.json','scripts/dev/fixtures/managed-table-v3-values.json']
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def save(path,value):
 with path.open('x') as f:json.dump(value,f,indent=2);f.write('\n')
def main():
 assert sys.platform=='darwin' and not sys.flags.optimize
 os.umask(0o077)
 out=pathlib.Path(sys.argv[1]) if len(sys.argv)==2 else pathlib.Path(tempfile.mkdtemp(prefix='pcw-table-v3-native.',dir='/private/tmp'))
 if len(sys.argv)==2:
  assert out.is_absolute() and out.is_relative_to('/private/tmp') and not out.exists();out.mkdir(mode=0o700)
 else:assert len(sys.argv)==1
 inputs=[]
 for rel in FILES+DATA:
  src=ROOT/rel;dst=out/'product'/rel;dst.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(src,dst);inputs.append({'path':rel,'sha256':sha(dst)})
 for name in ['home','tmp']:(out/name).mkdir(mode=0o700)
 command=['/usr/bin/xcrun','swiftc','-warnings-as-errors','-disable-sandbox','-module-cache-path',str(out/'cache'),'-parse-as-library']+[str(out/'product'/p) for p in FILES]+['-o',str(out/'proof')]
 manifest={'layer':'21 actual Foundation product inputs +1 fixture; mock RPC and local durable journal/two child process prepare only','inputCount':len(FILES),'inputs':inputs,'command':command,'compiler':subprocess.check_output(['/usr/bin/xcrun','swiftc','--version'],text=True,stderr=subprocess.STDOUT),'runnerSHA256':sha(pathlib.Path(__file__))}
 save(out/'inputs.json',manifest)
 with (out/'build.log').open('xb') as log:built=subprocess.run(command,stdout=log,stderr=log)
 result={'buildExitCode':built.returncode,'inputSnapshotsUnchanged':all(sha(out/'product'/item['path'])==item['sha256'] for item in inputs),'productSourceUnchangedDuringBuild':all(sha(ROOT/item['path'])==item['sha256'] for item in inputs),'Core':False,'fullApp':False,'GUI':False,'daily':False}
 if built.returncode==0:
  result['executableSHA256']=sha(out/'proof')
  env=dict(os.environ,HOME=str(out/'home'),TMPDIR=str(out/'tmp'))
  with (out/'run.log').open('xb') as log:ran=subprocess.run([str(out/'proof'),*[str(out/'product'/p) for p in DATA]],stdout=log,stderr=log,env=env)
  result['runExitCode']=ran.returncode;result['runLogSHA256']=sha(out/'run.log')
  result['passed']=ran.returncode==0 and 'PASS actual V3 Wire+Store/journal mock:' in (out/'run.log').read_text()
 else:result['passed']=False
 save(out/'result.json',result);print(out/'result.json');sys.exit(0 if result['passed'] else 1)
if __name__=='__main__':main()
