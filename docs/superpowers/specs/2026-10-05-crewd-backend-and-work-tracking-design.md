# crewd：独立后端 + 工作跟进机制 设计

> 写给实现者（Claude）看的，不是给人看的汇报。基准：`main@5fa34b7`（2026-10-05）。
> 来源：2026-10-05 与人的设计对话。人的原话/拍板标 **[人]**，我的推断标 **[推]**，读代码读到的标 **[码 file:line]**。
> 状态：草案。§13 是还要和人商量的点；语言选型（§3.1）待调研回来补。

## 0. 一句话

把「数据 + 协作调度」从 Swift app 里拿出来，做成独立、跨平台的后端进程 **crewd**；第一个在 crewd 里落地的功能是**工作跟进机制**（两层 Todo、负责人、检查点、Jev 调度、行政处）。Mac/iOS 都只是 crewd 的客户端。

## 1. 目标 / 非目标 / 判据

目标
- G1 布置下去的事一定有人跟进，不靠 session 记性，不靠人催 **[人]**。
- G2 卡在人身上只卡那一件，不卡 session **[人]**。
- G3 高频小判断（派给谁、谁收消息、归到哪条、侧边栏排序）由 Jev 做，机长只接低置信度 **[人]**。
- G4 前后端解耦，后端跨平台，Mac 与 iOS 平等地是客户端 **[人]**。
- G5 iOS 正式重启（08-23 的「当没有 iOS」作废）**[人]**。

非目标
- 不做云端账号/中继（2026-08-24「不登录到任何地方」仍有效）。Jev 是唯一新增外发，用户可关。
- 不保留旧机制的兼容层（项目未发布；记忆：UI-first、无兼容垫片）。
- 第一阶段不移植 PTY/终端仿真（见 §3.3 分期）。

验收判据（端到端，真实机组跑）
- 发一条 Todo → 无人工干预下被派、被整理成工作项、按检查点被叫醒推进、完成带证据。
- 中途让一个工作项卡在人身上：该 session 在同一小时内接到/推进别的工作；人回复后负责人被直接叫醒。
- 杀掉 app/daemon 重启：没有任何到期检查或排队唤醒丢失。
- iOS 能看群、发消息、回复待确认项、看工作项进度。

## 2. 现状要点（决定设计的事实）

- Todo 无负责人字段，负责人由「最后一条 in_progress 回复」推断；任何 session 可改任何 Todo 状态 **[码 McpServer.swift:1616-1695, CaptainTodoSweep.swift:41]**。
- 只有机长被 sweep/60s checkpoint；worker 永不被查 **[码 CrewSessionRunner.swift:491,577,608]**。
- blocked_on_human 被 sweep 视为「已覆盖」无限期 **[码 CaptainTodoSweep.swift:128]**；`continue_work` 拒收 blocked 项 → session 闲着。
- 延迟唤醒队列只在内存 **[码 CrewDeferredWakeQueue.swift]**；重启丢。
- Stop hook **刻意永不阻塞**（「把等人变空转」）**[码 McpTurnHook.swift:13]** —— 本设计的「停下即检查」必须走 idle 边沿重新唤醒，**不能**改成 Stop block。
- 上下文注入已有两条路：claude 的 PostToolUse `additionalContext`（白板未读）**[码 HookEmitter.swift:166-221]**；codex `turn/start.additionalContext` 按 key 去重 **[码 CodexProtocol turnStartParams]**。claude 目前没注册 UserPromptSubmit。
- 机组是多父 DAG（`parentIds`）**[码 LocalCrewStore.swift:416]**，总机组不是存下来的边。→「根机组」= 无存储父边的机组；多父时归属有歧义（§13 Q1）。
- 唤醒准入限流：全机 12/分、单 session 2/分、单机组 5/分，且人类唤醒也受全机限 **[码 AutomaticWakeAdmission.swift:282-330]**。检查点唤醒量上来会撞这个。
- 全部数据是 JSON + flock，无 DB **[码 MultiProcessJSONStore.swift]**；Mac UI 直接读写这些文件，绕过协议。
- 已有语言中立线协议（长度前缀帧：control JSON / terminal bytes / snapshot）**[码 SessionProtocol.swift:11-17]**，但只覆盖 session + 7 个 crew 读/发操作。
- iOS 配对用 TLS1.2 外部 PSK（Network.framework）**[码 SecureTCPTransport.swift]**。换语言后服务端未必有外部 PSK 支持（待调研）。

