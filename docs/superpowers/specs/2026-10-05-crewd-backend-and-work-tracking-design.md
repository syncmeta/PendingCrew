# crewd：独立后端 + 工作跟进机制 设计

> 写给实现者（Claude）看的，不是给人看的汇报。基准：`main@5fa34b7`（2026-10-05）。
> 来源：2026-10-05 与人的设计对话。人的原话/拍板标 **[人]**，我的推断标 **[推]**，读代码读到的标 **[码 file:line]**。
> 状态：定稿（2026-10-05）。Jev key 在本机钥匙串 service=`pendingcrew-jev`，2026-10-05 合成中文样例实测通过（choice 0.99，~0.95s 含网络）。

## 0. 一句话

把「数据 + 协作调度」从 Swift app 里拿出来，做成独立、跨平台的后端进程 **crewd**；第一个在 crewd 里落地的功能是**工作跟进机制**（两层 Todo、负责人、检查点、Jev 调度、行政处）。Mac/iOS 都只是 crewd 的客户端。

## 1. 目标 / 非目标 / 判据

目标
- G1 布置下去的事一定有人跟进，不靠 session 记性，不靠人催 **[人]**。
- G2 卡在人身上只卡那一件，不卡 session **[人]**。
- G3 高频小判断（派给谁、谁收消息、归到哪条、侧边栏排序）由 Jev 做，机长只接低置信度 **[人]**。
- G4 前后端解耦，后端跨平台，Mac 与 iOS 平等地是客户端 **[人]**。
- G5 iOS 正式重启（08-23 的「当没有 iOS」作废），与 iPadOS 一起做 **[人]**。

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
调研（2026-10-05，官方仓库/registry 核过）：
| | Go | Rust | TS/Bun |
|---|---|---|---|
| 无头终端仿真 | charmbracelet/x/vt（年轻、无 tag） | alacritty_terminal 0.26 / vt100（成熟；codex 自己用 vt100） | @xterm/headless 6.0（VS Code 在用，标 experimental） |
| PTY | creack/pty（Unix）+ go-pty（ConPTY） | portable-pty 0.9（含 ConPTY） | Bun 内置 PTY 有未修 bug：^C 不发信号、读无背压；node-pty 在 Bun 下 onData 不触发 |
| 官方 MCP SDK | go-sdk v1.8 | rmcp 3.5 | v2.3 |
| 外部 PSK TLS | 不支持 | 不支持 | 不支持 |
| 分发 | 交叉编译最容易 | 需各目标工具链，体积最小 | 一机交叉编译，60–100MB |
推荐 **Rust**：M2 最难的终端仿真只有 Rust 有久经考验的库；codex 本身 Rust，协议对齐方便；强类型对 agent 写代码是额外护栏。代价是迭代慢、tokio 异步复杂。Go 次选（协作部分最快，但终端仿真要自己兜 bug）。Bun 排除（PTY 的 ^C bug 直接打到驱动 claude）。**[人 已定 2026-10-05] Rust。**

### 3.2 进程与职责（终态）
- `crewd`：常驻。拥有全部持久状态与调度；对客户端提供 API；对 agent 提供 MCP（`crewd mcp` 子命令作为每个 session 的 stdio MCP helper，与今天「app 二进制再跑一遍」同构）；跑会话（PTY / codex app-server）。
- Mac app、iOS app：纯客户端，经同一 API。Mac 本机走 Unix socket，iOS 走配对后的加密直连。

### 3.3 构建顺序（全 Rust，无过渡桥）**[人 已定 2026-10-05]**
不做「crewd ↔ Swift 执行器」的桥（白写、双常驻进程）。crewd 独立开发到主路径对等后一次性切换；期间现有 Swift 版照用、只修 bug。
- **M1 地基**：Rust workspace、SQLite 存储、旧 JSON 只读导入、客户端 API（Unix socket）+ 契约 schema。
- **M2 执行器**：PTY（rustix-openpty 手写 + tokio，选型见 §18）+ alacritty_terminal 无头终端（屏幕文本、快照、对话框识别）、claude 启动参数与 hooks、codex app-server 客户端、孤儿回收、会话恢复（--resume）。
- **M3 协作**：机组/白板/人类待确认/审批台账、`crewd mcp` 与 `crewd hook` 子命令、唤醒投递与准入。细分见 §22（M3a 数据与 API → M3b agent 面 ∥ M3c 编排 → M4b）。
- **M4 工作跟进**：两层 Todo、负责人、检查点、停下即检查、卡住拆分（§4–6、§9）。
- **M5 Jev 调度**（§7）：**M5a** 客户端 + 中文实测（已交付，§20）→ **M5b** 接进调度（依赖 M3 的群与成员、M4 的工作项）。
- **M6 行政处**（§8）。
- **切换**：Mac 客户端改连 crewd（去掉所有直接读写 JSON 与 Swift 后台）；验收 §1 判据；Swift daemon/MCP helper 代码删除。
- **iOS + iPadOS**（人 2026-10-06：两端一起做，同一个 app、同一套客户端代码，iPad 按大屏布局适配——分栏 / 多窗口，不是放大的手机）：M3 的 API 定型后即可并行，TLS1.3 + SPKI pinning 重新配对（Q11）；验收在 iPhone 与 iPad 真机各跑一遍。
每个 M 都以 crewd 自己的集成测试 + CLI 可驱动为完成标准，不依赖 Swift UI。

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
- **限流**：检查点唤醒归「系统」优先级；全机并发会话数设上限（按额度，可配），超了排队。人类唤醒不应被全机限流挡（现有 bug，M3 修掉）。

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
- 偏向第一个选项 → 每次按种子打乱选项顺序（种子由这次的输入算出：同一输入同一顺序，诊断日志可复现；不同输入的顺序近似均匀）。
- 无关内容降准确率 → 每个决策只给它需要的 state，名单截到最近活跃前 ~15。
- 不懂日期/计数 → 时间、计数全由代码算，给 Jev 的是排名/区间词。
- 会被注入带偏 → Jev 结果只驱动可撤回的「叫醒/归类/排序/派活」，不驱动任何外部副作用。
- **中文未在文档中声明** → M5 先做实测。阈值（低置信度线，默认 0.5；noul 阈值）由实测定——M5a 读数与取值见 §20。

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

## 9. MCP 工具（M3–M4，取代现有 Todo/计划/续跑相关工具）
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
- ~~Q1~~ **[人 已定]** 多父机组归最早的根的行政处，人可改。
- ~~Q2~~ **[人 已定]** 默认方案到点后照常走权限审批；一旦触发审批框就不自动执行，转回人。不靠 agent 自报「可撤回」。
- ~~Q3~~ **[人 已定]** 机长照样看得到全部消息（下次醒来读未读），只是不被每条叫醒；另由行政巡检定期给机长简报。
- Q4（已自决）退出 ≠ 失联，先 resume。
- ~~Q5~~ **[自决]** v1 不做跨机组 DAG 边；遇到真实需要再加。
- ~~Q6~~ **[已告知人，无异议]** SQLite，crewd 唯一写者。
- ~~Q7~~ **[自决]** macOS 用钥匙串（security-framework crate），Linux 用配置目录下 0600 文件；key 只由 crewd 读取，客户端只能写入不能读回。
- ~~Q8~~ **[自决]** 发前做凭据正则脱敏（key/token/私钥/口令形状），脱敏命中计数进诊断日志。
- ~~Q9~~ **[人 已定]** 机长可接活（Jev 派活选项保留 captain）。
- ~~Q10~~ **[人 已定]** Rust。
- ~~Q11~~ **[调研 已定]** Go crypto/tls、rustls、Bun 均不支持外部 PSK。改为 TLS 1.3 + 配对时交换自签证书 SPKI 指纹做双向 pinning（iOS 用 Network.framework verify block）。现有 TLS-PSK 配对作废，需重新配对。
- ~~Q12~~ **[自决]** GUI 拉起 crewd（执行器在 crewd 内）；无登录项（维持 2026-09-07 人的决定）。升级 **[人 已定 2026-10-06]**：重启/升级打断会话是预期内的，要求只是**能恢复**（claude `--resume`、codex `thread/resume` 自动接回）。「升级不中断」是加分项、后做：M2 执行器在 crewd 进程内，但只经一个窄接口（启动/输入/停止/状态订阅/屏幕文本）与其余部分交互，以后把它拆成独立常驻子进程时只换接口的传输层。
- Q13 **[待人拍板]** Jev 默认阈值是否按 M5a 实测改（§20）。实测按计划的规则推荐 choice_min 0.30 / score_min 0.90 / wake_yes 0.70（四个决策都给出了推荐值，计划的「改默认值」条件成立），但执行时改代码这一步被 runner 的自动审查拦下，默认值仍是 0.5。选项：A 照推荐值改；B 维持 0.5（wake 在 0.5 下 82%，见 §20）；C 先扩大用例集（尤其 wake 全对的区间只有 (0.68, 0.75]）再定。注意：这批读数用的是有含义的英文选项名，会抬高准确率；无论选哪项，M5b 换成不透明别名后都要先重测（§20）。

## 14. M1 交付记录与遗留（2026-10-05，分支 crewd-m1）

已交付：Rust workspace（proto/store/legacy/server/crewd）、SQLite schema v1、旧 JSON 只读导入（单事务；附件先拷后提交；源/目标重叠拒绝；I/O 错误致命可重试；解析不了或没落库的文件原样归档；报告存 meta.importReport）、Unix socket NDJSON RPC（hello/crew.list/whiteboard.list/post/todo.list/events.subscribe）、单实例锁（serve 与 import 共用）、数据目录 0700、契约文件 `crewd/schema/crewd-api.schema.json` + 漂移测试。真实数据只读演练：62 机组 / 1009 session / 366764 消息 / 500 Todo，0 跳过。

遗留（M2 起处理）：
- ~~Q12 自相矛盾~~ **[人 已定 2026-10-06]** 见 §13 Q12：先保证可恢复，不中断后做。
- 契约还不能直接给 Swift 生成代码：结果/事件类型是字符串描述、缺 ServerFrame、每个类型各自一份 `$defs`、参数结构没 deny_unknown_fields、枚举是字符串。iOS/Mac 客户端接入前做一轮契约整理。
- store 的 SQLite 调用在 tokio 线程上同步执行（M2 PTY 进来前改成专用线程/actor）。
- `crewd serve` 不做源/目标重叠检查；悬空符号链接数据目录报错信息不友好；归档遍历会跟随符号链接目录、遇 FIFO 会阻塞。
- 订阅重订窗口可能重复投递一条事件；对端不读时连接任务可能挂住；`crewd rpc` 无超时。
- 其余小项见各任务评审（upsert 以 None 覆盖 extra、chrono 默认特性等），不影响正确性。

## 15. M2a 交付记录与遗留（分支 crewd-m2a，计划 `docs/superpowers/plans/2026-10-06-crewd-m2a-terminal-core.md`）

已交付：`crewd/crates/term`（crewd-term，纯计算，不起进程、不用 tokio）——alacritty_terminal 0.26 无头终端（`Processor<NoSync>`、终端回复发件箱、OSC 10/11 用配置色、每个 `?1004h` 补 `ESC[I`、旁路跟踪滚动区域）、TerminalScreenText 口径读屏、AnsiPlainTextTail（UTF-8 跨块拼接）、菜单解析（编号式 + 光标列表式）、输入框判据、PendingDecisionTracker（3s、按指纹粘住、时间由调用方传入）、快照编码器（逐格往返：回滚 + 屏、SGR + OSC 8、软换行含竖向缩小后的悬空底行、宽字符、制表符、滚动区域、模式、光标含 pending-wrap、当前画笔）。验收：4 个夹具上的 13 条期望；夹具每批往返、resize 往返、快照后接着喂两边一致。

评审修复轮带来的、计划原文里没有的改动：
- Task 2：同尺寸 `resize` 不重置旁路跟踪的滚动区域（与 alacritty `Term::resize` 提前返回一致）；DECCOLM（`?3h/l`）按 alacritty 把滚动区域复位；`?1004h` 被切在两次 `feed` 之间时仍恰好答一次 `ESC[I`。
- Task 7（快照编码器）：软换行续行若在 `cols` 之前结束，行尾补默认底色 + `ESC[K`（折行上滚会把整行刷成触发那一格的底色，BCE）；`row()` 跟踪终端光标应在的列，和源格子的列对不上时补 CHA；缺后半格的 WIDE_CHAR 之后用默认底色、`CHA col+2`、`ESC[X` 把自带的后半格擦回去；光标显隐一律显式写 `?25h` / `?25l`；OSC 8 超链接在切换到另一条链接之前先关、每行末尾也关。

遗留（M2b–M2d 处理）：
- 快照带不走解析器的半截状态：PTY 块切在 UTF-8 字符或转义序列中间时拍快照，接在后面的实时字节在客户端会坏掉一个字符或一个序列。M2d 定 attach 协议时二选一：只在解析器处于 ground 状态时取快照，或把半截字节随快照一起交给客户端。
- `Snapshot` 没带回滚行数（`history_size`）：消费方要自己从 `bytes` 推，或 M2d 在帧里补这个字段。
- 快照不还原：处在 alt 屏时的主屏（alacritty 不公开非活动缓冲区；测试钉成显式断言）、DECSC 存的光标、自定义制表位、G0–G3 字符集、窗口标题、kitty 键盘模式栈；不足 3 行时的悬空底行折行（区域至少要两行）；reflow 后行尾 `LEADING_WIDE_CHAR_SPACER` 而下一行不以宽字符开头时的该标志。夹具里只有 claude 启动最开头的 DECSC / `CSI r` / DECRC（字节 0–6），其余几项都没出现。
- 宽字符与折行的几处已知缺口（夹具里都没出现，合成测试之外没验）：
  - 最后一列放着没有后半格的 WIDE_CHAR（ICH 挤出来的）：回放里它再也放不进去，会把后面整行错位一行；
  - 软换行行的最后一列是孤立的 WIDE_CHAR_SPACER：这一行的软换行标志丢失；
  - 软换行行的最后一列是制表符（reflow 之后才会出现）：同样丢软换行；
  - 制表符格上挂的零宽字符被丢弃。
- 快照只验过 alacritty → alacritty（同一套解析器喂回去逐格比）。SwiftTerm 这个真正的消费方没验：冒号分隔的 SGR 子参数、OSC 8 的 `id=` 参数、滚动区域内的 SD、SwiftTerm 自己在有回滚时 OSC 8 范围的 bug，都是 M2d 接 SwiftTerm 前要实测的点。
- 快照会重新写入 `?1004h`，SwiftTerm 视图在 attach 时看到它就会发 `ESC[I`，这个字节会一路进到 PTY：M2d 要决定转不转发（claude 已经被 `Screen` 补过一次焦点进入，再来一次是重复的）。
- 悬空底行（2b 路径，竖向缩小后最底一行的软换行）下，制表符落在第 2 行第 0 列的情形没处理。
- 字节尾窗（`AnsiPlainTextTail`）把 CR 与 LF 各变成一个 `\n`（与 Swift 一致，已用单测钉住），而生产 PTY 输出行尾是 CRLF，行与行之间会多出空行、选项块不跨空行——所以字节尾窗源永远认不出多行菜单，屏幕源才是唯一能用的那条。要不要把 CRLF 折成一个换行是 M2b 的决定，M2a 不改行为。
- DA1/DA2 的回答串与 SwiftTerm 不同（alacritty 回 `ESC[?6c`）。这只在录制字节上验过，没有和真 claude 交互过——M2b 接上 PTY 后真机核一次 claude 的启动与主题选择。
- 群消息渲染（`SessionDecisionNotice`，按 `numbered` 分叉、绝不编造编号）归 M3。

## 16. M2 细分与 ACP 第三条后端（2026-10-06）

M2 拆为：**M2a** 终端核心（已交付，§15）→ **M2b** claude 会话驱动（PTY 启动、投递、健康、生命周期、resume）→ **M2c** codex app-server 客户端 → **M2d** attach 协议（快照/实时字节/codex 结构化历史给客户端）→ **M2e** ACP 通用后端。

