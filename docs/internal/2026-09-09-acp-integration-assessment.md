# ACP（Agent Client Protocol）接入研判

<!-- doc-ref-base: 7408c09 -->

- 日期：2026-09-09
- 对应人类 Todo：**#135**（父 crew「PendingCrew」）
- 性质：**只读研判**。没改一行产品代码，没提交、没 push。本目录下另有 `2026-09-09-acp-probe/`（六个探针脚本，可复跑）。
- 基准提交：`7408c09`（本文引用的所有 `file:line` 都对着这棵树）

---

## 0. 一句话结论

**能接，而且比预想的近。** ACP 覆盖了我们 `SessionBackend` 十三项必答里的十一项；剩下两项一项是产品取舍（PTY 真终端消失），一项是各家适配器的私有口子（system prompt 注入：claude 适配器有、codex 适配器没有）。

本机实测跑通了**两条**完整回路：`codex-acp` 和 `claude-agent-acp`。后者是 Anthropic + Zed + JetBrains 共同署名的官方适配器 —— 也就是说，接了 ACP 之后，**我们现有的两条后端本身也都能改走 ACP**。

---

## 第一层：实测到的

全部命令在 macOS 25.6.0 / `node v26.3.1` / `codex-cli 0.153.4` 上跑的，探针脚本见 `2026-09-09-acp-probe/`。

### 1.1 装的是什么

```
$ npm ls -g --depth=0
/opt/homebrew/lib
├── @agentclientprotocol/codex-acp@1.3.0
...
```

`codex-acp` **没有**在 PATH 上留下 `codex-acp` 命令（`ls /opt/homebrew/bin/codex-acp` → No such file），要 `node /opt/homebrew/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js` 直接跑。

它自带的 `@openai/codex` 缺 arm64 原生产物，起不来：

```
Error: Missing optional dependency @openai/codex-darwin-arm64.
```

设 `CODEX_PATH=/Users/hey/.local/bin/codex` 指向本机真 codex 后正常。

### 1.2 最小回路：跑通了

`client.mjs` 是从零写的 ACP 客户端（约 90 行，只用 node 内置模块）。对 `codex-acp`：

| 步骤 | 结果 |
|---|---|
| `initialize` | 成功，`protocolVersion: 1` |
| `session/new` | 成功，拿到 sessionId |
| `session/prompt` | 成功，流式收到 `agent_message_chunk` "P"/"ONG" |
| 返回 | `stopReason: "end_turn"`，带完整 usage |

端到端 7752 ms。

`initialize` 回来的能力声明（原样）：

```json
{
 "agentCapabilities": {
  "loadSession": true,
  "promptCapabilities": { "embeddedContext": true, "image": true },
  "sessionCapabilities": { "resume": {}, "list": {}, "close": {},
                           "delete": {}, "additionalDirectories": {} },
  "mcpCapabilities": { "acp": false, "http": true, "sse": false }
 },
 "authMethods": [{ "id": "api-key", "name": "API Key", ... }]
}
```

`session/new` 回来的 `configOptions` id 列表：`mode` / `collaboration_mode` / `model` / `reasoning_effort` / `fast-mode`；`modes` 三档：`read-only` / `agent` / `agent-full-access`。

### 1.3 我们真正依赖的五件事，逐个实测

`probe2.mjs`，同一条 codex session 上连做：

