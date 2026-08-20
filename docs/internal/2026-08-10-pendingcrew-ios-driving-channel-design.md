> ## 恢复说明（2026-08-20 补，非原文）
>
> **这一整块是恢复时补的元信息，`---` 分隔线以下是 2026-08-12 最终版原文，一字未改。**
>
> | | |
> |---|---|
> | 原路径 | `docs/superpowers/specs/2026-08-10-pendingcrew-ios-driving-channel-design.md`（旧 monorepo 内） |
> | 原提交 | `6097e4f6`（作者分支）→ `77103cab`（`--no-ff` 合 main）→ `81aa7d99`（同批的 `docs/progress.md` 08-12 补记） |
> | 为什么现在解析不出来 | **不是文件丢失。** 本仓库的 git 历史只回溯到 2026-08-15 —— 它是从 monorepo 快照 `61d876a7` 压平重建的（首个提交 `a61e7a5`，见 `docs/architecture.md` §1.6）。重建前的 sha 一律解析不出来是设计使然，**不代表工作不存在**。丢的只有这份文档本身：它当时提交在 `docs/superpowers/` 下，而该目录未随快照进入本仓库。 |
> | 正文从哪恢复 | Claude Code 的 session transcript（JSONL）中的 `Write` / `Edit` 工具调用**原始入参**，不是群聊转述。基版 `Write` 出自 `…PendingBot-dev--pendingcrew-worktrees-spec-PendingCrew-iOS-spec-UI-app-65115655/8388dbbf-937b-4347-89b5-8493d9e7dab5.jsonl`（2026-08-10T14:16Z，22686 字符），其后 4 次 `Edit` 同 session，2026-08-12 的 6 次修订 `Edit` 出自 `…-PendingCrew/2a5f0590-a8f0-4471-b193-cf7f66abbe1d.jsonl`。11 次操作按时间戳顺序重放，**每一次 `old_string` 都唯一命中，无一处失配**，得到 614 行 / 30068 字符。 |
> | 完整性 | **完整。** 没有任何一节是补写或转述的。 |
> | 恢复日期 | 2026-08-20 |
> | 恢复后的落点 | `docs/internal/`（本仓库无 `docs/superpowers/` 那套目录，不为此新建） |
>
> ### 这份文档现在的状态
>
> **原文第 3 行的「状态：设计待人类审阅」原样保留 —— 恢复不等于审过。** 找回一份文档只改变"它在不在"，不改变"它过没过"。
>
> ### 正文与当前代码的对照（2026-08-20 核，`main` = `ca1011f`）
>
> 这份 spec 定的是**路线 A**（relay 上的 `crew_ctl_*` 控制面）。按当前树核：
>
> - **§5 控制面契约、§6.1 session 状态、§6.2 审批、§6.3 驾驶舱、§8 fail-loud 骨架 —— 全部未落地。** 全树 `crew_ctl` 零命中，`channelState` 零命中。§10 那七步一步都没开始。
> - **§6.4 Todo / §6.5 派活 —— 原文写的"已经通了"仍然成立**：`Sources/Mac/Services/CrewRelayAgent.swift` 里 `handleRemoteTodoAdd`（`crew_todo_add`）与 `handleTaskRequest`（`task_request`）都在树上。
> - **§4.1「`serverLink` 保持 `nil`、路线 B 资产保留不删并标注休眠」—— 当前状态与定案一致。** `Sources/Mac/Views/CrewSessionWindowView.swift:818` 仍是 `let serverLink: CrewSessionServerLink? = nil`；`Sources/Mac/LocalRunner/SessionPermissionRelay.swift` 在。（`edgeQueueBindingReady` 这个门今天已不在树上，原文引用的是旧 monorepo 的行号。）
> - **一处需要读者注意的资产**：`Sources/Remote/`（`SessionProxyProtocol` 289 行 / `SessionProxyClient` 305 行 / `CrewSessionServerLink` 40 行）与 `Sources/Views/Remote/RemoteSessionsView.swift`（391 行）共 1025 行**在树上**（mtime 2026-08-11 20:45），`SessionProxyProtocolTests` 钉着。**这是 §3.2 描述的路线 B 资产，不是本 spec 的实现产物** —— 本 spec 恰恰否决了打开它。它今天的准确状态是「协议与两端实现完成、端到端未接通、iOS 侧无入口」：唯一引用 `RemoteSessionsView` 的 `Sources/Mac/Views/CrewCenterView.swift` 整个文件在 `#if os(macOS)` 之后。
>
> **路径读法**：原文写于 monorepo 时期，引用一律带 `apps/pendingcrew/` 前缀（如 `apps/pendingcrew/Sources/Mac/Services/CrewRelayAgent.swift`）。本仓库已拆成独立仓库，去掉该前缀即为本仓库路径；`apps/edge/` 与 `supabase/` 那些则属于 PendingBot 侧（同级的另一个仓库）。**行号一律按旧树，不要照搬。**
>
> 一句话：**这份 spec 目前仍是纯纸面设计，一步都没实施；树上那 1025 行属于它明确判为休眠的那条路线。**

---
# PendingCrew iOS 驾驶面：走哪条通道（通道定案）

**日期**：2026-08-10 ｜ **状态**：设计待人类审阅
**性质**：**只定通道，不定 UI**。这份 spec 落定之前，谁都不许动 iOS 驾驶面的实现。
**宿主**：**PendingCrew 的 iOS target**（不是 PendingBot 的机组 tab —— 见 §4.2 冲突二）

> **2026-08-12 修订**（上级补的三条事实，逐条落点如下，**结论未变，仍是路线 A**）：
> - **§3.4.1 / §3.4.2 新增** —— 双投那道门今天为什么不可达（一行 `hasServerSession` 过滤），以及去重到底要付什么（跨 edge 下发 + 两端账本 + 两套 busy 语义，**不是"加个 Set"**）。
> - **§3.5 改写** —— 新旧前提并排成表（含"PendingBot 只做轻的"这条新事实），并显式声明**按新前提从头算，不继承 08-08 的结论**。
> - **§6.2.0 新增** —— 审批的**服务端表示**从"没写"补成"定案"：一对日志行、不建表、本地发号、服务端不裁决，并把随之而来的代价（没有"查当前待审批"端点）当场付清。
> - **§6.3 改写** —— 「周期性全量快照最省」这个假设用**本机实测**正面回答：全量 ≈ 3.0 MB（其中 task 账 2.3 MB / 589 条），差两个数量级，故仍走索引推 + 正文拉。
> - **一处自我更正**：初稿把索引体积估成 `< 30 KB`（假设 ~100 条），实测 680 条 → **~80 KB**。结论不变，但新增了分片要求。

---

## ⚠️ 前提：这份设计挂在一个**现在是空的**依赖上

**先看这一段再看结论。** 下面整份设计（无论选 A 还是选 B）都建立在同一个前提上：**本地 crew 在服务端有一条对应的 conversation**。而这一环现在是空的：

> 线上库 `conversations` 表：`user_bot` 9 条、`self` 3 条、`group` 3 条，**`crew` 类型 0 条 —— 这个类型从没在表里出现过**。（2026-08-10 上级直接查库）

**这不是"没实现"，是"没人点过"** —— 通道代码是完整且活着的：

- `Mac/Views/CrewDetailInspector.swift:221` 有现成按钮「**接入 PendingBot**」（登录态 + 未绑定时显示）
- 点它 → `POST /v1/crews`（`apps/edge/src/routes/crew.ts:49`）→ RPC `open_crew_conv` 建 crew conversation → `remoteConversationId` 落进本地 crew JSON
- 之后 `CrewRelayAgent` 对这个 crew 跑双向搬运（拉/推 + hub 常驻 WS + 游标水位 + 幂等），**有单测、在跑**

