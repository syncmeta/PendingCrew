import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";
const AGENT = process.env.ACP_AGENT.split(" ");
const CWD = process.env.ACP_CWD;
const log = [];
const child = spawn(AGENT[0], AGENT.slice(1), { stdio: ["pipe","pipe","pipe"], cwd: CWD, env: process.env });
child.stderr.on("data", d => process.stderr.write("[stderr] " + d));
let nextId = 1; const pending = new Map(); let buf = "";
const updates = [];
child.stdout.on("data", c => { buf += c.toString(); let i;
  while ((i = buf.indexOf("\n")) >= 0) { const l = buf.slice(0,i).trim(); buf = buf.slice(i+1); if (!l) continue;
    let m; try { m = JSON.parse(l); } catch { continue; }
    log.push({ dir: "<-", m });
    if (m.method === "session/update") { updates.push(m.params.update); const u = m.params.update;
      if (u.sessionUpdate !== "agent_message_chunk" && u.sessionUpdate !== "agent_thought_chunk")
        console.error("UPD " + JSON.stringify(u).slice(0,300)); }
    else if (m.id !== undefined && m.method) handleReq(m);
    else if (m.id !== undefined) { const p = pending.get(m.id); pending.delete(m.id);
      if (p) m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result); }
  }});
function send(o){ log.push({dir:"->",m:o}); child.stdin.write(JSON.stringify(o)+"\n"); }
function call(method, params){ const id=nextId++; return new Promise((res,rej)=>{pending.set(id,{resolve:res,reject:rej}); send({jsonrpc:"2.0",id,method,params});}); }
function notify(method, params){ send({jsonrpc:"2.0",method,params}); }
const permissionsSeen = [];
function handleReq(m){
  console.error("AGENT-REQ " + m.method + " " + JSON.stringify(m.params).slice(0,500));
  if (m.method === "session/request_permission") {
    permissionsSeen.push(m.params);
    const opts = m.params.options || [];
    const pick = opts.find(o=>o.kind==="allow_once") || opts[0];
    send({jsonrpc:"2.0",id:m.id,result:{outcome:{outcome:"selected",optionId:pick?.optionId}}});
  } else send({jsonrpc:"2.0",id:m.id,error:{code:-32601,message:"unimplemented "+m.method}});
}
const out = {};
(async () => {
  out.init = await call("initialize", { protocolVersion: 1, clientInfo:{name:"probe2",version:"0"},
    clientCapabilities: { fs:{readTextFile:false,writeTextFile:false}, terminal:false }});

  // --- 1. MCP server injection ---
  const sess = await call("session/new", { cwd: CWD, mcpServers: [
    { name: "crew", command: process.execPath, args: [process.env.FAKE_MCP], env: [] }
  ]});
  out.sessionId = sess.sessionId;
  console.error("SESSION " + sess.sessionId);

  // --- 2. set config option (model/effort switch) ---
  try { out.setEffort = await call("session/set_config_option", { sessionId: sess.sessionId, configOptionId: "reasoning_effort", value: "low" }); }
  catch(e){ out.setEffortError = e.message; }
  // --- 3. set mode (read-only => should force approval on shell) ---
  try { out.setMode = await call("session/set_mode", { sessionId: sess.sessionId, modeId: "read-only" }); }
  catch(e){ out.setModeError = e.message; }

  // --- 4. prompt that must use the injected MCP tool ---
  const r1 = await call("session/prompt", { sessionId: sess.sessionId,
    prompt: [{type:"text",text:"Call the post_to_crew tool exactly once with message='ACP probe'. Then reply DONE."}]});
  out.prompt1 = r1;
  console.error("PROMPT1 " + JSON.stringify(r1.stopReason));

  // --- 5. prompt that must ask permission (read-only mode + shell) ---
  const r2 = await call("session/prompt", { sessionId: sess.sessionId,
    prompt: [{type:"text",text:"Run the shell command `echo hi > /tmp/acp-probe-write.txt` to create a file. You must actually run it."}]});
  out.prompt2 = r2;
  out.permissionsSeen = permissionsSeen;
  console.error("PROMPT2 " + JSON.stringify(r2.stopReason) + " permissions=" + permissionsSeen.length);

  // --- 6. cancel mid-turn ---
  const p3 = call("session/prompt", { sessionId: sess.sessionId,
    prompt: [{type:"text",text:"Count slowly from 1 to 500, writing each number on its own line."}]});
  await new Promise(r=>setTimeout(r,3000));
  notify("session/cancel", { sessionId: sess.sessionId });
  out.prompt3 = await p3;
  console.error("PROMPT3(cancel) " + JSON.stringify(out.prompt3.stopReason));

  out.updateKinds = [...new Set(updates.map(u=>u.sessionUpdate))];
  writeFileSync(process.env.OUT, JSON.stringify(out,null,1));
  writeFileSync(process.env.TRACE, JSON.stringify(log,null,1));
  child.kill(); process.exit(0);
})().catch(e=>{ console.error("FAILED "+e.message); out.error=e.message;
  writeFileSync(process.env.OUT, JSON.stringify(out,null,1));
  writeFileSync(process.env.TRACE, JSON.stringify(log,null,1)); child.kill(); process.exit(1); });
