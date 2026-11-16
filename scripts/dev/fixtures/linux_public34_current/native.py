"""Synthetic native child only; production normal Core and public command path."""
import json,os,time
from pathlib import Path
from common import birth,same,sha,wait_for
from runtime import Runner,children,executable


class NativeRunner(Runner):
    def __init__(self,*args):
        super().__init__(*args);self.helper_births={}
        proof=json.loads(self.a.helper_proof.read_bytes())
        assert proof['sourceRef']==self.a.expected_ref and proof['rustInputDigest']==self.a.expected_rust_input
        assert proof['normalCoreSHA256']==sha(self.a.core) and proof['features']==[] and proof['test'] is False
        assert not self.a.fake_codex.is_symlink() and self.a.fake_codex.name=='fake_codex'
        assert sha(self.a.fake_codex)==proof['files']['fake_codex']
        log=self.a.helper_proof.parent/proof['actualCargoLogFile']
        assert Path(proof['actualCargoLogFile']).name==proof['actualCargoLogFile'] and not log.is_symlink()
        assert sha(log)==proof['actualCargoLogSHA256']
        messages=[]
        for line in log.read_text().splitlines():
            try:value=json.loads(line)
            except json.JSONDecodeError:continue
            if value.get('reason')=='compiler-artifact' and value.get('target',{}).get('name')=='fake_codex':
                assert value['features']==[] and value['profile']['test'] is False
                assert Path(value['manifest_path'])==self.a.source/'crewd/crates/exec/Cargo.toml'
                assert Path(value['target']['src_path'])==self.a.source/'crewd/crates/exec/src/bin/fake_codex.rs'
                messages.append(value)
        assert len(messages)==1
        self.input_hashes[str(self.a.fake_codex)]=sha(self.a.fake_codex)
        self.input_hashes[str(self.a.helper_proof)]=sha(self.a.helper_proof);self.input_hashes[str(log)]=sha(log)
        self.native_log=self.runtime/'synthetic-native.jsonl'
        self.report.update(actualNativeExecutor=True,syntheticHelper=True,actualModel=False)
    def native_args(self):
        return ['--codex',str(self.a.fake_codex),'--codex-env','FAKE_CODEX_LOG='+str(self.native_log),
                '--codex-env','FAKE_CODEX_MODE=turn_hangs']
    def observe(self):
        while not self.watch_stop.wait(.03):
            try:
                with self.watch_lock:roots=list(self.births)
                for root in roots:
                    if not same(root,birth(root['pid'])):continue
                    pending=children(root['pid'])
                    while pending:
                        pid=pending.pop();original=birth(pid)
                        if original is None:continue
                        try:path=executable(pid)
                        except FileNotFoundError:
                            if birth(pid) is None:continue
                            raise
                        assert original['uid']==os.getuid() and path==str(self.a.fake_codex)
                        assert sha(self.a.fake_codex)==self.input_hashes[str(self.a.fake_codex)]
                        self.helper_births[(pid,original['startTime'])]=original
                        pending.extend(children(pid))
            except BaseException as error:self.watch_errors.append(type(error).__name__+': '+str(error))
    def quiet(self):
        settings=self.rpc('judgment.settings.get',{})['view']
        assert settings['settings']['config'] is None and settings['credentialState']=='missing'
        assert not self.rpc('remote.status',{})['listening']
    def succeed(self):
        assert self.helper_births and all(not same(b,birth(b['pid'])) for b in self.helper_births.values())
        self.report.update(originalHelperBirths=list(self.helper_births.values()),allHelpersPhysicallyGone=True)
        super().succeed()
        self.report['modelsOrExecutorsStarted']=True # actual synthetic native executor only
        self.report['realModelOrProviderStarted']=False
        self.report['syntheticNativeExecutorStarted']=True


def native_flow(r):
    r.boot()
    _,crew=r.command('crew.create',{'title':'Linux public34 synthetic native','workingDirectory':str(r.runtime/'work')})
    marker='PUBLIC34_SYNTHETIC_ORIGINAL_ONCE'
    original,launch=r.command('session.start',{'crewId':crew['result']['id'],'role':'worker','executor':'codex',
        'brief':marker,'isolation':False},phases=('launched',))
    physical=launch['result']['birth'];session=launch['result']['sessionId']
    assert same(physical,birth(physical['pid'])) and executable(physical['pid'])==str(r.a.fake_codex)
    assert physical['bootId'] and physical['pgid']!=os.getpgrp()
    def body_count():
        if not r.native_log.exists():return 0
        rows=[json.loads(x) for x in r.native_log.read_text().splitlines()]
        return sum(x.get('method')=='turn/start' and marker in json.dumps(x) for x in rows)
    wait_for(lambda:body_count()==1,'original synthetic input exactly once')
    wait_for(lambda:(physical['pid'],physical['startTime']) in r.helper_births,'original helper sampled under owned Core')
    before=r.native_log.read_bytes()
    target=r.rpc('session.controlTarget',{'sessionId':session})
    stop_original,stopped=r.command('session.stop',{'target':target,'reason':'public34 ordinary Linux owned helper gate'},phases=('confirmed',))
    assert not same(physical,birth(physical['pid']))
    try:os.kill(-physical['pgid'],0)
    except ProcessLookupError:pass
    else:raise AssertionError('original synthetic helper process group remains')
    r.record('original-helper-natural-product-stop',{'launchOriginal':original,'launchReceipt':launch,
        'stopOriginal':stop_original,'stopReceipt':stopped,'originalBirth':physical,'physicalBirthAndGroupGone':True,
        'syntheticProtocolOnly':True,'actualLLM':False})
    r.stop();r.boot()
    recovered=r.rpc('command.lookup',{'requestId':original['requestId'],'dbId':original['dbId']})
    assert recovered==dict(launch,historical=True)
    assert body_count()==1 and r.native_log.read_bytes()==before
    assert not same(physical,birth(physical['pid']))
    r.stop();r.report.update(originalSyntheticInputCount=1,reopenedOriginalLookupOnly=True,realModel=False)