**每个 crew 要人手点一次，这台机器上一个都没点过。**

### 为什么这一段必须在最前面

这是**同一个坑的第三次出场**：PendingBot 机组 tab（接在休眠的 `crew_sessions` 上）、PendingBot 轻接入、以及本条。**前两条都是先把上层做完，才发现底下是空的。** 所以把依赖显式写在最前面，而不是塞进附注 —— 读这份 spec 的人第一眼就该知道：**这份设计的第 0 号验收条件不是任何一行代码，是"在 Mac 上对某个 crew 点一次接入按钮，`conversations` 里出现第一条 `crew` 行"。**

### 边界（本 spec 不碰）

「本地 crew 镜像上服务端」已被上级认定为当前真正的瓶颈，同时压着三条线（PendingBot 轻接入 / 本条 iOS 驾驶面 / Todo #44 手机直连 fly 机器）。**那条线的诊断另有 session 负责**（要先分清 `#242`/`#297` 挂着 `in_progress` 到底是「写了没跑过」还是「压根没建」）。**本 spec 只声明依赖，不给那条线的解法，也不去查它。**

### 对通道选择的影响：无

这个空依赖对 A、B **同等成立**（B 的 `crew_sessions` 行也要挂在 crew conversation 上），所以它不改变 §3 的比较结论 —— 它改变的是**验收顺序**：见 §10 第 0 步。

---

## 0. 一句话结论

**走路线 A（relay 上的结构化消息），并给它一条与群聊分家的「控制面」子通道。路线 B（复活 `crew_sessions` 队列 + `serverLink`）本次不开门，且它的原始动机已经消失。**

三条关键论据，都不是偏好，是代码里的硬事实：

1. **A 和 B 在审批这一项上是数学互斥的，不是可以各取所长。** `permission_requests.crew_session_id` 是 `NOT NULL REFERENCES crew_sessions(id)`（`supabase/migrations/20260524090046_crew_dispatch_schema.sql:156`）。想用那张服务端表存审批，就**必须**先给每个本地 session 写一行 `crew_sessions` —— 也就是必须打开 `serverLink`，也就是整条 B。**"审批走 B、其它走 A"这个折中在 schema 层面不存在。**
2. **审批的通知半边今天已经在 relay 上跑着了。** `McpPermissionHook.handle` raise 待审批时会往本地白板贴一条带 `@human` 的消息（`Mcp/McpPermissionHook.swift:56`），这条消息被 `CrewRelayAgent.push` 原样带到 edge。**已接入的 crew，手机现在就能收到「待审批：xxx」**。缺的只是"批"这半，不是整条通道。选 A = 把已经通了一半的路补完；选 B = 另起一条路，还要把已经通的这半的双写问题一起解决。
3. **B 的原始动机已经失效。** 08-08 那份 spec 写 B，是为了让 **PendingBot 的机组 tab** 能远程起 session。现在全套驾驶挪到 PendingCrew iOS —— 而 PendingCrew iOS 与 Mac 之间本来就有 relay 绑定。**为"另一个 app 没有 relay 客户端"设计的绕路，在"有 relay 客户端的 app"上不再是必需品。**

---

## 1. 问题重述：这不是移植 UI

Mac 上的完整驾驶建立在「本地为家」上，四条能力各有各的本地数据源：

| 能力 | Mac 上的真源 | 位置 |
|---|---|---|
| session 状态 | 进程内 `CrewSessionRunner.runs` + `SessionStatus` | `Mac/LocalRunner/SessionBackend.swift:8` |
| 审批 | 本地 JSON `<dir>/<crewId>.approvals.json` | `Stores/LocalApprovalStore.swift` |
| 驾驶舱 | Mac 本地仓库文件 `docs/handbook` `docs/state` `docs/roadmap.md` + `~/.claude/tasks/<id>/` | `Models/CockpitModel.swift:362` `Models/CockpitTaskLedger.swift` |
| Todo | 本地 JSON `<dir>/<crewId>.todos.json` | `Stores/LocalTodoStore.swift` |

而 iOS 上没有 LocalBackend：`Stores/AppModel.swift:61` —— `credential != nil ? edgeBackend : nil`，iOS 恒 EdgeBackend。

**所以这四条在 iPhone 上不是"UI 还没搬"，是"没有数据源"。** 把 Mac 的视图编译进 iOS target，得到的是四个恒空的漂亮界面 —— 这正是 PendingBot 机组 tab 已经发生过的事（`docs/tech-debt.md` 2026-08-10 🔴「机组 tab 接的是一条休眠管子」：7 个文件 ~1100 行按 spec 全部落地，跑起来恒空）。

**这是第三次面对同一个坑。这份 spec 的全部意义就是不让它再发生一次。**

---

## 2. 判据：什么样的通道不会再造一次空管子

在比较两条路线之前，先把判据钉死 —— 否则"哪条更好"会退化成口味之争。

空管子的成因不是 UI 写错了，是**上行端从来没被写出来，而下行端看不出这件事**。所以判据只有一条：

> **让"通道没通"成为一个可判定、可显示的单一条件，而不是一串各自会静默失败的前置条件。**

按这条判据量两条路线的**启用前置条件链**：

| | 路线 A（relay） | 路线 B（crew_sessions 队列） |
|---|---|---|
| 前置条件 | ① crew 已「接入 PendingBot」（`remoteConversationId != nil`） | ① crew 已接入 ② Mac 已登录 ③ `listMySubjects` 解出 subject ④ `ensureRunnerHost` 登记成功 ⑤ `createSession` 落行 ⑥ `claimSession` 拿到 lease ⑦ `edgeQueueBindingReady` 门已开 |
| 条数 | **1** | **7** |
| 手机能不能判出"没通" | 能 —— crew 在不在列表里就是答案（未绑定的本地 crew 在 edge 上根本不存在，手机看不到，不会有"看得到但恒空"的中间态） | 不能 —— crew 行存在、列表页正常渲染，但 ②–⑦ 任何一环失败都表现为"列表是空的"，与"真的没有 session 在跑"不可区分 |

**这一栏就是 #456 那次翻车的解剖图**：七个条件里第 ⑦ 个写死 `false`，UI 层完全无从知晓，于是"恒空"被当成"暂时没活"看了两周。

路线 A 的单条件性不是它更聪明，是它的通道**同时承载着人眼可见的群聊** —— 群聊有消息在动，就证明这条管子活着；群聊没消息，人自己就知道这个 crew 没接。**可观测性是白送的，不需要额外的健康检查机制。**

---

## 3. 两条路线正面比较

### 3.1 路线 A —— relay 上的结构化消息

Mac 侧 `CrewRelayAgent`（`Mac/Services/CrewRelayAgent.swift`）对每个绑定 crew 跑双向搬运：5s timer + 每 crew 一条常驻 hub WS（`CrewRealtimeClient`），拉用 `relayCursor` 游标、推用白板水位，回环靠 `relay.origin == mac_relay` 过滤。

载体是 edge `messages` 表既有的 `log_kind` + `log_payload` 两列，**零迁移**。

**这不是设想，是已经跑通两次的既有范式**：

- `task_request`（#242）：iOS 发结构化指令 → `CrewRelayAgent.handleTaskRequest`（`:234`）镜像手动起 session 的整套工序真起 session，`processedTaskRequestIds` 防重，失败上行说明。**Mac 接收端是活的。**
- `crew_todo_add`（Phase 1 Task 9/10）：iOS 发 → `handleRemoteTodoAdd`（`:219`）→ `CrewLocalTodoLanding.land` 落 `LocalTodoStore` + 群里发「To do +1: #N」回执。幂等挂在 `importable`（首次落地）判定上。**已合 main。**

### 3.2 路线 B —— 复活 `crew_sessions` 队列 + `serverLink`