**ACP 定位**（沿用 2026-09-09 人的决定 A：「claude 和 codex 留现在的，ACP 是额外扩展接其它的用」；2026-10-06 复核官方现状后结论不变）：
- claude 不走 ACP：会丢 TUI（选择/回滚/重排）、PostToolUse 白板中途注入无对应、v1 无中途 steer；且 `claude-agent-acp` 基于 Agent SDK，官方写明未经批准第三方不得提供 claude.ai 登录——订阅登录经适配器有政策风险（待确认，未核实）。
- codex 不走 ACP：`codex-acp` 只是包了 app-server，反丢 `developerInstructions`、`additionalContext` 不可信包装、`turn/steer`。
- 其它 harness（opencode、gemini `--acp`、qwen-code `--acp`、cursor-agent、goose、kilo、copilot、Kiro 等，registry 41 家）走 ACP v1（`protocolVersion:1`，stdio JSON-RPC），客户端用 Rust crate `agent-client-protocol` 2.2；内部事件按 v2 的 idle/running/requires_action 建模，v2（draft）放 feature flag 后。
- 位置：新 crate 实现 §13 Q12 的同一窄执行器接口；「屏幕文本」由结构化 transcript 代替，走 M2d 给 codex 定的结构化历史 attach 路。每家 = 一份 `AgentSpec`（registry 条目 + 我们的覆盖）；世界观（system prompt）三态：有口子 / 明确没有 / 未确认则拒起（ACP 无标准入口，无 RFD）。
- 存储：session 的执行器类型字段设计为 `claude` / `codex` / `acp:<registry-id>`，M2b 起就按此建，避免日后迁移。
- 已知坑（09-09 实测）：`mcpServers[].env` 不继承父环境；探针须自动放行审批否则全被读成挂不上；copilot 静默不挂 MCP、cancel 后报 end_turn；MCP 是否真挂上必须用我们这侧的标记验证。
- M2e 起步 spike：Rust 重写普查器（initialize / new+MCP 标记 / prompt / cancel / load），先跑 codex-acp 当对照尺（证明会绿），再跑 opencode 当目标，再重测 gemini / qwen / copilot（09-09 读数已过期）。
- 风险：世界观缺口；各家 mode id / cancel 语义不一；每 session 多一个 node 进程（并发资源未量）；各家登录态不一；v2 仍在变。
- **ACP 可复用的社区件（2026-10-06）**：官方 Rust SDK `agent-client-protocol` 2.2（Apache-2.0）做客户端主循环；同仓库 `-polyfill`（把 MCP-over-ACP 适配成 HTTP，可能解决 MCP 挂载与 env 不继承）和 `-conductor`（代理链，可作注入世界观的模式）M2e 评估；registry（Apache-2.0）的 FORMAT.md 照着在 Rust 里实现安装/启动（binary 带 sha256 / npx / uvx），其每日 `.protocol-matrix` 普查当第二把尺子；acpx（TS/MIT）的会话持久化/排队/审批语义作设计参考；Agmente（Swift/MIT）的 `ACPClient` / `AppServerClient` 包可评估用于 Mac/iOS 的结构化会话视图（ACP 与 codex 都适用）。Zed / Toad / agent-shell 是 GPL/AGPL，只学不抄。

## 17. 机组内多群（2026-10-06，人提出并拍板，归 M3）

**问题**：一个机组只有一块白板 = 一个大群，什么都塞进去；想分开谈只能 `create_child_crew`，把「组织单位」和「对话空间」混成一个概念。每个 session 都收整个大群的未读，噪音大、白费上下文。

**模型**：**机组 = 组织**（成员、机长、工作目录、Todo）；**群 = 对话**（一个机组可以有很多个）。
- 每个机组有一个**大厅**（全员在，放公告与入口消息）。
- **任何 session 都能建群、拉人**；群有名字和用途（「发版」「IP 日志」「scroll bug 排查」）。私聊 = 两人的群。
- **跨机组拉人 [人 已定]**：群归属一个机组，成员可来自别的机组；这个群**同时显示在每个成员所在机组下**。可取代现在 `contact` 的跨组消息和一部分「为了说话而开的子机组」。
- **只有群成员被唤醒/被注入该群消息**：session 的上下文只装它所在的群。
- **工作项不自动建群 [人 已定]**：需要协作时负责人自己建，可把群关联到工作项；换负责人时把新负责人拉进关联群；工作项结束后行政处归档闲置群。

**人类与群 [人 已定]**：
- **每个根机组预置一个「人类群」**：人类参与，相当于以前「仅看 @人类」的过滤视图——要找人类、要人拍板的东西发这里（人类待确认清单的入口也在这里）。
- 人类**可以进任何群发言**，但 session **不能默认人类在看**任何群。
- 人类可以把某个群标成**「我在注视」**，让 agent 知道「找人类该发到哪」；**默认不注视**。agent 要找人类时：发到根机组人类群，或发到人类正在注视的群。
- 世界观提示词要写清这条规则；MCP 工具提供「人类现在注视哪些群」的查询。
- **@人类 = 叫人 [人 已定 2026-10-06]**：任何群里 @人类，该群在人类界面上显示红色「叫你」标记（类似微信被@），不论是否注视；人类看过该条后标记消失。
- 但 agent 必须清楚：**在人类没有注视的群里 @人类，相当于对方开着免打扰时叫他**——标记会亮，但人类可能很久才看到。急事或要拍板的事应发到根机组人类群（或写成人类待确认项）。为此，在未注视的群 @人类 时，工具回执要明说「人类未注视此群，@ 已标红但可能延迟看到；急事请发人类群」，而不是只回「已发送」（记忆：回执照回「已发到」误导过）。

**与其它机制的关系**：
- Jev（§7）判「人类消息叫醒谁」时只在该群成员里选；人类在大厅随手发的消息，Jev 还可判「它属于哪个群」（判错代价低，可直接执行；具体行为在 M5 定）。
- 行政处（§8）多一项职能：归档闲置群、提醒「这个群的事已经没人管了」。

**数据**（M3 schema v2）：`channels(id, crew_id 归属, name, kind: lobby|human|topic|dm, purpose, created_by, archived_at)`、`channel_members(channel_id, member_kind: session|human, member_id, joined_at)`、人类注视标记 `channel_watch(channel_id, since)`；`messages.channel_id` 取代 `crew_id` 作为归属（M1 导入的旧消息迁到各机组大厅）；`work_items.channel_id?`（M4）。

**待 M3 设计时细化**：群内是否需要「话题/回复串」（暂用已有的 reply_to）；子机组还保留哪些用途（独立工作目录/独立机长）。人 2026-10-06：按话题 / 按任务的协作用群，不再新开子机组；子机组只表达组织架构（部门 / 小组）。

## 18. M2b 交付记录与遗留（分支 crewd-m2b，计划 `docs/superpowers/plans/2026-10-06-crewd-m2b-claude-session.md`）

（编号说明：计划写成时本节叫 §17；§17 后来被「机组内多群」占用，§19 / §20 是别的分支的 M4a / M5a 记录。计划正文与代码注释里的「spec §17」一律读作 §18。）

已交付：`crewd/crates/exec`（crewd-exec）——找 claude（登录 shell PATH + 哨兵 + 5s 超时、childProcessPath、白名单环境）、argv 与 0600 启动文件（首条 prompt 绝不进 argv）、hooks / MCP 命令行指向可配置 helper（`HelperCommands` 一处拼；hooks 可关，等 M3 的 `crewd hook`）、手写 PTY（rustix-openpty + tokio `AsyncFd`，setsid / TIOCSCTTY / env_clear）、sans-IO 会话核心 `session::Core`（投递状态机、健康、待决策、退出，时间注入）、驱动任务与窄接口 `driver::SessionHandle` + `SessionEvent`（含 `StartError::AlreadyRunning`：同一个 claude 会话号不能在本进程里同时起两个）、孤儿登记与启动时刻核对的回收（macOS / Linux；身份含开机标识 `boot_id`，核对启动时刻 + 进程组 + boot_id）、会话号账本 trait + `crewd_server::ledger::StoreLedger`（会话号入库前校验字符集与长度）、续跑被拒换新号重起一次。`crewd claude` 子命令（前台起一个会话，事件打到 stdout；`--session` 走库里的账本、重跑自动 `--resume`，不给走内存账本；开场任务只经 `--prompt-file PATH`（`-` = 从 stdin 读到 EOF）给，不收命令行上的正文——`ps` 对本机所有进程可见）；`crewd serve` 与 `crewd claude` 拿到数据目录锁后都先回收孤儿（`crewd_server::reap_orphans`），结果逐条打到 stderr，且如实：杀成功 / 已不在 / pid 被复用没动（登记作废、删掉）/ 读不到身份没动 / 判定该回收但被拒绝（pgid ≤ 1 或是自己所在的组）/ 发信号失败——后三种登记仍留在 `exec-registry.json` 里，并补一行「有 N 条没能处理」，不会被「回收完成」盖住。schema v2：`sessions.agent_kind` → `executor`、`crews.captain_agent_kind` → `captain_executor`（词表都是 `claude` / `codex` / `acp:<registry-id>`，旧数据的 `terminal` / `unknown` 原样）；线上 JSON、契约文件、导入器同步。验收：假 claude（真 PTY）上的端到端——慢首屏、粘贴折叠、排队、粘贴陷阱、永不回显、真录制信任框回放后代答补投、限流菜单自动应答、额度用尽退出、子进程不读输入、70KB 开场任务；shell 脚本顶替 claude 的驱动层测试——落账、续跑、被拒重起、停止升级 killpg、拉起失败、登记表、认证探测、账本失败不起进程；`crewd claude` 两次运行之间经库续跑（`tests/claude_cli.rs`）。手动用假 claude 从命令行驱动过一遍（stdin 投第二条指令、`--screen` 在 stderr 看到 `GOT: 第二条指令`、SIGINT 后 `Exited(… stopped_by_user: true …)`）；这一遍抓到一个只有交互终端才会撞上的毛病：`crewd claude` 的 stdin 读取占着 tokio 阻塞线程，Ctrl-C 之后 runtime 收尾会等到用户再按一次回车。最初的修法是 `main` 在 `run()` 成功后直接 `exit(0)`（对所有子命令生效）；终评修正里换成在一条普通线程上读 stdin、经通道交给会话循环，`main` 正常返回退出码（`tests/claude_cli.rs` 钉住「会话结束、stdin 一直开着也马上退出」）。

与 Swift / 本 spec 原文不同的决定（理由见计划「裁定」）：
- PTY 手写，不用 portable-pty（§3.3 原写法，已改）也不用 `alacritty_terminal::tty`（继承父环境且加 `ALACRITTY_WINDOW_ID`、cwd 失败静默、Drop 里阻塞 wait）。
- §15 的 CRLF 问题：字节尾窗不改；执行器判待决策只看屏幕（不喂跟踪器的尾窗）。折掉 CRLF 会让答完的菜单在尾窗里挂着，把「已清除」拖住。额度 / 限流 / 续跑被拒这些单行短语仍用尾窗。
- 输入行连续在屏 300ms 才算就绪（且行文本要稳定）：`tui-claude-trust.bin` 按 64 字节喂，第 1024 字节处信任框半帧里 `❯  No, exit` 被认成输入行。
- 被对话框挡住 / 等不到输入框只报不终止，答完自己补投（Swift 驱动层把这两种当终局、丢了开场任务——与它自己状态机的测试矛盾）。
- 提交三拍发完才回读（粘贴占位符会把那个空格算成多一行）；正文里的 CR 变 LF、其余控制字符变 Unicode 控制图形；TERM 固定 `xterm-256color`；额外环境变量多拒一个 `CREWD_` 前缀。
- 续跑被拒同时看屏幕和字节尾窗（窄屏时那行原话会折行）。
- 孤儿启动时刻在 macOS 上取 `proc_pidinfo(PROC_PIDTBSDINFO)`（与 sysctl `KERN_PROC` 同一个内核字段；libc 没给 macOS 导出 `kinfo_proc`）。

控制器裁定（执行中定的，覆盖或补充计划；每条一句）：
- 计划里的「spec §17」一律改 §18（§17 在计划写成后被占用）。
- 把 `crews.captain_agent_kind` 一并改名 `captain_executor`、同一次 schema v2 迁移、同样的值映射——crews 与 sessions 共用一套词表，免第二次迁移；取代计划「遗留」里「captain_agent_kind 不动」那条。
- 最初裁定「不跑真 claude 冒烟、不跑 Linux 编译（要人点头 / 装工具链）」，之后人点头（2026-10-06），两件都跑了，结果见下。
- Task 1：登录 shell 抓 PATH 放进自己的进程组、读到收尾哨兵就停、成功 / 超时都 killpg（计划等 EOF：约 1/135 次挂住，且 rc 留下后台作业时丢 PATH）。
- Task 1：登录 shell 再进一步放进新 session（`setsid`），只换进程组在有控制终端时会让交互 zsh 卡住。
- Task 4：`next_deadline` 永不早于上次的 now；输入行过了期限还没出现就一次性报「卡住」并设 90s 终止上限；`ready_settle` 要求行文本稳定；截止时间运算全部饱和（计划自带的缺陷）。
- Task 5（对 Task 6 的裁定）：限流自动应答必须以「屏幕当下显示限流菜单且光标在『Stop and wait』上」为闸——只靠字节尾窗的短语会在冷却后过期、或匹配到回显 / 别的菜单，把「1」+ 回车敲进不该敲的地方。
- Task 6：排好的提交优先于丢弃（对话框出现 / 输入行消失时）；限流回车只在菜单仍在屏上时发；正文去毒一次、回读对着去毒后的文本；有排好的按键时 `next_wakeup` 不得落在过去；限流菜单在屏上就应答 + 冷却；健康优先级 LaunchFailed > 额度 / 限流 > 投递，`quota_at` 独立；停止后再 `send` 记 Abandoned（计划自带的缺陷，其中一条会替人批准权限提示）。
- Task 6：未确认的扫描器命中在一个冷却内仍有效（实现者疑虑 1，评审认可；安全靠屏幕闸）。
- Task 6（第二轮）：被中止的提交通知 Delivery（重发、计入重试）；有对话框时不进 Submit 步；`via_body` 的拍子有测试；额度类健康不被认证状态转换清掉。
- Task 7：回收器必须核对活进程的 pgid 与登记一致、拒绝 pgid ≤ 1 与自己所在的组（计划的 `apply` 可能 `kill(-1)`）；Linux 身份含 boot_id（重启后时钟滴答会撞）；杀失败的条目留在表里。
- Task 9：会话号校验（非空、`[A-Za-z0-9_-]`、不以 `-` 开头、≤128）在账本写入、`decide_resume`（非法 → 新号 + 说明）、`prepare_claude` 三处强制——库里读出的号会当路径段和 argv 用。
- Task 10：杀进程后等待退出必须同时排空 PTY（macOS 上量到过挂死）；停止 / 句柄全丢之后不再重起；文件 I/O 走 `spawn_blocking`。
- 时序抖动：`env::tests::background_job_holding_stdout_does_not_delay_or_survive` 断言墙钟 < 1s，在 `check.sh` 并行满载时偶发失败（早先那次「87/88」不明原因的红就是它）；上限放宽到 3s。修后 `check.sh` 连跑 5 次全绿。

