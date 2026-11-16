"""Pure closed N6 contracts: unchanged QA11 small; separate exact61 Work case.

No RPC, clock read, SQL, process or model. No generic pagination relaxation.
"""
import copy
import hashlib
import json
import re
import uuid
import patrol_contract as small
from patrol_contract import require, integer, number, SCANS, RUN_KEYS, STATUS_KEYS
def exact_value(left,right):
    # Preserve JSON type differences such as false versus 0, unlike Python ==.
    return json.dumps(left,ensure_ascii=False,sort_keys=True,separators=(',',':'),allow_nan=False)==json.dumps(
        right,ensure_ascii=False,sort_keys=True,separators=(',',':'),allow_nan=False)

RUN_ORDER=('runId','startedAtMs','completedAtMs','scan','after','capturedSeq','scannedCounts','findings','counts','truncated')

def work_create_payload(params):
    """Only this N6 fixture's six input fields, serialized as the real12-field DTO.

    Fixed7572 proto/work.rs144-169 declares this order and WorkLevel defaults to
    local. collab/work.rs563-565 clears only request_id before acl::digest.
    This is a witness calculation, never a replacement work.create request.
    """
    require(type(params) is dict and set(params)=={'requestId','crewId','title','goal','acceptance','owner'},
            'unexpected_N6_work_create_params')
    for key in ['requestId','crewId','title','goal','acceptance']:
        require(type(params[key]) is str,'invalid_N6_work_create_string')
        params[key].encode('utf-8')
    require(bool(params['crewId']) and params['owner'] is None,'invalid_N6_unowned_work_input')
    try:valid=str(uuid.UUID(params['requestId']))==params['requestId']
    except (ValueError,TypeError,AttributeError):valid=False
    require(valid,'invalid_original_work_create_uuid')
    return {'requestId':None,'channelId':None,'crewId':params['crewId'],'title':params['title'],
            'goal':params['goal'],'level':'local','acceptance':params['acceptance'],'owner':None,
            'parent':None,'dependsOn':[],'sources':[],'nextCheckAt':None}

def work_create_digest(params):
    payload=work_create_payload(params)
    # resource_acl::digest uses typed serde_json::to_vec, not sorted Value keys.
    encoded=json.dumps(payload,ensure_ascii=False,separators=(',',':'),allow_nan=False).encode('utf-8')
    return hashlib.sha256(encoded).hexdigest()

def work_creation_witness(row,original,crew):
    """Qualify the SQL typeof/length/lower(hex) projection; return only JSON data."""
    require(type(row) in (tuple,list) and len(row)==8,'invalid_work_creation_projection')
    require(type(original) is dict and set(original)=={'params','receipt'},'invalid_original_work_creation')
    params=original['params'];payload=work_create_payload(params);receipt=original['receipt']
    require(type(crew) is str and bool(crew) and params['crewId']==crew,'foreign_work_creation_crew')
    require(type(receipt) is dict and type(receipt.get('id')) is str
            and re.fullmatch(r'W[1-9][0-9]*',receipt['id']) is not None
            and type(receipt.get('crewId')) is str and receipt['crewId']==crew
            and 'owner' in receipt and receipt['owner'] is None,'invalid_original_work_creation_receipt')
    require(type(row[0]) is str and row[0]==params['requestId']
            and type(row[1]) is str and row[1]==receipt['id']
            and type(row[2]) is str and row[2]==crew
            and type(row[3]) is str and row[3]=='human' and row[4] is None,
            'original_work_creation_identity_differs')
    require(row[5]=='blob' and type(row[5]) is str and type(row[6]) is int and row[6]==32,
            'work_creation_digest_is_not_BLOB32')
    require(type(row[7]) is str and re.fullmatch(r'[0-9a-f]{64}',row[7]) is not None,
            'invalid_work_creation_digest_hex_projection')
    expected=work_create_digest(params)
    require(row[7]==expected,'work_creation_digest_differs_from_original_typed_params')
    return {'requestId':row[0],'workId':row[1],'crewId':row[2],'creatorKey':row[3],
            'sourceChannelId':row[4],'payloadDigest':{'storageClass':row[5],'byteLength':row[6],'hex':row[7]},
            'originalTypedPayload':payload,'expectedPayloadDigestHex':expected}