08-08 spec §3 的路径：只对 relay 绑定的 crew 打开 `edgeQueueBindingReady` 门 → iOS 落 `crew_sessions` 行 → realtime 通知绑定的 Mac → `claim_crew_session_for_subject` 认领启动 → 状态回写。

代码资产是真的：`SessionProxyDO` + viewer client + `SessionPermissionRelay`（`Mac/LocalRunner/SessionPermissionRelay.swift`，含 raise/ack 关联、offline queue、本地先决时的反向 mirror）三段都在。**死在起点**：`CrewSessionWindowView.swift:794` `serverLink = nil` 写死、`:914` `edgeQueueBindingReady { false }` 写死。

### 3.3 逐项对比

| 维度 | A | B |
|---|---|---|
| **迁移成本** | 零（复用 `log_kind`/`log_payload`） | 零表结构改动，但要开门 + 补去重 |
| **启用前置条件** | 1 条 | 7 条（§2） |
| **上行端工作量** | 与 B **相当** —— 两边都得有人把 Mac 的状态写出去。A 不比 B 省这份活 | 同左 |
| **状态语义** | append-only 日志，**当前状态要从日志重放** | `crew_sessions.status` 是可变行，天生表达"当前" |
| **实时性** | hub WS 推 + 5s 兜底轮询 | SessionProxyDO WS，亚秒 |
| **离线排队** | store-and-forward 天然支持（消息躺在 edge，Mac 回线拉走） | lease/claim 语义需要 Mac 在线才有意义 |
| **已登记的隐藏成本** | 无 | 🟡 **双投**（下段） |
| **可观测性** | 白送（群聊在动 = 管子活着） | 需要另造健康信号 |
| **审批表可用性** | 用不了 `permission_requests`（要自定义载体） | 能用，但**这正是它必须被复活的原因**，不是额外好处 |

### 3.4 B 的隐藏成本：双投（已登记 🟡，不许自己再踩一遍）

`docs/tech-debt.md` 2026-08-08 收口节：

> 🟡 **mailbox 唤醒与 relay 唤醒的双投隐患**：远端人类 @session 现在既被 edge enqueue 进 `session_mailbox`，又经 relay 落 Mac 白板触发 `CrewLocalMentionWaker` 直投。当前无双投 —— Mac 的 `CrewMailboxWaker` 挂在 `serverLink != nil` 的休眠路径上不可达。**接回 serverLink 时必须先做去重评估**，否则同一条 @ 会唤醒 session 两次。

也就是说：**"当前不双投"的唯一原因就是 B 关着。** 打开 B 的门 = 立刻激活这个缺陷，去重设计是 B 的入场费，不是可选优化。而 A 完全不碰这个开关，双投风险保持为零。

#### 3.4.1 那道门具体长什么样（本次逐段核过，不是推断）

不可达的位置是**一行过滤条件**，不是一个待写的模块 —— 所以"打开 B"这个动作会**顺带**把它接通，很容易在实现时无人察觉：

> `CrewMailboxWaker.drain()`（`Mac/Services/CrewMailboxWaker.swift`）挑唤醒目标时过滤 `$0.hasServerSession`，注释原文写着「登录态（serverLink 存在 == 服务端 session，inbox 才有意义）」。**`serverLink` 恒 nil ⇒ `targets` 恒空 ⇒ 这个 waker 今天一次都没跑过。**

而 relay 那条路是**活的**：`CrewLocalMentionWakeLogic.pending` 的规则 3（`isRelayHuman = senderKind == "user" && relayRemoteId != nil`）**专门为远端人类 @ 放行**（#554 修复）。所以同一条"手机 @ session"在 B 打开后会被两条路各唤醒一次。

#### 3.4.2 去重要付的具体代价

**好消息是键存在**：`session_mailbox_items` 有 `source_message_id` 外键指向 `messages`（`apps/edge/src/db/schema.ts:3080`），而 relay 侧白板条目的 `relayRemoteId` 存的正是同一个 edge 消息 id（`LocalWhiteboardStore.appendRelayMessage:197` 就靠它做幂等）。**理论上两条路可以对齐到同一个键。**

**坏消息是今天对不上，而且不止改一处**：

1. **客户端拿不到那个键** —— `CrewMailboxItem`（`Services/CrewModels.swift:319`）解的是 `id / sender_kind / message_kind / summary / status / created_at / payload`，**没有 `source_message_id`**。要去重先得 edge 下发 + 客户端解码，两端一起改。
2. **两条路的"已处理"账本不在一个地方，也不同步** —— mailbox 路消费靠**服务端** `mark-delivered`（`CrewMailboxWakeLogic.decide` 返回 `deliveredIds`），relay 路消费靠**本地**白板 `entryId` 落地判定。**一条路标了已处理，另一条路看不见。**
3. **谁先到不确定** —— 两条都是事件驱动异步（hub `.changed` 帧 vs 白板变更观察），没有天然先后。去重必须做成"后到的那条查得到先到的那条"，也就是**要么引入一个跨两路的共享已处理集，要么让其中一路彻底不参与唤醒**。
4. **busy 语义不同** —— mailbox 路 busy 时 `.noop` 且**不消费**（留给下一轮）；relay 路走 `CrewLocalMentionInjectLogic.decide` 另一套。同一条 @ 在两路上可能处于"一路已消费、一路还挂着"的中间态，去重逻辑要能表达这个。

**所以去重不是"加个 Set"**：它是一次跨 edge + 两端账本 + 两套 busy 语义的收口。**这就是 B 的入场费的真实尺寸**，写在这里是为了不让下一个人以为它是收尾时顺手能做的事 —— 那正是这类缺陷被漏掉的方式。

### 3.5 B 的原始动机在新前提下还剩多少

08-08 spec §0 的前提是「**iOS = 全套驾驶**」，而那里的 iOS 指 **PendingBot 的 crew tab**（§4 标题原文：「iOS 驾驶面（PendingBot crew tab）」）。PendingBot 里没有 relay 客户端（曾经有，`095dfc2b` 2026-06-15 当死代码删掉），所以对 PendingBot 而言，走 `crew_sessions` 这套 edge 聚合端点是**当时唯一能走的路**。B 是在那个约束下的正解。

现在前提换了，而且是**两处都换了**：

| | 08-08 写 B 时的前提 | 现在的前提（人类 2026-08-11 定） |
|---|---|---|
| 全套驾驶的宿主 | **PendingBot** 的 crew tab | **PendingCrew iOS** |
| PendingBot 的角色 | 驾驶面本体 | **只做轻的** —— crew 降为消息 tab 里的一种会话，独立 crew tab 撤掉 |
| 宿主有没有 relay 客户端 | **没有**（`095dfc2b` 删掉了） | **有** —— PendingCrew 两端本来就靠 relay 说话 |

**第三行是 B 存在的全部理由。** B 从来不是"更好的通道"，它是"**给一个没有 relay 的 app 用的替代通道**"。那个 app 已经不再是驾驶面的宿主了。

> **这里必须显式重算，不许继承旧结论。** 08-08 选 B 在它自己的前提下是**对的**——那不是一个当年写错、今天来纠正的决定。它失效不是因为论证有毛病，是因为**论证的输入变了**。所以下面这张表是按新前提从头算的，不是把旧结论打个折。

逐条清算 B 在新前提下还剩的价值：

