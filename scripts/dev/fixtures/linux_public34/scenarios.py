"""Complete Root-only flows; original business intents are each sent once."""
import json,os,stat,uuid
from pathlib import Path
from common import sha
from cold import ROOT,CREWS,closure

def tree_bytes(root):
    result={}
    for p in root.rglob('*'):
        info=p.lstat()
        if stat.S_ISREG(info.st_mode):result[str(p.relative_to(root))]=sha(p)
        elif stat.S_ISDIR(info.st_mode):continue
        else:result[str(p.relative_to(root))]={'mode':info.st_mode,'device':info.st_dev,'inode':info.st_ino}
    return result

def fixed_facts(r,value):
    assert value['dbId']==r.initial['dbId'] and value['facts']==r.initial['facts']

def original_table_rows(c):
    # Catalog comes from our actual stopped product copy, not caller SQL.
    tables=[x[0] for x in c.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
    def cell(value):
        if isinstance(value,bytes):return {'sqliteBlobHex':value.hex()}
        return value
    return {name:sorted([[cell(v) for v in row] for row in c.execute('SELECT * FROM "'+name.replace('"','""')+'"')],
        key=lambda row:json.dumps(row,sort_keys=True,separators=(',',':'))) for name in tables}

def fresh_contract(r):
    r.fresh();r.stop();before=r.cold();fixed_facts(r,before)
    draft=r.data/'welcome-crew/INDEX.md';draft.chmod(0o600);draft.write_text('Owned private simulated user draft: 必须保留全文。\n')
    note=r.data/'welcome-crew/user-private-note.txt';note.write_text('Never adopt or delete an unknown user file.\n')
    saved=(draft.read_bytes(),note.read_bytes())
    r.record('private-user-files-before-reopen',{'draft':str(draft),'draftSHA256':sha(draft),'note':str(note),'noteSHA256':sha(note)})
    r.boot();r.dag();objects,context=r.public_pages(r.initial)
    assert (draft.read_bytes(),note.read_bytes())==saved
    original=r.lookup(r.initial['publication']['request'],r.initial['publication']['origin'])
    assert original['lookup']['status']=='known' and original['lookup']['result']==r.initial['builtinReceipt']
    r.stop();after=r.cold();fixed_facts(r,after)
    closure(r.data,after['projection'],context,objects,r.initial['publication']['pages'])
    assert (draft.read_bytes(),note.read_bytes())==saved
    r.report.update(freshABC=True,originalNineResources=True,currentClosure=True,userFilesPreserved=True,builtinOriginalUUIDLookupOnly=True)

def mutation(r,action):
    r.fresh();page=r.initial['publication']['pages'][0];origin=r.workspace()['context'];intent=None
    backup=None
    if action=='edit':
        r.stop();r.cold()
        backup=Path(r.cli('backup')['stdout'].strip())
        assert backup.is_absolute() and backup.parent==r.data/'backups' and backup.is_file()
        r.record('formal-Core-backup-before-Knowledge-disturbance',{'path':str(backup),'sha256':sha(backup)})
        r.boot();origin=r.workspace()['context']
    if action=='unlink':
        listed=r.rpc('channel.knowledgeList',{'channelId':ROOT+'/lobby','limit':100})
        source=next(x['source'] for x in listed['items'] if x['source']['reference']['id']==page['view'])
        params={'requestId':str(uuid.uuid4()),'channelId':ROOT+'/lobby','expectedLinkRevision':listed['linkRevision'],'source':source}
        r.record('original-unlink-intent',{'params':params,'attempts':1,'neverResubmit':True})
        receipt=r.rpc('channel.knowledgeUnlink',params);assert receipt['linked'] is False and receipt['source']==source
        listed=r.rpc('channel.knowledgeList',{'channelId':ROOT+'/lobby','limit':100})
        assert listed['linkRevision']==receipt['linkRevision'] and not any(x['source']['reference']['id']==page['view'] for x in listed['items'])
        after_context=r.workspace()['context'];assert after_context==origin
    else:
        kind='view' if action=='archiveView' else 'block';key=page[kind];old=r.object(origin,kind,key)
        if action=='edit':
            edit={'kind':'updateBlock','id':key,'expectedRevision':old['revision'],'contentType':'text/plain',
                  'content':'Original public ordinary-Core edit: 全文保留；不是模型生成。\n','sources':None,'attributes':{}}
        else:edit={'kind':action,'id':key,'expectedRevision':old['revision'],'archived':True}
        intent=r.submit(origin,[edit],'Original private ordinary onboarding '+action);receipt=intent['receipt'];after_context=receipt['context']
        assert after_context!=origin
        r.refusal('data.request.get',{'requestId':intent['request'],'context':after_context},-32009,'original request owner or immutable context differs')
        changed=r.object(after_context,kind,key);assert changed['id']==key and changed['revision']=='2'
        if action=='edit':assert changed['content']==edit['content']
        else:assert changed['archived'] is True
    expected_objects=r.all_objects(after_context)
    r.stop();after=r.cold();fixed_facts(r,after)
    if action=='unlink':
        original_unlink=[x for x in after['resourceMutationReceipts'] if x['request_id']==params['requestId']]
        assert len(original_unlink)==1 and original_unlink[0]['operation']=='channel.knowledgeUnlink'
        assert original_unlink[0]['object_kind']=='knowledgeView' and json.loads(original_unlink[0]['result'])==receipt
    if action=='edit':
        closure(r.data,after['projection'],after_context,expected_objects,r.initial['publication']['pages'])
        before=tree_bytes(r.data)
        r.cli('restore','--from',backup,expected=None,contains='Core-only restore cannot replace a knowledge workspace')
        assert tree_bytes(r.data)==before,'refused Core-only restore changed the private active data'
        r.record('formal-Core-only-restore-refusal-kept-original-data',{'sourceBackup':str(backup),'dataHashes':before,'refusalNoOverride':True})
    else:assert after['projection']['state']!='ready' and after['projection']['current'] is None
    r.boot();r.dag();now=r.workspace()['context'];assert now==after_context
    assert r.all_objects(now)==expected_objects
    if intent:
        lookup=r.lookup(intent['request'],intent['origin']);assert lookup['lookup']['status']=='known' and lookup['lookup']['result']==intent['receipt']
        r.refusal('data.request.get',{'requestId':intent['request'],'context':now},-32009,'original request owner or immutable context differs')
        r.record('physical-reopen-original-UUID-lookup-only',{'intent':intent,'lookup':lookup,'submits':1,'resubmitted':False})
    if action=='unlink':
        listed=r.rpc('channel.knowledgeList',{'channelId':ROOT+'/lobby','limit':100})
        assert not any(x['source']['reference']['id']==page['view'] for x in listed['items'])
    r.stop();final=r.cold();fixed_facts(r,final)
    if action=='unlink':assert [x for x in final['resourceMutationReceipts'] if x['request_id']==params['requestId']]==original_unlink
    if action=='edit':closure(r.data,final['projection'],now,expected_objects,r.initial['publication']['pages'])
    else:assert final['projection']['state']!='ready' and final['projection']['current'] is None
    r.report.update(normalPublicMutation=action,originalRequestSubmittedOnce=True,physicalReopenNoResurrection=True)

def legacy(r,user=False):
    r.boot(legacy=True);old_crews=r.rpc('crew.list',{'includeRemoved':True})
    assert old_crews==[]
    user_id=None
    if user:
        _,receipt=r.command('crew.create',{'title':'Original genuine33 user organization','workingDirectory':str(r.runtime/'work')})
        user_id=receipt['result']['id'];user_number=receipt['result'].get('number');user_original=r.commands[-1]
    r.stop();old=r.cold(33)
    from cold import immutable,meta,rows,zero_business
    with immutable(r.out/'cold'/f'phase-{r.phase}'/'crewd.db') as original_db:
        original_rows=original_table_rows(original_db)
        original_catalog=original_db.execute('SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name,tbl_name').fetchall()
    assert old['facts'] is None and not any(x['id'] in CREWS for x in old['crews'])
    r.boot();listed=r.rpc('crew.list',{'includeRemoved':True})
    assert not any(c['id'] in CREWS for c in listed)
    assert {c['id'] for c in listed}=={c['id'] for c in old['crews']}
    if user:
        original_user=next(c for c in listed if c['id']==user_id)
        assert original_user['title']=='Original genuine33 user organization' and original_user.get('number')==user_number
    if user:
        recovered=r.rpc('command.lookup',{'requestId':user_original['requestId'],'dbId':user_original['dbId']})
        assert recovered==dict(receipt,historical=True)
        r.record('original-genuine33-request-recovered-without-resubmit',recovered)
    r.stop();new=r.cold()
    protected=list((r.data/'backups').glob('crewd-v33-pre-v34-*.db'))
    if user:
        assert len(protected)==1 and len(original_rows)==124
        with immutable(protected[0]) as snapshot:
            assert snapshot.execute('PRAGMA user_version').fetchone()[0]==33
            assert meta(snapshot,'dbId')==old['dbId'] and rows(snapshot,'crews')==old['crews']
            assert zero_business(snapshot)==old['business']
            assert original_table_rows(snapshot)==original_rows
            assert snapshot.execute('SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name,tbl_name').fetchall()==original_catalog
        r.record('real33-protected-migration-original-cold-facts',{'path':str(protected[0]),'sha256':sha(protected[0]),
            'oldFacts':old,'allOriginalTableRowsIncludingReceipts':original_rows,'originalFullCatalogDDL':original_catalog,'protected33':True})
    else:
        # Core protects a pre-migration snapshot only when has_data() sees
        # crews+messages. A genuine empty33 has neither; don't seed fake data.
        assert protected==[] and old['crews']==[] and new['crews']==[]
        assert old['business']['counts']['messages']==new['business']['counts']['messages']==0
        with immutable(r.out/'cold'/'phase-1'/'crewd.db') as original_db:
            assert original_table_rows(original_db)==original_rows
            assert original_db.execute('SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name,tbl_name').fetchall()==original_catalog
        r.record('real33-empty-no-protective-backup-required',{'noBusinessData':True,
            'hasDataDefinition':'original schema::has_data counts crews+messages',
            'noProtectiveBackupRequired':True,'protectedBackupCount':0,'dbId':old['dbId'],
            'oldFacts':old,'allOriginalTableRowsIncludingReceipts':original_rows,'originalFullCatalogDDL':original_catalog,
            'originalStoppedColdCopyPreserved':True,'syntheticBusinessSeedCreated':False})
    assert new['dbId']==old['dbId'] and sorted(new['crews'],key=lambda x:x['id'])==sorted(old['crews'],key=lambda x:x['id'])
    assert new['facts']['skipped'] is True and new['facts']['pending'] is None and new['facts']['ready']==[]
    assert not list((r.data/'knowledge').rglob('*.sqlite'))
    if user:
        backup=Path(r.cli('backup')['stdout'].strip());assert backup.parent==r.data/'backups' and backup.is_file()
        r.boot();_,disturbance=r.command('crew.create',{'title':'Private post-backup disturbance','workingDirectory':str(r.runtime/'work')})
        extra=disturbance['result']['id'];assert extra!=user_id;r.stop();r.cold()
        r.cli('restore','--from',backup)
        r.boot();listed=r.rpc('crew.list',{'includeRemoved':True})
        assert {x['id'] for x in listed}=={x['id'] for x in old['crews']} and extra not in {x['id'] for x in listed}
        assert next(c for c in listed if c['id']==user_id)['title']=='Original genuine33 user organization'
        assert next(c for c in listed if c['id']==user_id).get('number')==user_number
        r.stop();restored=r.cold();assert restored['dbId']==old['dbId']
        assert restored['facts']['skipped'] and restored['facts']['ready']==[] and restored['facts']['pending'] is None
        r.record('formal-empty-Knowledge-Core-restore-qualified',{'backup':str(backup),'sha256':sha(backup),
            'originalUserCrew':user_id,'disturbanceAbsent':extra,'replacedOriginals':[str(x) for x in r.data.glob('crewd.db.replaced-*')]})
    r.report.update(genuine33NormalProducer=True,oldOrganizationSkippedWithoutTutorial=True,formalCoreRestore=user)

def knowledge_restore(r):
    r.fresh();page=r.initial['publication']['pages'][0];ctx=r.workspace()['context'];key=page['block']
    def update(context,text):
        obj=r.object(context,'block',key)
        return r.submit(context,[{'kind':'updateBlock','id':key,'expectedRevision':obj['revision'],
            'contentType':'text/plain','content':text,'sources':None,'attributes':{}}],'Original bounded Knowledge restore fixture')
    original=update(ctx,'Original pre-snapshot document A — 完整保留。\n');ctx=original['receipt']['context']
    sql=r.submit(ctx,[{'kind':'sql','statements':[{'sql':'CREATE TABLE ordinary_onboarding_sample(id INTEGER PRIMARY KEY,value TEXT)','params':[]},
        {'sql':"INSERT INTO ordinary_onboarding_sample VALUES(1,'original')",'params':[]}]}],'Normal private Knowledge SQL fixture')
    ctx=sql['receipt']['context'];preview=r.rpc('data.preview',{'context':ctx,'statements':[{'sql':"UPDATE ordinary_onboarding_sample SET value='confirmed' WHERE id=1",'params':[]}]})
    confirm_id=str(uuid.uuid4());confirm_params={'requestId':confirm_id,'context':ctx,'token':preview['token']}
    r.record('original-confirm-intent',{'params':confirm_params,'attempts':1})
    confirmed=r.rpc('data.confirm',confirm_params);assert confirmed['phase']=='committed' and confirmed['requestId']==confirm_id
    lookup=r.lookup(confirm_id,ctx);assert lookup['lookup']['status']=='known' and lookup['lookup']['result']==confirmed
    snapshots=r.rpc('data.snapshots',{'context':confirmed['context']});snapshot=confirmed['snapshotId']
    assert snapshot and any(x['id']==snapshot for x in snapshots['items'])
    disturbed=update(confirmed['context'],'Private disturbance B — must be rewound by formal Knowledge restore.\n')
    before=disturbed['receipt']['context'];revision=int(r.object(before,'block',key)['revision'])
    restore_id=str(uuid.uuid4());params={'requestId':restore_id,'context':before,'snapshotId':snapshot}
    r.record('original-Knowledge-restore-intent',{'params':params,'attempts':1})
    restored=r.rpc('data.restore',params);assert restored['phase']=='committed' and restored['requestId']==restore_id
    current=restored['context'];assert current['restoreEpoch']!=before['restoreEpoch'] and current['namespace']==before['namespace']
    obj=r.object(current,'block',key);assert obj['id']==key and obj['content']=='Original pre-snapshot document A — 完整保留。\n'
    assert int(obj['revision'])>revision
    expected=r.all_objects(current)
    for request,origin,receipt in [(original['request'],original['origin'],original['receipt']),
                                   (r.initial['publication']['request'],r.initial['publication']['origin'],r.initial['builtinReceipt'])]:
        found=r.lookup(request,origin);assert found['lookup']['status']=='historical' and found['lookup']['result']==receipt
    found=r.lookup(restore_id,before);assert found['lookup']['status']=='historical' and found['lookup']['result']==restored
    r.stop();state=r.cold();fixed_facts(r,state);assert state['projection']['state']=='inactive' and state['projection']['current'] is None
    r.boot();r.dag();assert r.workspace()['context']==current and r.all_objects(current)==expected
    found=r.lookup(original['request'],original['origin']);assert found['lookup']['status']=='historical' and found['lookup']['result']==original['receipt']
    r.stop();state=r.cold();fixed_facts(r,state);assert state['projection']['state']=='inactive' and state['projection']['current'] is None
    r.report.update(formalKnowledgePreviewConfirmRestore=True,newRestoreScope=True,immutableOriginalReceiptsPreserved=True,noTutorialRepublicationAfterRestore=True)

def protected_purge(r):
    r.fresh();before={x['id']:x for x in r.rpc('crew.list',{'includeRemoved':True})}
    shared=before['pendingcrew-welcome-child'];ns=r.initial['context']['namespace']
    for key,(_,parents) in CREWS.items():
        if ROOT in parents:
            changed=r.rpc('crew.release',{'crewId':key,'fromParentId':ROOT,'toParentId':None})
            assert changed['id']==key and changed['number']==before[key]['number'] and changed['parentIds']==[]
    moved={x['id']:x for x in r.rpc('crew.list',{'includeRemoved':True})}
    assert moved[ROOT]['childIds']==[]
    assert moved[shared['id']]['number']==shared['number'] and moved[shared['id']]['parentIds']==shared['parentIds']
    r.rpc('crew.remove',{'crewId':ROOT});r.rpc('crew.purge',{'crewId':ROOT})
    assert not any(x['id']==ROOT for x in r.rpc('crew.list',{'includeRemoved':True}))
    r.stop();gone=r.cold();fixed_facts(r,gone)
    assert gone['projection']['state']=='cleared' and gone['projection']['current'] is None
    assert not os.path.lexists(r.data/'knowledge'/(ns+'.sqlite'))
    r.boot();after={x['id']:x for x in r.rpc('crew.list',{'includeRemoved':True})}
    assert set(after)==set(before)-{ROOT}
    assert after[shared['id']]['number']==shared['number'] and after[shared['id']]['parentIds']==shared['parentIds']
    for key,(_,parents) in CREWS.items():
        if ROOT in parents:assert after[key]['parentIds']==[] and after[key]['number']==before[key]['number']
    r.stop();gone=r.cold();fixed_facts(r,gone)
    assert gone['projection']['state']=='cleared' and gone['projection']['current'] is None
    assert not os.path.lexists(r.data/'knowledge'/(ns+'.sqlite'))
    r.report.update(formalProtectedReleaseRemovePurge=True,sharedDAGIdentityPreserved=True,purgedTutorialNotResurrected=True)

FLOWS=[('fresh-contract',fresh_contract),('legacy33-empty',lambda r:legacy(r,False)),
       ('legacy33-user-backup-restore',lambda r:legacy(r,True)),
       ('public-edit-and-Core-restore-refusal',lambda r:mutation(r,'edit')),
       ('public-unlink',lambda r:mutation(r,'unlink')),
       ('public-archive-view',lambda r:mutation(r,'archiveView')),
       ('public-archive-block',lambda r:mutation(r,'archiveBlock')),
       ('Knowledge-snapshot-restore',knowledge_restore),('protected-crew-purge',protected_purge)]
