import hashlib,json,pathlib,socket,subprocess,tempfile,time,sys,shutil
program=pathlib.Path(sys.argv[1]).resolve();expected=sys.argv[2];peer=pathlib.Path(sys.argv[3]).resolve();peer_sha=sys.argv[4]
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
assert sha(program)==expected and sha(peer)==peer_sha
base=pathlib.Path(tempfile.mkdtemp(prefix='pcw-M6-native-normal-',dir='/private/tmp'));print(base,flush=True)
for name in ['home','data']:(base/name).mkdir()
env={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(base/'home'),'SHELL':'/usr/bin/false'}
args=[str(program),'--data-dir',str(base/'data'),'serve','--codex','/usr/bin/false','--claude','/usr/bin/false','--codex-env','HOME='+str(base/'home'),'--claude-env','HOME='+str(base/'home')]
(base/'input-binaries.json').write_text(json.dumps({'normal':str(program),'normalSHA256':expected,'peer':str(peer),'peerSHA256':peer_sha,'command':args},indent=2));shutil.copyfile(__file__,base/'coordinator.py')
with (base/'daemon.log').open('w') as log:
 process=subprocess.Popen(args,env=env,stdout=log,stderr=log)
 try:
  for _ in range(200):
   assert process.poll() is None,'normal daemon exited'
   s=socket.socket(socket.AF_UNIX)
   try:s.connect(str(base/'data/crewd.sock'));break
   except OSError:s.close();time.sleep(.05)
  else:raise AssertionError('normal socket absent')
  s.settimeout(10);f=s.makefile('rb');seq=0
  def call(method,params):
   global seq
   seq+=1;s.sendall((json.dumps({'id':seq,'method':method,'params':params})+'\n').encode())
   while True:
    r=json.loads(f.readline())
    if r.get('id')==seq:assert 'error' not in r,r;return r['result']
  before=call('remote.status',{});assert not before['listening'] and not before['configuration']['enabled']
  reserve=socket.socket();reserve.bind(('127.0.0.1',0));port=reserve.getsockname()[1];reserve.close()
  import uuid
  call('remote.configure',{'requestId':str(uuid.uuid4()),'expectedRevision':before['configuration']['revision'],'enabled':True,'listenHost':'127.0.0.1','port':port})
  assert call('remote.status',{})['listening'];f.close();s.close()
  result=subprocess.run([str(peer),str(base/'data')],env=env,capture_output=True,timeout=50)
  (base/'native-peer.log').write_bytes(result.stdout+result.stderr);(base/'peer-exit.txt').write_text(str(result.returncode)+'\n')
  assert result.returncode==0,'Native peer failed; see '+str(base/'native-peer.log')
  assert (base/'data/native-result.txt').is_file()
 finally:
  if process.poll() is None:process.terminate()
  process.wait(timeout=20)
 assert process.returncode==0,process.returncode
assert sha(program)==expected and sha(peer)==peer_sha
print('PASS actual normal11 + Apple native memory-vault pairing/TLS/work/event/revoke; providers disabled',flush=True)