| B 的卖点 | 新前提下还成立吗 |
|---|---|
| 让没有 relay 客户端的 app 也能驾驶 | ❌ 动机消失 —— 宿主换成了有 relay 的 app |
| `crew_sessions.status` 天生表达"当前状态" | 🟡 真优点，但代价是 7 条前置 + 双投；A 侧用"最新一条状态消息"可达成同等效果（§6.1） |
| SessionProxyDO 的亚秒级 WS | ❌ 它的用武之地是**终端流式输出 + 亚秒 steer**，而 7-26 勘察 §5 已判「遥控端定位下建议不做终端输出」。**为一个不做的能力付整条通道的钱** |
| `permission_requests` 表现成 | ❌ 反向 —— 用它必须复活 `crew_sessions`（§0 第 1 条），这是成本不是收益 |
| Phase 1 已落地的四档 role 门 | ✅ **与通道无关，A 也照用**（role 判定在 crew 成员表上，不在 session 行上） |

**结论：B 的独有价值在新前提下只剩"状态行语义"一项，而它的代价是 7 条前置条件 + 一个已登记的双投缺陷 + 为不做的能力买单。不成比例。**

---

## 4. 定案与冲突处置

### 4.1 定案

- **通道**：路线 A。所有手机驾驶能力挂 relay 一根管子。
- **`serverLink` / `edgeQueueBindingReady` 保持 `false`，本次不动。** `SessionProxyDO` / `SessionPermissionRelay` / `crew_sessions` 队列**保留不删、明确标注为休眠**，等真要做终端流式遥控时再评估（届时必须先解双投）。
- **不做混合。** 审批不走 `permission_requests`（FK 拽出整条 B），session 状态不走 `crew_sessions.status`。

### 4.2 与 `2026-08-08-crew-multi-driver-design.md` 的冲突（该 spec 正在人类审阅，task #567）

**我没有改动那份 spec。以下是逐条冲突声明，请与它并读。**

**冲突一 —— 通道。** 08-08 §3 末段「远程起 session（iOS → 共有机器）」规定复活 `crew_sessions` 队列、只对 relay 绑定 crew 打开 `edgeQueueBindingReady` 门。**本 spec 主张不打开**，远程起 session 改回已经活着的 `task_request` relay 路径（Mac 接收端 `handleTaskRequest` 现成可用）。

- 为什么选另一条：§3.4 双投 + §3.5 动机失效。
- **附带收益**：08-08 那条路至今没开门（Phase 1 收口尾巴 ③「远程起 session 的 Mac 端自动认领留 Phase 1.5」），而 relay 那条**接收端今天就是通的** —— 改道等于立刻点亮这条能力，而不是再排一个 Phase。
- **不是共存**：两条路都实现 = 同一个"起 session"有两个入口、两套幂等账本（`processedTaskRequestIds` vs `handledSessionIds`），且同时触发双投。**必须二选一。**

**冲突二 —— 宿主（本次最关键）。** 08-08 §4 整节题为「iOS 驾驶面（**PendingBot crew tab**）」，把 crew 列表 / 群聊 / session 遥控 / 批权限 / 起新 session / 加 Todo 全部安置在 PendingBot 里，并写「现有 crew tab 遥控台（feat/crew-tab-remote）不动，接上 role 门」。

而当前方向是 **PendingBot 轻接入（crew 降为消息 tab 里的一种会话、撤独立 tab），PendingCrew iOS 补成正经 iPhone 客户端**。

→ **§4 的宿主已经换人。** 具体后果：

- 「现有 crew tab 遥控台不动」这条失去对象 —— 那个遥控台正是接在休眠管子上的 7 个文件，随 tab 一起撤。
- §4 里"复用消息 tab 的聊天组件族"的复用方向也随之改变（那是 PendingBot 内部复用；PendingCrew iOS 有自己的 `CrewChatView`）。
- **§4 中唯一与宿主无关、可原样继承的是「加 Todo」那条**（`crew_todo_add` 结构化 log 行 + Mac 落账 + 群消息回执 + 消息 id 幂等）—— 它本来就是路线 A 的写法，本 spec §6.4 原样沿用。

**不冲突、明确继承的部分**：§1 四档 role 权限语义（服务端强制）、§2 bot 唤醒管线、§5 请求-回执模型与回环过滤、§6 透明度与登出两级生命周期。这些与通道选择正交，本 spec 全部沿用，不重新发明。

---

## 5. 控制面：与群聊分家的子通道

### 5.1 为什么必须分家

路线 A 的载体是 `messages` 表，而 `role='log'` **已经在** `CHAT_ROLES` 里（`apps/edge/src/routes/crew-comms.ts:343`）。这意味着：**任何结构化 log 行都会被 `CrewRelayAgent.pull` 拉进 Mac 本地白板** —— 而本地白板正是 session 每轮上下文注入的来源。

如果 session 状态、审批请求、驾驶舱数据都以普通消息形式流过去，后果是两条：

1. **群聊被机器噪音淹没**（人看不下去）；
2. **session 的上下文被自己的状态回声撑爆**（更糟：agent 读到自己"正在运行"的播报，是纯污染）。

### 5.2 控制面契约（四条规则，必须有测试守着）

定义一组 `log_kind` 前缀 **`crew_ctl_*`**，规则：

1. **载体**：`role='log'` + `log_kind='crew_ctl_<name>'` + `log_payload=<结构体>`。零迁移，与 `session_post` / `interaction` / `recall` 同一套既有机制。
2. **不进白板**：`CrewRelayAgent.pull` 拉到 `crew_ctl_*` 时，**不调 `appendRelayMessage`**，改为分派给对应 handler。（今天 `crew_todo_add` 是既进白板又落账 —— 那是对的，因为 Todo 有社会意义；控制面没有。）
3. **不进 session 上下文**：即使某条 `crew_ctl_*` 因故落进了白板，白板→session 上下文的构造侧也要滤掉。**两道独立的门**，因为规则 2 是新代码、会被绕过，而上下文污染是不可逆的（agent 已经读了）。
4. **人类可读的部分单独发**：控制面事件里**有社会意义的那些**，另发一条**普通群聊消息**作为回执 —— 与 `crew_todo_add` 的「To do +1: #N」完全同款。例：手机批了一条权限 → 群里出现「小明批准了：`git push`（来自 session xxx）」。**控制面负责机器对账，群聊回执负责人类知情，两者不混。**

> 规则 4 同时是产品语义：**没看到群消息 = 没落账**（沿用 08-08 §5 的请求-回执模型）。

### 5.3 幂等与去重

- **下行（手机 → Mac）**：一律挂 `CrewRelaySyncLogic.importableRemoteIds` 的首次落地判定 —— 已落地的 remoteId 下轮 pull 不再进集合，天然幂等（`crew_todo_add` 已在用这个）。凡是"明确不可行需要标记跳过"的指令（如 task_request 的无工作目录），另记 processed 账本。
- **上行（Mac → 手机）**：控制面上行**不经本地白板水位**，直接 `POST`，避免污染水位游标；重发靠"最新一条覆盖旧的"语义（§6.1），不需要精确一次。
- **回环**：沿用 `relay.origin == 'mac_relay'` 过滤，与现有一致。

---

## 6. 逐能力方案

> 三条能力难度不同，逐个给，不用"都走结构化消息"一句话带过。

### 6.1 session 状态 —— 最容易

**要什么**：手机上看到这个 crew 现在有几个 session、各自在干什么、是不是卡在审批上。

**上行什么**：不发心跳，**发状态变更事件 + 低频全量快照**。

- 事件 `crew_ctl_session_state`，`log_payload`：
  `{ session_id, title, runner_kind, status: running|idle|waiting_approval|exited, exit_reason?, started_at, updated_at }`
  数据源全部现成：`CrewSessionRun` 的 `status` / `SessionStatus`（`Mac/LocalRunner/SessionBackend.swift:8`）/ `SessionExitReason`（同文件 `:16`）/ `CrewSessionTitle`。
- 触发点：**状态跃迁时**（起/停/退出/进出 busy/进出待审批），不按时间。
- **快照**：`crew_ctl_session_roster`，全量一条，在 ① 绑定建立时 ② 每 10 分钟兜底 ③ Mac 冷启动后首次 tick 各发一次。

