# ACP 第三条腿 · 实现方案（待评审）

<!-- doc-ref-base: 7408c09 -->

- 日期：2026-09-09
- 人类拍板：**A** —— 原话「claude 和 codex 留现在的，ACP 是额外扩展接其它的用」
- 前置读数：`docs/internal/2026-09-09-acp-integration-assessment.md`（本文不重复它的内容）
- 状态：**方案，未动实现**。本文写作期间只跑了只读探针，没改产品代码。

---

## 0. 这份方案回答什么

机长要的是「先接哪一家、为什么是它」。

上一份报告的边界第 3 条写着：「registry 那 40 家里有多少真能过我们这份清单，**完全没有读数**」。**不补上这条读数就选一家，等于闭着眼选。** 所以做方案前我先补了一轮横向对照 —— 那是第 1 节，也是本文最要紧的一节：**它把候选名单从「40 家」砍到了「2 家能当第一家、1 家已出局」。**

---

## 1. 新增读数：六家横向对照（实测）

探针脚本在 `2026-09-09-acp-probe/`，本轮新增 `probe6.mjs`（专测 cancel）、`probe7.mjs`（专测 MCP 挂载，带对照组）、`probe8.mjs`（**普查器**：只做 `initialize` + `session/new`，不发 prompt，零 token）。

**这张表是第二版。** 第一版只有五家、没有 opencode，而且 §1.3 记的两处尺子缺陷当时还没被发现 —— 那两处会让好的 agent 读成坏的。表里每一格都是用修好之后的尺子重跑的。

| | codex-acp 1.3.0 | claude-agent-acp 0.75.1 | **opencode 1.18.29** | **github copilot 1.0.83** | gemini-cli 0.46.0 | qwen 0.0.1-alpha.8 |
|---|---|---|---|---|---|---|
| 我们已有原生腿？ | 有 | 有 | **没有（真·新家）** | **没有（真·新家）** | 没有 | 没有 |
| 起法 | `node …/codex-acp/dist/index.js` | `npx …/claude-agent-acp` | `./opencode acp` | `npx @github/copilot --acp` | `gemini --experimental-acp` | — |
| `initialize` | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ **没有 acp 参数** |
| `session/new` | ✅ | ✅ | ✅ | ✅ | ❌ **缺 API key** | — |
| `session/prompt` | ✅ | ✅ | ✅ 4695 ms | ✅ 4640 ms | — | — |
| **挂我们的 MCP 工具** | ✅ 建会话即挂 | ✅ 建会话即挂 | ✅ **建会话即挂** | ❌ **静默不挂** | — | — |
| 审批过线 | ✅ | 未测 | 未测 | ✅ | — | — |
| cancel → `cancelled` | ✅ | 未测 | ✅ **31 段流后停住** | ❌ **生效但报 `end_turn`** | — | — |
| model / effort 可切 | ✅ 两个都有 | ✅ 两个都有 | ⚠️ 只有 `model`，无 `effort` | ❌ 只有 `mode` / `allow_all` | — | — |
| 冷进程恢复 | ✅ 57 ms | ✅ 1211 ms | 声明 `resume` / `fork` / `list`（未实跑） | 只声明 `list` / `close`，**无 `resume`** | 无 `sessionCapabilities` | — |
| 世界观口子 | ❌ 没有 | ✅ `_meta.systemPrompt` | ❌ **没有**（塞了口令，它答"没有定义"） | 未测 | — | — |

### 1.1 copilot 那条 ❌ 是这轮最重要的读数，而且做了对照

我先怀疑是我自己的请求形状写错了，所以做了对照组（`probe7.mjs`，同一句 prompt 跑两遍）：

| 形状 | 我们的 server 被 spawn 了吗 | agent 自报的工具表里有 `post_to_crew` 吗 |
|---|---|---|
| A：spec 原样（`{name, command, args, env}`，stdio 变体不带 `type`） | **NO** | 没有 |
| B：额外加 `type: "stdio"` | **NO** | 没有 |

判据是我们这侧的：假 MCP server 一被 spawn 就往 `/tmp/fake-mcp.log` 写字，两次跑完那个文件都不存在。

同一份形状 A 在 codex-acp 上**真的把工具调起来了**（上一份报告 1.3 节，日志里有 `CALLED {"name":"post_to_crew"…}`）—— 所以这不是我的形状问题。

