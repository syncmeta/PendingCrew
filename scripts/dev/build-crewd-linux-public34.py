#!/usr/bin/env python3
"""Explicit new public34 cohort. Prepare once; never publish/install a service.

Only a reviewed workflow_dispatch may execute this controller. Full historical
CI remains separate. The retained Core is an ordinary default Release ELF;
test and synthetic-helper artifacts never replace its original build witness.
"""
import hashlib,importlib.util,json,os,pathlib,platform,re,shutil,stat,subprocess,sys,traceback
sys.dont_write_bytecode=True
INPUT='cc063b67727aeb2869b60e95fbe9f615aeccbcccd24c4c79766057acc8946e55'
LEGACY='1edf6d47f64553773650389b40382a6181a1addb'
LEGACY_INPUT='0f1f1f96ce3a6704435217459b67c9f037c90d3a02edb88b6bb3153a4344dabc'


def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def save(path,value):
    with path.open('x') as f:json.dump(value,f,sort_keys=True,indent=2);f.write('\n');f.flush();os.fsync(f.fileno())
def git(root,*args):return subprocess.check_output(['git','--no-replace-objects','-C',str(root),*args])
def load(name,path):
    spec=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m


def inventory(root,ref):
    assert git(root,'rev-parse','HEAD').decode().strip()==ref and not git(root,'status','--porcelain')
    rows=[]
    for entry in git(root,'ls-tree','-rz','--full-tree',ref).split(b'\0'):
        if not entry:continue
        metadata,name=entry.split(b'\t',1);mode,kind,blob=metadata.decode().split();path=root/name.decode()
        assert kind=='blob' and mode in ['100644','100755','120000']
        info=path.lstat()
        if mode=='120000':assert stat.S_ISLNK(info.st_mode);actual=os.fsencode(os.readlink(path))
        else:
            assert stat.S_ISREG(info.st_mode) and bool(info.st_mode&0o100)==(mode=='100755')
            actual=path.read_bytes()
        assert actual==git(root,'cat-file','blob',blob)
        rows.append({'path':name.decode(),'sha256':hashlib.sha256(actual).hexdigest(),'gitMode':mode,'bytes':len(actual)})
    return rows


def rust_digest(rows):
    return hashlib.sha256(''.join(r['path']+'\t'+r['sha256']+'\n' for r in sorted(rows,key=lambda r:r['path']) if r['path'].startswith('crewd/')).encode()).hexdigest()


def source_checkpoint(workspace,out,bound,stage):
    sources={}
    for label,key in [('controller','controllerRef'),('source','sourceRef'),('legacy33','legacy33Ref')]:
        root=workspace/label
        head=git(root,'rev-parse','HEAD').decode().strip()
        status=git(root,'status','--porcelain').decode()
        sources[label]={'source':str(root),'expectedRef':bound[key],'actualHEAD':head,
            'statusPorcelain':status,'sourceClean':status=='','headMatchesExpected':head==bound[key]}
    # Persist unfiltered observations even when the original strict gate fails.
    save(out/('source-status-'+stage+'.json'),{'stage':stage,'sources':sources})
    assert all(x['sourceClean'] and x['headMatchesExpected'] for x in sources.values()),'actual source HEAD/status gate failed: '+stage
    return sources


def binding(workspace):
    refs=[os.environ[x] for x in ['REVIEWED_REF','SOURCE_REF','EXPECTED_RUST_TREE']]
    assert all(re.fullmatch('[0-9a-f]{40}',x) for x in refs)
    controller,source,tree=refs
    assert os.environ['GITHUB_ACTIONS']=='true' and os.environ['GITHUB_EVENT_NAME']=='workflow_dispatch'
    assert all(re.fullmatch('[1-9][0-9]*',os.environ[k]) for k in ['GITHUB_RUN_ID','GITHUB_RUN_ATTEMPT'])
    assert os.environ['GITHUB_SHA']==controller and os.environ['EXPECTED_RUST_INPUT']==INPUT
    assert platform.system()=='Linux' and platform.machine()=='x86_64' and os.getuid()==os.geteuid()!=0
    for key in ['RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CARGO_BUILD_TARGET','CREWD_GIT_SHA','CREWD_GIT_DIRTY','CC','CFLAGS']:
        assert not os.environ.get(key),'undeclared compiler override: '+key
    assert os.environ['CARGO_INCREMENTAL']=='0'
    result={'controllerRef':controller,'sourceRef':source,'rustTree':tree,'rustInputDigest':INPUT,
            'legacy33Ref':LEGACY,'runID':os.environ['GITHUB_RUN_ID'],'runAttempt':os.environ['GITHUB_RUN_ATTEMPT']}
    for label,ref,expected in [('controller',controller,INPUT),('source',source,INPUT),('legacy33',LEGACY,LEGACY_INPUT)]:
        root=workspace/label;rows=inventory(root,ref);assert rust_digest(rows)==expected
        result[label+'Files']=rows
        if label!='legacy33':assert git(root,'rev-parse','HEAD:crewd').decode().strip()==tree
    # All executable recipes must be the reviewed controller's exact files.
    controller_rows={r['path']:r for r in result['controllerFiles']}
    for row in result['sourceFiles']:
        if row['path'].startswith(('scripts/dev/','deploy/linux/','.github/workflows/')):
            assert row==controller_rows[row['path']],'reviewed source/controller delivery recipes differ'
    host=load('host_contract',workspace/'controller/deploy/linux/pendingcrew-host.py')
    host.qualify_public34_schema(json.loads((workspace/'source/crewd/schema/crewd-api.schema.json').read_bytes()))
    schema=load('package_contract',workspace/'controller/deploy/linux/package-host-release.py').source_schema(workspace/'controller/deploy/linux',source)
    # source_schema reads exact Git objects; both complete Rust trees already match.
    result['schemaEvidence']=schema
    for label in ['source','legacy33']:
        config=(workspace/label/'crewd/rust-toolchain.toml').read_bytes()
        assert config==(workspace/'source/crewd/rust-toolchain.toml').read_bytes(),'one explicit common locked toolchain required'
    return result