**手机怎么读当前状态**：**取每个 session_id 的最新一条**（`created_at` 最大），不重放全部历史。这就是路线 B 的 `crew_sessions.status` 行语义在 append-only 载体上的等价物 —— 代价只是客户端一个 `reduce`。

**频率 / 体积 / 成本**：一个活跃 session 一天的跃迁数量级是**几十条**（起、若干轮 busy 进出、若干次待审批、退出），单条 payload < 500B。一个 crew 同时 3 个 session 的日增量约 **50–150 条 / < 100KB**。加上 10 分钟兜底快照 = 每天 144 条。**这个量级对 `messages` 表和 relay 的 500 条/页拉取都不构成压力**，与群聊本身同量级。

> 明确不做：**不发 busy 心跳、不发 token 计数、不发终端输出**。心跳会把日增量推到数千条，且状态本来就能从跃迁推出来。

**失败形态**：见 §8。

---

### 6.2 审批 —— 最难，是"从无到有"

**为什么最难**：`LocalApprovalStore` 是纯本地 JSON，**服务端完全没有对应表示**。唯一存在的服务端表 `permission_requests` 拽着 `crew_sessions` 的 FK（§0 第 1 条），用不了。所以这不是"换个通道"，是**从零设计一套服务端表示 + 幂等 + 断线/重复批**。

**已经通了的那一半（别重复造）**：`McpPermissionHook.handle` raise 时已经往本地白板贴一条带 `@human` 的「待审批：<summary>」（`Mcp/McpPermissionHook.swift:56`），relay push 已把它带到 edge。**手机今天就收得到通知。** 所以本节只设计"结构化 + 批"这两半。

#### 6.2.0 服务端表示：**一对日志行，不是一张表**（这一条先定，后面才有意义）

「从无到有」的第一个问题不是"怎么传"，是"**服务端上这条审批到底以什么形式存在**"。定案：

> **服务端不持有审批状态。** 一条审批在服务端的全部表示 = `messages` 表上同一个 `approval_id` 的一串 `crew_ctl_approval_*` 日志行；**当前状态 = 取该 `approval_id` 最新一条**（与 §6.1 session 状态同一套 reduce）。**不建表、不建视图、零迁移。**

三条随之而来的硬约束，写死在这里，后面各节都靠它：

1. **`LocalApprovalStore`（Mac 本地 JSON）是唯一真源。** 服务端那串日志行是**投影**，不是账本。理由不是省事 —— 是 agent 侧的 hook 只认本地 store，服务端再记一份就立刻有两个真值源，而它们必然漂移（这正是「双写字段」那批债的成因）。
2. **`approval_id` 由本地发号**（= `LocalApprovalStore` 的 `item.id`），服务端不另发。**没有分配环节就没有映射表，也就没有映射表会漂的问题。**
3. **服务端不做任何裁决。** 它不判重、不判过期、不判风险 —— 那三件全在 Mac 侧 handler（§6.2.2 的三道 guard）。服务端在这条链路上的角色**只有存转发**，与它在群聊里的角色完全一致。

> **代价要说清楚**：这么定的直接后果是**手机离线期间的审批状态只能靠拉日志重建，没有一个"查当前待审批列表"的端点可调**。手机侧就得自己维护 `approval_id → 最新状态` 的归并（与 session 状态同款）。这是选 A 换来的 —— 换 B 就有 `permission_requests` 那张现成的表，但入场费是整条 B（§3.4）。**这笔账在这里付清，不留到实现时才发现。**

#### 6.2.1 上行：`crew_ctl_approval_open`

`log_payload`：

```
{ approval_id,          // = LocalApprovalStore 的 item.id（本地即权威 id，不另发号）
  session_id, session_title,
  kind: "permission" | "decision",
  summary,              // 现有
  tool_name,            // 新增
  command,              // 新增：完整命令原文，不截断（超长时截断并显式标注 truncated:true）
  cwd,                  // 新增：工作目录
  session_brief,        // 新增：这个 session 在干什么（taskBrief 首行）
  risk: "low" | "high", // 见 §7
  created_at }
```

**加强 ②「低危也要给足上下文」的落点在这里** —— 而且它暴露了一个必须先补的缺口：

> ⚠️ **`ApprovalItem` 现在只有 `summary`，没有命令原文、没有 cwd、没有风险级**（`Stores/LocalApprovalStore.swift:152-165`）。
> 好消息是原料就在手边：`McpPermissionHook.handle` 拿到的 stdin JSON 里有完整 `tool_input`（`:44`），现在被 `permissionSummary` 压成一句话就丢了。
> **所以审批这条的第一步不是接通道，是给 `ApprovalItem` 加字段、让 hook 把 `tool_input` 原样记下来。** 通道设计得再好，源头没记的信息也变不出来。

配对上行 `crew_ctl_approval_closed { approval_id, decision, decided_by, decided_at }` —— 人在 **Mac** 上批了之后发，让手机上那张卡当场消失（否则手机永远显示一堆已经批完的僵尸卡）。

#### 6.2.2 下行：`crew_ctl_approval_decide`

`log_payload`：`{ approval_id, decision: "allow"|"deny", decided_by_user_id }`

Mac 侧 handler：

```
guard let item = LocalApprovalStore.shared.item(crewId:, id: approval_id) else → 上行 rejected(unknown)
guard item.status == "pending"                                    else → 上行 rejected(already_decided)
guard riskOf(item) == .low                                        else → 上行 rejected(high_risk)   // §7，服务端侧的兜底门
LocalApprovalStore.shared.decide(crewId:, id:, decision:)
→ 阻塞中的 hook long-poll 解除
→ 发群聊回执（规则 4）：「<人名> 批准了：<command>（session <title>）」
→ 上行 crew_ctl_approval_closed
```

#### 6.2.3 断线 / 重复批 / 竞态

| 情况 | 处理 |
|---|---|
| **重复批**（手机重发 / 网络重试 / 用户连点两下） | `importable` 首次落地判定挡住同一条消息的重放；**不同消息但同 `approval_id`** 由 `status == "pending"` 守卫挡住 —— **先到的赢**，后到的回 `already_decided`，手机上那张卡改显示"已由他人处理" |
| **Mac 和手机同时批，且结论相反** | 同上，先写进 `LocalApprovalStore` 的赢。这是正确的：`LocalApprovalStore` 是真源，agent 的 hook 只认它 |
| **手机批了，Mac 离线** | 消息躺在 edge。Mac 回线后 pull 到，此时若 hook 已因 `maxWaits` 超时保守判 deny，则 `status != "pending"`，走 `already_decided` —— **手机得到明确回执，不是静默丢弃** |
| **Mac 批了，手机没收到 closed** | 手机那张卡本地有 TTL：`created_at` 超过 N 分钟且拉不到 closed → 卡自动转灰并标「状态未知，去 Mac 确认」。**不假装还能批** |
| **agent 已经等超时了** | hook 的 `maxWaits` 到点保守判 deny 是既有行为，不改。手机侧收到的 closed 里 `decision=deny, reason=timeout` |

> **不做**：手机侧不做乐观 UI。点了「批准」到收到 `closed` 之间，按钮进 pending 态并显示「已发出，等 Mac 确认」。**不可逆操作不许乐观。**

---

### 6.3 驾驶舱 —— 居中：索引推、正文拉

**先正面回答"周期性全量快照上行是不是最省"**（2026-08-12 上级提的假设）：**方向对 —— 驾驶舱确实该走"推快照"而不是"逐事件"；但"全量"这个量级不成立。** 下面是本机实测，不是估算。

数据源是 Mac 本地**仓库文件** —— `docs/handbook/**.md`（整棵树）+ `docs/state/*.md` + `docs/roadmap.md` + `~/.claude/tasks/<id>/*.json`（`Models/CockpitModel.swift:362` `load(crewRoot:)`）。本仓库当前实测：

