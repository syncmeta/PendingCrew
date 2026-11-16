"""Pure c84 periodic-first-round progress contract for this small QA fixture.

No clock reads, RPC, producer tick, product, SQL, model, network or credentials.
The caller supplies actual status/monotonic observations; no run is synthesized.
"""
import copy
import math
import uuid

SCANS={'work':0,'entries':1,'channels':2}
RUN_KEYS={'runId','startedAtMs','completedAtMs','scan','after','capturedSeq','scannedCounts','findings','counts','truncated'}
STATUS_KEYS={'crewId','run','error','captainBrief','manualRun','manualRunActive','manualError'}
FINDINGS={'unowned','lost','stop_escalated','long_waits','unclaimed','needs_ruling','archive_candidates',
          'idle_archive_candidates','incomplete_channels','archive_review_incomplete'}


def require(condition,code):
    if not condition:raise ValueError(code)


def number(value):return type(value) in [int,float] and math.isfinite(value)


def integer(value):return type(value) is int and 0<=value<=9223372036854775807


def require_fixture_clean(state,errors):
    require(type(errors) is list,'invalid_scripted_fixture_error_list')
    if errors:
        if state is not None:
            if state['failure'] is None:state['failure']='scripted_fixture_failed'
            state['scriptedFixtureErrors']=copy.deepcopy(errors)
        raise ValueError('scripted_fixture_failed')


def owned_active_topic_ids(crew,rows):
    require(type(crew) is str and crew,'invalid_owned_crew')
    require(type(rows) is list,'invalid_public_channel_list')
    seen=set();topics=[]
    for row in rows:
        require(type(row) is dict and type(row.get('channel')) is dict,'invalid_public_channel_summary')
        channel=row['channel'];identity=channel.get('id');owner=channel.get('crewId')
        require(type(identity) is str and identity and len(identity)<=256 and not any(ord(c)<32 or ord(c)==127 for c in identity),
                'invalid_public_channel_identity')
        require(identity not in seen,'duplicate_public_channel_identity');seen.add(identity)
        require(type(owner) is str and owner,'invalid_public_channel_owner')
        require(channel.get('kind') in ['lobby','human','topic','dm'],'invalid_public_channel_kind')
        archived=channel.get('archivedAt')
        require(archived is None or type(archived) is str,'invalid_public_channel_archive_state')
        if owner==crew and channel['kind']=='topic' and archived is None:topics.append(identity)
    require(len(topics)<=25,'owned_active_topic_cut_exceeds_fixed_page')
    return sorted(topics)


def require_topic_cut(state,topic_ids):
    state['latestObservedOwnedActiveTopicIds']=copy.deepcopy(topic_ids)
    try:
        require(state['failure'] is None,'earlier_wait_failure_is_not_recoverable')
        require(type(topic_ids) is list and all(type(v) is str and v for v in topic_ids)
                and len(set(topic_ids))==len(topic_ids) and sorted(topic_ids)==state['ownedActiveTopicIds'],
                'owned_active_topic_cut_changed')
    except BaseException as error:
        if state['failure'] is None:state['failure']=str(error)
        raise


def start(crew,active_ids,topic_ids,source_created_ms,monotonic_start,context):
    require(type(crew) is str and crew,'invalid_owned_crew')
    require(type(active_ids) is list and bool(active_ids) and all(type(v) is str and v for v in active_ids),'invalid_active_crews')
    require(len(set(active_ids))==len(active_ids) and crew in active_ids,'active_crew_cut_not_unique_or_missing_owned')
    require(type(topic_ids) is list and all(type(v) is str and v for v in topic_ids)
            and len(set(topic_ids))==len(topic_ids) and len(topic_ids)<=25,'invalid_owned_active_topic_cut')
    require(integer(source_created_ms) and number(monotonic_start),'invalid_source_cut_or_clock')
    require(context in ['readiness-unconfigured','full-after-committed-settings'],'invalid_source_stage_context')
    heartbeat=30.0;cycle=heartbeat*(len(active_ids)+1);total=3*cycle+heartbeat
    return {'crewId':crew,'activeCrewIds':sorted(active_ids),'N':len(active_ids),'context':context,
            'ownedActiveTopicIds':sorted(topic_ids),'expectedOwnedActiveTopicCount':len(topic_ids),
            'sourceCreatedAtMs':source_created_ms,'heartbeatSeconds':heartbeat,'cycleSeconds':cycle,
            'fixedPageLimit':25,'fixedRequiredPages':3,'totalBudgetSeconds':total,
            'noProgressBudgetSeconds':cycle+heartbeat,'startedMonotonic':monotonic_start,
            'deadlineMonotonic':monotonic_start+total,'lastProgressMonotonic':monotonic_start,
            'lastObservedMonotonic':monotonic_start,'samples':0,'firstRunIdentity':None,
            'lastRun':None,'firstCompleteRun':None,'completeReadCount':0,'progressEvents':[],
            'completedConfirmed':False,'failure':None}


