#!/usr/bin/env python3
"""Opt-in local Claude 2.1.301 enforcement probe; fake provider, isolated config.
No real model request or user configuration changes. Exercises actual CLI tools.
Production Claude case execution remains disabled until managed policy is attested.
--inject-plugin --unguarded reproduces the raw CLI vulnerability (expected red);
--inject-plugin verifies the actual Rust environment guard refuses before launch.
"""
import argparse, http.server, json, os, pathlib, subprocess, tempfile, threading, uuid

parser = argparse.ArgumentParser()
parser.add_argument('--claude', required=True)
parser.add_argument('--inject-plugin', action='store_true', help='Probe an inherited executable plugin environment')
parser.add_argument('--unguarded', action='store_true', help='Reproduce underlying CLI plugin bypass; expected failure with --inject-plugin')
args = parser.parse_args()
root = pathlib.Path(tempfile.mkdtemp(prefix='pcw-claude-native-profile-', dir='/private/tmp'))
project = root / 'project'; project.mkdir()
config = root / 'config'; config.mkdir()
(project / 'read.txt').write_text('read-only sentinel\n')
(root / 'outside.txt').write_text('outside sentinel\n')
(project / 'outside-link').symlink_to(root / 'outside.txt')
helper = root / 'mcp.py'
helper.write_text('''import sys,json,pathlib
marker=pathlib.Path(sys.argv[1]);marker.write_text("started")
for line in sys.stdin:
 try: q=json.loads(line)
 except ValueError: continue
 if 'id' not in q: continue
 m=q.get('method')
 if m=='initialize': r={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'probe','version':'1'}}
 elif m=='tools/list': r={'tools':[{'name':'case_probe','description':'Return isolated probe receipt','inputSchema':{'type':'object','properties':{}}}]}
 elif m=='tools/call': marker.with_suffix('.called').write_text('called');r={'content':[{'type':'text','text':'crew-only receipt'}]}
 else: r={}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':r}),flush=True)
''')
def mcp(marker):
    return {'type': 'stdio', 'command': '/usr/bin/python3', 'args': [str(helper), str(root / marker)]}
(project / '.mcp.json').write_text(json.dumps({'mcpServers': {'outsider': mcp('foreign-started')}}))
(config / 'settings.json').write_text(json.dumps({'permissions': {'allow': ['Bash', 'Write', 'Edit']}, 'hooks': {'SessionStart': [{'hooks': [{'type': 'command', 'command': 'touch ' + str(root / 'foreign-hook')}]}]}}))
settings = root / 'settings.json'
settings.write_text(json.dumps({'permissions': {'allow': ['Read', 'Grep', 'Glob', 'mcp__crew__*'], 'deny': ['Bash','PowerShell','Write','Edit','NotebookEdit','Agent','Task','Skill','WebFetch','WebSearch','AskUserQuestion']}, 'autoMemoryEnabled':False, 'claudeMdExcludes':['**'], 'disableClaudeAiMcpServers':True, 'env': {'ENABLE_TOOL_SEARCH':'false','ENABLE_CLAUDEAI_MCP_SERVERS':'false','CLAUDE_CODE_DISABLE_AUTO_MEMORY':'1'}}))
steps = [
 ('Read', {'file_path':str(project / 'read.txt')}),
 ('Bash', {'command':'touch ' + str(project / 'shell-write')}),
 ('Write', {'file_path':str(project / 'native-write'),'content':'bad'}),
 ('Edit', {'file_path':str(project / 'read.txt'),'old_string':'read-only','new_string':'changed'}),
 ('mcp__outsider__case_probe', {}),
 ('Read', {'file_path':str(root / 'outside.txt')}),
 ('Read', {'file_path':str(project / 'outside-link')}),
 ('mcp__crew__case_probe', {}),
]
state={'step':0, 'requests':[], 'tools':[], 'run':'new'}
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self,*_): pass
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers.get('content-length','0'))))
        if 'count_tokens' in self.path:
            self.send_response(200);self.end_headers();self.wfile.write(b'{"input_tokens":10}');return
        state['requests'].append(body)
        state['tools'].append([t.get('name') for t in body.get('tools',[])])
        index=state['step'];state['step']+=1
        if index<len(steps):
            name,params=steps[index];content=[{'type':'tool_use','id':f'tool_{state["run"]}_{index}','name':name,'input':params}];stop='tool_use'
        else: content=[{'type':'text','text':'Probe complete.'}];stop='end_turn'
        message={'id':'msg_probe','type':'message','role':'assistant','model':body.get('model','claude-sonnet-4-6'),'content':content,'stop_reason':stop,'stop_sequence':None,'usage':{'input_tokens':20,'output_tokens':20}}
        self.send_response(200)
        self.send_header('Content-Type','text/event-stream' if body.get('stream') else 'application/json');self.end_headers()
        if not body.get('stream'): self.wfile.write(json.dumps(message).encode());return
        def event(kind,data): self.wfile.write(('event: '+kind+'\ndata: '+json.dumps(data)+'\n\n').encode());self.wfile.flush()
        event('message_start',{'type':'message_start','message':dict(message,content=[],stop_reason=None)})
        for i,c in enumerate(content):
            event('content_block_start',{'type':'content_block_start','index':i,'content_block':dict(c,input={}) if c['type']=='tool_use' else dict(c,text='')})
            delta={'type':'input_json_delta','partial_json':json.dumps(c['input'])} if c['type']=='tool_use' else {'type':'text_delta','text':c['text']}
            event('content_block_delta',{'type':'content_block_delta','index':i,'delta':delta})
            event('content_block_stop',{'type':'content_block_stop','index':i})
        event('message_delta',{'type':'message_delta','delta':{'stop_reason':stop,'stop_sequence':None},'usage':{'output_tokens':20}})
        event('message_stop',{'type':'message_stop'})
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
threading.Thread(target=server.serve_forever,daemon=True).start()
env={k:v for k,v in os.environ.items() if k in ['PATH','HOME','USER','TMPDIR','LANG']}
env.update(CLAUDE_CONFIG_DIR=str(config),ANTHROPIC_API_KEY='isolated-fake-provider-key',ANTHROPIC_BASE_URL=f'http://127.0.0.1:{server.server_port}',CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC='1',ENABLE_TOOL_SEARCH='false',ENABLE_CLAUDEAI_MCP_SERVERS='false')
if args.inject_plugin:
    plugin=root/'plugin';(plugin/'.claude-plugin').mkdir(parents=True);(plugin/'hooks').mkdir()
    (plugin/'.claude-plugin'/'plugin.json').write_text(json.dumps({'name':'case-boundary-probe','version':'0.0.1'}))
    (plugin/'hooks'/'hooks.json').write_text(json.dumps({'hooks':{'SessionStart':[{'hooks':[{'type':'command','command':'touch '+str(root/'plugin-hook-ran')}]}]}}))
    env['CLAUDE_CODE_PLUGIN_DIRS']=str(plugin)
    if not args.unguarded:
        # Invoke the real Rust candidate-profile validator, not a Python copy.
        guard_env=dict(os.environ)
        guard_env['PCW_TEST_CASE_ENV_JSON']=json.dumps({'CLAUDE_CODE_PLUGIN_DIRS':str(plugin)})
        manifest=pathlib.Path(__file__).resolve().parents[2]/'crewd'/'Cargo.toml'
        guard=subprocess.run(['cargo','test','--manifest-path',str(manifest),'-p','crewd-exec','--lib','launch::tests::restricted_case_probe_environment_gate','--','--ignored','--exact'],env=guard_env,capture_output=True,text=True,timeout=120)
        (root/'environment-guard.log').write_text(guard.stdout+guard.stderr)
        assert guard.returncode!=0 and 'case environment refused before native launch' in guard.stdout,(guard.returncode,'validator did not provide the expected refusal')
        assert not (root/'plugin-hook-ran').exists()
        server.shutdown()
        print('PASS production Rust environment validator refused before invoking native CLI; Evidence:',root)
        raise SystemExit(0)
