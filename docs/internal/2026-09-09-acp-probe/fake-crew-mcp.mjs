// Minimal stdio MCP server exposing one tool, to prove ACP session/new.mcpServers works.
import { appendFileSync } from "node:fs";
const LOG = process.env.MCP_LOG || "/tmp/fake-mcp.log";
// Marker written at PROCESS START, before any stdin traffic. This separates
// "the agent spawned us but never spoke" from "the agent never spawned us" --
// the codex control proved those two look identical if you only log on stdin.
appendFileSync(LOG, "SPAWNED " + new Date().toISOString() + " envCount=" + Object.keys(process.env).length + " keys=" + Object.keys(process.env).sort().join(",").slice(0,300) + "\n");
let buf = "";
process.stdin.on("data", d => {
  buf += d.toString();
  let i;
  while ((i = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1);
    if (!line) continue;
    let m; try { m = JSON.parse(line); } catch { continue; }
    appendFileSync(LOG, "IN " + line + "\n");
    if (m.method === "initialize") reply(m.id, { protocolVersion: "2024-11-05", capabilities: { tools: {} }, serverInfo: { name: "fake-crew", version: "0.0.1" } });
    else if (m.method === "tools/list") reply(m.id, { tools: [{ name: "post_to_crew", description: "Post a message to the crew whiteboard.", inputSchema: { type: "object", properties: { message: { type: "string" } }, required: ["message"] } }] });
    else if (m.method === "tools/call") {
      appendFileSync(LOG, "CALLED " + JSON.stringify(m.params) + "\n");
      reply(m.id, { content: [{ type: "text", text: "已发到 crew 群聊白板。" }] });
    }
    else if (m.id !== undefined) reply(m.id, {});
  }
});
function reply(id, result) { const s = JSON.stringify({ jsonrpc: "2.0", id, result }); appendFileSync(LOG, "OUT " + s + "\n"); process.stdout.write(s + "\n"); }