## 3. crewd 架构

### 3.1 语言
待定。候选 Go / Rust / TS(Bun)。决定因素按权重：①无头终端仿真库成熟度（含 CJK 宽度）——只影响第二阶段；②官方 MCP SDK；③与 iOS 互通的安全信道；④单二进制分发与常驻内存。调研结果回来后填表并给推荐，**交人拍板**。

### 3.2 进程与职责（终态）
- `crewd`：常驻。拥有全部持久状态与调度；对客户端提供 API；对 agent 提供 MCP（`crewd mcp` 子命令作为每个 session 的 stdio MCP helper，与今天「app 二进制再跑一遍」同构）；跑会话（PTY / codex app-server）。
- Mac app、iOS app：纯客户端，经同一 API。Mac 本机走 Unix socket，iOS 走配对后的加密直连。

### 3.3 分期（绞杀式，每期结束系统可用）
- **P1 协作平面**：crewd 接管 机组/白板/两层 Todo/人类待确认/审批台账 的读写、调度（Jev+唤醒计划+检查点）、行政处、MCP 全部工具。Swift daemon 降级为「会话执行器」：只管 PTY/codex/终端快照，经现有 SessionProtocol 接受 crewd 的 `launch / submitWake / stop / screenText`。Mac UI 改为只经 crewd API 读写（去掉直接读写 JSON）。claude 的三个 hook（PostToolUse 白板注入、PreToolUse 审批、Stop 留痕）与 MCP helper 一起改指向 `crewd` 二进制——它们今天都是「app 二进制再跑一遍」**[码 LocalSessionLaunch.swift:131-133]**，P1 不改就会出现两个写者。
- **P2 执行平面**：会话执行器移进 crewd（终端仿真、codex 客户端、孤儿回收）。Swift daemon 删除。
- **P3 iOS**：对接 crewd 远程 API（可与 P2 并行，依赖 P1 的 API 定型）。
- **[推]** 为什么先协作平面：工作跟进的价值全在 P1；PTY 仿真是最大技术风险但和本需求无关；SessionProtocol 已语言中立，P1 期间 crewd→Swift 执行器的桥成本低。

### 3.4 数据
- 存储：**[推]** 换成单个 SQLite（crewd 独占写）。理由：多写者 JSON+flock 的历史问题（EPERM、读失败 spool、白板单向等）根源之一是多进程共享文件；crewd 成为唯一写者后，没有理由继续用文件做总线。客户端/MCP 一律经 crewd。`LocalCrewStore.swift:19` 当年拒 SQLite 的理由是多进程——前提已变。→ §13 Q6 确认。
- 迁移：首启一次性导入旧 JSON（只读，原目录保留作备份，记忆：热替换前先冷备份数据目录）。

### 3.5 API
- 一套 RPC（request/response + 订阅推送），JSON。传输：本机 Unix socket；远程走加密直连（方案待调研 §3.1③）。
- 契约文件（schema）放仓库，Swift 客户端与 crewd 都按它生成/校验类型，避免两边各写一份。
- 订阅：白板、Todo、工作项、会话状态的变更流；客户端断线重连按序号补。

## 4. 两层 Todo

### 4.1 第一层：入口（= 现在的 agent Todo）**[人]**
- 人发的原始需求，原文不改，编号 `#N` 沿用。
- 状态**由第二层推导**，agent 不再直接改：无关联工作项 → 待认领；有 → 进行中；关联项全部 done → 完成；全部 dropped → 交机长/人裁定（不自动关）。
- 认领入口 = 登记第二层工作项；入口下自动生成一行说明 + 链接（谁认领、对应哪些工作项）。