真机核对（§15 遗留的 DA1 一条）：
- 已跑（人 2026-10-06 点头，只跑这一条被忽略的测试）：claude 2.1.291，`CREWD_LIVE_CLAUDE=1 CREWD_LIVE_CLAUDE_WORKDIR=<$HOME 下 mktemp 的空目录> cargo test -p crewd-exec --test live_claude -- --ignored --nocapture --exact real_claude_starts_up_to_the_input_box`，**通过**（3.03s）。约 1.5s 后画出输入框 `❯ Try "how do I log an error?"`；**没有**弹信任框——工作目录特意放在 `$HOME` 下的临时空目录（`$HOME` 已被信任，claude 继承上级目录的信任），**没有用计划默认的仓库根目录**；放在 `/tmp` 下没有受信任的祖先目录，会弹信任框让测试失败、看不出输入框能不能画出来。终评修正把测试的缺省工作目录改成了 `$HOME` 下现建、跑完即删的临时目录（`CREWD_LIVE_CLAUDE_WORKDIR` 仍可覆盖，弹信任框时的报错会提示把它设成已信任的目录）；带 `--ignored` 却没设 `CREWD_LIVE_CLAUDE=1` 改为直接失败并说明原因；停止后等 `Exited` 加了 10s 上限。改后没有再跑真 claude。alacritty 的 DA1 / OSC 11 回答被真 claude 接受（首屏完整画出、没有转义残留；从外面看不到 claude 发了哪几条查询，没有逐条日志）。停止时 claude 自己在 2s 宽限内退出（`Code(143)`，`stopped_by_user: true`）。没发过任何 prompt。副作用：claude 在 `~/.claude.json` 里给那个临时路径加了一条它自己的 project 记录，未改。
- 实测给计划里测试的改动：循环里不再 `assert!`（真 claude 运行时 panic 会让 runtime 在没回收子进程的情况下 drop，留下孤儿），改为跳出并带原因、先停 claude 等 `Exited` 再 panic。

遗留：
- 提交三拍与正文验证窗口都从「排进写任务」起算，不是「真正写进 PTY」：子进程长时间不读输入时，空格 / 退格 / 回车可能挤进同一次 read 被当成粘贴，超长正文也可能在窗口内没写完而被 Ctrl-U 重投。假 claude 上 70KB 开场任务没出过事；真遇到了就让写任务回报「写完了」再排下一拍。
- 只在启动时回收孤儿；登记表交给会话管理、serve 拉起会话是 M3。`crewd claude` 拿数据目录锁，不能与 serve 同时跑。
- 登记表一个 session_id 一个槽位：同号再登记会覆盖留下的（被拒 / 杀失败 / 读不到身份的）孤儿条目；M3 把登记表交给会话管理时要处理。
- 同一个 crewd session id 并发起两次没有挡：`StartError::AlreadyRunning` 只按 claude 会话号挡（同一个 `--resume` 号起不了两份），而账本里还没记号时两次启动会各自生成新号、后落账的覆盖先落账的，登记表的同一个槽位也互相覆盖。M3 的会话管理负责「一个 session 同时只有一个进程」。
- server 的 RPC 仍在 tokio 线程上同步调 SQLite（§14 遗留）；M2b 的账本另开连接、在 `spawn_blocking` 里跑，没加重它。M3 把会话放进 serve 之前处理。
- 续跑被拒只做了判定；Swift 那套诊断文案（日志在不在盘上）与「新开一轮」的群通知归 M3。
- `/model` 中途切换（`SessionProfileSwitch`）、启动参数回显扫描、awaitingReply 归 M3。
- `sessions::upsert` 传 `agent_session_id = None` 会抹掉库里已记的会话号——M3 写 session 行时必须保留它。
- `TIOCSCTTY` 与「退出后读干净」只在 Linux 上有可观测差别，macOS 上的测试证明不了它们（计划 Task 3 / Task 10 有说明）。
- Linux：`cargo clippy -p crewd-exec --all-targets --target x86_64-unknown-linux-gnu -- -D warnings` 通过（人 2026-10-06 点头，工具链 1.94.0 已装该 target；只编译，没在 Linux 上跑测试；覆盖 `#[cfg(not(target_os = "macos"))]` 的 `/proc` 读法与 Linux 才编译的驱动层测试）。终评修正之后重跑，仍通过。

已知小问题（评审时记下、未修；每条一行，按模块分组。标「终评优先」的建议在整支分支的终评里先处理）：

启动与环境（Task 1、2）：
- 相对路径的 PATH 项按当前目录解析。
- `AgentPaths::discover` / `resolve` / `build_env_from_process` 没有直接的测试。
- 额外环境变量可以覆盖 PATH / TERM（留给调用方决定）。
- 登录 shell 测试用 1s 捕获超时，冷启动超过 1s 仍可能偶发（「脚本没写 pidfile」）——终评时留意。
- 非 UTF-8 路径会有损转换。
- 「开场任务不进文件」没有断言。
- 文档里指向 `crate::delivery` 的链接在当时悬空。

PTY（Task 3）：
- ~~终评优先：`spawn_pty` 在 `fionbio` / `AsyncFd` 设置失败时子进程已起、没人管~~（终评修正：已挪到 spawn 之前）。
- 背压与 SIGWINCH 没有提交的测试；并行测试里有 `set_var`。

投递状态机与会话核心（Task 4、6）：
- ~~终评优先：输入行文本一直在变时永远稳定不下来、也不报 NotReady~~（终评修正：`readiness_deadline` 同样约束 settle 等待）。
- Stalled 没有宽限（期限那一刻缺一帧就用掉唯一一次报告）。~~Stalled 的 Undelivered 文案与 `lost_tail()` 的建议矛盾~~（终评修正：改成「没法确认、重发前先看一眼」）。
- ~~对话框在屏与否不影响 Writing / Submitting 的放弃计时~~、~~对话框一直没答时 90s 后终结、下一条拼在后面；提示说「已发 CR」但其实没发~~、~~过期的文档注释~~（终评修正，见下）。
- 扫描器仍在喂但已不再起决定作用（以后删）；额度恢复后被拒的投递报告不会再次抛出；`last_now` 不被 `send` / `stop` / `on_exit` 更新。

健康（Task 5）：
- 额度扫描器对「you've reached your goal」之类句子会误报（6s 连续干活后自愈）。
- 每次节拍都 stat 一遍 `projects/*`；单条超过 256KB 的记录没有测试。

登记表与回收（Task 7、10）：
- 改名后没有 fsync 父目录；落盘失败时内存表与磁盘分叉；领头进程已死时组内其余成员不会被（孤儿回收）回收——驱动层自己起的会话已在退出后整组补刀（见下「终评修正」）。
- `PidReused` 的说明只显示启动时刻。
- 经登记表那条回收路径没有测试钉住；`REAP_LIMIT` 是常量。
- 登记表只收自成进程组的进程（pgid == pid）：回收按组杀，组号不对会杀错人。这条由调用方保证——`driver.rs` 的 `launch` 登记前探一次身份、`identity.pgid == pid` 才登记，否则当登记失败处理（杀掉刚起的进程、不起会话）；`apply_with` 本身不查 `pgid == pid`。

假 claude 器材（Task 11）：
- 限流菜单与额度用尽的文案是照着 Swift 版合成的，不是真 claude 录下来的字节。
- `ready` / `deaf` / `noread` 模式不发终端查询（DA1 / OSC 11…），这一路只有真机冒烟和回放的录制字节覆盖。
- `fake_claude` 是 crewd-exec 的一个 bin，随这个包一起编译（`cargo install` 之类的场合会多出它）。

库与账本（Task 8、9）：
- 迁移里 serde 属性后的文档注释位置、`foreign_keys` 关闭时的迁移测试、`execute_batch BEGIN` 写法。
- `StoreLedger` 的 `exists()` 有 TOCTOU、EACCES 被当成 NotFound（用不带 CREATE 的 `Store::open_existing` 可封住）。

终评修正（整支分支终评之后，一轮修完；每条都有先红后绿的测试，除非另注）：
- 停止：反复 `stop()`（或叫停后句柄全丢）只发一次 SIGTERM、SIGKILL 的期限不往后推（以前每次 `stop()` 都重新计时，叫停比 `stop_grace` 还勤时忽略 SIGTERM 的进程永远杀不掉）。会话首进程收尸之后整组补一刀 SIGKILL（与孤儿回收同一道闸：pgid ≤ 1 或 crewd 自己那一组不发；组空了 ESRCH 无害），首进程乖乖退出时它留下的、忽略 HUP / TERM 的子进程不再成孤儿——每次退出都补，不只叫停时。叫停之后 `Core` 不再报 `ResumeRefused`（每条 `ResumeRefused` 后面都跟一条 `Spawned`，只有换新号重起失败时例外：那时跟的是 LaunchFailed 与 `Exited`）。
- 投递：写入 / 提交 / 等重发阶段屏上摆着要人回答的对话框时，放弃计时停走（框答掉后本轮窗口与 stall 上限重新给满）——人没答就不终结，正文不会留在输入框里；一笔以非「已投递」的终局收场、而正文写进过输入框，下一笔的第一次写入先发 Ctrl-U；终局与 stall 的文案按「回车实际发出几次 / 几次在按下前被撤回」说实话；输入框一直不回来的终局不再说「一个字都没收到」（改成没法确认、重发前先看一眼，免得投两遍）；输入行文本一直在变也受 `readiness_deadline` 约束、报一次 NotReady；拉起报过「卡住」（LaunchFailed / Stalled）后终于有输出就撤掉那条 health。
- 窄接口：`SessionHandle::send` 对空 / 全空白的正文返回 `SendError::EmptyText`（不占投递编号；以前给一个永远等不到结果的编号）。
- 续跑被拒时没投出去的开场任务搬到新进程：以前没有测试钉住——假 claude 加 `refuse` 模式（带 `--resume` 起时打印 claude 的原话后 exit 1），端到端测开场任务只提交一次、投给新号、不报 Abandoned；把 `respawn` 里的 `take_pending()` 换成空表这条测试会红（变异验证过）。
- 环境：`CLAUDE_CONFIG_DIR` 设了就透传（claude 按它放会话日志，认证探测按 crewd 自己的环境找——两边要一致）。
- 登记表坏了（解析失败）的报错说清怎么办（把文件挪开再启动）与为什么拒绝启动（不冒险按读不懂的表杀进程）。
- PTY：主端的非阻塞与 reactor 注册挪到起子进程之前（测试：在运行时之外调用，注册失败时不能有子进程跑起来过）。
- `crewd claude`：`--prompt TEXT` 换成 `--prompt-file PATH`（`-` = stdin）；stdin 在普通线程上读、`main` 去掉全局 `exit(0)`；SIGINT 的信号流在循环外建一次（以前每轮新建 `ctrl_c()`，两轮之间来的 Ctrl-C 会丢——这条没有确定性的测试，靠结构保证）。
- 真机冒烟测试的缺省目录、闸与退出等待（见上「真机核对」）；假 claude 限流用例的轮询窗口 2s → 10s（满载时更稳）。
- 过期的文档注释（`Delivery::is_finished` / `submit_aborted`、`Core::flush_due_keys`）。
- 修这一轮时的一次事故：`--prompt` 被拒的那条 CLI 测试第一版没带 `--claude <假的>`，在修之前的代码上跑时 crewd 接受了参数、去 PATH 里找到了真 claude、在临时目录（`/var/folders/…`，未被信任）里起了约 5 分钟，带着开场任务「秘密任务」等在信任框前；按 pid 给 crewd 发 SIGTERM 正常停掉。`~/.claude/projects` 里没有那个会话号的日志（正文没被提交）；终评复审查过 `~/.claude.json`：没有那个临时路径的 project 记录。该测试已改为一定带假 claude。

## 19. M4a 交付记录与遗留（计划 `docs/superpowers/plans/2026-10-06-crewd-m4a-work-core.md`；计划里的分支名 crewd-m4a，实际在 worktree 分支 `worktree-agent-ae8cbb53a573831e1` 上做；§18 留给 M2b 交付记录）

M4 拆为 **M4a** 工作跟进核心（纯计算，本节）→ **M4b** 持久化（store 的下一个 schema 版本）+ MCP 工具（§9）+ 检查点 / 人回复 / 停下即检查的唤醒接线 + 「我的工作项」注入（§4.3）+ 迁移（§10）。

已交付：`crewd/crates/work`（crewd-work；只依赖 serde，不起进程、不碰 tokio / 文件 / 数据库、不读时钟）——编号 `W<n>` / `H<n>` / `#N` 分开命名空间（字符串形式唯一、按机组编号）；字段表（§4.2 全部字段 + `channel`（§17）、`lastReport`（§4.3 的「下一步」）、`dropReason`）；权限表（负责人 / 机长 / 行政处 / 人 / 调度层）；父项、依赖、卡点三种边的并图找环与整份不变量（每次修改后整份校验，被拒 = 什么都不动、不消耗号码）；命令：登记 / 认领入口、建项（含无主项）、关联群、回报 + 下次检查、卡住（必答「其余能否先推进」、可拆分、新问可附默认方案）、人回复、默认方案到点、带证据完成、事实性 / 业务放弃、改链接、入口裁定关闭、转交请求 / 接受 / 撤回、指派、失联重派、检查点叫醒记账、升级记账；视图：入口状态（由第二层推导）、就绪 / 待认领 / 负载、我的工作项、到期检查点（按负责人合并）、失联候选、停下即检查（含防死循环）、人的待确认清单与 24h / 72h 升级、默认方案到点、巡检汇总与触发；事件日志。持久化边界 = 整份 `BookState`（serde）+ 事件，`WorkBook::restore` 读回时整份校验。验收：§1 判据的纯逻辑场景测试 3 条；固定种子的随机命令序列性质测试（48 × 400 步 × 2 组，含墙钟回跳）。

人对计划待决问题的拍板（2026-10-06）：
- **Q1 H 号归谁发**：工作账本按机组发 H<n>，是唯一发号处；M3 的人类待确认台账复用这套编号、不另起计数器（避免两本账号码相撞）。为此工作账本提供 `open_ask(by, now, question, default, item?)`：不挂任何工作项（`HumanAsk.item` 可空）也能开一条问、拿一个 H 号，`block` 的新问走同一个发号入口；这类问只要还开着就进人的待确认清单（卡住时开的问仍只在卡着活项时才进）。
- **Q2 人的权限**：人在权限表里位于机长之上，可以直接改任何工作项（回报 / 卡住 / 完成 / 放弃 / 指派等），且只有人能回答 H<n>。
- **Q3 失联节奏**：叫醒后多久再叫（`Config::recheck_after_ms`）与 K（`missed_checks_limit`）都在账本的 `Config` 里可配，默认 **15 分钟、K=2**——一个不回话的负责人约 30 分钟被判失联（计划原定 30 分钟，按此改了默认值与相关测试）。

与本 spec 原文不同 / 补充的决定（理由见计划「裁定」）：
- 图的边方向：父 → 子、项 → 依赖、项 → 卡住它的项（卡点里的工作项也进图）。父项可以依赖子项，子项不能依赖祖先，两项互相卡住被拒。
- 补的不变量：active / blocked 必须有负责人；nextCheckAt 只在有负责人的 open / active 项上；blockedOn 不指向已结束的问或已终结的项；活项不挂在已终结的父项下（done 要求子项全部终结）、不依赖 dropped 项、不来自已裁定关闭的入口。
- 人回复 / 默认方案到点 / 卡它的项完成：卡点清空且依赖已齐 → active、nextCheckAt = now；依赖没齐 → open（§5 原文「回到 active」与 §4.2「依赖未齐不能 active」冲突，按不变量）。卡点没清空也照样叫醒负责人。
- 拆出的子项不继承入口；入口状态只看直接关联项；没有活项时至少一项 done 即完成，全部 dropped 才交裁定。
- 停下即检查的「active 缺 nextCheckAt」在不变量下不可能，换成「检查时间已过」；另加「叫醒过没回报」「依赖已齐的 open 项没排检查」。连续 3 次违反集合不变 → 交行政处，之后静默。
- 失联：检查点唤醒后检查时间顺延 15 分钟（`recheck_after_ms`，Q3）；missedChecks ≥ 2 且顺延时间也过 → 失联候选；负责人对自己项的任何成功操作都清零，别人替它回报不清零。恢复失败的 session 名下的 blocked 项也算失联（否则人回复了没人接）。
- 账本记录见过的最晚时间，命令用 max(now, clock)：墙钟往回跳不会让「检查时间 ≤ now + 2h」对已有项突然不成立而卡死整本账。
- 人的待确认清单只列此刻仍卡着活项的问（`open_ask` 直接开的问除外，见 Q1）。
- 行政巡检只把「停下即检查交过来的 session」「关联项全部终结的群」判为需要起行政 session；失联重派、待认领、入口派活由调度层直接做。

