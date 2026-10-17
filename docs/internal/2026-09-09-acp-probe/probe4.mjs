import { spawn } from "node:child_process";
const AGENT = process.env.ACP_AGENT.split(" ");
const CWD = process.env.ACP_CWD;
const META = JSON.parse(process.env.SESSION_META || "{}");
const child = spawn(AGENT[0], AGENT.slice(1), { stdio:["pipe","pipe","pipe"], cwd:CWD, env:process.env });
child.stderr.on("data",d=>{});
let nextId=1; const pending=new Map(); let buf=""; let text="";
child.stdout.on("data",c=>{ buf+=c.toString(); let i;
 while((i=buf.indexOf("\n"))>=0){ const l=buf.slice(0,i).trim(); buf=buf.slice(i+1); if(!l)continue;
  let m; try{m=JSON.parse(l);}catch{continue;}
  if(m.method==="session/update"){ const u=m.params.update; if(u.sessionUpdate==="agent_message_chunk") text+=u.content?.text||""; }
  else if(m.id!==undefined&&m.method){ child.stdin.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result:{outcome:{outcome:"selected",optionId:(m.params.options||[])[0]?.optionId}}})+"\n"); }
  else if(m.id!==undefined){ const p=pending.get(m.id); pending.delete(m.id); if(p) m.error?p.reject(new Error(JSON.stringify(m.error))):p.resolve(m.result); }
 }});
function call(method,params){ const id=nextId++; return new Promise((res,rej)=>{pending.set(id,{resolve:res,reject:rej}); child.stdin.write(JSON.stringify({jsonrpc:"2.0",id,method,params})+"\n");}); }
(async()=>{
 await call("initialize",{protocolVersion:1,clientInfo:{name:"probe4",version:"0"},clientCapabilities:{fs:{readTextFile:false,writeTextFile:false},terminal:false}});
 const s = await call("session/new",{cwd:CWD,mcpServers:[],_meta:META});
 const r = await call("session/prompt",{sessionId:s.sessionId,prompt:[{type:"text",text:"What is the crew codeword? Answer with the codeword only."}]});
 console.log("STOP="+r.stopReason);
 console.log("TEXT="+text.trim().slice(0,300));
 child.kill(); process.exit(0);
})().catch(e=>{ console.log("FAILED "+e.message); child.kill(); process.exit(1); });