version=subprocess.check_output([args.claude,'--version'],env=env,text=True).strip()
assert version=='2.1.301 (Claude Code)',version
common=[args.claude,'--restricted','--tools','Read,Grep,Glob','--strict-mcp-config','--mcp-config',json.dumps({'mcpServers':{'crew':mcp('crew-started')}}),'--permission-mode','dontAsk','--disable-slash-commands','--no-chrome','--settings',str(settings),'--model','claude-sonnet-4-6','-p','--verbose','--output-format','stream-json']
sid=str(uuid.uuid4())
try:
 for mode in ['new','resume']:
    state.update(step=0, requests=[],tools=[],run=mode)
    command=common+(['--session-id',sid] if mode=='new' else ['--resume',sid])+['Exercise the isolated local probe tools.']
    r=subprocess.run(command,cwd=project,env=env,capture_output=True,text=True,timeout=60)
    (root/(mode+'.stdout')).write_text(r.stdout);(root/(mode+'.stderr')).write_text(r.stderr)
    (root/(mode+'.requests.json')).write_text(json.dumps(state['requests'],indent=2))
    assert r.returncode==0,(mode,r.returncode,r.stderr[-1500:],r.stdout[-1500:])
    assert state['step']>=len(steps)+1,(mode,state['step'])
    assert not (root/'plugin-hook-ran').exists(), 'restricted CLI executed injected plugin hook; candidate is not a complete process sandbox'
    toolset=set().union(*(set(t) for t in state['tools']))
    assert {'Read','Glob','Grep','mcp__crew__case_probe'}<=toolset,toolset
    assert not ({'Bash','Write','Edit','Agent','Task','Skill','WebFetch','WebSearch','mcp__outsider__case_probe'}&toolset),toolset
    results={}
    for request in state['requests']:
      for message in request.get('messages',[]):
       for block in message.get('content',[]) if isinstance(message.get('content'),list) else []:
        if block.get('type')=='tool_result': results[block['tool_use_id']]=block
    for i in [1,2,3,4,5,6]: assert results.get(f'tool_{mode}_{i}',{}).get('is_error') is True,(mode,i,results)
    assert 'read-only sentinel' in json.dumps(results[f'tool_{mode}_0']),results
    assert 'crew-only receipt' in json.dumps(results[f'tool_{mode}_7']),results
    assert (project/'read.txt').read_text()=='read-only sentinel\n'
    for denied in [project/'shell-write',project/'native-write',root/'foreign-started',root/'foreign-hook']: assert not denied.exists(),denied
    print(mode, 'PASS', sorted(toolset))
 print('Evidence:',root)
finally:
 server.shutdown()
 print('Evidence:',root)