copilot 本身是有 MCP 的（它自报的工具表里有一串 `github-mcp-server-*`），它只是**不接受客户端从 ACP 传进来的**。schema 里 stdio 那一档写着「All Agents MUST support this transport」，`session/new` 也没报错 —— 它收下了，然后什么都没做。

**这一条直接把 copilot 判出局**：crew 工具是一个 session 的神经系统，挂不上就发不了群、读不了白板、`ask` 不了机长。**一个不能 `post_to_crew` 的 session 不是 crew 成员。**

### 1.2 opencode：一家真·新 harness，几乎全过

它是**唯一一家不是我们已有腿、又通过了全部关键项的**：建会话即挂上我们的 MCP 工具、真跑完一轮、cancel 语义正确（流了 31 段然后停住、返回 `cancelled`）、声明了 `resume` / `fork` / `list`、有 `model` 配置项。

两处缺：**没有 `effort` 旋钮**（只有 `model`）；**没有世界观口子**（塞了口令去问，它答「There is no crew codeword defined in this workspace.」—— 和 codex 一样）。

值得记一笔：`LocalCodingAgentKind.swift:6` 那条注释写着当初「不熟、降低复杂度」砍掉的正是 opencode。**现在有读数了，"不熟"这个理由不再成立。**

### 1.3 两处尺子缺陷 —— 它们会把好的 agent 读成坏的

普查器写完后我先拿 codex（已知能挂）当对照跑，连着读出三次 `false`。**三次都是我的尺子坏了，不是 agent 坏了。** 两个根因，两个都是接进产品时会原样再犯一次的：

**① ACP 的 `mcpServers[].env` 是那个进程的全部环境，父进程的环境不透传。** codex 起我们的 MCP server 时只给了 10 个变量：`HOME LANG LOGNAME MCP_LOG PATH SHELL TERM TMPDIR USER __CF_USER_TEXT_ENCODING` —— 其中 `MCP_LOG` 是我在 `env` 数组里显式声明的，不声明就没有。我原先把标记路径放在环境变量里传，于是假 server 起来了、却把标记写去了默认路径，探针盯着一个永远不会出现的文件。

**→ 落到实现上：crew helper 需要的一切必须走 argv 或显式 `env` 数组，不能指望继承。** 现在 `LocalSessionLaunch.swift:86-93` 那条 codex 路径是走 dict 传的，ACP 这条要重新过一遍这个清单。

**② 普查器必须自动放行审批请求，否则每一家都读成"挂不上"。** codex 把 MCP 工具调用挡在一次 `session/request_permission` 后面。我第一版探针对所有反向请求一律回错误 —— 于是审批被拒、工具没跑、server 没起、读数 `false`。

**→ 落到实现上：这就是「一个从不发声的检查」的形状。** 一个永远读 false 的挂载检测，和一个真的挂不上的 agent，长得一模一样。所以 §4 第 2 条那个检测器本身也必须先被证明会绿（拿 codex / claude / opencode），再被证明会红（拿 copilot）—— 只做后一半就是在信一把没校准过的尺子。

### 1.4 另外三条一起看，得出一个不在原问题里的结论

- gemini：ACP 那层是好的（`initialize` 回了完整能力声明），倒在**它自己的登录态**上 —— 本机有 `~/.gemini` 但 ACP 那条路要 API key。
- qwen：本机装的版本压根没有 ACP 参数（registry 上那条是新版）。
- copilot：`session/set_mode` 我传 `read-only` 被拒，因为它的 mode id 是 **URL 形状**（`https://agentclientprotocol.com/protocol/session-modes#agent`）。这条是我写错，但它教了一件事：**mode id 不可移植**，不能跨 agent 硬编码。

合起来：**接一家新 harness 的第一道门不是 ACP，是它自己的登录态和版本。** ACP 让「怎么说话」统一了，没让「能不能开口」统一。

---

## 2. 先接哪一家：**codex-acp**。理由四条，都建立在上面的读数上

我知道这看起来像在绕开题目 —— 人类要的是接**别家**。所以先把话说死：**这一步产品价值是零，它是台架，不是产品。** 产品价值从第二家开始。

选它的四条理由：