def start_small(crew,active,topics,created,began):
    require(len(active)==6 and topics==[],'exact_N6_zero_topic_fixture_required')
    state=small.start(crew,active,topics,created,began,'readiness-unconfigured')
    require(state['totalBudgetSeconds']==660,'original_660_second_bound_changed')
    return state

def observe_small(state,status,now):
    # Preserve the original small guard; add only a stricter source ordering
    # witness. Equal-ms source/start order is ambiguous and fails this QA.
    try:
        run=status.get('run') if type(status) is dict else None
        if type(run) is dict:
            require(integer(run.get('startedAtMs')) and run['startedAtMs']>state['sourceCreatedAtMs'],
                    'source_fact_start_order_not_strictly_witnessed')
        if type(run) is dict:
            require(run['scannedCounts'].get('work')==1 and run['findings']=={'unowned':state['fixedWorkIds']} and run['counts']=={'unowned':1},'Linux_small_one_unowned_work_contract')
            require(all(v==0 for k,v in run['scannedCounts'].items() if k!='work'),'Linux_small_unexpected_entries_or_channels')
        return small.observe(state,status,now)
    except BaseException as error:
        if state['failure'] is None:state['failure']=str(error)
        raise

def work_ids(original,additional):
    require(type(original) is str and re.fullmatch(r'W[1-9][0-9]*',original),'invalid_original_work')
    require(type(additional) is list and len(additional)==60,'exact_sixty_additional_work_required')
    require(all(type(x) is str and re.fullmatch(r'W[1-9][0-9]*',x) for x in additional),'invalid_added_work_id')
    require(len(set(additional+[original]))==61,'duplicate_or_original_added_work')
    require(all(int(x[1:])>int(original[1:]) for x in additional),'additional_work_not_after_original')
    return [original]+sorted(additional,key=lambda x:int(x[1:]))

def start_large(crew,active,topics,created,began,original,additional):
    state=start_small(crew,active,topics,created,began)
    state.update(fixedFixture='Linux-exact61-all-unowned',fixedWorkIds=work_ids(original,additional),
                 expectedUnownedIds=work_ids(original,additional),fixedRequiredPages=3)
    return state

def validate_large(run,state):
    require(type(run) is dict and set(run)==RUN_KEYS,'invalid_run_shape')
    try:valid=type(run['runId']) is str and str(uuid.UUID(run['runId']))==run['runId']
    except (ValueError,TypeError,AttributeError):valid=False
    require(valid,'invalid_original_run_uuid')
    require(integer(run['startedAtMs']) and integer(run['capturedSeq']),'invalid_original_run_cut')
    require(run['startedAtMs']>state['sourceCreatedAtMs'],'source_fact_start_order_not_strictly_witnessed')
    scanned=run['scannedCounts'];require(type(scanned) is dict,'invalid_scanned_coverage')
    n=scanned.get('work');require(type(n) is int and n in [25,50,61],'missing_or_invented_fixed_work_page')
    require(type(run['truncated']) is bool and run['truncated']==(n==61),'incorrect_fixed61_truncation')
    if n<61:
        require(scanned=={'work':n} and run['scan']=='work' and run['completedAtMs'] is None,'premature_phase_or_complete')
        require(run['after']==state['fixedWorkIds'][n-1],'incorrect_real_work_page_cursor')
    else:
        require(scanned=={'work':61,'entries':0,'channels':0} and run['scan']=='channels' and run['after'] is None,'missing_final_natural_three_coverage')
        require(integer(run['completedAtMs']) and run['completedAtMs']>=run['startedAtMs'],'invalid_completed_timestamp')
    findings=run['findings'];counts=run['counts']
    require(type(findings) is dict and type(counts) is dict and set(findings)==set(counts),'invalid_finding_keys')
    require('unowned' in findings and set(findings)=={'unowned'},'unexpected_fixed61_finding_category')
    expected=n;sample=state['expectedUnownedIds'][:min(expected,50)]
    require(type(counts['unowned']) is int and counts['unowned']==expected,'incorrect_real_unowned_count')
    require(findings['unowned']==sample,'missing_duplicate_or_foreign_unowned_sample')
    for category in set(findings)-{'unowned'}:
        require(findings[category]==[state['fixedWorkIds'][0]] and type(counts[category]) is int and counts[category]==1,'unexpected_owned_original_work_finding')
    return run

