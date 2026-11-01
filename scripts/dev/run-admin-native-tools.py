#!/usr/bin/env python3
"""Actual Codex + built crewd MCP + real case DB; loopback provider only.
Default: StandIn case birth, real native/helper/RPC/Tx seam. --core-fence:
real Core -> production adapter case birth and physical authority fence.
In core-fence mode the source uses the production adapter and the case is
created by the production periodic judgment/activation pump. The default mode
uses StandIn case birth. Neither mode proves live model success.
No daily data/settings touched.
"""
import argparse, base64, datetime, time, hashlib, http.server, json, os, pathlib, shutil, stat, subprocess, tempfile, threading

def clean_auth(base):
    runtime=base/'runtime'
    if not runtime.exists(): return []
    fixture=json.loads((base/'fixture.json').read_text())
    removed=[]
    for home in runtime.glob('codex-case-home-*'):
        info=home.lstat()
        assert stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode) and info.st_uid==os.getuid() and info.st_mode&0o077==0
        fd=os.open(home,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
        try:
            markerfd=os.open('pcw-case-home.json',os.O_RDONLY|os.O_NOFOLLOW,dir_fd=fd)
            with os.fdopen(markerfd) as stream:
                info=os.fstat(stream.fileno());assert info.st_size<16384 and info.st_mode&0o077==0
                marker=json.load(stream)
            assert marker['version'] in [1,2] and marker['session']==fixture['session'] and marker['cwd']==str(base/'project')
            assert pathlib.Path(marker['sourceHome']).is_absolute()
            if marker['version']==1: assert not pathlib.Path(marker['sourceHome']).is_relative_to(base)
            else: assert pathlib.Path(marker['sourceHome'])==base/'source'
            try: info=os.stat('auth.json',dir_fd=fd,follow_symlinks=False)
            except FileNotFoundError: continue
            assert stat.S_ISREG(info.st_mode) and info.st_nlink==1 and info.st_uid==os.getuid() and info.st_mode&0o077==0
            os.unlink('auth.json',dir_fd=fd);removed.append(home.name)
        finally: os.close(fd)
    return removed

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--codex',default=shutil.which('codex'));parser.add_argument('--core-fence',action='store_true');parser.add_argument('--refresh-on-case',action='store_true');args=parser.parse_args()
    assert not args.refresh_on_case or args.core_fence, 'refresh proof requires original native owner'
    assert args.codex,'codex executable required'
    repo=pathlib.Path(__file__).resolve().parents[2]
    base=pathlib.Path(tempfile.mkdtemp(prefix='pcw-adm-native-',dir='/private/tmp'));(base/'project').mkdir()
    state={'requests':[],'calls':[],'coreRequests':0,'refreshRequests':0,'case401':0,'responseAuthHashes':[],'caseAuthHashes':[]}
    source=base/'source';source.mkdir(mode=0o700)
    def jwt(salt):
        encode=lambda v:base64.urlsafe_b64encode(json.dumps(v).encode()).decode().rstrip('=')
        return encode({'alg':'none'})+'.'+encode({'exp':int(time.time())+86400,'sub':'synthetic-user','email':'synthetic@example.invalid','https://api.openai.com/auth':{'chatgpt_account_id':'synthetic-account','chatgpt_user_id':'synthetic-user','chatgpt_plan_type':'pro'}})+'.'+salt
    authfile=source/'auth.json'
    authfile.write_text(json.dumps({'auth_mode':'chatgpt','OPENAI_API_KEY':None,'tokens':{'access_token':jwt('initial'),'id_token':jwt('identity'),'refresh_token':'synthetic-refresh','account_id':'synthetic-account'},'last_refresh':datetime.datetime.now(datetime.timezone.utc).isoformat()}));authfile.chmod(0o600)
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self,*_): pass
        def do_GET(self):
            self.send_response(200);self.end_headers();self.wfile.write(b'{"data":[]}')
        def do_POST(self):
            raw=self.rfile.read(int(self.headers.get('content-length','0')))
            if self.path=='/oauth/token':
                state['refreshRequests']+=1;self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(json.dumps({'access_token':jwt('rotated'+str(state['refreshRequests'])),'id_token':jwt('identity'),'refresh_token':'synthetic-refresh-rotated'}).encode());return
            if self.path!='/v1/responses':
                self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(b'{}');return
            state['responseAuthHashes'].append(hashlib.sha256(self.headers.get('Authorization','').encode()).hexdigest())
            fixture=json.loads((base/'fixture.json').read_text())
            if fixture.get('mode')=='core': state['caseAuthHashes'].append(state['responseAuthHashes'][-1])
            if args.refresh_on_case and fixture.get('mode')=='core' and state['coreRequests']==1 and state['case401']==0:
                state['case401']+=1
                self.send_response(401);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(b'{"error":{"message":"synthetic access rejected","type":"invalid_request_error","code":"token_expired"}}');return
            body=json.loads(raw);state['requests'].append(body)
            f=json.loads((base/'fixture.json').read_text());i=len(state['requests'])-1
            if f.get('mode')=='core': i=state['coreRequests'];state['coreRequests']+=1
            close={'id':f.get('work','source-has-no-work'),'expected_revision':f.get('revision','unused'),'kind':'done','evidence':{'kind':'text','value':'isolated native integration fixture verified closure'},'note':'actual native case collaboration proof'}
            steps=([('work_item_get',{'id':f['work']}),('work_item_close',close)] if f.get('mode')=='core' else [] if f.get('mode')=='source' else [('work_item_get',{'id':f['work']}),('work_item_get',{'id':f['foreign']}),('work_item_close',dict(close,id=f['foreign'],expected_revision=f['foreignRevision'])),('post_to_crew',{'text':'must never be posted'}),('work_item_close',close),('work_item_close',close)])
            if f.get('mode')=='source': steps=[]
            rid=f'resp_{i}'
            if i<len(steps):
                name,params=steps[i]
                item={'type':'function_call','id':f'fc_{i}','call_id':f'call_{i}','namespace':'mcp__crew','name':name,'arguments':json.dumps(params),'status':'completed'}
            else: item={'type':'message','id':f'msg_{i}','role':'assistant','status':'completed','content':[{'type':'output_text','text':'Finished isolated native case tools proof.','annotations':[]}]}
            state['calls'].append(item);self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
            def event(kind,**data):
                self.wfile.write(('event: '+kind+'\ndata: '+json.dumps({'type':kind,**data})+'\n\n').encode());self.wfile.flush()
            event('response.created',response={'id':rid,'object':'response','status':'in_progress','output':[]})
            event('response.output_item.added',output_index=0,item=item)
            event('response.output_item.done',output_index=0,item=item)
            event('response.completed',response={'id':rid,'object':'response','status':'completed','output':[item],'usage':{'input_tokens':10,'output_tokens':10,'total_tokens':20}})
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
    wrapper=base/'codex-wrapper'
    argv=[str(pathlib.Path(args.codex).resolve())]
    for setting in ['model_provider="pcw_case"','model="gpt-5.1-codex"','model_providers.pcw_case.name="Local case tools proof"',f'model_providers.pcw_case.base_url="http://127.0.0.1:{server.server_port}/v1"','model_providers.pcw_case.wire_api="responses"','model_providers.pcw_case.requires_openai_auth=true',f'chatgpt_base_url="http://127.0.0.1:{server.server_port}"']:
        argv.extend(['-c',setting])
    wrapper.write_text('#!/usr/bin/env python3\nimport os,sys\nos.environ["CODEX_REFRESH_TOKEN_URL_OVERRIDE"]='+repr(f'http://127.0.0.1:{server.server_port}/oauth/token')+'\na='+repr(argv)+'\nos.execv(a[0],a+sys.argv[1:])\n');wrapper.chmod(0o700)
    try:
        build=subprocess.run(['cargo','build','-p','crewd'],cwd=repo/'crewd',capture_output=True,text=True,timeout=240)
        (base/'helper-build.log').write_text(build.stdout+build.stderr);assert build.returncode==0,build.stderr[-2500:]
        helper=repo/'crewd'/'target'/'debug'/'crewd'
        (base/'helper-sha256').write_text(hashlib.sha256(helper.read_bytes()).hexdigest()+'\n')
        env=dict(os.environ,CODEX_HOME=str(source),PCW_ADMIN_NATIVE_PROBE='loopback',PCW_ADMIN_NATIVE_PROGRAM=str(wrapper),PCW_ADMIN_NATIVE_HELPER=str(helper),PCW_ADMIN_NATIVE_EVIDENCE=str(base),TMPDIR=str(base))
        run=subprocess.run(['cargo','test','-p','crewd-server','--lib','real_core_native_case_source_revocation_stops_physical_process' if args.core_fence else 'real_native_case_tools_scope_and_factual_close','--','--ignored','--nocapture'],cwd=repo/'crewd',env=env,capture_output=True,text=True,timeout=240)
        (base/'test.log').write_text(run.stdout+run.stderr);assert run.returncode==0,(run.returncode,run.stdout[-3000:],run.stderr[-1500:])
        assert '1 passed' in run.stdout,'ignored test was not registered/executed'
        if args.core_fence:
            if args.refresh_on_case:
                assert state['case401']==1 and state['refreshRequests']==1, ('refresh must use original manager exactly once',state['case401'],state['refreshRequests'])
                # Case get, rejected next response, accepted retry, final response.
                # An idle resumed source need not issue any model HTTP request.
                hashes=state['caseAuthHashes'];assert len(hashes)==4 and hashes[1]!=hashes[2], ('401 retry must carry changed access',hashes)
            assert state['coreRequests']==3, ('first real Core turn incomplete',state['coreRequests'])
            assert not list((base/'runtime').glob('codex-case-home-*/auth.json')), 'case must not persist auth'
            receipt=json.loads((base/'core-fence-receipt.json').read_text())
            assert all(receipt[key] for key in ['ready','closureActorVerified','profileAdoptedFromUnset','liveBeforeRevocation','fencedBeforeCleanup','physicallyGoneBeforeCleanup','adminAdmissionRevoked'])
            assert receipt['closedWork']['status']=='done' and receipt['before']['executionRecord']['effectsCount']==1
            print('PASS automatic production judgment/pump -> real Core -> production Codex adapter -> actual native crew get/close with Admin event and one effect; source admission revoke -> physical native exit before cleanup. Original source also uses production Codex with synthetic credentials.')
            return
        assert len(state['requests'])==7,('sequence incomplete',len(state['requests']))
        results={e['call_id']:e.get('output','') for r in state['requests'] for e in r.get('input',[]) if isinstance(e,dict) and e.get('type')=='function_call_output'}
        assert 'administrative source' in str(results.get('call_0','')),results
        assert 'FOREIGN_NATIVE_CASE_SENTINEL' not in json.dumps(state['requests'])
        for n in [1,2,3,5]:
            output=str(results.get('call_'+str(n),''));assert output,('missing denial',n)
            assert any(s in output.lower() for s in ['error','refus','denied','unavailable','unknown','invalid','not found','not supported','unsupported','forbidden','没成','未写入']), (n,output)
        receipt=json.loads((base/'receipt.json').read_text());assert receipt['after']['status']=='done' and receipt['case']['executionRecord']['effectsCount']==1
        print('PASS actual native Codex -> actual built crew MCP -> real scoped case read/close; foreign/nonallowlisted/stale denied. StandIn Core birth only.')
    finally:
        server.shutdown();authfile.unlink(missing_ok=True);(base/'requests.json').write_text(json.dumps(state,indent=2))
        removed=clean_auth(base);(base/'auth-cleanup.json').write_text(json.dumps({'removedPrivateAuthCopies':removed})+'\n')
        print('Evidence:',base,flush=True)
if __name__=='__main__': main()