整分支评审后的修补（2026-10-06，机长裁定；每条先写会红的测试再修）：
- **改链接也要建子项的权**：`relink` 把项挂到新父项下时，按新父项的负责人走与建子项相同的 CreateChild 检查（能在新父项上改链接的机长 / 人 / 行政处的事实修正也放行）；worker 不能借改链接把自己的项挂到别人的项下、卡住对方完成。
- **失联重派要核实**：`reassign_lost(.., unrecoverable)` 只接受核实过失联的项——负责人在调用方给的「恢复失败」名单里，或按检查点记账已到 Silent 线（missed ≥ K 且复查时间已过，与 `owner_lost` 同一个判定）；否则 `NotLost`。行政处因此不能拿它当强制指派（§8.2 / §8.4）。
- **一个时间基准**：检查上限（≤ now + 2h）只约束这一次命令**写入**的检查时间（spec「写入时强制」）；`invariants::check` 不再含时间，`WorkBook::restore(state, cfg)` 不再带 now，调低上限后旧账本照样读得回。所有视图（到期检查点、失联候选、停下即检查、人的待确认、升级、默认方案到点、巡检）都按 max(调用方的 now, 账本时间) 算，与命令一致。「下次检查必须晚于 now」也按这个时间比；例外是迁移 / 派活的占位 `check_at(now)`——等于调用方自己的 now 就收，记成账本的「现在」。
- **前跳的风险（交 M4b）**：调用方的 now 超前账本时间时命令照常执行、账本时间跟上（棘轮）。若那是一次错误的前跳（坏的 NTP 校时），墙钟回来后账本时间仍停在跳过去的位置：「下次检查」必须晚于那个时间，检查点与升级都按那个时间算。`Config::max_clock_jump_ms`（默认 24h）+ `WorkBook::clock_jump(now)` 把超限的前跳报出来，M4b 应在执行命令前调用、超限就记日志 / 告警，必要时先交人确认。
- **巡检不粘住**：`patrol(now, unrecoverable, archived)`：`stop_escalated` 列交过行政处、且此刻仍有没着落的项的 session（复审 I1：集合变多 / 变少都算，改干净才掉下去）；名下已没有活项的 session 的连续计数在事务里清掉；已归档的群（`archived`，群状态在 M3 的群表）不再算可归档。`needs_admin_session` 因此会回到 false。
- **停下即检查的叫醒预算**（复审 P12）：除了「连续 3 次集合不变」，自上次干净以来累计叫满 `Config::stop_rewake_budget`（默认 6）次也交行政处，不论集合变没变；之后保持安静直到干净。挡住「每次只挪一项」的来回摆动。
- **H 号可以不挂工作项**：`open_ask(by, now, question, default, item?)`，见上面 Q1；`HumanAsk.item` 可空，新增权限 `OpenAsk`（任何 session 与调度层，人不需要给自己提问）。
- **检查点记账按负责人、去重、跳过**：`record_checkpoint_wake(by, now, owner, ids) -> WakeResult { recorded, skipped }`：同一项列多次只记一次；投递与记账之间已不归该负责人 / 卡住 / 收尾 / 不存在的项跳过并报回，不整批拒绝。
- 小修：`Rejected` / `Violation` 实现 `Display` 与 `std::error::Error`（开发者向的英文短句；给 agent / 人的文案仍归 M4b）；`Config` 每个字段可单独缺省（`#[serde(default)]`）；读回时额外拒绝：问 / 入口的键与记录号不一致、现负责人不是 ownerHistory 最后一条的接手人、记成按默认方案办却没有默认方案的问、没有理由的 dropped 项；事实性放弃（重复 / 被取代）像完成一样放行卡在它上面的项并叫醒其负责人（业务放弃仍被拒）；工作项记下 `created_by`，发起 request_help 的 worker 在没人接之前可以自己撤回（业务放弃）。

已知、留到后面（评审提出，本轮不改）：
- **依赖未齐也能 done**：`mark_done` 只要求证据与子项终结，不要求依赖已齐（依赖只约束 active）。要不要禁止「先于依赖完成」待定。
- **拆出的子项总是有负责人**：卡住时拆分，子项缺省归原项负责人，没有「拆出来交调度层派」的无主形态。
- **事件覆盖**：停下即检查的连续计数（软状态）不产出事件；重复的 `register_entry` 也不产出事件。M4b 若只按事件增量落库，要整份落 `BookState` 或另记这两类。
- **回报清不掉下次检查时间**（P13）：`report` 的 `next_check_at` 只能改不能清。在等依赖的项应 `block(Item(dep))`（依赖完成时自动放行），M4b 的工具说明要写明这一点。
- **僵尸负责人刷新 missedChecks**：负责人对自己项的任何成功操作都清零失联计数，包括来回挂 / 解关联群这类不推进工作的操作。是否只认「回报 / 状态变化」待定。
- **从没排过检查的项也被记账**：`record_checkpoint_wake` 会给 `nextCheckAt` 为空的 open 项也加 missedChecks（只是不顺延时间）；这种项不进 `due_checkpoints`，只在调用方自己列进来时发生。
- **`reassign_lost` 信任调用方给的恢复失败名单**：M4b 只能用 crewd 自己 `--resume` 失败的记录来填，绝不能来自 MCP 参数或行政处输入，否则又成了强制指派。
- **24h 以内的前跳**：`clock_jump` 只报超过 24h 的前跳；更小的错误前跳同样让按真实时间写的「下次检查」被拒（要晚于账本时间）。M4b 的工具应接受相对时长（「30 分钟后」），按 max(now, clock) 换算。
- **直接开的问被回答后要叫醒提问人**：`open_ask` 的问不卡任何项，回答 / 默认方案到点不产出 `BlockerCleared`；M4b 要按问的记录（`asked_by`）叫醒提问的 session。
- worker 用 `create_item(..from_entry(#N))` 建一个无主项，会让该入口显示为「进行中」、不再算待认领（负责人列表为空）。是否要求挂入口的项必须有负责人待定。

遗留（M4b 起处理）：
- H<n> 只由工作账本发（Q1）：M3 / M4b 接人类待确认台账时调 `open_ask` 拿 H 号，不能另起计数器。
- 停下即检查的连续计数是软状态：存在 `BookState` 里但不产出事件，M4b 若只按事件增量落库会丢它（丢了最多多叫 3 次）。
- 事务克隆整份状态（几百项量级够用）；`Rejected` 没有人类可读文案，MCP 层映射。
- 跨机组 DAG 边（§13 Q5）仍不做；一本账 = 一个机组在结构上排除了它。

## 20. M5a 交付记录与中文实测（分支 crewd-m5a，计划 `docs/superpowers/plans/2026-10-06-crewd-m5a-jev-client.md`）

已交付：`crewd/crates/jev`（crewd-jev）——System One 线上类型（choice 选项保序上线）、凭据脱敏（PEM / PGP 私钥块、URL 账号密码、Bearer / Basic、JWT、厂商前缀（含粘在 `_` 后的）、Slack / Discord webhook、带标识符前缀的字段赋值如 `DB_PASSWORD=`、中文口令；幂等，提交号 / UUID 不遮）、异步客户端（reqwest + rustls；Bearer；单次 5s / 总 12s；429 / 529 / 5xx / 连接错误退避重试，尊重 Retry-After；401 / 422 不重试；只收 https）、key 存放（macOS 钥匙串 `pendingcrew-jev`/`default`，其他平台 0600 文件，权限宽就拒读）、四个决策的构造与判定（种子打乱、名单截 15、量换词、选项校验）、门面 `Jev`（未配置不联网）、本机 JSONL 诊断日志（0600、5 MiB 轮转、不记 key）、40 条合成中文实测集与 `jev-eval`。给 Jev 看的文字全在 `crates/jev/prompts/*.md`。

实测（2026-10-06，`jev-latest` 实际解析为 jev-1.13.0，40 条 × 2 个种子，报告 `crewd/crates/jev/eval/results/2026-10-06.json`）：

目标准确率 95%；延迟 p50 286 ms / p95 414 ms（含网络与重试）。

| 决策 | 次数 | 出错 | 准确率 | 种子间一致 | 选中排第一（随机期望） | 推荐阈值 |
|---|---|---|---|---|---|---|
| route_todo | 26 | 0 | 100% | 13/13 | 3/26（14%） | 0.30（准确 100%，自动执行 100%） |
| wake | 22 | 0 | 82% | 11/11 | — | noul 0.70（逐成员判对 100%，72 次判断） |
| attribute | 16 | 0 | 100% | 8/8 | 6/16（26%） | 0.30（准确 100%，自动执行 100%） |
| urgency | 16 | 0 | 88% | 2/8 | — | 0.90（准确 100%，自动执行 44%） |

按置信度分桶（答对 / 次数）：

| 决策 | [0,.25) | [.25,.5) | [.5,.75) | [.75,.9) | [.9,1] |
|---|---|---|---|---|---|
| route_todo | 0/0 | 2/2 | 1/1 | 1/1 | 22/22 |
| wake | 2/4 | 9/11 | 3/3 | 2/2 | 2/2 |
| attribute | 0/0 | 0/0 | 0/0 | 6/6 | 10/10 |
| urgency | 0/0 | 0/0 | 1/1 | 6/8 | 7/7 |

取值：
- 未改默认阈值（仍 0.5）。按计划的规则四个决策都有推荐值（choice_min = 0.30（route_todo 0.30 / attribute 0.30 取大），score_min = 0.90，wake_yes = 0.70），改默认值的条件成立；但改代码这一步在执行时被 runner 的自动审查拦下，交人定，见 §13 Q13。
- 推荐值的含义：route_todo / attribute 的 0.30 是网格下限（0.30–0.90），即这批用例里置信度最低的答案也都答对，不说明更低的线也安全；urgency 的 0.90 只让 44% 的判断自动执行；72 次逐成员判断全对的阈值区间只有 (0.68, 0.75]（网格上 0.70 与 0.75 都全对，规则取离 0.5 近的 0.70），余量很窄（必须叫醒的 ios 拿到 0.75 / 0.76，不该叫醒的机长拿到 0.64 / 0.68）。

读数的边界：
- 用例只有 40 条、每个决策 8–13 条，阈值是粗估；「种子间一致」只说明同一输入换选项顺序答案稳不稳，不说明对不对。
- 用例是写计划的人按 PendingCrew 用法编的，措辞比真实群消息规整；真实分布下的准确率只会更低。阈值是在同一批用例上推出来又在同一批上量的（样本内），没有留出集。
- 「选中排第一」与随机期望比，只在两个种子、少量样本上看偏向，不足以证明没有偏向（route_todo 3/26 低于随机期望 14%；attribute 6/16 高于随机期望 26%）。
- wake 的 82%（18/22）是在 wake_yes = 0.5 下量的：错的 4 次全是「多叫醒了机长」（wake-01 机长 p = 0.64 / 0.68，wake-09 机长 p = 0.52 / 0.54），该叫的都叫到了。
- urgency 的「种子间一致 2/8」是尺子的问题：`eval` 比较的答案串带两位小数的 score（如 `0 (0.30)` 与 `0 (0.39)`），8 条用例的最可能级别在两个种子间其实全部一致。错的 2 次都是 urg-07（等了几个小时的一个问题，期望 2 级，模型给 3 级）。
- 真实应答与文档形状一致；score 的应答里 `legend` 回显的是我们发的级别描述（`prompts/urgency.md` 的文字），没有回显 state。choice 的 probabilities 列出了全部选项。
- 延迟含网络与重试，量的是本机到官方端点这一条路。
- 无出错（80 次请求全部拿到可解析的应答，逐条日志里 0 条 error；是否发生过被重试吸收的 429 / 5xx 没有记录）。
- **选项名本身在提示答案**：实测集的选项名是有含义的英文（`scroll`、`release`、`crewd`、`m-301`、`ch-voice`……），模型能从名字猜到答案，准确率因此偏高。真实调度里成员 key 来自 session，M5b 必须加一层别名：短而稳定的不透明 slug（如 `s1`、`s2`）↔ session id（UUID 36 字符，超过选项名 32 字符的约定，也不该外发）；并用不透明 key 重跑一次 `jev-eval` 之后，才能信这里的准确率与推荐阈值。wake 的题号（成员 key）同样随请求发出，同一别名层管。

复审修正（同分支，M5a 收尾）：脱敏补了带标识符前缀的字段赋值（`_` 是单词字符，原规则的 `\b` 漏掉 `DB_PASSWORD=` / `AWS_SECRET_ACCESS_KEY=` 等）和一批厂商形状；Retry-After 超过退避上限或越过总时限即放弃、交机长（不再压成上限硬重试）；base URL 已带 `/v1/systemone` 不再拼两遍；连接错误带上底层原因链；key 文件只认本用户自己的 0600 普通文件（拒符号链接、核属主、O_NOFOLLOW 打开后检查），缺的父目录按 0700 建；crate 声明只支持 unix。都没有再调真 API。

留给 M5b：上面的别名层与不透明 key 重测；叫醒集合的大小上限（一条消息叫醒过多成员时交机长）；侧边栏紧急度的调用缓存（机组状态没变就不重问）。

## 21. M2c 交付记录与遗留（分支 crewd-m2c，计划 `docs/superpowers/plans/2026-10-06-crewd-m2c-codex-session.md`）

已交付：`crewd/crates/exec/src/codex`——线协议（无 `jsonrpc`、整数与字符串 id、两套 id 分表、单请求期限）、参数构造与服务端请求答复表（0.160.1 的 11 个方法全覆盖）、协议健康、回合跟踪与自动压缩、sans-IO 会话核心 `CodexCore`、进程（setsid、白名单环境、stderr 排空、stdout 行上限、写任务）、驱动 `start_codex`（握手 + thread id 落账完成才返回；进程规矩与 M2b 终版的 claude 驱动一致：停止只开始一次、收尸后只发组信号补刀、先收拾后发 `Exited`、文件 IO 走阻塞线程池、收尸有上限）。窄接口的最小扩展：`send_with_context`（与 `send` 同一道空正文闸）、`interrupt`、`control`、`snapshot` 可空、`SessionEvent::{Item, TurnEnded, Notice, Profile}`、`HealthKind::{TurnFailed, VersionIncompatible}` 与共用的 `HealthKind::rank`、`StartError::Handshake`。`CODEX_HOME` 进环境白名单。`crewd codex` 子命令（开场任务只经 `--prompt-file`）；`StoreLedger::expecting`。

验收：假 app-server（`src/bin/fake_codex.rs`）上的端到端 26 条（握手、落账先于就绪、续跑与退回且事件与 claude 同序、每一种服务端请求、额度、请求超时、stderr 灌满、落账失败、登记表写不进去、停止与工具子命令、反复叫停、codex 自己退出后的子命令、工具子命令攥着管道不放、自动压缩与排队、打断、不读 stdin、3MB 条目、控制、登记表、句柄全丢、同一 thread 不能同时跑两个、启动失败后 thread id 与登记表已放开）；`crewd codex` 5 条（两次运行之间经库续跑、claude 的 session 被拒、开场任务经 stdin 读入走 `turn/start`、命令行上的 `--prompt` 被拒、stdin 开着也随会话退出）。`./scripts/check.sh` 全绿（工作区 710 passed、0 failed、4 ignored（终评修正后；交付时 703）：三条真机冒烟 + jev 一条）。