| 我们的能力 | ACP 做法 | 实测结果 |
|---|---|---|
| 挂 crew MCP 工具 | `session/new.mcpServers` 传 stdio 配置 | ✅ 我写了个假 MCP server（`fake-crew-mcp.mjs`，只暴露一个 `post_to_crew`），codex **真的调到了它**：`/tmp/fake-mcp.log` 里有 `CALLED {"name":"post_to_crew","arguments":{"message":"ACP probe"}}` |
| 审批弹给我们 | agent 反向发 `session/request_permission` | ✅ 一轮里收到两次。一次是 MCP 工具调用审批（`_meta.is_mcp_tool_approval: true`），一次是 shell 命令审批，带 `rawInput.command` 和 `cwd`，四个选项 `allow_once` / `allow_session` / `allow_always` / `decline`，各带 `kind` |
| 审批放行后真的执行 | 回 `{outcome:{outcome:"selected",optionId}}` | ✅ `/tmp/acp-probe-write.txt` 内容为 `hi`，`tool_call_update` 报 `exit_code: 0` |
| 中断 | `session/cancel` 通知 | ✅ 发出后 `session/prompt` 立刻返回 `stopReason: "cancelled"` |
| 切审批模式 | `session/set_mode` | ✅ 返回 `{}` 成功 |

一轮里实际收到的 `session/update` 种类：`available_commands_update` / `session_info_update` / `agent_message_chunk` / `usage_update` / `tool_call` / `tool_call_update`。

**踩到一个坑**：`session/set_config_option` 的字段叫 `configId`，不叫 `configOptionId`。写错时服务端回的是结构化错误，不是静默忽略：

```
{"code":-32602,"message":"Invalid params",
 "data":{"configId":{"_errors":["Invalid input: expected string, received undefined"]}}}
```

### 1.4 跨进程恢复：跑通了，两家都行

`probe3.mjs` —— **换一个全新的 codex-acp 进程**，只拿着上一轮的 sessionId：

| 动作 | 结果 |
|---|---|
| `session/list` | 列出 2 条，含目标 session，带 `title` / `cwd` / `updatedAt` |
| `session/load` | **57 ms**，回放 13 条历史 update（含 `user_message_chunk` / `tool_call` / `tool_call_update`） |
| `session/set_config_option` (`reasoning_effort`→low) | 成功，返回整张 configOptions |
| `session/set_config_option` (`model`→gpt-5.6-luna) | 成功 |
| 追问上一轮内容 | 答对："You asked me to call the `post_to_crew` tool." |

`probe5.mjs` 对 `claude-agent-acp` 做同一件事（A 进程记住一个口令 → 杀掉 → B 进程 load → 追问）：

```
A-sessionId=0803118f-dee6-428b-8649-d5efb6078c40  A-text=OK. Token noted: MAJIANG-4412.
B-listed=true
B-loadMs=1211  replayKinds=["user_message_chunk","agent_message_chunk"]
B-stop=end_turn  B-text=MAJIANG-4412
```

### 1.5 claude 那家：官方适配器存在，也跑通了

`npx -y @agentclientprotocol/claude-agent-acp@0.75.1`，一句 prompt 端到端 4363 ms，`stopReason: end_turn`。

它的能力声明比 codex 那家还厚：

```json
"sessionCapabilities": { "additionalDirectories":{}, "close":{}, "delete":{},
                         "fork":{}, "list":{}, "resume":{}, "subagents":{} },
"mcpCapabilities": { "http": true, "sse": true },
"loadSession": true
```

`session/new` 的 `configOptions` id：**`mode` / `model` / `effort` / `fast` / `agent`** —— 和我们 `set_session_profile` 要调的两个旋钮（model / effort）一一对上。`modes` 五档：`default` / `acceptEdits` / `plan` / `auto` / `bypassPermissions`，默认 `auto` —— 正是我们启动参数里那个 `--permission-mode auto`。

**额度信号是结构化过线的**，`usage_update._meta`：

```json
"_claude/rateLimit": {
  "status": "allowed", "resetsAt": 1788932400, "rateLimitType": "five_hour",
  "unifiedWindows": {
    "five_hour": { "utilization": 0.35, "resetsAt": 1788932400 },
    "seven_day": { "utilization": 0.4,  "resetsAt": 1789268400 }
  }
}
```

外加一条自定义通知 `_auth/status_update`，直接给出账号档位：`{"kind":"account","label":"Claude Max","account":{"plan":"max",...}}`。