### 4.2 第二层：工作项 WorkItem **[人]**
字段（**[推]** 编号用 `W<n>`，与 `#N` 分开命名空间——记忆：两本账号码会撞）：
```
id W<n>, crewId, title, body(目标 + 完成层级: local|public|production + 验收标准)
parent?: W, dependsOn: [W], sources: [#N]          // 层级 + 依赖 = DAG
owner?: sessionId, ownerHistory: [{from,to,by,how,note,at}]
status: open|active|blocked|done|dropped
blockedOn: [{human: H<n>} | {item: W}]               // blocked 必须非空
nextCheckAt?, missedChecks: Int
evidence?: {commit|url|text}                         // done 必填
pendingTransfer?: {to, requestedBy, at}
revision, createdAt, updatedAt
```
不变量（crewd 写入时强制，违反即拒）
- 无环（parent 与 dependsOn 的并图）。
- 单负责人；只有负责人、机长、行政处（限 §7.3 权限）能改状态。
- active ⇒ nextCheckAt 存在且 ≤ now + 上限（默认上限 2h，可配）。
- done ⇒ evidence。
- dependsOn 未全 done 的项不能 active（只能 open/blocked）。
- **[推]** v1 依赖只限同机组；跨机组靠入口 Todo 的 contact，不做跨组 DAG 边（§13 Q5）。

### 4.3 「我的工作项」常驻上下文 **[人]**
- 每次唤醒正文 + 每轮注入一段「你名下的工作项」（id、标题、状态、下一步、检查时间、被谁卡）。
- claude：新增 UserPromptSubmit hook 注入 `additionalContext`（每次唤醒都是一次 prompt 提交）；PostToolUse 照旧带白板未读。**实现前查官方 hooks 文档核字段**（记忆：接入必查官方文档）。
- codex：`turn/start.additionalContext` 新 key `my_work_items`，内容不变则不重发。

## 5. 负责人与跟进引擎

- **绑定**：派活即绑定负责人（Jev 或机长决定）。空负责人的 open 项是「待认领」，行政巡检会捞。
- **换负责人** **[人]**：原负责人发起 + 新负责人接受；或机长强制指派。交接说明必填，写入 ownerHistory。
- **检查点**：crewd 的持久化计划表。到点 → 给负责人发一次唤醒，正文列出到期的全部工作项（**同一 session 同一时刻合并为一条**，减压限流）。负责人忙 → 进持久化队列，idle 边沿投递。
- **负责人失联**：唤醒连续 K 次（默认 2）无回报，或会话不可恢复 → 重新派（Jev，排除原负责人）+ 告知机长。**[推]** 「session 退出」≠ 失联：现在叫醒一个已退出的 session 会按会话号 `--resume` 把它拉起来，所以先尝试恢复，恢复失败才算失联（§13 Q4 已自决，留记录）。
- **停下即检查**：session 进入 idle 时，若名下有 active 项缺 nextCheckAt，或 open 且 ready 的项无人推进 → 立即再唤醒一次，要求给每项写下一步。同一 session 连续 3 次无改善 → 停止重唤醒，交行政处（防死循环）。
- **人回复**：人类待确认 H<n> 被回复 → 直接唤醒 blockedOn 指向它的所有工作项的负责人（不等机长）；该项自动从 blocked 回到 active 并设 nextCheckAt=now。
- **限流**：检查点唤醒归「系统」优先级；全机并发会话数设上限（按额度，可配），超了排队。人类唤醒不应被全机限流挡（现有 bug，P1 修掉）。

## 6. 卡住只卡那一件 **[人]**

- 标 blocked 必须同时：①关联一条人类待确认（或另一个工作项）；②回答「其余部分能否先推进」——能则拆出子工作项（parent = 原项）继续派。
- blocked 项不计入该 session 的负载；Jev 派活时视其为空闲。
- 等人超时：24h 置顶到人的清单；72h 问「还要吗」。
- **默认方案**：提问时可附「X 小时无回复按 A 办」，仅限可撤回事项；生产/花钱/隐私/凭据一律不可默认（§13 Q2：谁判定「可撤回」）。

