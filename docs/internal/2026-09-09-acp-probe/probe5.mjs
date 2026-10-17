import { spawn } from "node:child_process";
const AGENT = process.env.ACP_AGENT.split(" ");
const CWD = process.env.ACP_CWD;
function connect(){
  const child = spawn(AGENT[0], AGENT.slice(1), { stdio:["pipe","pipe","pipe"], cwd:CWD, env:process.env });
  child.stderr.on("data",d=>{});
  let nextId=1; const pending=new Map(); let buf=""; const st={text:"",updates:[]};
  child.stdout.on("data",c=>{ buf+=c.toString(); let i;
   while((i=buf.indexOf("\n"))>=0){ const l=buf.slice(0,i).trim(); buf=buf.slice(i+1); if(!l)continue;
    let m; try{m=JSON.parse(l);}catch{continue;}
    if(m.method==="session/update"){ const u=m.params.update; st.updates.push(u.sessionUpdate);
      if(u.sessionUpdate==="agent_message_chunk") st.text+=u.content?.text||""; }
    else if(m.id!==undefined&&m.method){ child.stdin.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result:{outcome:{outcome:"selected",optionId:(m.params.options||[])[0]?.optionId}}})+"\n"); }
    else if(m.id!==undefined){ const p=pending.get(m.id); pending.delete(m.id); if(p) m.error?p.reject(new Error(JSON.stringify(m.error))):p.resolve(m.result); }
   }});
  const call=(method,params)=>{ const id=nextId++; return new Promise((res,rej)=>{pending.set(id,{resolve:res,reject:rej}); child.stdin.write(JSON.stringify({jsonrpc:"2.0",id,method,params})+"\n");}); };
  return {child,call,st};
}
const CAPS={protocolVersion:1,clientInfo:{name:"probe5",version:"0"},clientCapabilities:{fs:{readTextFile:false,writeTextFile:false},terminal:false}};
(async()=>{
 const a=connect(); await a.call("initialize",CAPS);
 const s=await a.call("session/new",{cwd:CWD,mcpServers:[]});
 await a.call("session/prompt",{sessionId:s.sessionId,prompt:[{type:"text",text:"Remember this token: MAJIANG-4412. Just say OK."}]});
 console.log("A-sessionId="+s.sessionId+" A-text="+a.st.text.trim().slice(0,60));
 a.child.kill();
 await new Promise(r=>setTimeout(r,1500));
 // fresh process
 const b=connect(); await b.call("initialize",CAPS);
 let listed=null; try{ const ls=await b.call("session/list",{cwd:CWD}); listed=(ls.sessions||[]).some(x=>x.sessionId===s.sessionId); }catch(e){ listed="ERR "+e.message.slice(0,120); }
 console.log("B-listed="+listed);
 const t0=Date.now();
 try{ await b.call("session/load",{sessionId:s.sessionId,cwd:CWD,mcpServers:[]}); console.log("B-loadMs="+(Date.now()-t0)+" replayKinds="+JSON.stringify([...new Set(b.st.updates)])); }
 catch(e){ console.log("B-loadError="+e.message.slice(0,200)); b.child.kill(); process.exit(1); }
 b.st.text="";
 const r=await b.call("session/prompt",{sessionId:s.sessionId,prompt:[{type:"text",text:"What token did I ask you to remember? Answer with the token only."}]});
 console.log("B-stop="+r.stopReason+" B-text="+b.st.text.trim().slice(0,120));
 b.child.kill(); process.exit(0);
})().catch(e=>{ console.log("FAILED "+e.message.slice(0,300)); process.exit(1); });