逐任务评审后 controller 追加的修正（都已落地并有测试钉住）：
- 回合跟踪：别的回合的 agent 正文不当作本回合的 `TurnEnded` 正文；重复的完成通知不重发 `turn_ended`、不提前结束压缩。
- 核心：stop 之后 `send` / `control` 不再开回合、不报 Delivered；空白开场任务不开回合。
- 传输层：`spawn_codex` 自己剥 `OPENAI_API_KEY` / `CODEX_API_KEY` / `CODEX_ACCESS_TOKEN`（结构上保证，不靠调用方）。
- 终评后追加的修正（分支 crewd-m2c 最后一轮，都有先红后绿的测试）：
  - `mcpServer/elicitation/request` 的 `mcp_tool_call` 只在 `serverName` 是我们挂的那个 server（常量 `proto::CREW_MCP_SERVER`，与 `config.mcp_servers` 的键同一处）时 accept；别的 server 的工具调用审批一律 decline + `Notice`（同种只喊一次）。`crewd codex` 不挂 crew server（`comms: None`），那里每一条工具审批都被拒，是对的。
  - `health_from` 对 `error` 通知只认 `willRetry: false`；`willRetry: true`（或没给）的 `unauthorized` / `usageLimitExceeded` / 版本文案不再变成 health。一个正常完成的回合现在也清掉 `AuthRequired`（连同额度 / 回合失败 / 版本）。
  - 带 `threadId` 且不是本 thread 的通知整条丢弃（不喂回合跟踪、不进健康）；服务端请求照旧一律回答。
  - 回合跟踪：只有属于进行中回合（或没带回合 id）的条目才把「干活中」立起来，已完成回合的迟到条目不再让它粘在 true；条目仍照常进结构化记录。
  - `parse_line`：有 `method` 键但不是字符串的对象归 Unclassifiable，不再落进 Response 分支。
- 驱动：启动失败（握手超时、落账失败）等整组杀掉、收尸、放开 thread id、清登记表之后才报——调用方见到失败即可同号重起，晚到的 `forget` 不会抹掉新进程的登记。
- 假 app-server 多了 `spawn_child_inherit` 模式（工具子命令继承管道），多两条相应的端到端；一条原变异步骤（删驱动里那处剥 key）因上一条已作废，改成同时删两处。

与 Swift 不同的决定（理由见计划「裁定」）：
- 服务端请求 id 支持字符串；`applyPatchApproval` / `execCommandApproval` 显式按审批拒；`currentTime/read` 照实回。
- 单请求期限（initialize 30s、thread 60s、其余 30s）；「早到响应缓冲」由「先登记后写 + 单任务顺序处理」在结构上消掉。
- 回合排队、不用 `turn/steer`（人 2026-10-06 定）；`TurnEnded` 只带本回合的最后一条 agent 正文；读数晚到时不在回合中间压缩。
- `serviceTier` 不选不传；`initialized` 不带 params；多订阅退两种输出增量。
- setsid + 退出后整组 SIGKILL；剥 `OPENAI_API_KEY` 之外再剥 `CODEX_API_KEY`、`CODEX_ACCESS_TOKEN`；`CODEX_HOME` 透传。
- thread id 落账失败 = 启动失败（Swift 只回调、不拦）。
- 续跑被拒的事件与 claude 同序（`Spawned{旧号}` → `ResumeRefused` → `Spawned{新号}`，pid 不变）；消费方以 `ResumeRefused` 之后那条 `Spawned` 的号为准。

真机核对（人 2026-10-06 同意跑，含花额度的回合；codex-cli 0.160.1，本机登录态，默认的非 git 临时空目录）：
- 握手通过：`CREWD_LIVE_CODEX=1 … live_codex_handshakes … --ignored`，2.2s。收到 `Spawned{resumed:false}`、`Profile`（codex 自己解析出模型、推理强度、fast=false）、状态 `Idle`、`Exited{Code(0), stopped_by_user}`。逐条对「未核实」表：`initialized` 不带 `params` 被接受 —— 证实；`thread/start.config.mcp_servers`（指向 `/usr/bin/true`，MCP 起不来）与 `developerInstructions` 被接受、不挡握手 —— 证实（MCP 真挂上要等 M3 的 `crewd mcp`）；`optOutNotificationMethods` 被接受 —— 证实（退订效果仍没验）；`serviceTier` 读数 —— `Profile.fast` 取到 `false`；工作目录不是 git 仓库时 `thread/start` 照收 —— 证实（无需 `CREWD_LIVE_CODEX_WORKDIR`）；SIGTERM 后 codex 干净退出码 0 —— 本次所见。
- 回合通过：`CREWD_LIVE_CODEX=turn … live_codex_turn … --ignored`，6.4s，一个最小回合。`TurnEnded` 的正文是白板里的暗号，说明经 `additionalContext`（`kind: untrusted`）送入的白板真的被模型看见（Swift 起就挂着的这条「需真机确认」落地）；`Delivery{Delivered}`、`Item`（userMessage、agentMessage）、`Working(true→false)` 的顺序符合预期。
- Linux：`cargo clippy -p crewd-exec --all-targets --target x86_64-unknown-linux-gnu -- -D warnings` 通过（只编译，没在 Linux 上跑测试）。
- 副作用：两次 `thread/start` 各在 codex 自己的 sessions 目录留了一条 thread 记录（握手那条应当没有回合——没有去翻 sessions 目录核对，未核实）。

仍未核实（真机没覆盖）：`_meta.codex_approval_kind` 的 elicitation 分界（要真调 crew 工具，M3）；`optOutNotificationMethods` 的退订效果；握手后即停、再用同号 `thread/resume` 能否接上（接不上按裁定 3 退回新 thread）；「requires a newer version of Codex」的原话。

已按 schema 核对、不再算未核实：`thread/settings/updated` 的 `threadSettings.effort`（0.160.1 `ServerNotification` schema 里有此字段，`Profile.effort` 读的就是它；schema 层核对，真机回显里它的取值没单独对过）。

遗留：
- `turn/steer`（运行中插话）没接，现在一律排队（人 2026-10-06 定）；M3 若要插话再说。
- 白板「未读」的取舍与去重归 M3；codex 自己按 key 去重（值没变就不再注入），claude 一侧每次都包。
- `Item` 事件只发不存（结构化记录只在内存里留最近 200 条）；attach 回放、背压归 M2d。
- `Notice` 只发事件；转群（Swift Todo #6 的群通知）归 M3。
- 原生默认模型探测（ephemeral `thread/start`）、codex 的 npm 托管回退没做。
- 「进程退出后等 stderr 读到 EOF」没有能稳定变红的测试（竞态），按结构保证。
- 评审里记下、没修的小项（都不影响正确性主线，酌情清）：
  - 值得修（上一轮列出的 elicitation 放行范围、`willRetry` 与 `AuthRequired`、`parse_line`、迟到 item 粘 working、threadId 过滤已在终评后修掉，见上）：还缺一条「服务端错误原样透传」的测试。
  - 回合跟踪：丢了 `turn/started` 会漏掉压缩相关的复位；idle 时的陈旧 foreign agentMessage 可能留进只经 `on_turn_accepted` 得知的回合；条目类型比较区分大小写。
  - 核心与驱动：`turn/start` 在途时 `interrupt` 是静默空操作，stop 宽限期内的 `interrupt` 仍会发 `turn/interrupt`（无害）；账本失败时状态是 Exited 而非 LaunchFailed；退回路径上 start 失败会丢掉续跑被拒的说明；压缩没有回合级期限（Swift 同）；`Phase::Recording` 无期限（账本卡死则 `start_codex` 不返回）；排空阶段忽略超长行（无 `Notice`）；登记表 `forget` 只按 session id（沿用 M2b）；e2e 只在 current_thread 运行时上跑。
  - 传输：`StderrTail::text` 遇多字节截断可能略超 4096 字节；写任务的 EPIPE 行为借用了 Rust 标准库二进制里忽略 SIGPIPE 的保证；为 codex 设了 `TERM=xterm-256color`。
  - `crewd codex`：`/compact` 在 `select!` 里等最多 30s，Ctrl-C 晚被察觉；`/interrupt`、`/compact`、`--transcript`、信号停止、缺 `--session` 行无自动化测试；`start_codex` 返回前（信号流还没建）收到 SIGINT 会杀掉 crewd 而把 codex 留到下次回收（`crewd claude` 同）。
  - M3c 要先面对的（没修，记在这里免得漏）：`ClaudeSpec` 没有 `initial_context`——M3c 要么把 claude 的开场任务自己先包好（`<external_键>…</external_键>`），要么给 `ClaudeSpec` 加这个字段；codex 一侧有 `initial_context`。
  - M2b 窄接口一侧：claude 侧 `Command::Send{context}` → `with_untrusted_context` 没有端到端测试；该函数不转义值里的闭合标签、不校验 key（**M3c 入口条件：在任何调用方把队友写的文本传进来之前，要先转义闭合标签并校验 key**）、全空白值产出空块；投递失败的预览显示包装而不是指令正文；`control` 的 `Unsupported` 用 `{op:?}` 的 Debug 文本。

## 22. M3 细分：M3a 数据与 API → M3b agent 面 ∥ M3c 编排（2026-10-06 设计，未实现）

> 基准：`crewd-m2b@b5a65b0`。M4a 读自 worktree 分支 `worktree-agent-ae8cbb53a573831e1`（其 §19），M5a 读自 `agent-a71a65945df7d496b`（其 §20）。Swift 路径相对仓库根。

### 22.1 切分与次序
```
M2b ─┐
M4a ─┴─> M3a 数据与 API ─┬─> M3b agent 面（crewd mcp / crewd hook / 工具 / 世界观）─┐
                         └─> M3c 编排（会话管理 / 唤醒 / 准入 / 重启恢复）─────────┴─> M3 集成验收 ─> M4b
M2c ──（可选：codex 经 M3c 的 Executor trait 接入，未合前 codex 起不来）
```
- M3a 先独立落 main，把 M3b / M3c 共享的类型、表、trait 定死（§22.5）。之后两者各开 worktree 并行，按 §22.5 的文件归属互不改对方文件。
- **v3 是 M3 唯一一次迁移**：M3a 一次建齐 M3b / M3c 要的表。M3b / M3c 不加迁移；缺字段 → 改 M3a 的 v3（M3 合并前 v3 未发布，可改），后合者重跑迁移测试。
- M4a（纯 crate，无冲突）必须先合 main：M3 的人类待确认用它的 `open_ask` 发 H 号（§19 Q1：工作账本是唯一发号处）。§17 末段写的「M3 schema v2」作废——v2 已被 M2b 用掉，群相关表进 v3。

### 22.2 M3a 数据与 API

目标
- schema v3（下）+ 迁移；store 搬到专用线程的 `Db` actor（§14 / §18 遗留：RPC 在 tokio 线程上同步调 SQLite）。
- 新 crate `crewd-collab`（同步、不碰 tokio、时间由调用方传）：群、成员、发消息（校验 + 解析 @ + 落库 + 发事件）、游标、红标、机组 DAG、人类待确认（经 crewd-work）、审批台账、唤醒入队。M3b / M3c 只经它写协作数据。
- 客户端 RPC + 事件（下）；契约整理到能给 Swift 生成代码（§14 遗留：typed result / event、params `deny_unknown_fields`、枚举不再是自由字符串）。
- `collab::render`：给 agent 看的消息渲染（注入块 / 唤醒正文）。文字放 `crates/collab/prompts/render.{zh,en}.md`，沿用 jev 的格式（`include_str!` + `## 节名`，节缺失测试钉住），代码里没有提示词字面量。

非目标：不起进程、不决定谁醒（只提供入队）、不实现 MCP、不写世界观文案、不把旧人类账迁成 H 号（与 §10 迁移一起归 M4b）。

schema v3（`schema.rs` 加 `V3`，`SCHEMA_VERSION = 3`）
```
channels(id TEXT PK, crew_id → crews, kind CHECK(lobby|human|topic|dm), name, purpose,
         created_by TEXT('human'|'system'|<session id>), owner_kind/owner_id（topic/dm；大厅、人类群 = 机长，现算）,
         pending_owner_*（转让）, created_at, archived_at)
  UNIQUE INDEX ON channels(crew_id) WHERE kind='lobby'；human 同样一机组至多一个
channel_members(channel_id → channels, member_kind CHECK(session|human), member_id,
         joined_at, left_at, PK(channel_id, member_kind, member_id))
channel_watch(channel_id PK → channels, since)                  -- 人类「我在注视」
messages   重建：v2 全部列，crew_id 换成 channel_id → channels；seq / id 原样搬；INDEX(channel_id, seq)
read_cursors(reader TEXT('human' | <session id>), channel_id, seq, updated_at, PK(reader, channel_id))
listens(session_id, channel_id, until, senders JSON, PK(session_id, channel_id))
wakes(id INTEGER PK, session_id, kind CHECK(message|timer|ask_answer|restart|system|checkpoint|stop_check),
      source TEXT, origin（触发它的 session，可空）, priority CHECK(human|system|auto), not_before, note,
      state CHECK(pending|inflight|delivered|dropped), attempts, last_error, created_at, settled_at,
      UNIQUE(session_id, kind, source))
wake_latches(scope CHECK(edge|session), id, reason, at, PK(scope, id))      -- §22.7 Q5：不按全机 / 机组限流
approvals(id PK, session_id, crew_id, tool_name, summary, tool_use_id, requested_at,
          decision CHECK(allow|deny) NULL, decided_at, decided_by, consumed_at)   -- consumed_at：一次性放行票用掉的时刻
restart_prompts(id, created_at, session_ids JSON, answer CHECK(resume|later) NULL, answered_at)
work_books(crew_id PK → crews, state JSON, updated_at)          -- crewd-work 的整份 BookState
ask_meta(crew_id, ask TEXT('H<n>'), resume_note, message_id, PK(crew_id, ask))
sessions += title, brief, desired CHECK(running|stopped) DEFAULT 'stopped', was_working INTEGER, ext（分机号）,
            last_active_at, last_output_at（发言）, last_progress_at（产出：工具调用 / 工作项）, awaiting_question,
            resume_failed_at, worktree（crewd 为它新建的 git worktree 的绝对路径，C8；NULL = 共享机组目录）
crews    += title_source, status_line, policy, captain_session_id, next_ext, position（同级里的次序，人手动排）
```
`checkpoint` / `stop_check` 两种 kind 是给 M4b 留的，M3 里没有写者。

迁移
- 每个机组建大厅，`id = '<crew_id>/lobby'`（确定性，测试可断言）；旧消息全部归到所属机组大厅。
- 人类群**懒建**：`'<root_crew_id>/human'`，在第一次有人往根机组人类群发言、或客户端 `channel.list` 列到根机组时建。
- 游标全部钉在迁移那一刻各群的末尾（每个 session 钉自己机组的大厅，human 钉每个群）：**迁移不产生任何未读、红标或唤醒**（#595 全机重放的教训）。代价是旧的 @人类 不再亮，接受。
- 导入的 session 一律 `desired='stopped'`；`extra` JSON 不解析。

群规则 [推]
- 大厅成员 = 本机组全部 session + 人类，**隐式**，不写 channel_members。人类群成员 = 人类 + 根机组机长（隐式）+ 显式行。topic / dm 全部显式。
- 只有成员能发言。例外：往根机组人类群发言自动入群（人回话时它才在群里）；system 可往任何群发——跨机组语法糖（`contact` 到别的机组、`report_to_parent`、`message_child_crew`）= system 发言 + 正文首行署名。dm 恰好两人、不能加人，归属发起者的机组。
- 跨机组成员：`channel.list(crewId)` 返回归属该机组的群 ∪ 有成员 session 属于该机组的群（§17「同时显示在每个成员所在机组下」）。
- 已归档的群拒收消息，可以解档。
- @ 在发送时解析成具体目标落库：`captain` → 群归属机组当时的机长 session id（写进 targetId，事后换机长不改历史）；`reply_to` 自动追加 `[broadcast, 被回复者]`（同 main 7af98b6）；目标不是群成员 → 拒，回执列出成员。
- §17 的待细化项：不做话题 / 回复串，用 `in_reply_to`；子机组只表达组织结构（部门 / 小组，可有独立工作目录与机长），按话题 / 任务协作用群（人 2026-10-06）。

红标与未读 [推]：不建表，按游标派生。某群 @人类红标数 = 该群 `seq > human 游标` 且 mentions 含 human 的消息数；`read.mark` 推进 human 游标，红标就消。Mac / iOS 共用同一个 human 游标。

根机组：`collab::root_of(crew)` = 没有父边的祖先里 created_at 最早的（§13 Q1）。根机组被收编后，它的人类群自动归档（消息保留）；它账本上的 H 号不动，改在新根的人类群里展示 [推]。

