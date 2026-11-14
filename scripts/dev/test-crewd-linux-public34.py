#!/usr/bin/env python3
"""Pure source/cohort controls, no product, SQL, service, provider or network.

Extract only literal constants and pure qualification functions from the actual
formal installer AST. This cannot run installer imports/main or its IO helpers.
Runtime/catalog/physical-process tests are the separate explicit Linux flow.
"""
import ast,copy,hashlib,json,pathlib,re,subprocess,sys


def source_clean_recipe_controls(root):
    builder=ast.parse((root/'scripts/dev/build-crewd-linux-public34.py').read_bytes())
    isolated=[]
    for node in ast.walk(builder):
        if not (isinstance(node,ast.Call) and isinstance(node.func,ast.Name) and node.func.id=='command'
                and node.args and isinstance(node.args[0],ast.List)):continue
        argv=node.args[0].elts
        if not (argv and isinstance(argv[0],ast.Attribute) and isinstance(argv[0].value,ast.Name)
                and argv[0].value.id=='sys' and argv[0].attr=='executable'):continue
        if any(isinstance(x,ast.Constant) and x.value=='-I' for x in argv):
            isolated.append(argv)
    # -I ignores PYTHONDONTWRITEBYTECODE; the interpreter itself must get -B.
    assert len(isolated)==3
    assert all(isinstance(a[1],ast.Constant) and a[1].value=='-B'
        and isinstance(a[2],ast.Constant) and a[2].value=='-I' for a in isolated)
    package=ast.parse((root/'deploy/linux/package-host-release.py').read_bytes())
    assert isinstance(package.body[1],ast.Import) and [n.name for n in package.body[1].names]==['sys']
    assert ast.dump(package.body[2],include_attributes=False)==ast.dump(
        ast.parse('sys.dont_write_bytecode = True').body[0],include_attributes=False)


def main():
    assert not sys.flags.optimize
    root=pathlib.Path(__file__).resolve().parents[2]
    source_clean_recipe_controls(root)
    source=root/'deploy/linux/pendingcrew-host.py';tree=ast.parse(source.read_bytes())
    selected=['PUBLIC34','PUBLIC34_H33_DDL','PUBLIC34_SECRETARY_DDL']
    pure=['release_generation','qualify_public34_release','qualify_public34_schema','qualify_release_manifest']
    values={}
    for node in tree.body:
        if isinstance(node,ast.Assign) and len(node.targets)==1 and isinstance(node.targets[0],ast.Name) and node.targets[0].id in selected:
            values[node.targets[0].id]=ast.literal_eval(node.value)
    assert set(values)==set(selected)
    class Refused(Exception):pass
    scope={'Refused':Refused,'re':re,'json':json,'sha':lambda data:hashlib.sha256(data).hexdigest(),**values}
    nodes=[node for node in tree.body if isinstance(node,ast.FunctionDef) and node.name in pure]
    assert len(nodes)==len(pure)
    exec(compile(ast.Module(body=nodes,type_ignores=[]),str(source),'exec'),scope)
    current=json.loads((root/'crewd/schema/crewd-api.schema.json').read_bytes())
    scope['qualify_public34_schema'](current)
    # Baseline is read from its original committed Git object, not regenerated.
    baseline=json.loads(subprocess.check_output(['git','--no-replace-objects','-C',str(root),'show',
        '1edf6d47f64553773650389b40382a6181a1addb:crewd/schema/crewd-api.schema.json']))
    before=set(baseline['methods']);after=set(current['methods'])
    assert len(before)==235 and len(after)==236 and after-before=={'adminCase.secretaryBrief'}
    changed=copy.deepcopy(current);changed['methods']['host.quota']['result']={'type':'null'}
    rejected=[]
    def refuses(label,call):
        try:call()
        except Refused:rejected.append(label)
        else:raise AssertionError('unsafe altered contract accepted: '+label)
    refuses('same-count-altered-DTO',lambda:scope['qualify_public34_schema'](changed))
    source_ref='a'*40;c=values['PUBLIC34']
    evidence={k:c[k] for k in ['schemaSourceSHA256','h33SourceSHA256','secretarySourceSHA256','apiSchemaSHA256','rustInputDigest']}
    evidence.update(sourceCommit=source_ref,storeVersion=34)
    valid={'format':3,'sourceCommit':source_ref,'architecture':'x86_64','storeVersion':34,'apiVersion':3,'cohort':c['cohort'],'schemaEvidence':evidence}
    assert scope['release_generation'](valid)==34
    for key,new in [('format',2),('storeVersion',33),('storeVersion',True),('cohort','private34'),('apiVersion',4),('architecture','aarch64')]:
        bad=copy.deepcopy(valid);bad[key]=new
        refuses('altered-'+key+'-'+str(new),lambda bad=bad:scope['release_generation'](bad))
    for key in evidence:
        bad=copy.deepcopy(valid);bad['schemaEvidence'].pop(key)
        refuses('missing-'+key,lambda bad=bad:scope['release_generation'](bad))
        bad=copy.deepcopy(valid);bad['schemaEvidence'][key]='f'*64
        refuses('forged-'+key,lambda bad=bad:scope['release_generation'](bad))
    bad=copy.deepcopy(valid);bad['schemaEvidence']['privateQueryFormat']='query34'
    refuses('extra-private-evidence',lambda:scope['release_generation'](bad))
    # Keep literal legacy formats and their historical acceptance exact.
    assert scope['release_generation']({'format':1,'storeVersion':21})==21
    old={'format':2,'storeVersion':33,'sourceCommit':source_ref,
         'schemaEvidence':{'sourceCommit':source_ref,'storeVersion':33,'schemaSourceSHA256':'b'*64}}
    assert scope['release_generation'](old)==33
    for historical in [{'format':1,'storeVersion':21},old]:
        relabeled=dict(historical,cohort=c['cohort'])
        refuses('historical-'+str(historical['storeVersion'])+'-relabel',lambda relabeled=relabeled:scope['release_generation'](relabeled))
    scope['qualify_release_manifest']({'format':1,'artifact':{}})
    scope['qualify_release_manifest']({'format':2,'cohort':c['cohort'],'artifact':{'rustInputDigest':c['rustInputDigest']}})
    for label,manifest in [('legacy-manifest-relabel',{'format':1,'cohort':c['cohort'],'artifact':{}}),
        ('wrong-new-input',{'format':2,'cohort':c['cohort'],'artifact':{'rustInputDigest':'f'*64}}),
        ('new-missing-cohort',{'format':2,'artifact':{'rustInputDigest':c['rustInputDigest']}})]:
        refuses(label,lambda manifest=manifest:scope['qualify_release_manifest'](manifest))
    for name,key,ddl in [('admin_ask_groups','h33SourceSHA256',values['PUBLIC34_H33_DDL']),
                         ('admin_secretary','secretarySourceSHA256',values['PUBLIC34_SECRETARY_DDL'])]:
        body=(root/f'crewd/crates/store/src/{name}.rs').read_bytes()
        assert hashlib.sha256(body).hexdigest()==c[key]
        matches=re.findall(rb'pub const DDL: &str = r#"(.*?)"#;',body,re.S)
        assert len(matches)==1 and matches[0].decode()==ddl
    print(json.dumps({'pureControlsPassed':True,'negativeControls':rejected,'public34Methods':236,
        'completeSchemaCanonicalSHA256':c['apiSchemaSHA256'],'old21And33BranchesPreserved':True,
        'actualSQLOrCoreOrLinuxOrInstallExecuted':False},indent=2))


if __name__=='__main__':main()
