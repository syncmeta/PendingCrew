import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";
const AGENT = process.env.ACP_AGENT.split(" ");
const CWD = process.env.ACP_CWD;
const SID = process.env.SESSION_ID;
const child = spawn(AGENT[0], AGENT.slice(1), { stdio:["pipe","pipe","pipe"], cwd: CWD, env: process.env });
child.stderr.on("data", d=>process.stderr.write("[stderr] "+d));
let nextId=1; const pending=new Map(); let buf=""; const replayed=[];
child.stdout.on("data", c=>{ buf+=c.toString(); let i;
 while((i=buf.indexOf("\n"))>=0){ const l=buf.slice(0,i).trim(); buf=buf.slice(i+1); if(!l)continue;
  let m; try{m=JSON.parse(l);}catch{continue;}
  if(m.method==="session/update"){ replayed.push(m.params.update); }
  else if(m.id!==undefined&&m.method){ send({jsonrpc:"2.0",id:m.id,result:{outcome:{outcome:"selected",optionId:(m.params.options||[])[0]?.optionId}}}); }
  else if(m.id!==undefined){ const p=pending.get(m.id); pending.delete(m.id); if(p) m.error?p.reject(new Error(JSON.stringify(m.error))):p.resolve(m.result); }
 }});
function send(o){ child.stdin.write(JSON.stringify(o)+"\n"); }
function call(method,params){ const id=nextId++; return new Promise((res,rej)=>{pending.set(id,{resolve:res,reject:rej}); send({jsonrpc:"2.0",id,method,params});}); }
const out={};
(async()=>{
 await call("initialize",{protocolVersion:1,clientInfo:{name:"probe3",version:"0"},clientCapabilities:{fs:{readTextFile:false,writeTextFile:false},terminal:false}});
 // session/list — can we enumerate persisted sessions from a cold process?
 try{ const ls = await call("session/list",{cwd:CWD}); out.listCount=(ls.sessions||[]).length; out.listFirst=(ls.sessions||[])[0]; }catch(e){ out.listError=e.message; }
 // session/load — replay full history into a cold client
 const t0=Date.now();
 try{ out.load = await call("session/load",{sessionId:SID,cwd:CWD,mcpServers:[]}); out.loadMs=Date.now()-t0; out.replayedCount=replayed.length;
      out.replayedKinds=[...new Set(replayed.map(u=>u.sessionUpdate))]; }catch(e){ out.loadError=e.message; }
 // session/set_config_option with the CORRECT field name
 try{ out.setEffort = await call("session/set_config_option",{sessionId:SID,configId:"reasoning_effort",value:"low"}); }catch(e){ out.setEffortError=e.message; }
 try{ out.setModel = await call("session/set_config_option",{sessionId:SID,configId:"model",value:"gpt-5.6-luna"}); }catch(e){ out.setModelError=e.message; }
 // memory check: does the resumed session remember the earlier turn?
 const r = await call("session/prompt",{sessionId:SID,prompt:[{type:"text",text:"In one short sentence: what tool did I ask you to call at the very start of this conversation?"}]});
 out.memoryStop=r.stopReason;
 out.memoryText=replayed.filter(u=>u.sessionUpdate==="agent_message_chunk").map(u=>u.content?.text||"").join("").slice(-400);
 writeFileSync(process.env.OUT, JSON.stringify(out,null,1));
 child.kill(); process.exit(0);
})().catch(e=>{ out.fatal=e.message; writeFileSync(process.env.OUT,JSON.stringify(out,null,1)); child.kill(); process.exit(1); });