1. **它是唯一一家我把全部必答项都实测过的。** 机长的口径是「挑你实测过的」。实测过的只有两家，另一家（claude-agent-acp）测了 4 件（最小回路 / 世界观注入 / 跨进程恢复 / MCP 挂载），**审批、中断、切配置、切 mode 这四件只有它自己的能力声明** —— 那是它说的，不是我量到的。
2. **它给一把参照尺。** 同一个底层 agent、两条后端：`CodexAppServerBackend`（原生，已上线、真机验过）和新写的 ACP 后端。任何行为差异**必定是我新后端的 bug**，不可能是 agent 的。**其它任何一家都给不了这把尺。** 这一条最值钱 —— 机长的口径 1 是「要能证明它真的在干活，而不是握手成功」，而有参照系才谈得上证明。
3. **它恰好是没有世界观口子的那家。** 于是「这家接不了世界观，明确报出来」这条路第一天就被走过，而不是留成一句写在文档里的假设。挑一家有口子的当第一家，等于把最容易出错的那条路推到以后。
4. **它不会被误当成产品。** codex 已经有原生腿，ACP 那条不进 UI 选项、不进 `LocalCodingAgentKind`，跑完即拆。这符合「别一次铺开」。

**第二家：opencode。现在就定得了，因为读数有了。**

写这份方案的头一版时我写的是「第二家不现在定，读数不够」。补完普查之后读数够了：opencode 是唯一一家不是我们已有腿、又通过全部关键项的（§1.2）。它还正好是我们自己当初以「不熟」为由砍掉的那家 —— 那个理由现在被读数取代了。

**它跟 codex 缺的是同一样东西（世界观口子），这反而让排序更干净**：R0–R2 用 codex 把台架和那条「接不了世界观」的路走通，R3 换 opencode 时那条路已经是走过的，新出现的问题就一定是「换了一家」带来的，不会跟「第一次遇到这个缺口」混在一起。

**排序不变的理由**：opencode 没有参照尺。同一句 prompt 我没有第二条已知正确的后端去比它。第一家必须是有尺的那家。

---

## 3. 要造的东西（形状，不是代码）

### 3.1 照着 codex 那条腿的孪生写，别发明第二种

仓库里已经有一条「非 PTY、结构化事件」的后端，形状是现成的。新东西逐个对位：

| 新文件 | 对位的已有文件 | 干什么 |
|---|---|---|
| `ACPProtocol.swift` | `CodexAppServer/CodexProtocol.swift` | 纯值类型的 params builder + 响应/通知分类。Foundation-only、可单测、不碰进程 |
| `ACPConnection.swift` | `CodexAppServer/CodexAppServerConnection.swift` | actor，行分隔 JSON-RPC over stdio，请求/响应配对 + 反向请求回调 |
| `ACPTranscript.swift` | `CodexAppServer/CodexTranscript.swift` | 把 `session/update` 折成可渲染条目 |
| `ACPSessionBackend.swift` | `CodexAppServer/CodexAppServerBackend.swift` | **第六个 `SessionBackend` 实现** |

`SessionBackend` 协议**一个字不改**（上一份报告 3.4 已核过十三项必答里没有只有 PTY 答得出的）。

### 3.2 一家 harness = 一份声明，不是一段 if

新增一个纯值类型 `ACPAgentSpec`：怎么起（可执行 + argv + 环境变量）、世界观口子在哪（`_meta` 路径，或者**明确的"没有"**）、mode id 怎么给。

**关键约束：`ACPAgentSpec` 里不许出现 `if agent == "copilot"` 这种分支。** 一家 harness 的差异全部是这个值里的字段，不是代码里的判断 —— 否则第三家进来时又是一次改代码，「接一次」的口号就在实现里复活了。

### 3.3 不进 `LocalCodingAgentKind`

那个枚举的 rawValue **落盘**（映射到 `crew_sessions.runner_kind`），而且 `inferred(fromDisplayName:)` 靠显示名前缀反推 kind。第一家不碰它 —— 台架期用一个写死的 kind，把存储层改动整个推到第二家。

### 3.4 世界观注入：三态，不是两态

`ACPAgentSpec` 里这一项有三种取值，**对应三种不同的对外说法**：

| 取值 | 行为 |
|---|---|
| 有口子（如 claude 的 `_meta.systemPrompt`） | 正常注入 |
| **明确没有**（如 codex-acp） | 起会话时**在群里说清「这家接不了世界观，这个 session 没有它」**，session 照常起 |
| 没问过 | **拒绝起 session**。不许「没填就当没有」—— 那是把「我不知道」记成「我知道没有」 |

