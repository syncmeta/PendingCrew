import { spawn } from "node:child_process";
const AGENT=process.env.ACP_AGENT.split(" "); const CWD=process.env.ACP_CWD;
const child=spawn(AGENT[0],AGENT.slice(1),{stdio:["pipe","pipe","pipe"],cwd:CWD,env:process.env});
child.stderr.on("data",d=>{});
let nextId=1; const pending=new Map(); let buf=""; let chunks=0; let lastChunkAt=0;
child.stdout.on("data",c=>{ buf+=c.toString(); let i;
 while((i=buf.indexOf("\n"))>=0){ const l=buf.slice(0,i).trim(); buf=buf.slice(i+1); if(!l)continue;
  let m; try{m=JSON.parse(l);}catch{continue;}
  if(m.method==="session/update"){ const u=m.params.update;
    if(u.sessionUpdate==="agent_message_chunk"){chunks++; lastChunkAt=Date.now();} }
  else if(m.id!==undefined&&m.method){ child.stdin.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result:{outcome:{outcome:"selected",optionId:(m.params.options||[])[0]?.optionId}}})+"\n"); }
  else if(m.id!==undefined){ const p=pending.get(m.id); pending.delete(m.id); if(p) m.error?p.reject(new Error(JSON.stringify(m.error))):p.resolve(m.result); }
 }});
const call=(method,params)=>{const id=nextId++; return new Promise((r,j)=>{pending.set(id,{resolve:r,reject:j}); child.stdin.write(JSON.stringify({jsonrpc:"2.0",id,method,params})+"\n");});};
(async()=>{
 await call("initialize",{protocolVersion:1,clientInfo:{name:"probe6",version:"0"},clientCapabilities:{fs:{readTextFile:false,writeTextFile:false},terminal:false}});
 const s=await call("session/new",{cwd:CWD,mcpServers:[]});
 const t0=Date.now();
 const p=call("session/prompt",{sessionId:s.sessionId,prompt:[{type:"text",text:"Write a very long detailed essay, at least 4000 words, about the history of the printing press. Do not stop early."}]});
 await new Promise(r=>setTimeout(r,6000));
 const tCancel=Date.now();
 child.stdin.write(JSON.stringify({jsonrpc:"2.0",method:"session/cancel",params:{sessionId:s.sessionId}})+"\n");
 const res=await p;
 console.log(`stopReason=${res.stopReason} cancelSentAt=+${tCancel-t0}ms returnedAt=+${Date.now()-t0}ms chunksReceived=${chunks} lastChunk=+${lastChunkAt?lastChunkAt-t0:-1}ms`);
 child.kill(); process.exit(0);
})().catch(e=>{console.log("FAILED "+e.message.slice(0,200)); child.kill(); process.exit(1);});