机组 DAG（移植 `LocalCrewStore.swift:356 attachParent`、`:501 adopt`、`:526 release`）：禁环（含自己挂自己）；内建机组（`builtin`）既不能被收编也不能收编别人；只能 release 直系子；release 带 `to` 时先挂新边、再摘旧边，失败不留副作用；能新建子机组、父机组；只能删非内建、且没有 `desired=running` session 的机组。号码规则照 `CrewDirectory.swift:8-13`（终身不变、不回收、分机 1 = 机长）。

人类待确认（H 号）
- `collab::asks`：每个机组一本 `crewd_work::WorkBook`，整份 `BookState` 存 `work_books`（M4a 定的持久化边界），每条命令在一个 SQLite 事务里读、改、写。M3 只用 `open_ask / answer_ask / apply_default`，外加 M3a 给 crewd-work 补的 `withdraw_ask(by, now, ask, reason)`（提问者 / 机长 / 人可撤；`AskState::Withdrawn{reason, at}`；问还卡着活项时拒绝）。
- 开问时在同一事务里往根机组人类群发一条 @human 消息，`ask_meta` 记下这条消息的 id 和 resume_note。
- 权限审批（照 Swift 驾驶舱 #75②，不阻塞）：`approvals::gate` 一个事务——有票 → 用掉、放行；同 session 同工具已有未决请求 → 只拒；否则开一条、在根机组人类群 @人 → 拒。人同意 = 一次性放行票（同 session 下一次同名调用用掉），决定后给提问者入队唤醒（source `approval:<id>`）。不发 H 号（不是工作卡点），人在 `approval.list` 里看到它。

客户端 RPC（`crewd_proto::methods`；`whiteboard.*` 直接删，不留别名）
| 方法 | params | result |
|---|---|---|
| crew.list / crew.get | — / {crewId} | Crew[]（按 position）/ CrewDetail（含 channels、sessions、rootId）|
| crew.reorder | {parentId?, crewIds}（这一层的全部机组，新顺序；只有客户端）| Crew[] |
| crew.create | {title, workingDirectory, parentId?} | Crew |
| crew.update | {crewId, title?, workingDirectory?, hidden?, statusLine?} | Crew |
| crew.adopt / crew.release / crew.delete | {crewId, parentId} / {crewId, fromParentId, toParentId?} / {crewId} | Crew / Crew / {} |
| channel.list | {crewId} | ChannelSummary[]（群 + 成员 + watched + unreadForHuman + humanMentions）|
| channel.create | {crewId, kind: topic 或 dm, name, purpose?, members: MemberRef[]} | Channel |
| channel.update | {channelId, name?, purpose?, archived?} | Channel |
| channel.addMembers / channel.removeMember | {channelId, members} / {channelId, member} | Channel |
| channel.watch / channel.setOwner | {channelId, on} / {channelId, owner}（人 = 强制）| ChannelSummary / Channel |
| message.list | {channelId, afterSeq?, beforeSeq?, limit?} | Message[] |
| message.post | {channelId, text, inReplyTo?, mentions?, attachments?} | Message（发送者 = 人）|
| message.search | {query, crewId?, channelIds?, after?, before?, limit?} | Message[] |
| read.mark | {channelId, seq} | ChannelSummary |
| session.list / session.get | {crewId?} / {sessionId} | Session[] / Session |
| ask.list / ask.answer / ask.withdraw | {rootCrewId?} / {crewId, ask, text} / {crewId, ask, reason} | HumanWait[] / Ask / Ask |
| approval.list / approval.decide | {} / {id, allow}（同意 = 一次性票）| Approval[] / Approval |
| restart.pending / restart.answer | {} / {id, answer: resume\|later} | RestartPrompt? / RestartPrompt |
| directory.list | {query?} | DirectoryRow[] |
| todo.list | 不变 | 不变 |

事件（主题 = 事件名第一段）：`message.appended`(Message)、`channel.changed`(ChannelSummary)、`channel.removed`({channelId}，群随机组删除被级联删掉)、`crew.changed`(Crew)、`crew.removed`({crewId})、`badge.changed`({channelId, unreadForHuman, humanMentions})、`ask.changed`(Ask)、`approval.changed`(Approval)、`restart.changed`(RestartPrompt)、`session.changed`(SessionLive，由 M3c 发)、`wake.changed`（M3c，排障）、`events.lagged`。`Message.crewId` 换成 `channelId`，不保留两个字段。M2d 与 iOS 只用这一套；iOS 的远程传输仍按 Q11。

验收
- 真实数据只读演练（照 M1 的做法，在副本上跑）：v2→v3 前后消息条数与 seq 集合一致；每个机组恰好一个大厅；迁移后 human 红标为 0、每个 session 未读为 0；再跑一遍迁移没有变化。
- 群规则每条一个测试：成员资格、dm 只能两人、归档后拒收、人类群自动入群、captain 解析落库、reply_to 追加、跨机组显示。
- DAG：成环 / 自挂 / 内建 / release 非直系子 / release-to 失败不留副作用，每条先写会红的测试。
- H 号：开问后重开库仍在、号连续；与 M4b 将来 block 开的问共用一个计数（直接断言 `WorkBook` 状态）；只有不卡活项时才能撤回。
- 契约：`crewd schema` 漂移测试；全部 params `deny_unknown_fields`；`crewd rpc` 能驱动全部方法。
- Db 线程：一条慢查询进行中，另一个连接的 `system.hello` 照常返回（有测试）。

### 22.3 M3b agent 面

目标
- `crewd mcp`：stdio MCP server，用官方 Rust SDK **rmcp 3.5.x**（2026-10-06 查 crates.io：最新 3.5.1，2026-10-05 发布，仓库 modelcontextprotocol/rust-sdk；features 用 `server` + `transport-io`，入口 `rmcp::transport::io::stdio()`）。工具表来自 crewd，所以**不用 `#[tool]` 宏**，手写 `ServerHandler::list_tools / call_tool`。helper 只是一根哑管道：连 `crewd.sock` → `agent.attach` → 原样转发。业务全在 serve 里（唯一写者，角色是当下的事实）。
- `crewd hook <post-tool-use|pre-tool-use|stop>`：读 stdin JSON → `agent.hook` → 把返回的 stdout 原样打印，exit 0。
- 工具实现在 serve 的 `server::agent::*`；描述文字在 `crates/collab/prompts/tools.{zh,en}.md`，每个工具一节。
- 世界观：`crates/collab/prompts/world-model.{zh,en}.md`，由 `collab::worldview::render(&WorldCtx) -> String` 渲染（变量照 `LocalSessionWorldModel.swift:73-90`）。claude 走 `--append-system-prompt-file`（M2b 已有通路），codex 走 `developerInstructions`（M2c）。内容按 §17 重写：机组与群的模型；只装自己所在群的消息；不能默认人类在看；注视规则；在没人注视的群 @人类 = 对方开着免打扰时叫他；删掉 plan / continue_work / sweep 那几段。
- `collab::turn_trace`：移植 `SessionTurnTrace.swift`。Stop 时停在一个没叫到人的问句上 → 代发一条：worker 在所在机组大厅 @机长，机长在根机组人类群 @人（`:21-26`，绝不 @ 自己）；按 prompt_id 去重；**永不阻塞**（`McpTurnHook.swift:13`）。同时写 `sessions.awaiting_question`。

非目标：不起 / 停进程（经 `SessionControl` 调 M3c）；不决定谁醒（只入队，交 M3c）；不加 UserPromptSubmit（M4b）；不接 Jev。

身份 [推]
- helper 命令行只留 `--data-dir D --session S --token-file F`。去掉 `--crew --captain --label --structured-post --agent --gate`：机组、角色、显示名在会话中途会变（换机长），写死在 argv 里会过期，改成每次调用时 serve 现查。
- token：M3c 起会话时用 `TokenTable::mint(session)` 生成，经 `CommsSpec.token` 交给 `prepare_claude`，写成 `<launch dir>/agent-token`（0600，随 LaunchFiles 一起删）；helper 读这个文件、`agent.attach{sessionId, token}`。用途是防串号（配错、复用旧配置），不是挡 agent 本身。token 只存在内存里：crewd 重启时旧进程已经跟着死了。
- 改 `crewd-exec::launch`：`CommsSpec { helper, data_dir, session_id, token, hooks, approval_gates }`，`HelperCommands` 按上面拼，hooks 默认开；PreToolUse 只给命中闸的工具注册（matcher）。三种 hook 都写显式 `timeout: 15`（官方默认 600s），helper 自己 5s 放弃。**实现前查官方 hooks 文档，核对字段名与默认超时。**

agent RPC（只有 attach 过的连接能调；没 attach 的调 → -32601）
- `agent.attach {sessionId, token}` → `{role, crewId, label}`
- `agent.tools {}` → `ToolDef[]`（按当下角色给）
- `agent.call {tool, args}` → `{text, isError}`（每次现查角色；越权 → isError 并说明原因，不看工具表）
- `agent.hook {event, payload}` → `{stdout?}`（三种 hook 都是一次往返，不等人）
- 角色变了：serve 在 attach 的连接上推 `agent.tools_changed`，helper 转成 `notifications/tools/list_changed`。客户端不理会也没关系，调用时还会现查。

hook 行为
- post-tool-use：先报产出——工具不是 crewd 自己的（`mcp__crew__*`）→ `sessions::note_progress(ToolCall)`（发言 / 读群不算，§22.7 控制者裁定）；再把本 session 所在的群（大厅 + 显式成员群 + 正在收听的群）里游标之后的未读渲染成 `hookSpecificOutput.additionalContext`（上限 9,000 字符，超了省略最旧的并自报）。可见性（M3a 的 `cursors`，移植 `CrewWhiteboardVisibility.swift:48`）：只有 session / captain 收窄可见范围，human 不收窄，broadcast 放宽，自己发的永远可见。注入后推进游标；这个游标与唤醒共用，一条消息对一个 session 至多注入一次（`HookEmitter.swift:166-221`）。
- pre-tool-use（照 Swift 驾驶舱 #75②，**不阻塞**）：tool_name 命中 `approval_gates`（默认 `["computer-use"]`）→ `approvals::gate`：有一次性票 → allow；否则 deny，并（同工具没在等时）开一条审批、在根机组人类群 @人；人同意后它被叫醒、重跑同一调用放行一次。人卡住的只有这一次调用，session 照常干别的。没命中 → 不输出。连不上 crewd：命中的 **deny**。
- stop：调 `turn_trace`；连不上 crewd → 什么都不做（宁可漏一次）。
- 任何失败都写 stderr，绝不输出坏 JSON。

工具表（Swift 源：`Sources/Mcp/McpServer.swift:190-740`；C = 机长专用）
| crewd 工具 | 来源 | 变化 |
|---|---|---|
| post(channel?, message 或 messages[≤6], mentions?, reply_to?, category?) | post_to_crew | 加 channel（缺省：有 reply_to 就发到那条所在的群，否则发本机组大厅）；删 plan / todo / todo_status / todo_reference / evidence* / headline / audience / crew_status；在没人注视的群 @人类 时，回执写明 §17 那句免打扰提示 |
| read_channel(channel?, limit?, before?) | read_whiteboard | 读到最新一条时推进游标 |
| search(query, channel?, after?, before?, limit?) | search_whiteboard | 范围限自己是成员的群 |
| list_channels() / human_watching() | 新 | 后者返回人类正在注视的群 + 根机组人类群 |
| channel_create(name, purpose, members[], kind?) / channel_invite(channel, members[]) / channel_leave(channel) / channel_archive(channel) | 新 | 归档：群主或机长 |
| channel_transfer(channel, to, force?) / channel_accept(channel) | 新（§22.7 Q2） | 群主发起 + 对方接受；机长 force |
| channel_listen(channel, minutes?, senders?) / channel_unlisten(channel) | listen | 按群收听（§22.7 Q2） |
| directory(query?) | directory | 加上群 |
| contact(to, message) | contact | 语法糖：`7` → 7 号机组大厅 @它的机长（system 发言 + 署名）；`7-3` → 与该 session 的 dm（没有就建） |
| ask(question, resume_note?, fallback_after_minutes?, fallback?) | ask + add_human_todo | 用 open_ask 发 H 号，不阻塞；fallback → DefaultOption |
| withdraw_ask(ask, reason) | withdraw_human_todo | 原来的 supersedes = 撤旧问 + 开新问 |
| respond_todo(number, response) | respond_todo | 去掉 status / evidence（§9）|
| get_quota / schedule_wakeup(after_minutes 或 at, note) / set_session_profile(model?, effort?) | 同名 | schedule_wakeup 入 `wakes(kind=timer)`，持久化；set_session_profile 下次启动生效 |
| C rename_crew / change_workdir / set_crew_status(text) / read_delegation_policy / update_delegation_policy | 同名；set_crew_status 是从 post 的 crew_status 拆出来的 | |
| C start_session / stop_session / inspect_session / nudge_session / list_sessions / handoff_captain_to_session / create_and_handoff_captain | 同名 | 经 `SessionControl`；start_session 保留 Swift 的 `isolation`（必填 bool，机长逐次定，`McpServer.swift:576`、`:1141`；C8）：`true` = crewd 新建 git worktree，`false` = 机组共享目录；建失败直接报错、不退回共享目录 |
| C report_to_parent / message_child_crew | 同名 | 语法糖：发到父 / 子机组大厅并 @它的机长（system 发言 + 署名） |
| C adopt_crew / release_crew / create_parent_crew / adopt_parent / create_child_crew | 同名 | create_child_crew = collab 建组 + `SessionControl::start` 起子机长 |
| 删 | continue_work / confirm_todo_sweep / plan_add / plan_update / plan_list | 由 M4 取代（§9）|
| 删（Q4） | raise_attention / clear_attention / crew_ordering_signals / arrange_crews | 由 @人类红标取代；侧栏由人手动排，agent 不排序 |

内建总机组上不列 ask / respond_todo / start_session（同 `McpServer.swift:745-753`），调用时再拒一次。

验收
- 假 claude（真 PTY）+ 真 helper：机长和 worker 拿到的工具表不同；worker 调机长专用工具被拒；中途换机长后不用重启就生效。
- 注入：@ 了别人的不进，@人类的进，broadcast 放宽，非成员群的不进；hook 与唤醒交错时每条消息至多一次。
- pre-tool-use：命中 → 当场 deny、出现一条审批（重试不重复开）；`approval.decide` 同意后同名调用放行一次；serve 停掉时命中的工具被 deny。
- stop：同一个 prompt_id 只代发一次；已经 @ 过机长 / 人的不代发。
- token 错或没 attach 时调 agent.* → 拒绝。
- 提示词：测试断言 `server::agent` 与 `collab` 的源码里没有工具描述 / 世界观字面量（照 jev 的节齐全测试），zh / en 两份的节名集合一致。
- 真 claude 冒烟一次（人点头后跑）：rmcp 握手、tools/list、一次 post、一次 PostToolUse 注入。

### 22.4 M3c 编排

目标：serve 里的会话管理 + 唤醒。跟时间有关的判定写成 sans-IO（新 crate `crewd-wake`：规划、环路熔断、队列选取，时间由调用方传，同 M4a 的风格）；IO 部分在 `server::orchestra`。