第三态是有意的：`LocalCodingAgentKind.swift:6` 那条注释记着上一次为了「不熟」砍掉两家；「不熟」是个该被说出来的状态，不该被默认值吞掉。

### 3.5 能力体检：起会话后对账，缺的明说

`initialize` + `session/new` 的响应里带着这家 agent 的能力声明。起会话后立刻拿它跟我们那份必答清单对账，**逐条**记下「有 / 没有 / 它没说」，缺的当场进群。

从第 1 节的读数直接推出来的三条必查：

- 没有 `model` / `effort` configOption（copilot 就是）→ `applyProfileSwitch` 必须返回明确失败，不能返回 `.applied`
- 没有 `sessionCapabilities.resume`（copilot 就是）→ 重启接不回原会话，起 session 时就要说，不能等重启那天才发现
- cancel 后回 `end_turn` 不回 `cancelled`（copilot 就是）→ 见 §4 第 5 条

这个体检器就是普查工具：跑一家新 harness 时先跑它，输出直接填进 `ACPAgentSpec`。

---

## 4. 先造红：验收怎么定，每条先证明尺子会红

机长的硬要求是「先造红」。做法：写一个**可编程的假 ACP agent**（一个 stdio 脚本，行为由参数控制），每条判据先让它坏给我看，再让真的过。

| # | 判据 | 怎么先造红 |
|---|---|---|
| 1 | 握手不回 / 进程立刻死 → `launchFailed`，不是「空闲」 | 假 agent：收到 `initialize` 不回；另一档：直接 exit 1 |
| 2 | **crew 工具真的挂上了**，不是「session/new 成功了」 | 假 agent：收下 `mcpServers` 但不 spawn（**这就是 copilot 的真实行为**）→ 断言我们报出来 |
| 3 | 没有 model/effort 旋钮时 `applyProfileSwitch` 明确失败 | 假 agent：`configOptions` 里不给 `model` |
| 4 | 没有世界观口子时明确报「接不了」，不静默当接上了 | 假 agent：忽略 `_meta.systemPrompt` |
| 5 | **我们发过 cancel 就判 `userStopped`**，不看 agent 报的 stopReason | 假 agent：cancel 后回 `end_turn`（**copilot 的真实行为**）→ 断言仍分类成 `userStopped` |
| 6 | 连接中途断掉 → 明确异常，不是静默空闲 | 假 agent：跑到一半关 stdout |
| 7 | 能力声明缺字段 → 记「它没说」，不记「它没有」 | 假 agent：`initialize` 只回最小必填 |

第 2 条是这一节的核心，也是机长口径 1 的落点。**难点是：ACP 不告诉客户端 MCP 挂没挂上。** 唯一可信的信号只能来自**我们自己这一侧** —— crew helper（`--mcp-serve`，我们自己的二进制）被 spawn 时留一个可观测的痕迹，backend 起会话后在超时窗内等它。等不到 = 这家挂不上工具，当场说。

**不能用「问 agent 你有哪些工具」代替。** 那要烧一轮 token，答案还是 agent 自报的 —— 又变成猜。用我们这侧的痕迹，才是把猜换成读。

### 4.1 参照尺（只有第一家有）

同一条 prompt、同一个 cwd，分别走原生 `CodexAppServerBackend` 和新的 ACP 后端，比对：起会话是否成功、`isBusy` 的翻转时序、审批是否弹到同一处、interrupt 后的终止分类、transcript 条目数量级。**差异一律先当我的 bug。**

---

## 5. 分几步

| 阶段 | 做什么 | 出门条件 |
|---|---|---|
| **R0** | 假 ACP agent + §4 那七条判据，**全部先红** | 七条都能红，且各自红在自己那条上（不是被前一条挡住） |
| **R1** | `ACPProtocol` + `ACPConnection`，纯值类型与传输，不接 `SessionBackend` | 单测绿；七条判据里属于传输层的转绿 |
| **R2** | `ACPSessionBackend` 实现十三项必答 | 七条全绿；与原生 codex 后端的参照比对无差异 |
| **R3** | 能力体检 + `ACPAgentSpec` 三态世界观 | 体检器先在 codex / claude / opencode 上**证明会绿**，再在 copilot 上**证明会红**；四家的输出与第 1 节那张表逐条对上 |
| **R4** | 换 opencode 起一个真 session | 它能在群里 `post_to_crew`；世界观那条按「明确没有」如实报出来 |