## 7. 调度层（Jev）**[人]**

### 7.1 定位
Jev 是机长的一部分。置信度中/高直接执行，**群里不留痕**；低 → 交机长按老办法处理。未配置/调用失败 = 低置信度。本机诊断日志记每次判断（输入摘要、答案、置信度、耗时），不记 key，不上传。

### 7.2 四个决策（官方 API：`POST /v1/systemone`，`choice`/`score`/`noul`，choice/score 带 `confidence`∈[0,1]）
| 决策 | 问题 | 状态（state） | 执行 |
|---|---|---|---|
| 入口 Todo 派给谁 | `choice`：captain / 每个活跃 session 一项 / new_session / new_crew | Todo 原文 + 活跃名单（角色、名下工作项标题、忙闲、最近活跃**相对排名**而非时间戳） | 绑定并唤醒；new_session → crewd 起 worker，首条唤醒 = Todo 原文 + 机组上下文；**new_crew → 交机长执行**（建组要起名/写章程，Jev 不能生成文字）**[推]** |
| 人类消息谁被叫醒 | 每个成员一道 `noul` | 消息 + 最近几条 + 名单与分工 | 叫醒集合 = p≥阈值；空集 → 机长。**可见性不变**（全员可读），Jev 只决定「谁现在醒」——对齐记忆：broadcast 管看得见、session 管谁醒 |
| 随手消息归属 | `choice`：最近 N 条消息 + 未完成入口/工作项 + none | 消息 + 候选摘要 | 写 reply_to / 关联工作项 |
| 侧边栏排序 | 每机组一道 `score`（紧急度） | 机组摘要 | 与代码算的最近活跃合成排序键 |

### 7.3 防坑（来自官方 jev-1.13 已知局限）
- 偏向第一个选项 → 每次随机打乱选项顺序。
- 无关内容降准确率 → 每个决策只给它需要的 state，名单截到最近活跃前 ~15。
- 不懂日期/计数 → 时间、计数全由代码算，给 Jev 的是排名/区间词。
- 会被注入带偏 → Jev 结果只驱动可撤回的「叫醒/归类/排序/派活」，不驱动任何外部副作用。
- **中文未在文档中声明** → P1 先做实测（需要 key）。阈值（低置信度线，默认 0.5；noul 阈值）由实测定。

### 7.4 配置
设置页：baseURL（默认官方）、key（Mac 钥匙串；crewd 跨平台后 key 存放位置 → §13 Q7）、模型名（默认 `jev-latest`）。

## 8. 行政处 **[人]**

### 8.1 编制
每个**根机组**一个行政处（管整棵子树），按需启动的临时 session，新角色 `admin`。

### 8.2 业务 vs 行政
机长 = 业务：做什么、怎么做、做到什么程度、优先级、验收。
行政 = 运转：账目、编制、督办、档案、人的待办。**无业务决策权**：只能基于事实关闭（已完成/重复/被取代，附证据），不能以「不值得做」关闭。

### 8.3 职能 × 执行者
| 职能 | 代码 | Jev | 行政 session |
|---|---|---|---|
| 前台/调度 | 名单、阈值、打乱 | 全部判断（§7） | — |
| 人事 | 活跃名单（最近活跃、负载）、并发上限 | 「该 session 是否可下线」 | — |
| 督办 | 计时器、持久队列、停下即检查 | 「回报是真进展还是原地踏步」「是否卡死」 | 需要协调时（谈换人、拆项） |
| 档案 | DAG/链接完整性 | 发现疑似过期/重复/已完成 | 核实（git/群聊）并带证据关闭 |
| 秘书 | 等待时长、提醒节奏 | 合并同类、排紧急度 | 写给人的简报 |
| 排序 | 最近活跃 | 紧急度打分 | — |