### 1.6 system prompt 注入：claude 有，codex 没有

`probe4.mjs`，`session/new` 带 `_meta.systemPrompt = {append: "…crew 口令是 XIANGQI-7788…"}`，然后问它口令：

```
STOP=end_turn
TEXT=XIANGQI-7788
```

**成立。** 对应实现在 `~/.npm/_npx/*/…/claude-agent-acp/dist/acp-agent.js:5838-5855`：默认 `{type:"preset",preset:"claude_code"}`，`params._meta.systemPrompt` 存在时把它的字段展开进去。

codex 那家**没有这个口子**。`codex-acp@1.3.0` 的整个 bundle 里能被客户端传进来的 `_meta` 只有一个：

```
$ grep -oE "_meta\??\.[A-Za-z_.?]+" dist/index.js | sort -u
_meta.gateway.protocol
```

我另外拉了 registry 上的最新版 `1.10.0` 核对（`npm pack @agentclientprotocol/codex-acp@1.10.0`）：**一样只有 `_meta.gateway.protocol`**。里面唯一那处 `developerInstructions` 是适配器自己 fork 线程做 file-change-report 用的，不是客户端入口。

### 1.7 生态盘子有多大

`https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`（`version: 1.0.0`，本机 2026-09-09 拉的）：**40 个 agent**。跟我们相关的几条原样：

| id | 版本 | 署名 | 怎么起 |
|---|---|---|---|
| `claude-acp` | 0.75.1 | Anthropic, Zed Industries, JetBrains | `npx @agentclientprotocol/claude-agent-acp` |
| `codex-acp` | 1.10.0 | OpenAI, JetBrains, Zed Industries | `npx @agentclientprotocol/codex-acp` |
| `gemini` | 0.59.0 | Google | `npx @google/gemini-cli --acp` |
| `github-copilot-cli` | 1.0.83 | GitHub | `npx @github/copilot --acp` |
| `cursor` | 2026.09.02 | Cursor | 下二进制，`cursor-agent acp` |
| `goose` | 1.50.0 | Block | 下二进制，`goose acp` |
| `opencode` | 1.18.29 | Anomaly | 下二进制，`opencode acp` |

registry 每条给的是 `distribution`（`npx` 包名 / 各平台二进制 URL + sha256 + cmd + args），**是一份可以直接拿来做「安装并起一个 agent」的机读清单**。

`opencode` 那条值得单独说：`LocalCodingAgentKind.swift:6` 的注释写着「spec v2 §8.1 砍掉了 Kilo Code / opencode（不熟、降低复杂度）」—— 当初因为「每接一家写一套」而砍掉的那家，现在在 registry 里。

---

## 第二层：读代码读到的

### 2.1 我们的后端协议实际要什么（这份清单是唯一标尺）

`Sources/Mac/LocalRunner/SessionBackend.swift`，十三项：

| # | 项 | 行 | 语义 |
|---|---|---|---|
| 1 | `status` | :62 | `.running` / `.exited(code)` |
| 2 | `isBusy` | :76 | 唤醒注入门禁。codex=`activeTurnId != nil`；claude 恒 `false` |
| 3 | `isWorking` | :81 | UI 状态点。claude 是「最近 ~1s 还在吐 PTY 字节」的近似 |
| 4 | `displayIsTyping` | :90 | 群聊打字气泡，claude 侧要防抖 |
| 5 | `health` | :95 | 七种（见 2.2） |
| 6 | `pendingDecision` | :106 | 「卡在终端选择菜单」 |
| 7 | `kind` | :108 | claude_code / codex / terminal |
| 8 | `hasObservedLaunchSignal` | :125 | **协议必答项，故意不给默认实现** |
| 9 | `send` | :126 | fire-and-forget |
| 10 | `submitWake` | :129 | **有回执**的提交（accepted / retry） |
| 11 | `interrupt` | :130 | |
| 12 | `stop` | :131 | |
| 13 | `clearQuotaHealth` / `applyProfileSwitch` | :134 / :140 | 额度恢复重新武装 / 中途切 model·effort |