def command(argv,cwd,log,env=None):
    with log.open('x') as f:
        f.write('Argv: '+json.dumps(list(map(str,argv)))+'\n');f.flush()
        child=subprocess.Popen(list(map(str,argv)),cwd=cwd,env=env,stdin=subprocess.DEVNULL,stdout=f,stderr=subprocess.STDOUT)
        # No timeout/signal/retry: original child result must remain observable.
        code=child.wait();f.write('\nExit: '+str(code)+'\n');f.flush();os.fsync(f.fileno())
    assert code==0,'original command failed; retain evidence, never automatically repeat: '+str(log)


def build_one(workspace,out,label,reference,digest):
    source=workspace/label;target=out.parent/'linux-public34-targets'/label;normal=out/(label+'-normal')
    assert not target.exists(),'fresh isolated target directory required'
    recorder=workspace/'controller/scripts/dev/record-crewd-build.py'
    env=dict(os.environ,CARGO_TARGET_DIR=str(target))
    command([sys.executable,'-B','-I',recorder,'prepare',source,normal,reference],source,out/(label+'-prepare.log'),env)
    command([sys.executable,'-B','-I',recorder,'build',source,normal,target/'release/crewd',
        'cargo','build','--offline','--locked','--release','-p','crewd','--message-format=json'],source,out/(label+'-build.log'),env)
    witness=out/(label+'-normal-witness.json');binary=normal/'crewd-presign'
    proof=load('normal_proof',workspace/'controller/scripts/dev/write-codex-first-provenance.py')
    value,evidence=proof.verify_witness(witness,binary,digest,{'target':'x86_64-unknown-linux-gnu','profile':'release',
        'defaultFeatures':True,'allFeatures':False,'features':[]},require_fresh=True)
    assert value['sourceRef']==reference and len(value['buildConfiguration']['workspaceFeatures'])==12
    assert not any(value['buildConfiguration']['workspaceFeatures'].values()) and value['buildConfiguration']['artifactFeatures']==[]
    assert value['buildConfiguration']['artifactProfile']['test'] is False
    return binary,witness,target