| 数据源 | 实测体积 | 条目数 |
|---|---|---|
| `docs/handbook/` | **644 KB** | 86 个 `.md` |
| `docs/state/` | 56 KB | 3 个 `.md` |
| `docs/roadmap.md` | 2 KB | 1 |
| `~/.claude/tasks/pendingbot/` | **2.3 MB** | **589** 个 JSON |
| **全量合计** | **≈ 3.0 MB** | |

按 §6.3 拟的 15 分钟一次（96 次/天）算：**每 crew 每天 ≈ 288 MB 上行**，而且这些内容 99% 每次都没变。当 `messages` 表的行发出去 —— 这是把一个 git 仓库当聊天消息发。**所以全量快照不是"贵一点"，是差两个数量级。**

> **顺带修正本 spec 上一版的一个低估**：初稿把索引体积估成「条目数量级 ~100，< 30 KB」。**实测 task 一本账就有 589 条，不是 ~100。** 按每条 ~120 B 算，索引真实体积是 **70–90 KB**，不是 30 KB。结论不变（仍比全量小 **~40 倍**），但两条要求随之而来：① 索引上行必须能分片，别赌单行装得下（`crew-comms` 侧未见显式 payload 上限，**不能当作没有上限**）；② 索引里**只放 frontmatter 抽出来的字段，一个正文字都不许混进去** —— 一旦混进正文，40 倍的差距会很快被吃掉。

**方案：两段式。**

**① 索引 —— 推。** `crew_ctl_cockpit_index`，低频全量覆盖式：

```
{ generated_at,
  roadmap_phases: [ { title, status, groups: [ { name, entries: [ { relpath, note, status } ] } ] } ],
  tasks: [ { id, title, status, owner, updated } ],   // 走 CockpitTaskLedger 判定出的那本账
  ledger_source: "live" | "repo",                     // 回落时如实标出（既有语义）
  ledger_fallback_reason?: String }
```

只带 frontmatter 抽出来的 status / 标题 / 路径，**不带任何正文**。

**体积（按上面的实测数，不是估算）**：roadmap + handbook/state 期望页约 90 条，task 账 589 条 → **约 680 条 × ~120 B ≈ 80 KB**。比全量的 3.0 MB 小 **~40 倍**，这个差距全部来自"不带正文"这一条 —— 所以它是硬约束，不是优化建议。

**频率**：绑定建立时 + 每 15 分钟 + Mac 侧检测到 `docs/` 目录变化时（已有 `DispatchSource` 目录监听基础设施）。取最新一条即当前索引。按 15 分钟一次算 **≈ 7.7 MB/天/crew** —— 与群聊同量级，可接受。

> **两条随体积而来的实现要求**（别等实现时才发现）：
> ① **索引必须支持分片上行**。80 KB 是本仓库今天的数，task 账只会涨；`crew-comms` 侧没看到显式 payload 上限，**"没找到上限"不等于"没有上限"**（还有 relay 单页 500 条的拉取窗口）。设计成一条装不下就分片，别赌。
> ② **覆盖式全量是有意的，别改成 delta**。全量 80 KB 换来的是"取最新一条 = 当前索引"这个无状态语义；改成 delta 就要维护重放，而重放丢一条的表现恰好是"驾驶舱显示的东西是错的且看不出来" —— 与 §8 的 fail-loud 契约直接冲突。

**② 正文 —— 拉。** 请求-回执：

- 手机点开某页 → `crew_ctl_cockpit_page_request { relpath }`
- Mac handler → `CockpitData.readExpectation(handbookDir:relpath:)`（现成，`:434`）→ `crew_ctl_cockpit_page { relpath, markdown, generated_at }`
- **上限 256KB**，超了截断并置 `truncated: true`，手机显示「这页太长，只显示前一部分，完整内容去 Mac 看」。
- 手机侧按 `relpath` 缓存，`generated_at` 变了才重拉。

**Mac 离线时**：请求躺在 edge，手机上那页显示「机器离线，这页拉不到（索引更新于 <索引时间>）」。**索引仍然可读** —— 这是两段式的额外好处：离线时驾驶舱不是全黑，而是"能看目录、看不了正文"，且这个降级是明说的。

**明确不做**：手机上编辑期望页（Mac 侧那个就地编辑不搬）。写回一个 git 仓库的冲突处理不值得为遥控端做。

---

### 6.4 Todo —— 已经通了，原样沿用

**这条不用设计，Phase 1 已落 main**：iOS 发 `message_kind='todo_add'` → edge 落 `log_kind='crew_todo_add'` + `log_payload={text}`（`apps/edge/src/routes/crew-comms.ts:551`）→ `CrewRelayAgent.pull` 首次落地判定 → `handleRemoteTodoAdd` → `CrewLocalTodoLanding.land` → `LocalTodoStore.add` + 群里「To do +1: #N」回执 + 唤醒机长。

**本 spec 只加两条**：

1. **读的那半还没有。** 现在手机只能"加"，看不到 Todo 列表和机器人的回应。补 `crew_ctl_todo_list` 索引上行（覆盖式全量，条目数量级 ~50，`{number, text, status, responses_count, updated_at}`，< 20KB），触发点挂 `LocalTodoStore.changes`（现成的变更信号）。
2. **`crew_todo_add` 是"进白板"的**（人要看见谁加了什么），与 §5.2 规则 2 不矛盾 —— 规则 2 只管 `crew_ctl_*` 前缀。**Todo 的新增走群聊、Todo 的列表走控制面**，这是有意的分工。

> 既有尾巴（`docs/tech-debt.md` 2026-08-08 🟢）：Mac 落账成功但回执消息发送失败时形成"已落账无回执"，与"没回执=没落账"的承诺反向。**本 spec 不修，但把它记为 §8 fail-loud 契约的已知例外。**

---

### 6.5 派活 / steer / 停 session —— 复用既有

- **派活**：`message_kind='task_request'`，Mac 接收端 `handleTaskRequest` 现成。**这是从 08-08 §3 的 `crew_sessions` 路径改道过来的**（§4.2 冲突一）。
- **steer（给正在跑的 session 插话）**：走既有的 `@session` 定向 mention → 白板唤醒路径，**不新造通道**。

  > **关于「远端人类 @session 不唤醒 session」这条断链 —— 账本记的现状已经过期，代码里已经修了。**
  >
  > 7-26 勘察 §4 与 `docs/tech-debt.md` 2026-08-10 都把它记成**现存的洞**。本次逐段核读源码，三段链路**都已接通**（修复提交 `48f3e34e`，Phase 1 Task 10）：
  > 1. `CrewRelayAgent.swift:162` —— 调用方**真的传了** `mentions: Self.localMentions(entry.mentions)`（关键一处：光有形参不传等于没修）
  > 2. `LocalWhiteboardStore.appendRelayMessage:192` —— 有 `mentions:` 形参，且落进 `LocalWhiteboardMessage` 行
  > 3. `CrewLocalMentionWakeLogic.swift:48-49` —— guard 显式放行 `isRelayHuman`（`senderKind == "user" && relayRemoteId != nil`），由 `CrewLocalMentionWaker.swift:84` 驱动
  >
  > **诚实标注**：这是**静态核读，未真机验**。所以本 spec 不把它当已闭合，而是列为 §10 第 0 步的 QA 项之一 —— 点完接入按钮之后，**「手机 @ 普通 session，那个 session 真被唤醒」必须实点一次**。（同一批要验的还有 @机长，但机长有自己的唤醒通道，@机长通不代表 @普通 session 通，两项要分开验。）
  >
  > 账本更正不归本 spec 做 —— 已由「Bot 侧 crew 轻接入」那条线一并处理（四处记录：tech-debt、progress、7-26 勘察快照、它自己那条尾巴）。
