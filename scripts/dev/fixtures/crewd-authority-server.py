import socket,json,sys,pathlib,threading
root=pathlib.Path(sys.argv[1]);label=sys.argv[2];sock=socket.socket(socket.AF_UNIX);sock.bind(str(root/'s'));sock.listen(8);(root/'ready').touch()
def serve(c):
 with c,c.makefile('rb') as file:
  for raw in file:
   q=json.loads(raw);m=q['method'];(root/'requests').open('a').write(m+'\n')
   h={'ask':'H1','crewId':'crew','question':label+' question','state':'open'}
   w={'id':'W1','crewId':'crew','title':label+' plan','revision':'1','status':'open','sources':['#1'],'frozen':False}
   t={'requirementId':'#1','crewId':'crew','text':label+' requirement','status':'unclaimed','closure':None}
   if m=='system.hello':v={'server':'crewd','apiVersion':3,'minApiVersion':3,'version':label,'capabilities':['session.respondRunnerRequest','channelWork']}
   elif m=='events.subscribe':v={}
   elif m=='crew.list':v=[]
   elif m=='ask.list':v=[{'ask':h}]
   elif m=='restart.pending':v=None
   elif m=='work.list':v={'items':[w]}
   elif m=='work.get':v=w
   elif m=='todo.list':v=[t]
   elif m=='todo.get':v=t
   elif m=='ask.get':v=h
   else:raise RuntimeError('unexpected write/method '+m)
   c.sendall((json.dumps({'id':q['id'],'result':v})+'\n').encode())
while True:
 c,_=sock.accept();threading.Thread(target=serve,args=(c,),daemon=True).start()