外加编排层要的（不在协议里，但接一家新 harness 一样要）：

| 项 | 在哪 | claude 怎么做 | codex 怎么做 |
|---|---|---|---|
| 世界观 system prompt | `SessionConfig.swift:149` | `--append-system-prompt-file` | `thread/start.developerInstructions`（`CodexProtocol.swift:40-61`） |
| crew MCP 工具 | `LocalSessionLaunch.swift:86-93` | `--mcp-config <file>` | `config.mcp_servers`（`CodexProtocol.swift:33-46`） |
| 每轮注入未读白板 | `LocalSessionLaunch.swift:118` | `PostToolUse` hook 起 helper 子进程 | `turn/start.additionalContext`（`CodexProtocol.swift:107-134`） |
| 审批门 | `LocalSessionLaunch.swift:119` | `PreToolUse` hook | `*/requestApproval` server-request（`CodexProtocol.swift:173-180`） |
| 一轮结束留痕 | `LocalSessionLaunch.swift:120` | `Stop` hook | `turn/completed` |
| 续跑同一会话 | `SessionConfig.swift:143-147` | `--resume <uuid>` / `--session-id <uuid>` | `thread/resume`（`CodexProtocol.swift:63-89`） |
| 额度快照 | `AgentQuota.swift` | 读 `~/.claude.json` 等本地文件 + 文本解析人话时刻 | 读 codex 本地 rateLimits |
| 模型表 | `AgentModelCatalog.swift:10` | `claude -p "/model"` 回显解析 | `model/list` |
| 裸按键注入 | `AgentTerminalSession.swift:52` | `sendRaw`，机长 `nudge_session` 按菜单用 | 无（codex 恒 `pendingDecision == nil`） |

### 2.2 七种 health（`SessionHealth.swift:14-43`）

`authRequired` / `usageLimit` / `rateLimited` / `launchFailed` / `cliVersionIncompatible` / `turnFailed` / `briefUndelivered`。

claude 侧全部靠**扫 PTY 文本**（`AgentSessionCore.swift:435` `healthScanner.feed`、`:436` `rateLimitScanner`）；codex 侧靠协议字段（`CodexProtocol.swift:203-291`）。

### 2.3 抽象是真的，而且已经有五个实现

`grep -l ": SessionBackend"`：

- `AgentTerminalSession.swift`（claude，PTY 门面）
- `CodexAppServer/CodexAppServerBackend.swift`（codex，app-server）
- `HeadlessSessionBackend.swift`（daemon 里那份）
- `RemoteSessionBackend.swift`（app 当 viewer 时那份）
- `PlainTerminalSession.swift`（纯 shell，不接编排）

### 2.4 「这件事已经做了一半」——落在哪

`docs/internal/2026-08-19-backend-split-design.md`：

- `:50` —— 「**结论：判断成立。**『搬出去 = 再加第三个实现』这条路是通的。上层编排（`CrewSessionRunner`）只通过这个协议 + `CrewSessionRun` 的 `@Published` 看 session。」
- `:203` —— 「app 侧的 `RemoteSessionBackend` 实现 `SessionBackend`：收到 delta → 更新自己的 `@Published` → 上层编排（如果那时还有的话）和 UI 一行不用改。**这就是父机长预言的『第三个实现』，字面成立。**」
- `:197` —— attach 的内容恢复**已经按 backend 分流**：终端型发 PTY 快照片段，codex 型发结构化 `CodexTranscript` 历史，靠 `hello.capabilities` 里的 `transcript-events` 协商，缺能力降级为空历史、不拒连。

**换句话说：「一个不是 PTY、只有结构化事件的后端」这条路，daemon 那侧已经为 codex 铺过一遍了。**ACP 后端走的是同一条路，不是新路。

---