- **停 session**：`crew_ctl_session_stop { session_id }` → Mac 侧调既有的 `SessionStopCoordinator`。机长已有 `stop_session` 工具，编排复用。

权限一律走 08-08 §1 的四档 role（member 只能管自己起的），**服务端强制**，与通道选择无关。

---

## 7. 风险分级清单（**留给人类过目 / 直接增删**）

> 机长 2026-08-10 拍板：手机审批走**分级**（甲），配三条加强。本节是加强 ③ 的落点 —— 单独成节，人类审 spec 时一眼扫完。

### 7.1 三条硬规则

1. **未知一律高危。** 分级器不认识的 `tool_name` / 命令形态，**一律归高危**，不许当低危放行。规则总有覆盖不到的命令，**默认值往哪边倒决定这个设计是安全的还是危险的**。判错要往"人在手机上干等"这边错，不许往"误批不可逆操作"那边错。
2. **分级在 Mac 侧算，不在手机侧算。** 手机只读 payload 里的 `risk` 字段。手机侧另有一道兜底：**payload 里没有 `risk` 字段的一律按高危渲染**（老版本 Mac 上行的消息不会因为字段缺失而变成可批）。
3. **服务端侧也有门。** `crew_ctl_approval_decide` 的 Mac handler 会**重算一次 risk** 并拒绝高危（§6.2.2）。UI 隐藏按钮不是安全边界 —— 与 08-08 §6「observer 只读由服务端强制，不是 UI 假装」同一原则。

### 7.2 初版清单（**待人类增删**）

**低危 —— 手机可直接批**

| 类别 | 判据 |
|---|---|
| 只读文件 | `Read` / `Glob` / `Grep` |
| 只读 git | `git status` / `git diff` / `git log` / `git show` / `git branch`（无 `-d`/`-D`/`-m`） |
| 跑测试 | `xcodebuild ... test` / `vitest run` / `bun test` / `swift test` |
| 类型检查 / 构建 | `bun run typecheck` / `xcodebuild ... build` / `tsc --noEmit` |
| 包管理只读 | `bun install`（无 `--production` 之外的副作用）/ `bun pm ls` |
| 在**当前 crew 工作目录内**新建或修改文件 | `Write` / `Edit`，且 `path` 在 `workingDirectory` 子树内 |

**高危 —— 手机只显示、不给批钮**

| 类别 | 判据 |
|---|---|
| 删除 | `rm` / `git clean` / `Bash` 含 `rm -rf` / 删目录的任何形态 |
| 推送与发布 | `git push` / `wrangler deploy` / `supabase db push` / 任何 TestFlight/上架动作 |
| 历史重写 | `git reset --hard` / `git rebase` / `git commit --amend` / `git filter-*` |
| 外发网络 | `curl` / `wget` / 任何 POST 到外部域名 / `gh` 的写操作（PR/issue 创建、comment） |
| 凭据与密钥 | 读写 `.env` / Keychain / `secrets` / `wrangler secret` |
| 越出工作目录的写 | `Write`/`Edit` 的 `path` 在 `workingDirectory` 子树**之外**（含 `~/.claude/` 与其它 worktree） |
| 系统与权限 | `sudo` / `chmod` / `launchctl` / 安装到 `/Applications` |
| 界面自动化 | `osascript` 驱 System Events / `cliclick` / `screencapture` / computer-use（世界观里本来就要先问人） |
| 数据库写 | 任何 `INSERT`/`UPDATE`/`DELETE`/`DROP`/migration 应用 |
| **以上都不匹配** | **默认高危**（规则 1） |

### 7.3 规则放哪儿、谁维护

- **放哪儿**：Mac 侧一个自包含的纯函数 + 一份数据表（与 `SessionExitReason.classify` 同款：纯判定、可单测、编进 `pendingcrew-mcp` helper bundle）。**不放 JSON 配置文件** —— 分级规则是安全边界，改它应当经代码评审与测试，不该是一个能被随手编辑的运行时文件。
- **谁维护**：随 `gates`（`McpPermissionHook.gates`）一起演进。**新增一条低危规则必须同时新增一条单测**，否则 CI 红。
- **必须有的测试**：一条"未知命令 → 高危"的正面断言。没有它，规则 1 会在某次重构里静默失效，而失效方向恰好是危险的那边。

### 7.4 高危卡的文案（必须让人一眼看懂是设计不是故障）

**不做灰按钮。** 灰按钮读起来像"卡住了"或"你没权限"，人会反复点、会以为是 bug。

高危卡上按钮位置直接换成一行说明：

> **这条要回 Mac 批**
> 删除 / 推送 / 外发这类操作不可逆，手机上看不到终端上下文，所以只在 Mac 上批。
> 〔在 Mac 上打开这个 session〕

—— 陈述句、给出原因、给出下一步动作。与 §8 的 fail-loud 是同一件事：**把系统的真实状态说出来，不要用视觉手段暗示。**

---

## 8. 失败形态：fail-loud 契约

> 这一节是本 spec 存在的原因。机组 tab 那次的直接教训：**数据源没接通时渲染一个恒空的漂亮界面，等于把故障伪装成"暂时没内容"。**

### 8.1 总则

**每一个驾驶面区块都必须能区分三种状态，并且在 UI 上长得不一样：**

| 状态 | 含义 | 长什么样 |
|---|---|---|
| **通了、空的** | 通道正常，确实没有内容 | 常规空态（「还没有 session 在跑」） |
| **没通** | 前置条件不满足 | **显式故障态**：说明缺什么 + 怎么补 |
| **不知道** | 曾经通过，现在拉不到 | **陈旧态**：显示最后已知数据 + 「更新于 X 分钟前，机器可能离线」 |

**"通了、空的"与"没通"长得一样 = 这份 spec 失败了。**

### 8.2 逐条

| 区块 | 没通的判据 | 文案 |
|---|---|---|
| **crew 列表** | 未登录 | 「登录后才能看到接入的 crew」（既有 WelcomeView 时态） |
| **crew 列表** | 登录了但一个 crew 都没有 | 「还没有 crew 接入手机 —— 在 Mac 上打开 crew，点侧栏的「接入 PendingBot」」。**不显示空列表了事** |
| **session 状态** | 该 crew 从没上行过 `crew_ctl_session_roster` | **「这台 Mac 还没上报过 session 状态 —— 可能是 Mac 上的 PendingCrew 版本较旧，或没在运行」**。不是"没有 session" |
| **session 状态** | 上行过，但最新快照 > 30 分钟 | 陈旧态：列表照显，顶部一条「更新于 X 分钟前，Mac 可能离线」 |
| **审批** | 从没上行过任何 `crew_ctl_approval_*` | 「这台 Mac 还没上报过审批 —— 待审批只会在 Mac 上出现」 |
| **审批** | 高危卡 | §7.4 文案 |
| **审批** | 点了批，超时没收到 closed | 卡转灰 + 「状态未知，去 Mac 确认」。**不重发、不假装成功** |
| **驾驶舱** | 从没上行过索引 | 「这台 Mac 还没上报过驾驶舱索引」 |
| **驾驶舱** | 有索引、正文拉不到 | 「机器离线，这页拉不到（索引更新于 X）」。**索引仍可浏览** |
| **驾驶舱** | 正文被截断 | 「这页太长，只显示前一部分，完整内容去 Mac 看」 |
| **Todo** | 从没上行过列表 | 「这台 Mac 还没上报过 Todo 列表」。注意：**「加 Todo」这个入口仍然可用**（它走群聊、不依赖控制面），加完看不到列表是正常的降级，要说明 |

### 8.3 怎么保证这些不会退化成空态

**"UI 会 fail-loud"是一句没有约束力的话，除非有测试。** 两条要求写进实现计划：

