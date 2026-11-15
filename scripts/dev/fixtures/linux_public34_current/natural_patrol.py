"""Two Linux-only, no-executor natural N6 first rounds; no Jev/native account.

The original eleven runners retain their zero-business guard. Only this subclass
permits its exact original public Work objects. No timer/tick/product override.
"""
import copy,datetime,json,time,uuid
from pathlib import Path
from common import sha
from cold import immutable,meta
from runtime import Runner
import patrol_contract as small
import patrol_n6_contract as fixed


def created_ms(work):
    at=datetime.datetime.fromisoformat(work['createdAt'].replace('Z','+00:00'))
    assert at.tzinfo is not None
    delta=at-datetime.datetime(1970,1,1,tzinfo=datetime.timezone.utc)
    return delta.days*86400000+delta.seconds*1000+delta.microseconds//1000


class PatrolRunner(Runner):
    def __init__(self,*args):
        super().__init__(*args)
        self.works={};self.receipts=[];self.other_runs={};self.other_partial=set();self.spans=[]
        self.final_target=None;self.crew=None
        self.report.update(LinuxNoExecutorNaturalN6=True,realModelOrAccount=False,
            noNativeCLI=True,noJev=True,heartbeatSeconds=30,unchangedDeadlineSeconds=660)

    def rpc(self,method,params,label=None):
        assert method not in ['work.patrolStart','judgment.settings.set','adminCase.secretaryRequest','session.start']
        before=time.monotonic();error=None
        try:return super().rpc(method,params,label)
        except BaseException as exc:error=type(exc).__name__;raise
        finally:
            value={'method':method,'requestId':params.get('requestId'),'beforeMonotonic':before,
                   'afterMonotonic':time.monotonic(),'errorClass':error,'attempts':1}
            self.spans.append(value);self.record('natural-patrol-original-RPC-span',value)

    def quiet(self):
        assert self.rpc('session.list',{'includeRemoved':True})==[]
        assert self.rpc('ask.list',{})==[]
        found=self.rpc('work.list',{'liveOnly':False,'includeRemoved':True,'limit':200})
        assert found['nextAfter'] is None and {v['id'] for v in found['items']}==set(self.works)
        assert len(found['items'])==len(self.works)
        for key,expected in self.works.items():assert self.rpc('work.get',{'id':key})==expected
        settings=self.rpc('judgment.settings.get',{})['view']
        assert settings['settings']['config'] is None and settings['credentialState']=='missing'
        assert settings['pendingRequestId'] is None

    def source(self,total):
        assert total in (1,61)
        self.boot();self.dag()
        original=self.rpc('crew.list',{'includeRemoved':False});assert len(original)==5
        self.original_crew_ids=sorted(x['id'] for x in original)
        intent,receipt=self.command('crew.create',{'title':'Linux natural no-executor N6 '+str(total),
                                                 'workingDirectory':str(self.runtime/'work')})
        self.crew=receipt['result']['id'];assert self.crew not in self.original_crew_ids
        self.original_crew_intent=intent;self.original_crew_receipt=receipt
        before=self.rpc('work.patrolStatus',{'crewId':self.crew})
        assert before['run'] is None and before['error'] is None,'first run already started; retain failure, never reset'
        self.natural_began=time.monotonic()
        for index in range(total):
            request=str(uuid.uuid4())
            params={'requestId':request,'crewId':self.crew,'title':f'Linux stable unowned Work {index+1:02d}',
                    'goal':'Natural fixed page coverage','acceptance':'Remain exactly unchanged','owner':None}
            self.record('original-Linux-unowned-Work-intent',{'params':params,'submitCount':1,'neverResubmit':True})
            result=self.rpc('work.create',params)
            assert result['crewId']==self.crew and result['owner'] is None and 'channelAssociation' not in result
            self.receipts.append({'params':params,'receipt':copy.deepcopy(result)})
            assert result['id'] not in self.works;self.works[result['id']]=copy.deepcopy(result)
        self.quiet()
        active=self.rpc('crew.list',{'includeRemoved':False});ids=sorted(v['id'] for v in active)
        assert ids==sorted(self.original_crew_ids+[self.crew]) and len(ids)==6
        assert small.owned_active_topic_ids(self.crew,self.rpc('channel.list',{'crewId':self.crew}))==[]
        self.cut=max(created_ms(x) for x in self.works.values())
        self.record('exact-Linux-natural-source-cut',{'crewId':self.crew,'activeCrewIds':ids,
            'fixedWorkCount':total,'allUnowned':True,'sourceCreatedAtMs':self.cut,
            'fullOriginalReceipts':self.receipts,'originalCrewCommand':intent,'crewReceipt':receipt,
            'noExecutorOrJev':True,'activeTopics':[],'noHSourceCreated':True})
        return ids

    def wait_natural(self,total,ids):
        began=self.natural_began;keys=sorted(self.works,key=lambda k:int(k[1:]))
        state=(fixed.start_large(self.crew,ids,[],self.cut,began,keys[0],keys[1:]) if total==61
               else fixed.start_small(self.crew,ids,[],self.cut,began))
        state['fixedWorkIds']=keys
        assert state['totalBudgetSeconds']==660 and state['noProgressBudgetSeconds']==240
        self.report['naturalWait']=state
        observer=fixed.observe_large if total==61 else fixed.observe_small
        timeout=self.human.socket.gettimeout();previous_others={};first=None;confirmed=None;last_progress=None
        try:
            def remaining():
                assert not self.watch_errors
                left=state['deadlineMonotonic']-time.monotonic();assert left>0
                self.human.socket.settimeout(min(5,left));return left
            while True:
                remaining();status=self.rpc('work.patrolStatus',{'crewId':self.crew});now=time.monotonic()
                event=observer(state,status,now)
                for other in self.original_crew_ids:
                    remaining();observed=self.rpc('work.patrolStatus',{'crewId':other})
                    run=fixed.empty_other(observed,other,self.other_runs.get(other));self.other_runs[other]=run
                    if run is not None and run['completedAtMs'] is not None and state['lastRun'] is not None and state['lastRun']['completedAtMs'] is None:
                        self.other_partial.add(other)
                    if observed!=previous_others.get(other):
                        self.record('all-initialization-crew-natural-progress',{'crewId':other,'status':observed,
                            'targetPartial':state['lastRun'] is not None and state['lastRun']['completedAtMs'] is None,
                            'observedMonotonic':time.monotonic()})
                        previous_others[other]=copy.deepcopy(observed)
                if event not in ['unchanged','waiting-for-first-run']:
                    small.require_topic_cut(state,small.owned_active_topic_ids(self.crew,self.rpc('channel.list',{'crewId':self.crew})))
                    assert sorted(x['id'] for x in self.rpc('crew.list',{'includeRemoved':False}))==ids
                    self.record('actual-Linux-natural-selected-progress',{'event':event,'status':status,
                        'observedMonotonic':now,'state':state,'otherFive':self.other_runs})
                    last_progress=now
                if event=='first-complete-read':first=now
                if event=='completed-confirmed':
                    if confirmed is None:confirmed=now
                    if len(self.other_runs)==5 and all(v is not None and v['completedAtMs'] is not None for v in self.other_runs.values()):break
                time.sleep(min(1,remaining()))
            self.final_target=copy.deepcopy(state['firstCompleteRun'])
            # Both Linux cases require all six first natural rounds. The large
            # fixture additionally witnesses every other crew during its pages.
            assert all(v is not None and v['completedAtMs'] is not None for v in self.other_runs.values())
            assert set(self.other_runs)==set(self.original_crew_ids)
            if total==61:assert self.other_partial==set(self.original_crew_ids)
            self.quiet()
            last=[x for x in self.spans if x['method']=='work.create'][-1]
            self.report['actualLinuxNaturalTiming']={'sourceFactsCreatedAtMs':self.cut,'fixtureConstructionStartedMonotonic':began,
                'finalCreateRPCBeforeMonotonic':last['beforeMonotonic'],'finalCreateRPCAfterMonotonic':last['afterMonotonic'],
                'firstCompleteObservedMonotonic':first,'doubleReadConfirmedMonotonic':confirmed,'allSixCompleteObservedMonotonic':now,
                'finalCreateReturnToFirstObservedSeconds':first-last['afterMonotonic'],
                'firstCompleteToDoubleReadSeconds':confirmed-first,'lastProgressObservedMonotonic':last_progress,
                'coreStartedAtMs':self.final_target['startedAtMs'],'coreCompletedAtMs':self.final_target['completedAtMs'],
                'coreBackgroundTimestampDeltaMs':self.final_target['completedAtMs']-self.final_target['startedAtMs'],
                'sameTimestampMeansSameTransactionNotZeroCPU':True,'foregroundLatencyMeasured':False,
                'historicalMac420_004NotThisLinuxResult':True,'LinuxMacFixturesDistinct':True}
        finally:self.human.socket.settimeout(timeout)

    def cold_proof(self):
        assert self.proc is None and self.human is None
        directory=self.out/'cold'/f'phase-{self.phase}';db=directory/'crewd.db';before=sha(db)
        with immutable(db) as c:
            assert c.execute('PRAGMA user_version').fetchone()[0]==34
            scope={'dbId':meta(c,'dbId'),'restoreEpoch':meta(c,'personalDraft.restoreStamp.v1')}
            revision=c.execute('SELECT revision FROM remote_configuration WHERE singleton=1').fetchone()[0]
            epoch=fixed.judgment_epoch(meta(c,'judgment.restoreEpoch.v1'),meta(c,'judgment.remoteRevision.v1'),revision)
            checks={}
            for crew,run in {self.crew:self.final_target,**self.other_runs}.items():
                value=json.loads(meta(c,'patrol.secretary.completed.v1:'+crew))
                checks[crew]=fixed.completed_checksum(value,crew,scope,epoch,run)
            assert len(checks)==6
            creations=c.execute('SELECT request_id,work_id,crew_id,creator_key,source_channel_id,payload_digest FROM work_creation_requests ORDER BY request_id').fetchall()
            assert len(creations)==len(self.receipts)
            by_request={x[0]:x for x in creations};assert len(by_request)==len(creations)
            for original in self.receipts:
                row=by_request[original['params']['requestId']]
                assert row[1:5]==(original['receipt']['id'],self.crew,'human',None)
                assert type(row[5]) is str and len(row[5])==64 and all(x in '0123456789abcdef' for x in row[5])
            assert c.execute('SELECT count(*) FROM sessions').fetchone()[0]==0
            assert c.execute('SELECT count(*) FROM ask_meta').fetchone()[0]==0
            assert c.execute('SELECT count(*) FROM wakes').fetchone()[0]==0
        assert sha(db)==before
        self.report.update(originalNaturalRunChecksums=checks,actualCompletedScope=scope,actualJudgmentEpoch=epoch,
            fixedUnownedWorkCount=len(self.works),allWorkFullObjectsUnchanged=True,originalWorkUUIDsSubmittedOnce=True,
            originalWorkCreationWitnesses=creations,allSixNaturalFirstRounds=True,regularFileCaptureEOF='not-a-pipe',
            coldOnlyAfterOriginalWaitAndSocketGone=True,otherFiveCompletedWhileTargetPartial=sorted(self.other_partial))
        self.record('Linux-natural-original-cold-proof',{'checksums':checks,'scope':scope,'epoch':epoch,
                    'DBSHA256':before,'originalWorkCreationRows':creations,'timing':self.report['actualLinuxNaturalTiming']})


def natural_flow(r,total):
    ids=r.source(total);r.wait_natural(total,ids);r.stop();r.cold_proof()

def small_flow(r):natural_flow(r,1)
def large_flow(r):natural_flow(r,61)