## 第三层：逐条比对（推出来的）

标尺是 2.1 那十三项 + 九项编排要求。

### 3.1 直接对得上（15 条）

| 我们的 | ACP 的 | 依据 |
|---|---|---|
| `status` | 子进程存活 + `session/close` | 实测 |
| `isBusy` | `session/prompt` 请求在飞 = busy | 实测（请求-响应形态） |
| `isWorking` | 同上，另有 `session_info_update._meta.codex.threadStatus` | 实测 |
| `hasObservedLaunchSignal` | 收到 `initialize` 响应 | 实测 |
| `send` / `submitWake` | `session/prompt`，**天生有回执**（返回 stopReason） | 实测 |
| `interrupt` | `session/cancel` → `stopReason: cancelled` | 实测 |
| `applyProfileSwitch` | `session/set_config_option`（`model` / `effort`） | 实测（两家 configOptions 都有） |
| 审批门 | `session/request_permission` | 实测 |
| `pendingDecision` | 同上 —— **结构化，不用扫屏** | 实测 |
| crew MCP 工具 | `session/new.mcpServers`（stdio 全体 agent MUST 支持） | 实测 |
| 续跑同一会话 | `session/load` + `session/list` + `session/resume` | 实测（冷进程两家都过） |
| 每轮注入白板 | prompt 是 ContentBlock 数组，前置一块即可 | 推（形状成立，未实测语义等价） |
| 模型表 | `session/new` 回的 `configOptions[model].options` | 实测 |
| 一轮结束留痕 | `stopReason` + 本轮最后一条 `agent_message_chunk` | 实测 |
| 起会话 / cwd | `session/new.cwd` + `additionalDirectories` | 实测 |

### 3.2 「ACP 有但我们没用过」（3 条）

- **额度**：claude 适配器把 5 小时窗 / 7 天窗的 `utilization` 和 `resetsAt` 结构化过线（1.5）。我们现在是读本地文件 + 解析 `"Jul 5 at 4:39am (Asia/Shanghai)"` 这种人话时刻（`AgentQuota.swift:28-29`）。**这是升级，不是缺口。** 但 codex 适配器只给 token 计数（`_meta.quota.token_count`），**没有**订阅窗口百分比 —— 两家不对称。
- **usage_update**：上下文窗口占用（`used` / `size`）逐轮过线。我们现在没有这个信号。
- **session/fork**：claude 适配器声明支持。我们没有对应概念。

### 3.3 缺的（4 条，按要紧程度排）

**① system prompt 注入没有统一入口 —— 这条最要紧。**

ACP v1 的 `NewSessionRequest` 只有 `cwd` / `mcpServers` / `additionalDirectories` / `_meta`（schema 实测）。世界观注入**不在 spec 里**，是各家适配器自己在 `_meta` 里开的私有口子：claude 有（`_meta.systemPrompt`，实测生效），codex **没有**（1.3.0 和 1.10.0 都核过）。

性质是「**ACP 没有**」，不是「有但我们没用过」。落到 codex-over-ACP 上，世界观只能退化成「拼进第一条 prompt 正文」——比 `developerInstructions` 弱：它会进对话历史、会被压缩、agent 可能把它当用户说的话。

**② PTY 真终端消失 —— 但这是产品取舍，不是协议缺陷。**

ACP 给的是结构化事件流，不是字节流。claude 那条腿现在的可选中复制 / 回滚缓冲 / 改宽度重排（backend-split-design §1.1 A2 明确要求「不退化」）在 ACP 上不存在，只能退化成 codex 那样的结构化 transcript。

连带没了的：`sendRaw` 裸按键注入（`AgentTerminalSession.swift:52`）—— 机长 `nudge_session` 替 worker 按菜单那条路。不过**替代品更好**：菜单本来就变成了 `session/request_permission`，有结构化选项列表，不用再看 `❯` 停在哪一项。

**③ 七种 health 里三种没有直接对应。**