会话管理 `server::orchestra::Sessions`（实现 M3a 定的 `SessionControl`）
- 持有全部会话句柄与 `ProcessRegistry`。**一个 crewd session 同时只能有一个进程**（§18 遗留：按 session id 加锁，第二次 start 回 `AlreadyRunning`）；登记表同号覆盖的问题一并解决。
- 执行器抽象：`trait Executor`（启动返回 `(Box<dyn ExecHandle>, 事件流)`），`ExecHandle` = §13 Q12 的窄接口（send / raw / stop / status / screen_text / snapshot）。claude 用 M2b 的实现；codex 等 M2c，合进来之前 `start` 对 codex 回 `Unsupported`。
- 起会话：渲染世界观（`collab::worldview::render`）、mint token、填 `CommsSpec`、写 sessions 行（保留 agent_session_id，§18 遗留）、`desired='running'`。`isolation: true` 时先在机组工作目录里新建 git worktree（位置 `<repo 根>/.pendingcrew/worktrees/<slug>-<8 位 hex>`、新分支 `pendingcrew/session-<6 位 hex>` 从当前分支起；照 Swift `SessionWorkspace.swift:20-34`、`GitWorktreeService.swift:124-158`；失败 = 起不来、不写行、不退回共享目录），路径记进 `sessions.worktree`、session 在那里起；crewd 永不删 worktree（删除是人的动作）。`set_profile` 只落库、下次启动生效。
- 会话事件 → `session.changed`：Spawned / Working / Health / Decision / Exited；同时落 `last_active_at`、`was_working`。Decision 在群里渲染（§15：「SessionDecisionNotice 归 M3」，按 numbered 分叉，不编造编号）。ResumeRefused → 群里发系统消息（§18 遗留的诊断文案）。
- 额度用尽：按重置时刻入队 `wakes(kind=system, source='quota:<ts>')`，到点先 `clear_quota_health` 再投递（M2b 的 health 没有重置时刻 → 留 `QuotaSource` 口，缺省 45 分钟退避并在群里说明）。
- 并发上限（§22.7 Q5）：**默认不设**；设置里可选（`orchestra.settings.set`）。只挡自动拉起，超了留 pending；人起 / 恢复、人类唤醒拉起都不受它挡。

唤醒规划：`crewd_wake::plan(msg, ctx) -> Vec<WakeTarget>`（纯函数；ctx = 群成员、各成员角色、收听表、now）
- 只在群成员里选（§17）。自己发的不叫自己。超过 6h 的消息不叫（`CrewLocalMentionWakeLogic.swift:53`）。
- @session / @captain（发送时已解析成 session id）→ 叫目标。broadcast / human 不叫任何人（`:74`）。
- 人发、没有 @：先问 `Arbiter`（M5b 挂 Jev，候选 = 群成员；M3 的默认实现返回 None）；None / 低置信 → 群主（大厅、人类群的群主 = 机长；dm → 另一方）。
- 正在收听的 session：在 listen 窗口内、发送者匹配 → 叫（与上面取并集，一条消息一个 session 只叫一次）。
- system 发的消息照样参与唤醒，但不做回执判定。
- 优先级：人发的 → human；system 发的与计时 → system；session 发的 → auto。

队列 `wakes`（持久化，取代只在内存里的 `CrewDeferredWakeQueue`）
- 入队存来源身份，不存渲染好的正文（`CrewDeferredWakeQueue.swift:13`），出队时现渲染；靠 `UNIQUE(session, kind, source)` 去重。
- 在 idle 边沿（`SessionEvent::Working(false)`）和计时到点时调度：取该 session 已到期的 pending，**只取最高一档优先级、合并成一次投递**（上限 10 条 / 8KB，剩下的等下一个 idle）[推]。Swift 是一次一条（`:50`）；合并能少转几轮空转，也减轻限流压力。
- 投递前过滤掉已注入的：消息类唤醒若游标已越过它（目标忙时 hook 已经注入过）→ 直接记 `delivered`（`CrewSessionRunner.swift:479`）。
- 目标在干活 → 不投，等 idle（claude 忙时 hook 会把未读带进去）。目标没在跑 → 用 `--resume` 拉起，以唤醒正文开场（`CrewLocalMentionWaker.swift:20-21`），受可选上限约束（人类唤醒不受）；在等重启答复的不被自动拉起（§22.7 C4）。
- 投出去记 `inflight`；收到 `Delivery::Delivered` 才记 `delivered` 并推进游标（唤醒正文带上游标之后到这一条为止它看得见的全部消息）。失败按 30s / 2min 退避（写进 `not_before`），第 3 次放弃 → 在本机组大厅 @群主（机长自己 → 人类群 @人）。人停掉一个 session → 丢弃它 pending 的唤醒。
- 重启时 `inflight` 退回 `pending`；消息类重投前先查游标，防重放。已结的行保留 7 天，M3c 定时清。

环路熔断 `crewd_wake::breaker`（§22.7 Q5；取代 `AutomaticWakeAdmission`，**不移植**它的全机 12 次/分、单机组 5 次/分、单 session 2 次/分——那会挡住 30 个 session 的正常收发）
- 判据：目标 T 上一次有产出之后、10 分钟内送到 T 的非人类唤醒，按「来源 → T」与「T 的非 session 来源合计」两级各超 8 次 → 熔断那一条，落 `wake_latches`；告诉人（根机组人类群）和群主。阈值是一处常量。
- 产出 = `Change::Progress`（M3a `sessions::note_progress`：工具调用、工作项变化）；T 发的消息叫醒了正在叫它的来源以外的人也算。**回话给叫醒你的人不算**（否则互相 @ 的环路永远截不住）。
- 人类唤醒永不计数、永不被挡；system 唤醒照常计数。被挡 = 留在 pending（背压），不是丢掉。只有人能从客户端 `admission.recover` 解。

idle 边沿（取代 `CrewSessionRunner.swift:490-541 runBecameIdle`），按顺序：
1. 到期唤醒合并投递（见上）；投了就结束这次边沿。
2. `IdleHook::on_idle(session, now) -> Option<WakeRequest>`：M4b 的「停下即检查」挂在这里；M3 的默认实现返回 None。
3. 到此为止。删掉：机长 60s 检查点定时器（`:608`）、continue_work 租约、孤儿租约复核、Todo sweep 提醒（`:576`）——都由 M4a 取代。

重启恢复（§22.7 Q1：问人）
- serve 启动：回收孤儿 → inflight 退回 pending → `desired='running'` 的 session **一个都不起**，进待恢复名单，`restart::open` 在相关根机组人类群 @人、客户端弹窗。人答「恢复」→ 机长在前、错开 2s 逐个 `--resume`，`was_working=1` 的开场补一句「crewd 重启打断了上一轮，先核对状态再继续」；答「先不」或不答 → 都不起，唤醒照常入队，之后可整批或单个恢复。等答复期间人在群里 @ 了名单里的某一个 → 只恢复它（算它单个 resume），其余照旧等。
- resume 失败（续跑被拒、换新号也起不来，或根本起不来）→ 写 `resume_failed_at`，并在大厅 @机长。M4b 的 `reassign_lost(unrecoverable)` 只认这一列（§19：这份名单只能来自 crewd 自己的恢复失败记录）。
- 计时类唤醒（schedule_wakeup、额度重置、退避）都存在 `wakes.not_before` 里，重启不会丢；serve 只用一个定时任务，按晚于现在的最早 not_before（`wakes::next_due_after`）醒，另有 30s 心跳兜底；心跳顺带调 `asks::apply_due_defaults`（默认方案到点）。

问被回答：M3a 的 `asks::answer` 在同一事务里给提问者入队 `wakes(kind=ask_answer, source='ask:<crew>/H<n>', priority=human)`（H 号按机组发，单独的 `H<n>` 不唯一），正文出队时现渲染（问题、答复、resume_note）；提问者已无法恢复 → 叫它所在机组的机长转达。M3c 只投递，并把 `Change::AskAnswered.cleared` 交给 `AskAnsweredHook`（M4b 的 BlockerCleared 叫醒负责人）。审批有结果的唤醒同理（source `approval:<id>`）。

客户端 RPC（M3c 新增）：`session.start {crewId, role, executor, brief, title?, model?, effort?, isolation}`（`isolation` 必填，C8）、`session.stop {sessionId, reason}`、`session.resume {sessionId}`、`session.live {sessionId}` → SessionLive（working / health / pendingDecision / awaitingQuestion）、`crew.setCaptain {crewId, sessionId}`、`wake.list {sessionId?}`（排障用）、`admission.recover {scope: edge|session, targetSessionId, from?}`（只有客户端能调，agent 没有这个入口）、`orchestra.settings.get / set {maxLiveSessions?}`。事件：`session.changed`、`wake.changed`（主题 wake，排障用）。重启询问的 `restart.pending / restart.answer` 是 M3a 的。

验收
- `crewd-wake` 纯测试：plan 表驱动，上面每条规则一例；breaker：互相 @ 的环路在一条边上被截、30 个 session 正常收发不被截、熔断不挡 human、回叫不算产出；合并时只取最高一档。
- 假 claude 端到端：机长 @worker → worker 醒，回 @机长 → 机长醒；worker 忙时 @它 → hook 注入，idle 后不重投；@ 一个已停的 session → 它被 `--resume` 拉起。
- kill -9 serve：重启后先问人；答「恢复」后 `desired=running` 的被接回、`was_working` 的收到被打断的提示；崩溃前 pending 的 schedule_wakeup 恰好投一次（inflight 的非消息类唤醒至少一次，消息类靠游标恰好一次）。
- 对同一 session 并发 `session.start` → 第二个被拒，只有一个进程。
- `ask.answer` → 提问者被叫醒，正文里有 resume_note。

### 22.5 M3a 定死、M3b / M3c 共享的接口
完整签名以 M3a 计划的「冻结给 M3b / M3c 的接口」块为准（M3a 合进 main 后只能加、不能改签名）；这里只列分工。
| 接口 | 定义处（M3a 落地） | 写者 | 读者 |
|---|---|---|---|
| schema v3 全部表 | `crewd-store` | M3a | 全部 |
| `Db`：专用线程 actor，`db.call(\|s: &mut Store\| ..).await` | `crewd-store` | M3a | 全部 |
| `crewd-collab`：群 / 成员（`members` = 有效成员，含隐式）/ 发消息 / 游标（`cursor`、`unread_for`、`advance`）/ 机组 DAG 与 `reorder` / sessions（含 `note_progress`）/ asks / approvals（`gate`、`decide`）/ todos::respond / wakes（队列读写与清理）/ latches / restart / settings | `crewd-collab` | M3a | M3b、M3c |
| `render::{blocks_for, injection, build_batch, wake_text}`（文字在 prompts/render.*.md）、`prompts::fill`（一遍扫描） | `crewd-collab` | M3a | M3b（hook）、M3c（唤醒、codex additionalContext） |
| `CollabEvent` = `crewd_collab::Change`（`MessageAppended`、`WakeEnqueued`、`AskAnswered{cleared}`、`RestartAnswered`、`Progress` …），经 `Hub` 发出 | 类型在 collab，`crewd-server::hub` 重导出 | M3a | M3c、M3b 订阅 |
| `WakeRequest{session, kind, source, priority, not_before_ms, note, origin}` | `crewd-collab` | M3b（timer）、M3a（ask_answer / 审批结果）、M4b（checkpoint / stop_check） | M3c |
| `TokenTable{mint, verify, revoke}` | `crewd-server` | M3c（mint） | M3b（verify） |
| `trait SessionControl`（start / resume / stop / nudge / inspect / set_profile / handoff_captain / quota / live；`BoxFuture`，可做 dyn）+ `FakeSessionControl`；`StartRequest.isolation: bool`（C8；没有路径字段） | `crewd-server` | M3c 实现 | M3b 的工具、M3a 的 RPC |
| `AppState { db, hub, sessions, tokens, agent, orchestra }`（后两个 M3a 放占位类型） | `crewd-server` | M3a | M3b、M3c |
| `CommsSpec{helper, data_dir, session_id, token, hooks, approval_gates}` | `crewd-exec::launch` | M3b 改 | M3c 填 |
| `worldview::render(&WorldCtx) -> String`、`turn_trace::on_turn_end(tx, session, prompt_id, last_text) -> TurnEnd`（5 个值）的签名 | `crewd-collab`（M3a 先放 stub） | M3b 实现 | M3c 调 |

文件归属（M3a 先合 main，之后不改 M3a 的文件）：
- M3a：`server/Cargo.toml` 的 `testkit` 特性（控制者裁定 C6 的 G11，M3a 已建；M3b / M3c 只用、不改）。
- M3b：`crates/crewd/src/{mcp_cmd,hook_cmd,agent_client}.rs`、`server/src/agent/**`（替换 M3a 的占位）、`server/src/conn.rs` 的 agent 路由、`collab/src/{worldview,turn_trace}.rs`、`collab/prompts/{world-model,tools}.*`、`exec/src/launch.rs`、`proto/src/agent.rs`。
- M3c：`crates/wake/`、`server/src/orchestra/**`（替换 M3a 的占位）、`server/src/dispatch.rs` 一行、`server/src/lib.rs` 的 serve 启动段、`proto/src/orchestra.rs`、`crates/crewd/tests/e2e.rs`。
- 共享、各改不相交段，**后合者**保留两边并重新生成（不挑边，pbxproj 的教训）：`proto/src/lib.rs`、`crates/crewd/src/main.rs`、`Cargo.toml` / `Cargo.lock`、契约 json；`orchestra/exec_claude.rs::comms_for` 由后合者对上 `CommsSpec` 的形状。

### 22.6 M4b 的位置与 M3 留的口
M4b 在 M3b 与 M3c 都合进 main、M3 集成验收通过后再开工。M3 交付时必须已经有：
- `work_books` 表与按机组读、改、写 BookState 的事务（M3a）；M4b 只需加事件日志表（v4）和工作项命令。
- H 号的唯一发号处已经是 `open_ask`（M3 的 ask 在用）；M4b 的 `work_item_block` 开新问时共用它。
- `agent.call` 的分发与 `prompts/tools.*.md`：M4b 加 §9 的工具，只需加节和 handler。
- `crewd hook` 的子命令表留了扩展位：M4b 加 `user-prompt-submit`（§4.3），`HookEvent` 加一个枚举值。
- `IdleHook`（停下即检查）、`WakeKind::{checkpoint, stop_check}` 与对应的渲染节、按 `not_before` 触发的计时器（检查点不另起定时器）。
- `ContextExtras` 口：codex 的 `additionalContext` 按 key 组装（M3c 做），M4b 在这里注册 `my_work_items`。
- `sessions.resume_failed_at`：`reassign_lost` 唯一的名单来源。
- 卡住类问被回答后的 BlockerCleared 叫醒：M3a 的 `Change::AskAnswered.cleared`，M3c 交给 `AskAnsweredHook`，M4b 在那里按事件叫醒负责人。
- 产出信号：M4b 的每条工作项命令都必须调 `sessions::note_progress(WorkItem)`，否则 M3c 的环路熔断会把正在推进工作项的 session 当成空转。

M5b（切换之后）只需换掉 `Arbiter` 的实现（Jev + 别名层，见 §20）。

