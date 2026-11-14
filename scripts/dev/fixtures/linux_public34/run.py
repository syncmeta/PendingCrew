#!/usr/bin/env python3
"""One explicit actual Linux public34 qualification. Never build/install/publish."""
import argparse,json,os,sys,traceback
from pathlib import Path
sys.dont_write_bytecode=True
from common import save
from qualification import qualify,INPUT
from runtime import Runner
from scenarios import FLOWS
from native import NativeRunner,native_flow
from installation import installation_flow


def main():
    if sys.flags.optimize:raise SystemExit('assertion optimization forbidden')
    parser=argparse.ArgumentParser(description=__doc__)
    for flag in ['core','witness','source','legacy-core','legacy-witness','fake-codex','helper-proof','install-package','out']:
        parser.add_argument('--'+flag,type=Path,required=True)
    for flag in ['core-sha256','expected-ref','expected-rust-input']:parser.add_argument('--'+flag,required=True)
    a=parser.parse_args()
    assert sys.platform=='linux' and os.uname().machine=='x86_64' and os.getuid()==os.geteuid()!=0
    assert all(x.is_absolute() for x in [a.core,a.witness,a.source,a.legacy_core,a.legacy_witness,a.out,a.fake_codex,a.helper_proof,a.install_package])
    assert not a.source.is_symlink() and a.source.is_dir() and not a.out.exists()
    os.umask(0o077);a.out.mkdir(mode=0o700);index=0;results=[]
    def record(name,value):
        nonlocal index
        index+=1;save(a.out/(f'qualification-{index:03d}-'+name+'.json'),value)
    try:
        inputs,qualified=qualify(a,record)
        scopes=[(label,flow,Runner) for label,flow in FLOWS]
        scopes.extend([('synthetic-native',native_flow,NativeRunner),('installer-qualification',installation_flow,Runner)])
        for label,flow,kind in scopes:
            runner=kind(a,a.out/label,label,qualified,inputs)
            try:runner.prepare();flow(runner);runner.succeed()
            except BaseException as error:runner.report.update(exit=1,errorType=type(error).__name__,error=str(error),traceback=traceback.format_exc(),fullScopeNotQualified=True)
            finally:runner.finish()
            results.append(runner.report)
            if runner.report['exit']!=0:break
        result={'exit':0 if len(results)==len(scopes) and all(x['exit']==0 for x in results) else 1,
            'sourceRef':a.expected_ref,'rustInputDigest':INPUT,'scopes':results,
            'actualLinuxOrdinaryCore':any(x.get('actualCoreExecutableQualified',False) for x in results),
            'realModelOrProvider':False,'allBusinessIntentsSubmittedOnce':True,
            'originalUUIDReopenLookupOnly':True,'installedSystemd':False,'published':False}
    except BaseException as error:
        result={'exit':1,'stage':'source/input/setup qualification','errorType':type(error).__name__,
                'error':str(error),'traceback':traceback.format_exc(),'scopes':results,'noRetryOrForcedCleanup':True}
    save(a.out/'result.json',result);print(json.dumps(result),flush=True);return result['exit']


if __name__=='__main__':sys.exit(main())
