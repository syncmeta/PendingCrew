"""Full Linux ordinary-build qualification; import has no IO or product action."""
import hashlib,importlib.util,json,os,re,subprocess
from pathlib import Path
from common import sha

INPUT='cc063b67727aeb2869b60e95fbe9f615aeccbcccd24c4c79766057acc8946e55'
LEGACY='1edf6d47f64553773650389b40382a6181a1addb'
LEGACY_INPUT='0f1f1f96ce3a6704435217459b67c9f037c90d3a02edb88b6bb3153a4344dabc'


def load(name,path):
    spec=importlib.util.spec_from_file_location(name,path)
    value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value


def qualify(args,record):
    assert re.fullmatch('[0-9a-f]{40}',args.expected_ref) and args.expected_rust_input==INPUT
    git=lambda *v:subprocess.check_output(['git','--no-replace-objects','-C',str(args.source),*v])
    head=git('rev-parse','HEAD').decode().strip()
    status=git('status','--porcelain').decode()
    record('actual-source-before-qualification',{'source':str(args.source),'expectedRef':args.expected_ref,
        'actualHEAD':head,'statusPorcelain':status,'sourceClean':status==''})
    assert head==args.expected_ref and status==''
    p=load('original_normal_proof',args.source/'scripts/dev/write-codex-first-provenance.py')
    host=load('single_formal_host',args.source/'deploy/linux/pendingcrew-host.py');args.host_contract=host
    assert host.PUBLIC34['rustInputDigest']==INPUT
    inputs={};results={}
    for label,binary,witness,ref,digest,count in [
        ('current',args.core,args.witness,args.expected_ref,INPUT,680),
        ('legacy33',args.legacy_core,args.legacy_witness,LEGACY,LEGACY_INPUT,662)]:
        assert binary.is_absolute() and binary.is_file() and not binary.is_symlink() and os.access(binary,os.X_OK)
        raw=binary.read_bytes();assert raw[:6]==b'\x7fELF\x02\x01' and int.from_bytes(raw[18:20],'little')==62
        value=json.loads(witness.read_bytes())
        assert value['sourceRef']==ref and value['binarySHA256']==sha(binary) and len(value['files'])==count
        expected={'target':'x86_64-unknown-linux-gnu','profile':'release','features':[],
                  'defaultFeatures':True,'allFeatures':False}
        verified,evidence=p.verify_witness(witness,binary,digest,expected,require_fresh=True)
        b=verified['buildConfiguration']
        assert b['artifactFeatures']==[] and len(b['workspaceFeatures'])==12
        assert all(x==[] for x in b['workspaceFeatures'].values())
        assert b['artifactProfile']['test'] is False and b['artifactProfile']['opt_level']=='3'
        assert b['artifactProfile']['debug_assertions'] is False
        for key in ['RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CC','CFLAGS']:
            assert not b['compilerEnvironment'].get(key)
        paths=git('ls-tree','-r','--name-only',ref,'--','crewd').decode().splitlines()
        rows=[]
        for path in paths:
            body=git('show',ref+':'+path);h=hashlib.sha256(body).hexdigest()
            rows.append({'path':path,'sha256':h})
            if label=='current':
                actual=args.source/path;assert not actual.is_symlink() and actual.read_bytes()==body;inputs[str(actual)]=h
        assert sorted(rows,key=lambda x:x['path'])==verified['files'] and p.digest(rows)==digest
        # verify_witness consumes every frozen snapshot + original sidecar/log.
        inputs[str(binary)]=sha(binary);inputs[str(witness)]=sha(witness)
        for path in evidence.rglob('*'):
            if path.is_file():assert not path.is_symlink();inputs[str(path)]=sha(path)
        results[label]={'binary':str(binary),'sha256':sha(binary),'sourceRef':ref,'witness':verified}
    assert sha(args.core)==args.core_sha256
    for path in [*Path(__file__).parent.glob('*.py'),args.source/'deploy/linux/pendingcrew-host.py']:
        inputs[str(path)]=sha(path)
    import cold
    cold.SOURCE=args.source
    record('full-qualified-Linux-inputs',results)
    return inputs,results