def validate_run(run,state):
    require(type(run) is dict and set(run)==RUN_KEYS,'invalid_run_shape')
    try:valid_uuid=type(run['runId']) is str and str(uuid.UUID(run['runId']))==run['runId']
    except (ValueError,TypeError,AttributeError):valid_uuid=False
    require(valid_uuid,'invalid_original_run_uuid')
    require(integer(run['startedAtMs']) and integer(run['capturedSeq']),'invalid_original_run_cut')
    require(run['startedAtMs']>=state['sourceCreatedAtMs'],'producer_started_before_original_source_facts')
    require(type(run['scan']) is str and run['scan'] in SCANS,'invalid_scan_phase')
    require(run['after'] is None,'unexpected_extra_page_outside_fixed_fixture')
    require(type(run['truncated']) is bool and run['truncated'] is False,'truncated_fixed_fixture')
    scanned=run['scannedCounts'];require(type(scanned) is dict and set(scanned)<=set(SCANS),'invalid_scanned_coverage')
    require(all(integer(v) and v<=25 for v in scanned.values()),'invalid_or_extra_scanned_page')
    if 'work' in scanned:require(scanned['work']==1,'original_one_work_not_covered')
    if SCANS[run['scan']]>=1:require('work' in scanned,'missing_prior_work_page')
    if SCANS[run['scan']]>=2:require({'work','entries'}<=set(scanned),'missing_prior_entries_page')
    findings=run['findings'];counts=run['counts']
    require(type(findings) is dict and type(counts) is dict and set(findings)==set(counts)<=FINDINGS,'invalid_finding_keys')
    for key,ids in findings.items():
        require(type(ids) is list and bool(ids) and len(ids)<=50 and all(type(v) is str and v for v in ids),'invalid_finding_ids')
        require(len(set(ids))==len(ids) and integer(counts[key]) and len(ids)<=counts[key]<=50,'invalid_finding_counts')
    completed=run['completedAtMs']
    if completed is not None:
        require(integer(completed) and completed>=run['startedAtMs'],'invalid_completed_timestamp')
        require(run['scan']=='channels' and set(scanned)==set(SCANS),'incomplete_final_coverage')
        require(scanned['channels']==state['expectedOwnedActiveTopicCount'],'original_owned_active_topic_count_not_covered')
    return run


def observe(state,status,monotonic_now):
    state['latestRawStatus']=copy.deepcopy(status)
    try:
        require(state['failure'] is None,'earlier_wait_failure_is_not_recoverable')
        require(number(monotonic_now) and monotonic_now>=state['lastObservedMonotonic'],'monotonic_clock_regressed')
        state['lastObservedMonotonic']=monotonic_now;state['samples']+=1
        require(monotonic_now<state['deadlineMonotonic'],'total_deadline_exceeded')
        require(monotonic_now-state['lastProgressMonotonic']<state['noProgressBudgetSeconds'],'producer_stalled')
        require(type(status) is dict and {'crewId','run','error'}<=set(status)<=STATUS_KEYS,'invalid_periodic_status_shape')
        require(status['crewId']==state['crewId'],'foreign_patrol_crew')
        require(status['error'] is None,'actual_periodic_producer_error')
        require(status.get('manualRun') is None and status.get('manualError') is None and (status.get('manualRunActive') is None or status.get('manualRunActive') is False),
                'manual_patrol_cannot_supply_periodic_proof')
        if status['run'] is None:
            require(state['firstRunIdentity'] is None,'original_run_disappeared')
            return 'waiting-for-first-run'
        run=validate_run(status['run'],state)
        identity={key:run[key] for key in ['runId','startedAtMs','capturedSeq']}
        previous=state['lastRun'];progress=previous is None
        if state['firstRunIdentity'] is None:state['firstRunIdentity']=copy.deepcopy(identity)
        else:require(identity==state['firstRunIdentity'],'original_run_identity_replaced')
        if previous is not None:
            require(SCANS[run['scan']]>=SCANS[previous['scan']],'scan_phase_regressed')
            for field in ['scannedCounts','counts']:
                require(set(previous[field])<=set(run[field]) and all(run[field][k]>=v for k,v in previous[field].items()),field+'_regressed')
            for key,ids in previous['findings'].items():require(run['findings'].get(key,[])[:len(ids)]==ids,'finding_identity_regressed')
            require(previous['completedAtMs'] is None or run==previous,'completed_original_run_changed')
            progress=(run!=previous)
        if progress:
            state['lastProgressMonotonic']=monotonic_now
            state['progressEvents'].append({'monotonic':monotonic_now,'identity':copy.deepcopy(identity),
                'scan':run['scan'],'scannedCounts':copy.deepcopy(run['scannedCounts']),'completedAtMs':run['completedAtMs']})
        state['lastRun']=copy.deepcopy(run)
        if run['completedAtMs'] is None:return 'producer-progress' if progress else 'unchanged'
        if state['firstCompleteRun'] is None:
            state['firstCompleteRun']=copy.deepcopy(run);state['completeReadCount']=1
            return 'first-complete-read'
        require(run==state['firstCompleteRun'],'completed_double_read_not_identical')
        state['completeReadCount']+=1;state['completedConfirmed']=True
        return 'completed-confirmed'
    except BaseException as error:
        if state['failure'] is None:state['failure']=str(error)
        raise
