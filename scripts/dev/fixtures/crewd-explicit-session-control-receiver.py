import pathlib,tempfile,socket,threading,json,subprocess,time
base=pathlib.Path(tempfile.mkdtemp(prefix='pcw-explicit-control-',dir='/private/tmp'));print(base,flush=True);results=[]
for mode in ['resume','interrupt','stop','typed','mixed','missing','foreign','failure','late','stopping']:
 root=base/mode;root.mkdir(mode=0o700);server=socket.socket(socket.AF_UNIX);server.bind(str(root/'crewd.sock'));server.listen();calls=[]
 def receive():
  conn,_=server.accept();f=conn.makefile('rwb',buffering=0)
  try:
   for line in f:
    frame=json.loads(line);method=frame.get('method');calls.append(frame);value=[];error=None
    if method=='system.hello':value={'server':'crewd','apiVersion':3,'capabilities':['session.respondRunnerRequest','orchestra','command.receipts'],'runtimeIdentity':{'dbId':'db','runtimeBindingId':'bf4e6889-8f2d-48f7-9323-623ff459d2a5'}}
    elif method=='events.subscribe':value={}
    elif method=='remote.status':value={'identity':{}}
    elif method=='lifecycle.status':value={'role':'owner','ownerPresent':True,'previousExit':'clean'}
    elif method=='lifecycle.exit':value={'status':'timedOut','mode':'safePause','clean':[],'interrupted':[],'failed':[]}
    elif method=='crew.list':value=[{'id':'crew'}]
    elif method=='restart.pending':value=None
    elif method=='session.controlTarget':
     assert frame['params']=={'sessionId':'selected-session'}
     if mode in ['late','stopping']:time.sleep(.35)
     value={'sessionId':'foreign-session' if mode=='foreign' else 'selected-session','generation':'current-boot:0:0','controlRevision':'0','turnId':None}
     if mode=='failure':error={'code':-32009,'message':'controlled read refusal'}
    elif method=='command.submit':
     e=frame['params'];cmd=e['command'];p=cmd['params'];target=p['target'] if mode=='stop' else p
     assert target==({'sessionId':'selected-session','generation':'typed-old','controlRevision':'7','turnId':None} if mode=='typed' else {'sessionId':'selected-session','generation':'current-boot:0:0','controlRevision':'0','turnId':None})
     assert cmd['operation']==('session.interrupt' if mode=='interrupt' else 'session.stop' if mode=='stop' else 'session.resume')
     phase='dispatched' if mode=='resume' else 'rejected' if mode=='typed' else 'confirmed'
     value={'requestId':e['requestId'],'dbId':e['dbId'],'runtimeBindingId':e['runtimeBindingId'],'command':cmd,'phase':phase,'historical':False,'payloadSha256':'a'*64,'createdAt':'2026-10-16T00:00:00Z','updatedAt':'2026-10-16T00:00:00Z','result':{}}
    response={'jsonrpc':'2.0','id':frame['id']};response['error' if error else 'result']=error or value
    f.write(json.dumps(response).encode()+b'\n')
  except (BrokenPipeError,ConnectionResetError):pass
  finally:conn.close()
 t=threading.Thread(target=receive,daemon=True);t.start()
 r=subprocess.run(['/private/tmp/pcw-client-explicit-session-control',str(root),mode],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=20)
 t.join(1);server.close();(root/'probe.log').write_text(r.stdout);(root/'calls.json').write_text(json.dumps(calls,indent=2));methods=[x.get('method') for x in calls]
 submits=methods.count('command.submit');reads=methods.count('session.controlTarget');expected=mode in ['resume','interrupt','stop','typed']
 print(mode,r.returncode,'submits',submits,'targetReads',reads,r.stdout[:180],flush=True)
 results.append({'mode':mode,'exit':r.returncode,'submits':submits,'targetReads':reads})
 assert r.returncode==0 and submits==int(expected) and reads==int(mode not in ['typed','mixed','missing'])
(base/'results.json').write_text(json.dumps(results,indent=2))
