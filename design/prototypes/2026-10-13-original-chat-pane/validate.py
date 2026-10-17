from sketch_helpers import *
import hashlib
def validate(v,s,path='root'):
 if '$ref' in s:return validate(v,D[s['$ref'].split('/')[-1]],path)
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
  if s.get('format')=='uuid':uuid.UUID(v)
 elif t=='boolean':assert isinstance(v,bool),(path,t)
 elif t in ('number','integer'):
  assert isinstance(v,(int,float)) and not isinstance(v,bool),(path,t)
  assert v>=s.get('minimum',float('-inf')) and v<=s.get('maximum',float('inf')),(path,'bounds',v)
z=zipfile.ZipFile(P/'PendingCrew-OriginalChatPane.sketch')
doc=json.loads(z.read('document.json'));page=json.loads(z.read(doc['pages'][0]['_ref']+'.json'));doc['pages']=[page]
validate(doc,schema['properties']['document']);validate(json.loads(z.read('meta.json')),D['Meta']);validate(json.loads(z.read('user.json')),D['User'])
boards=page['layers'];layers=[l for b in boards for l in b['layers']];ids=[l['do_objectID'] for l in boards+layers];targets={b['do_objectID'] for b in boards};flows=[l['flow']['destinationArtboardID'] for l in layers if 'flow' in l]
assert len(set(ids))==len(ids);assert all(t in targets for t in flows);assert sum(b['isFlowHome'] for b in boards)==1
result={'status':'PASS','artboards':len(boards),'nativeFlowLinks':len(flows),'uniqueLayerIDs':len(ids),'startPoints':1,'sha256':hashlib.sha256((P/'PendingCrew-OriginalChatPane.sketch').read_bytes()).hexdigest()}
(P/'native-integrity.json').write_text(json.dumps(result,indent=2));print(result)
