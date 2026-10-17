// Minimal ACP client: spawn an ACP agent over stdio, initialize, new session, prompt.
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";

const AGENT_CMD = process.argv[2];
const AGENT_ARGS = process.argv.slice(3);
const CWD = process.env.ACP_CWD || process.cwd();
const PROMPT = process.env.ACP_PROMPT || "Reply with exactly: PONG. Do not use any tools.";

const log = [];
function rec(dir, obj) {
  log.push({ t: Date.now(), dir, obj });
  console.error(`${dir} ${JSON.stringify(obj).slice(0, 400)}`);
}

const child = spawn(AGENT_CMD, AGENT_ARGS, { stdio: ["pipe", "pipe", "pipe"], cwd: CWD, env: process.env });
child.stderr.on("data", d => process.stderr.write("[agent stderr] " + d));
child.on("exit", (c, s) => console.error(`[agent exit] code=${c} sig=${s}`));

let nextId = 1;
const pending = new Map();
let buf = "";
child.stdout.on("data", chunk => {
  buf += chunk.toString();
  let i;
  while ((i = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, i).trim();
    buf = buf.slice(i + 1);
    if (!line) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { console.error("[unparsed] " + line.slice(0,200)); continue; }
    rec("<-", msg);
    if (msg.id !== undefined && msg.method) { handleAgentRequest(msg); }
    else if (msg.id !== undefined) {
      const p = pending.get(msg.id); pending.delete(msg.id);
      if (p) msg.error ? p.reject(new Error(JSON.stringify(msg.error))) : p.resolve(msg.result);
    }
  }
});

function send(obj) { rec("->", obj); child.stdin.write(JSON.stringify(obj) + "\n"); }
function call(method, params) {
  const id = nextId++;
  return new Promise((resolve, reject) => { pending.set(id, { resolve, reject }); send({ jsonrpc: "2.0", id, method, params }); });
}
function notify(method, params) { send({ jsonrpc: "2.0", method, params }); }

// Client side of the protocol: we advertise nothing, so the agent must handle
// fs/terminal itself. We DO answer permission requests (allow once).
function handleAgentRequest(msg) {
  if (msg.method === "session/request_permission") {
    const opts = msg.params?.options || [];
    const pick = opts.find(o => o.kind === "allow_once") || opts.find(o => String(o.kind||"").startsWith("allow")) || opts[0];
    send({ jsonrpc: "2.0", id: msg.id, result: { outcome: { outcome: "selected", optionId: pick?.optionId } } });
  } else {
    send({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "client does not implement " + msg.method } });
  }
}

const updates = [];

(async () => {
  const init = await call("initialize", {
    protocolVersion: 1,
    clientInfo: { name: "pendingcrew-probe", version: "0.0.1" },
    clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
  });
  console.error("=== INITIALIZE RESULT ===\n" + JSON.stringify(init, null, 2));

  const sess = await call("session/new", { cwd: CWD, mcpServers: [] });
  console.error("=== NEW SESSION ===\n" + JSON.stringify(sess, null, 2));

  const t0 = Date.now();
  const promptP = call("session/prompt", {
    sessionId: sess.sessionId,
    prompt: [{ type: "text", text: PROMPT }],
  });
  const res = await promptP;
  console.error(`=== PROMPT RESULT (${Date.now()-t0}ms) ===\n` + JSON.stringify(res, null, 2));

  writeFileSync(process.env.ACP_LOG || "/tmp/acp-trace.json", JSON.stringify(log, null, 1));
  child.kill();
  process.exit(0);
})().catch(e => {
  console.error("FAILED: " + e.message);
  writeFileSync(process.env.ACP_LOG || "/tmp/acp-trace.json", JSON.stringify(log, null, 1));
  child.kill();
  process.exit(1);
});
