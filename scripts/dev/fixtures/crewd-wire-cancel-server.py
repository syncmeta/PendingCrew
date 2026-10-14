import socket,json,sys,pathlib
root=pathlib.Path(sys.argv[1]); sock=socket.socket(socket.AF_UNIX); sock.bind(str(root/'s'));sock.listen(2)
(root/'ready').touch()
for generation in range(2):
 c,_=sock.accept(); file=c.makefile('rb'); held=None
 def send(obj):c.sendall((json.dumps(obj,ensure_ascii=False)+'\n').encode())
 for raw in file:
  q=json.loads(raw);m=q['method']; (root/'requests').open('a').write(m+'\n')
  if m=='system.hello':send({'id':q['id'],'result':{'server':'crewd','apiVersion':3,'minApiVersion':3,'version':'synthetic','capabilities':['session.respondRunnerRequest']}})
  elif m=='events.subscribe':send({'id':q['id'],'result':{}})
  elif m=='hold':held=q['id'];(root/'held').touch()
  elif m=='other':
   send({'event':'work.changed','data':{'marker':'unaffected'}});send({'id':q['id'],'result':'other-ok'});send({'id':held,'result':'late-cancelled'})
  elif m=='error':send({'id':q['id'],'error':{'code':-32009,'message':'CAS','data':{'currentWork':{'revision':'9007199254740993'}}}})
  elif m=='switch':
   send({'event':'switch.authority','data':{}});c.shutdown(socket.SHUT_WR);break
  elif m=='probe':send({'id':q['id'],'result':'fresh-ok'})
  else:raise RuntimeError(m)
 c.close()
sock.close()