def observe_large(state,status,now):
    state['latestRawStatus']=copy.deepcopy(status)
    try:
        require(state['failure'] is None,'earlier_wait_failure_is_not_recoverable')
        require(number(now) and now>=state['lastObservedMonotonic'],'monotonic_clock_regressed')
        state['lastObservedMonotonic']=now;state['samples']+=1
        require(now<state['deadlineMonotonic'],'total_deadline_exceeded')
        require(now-state['lastProgressMonotonic']<state['noProgressBudgetSeconds'],'producer_stalled')
        require(type(status) is dict and {'crewId','run','error'}<=set(status)<=STATUS_KEYS,'invalid_periodic_status_shape')
        require(status['crewId']==state['crewId'] and status['error'] is None,'foreign_crew_or_actual_producer_error')
        require(status.get('manualRun') is None and status.get('manualError') is None and (status.get('manualRunActive') is None or status.get('manualRunActive') is False),'manual_patrol_cannot_supply_periodic_proof')
        if status['run'] is None:
            require(state['firstRunIdentity'] is None,'original_run_disappeared');return 'waiting-for-first-run'
        run=validate_large(status['run'],state);previous=state['lastRun']
        identity={k:run[k] for k in ['runId','startedAtMs','capturedSeq']}
        if state['firstRunIdentity'] is None:
            require(run['scannedCounts']['work']==25,'first_real_work_page_was_not_observed')
            state['firstRunIdentity']=copy.deepcopy(identity)
        else:require(identity==state['firstRunIdentity'],'original_run_identity_replaced')
        progress=previous is None or run!=previous
        if previous is not None:
            before=previous['scannedCounts']['work'];after=run['scannedCounts']['work']
            require(after==before or (before,after) in [(25,50),(50,61)],'missing_or_regressed_real_work_page')
            require(sum(run['scannedCounts'].values())-sum(previous['scannedCounts'].values())<=25,'selected_fact_increment_exceeds25')
            for field in ['scannedCounts','counts']:
                require(set(previous[field])<=set(run[field]) and all(run[field][k]>=v for k,v in previous[field].items()),field+'_regressed')
            for k,ids in previous['findings'].items():require(run['findings'].get(k,[])[:len(ids)]==ids,'finding_identity_regressed')
            require(previous['completedAtMs'] is None or run==previous,'completed_original_run_changed')
        if progress:
            state['lastProgressMonotonic']=now
            state['progressEvents'].append({'monotonic':now,'identity':copy.deepcopy(identity),'scan':run['scan'],
                'scannedCounts':copy.deepcopy(run['scannedCounts']),'completedAtMs':run['completedAtMs'],'after':run['after']})
        state['lastRun']=copy.deepcopy(run)
        if run['completedAtMs'] is None:return 'producer-progress' if progress else 'unchanged'
        if state['firstCompleteRun'] is None:
            require([p['scannedCounts']['work'] for p in state['progressEvents']]==[25,50,61],'three_real_work_pages_missing')
            state['firstCompleteRun']=copy.deepcopy(run);state['completeReadCount']=1;return 'first-complete-read'
        require(run==state['firstCompleteRun'],'completed_double_read_not_identical')
        state['completeReadCount']+=1;state['completedConfirmed']=True;return 'completed-confirmed'
    except BaseException as error:
        if state['failure'] is None:state['failure']=str(error)
        raise