def execute(workspace,out,bound):
    controller=workspace/'controller';source=workspace/'source';reference=bound['sourceRef']
    source_checkpoint(workspace,out,bound,'before-build')
    current,witness,target=build_one(workspace,out,'source',reference,INPUT)
    old,old_witness,_=build_one(workspace,out,'legacy33',LEGACY,LEGACY_INPUT)
    retained=sha(current);env=dict(os.environ,CARGO_TARGET_DIR=str(target))
    # No full CI duplicate: exactly the public34 Store open/migration and 34
    # onboarding contracts affected by this Linux cohort. Original ignores stay.
    selections=[('crewd-store','schema_v34_admin_secretary_tests::',9),
                ('crewd-store','spawn_refuses_unqualified_opened_catalog_instead_of_advertising_support',1),
                ('crewd-server','onboarding::',34)]
    for index,(package,selector,count) in enumerate(selections):
        log=out/f'affected-{index}.log'
        command(['cargo','test','--offline','--locked','--release','-p',package,'--lib',selector,'--','--nocapture'],source/'crewd',log,env)
        summaries=re.findall(r'^test result: ok\. ([0-9]+) passed; 0 failed; ([0-9]+) ignored;',log.read_text(),re.M)
        assert summaries==[(str(count),'0')],'actual affected selection/count/ignore differs'
    assert sha(current)==retained and sha(target/'release/crewd')==retained
    helper_log=out/'synthetic-helper-build.log'
    command(['cargo','build','--offline','--locked','--release','-p','crewd-exec','--bin','fake_codex','--bin','fake_claude','--message-format=json'],source/'crewd',helper_log,env)
    messages=[]
    for line in helper_log.read_text().splitlines():
        try:value=json.loads(line)
        except json.JSONDecodeError:continue
        if value.get('reason')=='compiler-artifact' and value.get('target',{}).get('name') in ['fake_codex','fake_claude']:
            assert value['profile']['test'] is False and value['features']==[];messages.append(value)
    assert {x['target']['name'] for x in messages}=={'fake_codex','fake_claude'}
    helpers=out/'synthetic-helpers';helpers.mkdir(mode=0o700)
    for name in ['fake_codex','fake_claude']:shutil.copy2(target/'release'/name,helpers/name)
    save(out/'helper-proof.json',{'sourceRef':reference,'rustInputDigest':INPUT,'normalCoreSHA256':retained,
        'features':[],'test':False,'files':{p.name:sha(p) for p in helpers.iterdir()},'actualCargoLogFile':helper_log.name,'actualCargoLogSHA256':sha(helper_log)})
    assert sha(target/'release/crewd')==retained
    # Preserve the CLI's original stdout/stderr bytes exactly once.
    actual=out/'actual-schema.json';errors=out/'actual-schema.stderr'
    with actual.open('xb') as stdout,errors.open('xb') as stderr:
        process=subprocess.Popen([str(current),'schema'],cwd=source,stdout=stdout,stderr=stderr);code=process.wait()
    assert code==0 and errors.read_bytes()==b''
    generated=json.loads(actual.read_bytes());assert generated==json.loads((source/'crewd/schema/crewd-api.schema.json').read_bytes())
    host=load('official_installer',controller/'deploy/linux/pendingcrew-host.py');host.qualify_public34_schema(generated)
    before_package=source_checkpoint(workspace,out,bound,'before-package')
    build_proof={'sourceCommit':reference,'sourceClean':before_package['source']['sourceClean'],'defaultFeatures':True,'features':[],'test':False,
        'binarySha256':retained,'schemaEvidence':bound['schemaEvidence'],'originalWitnessSHA256':sha(witness)}
    save(out/'build-proof.json',build_proof)
    base='https://github.com/syncmeta/PendingCrew/releases/download/host-'+reference
    command([sys.executable,'-B','-I',source/'deploy/linux/package-host-release.py','--core',current,'--proof',out/'build-proof.json',
        '--source',reference,'--architecture','x86_64','--artifact-url',base+'/host.tar.gz','--manifest-url',base+'/manifest.json',
        '--installer-url',base+'/install.py','--output',out/'install'],source,out/'package.log')
    source_checkpoint(workspace,out,bound,'after-package')
    assert binding(workspace)==bound
    command([sys.executable,'-B',controller/'scripts/dev/fixtures/linux_public34/run.py','--core',current,'--core-sha256',retained,
        '--witness',witness,'--source',source,'--expected-ref',reference,'--expected-rust-input',INPUT,
        '--legacy-core',old,'--legacy-witness',old_witness,'--fake-codex',helpers/'fake_codex','--helper-proof',out/'helper-proof.json',
        '--install-package',out/'install','--out',out/'runtime'],source,out/'runtime-driver.log')
    runtime=json.loads((out/'runtime/result.json').read_bytes());assert runtime['exit']==0 and len(runtime['scopes'])==11
    assert sha(current)==retained and sha(target/'release/crewd')==retained
    source_checkpoint(workspace,out,bound,'after-runtime')
    actual_after=binding(workspace);assert actual_after==bound
    save(out/'source-after.json',actual_after)
    save(out/'result.json',{'exit':0,'cohort':host.PUBLIC34['cohort'],'sourceRef':reference,'rustInputDigest':INPUT,
        'storeVersion':34,'methodCount':len(generated['methods']),'actualSchemaMatchesCommitted':True,'binarySHA256':retained,
        'ordinaryLinuxDefaultReleaseFresh12':True,'affectedScopesOnly':selections,'runtime':runtime,
        'originalNormalWitnessSHA256':sha(witness),'installedSystemd':False,'published':False,
        'packageFiles':{p.name:sha(p) for p in (out/'install').iterdir()}})


def main():
    assert not sys.flags.optimize,'qualification assertions must be active'
    workspace=pathlib.Path(os.environ['GITHUB_WORKSPACE']).resolve()
    out=pathlib.Path(os.environ['PUBLIC34_EVIDENCE']).resolve();mode=sys.argv[1]
    bound=binding(workspace)
    if mode=='bind':
        out.mkdir(mode=0o700);save(out/'source-before.json',bound)
        for label in ['controller','source','legacy33']:
            ref=bound[{'controller':'controllerRef','source':'sourceRef','legacy33':'legacy33Ref'}[label]]
            # Original complete Git source, including non-Rust recipe/component files.
            command(['git','--no-replace-objects','-C',workspace/label,'archive','--format=tar','-o',out/(label+'-full-source.tar'),ref],workspace,out/(label+'-archive.log'))
        return
    assert mode=='execute' and bound==json.loads((out/'source-before.json').read_bytes())
    execute(workspace,out,bound)


if __name__=='__main__':
    try:main()
    except BaseException as error:
        # No signal, rollback, re-submission or fresh UUID following a failure.
        out=pathlib.Path(os.environ['PUBLIC34_EVIDENCE'])
        if out.is_dir() and not (out/'controller-failure.json').exists():
            save(out/'controller-failure.json',{'exit':1,'error':repr(error),'traceback':traceback.format_exc(),
                'unqualified':True,'originalMutableRuntimeRetained':True,'automaticRetry':False})
        raise
