// Cheap capability survey: initialize + session/new only. NO prompt, so no model tokens.
// Detects whether the agent actually spawned our client-provided stdio MCP server,
// using a marker file our fake server writes on startup -- a signal from OUR side,
// not the agent's self-report.
import { spawn } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
const out={};
const AGENT = process.env.ACP_AGENT.split(" ");
const CWD = process.env.ACP_CWD;
const MARKER = process.env.MCP_LOG;
const WAIT_MS = Number(process.env.MCP_WAIT_MS || 8000);
try { rmSync(MARKER, { force: true }); } catch {}
const child = spawn(AGENT[0], AGENT.slice(1), { stdio:["pipe","pipe","pipe"], cwd:CWD, env:process.env });
let stderrTail = "";
child.stderr.on("data", d => { stderrTail = (stderrTail + d).slice(-400); });
let nextId=1; const pending=new Map(); let buf="";
child.stdout.on("data",c=>{ buf+=c.toString(); let i;
 while((i=buf.indexOf("\n"))>=0){ const l=buf.slice(0,i).trim(); buf=buf.slice(i+1); if(!l)continue;
  let m; try{m=JSON.parse(l);}catch{continue;}
  if(m.id!==undefined&&m.method){
    // MUST auto-allow permission requests. codex gates the MCP tool call behind one;
    // refusing it means the tool never runs, the server never spawns, and every agent
    // reads as "cannot mount MCP". The control caught exactly this false negative.
    if(m.method==="session/request_permission"){
      const opts=m.params?.options||[];
      const pick=opts.find(o=>o.kind==="allow_once")||opts.find(o=>String(o.kind||"").startsWith("allow"))||opts[0];
      out.permissionRequests=(out.permissionRequests||0)+1;
      child.stdin.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result:{outcome:{outcome:"selected",optionId:pick?.optionId}}})+"\n");
    } else {
      out.unhandledAgentRequests=[...new Set([...(out.unhandledAgentRequests||[]),m.method])];
      child.stdin.write(JSON.stringify({jsonrpc:"2.0",id:m.id,error:{code:-32601,message:"probe8 client does not implement "+m.method}})+"\n");
    }
  }
  else if(m.id!==undefined){ const p=pending.get(m.id); pending.delete(m.id);
    if(p) m.error?p.reject(new Error(JSON.stringify(m.error))):p.resolve(m.result); }
 }});
const call=(method,params)=>{const id=nextId++;return new Promise((r,j)=>{pending.set(id,{resolve:r,reject:j});child.stdin.write(JSON.stringify({jsonrpc:"2.0",id,method,params})+"\n");});};
const timeout=(ms)=>new Promise((_,j)=>setTimeout(()=>j(new Error("timeout after "+ms+"ms")),ms));
(async()=>{
 out.agent=process.env.ACP_LABEL||AGENT.join(" ");
 try {
   const init=await Promise.race([call("initialize",{protocolVersion:1,clientInfo:{name:"probe8",version:"0"},
     clientCapabilities:{fs:{readTextFile:false,writeTextFile:false},terminal:false}}), timeout(30000)]);
   out.protocolVersion=init.protocolVersion;
   out.agentInfo=init.agentInfo;
   out.caps=init.agentCapabilities;
   out.authMethods=(init.authMethods||[]).map(a=>a.id);
 } catch(e){ out.initializeError=e.message.slice(0,300); out.stderr=stderrTail; console.log(JSON.stringify(out,null,1)); child.kill(); process.exit(0); }
 try {
   const s=await Promise.race([call("session/new",{cwd:CWD,mcpServers:[
     {name:"crew",command:process.execPath,args:[process.env.FAKE_MCP],env:[{name:"MCP_LOG",value:MARKER}]}]}), timeout(60000)]);
   out.sessionOk=true; out.sessionId=s.sessionId;
   out.modeIds=(s.modes?.availableModes||[]).map(m=>m.id);
   out.configOptionIds=(s.configOptions||[]).map(c=>c.id);
 } catch(e){ out.sessionNewError=e.message.slice(0,300); out.stderr=stderrTail; console.log(JSON.stringify(out,null,1)); child.kill(); process.exit(0); }
 // Phase 1: did it mount eagerly, at session/new?
 let t0=Date.now();
 while (Date.now()-t0 < WAIT_MS && !existsSync(MARKER)) await new Promise(r=>setTimeout(r,200));
 out.mountedEagerly = existsSync(MARKER);
 // Phase 2: codex mounts lazily -- verified against the control, it stays unspawned
 // through session/new AND through a trivial turn. Only a turn that actually reaches
 // for the tool forces the mount. Without this phase a lazy mounter reads identical
 // to a non-mounter: that false negative is what the control caught.
 if (!out.mountedEagerly && process.env.ALLOW_TURN === "1") {
   try {
     const r = await Promise.race([call("session/prompt",{sessionId:out.sessionId,
       prompt:[{type:"text",text:"Call the MCP tool named post_to_crew exactly once with message='survey'. If you cannot find that tool, say NOTOOL and do not run any shell command."}]}), timeout(180000)]);
     out.trivialTurnStop = r.stopReason;
   } catch(e){ out.trivialTurnError = e.message.slice(0,200); }
   t0=Date.now();
   while (Date.now()-t0 < WAIT_MS && !existsSync(MARKER)) await new Promise(r=>setTimeout(r,200));
 }
 out.mcpServerSpawned = existsSync(MARKER);
 console.log(JSON.stringify(out,null,1));
 child.kill(); process.exit(0);
})().catch(e=>{ out.fatal=e.message.slice(0,300); console.log(JSON.stringify(out,null,1)); child.kill(); process.exit(1); });
