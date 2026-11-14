"""Private candidate activation, stopped-format refusal and exact receipt recovery.

This exercises the original managed candidate installer, not root/systemd install.
The separately prepared official entry remains uninstalled and unpublished.
"""
import contextlib,fcntl,hashlib,json,os,shutil,sqlite3,stat,uuid
from pathlib import Path
from common import sha
from scenarios import tree_bytes


def candidate(r,label):
    q=r.qualified[label];w=q['witness'];package=r.out/(label+'-candidate');(package/'bin').mkdir(parents=True)
    shutil.copy2(q['binary'],package/'bin/crewd')
    witness=r.a.witness if label=='current' else r.a.legacy_witness
    shutil.copy2(witness,package/'normal-build-witness.json')
    shutil.copytree(witness.parent/w['evidenceDirectory'],package/w['evidenceDirectory'])
    value={'format':1,'sourceProofTier':'fresh-owning-workspace-production-graph','sourceRef':q['sourceRef'],
        'platform':'linux-x86_64','binary':'bin/crewd','binarySHA256':sha(package/'bin/crewd'),
        'witness':'normal-build-witness.json','witnessSHA256':sha(package/'normal-build-witness.json'),
        'distribution':'development-candidate','installed':False,'published':False}
    (package/'candidate.json').write_text(json.dumps(value,indent=2)+'\n');return package


def invalid_catalog_controls(r):
    # All inputs are copies of the stopped normal product's private DB. Only
    # negatives use direct SQL. No positive business fixture is synthesized.
    host=r.a.host_contract;source=tree_bytes(r.data);results=[]
    cases={
        'public-format': "UPDATE meta SET value='private-query34' WHERE key='adminSecretary.publicFormat.v34'",
        'public-identity': "UPDATE meta SET value='foreign' WHERE key='adminSecretary.publicDatabase.v34'",
        'H33-format': "UPDATE meta SET value='private-query33' WHERE key='adminAskGroups.schemaFormat.v1'",
        'H33-identity': "UPDATE meta SET value='foreign' WHERE key='adminAskGroups.schemaDatabase.v1'",
        'catalog-missing': 'DROP TABLE admin_secretary_publications',
        'catalog-extra': "CREATE TRIGGER unsupported_secretary_trigger AFTER INSERT ON admin_secretary_observers BEGIN SELECT 1; END",
        'private-meta': "INSERT INTO meta(key,value) VALUES('managedViews.unpublished','private')",
        'private-table': 'CREATE TABLE work_view_unpublished(id TEXT)',
        'future': 'PRAGMA user_version=35',
        'partial-pre34': 'PRAGMA user_version=33',
    }
    for label,damage in cases.items():
        target=r.runtime/('negative-'+label);target.mkdir(mode=0o700)
        shutil.copy2(r.data/'crewd.db',target/'crewd.db')
        original=(r.data/'crewd.db.id').read_text().splitlines()[0]
        (target/'crewd.db.id').write_text(original+'\nino='+str((target/'crewd.db').stat().st_ino)+'\n')
        (target/'crewd.db.id').chmod(0o600)
        lease=host.data_lease(target,os.getuid(),os.getgid())
        try:
            control=host.qualify_stopped_public34(target);assert control['public34Qualified']
            with contextlib.closing(sqlite3.connect(target/'crewd.db')) as c:
                with c:c.execute(damage)
            before=tree_bytes(target)
            try:host.qualify_stopped_public34(target)
            except host.Refused as error:message=str(error)
            else:raise AssertionError('invalid actual stopped catalog accepted: '+label)
            assert 'pending database recovery' not in message
            assert tree_bytes(target)==before and tree_bytes(r.data)==source
            results.append({'case':label,'originalDBUnchanged':True,'negativeCopyUnchangedByQualifier':True,'refusal':message})
        finally:os.close(lease)
    r.record('real-stopped-copy-format-catalog-authority-negatives',results)