### 8.4 运行
- 巡检触发：子树内每 10 条消息 / 定时（默认 30min）/ 人手动按钮 **[人]**。
- 巡检 = 代码检查 + 一次 Jev fan-out 调用；**只有标出需要动手的事才启动行政 session**，处理完下线。
- 行政 session 工具：读全子树账本、带证据关闭、改链接、因失联重派、写人类清单简报。没有强制指派（那是机长权力）。

## 9. MCP 工具（P1，取代现有 Todo/计划/续跑相关工具）
`claim_todo(#N, items[])`、`work_item_create`、`work_item_update(id, status?, next_check_at?, evidence?, note)`、`work_item_block(id, on, can_continue_split?)`、`work_item_transfer_request/accept`、`work_item_assign`（机长）、`my_work_items`、`request_help(text)`（任何 session 发子工作项交调度层——「同事无限」的入口）**[人]**。
删除：`continue_work`、`confirm_todo_sweep`、`plan_*`、`respond_todo` 的状态参数（保留为纯回复）。其余现有工具（群聊、目录、ask、会话管理、机组管理）在 crewd 内重实现，语义不变。

## 10. 迁移（一次性，首启）
- 计划板 plan.json 每条 → 工作项，负责人 = 机长。
- 未结入口 Todo：最后 in_progress 回复者仍可恢复 → 建占位工作项归它，nextCheckAt=now（首次检查时被要求整理）；否则留待认领，行政巡检捞。
- 删除：todo-sweep.json、session-continuations.json 的语义（文件留作备份不读）。

## 11. UI（最小）
- 入口 Todo 行内显示「→ W3 W4（2/3）」与负责人。
- 人的待确认清单：按紧急度排，显示已等时长。
- 机组页一个「行政巡检」按钮。设置页 Jev 一节。
- 工作项 DAG 不做专门可视化（人说看不完；需要时点进去看列表）。

## 12. 测试
- crewd 单元：DAG 不变量、换负责人规则、检查点调度（假时钟）、持久队列重启不丢、停下即检查与防死循环、入口状态推导、迁移导入（用脱敏样本，**不用真实账本**——仓库公开）。
- Jev：录制真实响应做夹具；中文实测脚本单独跑（不进 CI）。
- 契约：Swift 客户端与 crewd 对同一 schema 做往返测试。
- 端到端：§1 验收判据逐条在真实机组跑；iOS 真机。

## 13. 待讨论 / 未决
- **Q1 多父机组归哪个行政处？** 选项：按最早的根 / 每个根都管（重复劳动）/ 人指定。倾向：最早的根，人可改。
- **Q2 「可撤回」谁判定？** agent 自报不可靠。倾向：默认方案到点后仍走 agent 正常权限审批；若动作触发审批则不自动执行。
- **Q3 机长是否仍在群里看到所有人类消息？** 可见性全员不变，但不再被每条叫醒。机长靠什么保持对业务全局的了解——巡检简报？待和人确认。
- Q4（已自决）退出 ≠ 失联，先 resume。
- **Q5 跨机组依赖**：v1 不做，是否够用？
- **Q6 存储换 SQLite**：需人知情（单写者后合理；但这是基础改变）。
- **Q7 Jev key 存放**：crewd 跨平台后不能只靠 macOS 钥匙串；Linux 上用什么（文件 600 / secret service）。
- **Q8 发给 Jev 的内容是否需要过滤**（群聊可能含凭据片段）。倾向：发前做一遍凭据正则脱敏。
- **Q9 机长分工**（人说「等下来具体商量」）：机长自己能否动手。与 Jev 派活相关：Jev 选项里有 captain，意味着机长仍可接小事——这本身就是一个回答，需人确认。
- **Q10 语言选型** 待调研。
- **Q11 iOS 安全信道**：外部 PSK 若服务端语言不支持，配对协议要换（如配对时交换自签证书指纹做 pinning）。
- **Q12 P1 期间 Swift 执行器与 crewd 两个常驻进程**：启动/升级顺序、谁拉起谁。现状 daemon 由 GUI 拉起、无登录项。倾向：GUI 拉 crewd，crewd 拉执行器。