### 22.7 决定与待人拍板
本节标 **[推]** 的都是我自己定的：v3 一次建齐；大厅用确定性 id；迁移不产生未读 / 红标；红标按游标派生；@ 在发送时解析；审批不发 H 号；helper 不带角色参数、改用 token 文件；工具文字放 md；合并投递；收听按群；人类群懒建、被收编即归档。
人的拍板（2026-10-06，取代原 Q1–Q5 的推荐）：
- **Q1 重启后接回**：**问人**。crewd 起来后若有上次在跑（`desired=running`）的 session，发一条「是否立即恢复这 N 个 session」的询问（客户端弹出 + 人类群里一条，带名单）；人答「恢复」→ 错开时间逐个 `--resume`、被打断的补一条提示；答「先不」或没答 → 都不起，期间给它们的唤醒照常入队（持久化），人之后可整批或单个恢复。M3c 只实现「询问 + 两种答复」，不自动选。
- **Q2 群主与叫谁**：**每个群都有群主**（`channels.owner`），默认是建群者，可转让（转让走与工作项负责人相同的「双方确认或机长强制」）。人类群、大厅的群主 = 机组机长。人在群里不 @ 任何人发言时**由 Jev 决定叫醒谁**（候选 = 群成员）；Jev 未接入（M5b 之前）或低置信 → 交给**群主**（群主在群里相当于这个群的机长）。另外 session 可以**收听**某个群（`channel_listen`），收听者在该群有新消息时也会被叫醒（与 Jev 的选择取并集，同一条消息一个 session 只叫一次）。
- **Q3 私有群**：不做。这个问题是 §17 写作时我自己提的，没有需求来源，删掉。
- **Q4 旧工具**：`raise_attention` / `clear_attention`（Todo #71 起已不控制任何东西，只是旧会话兼容）删；`crew_ordering_signals` / `arrange_crews`（机长给总机组侧栏排序 + 写一句摘要）删，**侧栏不再自动排序**（人 2026-10-06）：机组顺序由人手动排（拖动），存 `crews.position`（同一父机组内的次序），新机组排在同级末尾；没有基础序、没有覆盖层、不需要排序信号。原因：当初自动排是因为子机组太多太乱，而很多原来用子机组做的事（按话题 / 按任务协作）现在应该用机组内的群（§17）来做；组织架构本身不需要排序。
- **Q5 并发上限**：**默认不设上限**；设置里提供可选上限（默认关）。必须防的是**异常唤醒循环**（程序错误导致同一 session 被反复叫醒、两个 session 互相叫醒打转）：按「唤醒来源 → 目标」与「目标 session」两级做熔断，判据是**短时间内重复唤醒且没有新产出**（没有新消息 / 工作项变化 / 工具调用），熔断只停那一条环路并告诉人和群主，绝不按全机总量限流——大量 session 同时正常工作不受影响。阈值与判据写成一处常量，带测试（循环被截住；30 个 session 各自正常收发不被截）。
控制者裁定（2026-10-06，三份 M3 计划对齐时定；正文 §22.2–§22.6 已按此改）：
- **C1 侧栏手动排**：`crews.position` + 只有客户端能调的 `crew.reorder {parentId?, crewIds}`（一次给出一层的完整新顺序），新建 / 收编 / 转挂进来的排在同级末尾；没有任何按动静排序的 API。话题 / 任务协作用群，子机组只表达组织结构（§17）。
- **C2 审批照 Swift 驾驶舱 #75②，不阻塞**（取代原 §22.3 的「等 30 分钟」）：命中闸的 PreToolUse 当场 deny 并给人开一条待确认；人同意 = 一次性放行票，同一 session 下一次同名工具调用用掉它。理由：人卡住的只能是那一件，绝不能卡住整个 session。闸的 matcher 与「连不上 crewd → deny」保留。
- **C3 产出信号**：M3a 的 `sessions::note_progress(ToolCall | WorkItem)` 写 `last_progress_at` 并发 `Change::Progress`，与 `last_output_at`（发言）分开；回话给叫醒你的人不算产出。M3b 的 post-tool-use 对非 crewd 工具调它，M4b 的工作项命令必须调它，M3c 的熔断消费它。
- **C4 重启询问 vs 人 @**：等答复期间，人在群里 @ 了名单里的某一个 → 只恢复它（算它单个 resume），其余照旧等；启动前就排着的人类唤醒不算。
- **C5** 人类唤醒绕过可选并发上限；其余照 M3c 的默认：环路窗口 10 分钟 / 8 次、人停 session 丢弃它 pending 的唤醒、`set_profile` 下次启动生效。（原写的「`worktree` = 已存在的目录、`start_session` 改收 `workdir?`」已被 C8 取代。）
- **C6** M3b / M3c 要的读写口全部由 M3a 提供并冻结（M3b 的 G3 / G-F / G-S / G-T / G9 / G11；M3c 的 `next_due_after`、`cursor`、`mark_failed` 带退避时刻、`requeue`、`by_ids`、`prune`、`members` 含隐式成员）；任何计划都不往别的计划的表里写 SQL。
- **C7** 默认方案到点由 M3c 的定时拍调 `asks::apply_due_defaults`；ask 唤醒的 source 一律 `ask:<crew>/H<n>`；跨机组语法糖（`contact` 到别的机组 / `report_to_parent` / `message_child_crew`）= system 发言 + 正文首行署名。
- **C8 `start_session` 保留 Swift 的 `isolation`**（必填 bool，机长逐次定，取代 C5 里「`worktree` = 已存在目录」）：`true` = M3c 的会话管理器在机组工作目录里新建 git worktree（位置、分支名、slug 照 Swift，见 §22.4），记进 `sessions.worktree`，session 在那里起；`false` = 机组共享目录；建失败 = start 失败并带原因、不写行、不起进程、不退回共享目录；crewd 永不删 worktree（删除是人的动作）。落点：M3a `StartRequest.isolation` 与列注释，M3b `start_session` 工具，M3c `orchestra/worktree.rs` + `Core::start_new`。测试一律用临时 git 仓库（合成），不碰真仓库。
- **C9 一次性放行票的范围 = (session, tool)**（照原计划，对 C2 的补记）：人同意一条审批 = 只放行「同一个 session 的下一次同名工具调用」，不跨 session、不按工具名全局放行。
- **C10 审批仍是独立的表 / 清单，客户端「等你拍板」视图把 ask 与 approval 合并显示**（红标、侧栏顶层）：合并在客户端做；M3a 的 `Ask`（`crewId`、`askedAt`）与 `Approval`（`crewId`、`requestedAt`）都带机组与创建时间，人类群 = `<根机组>/human`（`crew.list` 的父链推得），`ask.changed` / `approval.changed` 负载就是整条，足够客户端合并、排序、计徽标；不把审批并进 `ask.list`（H 号只给工作卡点，C2）。

## 23. M3a 交付记录与遗留（计划 `docs/superpowers/plans/2026-10-06-crewd-m3a-data-and-api.md`，分支 crewd-m3a）

已交付：schema v3 + v2→v3 迁移（一个事务；seq 高水位、sender / mention / role 词表归一且认不出即中止、大厅回填、游标钉末尾、机长指针、分机号、机组号计数器、旧的无 target @机长 一次性落成迁移时的机长）；Db actor（专用线程）；crewd-collab（channels / messages / cursors / listens / crews（DAG、编号、`reorder`）/ sessions（`note_progress`）/ directory / asks / approvals / todos / wakes + latches / restart / settings / render + prompts，worldview 与 turn_trace 为签名桩）；`Hub` 内部事件通道与 `AppState::collab`（库线程上提交后按序发布）、`SessionControl` + `FakeSessionControl` + `NoSessions`、`TokenTable`、feature `testkit`；客户端 RPC 32 个方法（system.hello、crew.{list,get,create,update,adopt,reorder,release,delete}、channel.{list,create,update,addMembers,removeMember,watch,setOwner}、message.{list,post,search}、read.mark、session.{list,get}、ask.{list,answer,withdraw}、approval.{list,decide}、restart.{pending,answer}、directory.list、todo.list、events.subscribe）与 11 个事件（含终审补的 `channel.removed`），删 `whiteboard.*`；契约 v2（`API_VERSION` 2、单一 `$defs`、类型化方法 / 事件表、`Event` 与 `ServerFrame` 判别 oneOf、topics 枚举、`$schema` 2020-12）；`crewd rpc` 端到端驱动全部客户端方法。

真实数据只读演练（副本，2026-10-06；只记计数）：
- 做法：源目录只读复制到会话 scratchpad；v2 crewd（M3a 开工前的 083398c）导入副本；迁移由一个只调 `crewd_store::Store::open` 的临时程序触发（与 serve 启动时开库是同一个调用），**没跑 serve / hello**（真实数据规则：不在真实数据上起 serve，副本也算）；也没在 crew 群里预告（只读计数，不往人的 app 里发东西）。副本与产出在记下计数后删除。
- v2 导入：62 机组 / 1014 session / 367066 消息（seq 高水位 367066）；导入报告 skipped 0、warnings 0。
- M3a 迁移后：`user_version` 3；消息 367066 条，seq 集合与迁移前 diff 为空，高水位 367066；大厅 62 = 机组 62；session 大厅未读 0、人类未读群 0、唤醒 0、外键不成立 0、`integrity_check` ok；sender 归一 user 1412 → human、pendingcrew 348932 → system，captain 8105 / session 8617 不变；mention 全在词表内（broadcast 934 / captain 4273 / human 3029 / session 1646）；无 target 的 @机长 4268 → 0（全部落成当时的机长）；游标 1076 行、`nextCrewNumber` 62。
- 再开一次：channels 62 / read_cursors 1076 / nextCrewNumber 62 等十项读数逐项相等，整库 `.dump` 的哈希相同。
- v3 直接导入：报告计数与 v2 导入逐项相同（skipped 0、warnings 0、无 `messagesWithUnknownSender` / `messagesWithUnknownMention`）；同一组 SQL 全 0；19 张表的行数与迁移出的库逐表相同。
- 认不出的 sender / mention / role：无。

与 spec 原文不同 / 补充的决定（理由见计划「裁定」与本节的控制者裁定）：
- 计划裁定：`CollabEvent` 类型放在 collab（裁定 3）；v3 补列 `crews.captain_session_id` / `next_ext` / `position`、`sessions.ext` / `last_output_at` / `last_progress_at`、`approvals.consumed_at`、`meta.nextCrewNumber`、`restart_prompts` 表、`channels.owner_*` / `pending_owner_*`（裁定 8、§22.7）；`wake_latches.scope` 为 `edge|session`（§22.7 Q5）；ask_answer 的 source 带机组（裁定 13）；`SessionControl` 多 `resume` / `handoff_captain`（裁定 15）；队列与 session 生命周期的写函数全在 collab（裁定 16）；`ask.answer` 的回调点 = `Change::AskAnswered{cleared}`（裁定 17）；附件上传未实现（裁定 14）；审批不阻塞、一次性放行票 `approvals::gate`（C2）；产出信号 `note_progress` + `Change::Progress`，发言不算（C3）；补给 M3b / M3c 的口（C6）。
- R9：迁移有意从 `extra` 里解析两个键（`$.titleSource`、`$.member.extensionNumber`），违背 §22.2「extra 不解析」，否则丢数据；其余不解析。
- C1 按父边排序：次序存在 `crew_parents.position`（每条父边一份，多父机组在每个父下各有次序），线上 `Crew.childIds` 是该父下的有序子列表；`crews.position` 只管根层；`crew.reorder {parentId?, crewIds}` 一次排一层。
- R6 一个窗口规则：`unread_for` 与 `build_batch` 每群都取**最旧**的至多 200 条；游标只推进到实际投递的最大 seq。`WakeBatch.deferred`（窗口外的排队行，留待下一批）+ `cursor_targets()`（每群推进到哪）；开头一段对该 session 不可投递的消息自动越过，不会永久卡住。
- 帧伪造中和：渲染时正文续行缩进、名字压成一行，消息正文冒充不了注入帧的头（人 / 系统）。
- 群规则补充：R1 根机组人类群由第一条发言建；R10 dm 归发起方 session 的机组（机组只管新建：`ensure_dm` 复用已有 dm 时不论传哪个机组都返回它，终审裁定，推翻 Task 7 修复 #4，否则对方从自己机组永远打不开这条 dm）；session 只能往自己树内的根人类群发（跨树走 C7 的 System）；正文上限 256 KiB、纯零宽正文拒收、保留 category（ask / approval / restart）只经 `post_internal`；内建总机组也有自己的人类群；R3 role 词表含 admin（M3 无行政行为）。
- 问 / 审批 / 重启：新的重启询问取代全部未决的旧询问（各发一条 `restart.changed`）；撤回限提问者、问所在机组的机长与人（祖先机组的机长不行）；`apply_due_defaults` 也发 `AskAnswered{cleared}`；叫提问者回退链 提问者 → 机组机长 → 根机长；`with_book` 不改账本的命令不写库。
- 错误码：-32602 参数错（含数组参数、未知字段）；-32004 不存在（含未知 H 号，R11；`crew.release` 的未知父边）；-32009 状态冲突（问已不开放、转让进行中 / 没有、条目已关、终态、问被占用、审批重复裁决——Mac 与 iOS 同时答同一个问看到的是「已答」）；-32603 内部错（`Rejected::Invariant`；不把 SQL 文本给客户端）。
- 严格参数类型：嵌套在参数里的 @ / 附件 / 成员用 `MentionParam` / `AttachmentParam` / `MemberParam`（`deny_unknown_fields`，经 `From` 转模型类型）；模型类型对库里读出的行保持宽松；两边的形状由漂移测试钉住。R11：解析器收 `null` 的参数在契约里写成 `object | null`。`Response` 线上为 `{id, outcome}`，调用方用 `into_result()`。
- 终审修复：@机长 与 @session 同规则，机长不在群里（topic / dm）即 `Invalid` 并列出成员，大厅 / 根人类群里机长是隐式成员；`crew.delete` 仍级联（人显式的操作），但先把该机组 session 从别的机组的群里摘掉（`left_at`）、它们当群主的群交给该群机组此刻的机长（没有就交给人）、转给它们的待接受转让作废，活下来的群发 `channel.changed`，级联掉的每个群发新事件 `channel.removed {channelId}`；session 发言者在注入面上总带修饰（`sender.session` = 名字 + 分机号、`sender.captain`、无号的 `sender.session.unnumbered`），从不出裸名字，且 session 标题 / 显示名与 topic 名不能等于任一语言的 `sender.human` / `sender.system`（去空白、压换行、不分大小写比较，`Invalid`）；注册 session 与机长指针变动都对大厅发 `channel.changed`；迁移把机长指针之外的 role='captain' 行降为 worker 并发分机号（幂等）；`ask.withdraw` 的 `ask.changed` 用规范 H 号；库里账本读回不合法按不变式处理（-32603，细节不上线）。
- 迁移外键不成立时的报错列出每个 (表, 父表) 与至多 5 个 rowid；store 的字符串词表（`normalize_sender_kind`、`MENTION_KINDS`、迁移 SQL）与 proto 的封闭枚举由测试钉住。

遗留：
- `worldview::render` 是空串（M3b 实现；M3 集成验收必须核世界观非空）。
- 默认方案到点的 `asks::apply_due_defaults` 由 M3c 的定时拍调（C7，M3a 只提供函数）。
- 语言没有设置 RPC（R13），只能写 `meta.locale`（`collab::settings::set_locale`），缺省 zh。
- `message.search` 是全表 `instr(lower(..))` 扫描，只折 ASCII 大小写（量大再加 FTS）。
- `channel.list` 经 `channels::list_for_crew` 懒建人类群，有写副作用，所以走写路径。
- 小项：Db 线程在 serve 关闭时没被 join（关闭次序归 M3c）；`register(Admin)` 不分分机号、不进通讯录；`with_book` 每条命令克隆一次状态；熔断台账的 set / clear 不发 Change；`Crew.titleSource` / `Todo.status` 仍是自由字符串（旧数据）。

给 M3b / M3c 的跨计划注意（执行前要并进各自计划）：
- M3c：排队的 `deferred` 行留着不动、绝不 `mark_dropped`；游标只推进到 `cursor_targets()`（不是排队的最大 seq），`Inflight` 带着它、`on_delivery` 用它；只有积压、`wake_ids` 为空的批也要投；`render_fitted` 不含 deferred；`UNREAD_CAP` 按群计、各群游标独立推进；`build_batch` 报错（账本坏）是 fail-loud，不能紧循环重试；游标后面的排队行会重渲已读消息，先过滤；显式 @机长 + 回复机长会得到 `Captain(x)` + `Session(x)` 两条 mention，同一条消息每个 session 只叫一次；latches 的 get / clear / list 遇坏 id / 坏行回 `Invalid`，启动要处理；`reset_inflight` 至少一次、只在启动时调；serve 关闭次序 = 停止接入 → 收完 / 中止连接任务 → 丢 `AppState` → join Db 线程（`Db` 要有 JoinHandle）；`CollabEvent` 落后（Lagged）时从库重推状态（`restart::pending`、问的 cleared），不只重排未读；停 session 时 `TokenTable::revoke`；`orchestra::schema_entries` 用共享的 `SchemaGenerator`，要么自成一节要么扩 `schema_lists_exactly_the_client_methods`；`blocks_for` / `build_batch` 要传 `Locale`。
- M3b：调用点改 `Response::into_result()`；`agent::schema()` 用共享的 `SchemaGenerator`（`sub::<T>(g)`）后再 `take_definitions`，不嵌套 `$defs`；`blocks_for` / `build_batch` 传 `Locale`。
- M2c → M3c：`driver::with_untrusted_context` 需要同样的帧伪造中和（转义闭合标签 / 校验键）后才能塞队友文字；`ClaudeSpec` 没有 `initial_context`；codex 的 Notice 拒绝要按 MCP server 报；`CommsSpec` / `mcp_args_for` 用 M2c 合进 main 的接口。