R3 那条出门条件是有意这么定的，而且是 §1.3 那两处尺子缺陷换来的：**只验「会红」不够**。一个永远读 false 的检测器在 copilot 上也会红，看起来一样对。必须先拿三家已知能过的证明它会绿，那条红才有意义。

---

## 6. 边界（我没覆盖的 / 我的假设 / 这条路什么时候会给错答案）

1. ~~claude-agent-acp 的 MCP 挂载没测~~ —— **已补（`probe7.mjs`，2026-09-09）：它挂得上，工具真被调到了**（假 server 日志里有 `CALLED {"name":"post_to_crew","arguments":{"message":"probe7"}}`，agent 自报的工具表里也列出了 `crew: post_to_crew`）。

   补完之后浮出一条**比原问题更值得盯的事实**：五家里挂得上 crew 工具的只有两家 —— **恰好就是我们已经有原生腿的那两家**。唯一走到能测 MCP 那一步的新 harness（copilot）挂不上。
   **但这条不能当成「新 harness 普遍挂不上」**：n=1，另外两家（gemini / qwen）倒在更早的门上，根本没走到这一步。要判这条趋势成不成立，得再测到至少两三家能起会话的新 harness —— 那正是普查工具（§3.5）第一件该干的事。
2. **我只测了 6 家，registry 上有 40 家。** 剩下 34 家一个没碰（cursor / goose / amp / kilo / vtcode 等都下得到二进制，我没跑）。第 1 节那张表说明不了「ACP 生态普遍如此」，只说明这 6 家如此。**真·新 harness 的样本仍然只有 2 个**（opencode 过、copilot 不过）—— 一个过一个不过，这个比例不构成任何趋势。
3. **opencode 有四项我没测**：审批请求（`session/request_permission` 我一次都没在它身上看到过）、冷进程 `session/load` 实跑（只有它的能力声明）、`session/set_config_option` 换模型、连接中断行为。**把它定为第二家是建立在「它声明了 `resume`」上的，那是它说的，不是我量到的。**
4. **opencode 那一轮 prompt 花了 0 美元**（它自己报的 `cost.amount: 0`）。我**没查它用的哪个 provider、哪来的凭据**。如果它悄悄用了某个我不知道的本地配置，那么「它能跑」这条读数在别人机器上不一定复现。
5. **§1.3 那两处尺子缺陷是我自己制造的，也可能还有第三处我没发现。** 我能找到那两处，是因为先拿一家已知能过的当对照。**只有 codex 一家做过这个对照** —— claude / opencode 那两个 ✅ 我没有独立的第二种方法交叉验证。
6. **cancel 那条读数只测了一次，没做非故障时刻的对照。** copilot 返回 `end_turn` 且 6 秒内 0 个 chunk —— 我推断「cancel 生效了但 stopReason 报错」是因为响应在我发 cancel 后 9 ms 就到了。**我没有测「不发 cancel 时它要多久」**，所以「它本来就快」这个可能性我排除得不够硬。
7. **§4 第 2 条那个 helper 痕迹机制我还没设计。** 我知道它该是什么形状（我们这侧可观测），不知道现有 `--mcp-serve` 启动路径上有没有现成的落点。**这是方案里唯一一处我还不知道怎么落地的**，可能推翻 R2 的排期。
8. **参照尺只在第一家成立。** 第二家没有原生对照，「差异一律当我的 bug」这条纪律那时候就没了 —— 到时候要换一把尺，我现在没有。
9. **本方案假设 `SessionBackend` 一个字不用改。** 依据是上一份报告 3.4 逐条核过的十三项。这个结论是**读代码读出来的，不是写出来的** —— R2 是它第一次被真正证伪的机会。
10. **没读 ACP v2。** SDK 里带着 v2 unstable schema，官方索引显示它砍掉了 File System 和 Terminals、把 Prompt Turn 换成 Prompt Lifecycle。**全程按 v1 做**，v2 来了要重估多少，没算过。
11. **本文行号对着 `7408c09`。** main 常年领先 origin，行号会漂。