def installation_flow(r):
    host=r.a.host_contract;official=r.a.install_package
    manifest=json.loads((official/'manifest.json').read_bytes());payload=(official/'host.tar.gz').read_bytes()
    assert manifest['format']==2 and manifest['cohort']==host.PUBLIC34['cohort']
    assert sha(official/'host.tar.gz')==manifest['artifact']['sha256']
    assert manifest['artifact']['sourceCommit']==r.a.expected_ref
    unpack=r.out/'official-unpack';unpack.mkdir(mode=0o700)
    directory,release=host.extract_release(payload,unpack,manifest['artifact'])
    assert host.release_generation(release)==34 and sha(directory/'bin/crewd')==sha(r.a.core)
    # The standalone official installer embeds the exact immutable manifest digest.
    import ast
    nodes=ast.parse((official/'install.py').read_bytes()).body
    values={n.targets[0].id:ast.literal_eval(n.value) for n in nodes if isinstance(n,ast.Assign)
        and len(n.targets)==1 and isinstance(n.targets[0],ast.Name)
        and n.targets[0].id in ['DEFAULT_MANIFEST_SHA256','DEFAULT_MANIFEST_URL']}
    assert values['DEFAULT_MANIFEST_SHA256']==sha(official/'manifest.json')
    assert host.immutable_release_url(values['DEFAULT_MANIFEST_URL'],r.a.expected_ref)
    corrupt=bytearray(payload);corrupt[len(corrupt)//2]^=1
    assert hashlib.sha256(corrupt).hexdigest()!=manifest['artifact']['sha256']
    r.record('official-version-package-qualified-not-installed',{'release':release,
        'manifestSHA256':sha(official/'manifest.json'),'installerSHA256':sha(official/'install.py'),
        'rootSystemdInstalled':False,'published':False})
    old=candidate(r,'legacy33');new=candidate(r,'current');prefix=r.runtime/'prefix'
    installer=r.a.source/'scripts/dev/install-crewd-linux-candidate.py'
    def install(action,rid,expected,package=None,reference=None,success=True,contains=None):
        argv=[str(installer),action,'--prefix',str(prefix),'--data-dir',str(r.data),
              '--request-id',rid,'--expected-current',expected or 'none']
        if package:argv+=['--package',str(package)]
        if reference:argv+=['--source-ref',reference]
        return r.tool([__import__('sys').executable,*argv],action+'-'+rid,expected=0 if success else None,contains=contains)
    def status(rid):
        value=r.tool([__import__('sys').executable,str(installer),'status','--prefix',str(prefix),'--request-id',rid],'status-'+rid)
        return json.loads(value['stdout'])
    first=str(uuid.uuid4());install('install',first,None,package=old);active=status(first)['currentActivation']
    assert status(first)['outcome']=='committed-current'
    r.launcher=prefix/'bin/crewd'
    r.qualified['legacy33']=dict(r.qualified['legacy33'],binary=str(prefix/'releases'/r.qualified['legacy33']['sourceRef']/'bin/crewd'))
    r.boot(legacy=True)
    intent,receipt=r.command('crew.create',{'title':'Original managed genuine33 user','workingDirectory':str(r.runtime/'work')})
    crew=receipt['result'];old_db=r.human.identity['dbId'];r.stop()
    old_bytes=tree_bytes(r.data)
    second=str(uuid.uuid4());install('install',second,active,package=new);active=status(second)['currentActivation']
    assert tree_bytes(r.data)==old_bytes,'installation must not migrate or reinterpret the data'
    r.qualified['current']=dict(r.qualified['current'],binary=str(prefix/'releases'/r.a.expected_ref/'bin/crewd'))
    r.boot();assert r.human.identity['dbId']==old_db
    crews=r.rpc('crew.list',{'includeRemoved':True});assert any(c['id']==crew['id'] and c['title']==crew['title'] for c in crews)
    assert not any(c['id']=='pendingcrew-welcome' for c in crews)
    looked=r.rpc('command.lookup',{'requestId':intent['requestId'],'dbId':intent['dbId']})
    assert looked==dict(receipt,historical=True)
    # Both locks are held by the actual installed Core process after exec.
    for lock in [prefix/'.runtime.lock',prefix/'.data-leases'/hashlib.sha256(str(r.data).encode()).hexdigest()]:
        info=lock.stat();fd=os.open(lock,os.O_RDWR|os.O_NOFOLLOW)
        try:
            try:fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
            except BlockingIOError:pass
            else:raise AssertionError('original installed runtime lease missing')
        finally:os.close(fd)
        assert any(x.stat().st_dev==info.st_dev and x.stat().st_ino==info.st_ino for x in Path('/proc',str(r.proc.pid),'fd').iterdir())
    blocked=str(uuid.uuid4());install('install',blocked,active,package=new,success=False,contains='Errno 11')
    assert status(second)['currentActivation']==active and status(blocked)['outcome']=='absent'
    r.stop();before=tree_bytes(r.data);invalid_catalog_controls(r)
    downgrade=str(uuid.uuid4());install('rollback',downgrade,active,reference=r.qualified['legacy33']['sourceRef'],success=False,contains='managed.future_schema')
    assert status(downgrade)['outcome']=='absent' and status(second)['currentActivation']==active
    assert tree_bytes(r.data)==before
    stale=str(uuid.uuid4());install('install',stale,'activations/'+str(uuid.uuid4()),package=new,success=False,contains='activation CAS conflict')
    assert status(stale)['outcome']=='absent'
    # A separately planned positive same-version rollback comes BEFORE the
    # Unknown negative. Nothing activates or retries after that later Unknown.
    same_version=str(uuid.uuid4());install('rollback',same_version,active,reference=r.a.expected_ref)
    active=status(same_version)['currentActivation'];assert status(same_version)['outcome']=='committed-current'
    assert tree_bytes(r.data)==before
    # Genuine preparation failure AFTER immutable transaction persistence.
    unknown=str(uuid.uuid4());(prefix/('.current-'+unknown)).mkdir(mode=0o700)
    install('install',unknown,active,package=new,success=False,contains='File exists')
    found=status(unknown);assert found['outcome']=='prepared-unknown' and found['currentActivation']==active
    original=(prefix/'activations'/unknown/'transaction.json').read_bytes()
    assert tree_bytes(r.data)==before
    assert status(unknown)['outcome']=='prepared-unknown'
    r.boot();assert r.human.identity['dbId']==old_db
    assert r.rpc('command.lookup',{'requestId':intent['requestId'],'dbId':intent['dbId']})==looked
    r.stop()
    assert status(unknown)['outcome']=='prepared-unknown'
    assert (prefix/'activations'/unknown/'transaction.json').read_bytes()==original
    r.report.update(privateManagedInstall33To34=True,actualInstalledLeases=True,
        incompatibleDatabaseRollbackRefused=True,sameVersionCodeRollbackQualified=True,
        preparedUnknown=unknown,unknownUUIDStatusOnly=True,originalCrewRequestLookupOnly=True,
        formalPackageDigestQualified=True,rootSystemdInstalled=False)