`launchFailed` 可以由「`initialize` 超时/失败」推出来；`turnFailed` 由 `stopReason` 推；`authRequired` 有 `authMethods` + `_auth/status_update`（claude）/ `auth_required` 错误。但 `cliVersionIncompatible` / `briefUndelivered` / `rateLimited` 三种没有协议位置 —— 前两种是我们自己造的诊断，后一种依赖各家 `_meta`。性质是「ACP 没有」，但这三种本来就是我们从别人的故障里自己长出来的，换个后端要重新长一次。

**④ 多出来一层进程。**

`codex-acp` 是「起 codex app-server 再翻译」，不是 codex 本身。多一层 node 进程、多一层版本漂移面。实测已经踩到两次：PATH 上没有 `codex-acp` 命令、自带 codex 缺原生产物。

### 3.4 我们的 `SessionBackend` 抽象够不够

**够，不用改形状。** 十三项里没有一项是「只有 PTY 才答得出」的：

- `pendingDecision` 有默认实现（`SessionBackend.swift:152-155` 的 extension，「默认没有终端菜单这回事」）——codex 已经走这条路了。
- `displayIsTyping` 有默认实现（:156 直接复用 `isWorking`）。
- `clearQuotaHealth` / `applyProfileSwitch` 都有默认实现。
- 唯一的必答项 `hasObservedLaunchSignal`（:125，注释明确写了「故意不给默认实现」「下一种后端不回答『我起来没有』就编不过」）—— ACP 后端答得出：收到 `initialize` 响应。

**真正要改形状的不是 backend 协议，是它周围三处：**

1. **`LocalCodingAgentKind`**（`LocalCodingAgentKind.swift`）是个封闭三值枚举，而且它的 rawValue **落盘**（`serverRunnerKind` 映射到 `crew_sessions.runner_kind`）。接「任意 harness」意味着它要从枚举变成「枚举 + 一个带 agent id 的 acp case」，那是一次存储层改动。
2. **`inferred(fromDisplayName:)`**（同文件 :63）靠 `displayName` 前缀反推 kind，用于 @ 唤醒已退出成员。ACP agent 的显示名来自 registry，不是我们定的常量 —— 这条推断会失效。
3. **daemon 的 attach 分流**（backend-split-design `:197`）现在是二值（PTY 快照 / codex 结构化历史）。ACP 后端要么复用 codex 那条（结构化事件），要么再加一档。复用是可能的，但 `CodexTranscript` 的 item 模型是照 codex 的 `thread/item` 长的，不是照 ACP 的 `SessionUpdate` 长的。

---

## 边界（我没看的 / 不确定的 / 只在某种条件下成立的）

写下来的每一条都是我知道自己没做到的地方。

