import socket,json,sys,pathlib,threading,time
root=pathlib.Path(sys.argv[1]);a='11111111-1111-4111-8111-111111111111';b='22222222-2222-4222-8222-222222222222';c='33333333-3333-4333-8333-333333333333'
old='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';fresh='cccccccc-cccc-4ccc-8ccc-cccccccccccc';bid='bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';started='dddddddd-dddd-4ddd-8ddd-dddddddddddd'
sock=socket.socket(socket.AF_UNIX);sock.bind(str(root/'s'));sock.listen(1);(root/'ready').touch();conn,_=sock.accept();lock=threading.Lock();created=[]
def send(value):
 with lock:conn.sendall((json.dumps(value,ensure_ascii=False)+'\n').encode())
def held(name,q,result):
 (root/('held-'+name)).write_text(json.dumps(q,ensure_ascii=False))
 deadline=time.monotonic()+10
 while not (root/('release-'+name)).exists():
  if time.monotonic()>deadline:raise RuntimeError('controlled response gate expired '+name)
  time.sleep(.01)
 send({'id':q['id'],'result':result})
def member(crew,id):return {'id':id,'crewId':crew,'role':'worker','name':id,'executor':'codex'}
with conn,conn.makefile('rb') as file:
 for raw in file:
  q=json.loads(raw);m=q['method'];p=q.get('params') or {}
  with (root/'requests.jsonl').open('a') as out:out.write(json.dumps(q,ensure_ascii=False)+'\n')
  if m=='system.hello':v={'server':'crewd','apiVersion':3,'minApiVersion':3,'version':'synthetic-mobile-flow','capabilities':['session.respondRunnerRequest']}
  elif m=='events.subscribe':v={}
  elif m=='crew.list':
   v=[{'id':a,'title':'fresh A' if (root/'fresh-crews').exists() else 'A','workingDirectory':'/remote/A'},{'id':b,'title':'B','workingDirectory':'/remote/B'}]+created
   if (root/'hold-crews').exists() and not (root/'held-crews').exists():
    (root/'held-crews').touch();threading.Thread(target=held,args=('crews',q,v),daemon=True).start();continue
  elif m=='session.list':
   crew=p['crewId'];v=[member(crew,(fresh if (root/'fresh-directory').exists() else old) if crew==a else bid)] if crew in (a,b) else []
   if crew==a and (root/'hold-directory').exists() and not (root/'held-directory').exists():
    (root/'held-directory').touch();threading.Thread(target=held,args=('directory',q,v),daemon=True).start();continue
  elif m=='channel.list':v=[{'id':'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee','crewId':a,'kind':'topic','name':'Synthetic message channel','members':[]}] if (root/'qa-mode').read_text().strip()=='unknown-message' else []
  elif m=='message.post':
   assert p['text']=='original message text before actual EOF',q
   (root/'received-message.json').write_text(json.dumps(q));conn.shutdown(socket.SHUT_RDWR);break
  elif m in ('message.list','ask.list','todo.list'):v=[]
  elif m=='work.list':v={'items':[]}
  elif m=='restart.pending':v=None
  elif m=='session.inspect':v={'id':p['sessionId'],'screen':'synthetic','runnerItems':[]}
  elif m=='session.history':v={'items':[]}
  elif m=='session.start':
   if p['brief']=='reject':send({'id':q['id'],'error':{'code':-32009,'message':'state conflict','data':{'refusal':'session.state_conflict'}}});continue
   if p['brief']=='bad-start':v={}
   else:
    assert p['crewId']==a and p['executor']=='codex' and p['isolation'] is True,q
    threading.Thread(target=held,args=('start',q,{'id':started}),daemon=True).start();continue
  elif m=='crew.create':
   if p['title']=='bad-receipt':v={}
   else:
    assert p=={'title':'held-create','workingDirectory':'/remote/A','parentId':a},q
    created.append({'id':c,'title':p['title'],'workingDirectory':p['workingDirectory']});threading.Thread(target=held,args=('create',q,{'id':c}),daemon=True).start();continue
  else:raise RuntimeError('unexpected method '+m)
  send({'id':q['id'],'result':v})
sock.close()