def empty_other(status,crew,previous):
    require(type(status) is dict and {'crewId','run','error'}<=set(status)<=STATUS_KEYS,'invalid_other_status_shape')
    require(status['crewId']==crew and status['error'] is None,'other_crew_error_or_mismatch')
    require(status.get('manualRun') is None and status.get('manualError') is None and (status.get('manualRunActive') is None or status.get('manualRunActive') is False),'other_manual_not_periodic')
    run=status['run']
    if run is None:
        require(previous is None,'other_original_run_disappeared');return None
    require(type(run) is dict and set(run)==RUN_KEYS,'invalid_other_run_shape')
    require(type(run['runId']) is str and str(uuid.UUID(run['runId']))==run['runId'],'invalid_other_original_uuid')
    require(integer(run['startedAtMs']) and integer(run['capturedSeq']),'invalid_other_run_cut')
    require(run['scan'] in SCANS and run['after'] is None and run['truncated'] is False,'invalid_other_empty_phase')
    require(type(run['scannedCounts']) is dict and set(run['scannedCounts'])<=set(SCANS) and all(type(n) is int and n==0 for n in run['scannedCounts'].values()),'other_crew_not_empty_fixed_fixture')
    require(run['counts']=={} and run['findings']=={},'other_crew_unexpected_findings')
    if previous:
        require(all(run[k]==previous[k] for k in ['runId','startedAtMs','capturedSeq']),'other_original_identity_changed')
        require(SCANS[run['scan']]>=SCANS[previous['scan']] and set(previous['scannedCounts'])<=set(run['scannedCounts']),'other_progress_regressed')
        require(previous['completedAtMs'] is None or run==previous,'other_completed_run_changed')
    if run['completedAtMs'] is not None:
        require(integer(run['completedAtMs']) and run['completedAtMs']>=run['startedAtMs'] and run['scan']=='channels' and set(run['scannedCounts'])==set(SCANS),'other_fake_complete')
    return copy.deepcopy(run)

def completed_checksum(value,crew,scope,epoch,run):
    require(type(value) is dict and set(value)=={'version','crew','scope','epoch','run','checksum'},'invalid_completed_metadata_shape')
    require(type(value['version']) is int and value['version']==1 and value['crew']==crew and exact_value(value['scope'],scope) and value['epoch']==epoch and exact_value(value['run'],run),'completed_metadata_not_original_scope_or_run')
    require(type(scope) is dict and set(scope)=={'dbId','restoreEpoch'} and all(type(v) is str and v for v in scope.values()),'invalid_actual_completed_scope')
    require(type(epoch) is str and epoch and type(crew) is str and crew,'invalid_original_scope_epoch')
    ordered={k:run[k] for k in RUN_ORDER}
    for k in ['scannedCounts','findings','counts']:ordered[k]={name:run[k][name] for name in sorted(run[k])}
    material=[1,crew,{'dbId':scope['dbId'],'restoreEpoch':scope['restoreEpoch']},epoch,ordered]
    wanted=hashlib.sha256(json.dumps(material,ensure_ascii=False,separators=(',',':'),allow_nan=False).encode()).hexdigest()
    require(type(value['checksum']) is str and value['checksum']==wanted,'actual_completed_checksum_differs')
    return wanted

def judgment_epoch(key,observed,remote_revision):
    """Exact frozen judgment_state::epoch semantics; active config ID is unrelated."""
    require(type(remote_revision) is int and remote_revision>0,'invalid_actual_remote_revision')
    require(key is None or type(key) is str,'invalid_restore_epoch_metadata_type')
    require(observed is None or type(observed) is str,'invalid_remote_witness_metadata_type')
    try:valid=type(key) is str and bool(uuid.UUID(key))
    except (ValueError,TypeError,AttributeError):valid=False
    if valid and observed==str(remote_revision):return key
    return 'unwitnessed:'+(key or '')+':'+(observed or '')+':'+str(remote_revision)