1. **版本**：我实测的是本机装着的 `codex-acp@1.3.0`，registry 上最新是 `1.10.0`。1.10.0 我只做了一件事 —— 拉 tarball grep `_meta` 键和 `developerInstructions`，**没有跑过它**。1.3.0 上的每一条实测结论对 1.10.0 都未经验证。
2. **claude 那家我测得比 codex 浅**：`claude-agent-acp` 只测了最小回路、system prompt append、跨进程 load。**没测**它的 MCP 注入、审批请求、cancel、set_config_option —— 那四件我只有它 `initialize` 里的能力声明，那是**它自己说的**，不是我量到的。
3. **只测了两家适配器**。gemini / copilot / cursor / goose / opencode 一个都没跑。registry 那 40 家里有多少真能过我们这份清单，**完全没有读数**。ACP 的承诺是「接一次接上所有」，我验证的是「接一次接上两个」。
4. **每轮白板注入没实测**。我说「prompt 是 ContentBlock 数组、前置一块即可」是从 schema 推的形状，没有跑过「注入的白板被当成数据而不是指令」这件事。codex 现在那条路（`additionalContext` + `kind:"untrusted"` 包 `<external_*>`）的注入卫生，ACP 上**没有对应机制**，这一条我只知道形状对得上，不知道语义等不等价。
5. **`session/set_mode` 之后的持久性存疑**：probe2 里我把 codex session 设成 `read-only`，probe3 冷进程 load 回来后 `configOptions[mode].currentValue` 是 `agent`。是没持久化、还是 load 重置、还是我读错了 —— **没查**。
6. **审批的「持久选项」没测**：`allow_session` / `allow_always` 我一次都没选过，只选了 `allow_once`。我们现在的审批卡片是有意只做 allow/deny 两档（`CodexProtocol.swift:184-189` 明说了理由），ACP 给了四档，这个差异怎么落到 UI 上没研究。
7. **没测多 session 并发**。我们一台机器同时跑十几个 session，每个 ACP 后端是一个独立 node 进程 + 一个真 agent 进程。资源占用、句柄数、启动风暴 —— 没量。
8. **没测 daemon 集成**。所有实测都是我自己写的一次性客户端，**没有**任何一行走过 `SessionBackend` / `SessionProtocol` / daemon。「接进去要动哪儿」那一节是读代码推的，不是搭出来验的。
9. **`_auth/status_update` 是私有方法**（下划线开头），不在 spec 的方法表里。我把它算作「claude 适配器的额度信号」，但它随时可能变。
10. **没读 ACP v2**。SDK 里带着 `schema/v2/schema.unstable.json`，官方文档索引显示 v2 砍掉了 File System 和 Terminals、把 Prompt Turn 换成 Prompt Lifecycle。**我全程按 v1 做的**，v2 的迁移成本没估。
11. **本文引用的行号对着 `7408c09`**。这棵树上 main 常年领先 origin，行号会漂。

---

## 建议（只建立在上面的读数上）

不是方案，是三条从读数直接推出来的判断：

1. **接 ACP 的价值不在「多接几家」，在「把 claude 那条腿从扫屏改成读协议」。** 这是全部读数里最硬的一条：claude 现在的 health 全靠扫 PTY 文本、菜单靠认屏、额度靠解析人话时刻、模型表靠 `-p "/model"` 回显解析。ACP 上这四件全是结构化字段，claude 官方适配器已经在提供了。「接第三家」是顺带的。

2. **第一步该是把 ACP 当成第六个 `SessionBackend` 实现试出来，而不是替换任何一条现有腿。** 依据：抽象够（3.4）、daemon 的非 PTY 分流已经为 codex 铺过（2.4）、两条回路都实测跑通（1.2 / 1.5）。风险集中在 `LocalCodingAgentKind` 那次存储层改动，先不动它、写死一个 kind 试，能把风险推后。

3. **世界观注入那条缺口要先有答案，别等接到一半才发现。** 它是唯一一条「ACP 真的没有」而且我们真的依赖的东西 —— 每个 session 的世界观就是这套编排的地基。claude 适配器给了私有口子，codex 适配器没给。这意味着「接一次、接上所有」在这一项上**不成立**：每接一家都要单独问它「你的 system prompt 口子在哪」。这一条值得在动手前先向上游问清楚（ACP 有没有把它标准化的 RFD）。

---

## 复跑

```
cd docs/internal/2026-09-09-acp-probe
mkdir -p /tmp/acp-work
CODEX_PATH=$(command -v codex) NO_BROWSER=1 ACP_CWD=/tmp/acp-work ACP_LOG=/tmp/t.json \
  node client.mjs node /opt/homebrew/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js

ACP_CWD=/tmp/acp-work ACP_LOG=/tmp/t2.json \
  node client.mjs npx -y @agentclientprotocol/claude-agent-acp@0.75.1
```

`probe2.mjs`（MCP+审批+中断）/ `probe3.mjs`（冷进程恢复）/ `probe4.mjs`（system prompt）/ `probe5.mjs`（跨进程记忆）各自要的环境变量写在脚本头几行。
