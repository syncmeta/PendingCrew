import pathlib,shutil,subprocess,os,json,hashlib,time
import argparse
p=argparse.ArgumentParser();p.add_argument('--binary',required=True);p.add_argument('--witness',required=True);p.add_argument('--expected-source-ref',required=True);p.add_argument('--source-data',required=True);p.add_argument('--out',required=True);a=p.parse_args()
witness=json.loads(pathlib.Path(a.witness).read_text());binary=pathlib.Path(a.binary)
assert witness['sourceRef']==a.expected_source_ref and witness.get('freshWorkspaceArtifacts') and hashlib.sha256(binary.read_bytes()).hexdigest()==witness['binarySHA256']
out=pathlib.Path(a.out);assert out.is_absolute() and not out.exists();out.mkdir(mode=0o700)
original=pathlib.Path(a.source_data);assert original.is_absolute() and not original.is_symlink()
sourceResult=json.loads((original.parent/'result.json').read_text());assert sourceResult['status']=='PASS' and sourceResult['sourceRef']==a.expected_source_ref and sourceResult['daemonTerminal']==[-9,0,0]
assert not (original/'crewd.sock').exists(), 'source daemon must be physically stopped'
results=[]
def snapshot(d):return {str(p.relative_to(d)):{'sha':hashlib.sha256(p.read_bytes()).hexdigest(),'mode':p.stat().st_mode&0o777} for p in d.rglob('*') if p.is_file() and p.name!='crewd.lock'}
for case in ['bad-wal','corrupt-btree','id-mismatch','bad-registry']:
 d=out/case;shutil.copytree(original,d);os.chmod(d,0o700);reg=d/'exec-registry.json';reg.write_text(json.dumps({'gone':{'pid':2147483647,'pgid':2147483647,'startTime':1,'bootId':'unproven','command':'synthetic-gone-obligation'}}));os.chmod(reg,0o600)
 if case=='bad-wal':(d/'crewd.db-wal').write_bytes(bytes(64))
 elif case=='corrupt-btree':
  db=d/'crewd.db';raw=bytearray(db.read_bytes());raw[100]=255;db.write_bytes(raw)
 elif case=='id-mismatch':(d/'crewd.db.id').write_text('foreign-identity\n')
 elif case=='bad-registry':reg.write_bytes(b'{invalid-registry')
 before=snapshot(d)
 for name,args in [('inspect',['recovery','inspect','--json']),('reconcile',['recovery','reconcile','--json']),('serve',['serve','--codex','/usr/bin/false','--claude','/usr/bin/false']),('codex',['codex','--workdir',str(d),'--codex','/usr/bin/false']),('claude',['claude','--workdir',str(d),'--claude','/usr/bin/false'])]:
  env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(out),'XDG_CACHE_HOME':str(out),'CLAUDE_CONFIG_DIR':str(out),'TMPDIR':str(out)}
  start=time.monotonic();p=subprocess.run([str(binary),'--data-dir',str(d),*args],env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=8)
  (out/(case+'-'+name+'.out')).write_bytes(p.stdout);(out/(case+'-'+name+'.err')).write_bytes(p.stderr)
  expected=0 if name=='inspect' and case!='bad-registry' else 1
  assert (p.returncode==0)==(expected==0),(case,name,p.stderr)
  assert snapshot(d)==before,(case,name,'original files mutated')
  assert not (d/'crewd.sock').exists()
  results.append({'case':case,'entry':name,'exit':p.returncode,'physicalFilesUnchanged':True,'elapsed':time.monotonic()-start})
(out/'result.json').write_text(json.dumps({'status':'PASS','binarySHA':hashlib.sha256(binary.read_bytes()).hexdigest(),'fixtureSHA':hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),'originalSource':str(original),'sourceRef':witness['sourceRef'],'originalWitness':a.witness,'noSignalsToRuntime':True,'noProviderExecuted':True,'cases':results},indent=2)+'\n');print('PASS twenty closed-entry guards',flush=True)
