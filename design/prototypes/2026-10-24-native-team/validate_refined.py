from refined_helpers import *
import sys, re
filename=sys.argv[1] if len(sys.argv)>1 else "PendingCrew-SwiftUI-Refined.sketch"
import hashlib
def validate(v,s,path='root'):
 if '$ref' in s:
  if s['$ref'].split('/')[-1] == 'Uuid':
   try:uuid.UUID(v)
   except (ValueError,TypeError,AttributeError):raise AssertionError((path,'uuid',v))
  return validate(v,D[s['$ref'].split('/')[-1]],path)
 if 'const' in s:assert v==s['const'],(path,'const')
 if 'enum' in s:assert v in s['enum'],(path,'enum',v)
 if 'oneOf' in s:
  good=0
  for o in s['oneOf']:
   try:validate(v,o,path);good+=1
   except (AssertionError,KeyError,TypeError):pass
  assert good==1,(path,'oneOf',good);return
 t=s.get('type')
 if t=='object':
  assert isinstance(v,dict),(path,t)
  for k in s.get('required',[]):assert k in v,(path,'missing',k)
  for k,val in v.items():
   if k in s.get('properties',{}):validate(val,s['properties'][k],path+'.'+k)
   elif s.get('additionalProperties') is False and not s.get('patternProperties'):raise AssertionError((path,'extra',k))
   elif s.get('patternProperties'):
    for sub in s['patternProperties'].values():validate(val,sub,path+'.'+k)
 elif t=='array':
  assert isinstance(v,list),(path,t)
  for i,a in enumerate(v):validate(a,s['items'],path+f'[{i}]')
 elif t=='string':
  assert isinstance(v,str),(path,t)
  if 'pattern' in s:assert re.search(s['pattern'],v),(path,'pattern',v)
  if s.get('format')=='uuid':uuid.UUID(v)
 elif t=='boolean':assert isinstance(v,bool),(path,t)
 elif t in ('number','integer'):
  assert isinstance(v,(int,float)) and not isinstance(v,bool),(path,t)
  assert v>=s.get('minimum',float('-inf')) and v<=s.get('maximum',float('inf')),(path,'bounds',v)
z=zipfile.ZipFile(P/filename)
doc=json.loads(z.read('document.json'));pages=[json.loads(z.read(p['_ref']+'.json')) for p in doc['pages']];page=pages[0];doc['pages']=pages
validate(doc,schema['properties']['document']);validate(json.loads(z.read('meta.json')),D['Meta']);validate(json.loads(z.read('user.json')),D['User'])
boards=page['layers']
def walk(layer):
 yield layer
 for child in layer.get('layers',[]):yield from walk(child)
all_layers=[layer for p in pages for layer in walk(p)]
layers=[l for l in all_layers if l['_class'] not in ['page','artboard','symbolMaster']]
ids=[l['do_objectID'] for l in all_layers];targets={b['do_objectID'] for b in boards};flows=[l['flow']['destinationArtboardID'] for l in all_layers if 'flow' in l]
assert len(set(ids))==len(ids);assert all(t in targets or t == 'back' for t in flows);assert sum(b['isFlowHome'] for b in boards)==1
reached={b['do_objectID'] for b in boards if b['isFlowHome']}
while True:
 expanded=reached|{l['flow']['destinationArtboardID'] for b in boards if b['do_objectID'] in reached for l in walk(b) if 'flow' in l and l['flow']['destinationArtboardID']!='back'}
 if expanded==reached:break
 reached=expanded
assert reached==targets,('unreachable',targets-reached)
sources={l['symbolID'] for l in all_layers if l['_class']=='symbolMaster'}
assert all(l['symbolID'] in sources for l in layers if l['_class']=='symbolInstance')
result={'status':'PASS','artboards':len(boards),'nativeSymbols':len(pages[1]['layers']),'nativeSymbolInstances':sum(l['_class']=='symbolInstance' for l in layers),'nativeFlowLinks':len(flows),'uniqueLayerIDs':len(ids),'startPoints':1,'reachableArtboards':len(reached),'backLinks':flows.count('back'),'sha256':hashlib.sha256((P/filename).read_bytes()).hexdigest()}
(P/('refined-proof-integrity.json' if 'Four-Proofs' in filename else 'sidebar-native-integrity.json' if 'Sidebar-Navigation' in filename else 'refined-native-integrity.json')).write_text(json.dumps(result,indent=2));print(result)