1. **每个区块的"没通"态必须有单测**：喂一个"控制面从未上行过"的 store，断言渲染出的是故障文案而不是空态。
2. **通道健康度是一个显式的 model 字段，不是 `items.isEmpty`。** 客户端 model 里要有 `channelState: .neverSeen | .fresh(at:) | .stale(at:)`，UI 分支打在它上面。**只要 UI 是从 `isEmpty` 推断状态的，这个 spec 的保证就是假的** —— 这是 #456 那次的确切失败机制。

---

## 9. 不做什么（明确的非目标）

- **终端流式输出**（7-26 勘察 §5 已判：遥控端定位下不做。用「进展摘要进群聊」替代）
- **打开 `serverLink` / `edgeQueueBindingReady` / SessionProxyDO**（保留代码、标注休眠）
- **iOS 上创建 crew**（沿用 08-08 §4：建 crew 归 Mac）
- **iOS 上编辑驾驶舱期望页**（§6.3）
- **手机批高危权限**（§7）
- **新表 / 新迁移**（整份设计零迁移；一旦发现某条能力必须建表，回来重新论证通道，不要偷偷加表）
- **PendingBot 侧的机组 tab 改造**（那是另一条线：Bot 轻接入）
- **推送（APNs）**：edge crew-comms 的 POST 目前不发推送，接推送是独立一条，不在本 spec

---

## 10. 实施顺序（给 writing-plans 的输入，不是计划本身）

按"最早点亮 + 最小风险"排：

**0. 先让服务端出现第一条 `crew` conversation —— 这一步不写代码，且它挡着后面全部七步。**

按前提段：线上 `conversations` 里 `crew` 类型 0 条。所以动工前先做一次人工操作 + 一次实点验证：

- 在 Mac 上对某个 crew 点一次「接入 PendingBot」（`CrewDetailInspector.swift:221`）
- 核 `conversations` 里出现该 crew 的行、本地 crew JSON 里有 `remoteConversationId`
- **实点三项**（都是"通道通没通"的直接证据，不是"代码写了"）：
  1. Mac 发一条群聊 → 手机上看得到
  2. 手机发一条 → Mac 白板里看得到
  3. **手机 @ 一个普通 session → 那个 session 真被唤醒**（§6.5 那条静态核读过、未真机验的链路）

**这三项任何一项不通，后面七步全部暂停** —— 因为它们全都挂在这根管子上。**别在管子没验通之前写驾驶面 UI**，那正是前两次翻车的动作顺序。

---

1. **控制面契约 + 两道过滤门**（§5.2 规则 2/3）+ 它们的测试。**必须先做** —— 后面每一条都往这条管子里灌东西，门没装好就灌 = 白板被污染，且很难回收。
2. **session 状态**（§6.1）。最容易，且它是"通道到底通没通"的第一个可见证据。
3. **fail-loud 骨架**（§8.3 的 `channelState` model + 测试）。**必须在任何驾驶面 UI 之前** —— 否则"先做 UI，fail-loud 后补"必然变成永远不补。
4. **Todo 读的那半**（§6.4）。小，且写的那半已通，能立刻形成闭环。
5. **审批**（§6.2）。先补 `ApprovalItem` 字段 + 分级函数 + 分级测试（§7.3），再接通道。
6. **驾驶舱**（§6.3）。索引先行，正文按需拉后做。
7. **派活改道**（§6.5）—— 把 08-08 §3 的 `crew_sessions` 路径正式作废，改用 `task_request`。

---

## 11. 账本要改的

- **`docs/tech-debt.md` 2026-08-10 🔴「机组 tab 接的是一条休眠管子」**：该条「补法已有设计，等人拍板」指向 08-08 §3/§4。本 spec 落定后，那条的补法**改指本文**，并注明 08-08 §3 的复活路径被显式否决（§4.2 冲突一）、§4 的宿主已换（冲突二）。
- **`docs/tech-debt.md` 2026-08-08 🟡「双投隐患」**：状态改为「**本 spec 选 A 后短期不会触发**（`serverLink` 保持休眠）；该缺陷随 B 一起搁置，未来若开 B 仍是入场前置」。
- **`docs/progress.md`**：记本次通道定案 + 与 08-08 spec 的两条冲突。
- **`#242`** 的描述（「代码全落待 E2E」）在本 spec 下变准了一半：发起端将由 PendingCrew iOS 重建（不是 PendingBot）。等实现落地再翻。
- **roadmap / handbook**：`concepts/multi-driver.md` 期望页已存在（2026-08-09 建），本 spec 落地后需补一句通道选择，否则驾驶舱上看到的期望与实际通道不符。
- **「远端人类 @session 不唤醒」的过期记录**（7-26 勘察 §4 / `docs/tech-debt.md` 2026-08-10 / `docs/progress.md`）：**不归本 spec 改，已由「Bot 侧 crew 轻接入」那条线一并更正**（修复提交 `48f3e34e`）。本 spec 只在 §6.5 留证据与"未真机验"的标注，避免两条线重复改同一处、互相覆盖。
- **`conversations` 无 `crew` 行这条前提**：本 spec 只声明依赖（前提段）。**瓶颈本身的诊断另有 session 负责**，本 spec 不给解法、不查那条线 —— 见前提段「边界」。

---

## 12. 主要引用

**勘察与既有设计**

- `docs/superpowers/specs/2026-07-26-ios-remote-console-survey.md` —— §4 数据通道现状、§5 逐项能力判定、§6 路径建议（本 spec 的路线 A 出处）
- `docs/superpowers/specs/2026-08-08-crew-multi-driver-design.md` —— §1 四档 role（沿用）、§3 远程起 session（**否决**）、§4 iOS 驾驶面（**宿主已换**）、§5 请求-回执（沿用）
- `docs/superpowers/specs/2026-06-10-pendingcrew-pendingbot-integration-v2.md` —— 本地为家 / edge 是邮差
- `docs/superpowers/specs/2026-06-08-pendingcrew-ask-approval-design.md` —— 本地审批链原始设计
- `docs/tech-debt.md` 2026-08-10 🔴 机组 tab / 2026-08-08 🟡 双投

**代码（勘察时逐条核过）**

- `apps/pendingcrew/Sources/Stores/AppModel.swift:61` —— iOS 恒 EdgeBackend
- `apps/pendingcrew/Sources/Mac/Views/CrewSessionWindowView.swift:794` / `:914` —— `serverLink = nil` / `edgeQueueBindingReady { false }`
- `apps/pendingcrew/Sources/Mac/Services/CrewRelayAgent.swift` —— 双向搬运；`:219` todo 落账、`:234` task_request
- `apps/pendingcrew/Sources/Stores/LocalApprovalStore.swift:152` —— `ApprovalItem` 字段（缺命令原文 / cwd / risk）
- `apps/pendingcrew/Sources/Mcp/McpPermissionHook.swift:44` / `:56` —— `tool_input` 可用但被丢弃；白板通知已在 relay 上
- `apps/pendingcrew/Sources/Mac/LocalRunner/SessionBackend.swift:8` / `:16` —— `SessionStatus` / `SessionExitReason`
- `apps/pendingcrew/Sources/Models/CockpitModel.swift:362` / `:434` —— 驾驶舱本地文件加载 / 单页读取
- `apps/pendingcrew/Sources/Mac/LocalRunner/SessionPermissionRelay.swift` —— 路线 B 的审批桥（休眠）
- `apps/edge/src/routes/crew-comms.ts:343` `CHAT_ROLES` 含 `log` / `:407` `message_kind` 枚举 / `:551` `crew_todo_add`
- `supabase/migrations/20260524090046_crew_dispatch_schema.sql:156` —— `permission_requests.crew_session_id` FK（A/B 互斥的证据）
