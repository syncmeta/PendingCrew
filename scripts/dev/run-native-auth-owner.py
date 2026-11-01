#!/usr/bin/env python3
"""Real native auth owner proof with synthetic file credentials and loopback OAuth."""
import base64,datetime,hashlib,http.server,json,os,pathlib,shutil,subprocess,tempfile,threading,time
repo=pathlib.Path(__file__).resolve().parents[2]
base=pathlib.Path(tempfile.mkdtemp(prefix='pcw-native-auth-owner-',dir='/private/tmp'))
for name in ['project','source']:(base/name).mkdir(mode=0o700)
def jwt(salt):
    enc=lambda x:base64.urlsafe_b64encode(json.dumps(x).encode()).decode().rstrip('=')
    return enc({'alg':'none'})+'.'+enc({'exp':int(time.time())+86400,'sub':'synthetic-user','email':'synthetic@example.invalid','https://api.openai.com/auth':{'chatgpt_account_id':'synthetic-account','chatgpt_user_id':'synthetic-user','chatgpt_plan_type':'pro'}})+'.'+salt
state={'refreshRequests':0,'otherRequests':[]}
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self,*_):pass
    def do_GET(self):
        state['otherRequests'].append({'method':'GET','path':self.path})
        self.send_response(200);self.end_headers();self.wfile.write(b'{"data":[]}')
    def do_POST(self):
        body=self.rfile.read(int(self.headers.get('content-length','0')))
        if self.path=='/oauth/token':
            state['refreshRequests']+=1
            self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers()
            self.wfile.write(json.dumps({'access_token':jwt('rotated'),'id_token':jwt('identity'),'refresh_token':'synthetic-rotated-refresh'}).encode())
        else:
            state['otherRequests'].append({'method':'POST','path':self.path})
            self.send_response(404);self.end_headers()
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
auth={'auth_mode':'chatgpt','OPENAI_API_KEY':None,'tokens':{'access_token':jwt('initial'),'id_token':jwt('identity'),'refresh_token':'synthetic-original-refresh','account_id':'synthetic-account'},'last_refresh':datetime.datetime.now(datetime.timezone.utc).isoformat()}
path=base/'source'/'auth.json';path.write_text(json.dumps(auth));path.chmod(0o600)
wrapper=base/'codex-wrapper';binary=shutil.which('codex');assert binary
argv=[binary]
for setting in ['model_provider="pcw_probe"','model="gpt-5.1-codex"','model_providers.pcw_probe.name="Loopback"',f'model_providers.pcw_probe.base_url="http://127.0.0.1:{server.server_port}/v1"','model_providers.pcw_probe.wire_api="responses"','model_providers.pcw_probe.requires_openai_auth=true','cli_auth_credentials_store="file"']:
    argv.extend(['-c',setting])
wrapper.write_text('#!/usr/bin/env python3\nimport os,sys\na='+repr(argv)+'\nos.execv(a[0],a+sys.argv[1:])\n');wrapper.chmod(0o700)
try:
    env=dict(os.environ,PCW_AUTH_OWNER_PROBE='synthetic-loopback',PCW_AUTH_OWNER_BASE=str(base),PCW_AUTH_OWNER_PROGRAM=str(wrapper),PCW_AUTH_OWNER_REFRESH_URL=f'http://127.0.0.1:{server.server_port}/oauth/token')
    run=subprocess.run(['cargo','test','-p','crewd-exec','--test','native_auth_owner','--','--ignored','--nocapture'],cwd=repo/'crewd',env=env,capture_output=True,text=True,timeout=180)
    (base/'test.log').write_text(run.stdout+run.stderr)
    assert run.returncode==0,(run.returncode,run.stdout[-1500:],run.stderr[-1500:])
    assert state['refreshRequests']==1,state
    print('PASS actual native original owner refresh serialized; concurrent callers reuse one access value; exit rejects export.')
finally:
    server.shutdown();path.unlink(missing_ok=True)
    (base/'http-counts.json').write_text(json.dumps(state,indent=2))
    print('Evidence:',base)
