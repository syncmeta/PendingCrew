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
- G3 高频小判断（派给谁、谁收消息、归到哪条、~~侧边栏排序~~）由 Jev 做，机长只接低置信度 **[人]**。 **〔2026-10-07 改，见 §24〕** 侧栏不自动排序（Q4，§22.7 / C1），「侧边栏排序」这项删除。
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
- 机组是多父 DAG（`parentIds`）**[码 LocalCrewStore.swift:416]**，总机组不是存下来的边。→「根机组」= 无存储父边的机组；多父时归属有歧义（§13 Q1）。 **〔2026-10-07 改，见 §24〕** 这是 Swift 版的事实。crewd 里总机组是独立的管理机组、不在树里（H2）；树是森林，人类群归顶层机组（`home_of`）。
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
- **M3 协作**：机组/白板/人类待确认/审批台账、`crewd mcp` 与 `crewd hook` 子命令、唤醒投递与准入。细分见 §22（M3a 数据与 API → M3b agent 面 ∥ M3c 编排 → M4b）。 **〔2026-10-07 改，见 §24〕** 审批台账不做了（runner 自己的权限系统处理）。
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
- **[推]** v1 依赖只限同机组；跨机组靠入口 Todo 的 contact，不做跨组 DAG 边（§13 Q5）。 **〔2026-10-07 改，见 §24〕** `contact` 删除，跨机组说话 = dm；工作账本改为全组织一本（M4b）。

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

### 7.2 三个现行决策（官方 API：`POST /v1/systemone`，`choice`/`score`/`noul`，choice/score 带 `confidence`∈[0,1]）

不自动排序（Q4）；下面保留划掉的历史排序项说明裁定，urgency 实现清理归 M5b。
| 决策 | 问题 | 状态（state） | 执行 |
|---|---|---|---|
| 入口 Todo 派给谁 | `choice`：captain / 每个活跃 session 一项 / new_session / new_crew | Todo 原文 + 活跃名单（角色、名下工作项标题、忙闲、最近活跃**相对排名**而非时间戳） | 绑定并唤醒；new_session → crewd 起 worker，首条唤醒 = Todo 原文 + 机组上下文；**new_crew → 交机长执行**（建组要起名/写章程，Jev 不能生成文字）**[推]** |
| 人类消息谁被叫醒 | 每个成员一道 `noul` | 消息 + 最近几条 + 名单与分工 | 叫醒集合 = p≥阈值；空集 → 机长。**可见性不变**（全员可读），Jev 只决定「谁现在醒」——对齐记忆：broadcast 管看得见、session 管谁醒 **〔2026-10-07 改，见 §24〕** 可见性 = 群成员资格，@ 只决定叫醒谁；broadcast 已删。 |
| 随手消息归属 | `choice`：最近 N 条消息 + 未完成入口/工作项 + none | 消息 + 候选摘要 | 写 reply_to / 关联工作项 |
| ~~侧边栏排序~~ | ~~每机组一道 `score`（紧急度）~~ | ~~机组摘要~~ | ~~与代码算的最近活跃合成排序键~~ **〔2026-10-07 改，见 §24〕** 作废：侧栏由人手排（§22.7 Q4）；urgency 决策一并删除（M5b） |

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
每个**根机组**一个行政处（管整棵子树），按需启动的临时 session，新角色 `admin`。 **〔2026-10-07 改，见 §24〕** 总机组是独立的管理机组、不是根；行政处的管辖范围（按顶层机组还是全组织）留给 M6 重定。

### 8.2 业务 vs 行政
机长 = 业务：做什么、怎么做、做到什么程度、优先级、验收。
行政 = 运转：账目、编制、督办、档案、人的待办。**无业务决策权**：只能基于事实关闭（已完成/重复/被取代，附证据），不能以「不值得做」关闭。

### 8.3 职能 × 执行者

不自动排序（Q4）；秘书的合并 / 等待时长与侧栏机组顺序是不同的事。
| 职能 | 代码 | Jev | 行政 session |
|---|---|---|---|
| 前台/调度 | 名单、阈值、打乱 | 全部判断（§7） | — |
| 人事 | 活跃名单（最近活跃、负载）、并发上限 | 「该 session 是否可下线」 | — |
| 督办 | 计时器、持久队列、停下即检查 | 「回报是真进展还是原地踏步」「是否卡死」 | 需要协调时（谈换人、拆项） |
| 档案 | DAG/链接完整性 | 发现疑似过期/重复/已完成 | 核实（git/群聊）并带证据关闭 |
| 秘书 | 等待时长、提醒节奏 | 合并同类、~~排紧急度~~ | 写给人的简报 |
| ~~排序~~ | ~~最近活跃~~ | ~~紧急度打分~~ | — **〔2026-10-07 改，见 §24〕** 作废（侧栏人排） |

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
- 侧栏机组序由人手动拖拽，不自动排序（Q4）；不把待人类处理清单的等候时长排序当成机组排序。
- 入口 Todo 行内显示「→ W3 W4（2/3）」与负责人。
- 人的待确认清单：~~按紧急度排~~按已等时长排，显示已等时长。**〔2026-10-07 改，见 §24〕** 清单即全组织一个的「待人类处理」收件箱（M4b）。
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
- **每个根机组预置一个「人类群」**：人类参与，相当于以前「仅看 @人类」的过滤视图——要找人类、要人拍板的东西发这里（人类待确认清单的入口也在这里）。 **〔2026-10-07 改，见 §24〕** 改为：每个顶层机组一个人类群，总机组有自己的管理用人类群；不是顶层的机组用顶层祖先（home）的。
- 人类**可以进任何群发言**，但 session **不能默认人类在看**任何群。
- 人类可以把某个群标成**「我在注视」**，让 agent 知道「找人类该发到哪」；**默认不注视**。agent 要找人类时：发到根机组人类群，或发到人类正在注视的群。 **〔2026-10-07 改，见 §24〕** 「根机组人类群」读作「本机组 home（顶层祖先）的人类群」。
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

留给 M5b：上面的别名层与不透明 key 重测；叫醒集合的大小上限（一条消息叫醒过多成员时交机长）。~~侧边栏紧急度的调用缓存（机组状态没变就不重问）。~~ **此旧建议已由 S101 / §24.4 作废：侧边栏由人手排序，不按紧急度自动排序，也不为此保留调用缓存任务；上面的历史实测数不改写。**

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
- 人类群**懒建**：`'<root_crew_id>/human'`，在第一次有人往根机组人类群发言、或客户端 `channel.list` 列到根机组时建。 **〔2026-10-07 改，见 §24〕** id 是 `<home 机组 id>/human`（home = 顶层机组；总机组 = 它自己的）。
- 游标全部钉在迁移那一刻各群的末尾（每个 session 钉自己机组的大厅，human 钉每个群）：**迁移不产生任何未读、红标或唤醒**（#595 全机重放的教训）。代价是旧的 @人类 不再亮，接受。
- 导入的 session 一律 `desired='stopped'`；`extra` JSON 不解析。

群规则 [推]
- 大厅成员 = 本机组全部 session + 人类，**隐式**，不写 channel_members。人类群成员 = 人类 + 根机组机长（隐式）+ 显式行。topic / dm 全部显式。 **〔2026-10-07 改，见 §24〕** 「根机组」读作 home（顶层机组，`crews::home_of`）。
- 只有成员能发言。例外：往根机组人类群发言自动入群（人回话时它才在群里）；system 可往任何群发——跨机组语法糖（`contact` 到别的机组、`report_to_parent`、`message_child_crew`）= system 发言 + 正文首行署名。dm 恰好两人、不能加人，归属发起者的机组。 **〔2026-10-07 改，见 §24〕** system 署名的跨机组语法糖（C7）作废，跨机组说话 = dm。
- 跨机组成员：`channel.list(crewId)` 返回归属该机组的群 ∪ 有成员 session 属于该机组的群（§17「同时显示在每个成员所在机组下」）。
- 已归档的群拒收消息，可以解档。
- @ 在发送时解析成具体目标落库：`captain` → 群归属机组当时的机长 session id（写进 targetId，事后换机长不改历史）；`reply_to` 自动追加 `[broadcast, 被回复者]`（同 main 7af98b6）；目标不是群成员 → 拒，回执列出成员。 **〔2026-10-07 改，见 §24〕** reply_to 不再追加 broadcast，只 @ 原发送者且他仍在群里；broadcast 种类删除。
- §17 的待细化项：不做话题 / 回复串，用 `in_reply_to`；子机组只表达组织结构（部门 / 小组，可有独立工作目录与机长），按话题 / 任务协作用群（人 2026-10-06）。

红标与未读 [推]：不建表，按游标派生。某群 @人类红标数 = 该群 `seq > human 游标` 且 mentions 含 human 的消息数；`read.mark` 推进 human 游标，红标就消。Mac / iOS 共用同一个 human 游标。

根机组：`collab::root_of(crew)` = 没有父边的祖先里 created_at 最早的（§13 Q1）。根机组被收编后，它的人类群自动归档（消息保留）；它账本上的 H 号不动，改在新根的人类群里展示 [推]。 **〔2026-10-07 改，见 §24〕** `root_of` 删除，换成 `home_of`（总机组 → 自己；顶层机组 → 自己；更深 → 顶层祖先里最早建的）。

机组 DAG（移植 `LocalCrewStore.swift:356 attachParent`、`:501 adopt`、`:526 release`）：禁环（含自己挂自己）；内建机组（`builtin`）既不能被收编也不能收编别人；只能 release 直系子；release 带 `to` 时先挂新边、再摘旧边，失败不留副作用；能新建子机组、父机组；只能删非内建、且没有 `desired=running` session 的机组。号码规则照 `CrewDirectory.swift:8-13`（终身不变、不回收、分机 1 = 机长）。 **〔2026-10-07 改，见 §24〕** 内建特例删除；总机组是独立的管理机组、不进树（当谁的父或子都拒绝），不能隐藏 / 移除 / 永久删除；新机组不给父就是顶层机组；删机组分成可逆的「移除」（`crew.remove`）与「永久删除」（`crew.purge`），见 §26.3。

人类待确认（H 号）
- `collab::asks`：每个机组一本 `crewd_work::WorkBook`，整份 `BookState` 存 `work_books`（M4a 定的持久化边界），每条命令在一个 SQLite 事务里读、改、写。M3 只用 `open_ask / answer_ask / apply_default`，外加 M3a 给 crewd-work 补的 `withdraw_ask(by, now, ask, reason)`（提问者 / 机长 / 人可撤；`AskState::Withdrawn{reason, at}`；问还卡着活项时拒绝）。
- 开问时在同一事务里往根机组人类群发一条 @human 消息，`ask_meta` 记下这条消息的 id 和 resume_note。 **〔2026-10-07 改，见 §24〕** 「根机组人类群」读作 home（顶层机组）的人类群。
- 权限审批（照 Swift 驾驶舱 #75②，不阻塞）：`approvals::gate` 一个事务——有票 → 用掉、放行；同 session 同工具已有未决请求 → 只拒；否则开一条、在根机组人类群 @人 → 拒。人同意 = 一次性放行票（同 session 下一次同名调用用掉），决定后给提问者入队唤醒（source `approval:<id>`）。不发 H 号（不是工作卡点），人在 `approval.list` 里看到它。 **〔2026-10-07 改，见 §24〕** **作废**：PendingCrew 不做审批（人 2026-10-07），claude / codex 自己的权限系统处理；v4 删表。

客户端 RPC（`crewd_proto::methods`；`whiteboard.*` 直接删，不留别名）
| 方法 | params | result |
|---|---|---|
| crew.list / crew.get | — / {crewId} | Crew[]（按 position）/ CrewDetail（含 channels、sessions、rootId）**〔N1 落地，见 §26〕** `crew.list {includeRemoved?}`：默认不列已移除的机组；次序 = 总机组在前，再按森林的先序（顶层按 `crews.position`，子机组按父边次序）；`Crew` 多 `management`、`removedAt`；`CrewDetail.rootId` → `homeId`。 |
| crew.reorder | {parentId?, crewIds}（这一层的全部机组，新顺序；只有客户端）| Crew[] **〔2026-10-07 改，见 §24〕** **〔N1 落地，见 §26.3〕** `parentId` 可省（省 = 排顶层机组，写 `crews.position`；总机组不在树里：当 `parentId` 是 -32602 `crew.management_not_in_tree`，放进 `crewIds` 是 -32602 `crew.reorder_mismatch`）；`crewIds` 要含这一层的全部**未移除**机组（不重不漏、无未知 id）；没提到的已移除机组按原相对次序排在后面。 |
| crew.create | {title, workingDirectory, parentId?} | Crew |
| crew.update | {crewId, title?, workingDirectory?, hidden?, statusLine?} | Crew |
| crew.adopt / crew.release / ~~crew.delete~~ | {crewId, parentId} / {crewId, fromParentId, toParentId?} / {crewId} | Crew / Crew / {} **〔N1 落地，见 §26.3〕** `crew.delete` 不存在了（-32601）：`crew.remove {crewId}`（移除 = 归档，可逆）→ 移除后的 `Crew`；`crew.restore {crewId}`（恢复）→ `Crew`；`crew.purge {crewId}`（永久删除，只收已移除的；先快照再删，快照 30 天后轮转删掉）→ `{}`。adopt / release 涉及总机组 → -32602 `crew.management_not_in_tree`；摘掉最后一个父 = 成为顶层机组。 |
| channel.list | {crewId} | ChannelSummary[]（群 + 成员 + watched + unreadForHuman + humanMentions）|
| channel.create | {crewId, kind: topic 或 dm, name, purpose?, members: MemberRef[]} | Channel |
| channel.update | {channelId, name?, purpose?, archived?} | Channel |
| channel.addMembers / channel.removeMember | {channelId, members} / {channelId, member} | Channel |
| channel.watch / channel.setOwner | {channelId, on} / {channelId, owner}（人 = 强制）| ChannelSummary / Channel |
| message.list | {channelId, afterSeq?, beforeSeq?, limit?} | Message[] |
| message.post | {channelId, text, inReplyTo?, mentions?, attachments?} | Message（发送者 = 人）|
| message.search | {query, crewId?, channelIds?, after?, before?, limit?} | Message[] |
| read.mark | {channelId, seq} | ChannelSummary |
| session.list / session.get | {crewId?} / {sessionId} | Session[] / Session **〔N1 落地，见 §26〕** `session.list {crewId?, includeRemoved?}`：给了 `crewId` 照实列（已移除的也列）；不给时已移除机组的 session 只在 `includeRemoved: true` 时列。 |
| ask.list / ask.answer / ask.withdraw | {rootCrewId?} / {crewId, ask, text} / {crewId, ask, reason} | HumanWait[] / Ask / Ask **〔2026-10-07 改，见 §24〕** `rootCrewId` → `homeCrewId`（home = 顶层机组）。 |
| approval.list / approval.decide | {} / {id, allow}（同意 = 一次性票）| Approval[] / Approval **〔2026-10-07 改，见 §24〕** 删除（不做审批）。 |
| restart.pending / restart.answer | {} / {id, answer: resume\|later} | RestartPrompt? / RestartPrompt |
| directory.list | {query?} | DirectoryRow[] |
| todo.list | 不变 | 不变 |

事件（主题 = 事件名第一段）：`message.appended`(Message)、`channel.changed`(ChannelSummary)、`channel.removed`({channelId}，群随机组删除被级联删掉)、`crew.changed`(Crew)、`crew.removed`({crewId})、`badge.changed`({channelId, unreadForHuman, humanMentions})、`ask.changed`(Ask)、`approval.changed`(Approval)、`restart.changed`(RestartPrompt)、`session.changed`(SessionLive，由 M3c 发)、`wake.changed`（M3c，排障）、`events.lagged`。`Message.crewId` 换成 `channelId`，不保留两个字段。M2d 与 iOS 只用这一套；iOS 的远程传输仍按 Q11。 **〔2026-10-07 改，见 §24〕** `approval.changed` 与主题 `approval` 删除。**〔N1 落地，见 §26〕** `crew.removed` / `channel.removed` 改名 `crew.purged` / `channel.purged`，只由 `crew.purge`（永久删除）发；`crew.remove`（移除）发 `crew.changed` + 每个被归档的群一条 `channel.changed`。规则拒绝的错误带 `data.refusal`（稳定码）。

验收
- 真实数据只读演练（照 M1 的做法，在副本上跑）：v2→v3 前后消息条数与 seq 集合一致；每个机组恰好一个大厅；迁移后 human 红标为 0、每个 session 未读为 0；再跑一遍迁移没有变化。
- 群规则每条一个测试：成员资格、dm 只能两人、归档后拒收、人类群自动入群、captain 解析落库、reply_to 追加、跨机组显示。
- DAG：成环 / 自挂 / 内建 / release 非直系子 / release-to 失败不留副作用，每条先写会红的测试。
- H 号：开问后重开库仍在、号连续；与 M4b 将来 block 开的问共用一个计数（直接断言 `WorkBook` 状态）；只有不卡活项时才能撤回。
- 契约：`crewd schema` 漂移测试；全部 params `deny_unknown_fields`；`crewd rpc` 能驱动全部方法。
- Db 线程：一条慢查询进行中，另一个连接的 `system.hello` 照常返回（有测试）。

### 22.3 M3b agent 面

**2026-10-09 现行交付以 §27 为准**：下面保留原始需求及已标注的作废项；helper 为 `post-tool-use|stop|session-start`，注册 PostToolUse / PostToolUseFailure / Stop / SessionStart，不注册 PreToolUse。`CommsSpec` 已无 approval_gates；attach 还带 instructions / surfaceHash，初始化指纹在 serverInfo.version 中。prod launch 拒绝阻塞的 AskUserQuestion、保留 crew alwaysLoad；不是修改用户 harness 的规则。

目标
- `crewd mcp`：stdio MCP server，用官方 Rust SDK **rmcp 3.5.x**（2026-10-06 查 crates.io：最新 3.5.1，2026-10-05 发布，仓库 modelcontextprotocol/rust-sdk；features 用 `server` + `transport-io`，入口 `rmcp::transport::io::stdio()`）。工具表来自 crewd，所以**不用 `#[tool]` 宏**，手写 `ServerHandler::list_tools / call_tool`。helper 只是一根哑管道：连 `crewd.sock` → `agent.attach` → 原样转发。业务全在 serve 里（唯一写者，角色是当下的事实）。
- `crewd hook <post-tool-use|pre-tool-use|stop>`：读 stdin JSON → `agent.hook` → 把返回的 stdout 原样打印，exit 0。
- 工具实现在 serve 的 `server::agent::*`；描述文字在 `crates/collab/prompts/tools.{zh,en}.md`，每个工具一节。
- 世界观：`crates/collab/prompts/world-model.{zh,en}.md`，由 `collab::worldview::render(&WorldCtx) -> String` 渲染（变量照 `LocalSessionWorldModel.swift:73-90`）。claude 走 `--append-system-prompt-file`（M2b 已有通路），codex 走 `developerInstructions`（M2c）。内容按 §17 重写：机组与群的模型；只装自己所在群的消息；不能默认人类在看；注视规则；在没人注视的群 @人类 = 对方开着免打扰时叫他；删掉 plan / continue_work / sweep 那几段。
- `collab::turn_trace`：移植 `SessionTurnTrace.swift`。Stop 时停在一个没叫到人的问句上 → 代发一条：worker 在所在机组大厅 @机长，机长在根机组人类群 @人（`:21-26`，绝不 @ 自己）；按 prompt_id 去重；**永不阻塞**（`McpTurnHook.swift:13`）。同时写 `sessions.awaiting_question`。 **〔2026-10-07 改，见 §24〕** 「根机组人类群」读作 home（顶层机组）的人类群。

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
- post-tool-use：先报产出——工具不是 crewd 自己的（`mcp__crew__*`）→ `sessions::note_progress(ToolCall)`（发言 / 读群不算，§22.7 控制者裁定）；再把本 session 所在的群（大厅 + 显式成员群 + 正在收听的群）里游标之后的未读渲染成 `hookSpecificOutput.additionalContext`（上限 9,000 字符，超了省略最旧的并自报）。可见性（M3a 的 `cursors`，移植 `CrewWhiteboardVisibility.swift:48`）：只有 session / captain 收窄可见范围，human 不收窄，broadcast 放宽，自己发的永远可见。注入后推进游标；这个游标与唤醒共用，一条消息对一个 session 至多注入一次（`HookEmitter.swift:166-221`）。 **〔2026-10-07 改，见 §24〕** 可见性规则改为「群成员全可见」，不再有收窄 / broadcast。
- pre-tool-use（照 Swift 驾驶舱 #75②，**不阻塞**）：tool_name 命中 `approval_gates`（默认 `["computer-use"]`）→ `approvals::gate`：有一次性票 → allow；否则 deny，并（同工具没在等时）开一条审批、在根机组人类群 @人；人同意后它被叫醒、重跑同一调用放行一次。人卡住的只有这一次调用，session 照常干别的。没命中 → 不输出。连不上 crewd：命中的 **deny**。 **〔2026-10-07 改，见 §24〕** **作废**：不做审批，PreToolUse 不注册。
- stop：调 `turn_trace`；连不上 crewd → 什么都不做（宁可漏一次）。
- 任何失败都写 stderr，绝不输出坏 JSON。

工具表（历史基线；**superseded by §27 catalog**，2026-10-09：当前名字 / 合并 / 权限 / hints 以 §27.2 及代码 golden 为准；下面不是当前可调用接口）：
| crewd 工具 | 来源 | 变化 |
|---|---|---|
| post(channel?, message 或 messages[≤6], mentions?, reply_to?, category?) | post_to_crew | 加 channel（缺省：有 reply_to 就发到那条所在的群，否则发本机组大厅）；删 plan / todo / todo_status / todo_reference / evidence* / headline / audience / crew_status；在没人注视的群 @人类 时，回执写明 §17 那句免打扰提示 |
| read_channel(channel?, limit?, before?) | read_whiteboard | 读到最新一条时推进游标 |
| search(query, channel?, after?, before?, limit?) | search_whiteboard | 范围限自己是成员的群 |
| list_channels() / human_watching() | 新 | 后者返回人类正在注视的群 + 根机组人类群 **〔2026-10-07 改，见 §24〕** 「根机组人类群」读作 home 的人类群；M3b 里 `human_watching` 是工具。 |
| channel_create(name, purpose, members[], kind?) / channel_invite(channel, members[]) / channel_leave(channel) / channel_archive(channel) | 新 | 归档：群主或机长 |
| channel_transfer(channel, to, force?) / channel_accept(channel) | 新（§22.7 Q2） | 群主发起 + 对方接受；机长 force |
| channel_listen(channel, minutes?, senders?) / channel_unlisten(channel) | listen | 按群收听（§22.7 Q2） |
| directory(query?) | directory | 加上群 |
| contact(to, message) | contact | 语法糖：`7` → 7 号机组大厅 @它的机长（system 发言 + 署名）；`7-3` → 与该 session 的 dm（没有就建） **〔2026-10-07 改，见 §24〕** 删除：跨机组 = `post(to: "7" 或 "7-3")` 的 dm（M3b T12）。 |
| ask(question, resume_note?, fallback_after_minutes?, fallback?) | ask + add_human_todo | 用 open_ask 发 H 号，不阻塞；fallback → DefaultOption |
| withdraw_ask(ask, reason) | withdraw_human_todo | 原来的 supersedes = 撤旧问 + 开新问 |
| respond_todo(number, response) | respond_todo | 去掉 status / evidence（§9）|
| get_quota / schedule_wakeup(after_minutes 或 at, note) / set_session_profile(model?, effort?) | 同名 | schedule_wakeup 入 `wakes(kind=timer)`，持久化；set_session_profile 下次启动生效 |
| C rename_crew / change_workdir / set_crew_status(text) / read_delegation_policy / update_delegation_policy | 同名；set_crew_status 是从 post 的 crew_status 拆出来的 | |
| C start_session / stop_session / inspect_session / nudge_session / list_sessions / handoff_captain_to_session / create_and_handoff_captain | 同名 | 经 `SessionControl`；start_session 保留 Swift 的 `isolation`（必填 bool，机长逐次定，`McpServer.swift:576`、`:1141`；C8）：`true` = crewd 新建 git worktree，`false` = 机组共享目录；建失败直接报错、不退回共享目录 |
| C report_to_parent / message_child_crew | 同名 | 语法糖：发到父 / 子机组大厅并 @它的机长（system 发言 + 署名） **〔2026-10-07 改，见 §24〕** 删除，同上。 |
| C adopt_crew / release_crew / create_parent_crew / adopt_parent / create_child_crew | 同名 | create_child_crew = collab 建组 + `SessionControl::start` 起子机长 |
| 删 | continue_work / confirm_todo_sweep / plan_add / plan_update / plan_list | 由 M4 取代（§9）|
| 删（Q4） | raise_attention / clear_attention / crew_ordering_signals / arrange_crews | 由 @人类红标取代；侧栏由人手动排，agent 不排序 |

内建总机组上不列 ask / respond_todo / start_session（同 `McpServer.swift:745-753`），调用时再拒一次。 **〔2026-10-07 改，见 §24〕** 作废：总机组没有工具特例。

验收
- 假 claude（真 PTY）+ 真 helper：机长和 worker 拿到的工具表不同；worker 调机长专用工具被拒；中途换机长后不用重启就生效。
- 注入：@ 了别人的不进，@人类的进，broadcast 放宽，非成员群的不进；hook 与唤醒交错时每条消息至多一次。 **〔2026-10-07 改，见 §24〕** 改为：所在群的消息全进，行内标出 @ 了谁。
- pre-tool-use：命中 → 当场 deny、出现一条审批（重试不重复开）；`approval.decide` 同意后同名调用放行一次；serve 停掉时命中的工具被 deny。 **〔2026-10-07 改，见 §24〕** 作废（不做审批）。
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
- @session / @captain（发送时已解析成 session id）→ 叫目标。broadcast / human 不叫任何人（`:74`）。 **〔2026-10-07 改，见 §24〕** broadcast 删除；session 在 dm 里发言 = 隐式 @ 对方（M3c T10a）。
- 人发、没有 @：先问 `Arbiter`（M5b 挂 Jev，候选 = 群成员；M3 的默认实现返回 None）；None / 低置信 → 群主（大厅、人类群的群主 = 机长；dm → 另一方）。
- 正在收听的 session：在 listen 窗口内、发送者匹配 → 叫（与上面取并集，一条消息一个 session 只叫一次）。
- system 发的消息照样参与唤醒，但不做回执判定。
- 优先级：人发的 → human；system 发的与计时 → system；session 发的 → auto。

队列 `wakes`（持久化，取代只在内存里的 `CrewDeferredWakeQueue`）
- 入队存来源身份，不存渲染好的正文（`CrewDeferredWakeQueue.swift:13`），出队时现渲染；靠 `UNIQUE(session, kind, source)` 去重。
- 在 idle 边沿（`SessionEvent::Working(false)`）和计时到点时调度：取该 session 已到期的 pending，**只取最高一档优先级、合并成一次投递**（上限 10 条 / 8KB，剩下的等下一个 idle；8KB 对消息项只是建议值——一个群的窗口不能截半）[推]。Swift 是一次一条（`:50`）；合并能少转几轮空转，也减轻限流压力。
- 投递前过滤掉已注入的：消息类唤醒若游标已越过它（目标忙时 hook 已经注入过）→ 直接记 `delivered`（`CrewSessionRunner.swift:479`）。
- 目标在干活 → 不投，等 idle（claude 忙时 hook 会把未读带进去）。目标没在跑 → 用 `--resume` 拉起，以唤醒正文开场（`CrewLocalMentionWaker.swift:20-21`），受可选上限约束（人类唤醒不受）；在等重启答复的不被自动拉起（§22.7 C4）。
- 投出去记 `inflight`；收到 `Delivery::Delivered` 才记 `delivered` 并推进游标——推到 `WakeBatch::cursor_targets()`（每个群这批实际带上的最大 seq），不是排队行里最大的 seq：唤醒正文每个群带「游标之后」它看得见的最旧的至多 200 条（R6，见 §23），窗口外的排队行是 `deferred`，留 pending、下一批带上；窗口里只有积压（被叫的那几条全在窗口外）时也照样投。失败按 30s / 2min 退避（写进 `not_before`），第 3 次放弃 → 在本机组大厅 @群主（机长自己 → 人类群 @人）。人停掉一个 session → 在同一个 collab 事务里写 `desired=stopped` / `was_working=false`、持久取消水位与全部 pending 作废；提交失败就回失败、不叫停进程。
- **人停的持久取消意图**：现有 `meta` 里每 session 的 `human_stop_wake_floor:<session id>` 保存截至停止事务的最大 wake ID，只前进、不随后来的新唤醒 / 显式 resume / `desired=running` 清掉。`wakes.id` 是 AUTOINCREMENT（prune 之后也不复用）；水位独立于可变的 desired / updated_at。旧 inflight（含 riders、starting 与结算失败后无人持有的行）先保留终局机会：迟到的真实 Delivered 仍把行与游标一起确认；否则后续失败 / requeue / boot 经共享生命周期转移作废，不重试、不退次数、不触发 give-up。新 ID 大于水位的工作照常可以启动、退避与恢复。坏水位 / 读写错误 fail-loud，不静默当 0；没有停止记录的 session 正常按未取消处理。此状态复用现有 meta，无新表/字段、无 schema 版本或 API 形状变更；N4 的快照、身份与迁移保护路径不变。
- 重启时未取消的 `inflight` 按库里 claim 记的次数回 `pending`（不退次数，C25；到上限才放弃）；已有人停取消意图的旧行直接作废、不列入重试 / give-up 通知。消息类重投前先查游标，防重放。已结的行保留 7 天，M3c 定时清；取消水位不随 prune 退回。

环路熔断 `crewd_wake::breaker`（§22.7 Q5；取代 `AutomaticWakeAdmission`，**不移植**它的全机 12 次/分、单机组 5 次/分、单 session 2 次/分——那会挡住 30 个 session 的正常收发）
- 判据：目标 T 上一次有产出之后、10 分钟内送到 T 的非人类唤醒，按「来源 → T」与「T 的非 session 来源合计」两级各超 8 次 → 熔断那一条，落 `wake_latches`；告诉人（根机组人类群）和群主。阈值是一处常量。 **〔2026-10-07 改，见 §24〕** 「根机组人类群」读作 home（顶层机组）的人类群。
- 产出 = `Change::Progress`（M3a `sessions::note_progress`：工具调用、工作项变化）；T 发的消息叫醒了正在叫它的来源以外的人也算。**回话给叫醒你的人不算**（否则互相 @ 的环路永远截不住）。
- 人类唤醒永不计数、永不被挡；system 唤醒照常计数。被挡 = 留在 pending（背压），不是丢掉。只有人能从客户端 `admission.recover` 解。

idle 边沿（取代 `CrewSessionRunner.swift:490-541 runBecameIdle`），按顺序：
1. 到期唤醒合并投递（见上）；投了就结束这次边沿。
2. `IdleHook::on_idle(session, now) -> Option<WakeRequest>`：M4b 的「停下即检查」挂在这里；M3 的默认实现返回 None。
3. 到此为止。删掉：机长 60s 检查点定时器（`:608`）、continue_work 租约、孤儿租约复核、Todo sweep 提醒（`:576`）——都由 M4a 取代。

重启恢复（§22.7 Q1：问人）
- serve 启动：回收孤儿 → inflight 退回 pending → `desired='running'` 的 session **一个都不起**，进待恢复名单，`restart::open` 在相关根机组人类群 @人、客户端弹窗。人答「恢复」→ 机长在前、错开 2s 逐个 `--resume`，`was_working=1` 的开场补一句「crewd 重启打断了上一轮，先核对状态再继续」；答「先不」或不答 → 都不起，唤醒照常入队，之后可整批或单个恢复。等答复期间人在群里 @ 了名单里的某一个 → 只恢复它（算它单个 resume），其余照旧等。 **〔2026-10-07 改，见 §24〕** 「相关根机组」读作各 home（`by_home` 分组）。
- resume 失败（续跑被拒、换新号也起不来，或根本起不来）→ 写 `resume_failed_at`，并在大厅 @机长。M4b 的 `reassign_lost(unrecoverable)` 只认这一列（§19：这份名单只能来自 crewd 自己的恢复失败记录）。
- 计时类唤醒（schedule_wakeup、额度重置、退避）都存在 `wakes.not_before` 里，重启不会丢；serve 只用一个定时任务，按晚于现在的最早 not_before（`wakes::next_due_after`）醒，另有 30s 心跳兜底；心跳顺带调 `asks::apply_due_defaults`（默认方案到点）。

问被回答：M3a 的 `asks::answer` 在同一事务里给提问者入队 `wakes(kind=ask_answer, source='ask:<crew>/H<n>', priority=human)`（H 号按机组发，单独的 `H<n>` 不唯一），正文出队时现渲染（问题、答复、resume_note）；提问者已无法恢复 → 叫它所在机组的机长转达。M3c 只投递，并把 `Change::AskAnswered.cleared` 交给 `AskAnsweredHook`（M4b 的 BlockerCleared 叫醒负责人）。审批有结果的唤醒同理（source `approval:<id>`）。 **〔2026-10-07 改，见 §24〕** 审批有结果的唤醒作废；人在人类群里回复那条问的消息 = 回答它（N1）。

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
| `crewd-collab`：群 / 成员（`members` = 有效成员，含隐式）/ 发消息 / 游标（`cursor`、`unread_for`、`advance`）/ 机组 DAG 与 `reorder` / sessions（含 `note_progress`）/ asks / approvals（`gate`、`decide`）/ todos::respond / wakes（队列读写与清理）/ latches / restart / settings | `crewd-collab` | M3a | M3b、M3c **〔2026-10-07 改，见 §24〕** `approvals` 删除；`root_of` → `home_of`；`cursors::visible_to` 删除（N1）。 |
| `render::{blocks_for, injection, build_batch, wake_text}`（文字在 prompts/render.*.md）、`prompts::fill`（一遍扫描） | `crewd-collab` | M3a | M3b（hook）、M3c（唤醒、codex additionalContext） |
| `CollabEvent` = `crewd_collab::Change`（`MessageAppended`、`WakeEnqueued`、`AskAnswered{cleared}`、`RestartAnswered`、`Progress` …），经 `Hub` 发出 | 类型在 collab，`crewd-server::hub` 重导出 | M3a | M3c、M3b 订阅 |
| `WakeRequest{session, kind, source, priority, not_before_ms, note, origin}` | `crewd-collab` | M3b（timer）、M3a（ask_answer / 审批结果）、M4b（checkpoint / stop_check） | M3c |
| `TokenTable{mint, verify, revoke}` | `crewd-server` | M3c（mint） | M3b（verify） |
| `trait SessionControl`（start / resume / stop / nudge / inspect / set_profile / handoff_captain / quota / live；`BoxFuture`，可做 dyn）+ `FakeSessionControl`；`StartRequest.isolation: bool`（C8；没有路径字段） | `crewd-server` | M3c 实现 | M3b 的工具、M3a 的 RPC |
| `AppState { db, hub, sessions, tokens, agent, orchestra }`（后两个 M3a 放占位类型） | `crewd-server` | M3a | M3b、M3c |
| `CommsSpec{helper, data_dir, session_id, token, hooks, approval_gates}` | `crewd-exec::launch` | M3b 改 | M3c 填 **〔2026-10-07 改，见 §24〕** `approval_gates` 删除（不做审批）。 |
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
- **Q4 旧工具**：`raise_attention` / `clear_attention`（Todo #71 起已不控制任何东西，只是旧会话兼容）删；`crew_ordering_signals` / `arrange_crews`（机长给总机组侧栏排序 + 写一句摘要）删，**侧栏不再自动排序**（人 2026-10-06）：机组顺序由人手动排（拖动），存 `crews.position`（根层）与 `crew_parents.position`（某个父下的次序，每条父边一份，M3a 落地，见 §23），新机组排在同级末尾；没有基础序、没有覆盖层、不需要排序信号。原因：当初自动排是因为子机组太多太乱，而很多原来用子机组做的事（按话题 / 按任务协作）现在应该用机组内的群（§17）来做；组织架构本身不需要排序。
- **Q5 并发上限**：**默认不设上限**；设置里提供可选上限（默认关）。必须防的是**异常唤醒循环**（程序错误导致同一 session 被反复叫醒、两个 session 互相叫醒打转）：按「唤醒来源 → 目标」与「目标 session」两级做熔断，判据是**短时间内重复唤醒且没有新产出**（没有新消息 / 工作项变化 / 工具调用），熔断只停那一条环路并告诉人和群主，绝不按全机总量限流——大量 session 同时正常工作不受影响。阈值与判据写成一处常量，带测试（循环被截住；30 个 session 各自正常收发不被截）。
控制者裁定（2026-10-06，三份 M3 计划对齐时定；正文 §22.2–§22.6 已按此改）：
- **C1 侧栏手动排**：`crews.position`（根层）+ `crew_parents.position`（父下，按边；M3a 落地）+ 只有客户端能调的 `crew.reorder {parentId?, crewIds}`（一次给出一层的完整新顺序），新建 / 收编 / 转挂进来的排在同级末尾；没有任何按动静排序的 API。话题 / 任务协作用群，子机组只表达组织结构（§17）。
- **C2 审批照 Swift 驾驶舱 #75②，不阻塞**（取代原 §22.3 的「等 30 分钟」）：命中闸的 PreToolUse 当场 deny 并给人开一条待确认；人同意 = 一次性放行票，同一 session 下一次同名工具调用用掉它。理由：人卡住的只能是那一件，绝不能卡住整个 session。闸的 matcher 与「连不上 crewd → deny」保留。 **〔2026-10-07 改，见 §24〕** **作废**（人 2026-10-07：不做审批）。
- **C3 产出信号**：M3a 的 `sessions::note_progress(ToolCall | WorkItem)` 写 `last_progress_at` 并发 `Change::Progress`，与 `last_output_at`（发言）分开；回话给叫醒你的人不算产出。M3b 的 post-tool-use 对非 crewd 工具调它，M4b 的工作项命令必须调它，M3c 的熔断消费它。
- **C4 重启询问 vs 人 @**：等答复期间，人在群里 @ 了名单里的某一个 → 只恢复它（算它单个 resume），其余照旧等；启动前就排着的人类唤醒不算。
- **C5** 人类唤醒绕过可选并发上限；其余照 M3c 的默认：环路窗口 10 分钟 / 8 次、人停 session 丢弃它 pending 的唤醒、`set_profile` 下次启动生效。（原写的「`worktree` = 已存在的目录、`start_session` 改收 `workdir?`」已被 C8 取代。）
- **C6** M3b / M3c 要的读写口全部由 M3a 提供并冻结（M3b 的 G3 / G-F / G-S / G-T / G9 / G11；M3c 的 `next_due_after`、`cursor`、`mark_failed` 带退避时刻、`requeue`、`by_ids`、`prune`、`members` 含隐式成员）；任何计划都不往别的计划的表里写 SQL。
- **C7** 默认方案到点由 M3c 的定时拍调 `asks::apply_due_defaults`；ask 唤醒的 source 一律 `ask:<crew>/H<n>`。**VOID（跨机组 system 语法糖半句）**：`contact` / `report_to_parent` / `message_child_crew` 不再 system 发言或正文署名，跨机组走 `post(to:)` 的 dm（§24.2 / §27）；默认方案的定时职责不因此作废。
- **C8 `start_session` 保留 Swift 的 `isolation`**（必填 bool，机长逐次定，取代 C5 里「`worktree` = 已存在目录」）：`true` = M3c 的会话管理器在机组工作目录里新建 git worktree（位置、分支名、slug 照 Swift，见 §22.4），记进 `sessions.worktree`，session 在那里起；`false` = 机组共享目录；建失败 = start 失败并带原因、不写行、不起进程、不退回共享目录；crewd 永不删 worktree（删除是人的动作）。落点：M3a `StartRequest.isolation` 与列注释，M3b `start_session` 工具，M3c `orchestra/worktree.rs` + `Core::start_new`。测试一律用临时 git 仓库（合成），不碰真仓库。
- **C9 一次性放行票的范围 = (session, tool)**（照原计划，对 C2 的补记）：人同意一条审批 = 只放行「同一个 session 的下一次同名工具调用」，不跨 session、不按工具名全局放行。 **〔2026-10-07 改，见 §24〕** **作废**（不做审批）。
- **C10 审批仍是独立的表 / 清单，客户端「等你拍板」视图把 ask 与 approval 合并显示**（红标、侧栏顶层）：合并在客户端做；M3a 的 `Ask`（`crewId`、`askedAt`）与 `Approval`（`crewId`、`requestedAt`）都带机组与创建时间，人类群 = `<根机组>/human`（`crew.list` 的父链推得），`ask.changed` / `approval.changed` 负载就是整条，足够客户端合并、排序、计徽标；不把审批并进 `ask.list`（H 号只给工作卡点，C2）。 **〔2026-10-07 改，见 §24〕** **作废**（不做审批）；「等你」清单只剩问（M4b 的「待人类处理」）。

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
- 错误码：-32602 参数错（含数组参数、未知字段）；-32004 不存在（含未知 H 号，R11；`crew.release` 的未知父边）；-32009 状态冲突（问已不开放、转让进行中 / 没有、条目已关、终态、问被占用、审批重复裁决——Mac 与 iOS 同时答同一个问看到的是「已答」）；-32010 存储故障（N4：磁盘满 / I/O / 库损坏 / 只读 / 打不开，消息里是一句原因 + 日志编号）；-32603 内部错（`Rejected::Invariant`；不把 SQL 文本给客户端）。
- 严格参数类型：嵌套在参数里的 @ / 附件 / 成员用 `MentionParam` / `AttachmentParam` / `MemberParam`（`deny_unknown_fields`，经 `From` 转模型类型）；模型类型对库里读出的行保持宽松；两边的形状由漂移测试钉住。R11：解析器收 `null` 的参数在契约里写成 `object | null`。`Response` 线上为 `{id, outcome}`，调用方用 `into_result()`。
- 终审修复：@机长 与 @session 同规则，机长不在群里（topic / dm）即 `Invalid` 并列出成员，大厅 / 根人类群里机长是隐式成员；`crew.delete` 仍级联（人显式的操作），但先把该机组 session 从别的机组的群里摘掉（`left_at`）、它们当群主的 topic 交给该群机组此刻的机长并让它入群（群主永远是成员），没有机长或是 dm 就交给剩下的成员里最早入群的，一个不剩群主留空（v3 的 CHECK 因此只要求大厅 / 人类群没有群主）、转给它们的待接受转让作废，活下来的群发 `channel.changed`，级联掉的每个群发新事件 `channel.removed {channelId}`；session 发言者在注入面上总带修饰且修饰在名字前面（`sender.session` = 分机号 + 名字、`sender.captain`、无号的 `sender.session.unnumbered`），从不出裸名字，名字写成「人类：…」或别人渲染后的样子也冒充不了；渲染时每个插进去的名字（发言者、回复对象、发给谁（也带修饰）、群头、机组头、唤醒正文里的号 / 工具 / 摘要）都过同一个 `safe_name`（去 Cf / Cc / Zl / Zp、帧括号换成「」、压成一行），旧数据一样中和，且 session 标题 / 显示名与 topic 名不能等于任一语言的 `sender.human` / `sender.system`（压换行、去 Cf / Cc / Zl / Zp、去空白、不分大小写比较，`Invalid`）；注册 session 与机长指针变动都对大厅发 `channel.changed`；迁移把机长指针之外的 role='captain' 行降为 worker 并发分机号（幂等）；`ask.withdraw` 的 `ask.changed` 用规范 H 号；库里账本读回不合法按不变式处理（-32603，细节不上线）。
- 迁移外键不成立时的报错列出每个 (表, 父表) 与至多 5 个 rowid；store 的字符串词表（`normalize_sender_kind`、`MENTION_KINDS`、迁移 SQL）与 proto 的封闭枚举由测试钉住。

- **M3a.1 补充（分支 crewd-m3a1，只加不改冻结签名，M3b / M3c 可直接用）**：`messages::get_by_seq(conn, seq) -> Result<Message>`（没有 → `NotFound`）；`Db::shutdown(self)`（阻塞：已入队的任务先跑完、关通道、join 库线程；其它克隆之后的 `call` 得 `DbClosed`；async 里用 `spawn_blocking` 包，serve 现有行为不变，关闭次序仍归 M3c）；`HealthKindView` 加 `turn_failed` / `version_incompatible`（线上 snake_case，与 `crewd_exec::health::HealthKind` 八档一一对应，契约 schema 已重生成）；`render::safe_name` 与 `render::sender_label(conn, &Message, Locale) -> Result<String>` 公开（注入面 `blocks_for` 也只经 `sender_label`，read_channel / search 的发言者标签与注入逐字相同）；`channels::display_label(conn, &Channel, viewer: &MemberRef, Locale) -> Result<String>`（dm = 对方的修饰标签，旁观者两边都列；其余 = 注入头里的名字槽值：大厅 / 人类群 = 机组标题、topic = 群名，都过 `safe_name`，不含头部文案）。

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

## 24. 2026-10-07 复审：改进计划与裁定

> 来源：2026-10-07 对 M3 设计与旧版使用情况的复审（九份研究，底稿在仓库外的私有目录，含真实使用数据，**不进仓库**）与当天人的拍板。本节只记结论与理由；旧版的教训只写成抽象的形状，不写任何计数。上文与本节冲突的地方已就地加「〔2026-10-07 改，见 §24〕」的标注。
>
> 同一天下午人又改了三处（本节已按最终结论改写，旧说法不再保留）：总机组从「唯一的根」改成**独立的管理机组**（不在层级里，机组树又是森林）；进度表之外**可以自建任意的表**（M4b 走「组织操作系统」方向）；删机组的叫法定为**移除 / 恢复 / 永久删除**。

### 24.1 人的裁定（2026-10-07）

- **H1 被 @ 先回一声**：保留在世界观里（「收到」类回复不改成系统回执）；降噪靠限一句、对其他 agent 折叠、在人的界面里折叠、以及「已送达 / 处理中」指示器，不靠删掉这句话。
- **H2 总机组 = 独立的管理机组（管理层），不在机组层级里**：它不是任何机组的父或子（「你说要出现很多特例，就不要这样做了」，取代当天早些时候「唯一的根、其余机组都挂在它下面」的做法）。机组树是**森林**：顶层机组没有父；`home_of` = 顶层祖先（多父取最早建的，同 §13 Q1）。**每个顶层机组保留自己的人类群，总机组有自己的管理用人类群**；今天一切「找根机组人类群」的地方改成找 home（顶层机组）的人类群。总机组至多一个（`crews.management`），号码 0，不能隐藏 / 移除 / 永久删除，不能进树（当谁的父或子都拒绝），侧栏单独列在最前；不再有「内建机组」的特例分支。
  - **权限**（人：「像所有机组的根机组一样的权限」）：总机组的**机长**在每个机组都按那个机组的机长算（管群、强制换群主、拉人进人类群、撤问；M3b 的机长工具同样）；总机组的**其他 session** 读得到全组织的群、能往任何活的大厅 / topic / 人类群发言（不必入群；dm 除外），其余地方按普通成员。
  - **迁移**：旧数据里的顶层机组原样留在顶层、人排过的顺序不变（不挂到谁下面）；内建机组成为总机组，指向它 / 来自它的边去掉，只挂在它下面的子机组成为顶层、按原次序排在末尾。
- **B15 没有私有群**：两个 session 之间的 dm，所在机组的其他成员也读得到（「不是自己的消息也能看」）。读得到 ≠ 被注入 / 被叫醒。
- **不做审批**：「pendingcrew 不做审批了，claude code 和 codex 会处理好的」。§22 的审批台账、一次性放行票、PreToolUse 审批闸、审批唤醒、客户端审批清单全部删除（C2 / C9 / C10 作废）。屏幕上 runner 自己弹的权限框仍按待决策处理（交给人选，见 24.3 的 T10c）。
- **C26 休眠**：人可以让 crewd「像休眠一样」冻结全部 session（给一个期限让在干活的 session 停在安全点，到点打断、干净退出，排队的唤醒全部保留），开机后在重启询问里列出被休眠的 session，按人的答复续跑并告诉它被挂起过、当时在做什么。归 M3c T9b。
- **移除 / 恢复 / 永久删除**（叫法人定）：「**移除**」机组 = 隐藏并冻结（库里叫归档：消息、session、账本都留着，只读），`crew.remove`；「**恢复**」= `crew.restore`；「**永久删除**」= `crew.purge`，只收已移除的机组，**真的删掉**这个机组的一切。删之前拍一份快照（N4），只是安全网：30 天后随定时备份的轮转删掉。
- **工作模型的命名**：人交代的 = 「**原始需求**」；agent 整理后的 = 「**计划**」（计划项带可读代号，例如 M4b / M4b-1）；要人做的 = 「**待人类处理**」（全组织一个收件箱）。
- **完成不需要「验证过的凭据」**：负责的 agent 自己标完成（可附一段文字凭据），不等任何人或系统验收，免得事情卡在验证上。此前「完成 = 凭据被系统验过」的提议被人否决。
- **工作模型其余要点（人已同意，详细设计在做，归 M4b）**：一本账、三种条目（原始需求 / 计划 / 待人类处理）互相连边；稳定编号全组织唯一、永不复用，另有可读代号，挪到别的父项下按新父项重编、旧代号永久可跳转，最多 3 层；**进度表 = 计划项上的视图**，放进群 / session / 机组的是对同一份视图的引用（改一处处处变），每项只有一个覆盖式备注栏（「不做自由表格」已被下一条取代）；「待人类处理」全组织一个，每条带上下文与推荐选项，可以回答 / 稍后 / 不要了，事项结束自动撤回。
- **自建表与组织操作系统（M4b 方向，取代「不做自由表格」）**：驾驶舱里的表不只是进度 / 计划的视图——任何 session 和人都能建任意种类的表，只要它让人与 agent、agent 与 agent 之间说得更清楚。M4b 按「让组织里的信息沉淀下来形成操作系统」设计一层本体（对象类型、链接、动作、视图；自建表 = 轻量的自定义对象类型）。已拍板：对象**全组织可读**；agent **可以推翻**人的决定；机组被移除时它的对象照样可读、随机组冻结，被永久删除时**连同它的对象一起删**（不转给总机组）；编号前缀 `D`（决定）`L`（教训）`R`（风险）`O`（自定义对象的行）`T`（表）采用。设计底稿在仓库外的私有目录，M4b 写计划时进 spec。
- 维持不变的已有拍板：每个群都有群主（可转让）；重启后先问人；侧栏不自动排序（人手排）。

### 24.2 采纳的消息模型

控制者在人授权「可以重新想」的范围内采纳，已报人、人可否决。借鉴了 Slack / Zulip / Matrix 的做法：**可见性跟着成员资格走，@ 只是通知**。

- **看得见 = 在群里**（加上 B15 的可读规则：本机组名下的群、自己 home（顶层机组）的人类群也读得到；总机组的 session 读得到全组织的群）。
- **@ 只决定叫醒谁**：删掉「@ 某人就让别人看不见」的收窄规则和 `broadcast` 这个 mention 种类（库里的旧数据在迁移时剥掉）。注入给 agent 的每一行标出 @ 了谁。
- **回复只 @ 原发送者**，而且只在他还在群里时（叫不醒的不 @）；回复系统消息不叫醒任何人。
- **dm 里 session 说话默认叫醒对方**（不用再 @）；世界观写明「不 @ 谁就不叫醒任何人」。
- **在人类群里回复一条问 = 回答这条问**：同一事务里记答复、按原有规则叫醒提问者（带 resume note）；问已答 / 已撤时当普通消息。
- **跨机组说话 = dm**：`post(to: 机组号 或 分机号)`，填机组号 = 对方现任机长。`contact` / `report_to_parent` / `message_child_crew` 这类「系统代发 + 署名」的语法糖删除（C7 那半句作废）——旧版里代发的消息回不了话、署名还绕过了防冒名。
- `post` 不再默认发到大厅：没给群 / 收件人 / 回复对象就报错并列出你的群。
- 理由（抽象的教训）：旧版里收窄规则让旁观者只看到回复、看不到问题，而「只跟一个人说」本来就有 dm；两套机制做同一件事，agent 必然用得不一致。降噪不靠藏消息，靠 24.3 里的发帖限速、系统事件不进群、对其他 agent 折叠进度类消息。

### 24.3 改进计划（任务清单）

| 任务 | 一句话 |
|---|---|
| **N0 CI** | CI 里跑 crewd（macOS + Linux，`check.sh`，nextest 重试且 flaky 记成第三态，另跑 doctest）；之后加 cargo-mutants 只报差异。 |
| **N1 M3a.2 协作模型对齐**（main，计划 `docs/superpowers/plans/2026-10-07-crewd-m3a2-collab-alignment.md`） | 删审批；H2 管理机组（schema v4、总机组在森林之外、`home_of` 取代 `root_of`、无 builtin 特例）；24.2 的消息模型在 collab 侧的部分（删收窄与 broadcast、B15 可读、dm 原语、回复即回答）；session 标题、移除 / 永久删除、拒绝文字的稳定码。必须在 M3b T12 与 M3c T10a 之前，且在 N4 之后合 main。交付记录见 §26。 |
| **N3 数据目录** | 数据目录读不了时大声报（健康事件，不把「打不开」当成「空」）；启动自检；运行期文件（token、启动文件、socket）移出数据目录；库的位置等下一次故障时量过再定。 |
| **N4 存储安全** | 迁移前 `VACUUM INTO` 快照（失败就拒绝启动）；在线定期备份与 `crewd backup / restore`；pragma 读回断言；「库应该在却不在」时拒绝启动；完整性检查；存储错误分类成健康事件；永久删除前快照。 |
| M3b T12 | 消息工具：`post.to`、不给目标就报错、每个发送者限速与相同文本合并、回执写明会叫醒谁。 |
| M3b T13 | 群工具：dm 只经 `post.to` 产生；`contact` 不写；对其他 agent 的进度 / 「收到」类消息折叠成摘要；收听与群主转让各合成一个工具。 |
| M3b T14 | 问人：问去 home（顶层机组）的人类群；回执回显原文；定时唤醒的目标已删 / 已归档时只告警一次。 |
| M3b T15a | 会话控制工具（起 / 停 / 查看 / 打断 / 交接 / 配置）：标题必填或从 brief 派生；换机长后旧机长降为普通成员。 |
| M3b T15b | 机组结构：五个改结构的工具合成一个只给机长的 `crew_structure`；新机组排在同级末尾、不动人排好的顺序；工具注解；建子机组没起来机长时说清楚。 |
| M3b T15c | 世界观与文字改版：精简；「先回一声」限一句；写明群主的责任；不 @ 就不叫醒；session 不能代表人同意；管理机组一句话；删 builtin 与审批相关的段落。 |
| M3b T16 | 端到端与真 claude 冒烟；`crew` MCP 标成总是加载（防工具搜索把发言工具藏起来）；启动参数钉死；拦掉 runner 自带的阻塞式提问工具；几条 agent 文字评测场景。 |
| M3c T9a | 重启 / 定时拍 / 停机 / actor：重启回收算一次尝试、actor 崩溃就整体退出、钩子带超时、过期唤醒清理。 |
| M3c T9b | 休眠（C26）。 |
| M3c T10a | DbPort、协作事件转发、SessionControl：按 24.2 改唤醒规划（删收窄与 broadcast、dm 默认叫醒对方）、唤醒正文的裁剪、假实现与真实现共用契约测试。 |
| M3c T10b | 客户端 RPC 与契约：`why_not_woken` 视图、统一的设置读写、hello 带构建身份、未知值兜底、「已送达 / 处理中」的事件字段、存储错误码。 |
| M3c T10c | 通知与待决策：系统事件不进群（只在状态变化时报一次、恢复时再报一次）；屏幕上的待决策框首报就交给人、客户端能直接选；删掉「机长代按」。 |
| M3c T10d | 安全按键：按键前核对屏幕指纹（不一致就不按，带回现在的框）；输入框里已有人打了一半的字时不写入。 |
| M3c T11 | serve 接线与端到端；有序停机；helper 用自己的可执行文件路径；运行期文件移出数据目录。 |
| **M3i-a 合并后集成正确性** | 钩子注入与唤醒投递不再把同一条消息送两遍（预留游标）；每个钩子一条接线测试；多方环路的因果跳数；回执接上「将叫醒谁」。 |
| **M3i-b 合并后安全与可观测** | 只有人的客户端能调人的 API（按对端进程识别 session 子进程）；tracing 日志写到 `~/Library/Logs/PendingCrew/`；panic 隔离；helper 版本不一致时报出来。 |
| **N2 claude 结构化信号** | 用 claude 的钩子报回合开始 / 失败 / 会话号，忙闲边沿以钩子为准、读屏只做矛盾检测；起进程前预检登录；「在干活但很久没进展」报出来；人发的消息显示已送达 / 处理中、长时间没人接时提醒人（不自动接管）。 |

次序：N0、N4、N1 先并行（N1 在 N4 之后合 main）→ M3c 8b → T9a/b → T10a… ∥ M3b T12… T15c → T16 → 两条分支合 main → M3i-a/b → N2、N3 → 切换。schema 版本：N1 占 v4，M4b 的工作账本表顺延为 v5。

### 24.4 驳回 / 推迟（及理由）

| 项 | 处理 | 理由 |
|---|---|---|
| 把「收到」改成已读回执 | 驳回（H1） | 人要的是「知道有人在接」；回执另做成指示器，不取代这句话。 |
| 去掉总机组 | 驳回（H2） | 改成独立的管理机组（管理层），不在机组层级里。 |
| agent 之间的协调对人默认隐藏 | 驳回「隐藏」 | 没有私有群（B15）；改成在客户端里折叠。 |
| 熔断默认自动恢复 | 驳回 | 环路熔断只能由人解（§22.7 Q5）；解法是让熔断看得见、能一键恢复。 |
| 停摆时自动重启或接管机长 | 驳回自动接管 | 重启先问人、换机长归人；只做提醒。 |
| 侧栏紧急度打分（含缓存、确定性评分） | 驳回 | 侧栏不自动排序；urgency 这项 Jev 决策删除。 |
| 删掉 `post.category` | 驳回 | 留给「对其他 agent 折叠进度类消息」用。 |
| 删掉改组织结构的工具 | 改成合并 | 人自己也会让 agent 整理机组；合成一个只给机长的工具，不打乱人排的顺序。 |
| 完成 = 凭据被系统验过 | 驳回（人） | 会让事情卡在验证上；负责人自己标完成，凭据可选。 |
| 审批台账（含一次性票绑定具体调用） | 删除（人） | runner 自己的权限系统已经处理；PendingCrew 不再重做一层。 |
| 客户端 Keychain token | 推迟 | 先按对端进程识别；Keychain 方案要先试。 |
| Claude Code channels / agents 列表当第二把尺子 | 推迟，只观察 | 还在研究预览，没有注入协议。 |
| 整体换成 stream-json / SDK 驱动 claude | 驳回 | 会丢掉 TUI attach（§16）。 |
| loom / turmoil / madsim；给唤醒加令牌桶 | 驳回 | 竞态是消息顺序的问题；令牌桶会伤到正常繁忙的机组。 |
| Litestream；库坏了就删库重建 | 驳回 | 太重；数据只在本机，不能删。 |
| 改由 launchd / SMAppService 托管 | 推迟 | 与「不加登录项」冲突，而且要先量数据目录的故障。 |
| minidump；换 proptest | 推迟 | 收益要等真遇到。 |
| claude 的额度自动续跑与额度唤醒双发 | 推迟到 M3d | 先确认读用户设置的方式。 |
| codex 服务端自动压缩 | 推迟（待核） | 还没读源码确认。 |

### 24.5 待人确认（不阻塞执行）

- 总机组的号码定为 0（`post(to: "0")` = 找总机组机长）。
- 不是顶层的机组的问去顶层祖先（home）的人类群；多父时取最早建的那个顶层祖先（同 §13 Q1 的口径）。
- B15 的「本机组名下的群读得到」只看本机组，不含子机组的群；子树只额外读得到所属顶层机组的人类群——**例外：总机组（管理层）的 session 读得到全组织的群**（控制者 N1-R3，PM 2026-10-07；不分机长与否）。人类群的读 / 发言 / 注入权从 `home_of` 推出，不认陈旧的成员行（机组挪到别的顶层机组下，旧 home 人类群的权利随之没了）。

## 25. N4 存储安全交付记录（分支 crewd-n4-store，需求见改进清单 B2 S20–S32）

- pragma 写死并读回：WAL（拿不到就拒绝打开）、`synchronous=FULL`、`fullfsync=ON`（本机实测单行提交 p50 3.9ms / p99 6ms，关掉是 40µs / 60µs）、`checkpoint_fullfsync=ON`、`busy_timeout=5000`、`journal_size_limit=64MiB`、`foreign_keys=ON`。
- 权限（PM 2026-10-07）：数据目录 0700（已有的去掉组 / 其他人位）；库在打开前以 0600 预建，SQLite 让 -wal / -shm 沿用库文件权限；快照 0600。数据库不加密（FileVault）；离开本盘的导出要加密，未实现，`crewd backup` 只写 `backups/`。
- 身份：`PRAGMA application_id`=「CRWD」，`meta.dbId`，旁放 `crewd.db.id`。`Store::open_data_dir`：库不在而 id 文件或备份在 → `DbMissing`；dbId 对不上 → `DbIdMismatch`（迁移之前就拒绝）；别人的 SQLite 文件拒绝。serve 与 import 都走它。
- 迁移前：非空旧库先 `integrity_check` 再 `VACUUM INTO backups/crewd-v{旧}-pre-v{新}-{ts}.db`，任一步失败 → `MigrationRefused`，库原样不动。
- 备份：serve 起 `backup::Ticker`（D-8，serve 自己的线程，不进 Core）：启动时与每小时一 tick；到点（24h）的那次对在用的库跑 `integrity_check` 并拍 `crewd-auto-*`，其他 tick 只跑 `quick_check`；快照自己过 `quick_check` 才改名；轮转 = 最近 7 个有备份的日子各一份 + 更早 4 个 ISO 周各一份；备份失败或在用的库没过检查时不淘汰。`crewd backup` / `crewd check [--quick]` / `crewd restore --from`（拿数据目录锁；旧库连 -wal/-shm 改名为 `crewd.db.replaced-{ts}`，永不覆盖）。
- 检查连接用 READ_WRITE + `query_only`，不用 `SQLITE_OPEN_READ_ONLY`：只读打开时 SQLite 不解析 CHECK 约束，检查会漏（实测）。快照用真只读连接（`query_only` 会挡 `VACUUM INTO`）。
- 错误：`StorageKind`（DiskFull / Io / Corrupt / ReadOnly / CantOpen）经 `From<rusqlite::Error>` 自动归类（collab 的 From 也走它）；RPC -32010「存储故障：{原因}」+ 日志编号。只读降级、健康事件未做。
- 停机：库线程退出前 `PRAGMA optimize` + `wal_checkpoint(TRUNCATE)`；serve 先停 Ticker、再 `Db::shutdown`（spawn_blocking），之后才放锁。M3c 计划 Task 11 ⑤「只能 drop」那段已过时。
- `crew.delete`：删之前 `Store::snapshot_before_purge` → `backups/crewd-pre-delete-{机组}-{ts}.db`，拍不成不删。默认归档由 N1 做；合并时这一步移到永久删除（purge）上。 **〔N1 落地〕** 快照只在 `crew.purge`（永久删除）上拍；不再永远保留，30 天后随定时备份的轮转删掉（`backup::expire_pre_delete`，见 §26）。
- 旁路文件：`fsutil::atomic_write` / `rename_durably`（rename 后 fsync 目录）；登记表与 jev key 补上目录 fsync；导入时附件逐个 fsync。
- 量级（合成 367066 条消息，约 145MiB，release）：quick_check 0.16s，integrity_check 0.29s，+外键 0.32s，在线备份 0.38s。
- 复审第 1 轮：`crewd.db.id` 在时，打开前只读偷看——没有 dbId 的库（典型：迁移前快照被 `cp` 回来）→ `DbUnmarked`，空文件 → `DbMissing`，dbId 不同 → `DbIdMismatch`，都在任何 pragma / 迁移之前，文件不动；提示「挪出数据目录再 `crewd restore --from`，或删 `crewd.db.id` 认领；找原库看 backups/ 与 `crewd.db.replaced-*`」。认人（application_id / 版本 / 有无表）挪到 pragma 之前。检查连接与 restore 的检查点连接也设 fullfsync / checkpoint_fullfsync。快照 + 删除在同一个库线程任务里：`Store::snapshot_then`、`AppState::collab_after_snapshot`（N1 的 `crew.purge` 复用它）。backups/ 收紧到 0700，快照临时文件以 0600 预建，Ticker 启动时清死进程留下的 `.*.tmp-<pid>`；socket 绑不上时同样先停 Ticker、关库再放锁。
- 复审第 2 轮：偷看改成 `immutable=1` 只读主文件（旧 WAL 不能替换进来的库「作证」，也不留旁路文件）；写 `crewd.db.id` 之前先检查点，保证 dbId 在主文件里；`crewd.db.id` 第二行记库文件 inode，inode 变了而旁边有非空 -wal → `DbReplaced`。restore 不管库在不在，残留 -wal/-shm 一律挪成 `replaced-*`。快照前先跑只读前置条件（`crews::check_deletable`）：删不了的不拍整库快照。Ticker 启动时同步清死进程的临时文件（backups/ 与数据目录）；Ticker 起不来也先关库再放锁。

## 26. N1 交付记录与遗留（M3a.2 协作模型对齐；计划 `docs/superpowers/plans/2026-10-07-crewd-m3a2-collab-alignment.md`，分支 crewd-n1-collab）

> 本节不写真实数据的计数（演练读数在仓库外的私有任务报告里）。§24 是本计划的来源，§25 是 N4 的交付记录（合并时 N4 的那节编为 §25，所以本节是 §26，不是计划里写的 §25）。计划按「总机组 = 唯一的根」写成；同一天人改成「总机组 = 独立的管理机组」（§24.1 H2），分支上合 main 之前返工（N1b），本节按**最终落地**的样子写，中间形状只在 26.2 提一句。

### 26.1 已交付

- **Task 1 删审批**：collab `approvals` 模块、`approval.{list,decide}`、`approval.changed` 事件与主题、`WakeBody::Approval` 与唤醒来源 `approval:`、渲染文字里的审批段全删；`API_VERSION` 2 → **3**；老客户端调 `approval.*` 得 -32601；有一条「渲染文字里不许出现审批」的守卫测试。
- **Task 2 schema v4**（一个事务）：删 `approvals` 表；`crews.builtin` 改名 **`management`**，部分唯一索引 `crews_one_management` 保证至多一个；`crews.archived_at`；作废排队中的审批唤醒；剥掉 `broadcast` mention；没标题也没显示名的 session 补标题。`backfill_v4` 迁移与旧 JSON 导入共用（同一份数据两条路结果一样，有对拍测试）：多个内建机组只留最早建的；内建机组成为总机组——号码 0（没号且 0 没人用时）、`position` 0、不隐藏、**去掉指向它 / 来自它的边**；只挂在它下面的子机组成为顶层、排在现有顶层之后、保持原次序；旧顶层机组原样留在顶层。失败整体回滚、报「v4 迁移失败，已回滚（库保持 v3）」。v4 接在 N4 的 `migrate_guarded` 链上：迁移前快照在 v4 任何 DDL 之前。
- **Task 3 `home_of` 取代 `root_of`**：一切「找根机组人类群」改成找 home 的人类群；home = 总机组自己 / 顶层机组自己 / 更深 = 顶层祖先里最早建的（多父同 §13 Q1）；遇环退回自己（全函数）。`is_home`、`crews::is_management`（一条 SQL）。
- **Task 4 / N1b 森林规则**：builtin 特例删除。总机组不进树：`create(parent=总机组)`、`adopt` / `release` 任何一端是它、`reorder(parentId=总机组)` 一律 Invalid `crew.management_not_in_tree`；隐藏 / 移除 / 永久删除它 → Forbidden `crew.management_fixed`。不给父的新机组排在顶层末尾；`adopt` 只加边（顶层机组被收编 = 不再是 home，它的人类群归档）；摘掉最后一个父 = 成为顶层机组、排在末尾，原来的人类群解档。顶层次序存在 `crews.position`（`crew.reorder` 不给 `parentId` 时改它），子机组次序存在父边上。总机组懒建（空库第一次 `crew.create`，或回填时库里有机组）。
- **Task 5 @ 只决定叫醒谁**：删 @ 收窄（`cursors::visible_to` / `directed_targets`）与 `MentionKind::Broadcast`；注入的每一行标出 @ 了谁（`MsgLine.mentions`）；回复只 @ 原发送者且只在他还在群里时，回复系统消息不 @ 任何人；导入时也剥 broadcast（报告键 `broadcastMentionsDropped`）；`Mention` / `MentionKind` 不再有 `Default`（编译期守卫）。
- **Task 6 B15 可读 + dm 原语**：`channels::readable_by` / `readable_channels`（⓪ 总机组的**任何** session 读全组织；① 成员；② 挂在本机组下的群；③ 自己 home 的人类群；人类群的权利从 `home_of` 推出、不认陈旧成员行）；`channels::dm_peer`；`directory::resolve`（`"N"` / `"N-1"` = 机组此刻的机长，`"N-E"` = 分机，`"0"` = 总机组）；唤醒批次按**成员资格**（`channels_of_session` ∪ 收听行）放行，不按「读得到」。收听要求读得到且群没归档（`listens::set` 拒 Forbidden `listen.not_readable`；之后失去读权的收听行留着，但在注入面、唤醒名单、`build_batch` 三处都不算）。
- **Task 7 人类群里回复一条问 = 回答它**：人回复一条开着的问 → 同一事务里答问、叫醒提问者（不再额外 @）；`Posted.answered_ask`（只在 collab 内，不上线）；问已答 / 已撤 / 已默认时当普通消息；session 的回复不算回答。
- **Task 8 session 标题**：`sessions::TITLE_MAX_CHARS = 18`（按码点数，先去掉不可见字符、压空白）；`register` 要标题或 brief（从 brief 第一条非空行派生），两样都没有 → 拒绝；v4 回填没标题也没显示名的旧 session。
- **Task 9 移除 / 恢复 / 永久删除**：`crew.remove` 归档并返回 `Crew`；`crew.restore`；`crew.purge`（只收已移除的，经 N4 的 `collab_after_snapshot` 先快照再删）；已移除的机组及其群只读；`crew.list` / `session.list` / `directory.list` 默认不含；`resolve` 到已移除机组的号 → Conflict `directory.crew_removed`；`wakes::enqueue` 对已移除机组的 session 返回 `Enqueued::Skipped`（不写行、不发变更）；`asks::open` 拒已移除机组的 session。删前快照 30 天后随定时备份的轮转删掉（`backup::expire_pre_delete`，按文件名里的时间；备份失败或在用的库没过检查时不删）。
- **Task 10 拒绝文字的稳定码**：`CollabError::{NotFound,Invalid,Forbidden,Conflict}` 装 `Refusal { code, args }`；码 = `area.name`，全集在 `collab::refusal::Code`，文字在 `crates/collab/prompts/refusals.{zh,en}.md`（每句都说下一步怎么办；两种语言节名与占位符由测试钉住；collab 源码里的散文由扫描测试拦住）；`RpcError.data.refusal` 把码带给客户端（只加不改）。
- **N1b 管理机组的权限**：`crews::is_management_session` / `is_management_captain` / `acts_as_captain_of`（`pub`，M3b 的机长工具用它）；总机组机长在 `channels::may_manage` / `force_owner` / 人类群拉人 / `asks::withdraw` 上按那个机组的机长算；总机组的其他 session 不行。`messages::post`：总机组的 session 可往任何活的大厅 / topic / 别的 home 的人类群发言而不入群（dm 不行；已归档的仍 `channel.archived`；@ 仍只认群成员）。
- **Task 11 / N1b-2**：真实数据只读演练两次（26.4）；本节与 README。

### 26.2 与计划不同的地方（含改了期望的老测试）

- **单根 → 管理机组**：计划与 Task 2–4 实现的是「总机组 = 唯一的根」（旧根挂到总机组下、I-root 不变式、`crew.reorder` 必须给 `parentId`、`org_root` 标记）；人改主意后 N1b 返工成森林 + 独立的管理机组（v4 未发布，就地改 DDL，没有 v5）。返工删掉的：`hang_under_org_root`、根相关的拒绝码 `crew.move_to_first_level` / `crew.release_to_root_other_parents`、「从根摘出是空操作」「挂到根下是空操作」类测试；改名：`org_root` → `management`、`crew.org_root_fixed` → `crew.management_fixed`、「一级机组」→「顶层机组」。这段时间里用分支建出来的 v4 库（列名 `org_root`、子机组挂在总机组下）不迁移，从 v3 重来。
- **改了期望的老测试**（每条都因为语义按裁定变了，不是为了变绿）：
  - socket：`approval.decide` 从 -32004 改断言 **-32601**（钉住「不留空壳方法」）；旧名 `crew.delete` / `crew.unarchive` 断言 -32601；`crew.create` 多一条新大厅的 `channel.changed`（总机组不是谁的父，不再发它的 `crew.changed`）；`crew.list[0]` 是总机组（`management: true`）；归档群发言从 -32602 改成 **-32009** 并断言 `data.refusal == "channel.archived"`；机组往返测试断言 `removedAt`、默认列表不含、`includeRemoved` 列出、恢复后回来；永久删除没移除的机组断言 `crew.not_removed`、改已移除的机组断言 `crew.removed`；顶层 `crew.reorder` 不给 `parentId`；永久删除的事件是 `crew.purged` / `channel.purged`。
  - store：`user_version` 3 → `SCHEMA_VERSION`；v1 库迁移后多出懒建的 `pendingcrew-chief`；`migration_resolves_untargeted_captain_mentions…` 不再期望 broadcast；v3 迁移断言旧顶层原样、总机组无边、`position` 0；N4 的「删前快照永不轮转」改成「按份数的轮转不碰它、30 天后按日子过期」。
  - collab：收窄相关的测试删掉、改成「@ 只叫醒、不藏消息」；匹配中文拒绝字符串的断言改成断言码 / 占位符值；老的 `crew.delete` 测试改成「先归档再 purge」；根相关的结构测试换成森林不变式（总机组没有边；每个非管理机组的 home 是没有父的非管理机组；无环；被拒的操作什么都不变）；几个测试原先用 `set_parents(…, [chief])` 造「一级机组」，现在顶层机组本来就没有父，删掉了这步；`asks_survive_reopening_the_database` 注册 session 时给标题；撤问原因文字「机组已归档」→「机组已被移除」。
  - legacy：导入的顶层机组不挂到内建机组下；加了「内建机组的子机组变顶层」与迁移对拍的用例。
  - e2e：加了打总机组自己的人类群（`pendingcrew-chief/human`）的用例，方法覆盖检查因此按方法名去重。
  - crewd `storage_cli`：备份 / 恢复测试改按标题断言（`crew.create` 会懒建总机组，机组个数不再是原来的数）。
- **范围外但按裁定做了**：`crew.purge` 有 RPC（计划写「不开 RPC」，裁定改成开、经快照）；移除时撤回该机组开着的问（恢复不重开）、有 `desired=running` 的 session 时拒绝、开着的问还卡着活项时拒绝、有 inflight 唤醒时拒绝（`crew.wakes_inflight`）、只作废 **pending** 唤醒；已移除的机组拒绝 `crew.update`（先恢复）；`remove_member` 在归档群上与 `add_members` 同一个拒绝；`list_for_crew` 不列陈旧的跨 home 人类群；一个已归档（不再是 home 的）人类群的群主只作显示（「群主 ∈ 成员」只对活群成立）；永久删除的事件改名 `*.purged`（`crew.removed` 会被读成可逆的 `crew.remove`）。
- v4 的任何错误都包成 `StoreError::Invalid`（变体丢了、原因留着；唯一调用方是开库 → 拒绝启动）。
- 线上与内部的叫法分开：**线上（方法、字段、参数、机组一级的拒绝码）与文字一律叫移除 / removed**——`Crew.removedAt`、`includeRemoved`、`crew.removed` / `crew.parent_removed` / `crew.not_removed` / `directory.crew_removed` / `ask.remove_blocked`；**群一级的仍叫归档 / archived**（`channel.archived`、`Channel.archivedAt`、`channel.update {archived}`、`channel.fixed_no_archive`：人手动归档一个群是另一回事）。库与 collab 内部仍叫归档（列 `crews.archived_at`、`crews::archive` / `unarchive` / `is_archived`、`list(conn, include_archived)`、`Change::CrewRemoved`），只在线上换名。

### 26.3 客户端记录（switch / iOS 要跟着改；契约 v3，未发布、不留别名）

- **审批全删**：`approval.list` / `approval.decide` → -32601；没有 `approval.changed` 与主题 `approval`。
- **线上的「移除」**：机组的 `Crew.archivedAt` → **`removedAt`**，`includeArchived` → **`includeRemoved`**（`crew.list` / `session.list`）；群的 `Channel.archivedAt` / `channel.update {archived}` 不变（群的归档是另一回事）。
- **管理机组**：`Crew.builtin` → **`management`**（bool，必有）；至多一个，号码 0，`parentIds` / `childIds` 永远是空的（不是任何机组的父或子）；`crew.list` 把它放在最前，侧栏单独列。`Crew.position` 只表示顶层机组之间的次序（总机组恒为 0）；`parentIds` 非空时这个值没有意义，子机组的次序看父的 `childIds`。`crew.list` 的次序 = 总机组，再按森林的先序（顶层按 `position`，子机组按父的 `childIds`）；客户端按 `parentIds` / `childIds` 建树。
- **移除 / 恢复 / 永久删除**（文案用这三个词）：
  - `crew.remove {crewId}` = **移除**：隐藏并冻结、什么都不删，返回移除后的 `Crew`（读 `removedAt`），发 `crew.changed` + 每个被归档的群一条 `channel.changed`。
  - `crew.restore {crewId}` = **恢复** → `Crew`（大厅解档；是 home 就把人类群解档；topic / dm 留归档，人逐个解档）。
  - `crew.purge {crewId}` = **永久删除** → `{}`：只收已移除的（否则 -32009 `crew.not_removed`），总机组 -32003 `crew.management_fixed`，不存在 -32004，快照拍不成 -32010 且什么都不删；发 **`crew.purged`** + 每个被删的群一条 **`channel.purged`**（旧名 `crew.removed` / `channel.removed` 没有了）。确认文案要写明：真的删掉这个机组的一切，包括它的 home 人类群（含下级机组发的帖与问的公告）和挂在那里的 dm；删前快照只是安全网，30 天后自动删掉。M4b 起它的对象一起删（§24.1）。
  - 旧名 `crew.delete` / `crew.unarchive` → -32601。
- `crew.list {includeRemoved?}`、`session.list {crewId?, includeRemoved?}`：默认不含已移除的机组（及其 session）；`Crew.childIds` 照实含已移除的子机组，列表里没有的 id 跳过。`directory.list` 不列已移除的机组。
- **`crew.reorder {parentId?, crewIds}`**：`parentId` 省略或 `null` = 排顶层机组（改 `crews.position`）；给了就排那个父下的子机组。`crewIds` 给这一层的全部**未移除**机组（不重不漏、无未知 id），已移除的可给可不给（不给的按原相对次序排在后面）。总机组当 `parentId` → -32602 `crew.management_not_in_tree`；放进 `crewIds` → -32602 `crew.reorder_mismatch`。`parentId` 给的父已移除 → -32009 `crew.removed`（移除 = 冻结，次序也不能改）。
- `crew.create` 不给 `parentId` = 新的顶层机组、排在末尾；`parentId` = 总机组 → -32602 `crew.management_not_in_tree`。`crew.adopt` / `crew.release` 任何一端是总机组 → -32602 `crew.management_not_in_tree`（以前是 -32003）；`crew.release` 摘掉最后一个父 = 成为顶层机组。`crew.update {hidden:true}` 对总机组 → -32003 `crew.management_fixed`。
- `CrewDetail.rootId` → `homeId`（home = 顶层祖先）；`ask.list {rootCrewId}` → `{homeCrewId}`；`DirectoryRow.orgPath` 从顶层机组开始，总机组与顶层机组是空串。
- **规则拒绝**：`RpcError.data.refusal` = 稳定码（如 `channel.archived`），`message` 只是中文缺省；客户端按码本地化。码与两种语言的文字在 `crates/collab/prompts/refusals.{zh,en}.md`。N1b 的变化：新增 `crew.management_not_in_tree`、`listen.not_readable`（agent 的收听工具用，客户端不会收到）；改名 `crew.org_root_fixed` → `crew.management_fixed`，机组一级的「归档」码改叫 removed：`crew.archived` → `crew.removed`、`crew.parent_archived` → `crew.parent_removed`、`crew.not_archived` → `crew.not_removed`、`directory.crew_archived` → `directory.crew_removed`、`ask.archive_blocked` → `ask.remove_blocked`（群一级的 `channel.archived` 不变）；删除 `crew.move_to_first_level`、`crew.release_to_root_other_parents`。`work.*` 的拒绝与参数解析错误还没有码（见 26.5）。
- **错误号变化**：往归档的大厅 / topic / dm 发言 -32602 → **-32009**（`channel.archived`）；往归档的非 home 人类群发言 -32602 → -32009；`channel.addMembers` / `channel.setOwner` 遇归档群 -32602 → -32009；`channel.removeMember` 遇已移除的机组或归档群 → -32009；`crew.reorder` 不给已移除的机组 → 成功（以前报不一致）；总机组的结构操作见上。
- **`MentionKind` 少了 `broadcast`**：`message.post` 带 `{kind:"broadcast"}` 现在是参数错误；存下的消息不再带它（v4 迁移与导入都剥掉）；回复自动 @ 的是 `[发送者]`，不再是 `[broadcast, 发送者]`。
- **服务端语义**（客户端看不到线上变化，但行为变了）：总机组的 session 读得到全组织的群，能往任何活的大厅 / topic / 人类群发言而不入群（所以群里可能出现非成员的发言者）；总机组的机长在每个机组都按机长算。对已移除机组的 session 入队唤醒是空操作（`Enqueued::Skipped`）。

### 26.4 真实数据只读演练（副本，2026-10-07，两次）

- 做法（两次相同）：旧 app 数据目录只读复制到会话 scratchpad（不含附件 / 备份 / 崩溃日志，读数用不到）；用 main 上的 crewd（v3，N1 的合并基点）导入副本得到一个真正的 v3 库；把这个数据目录整个挪（不是复制，保住 `crewd.db.id` 记的 inode）给一个只调 `crewd_store::Store::open_data_dir` 的临时程序（与 serve / import 开库是同一个调用；**没起 serve**），读数、关库、再开、再读数；另用本分支的 crewd 直接导入同一份副本（新库直接到 v4）逐表对拍。副本与产出用完即删。第一次在单根形状上做（Task 11），第二次在 N1b 返工之后做（N1b-2），以第二次为准。
- 判据（第二次全部通过）：`user_version` 4；恰好一个管理机组，就是 v3 里的内建机组（没有另建），号码 0、`position` 0、没隐藏、没有任何指向它或来自它的边、没有机组以它为父；顶层机组 = v3 的顶层机组（次序不变）+ 原先只挂在内建机组下的子机组（接在后面、原次序）；原有父边与次序一条不少；`broadcast` mention 剥净，消息只有 `mentions` 一列变了；`approvals` 表不存在；没有 pending / inflight 的 `approval:` 唤醒；没有标题也没有显示名的 session 为 0（回填只动了这一类行的标题）；外键不成立 0、`integrity_check` ok；`backups/` 里恰好一份 `crewd-v3-pre-v4-*.db`，它是 v3、过完整性检查、与迁移前的库逐表相同；再开一次读数逐项不变、也没有第二份快照；`crews.number` 没有撞号；迁移出的库与直接导入的 v4 库**逐表**（全部列）相同，只差 `meta` 里的导入时间 / 库 id / 导入报告（导入报告多一个 `broadcastMentionsDropped` 键）与 `read_cursors.updated_at`（导入时刻）。
- 判据的边界（读数里看得出、但这份数据覆盖不到的）：旧 JSON 导入不产生唤醒，也不建人类群（人类群懒建），所以「审批唤醒被作废」与「顶层机组的人类群仍未归档」是空判据；这份数据里内建机组下面没有子机组，「只挂在内建机组下的子机组接到顶层末尾」也是空判据——这三条由单元测试（store 与 legacy 的对拍）和 `a_v3_data_dir_is_snapshotted_then_migrated_to_v4_through_the_guarded_open` 覆盖。**serve 写过的 v3 库没有演练**（切换时才会有）：开发期 serve 跑出来的 v3 库里，总机组 / 它的人类群里的 session 与消息在管理机组的改动之后意义可能变了（例如原先挂在总机组下的机组成了顶层，旧 home 人类群里的成员行不再给权利）。超过 18 字的 session 标题：这份数据里不存在，规则也不保证（26.5）。
- 量级：本机 release 构建下，带迁移前快照（integrity_check + `VACUUM INTO`）的整条 v3 → v4 在几秒内完成，再开一次在毫秒以下；具体读数在私有报告里。

### 26.5 遗留

- `work.*` 的拒绝（`Work(Rejected)`，英文）与 `conn.rs` / `dispatch.rs` 的参数解析错误还没有稳定码，只有文字；要的话另开一项。
- 收听的 `until`：collab 不读时钟，`build_batch` 对有收听行的群一律放行；M3c 要把 `now_ms` 传进 `build_batch`，过期的收听不再放行。
- 旧数据的标题：v4 回填用「机组标题 + ` #` + 分机号」，不截断，可能超过 18 字；任何改名路径都不能因为旧标题超长而拒绝。
- `crews.number` 没有唯一索引（撞号时 `resolve` 取最早建的）；两次演练都没有撞号，建议下一次升 schema 版本时加 `CREATE UNIQUE INDEX … ON crews(number) WHERE number IS NOT NULL`（迁移前先查撞号，有就拒绝并列出）。
- M3c：唤醒准入与重启询问的答复要按 session 跳过已移除机组的 session（不让整批失败）；`claim` 可能撞上在 `due_for` 与 `claim` 之间被移除作废的行 → 这一批失败、下一拍再来。
- `answered_ask` 不上线（`message.post` 只回 `Message`，客户端从 `ask.changed` 知道问被答了）；要「这条回复答了 H3」的回执是 M3b T12 / 契约的事。
- M3b：总机组的 session 往自己不在的群发言后，别人回复它**叫不醒它**（@ = 叫醒，只在成员里选）；要不要让这种发言顺手入群或订阅回复，T12 定。
- `readable_channels` 扫全表；`list_for_crew` 仍有懒建的写副作用（同 §23）。森林不变式（总机组没有边、至多一个、不能隐藏）只在 collab / 回填里保证，不是库约束（`crews_one_management` 唯一索引除外）。
- `ensure_management` 在没有 `management=1` 的行时直接插 `pendingcrew-chief`；若有同 id 的普通机组会主键冲突、整个迁移回滚（大声失败；Swift 数据里总机组总是内建的，到不了）。
- M4b：永久删除机组时连同它的本体对象一起删，移除时对象冻结但照样可读（§24.1）；对象表进来时 `purge` 要跟着删。
- 文档：§22 里讲审批与单根的旧文字已就地标注；M3a / M3b 计划里的旧文字没改（计划是历史记录），M3b / M3c 合并时以 §24 与本节为准。

### 26.6 给 M3b 的合并注意（N1 合 main 后 M3b `git merge main`；行号以 grep 为准）

不全是编译错误：有些名字有意删掉、不留别名（编译期就报），有些是**只在运行时 / 语义上**变了（编译照过），下表标「静默」的那几行要专门看。

| M3b 的位置 | 变化 | 怎么改 |
|---|---|---|
| 审批：M3b 已在分支上删掉 PreToolUse 审批闸与 agent 侧的审批调用（`87c2193`）；M3b 分支上 collab 的 `approvals.rs` 与 `render.{zh,en}.md` 的审批节还是 M3a 的原样 | N1 删了 collab 的 `approvals` 模块、`Change::ApprovalChanged`、`WakeBody::Approval`、`wakes::source::approval` 与 render 的审批节 | 合并取 main 的删除；两道守卫都留：M3b 的 `worldview.rs` `nothing_the_agent_reads_mentions_an_approval_gate` 与 collab 的 `prompts::tests::render_text_has_no_approval_wording` |
| `collab/src/worldview.rs`（`root_of`、`builtin_crew`、`role.captain.builtin` / `ask.builtin` / `builtin` 节；第 22 行注释说「一级机组」）、`collab/src/turn_trace.rs`（`root_of`） | `crews::root_of` / `is_root` 删；`Crew.builtin` → `management`；无 builtin 特例；森林 | `crews::home_of`（= 顶层祖先，人类群）+ `crews::is_management`；三节删，换成「管理机组」的一句（T15c）；`WorldCtx` **取 M3b 的结构**：M3b 有意删了 `channels` / `human_watching`（`human_watching` 变成工具），不要把 N1 的两个字段带回来；人类群 id 用 `crews::home_of` 算（M3b 的 `root_human_channel_id` 建议顺手改名 `home_human_channel_id`）；`builtin_crew` / `crew.builtin` 换成 `crew.management`，在 T15c 删掉 builtin 三节之前先这样顶着；第 22 行（N1 已改成「顶层机组」）取 N1 的；`turn_trace` 用 `home_of` + `channels::ensure_human_channel` |
| `server/src/agent/port.rs` 的 `cursors::visible_to` 过滤（read / search）、`Reader`（roots、`inside`、`can_read`）、`set_listen` 的「别的树」门 | `visible_to` 删；B15 规则在 collab | 不再过滤；可读范围 = `channels::readable_by` / `readable_channels`（含 ⓪ 总机组读全组织）∪ 收听；删 M3b 自己的 `Reader`；收听工具直接调 `listens::set`——它已经要求读得到（拒 Forbidden `listen.not_readable`），不要另做一道门；`messages::search` 一律传 `channel_ids = readable_channels ∪ 收听` |
| `port.rs:377` `crews::list(conn)` | 签名变成 `list(conn, include_archived)`；号码解析在 collab | 按需要传 `include_archived`；号码一律走 `directory::resolve`（`"0"` = 总机组；已移除的机组 → Conflict `directory.crew_removed`，原样回给 agent） |
| `port.rs` 的 `From<CollabError>` 与 `CollabError::Forbidden("…".into())` | 四个变体装 `Refusal`（`collab::refusal::{Code, Refusal}`） | `Self::Refused(r.text(locale))`（agent 的语言）；需要新的拒绝就加码 + 两种语言的 md 节，不在代码里写句子（扫描测试会红） |
| `port.rs` 里 `builtin` / `builtin_crew` 与 `not_on_builtin` | `Crew.management` | 映射到 `management`；T15b 删 `not_on_builtin`；`ask` 不再限制总机组（N1-R9：它的问去它自己的管理用人类群） |
| `port.rs` 里人类群 = `ensure_human_channel(root_of(crew))` | 同上 | `home_of`（顶层祖先） |
| T15b 机长工具（`crew_structure` 等）的权限判断 | 总机组的**机长**在每个机组都按机长算；总机组的其他 session 不算 | 用 `crews::acts_as_captain_of`（`pub`）或 `is_management_captain`，不要只比 `crew.captain_session_id`；结构操作碰到总机组时 collab 会拒 `crew.management_not_in_tree`，原样回给 agent |
| T12 `post`（**静默**） | 总机组的 session 可往任何活的大厅 / topic / 人类群发言而不入群（collab `messages::post` 已放行，dm 除外） | 工具层不要另加「必须是成员」的门；回执要说清它不在群里、别人回复叫不醒它（26.5） |
| `agent/catalog.rs:51` 与 `:435` 的 `MENTION_KINDS`（含 `"broadcast"`）；`collab/prompts/tools.{zh,en}.md` 第 10、23、26 行与 `world-model.{zh,en}.md` 第 58–59 行教 @ 收窄、@broadcast、回复 @ `[broadcast, 发送者]`（**静默**：文字不进编译） | `MentionKind::Broadcast` 删；@ 只叫醒；回复只 @ 原发送者 | `MENTION_KINDS` 去掉 `broadcast`；T15c 把这几处改成「@ 只决定叫醒谁；回复只 @ 原发送者（他还在群里时）」 |
| `agent/inject.rs` 测试里的 `MentionKind::Broadcast`、`render::MsgLine.directed_to` | 删 | 按 `MsgLine.mentions`（`Vec<String>`，读者自己显示成「你」）改；「@ 了别人的不进 / broadcast 放宽」改成「群里别人发的全进、标出 @ 了谁」 |
| `agent/testkit.rs:98` `Fixture::register`（**静默**：运行时 panic `session.title_or_brief_required`） | `sessions::register` 要标题（≤18 个码点）或 brief | `register` 传 title 或 brief |
| T15 `start_session` / `create_child_crew` | 同上 | 工具参数把 title / brief 传下去；拒绝时把 `Refusal::text(locale)` 回给 agent |
| T12 `post` | `Posted.answered_ask`；`directory::resolve`；`channels::ensure_dm`；`channels::dm_peer` | 回执写「已回答 H<n>」；`post.to` = `resolve` + `ensure_dm(tx, 我的机组, me, 对方)`；不给 channel / to / reply_to 报错（S64） |
| T13 `contact`、T15 `report_to_parent` / `message_child_crew` | C7 作废 | 不写（跨机组 = dm） |
| 任何 `crews::delete` / `crew.delete` / `check_deletable` | 改名 `purge` / `check_purgeable`；`crew.delete` → `crew.remove`（`crews::archive`），`crew.unarchive` → `crew.restore`；事件 `crew.removed` / `channel.removed` → `crew.purged` / `channel.purged` | 机长工具不提供移除 / 永久删除；读状态用 `crews::is_archived` |
| `collab::prompts::placeholders` | N1 没有新加同名函数，只是让 main 上已有的那个去重 | M3b 若另写过一份就删掉，调用点都指向 `prompts::placeholders` |
| 唤醒入队（**静默**） | `wakes::enqueue` 多了 `Enqueued::Skipped`（目标 session 的机组已移除：不写行、不发变更） | 对 `Enqueued` 的穷举 match 要处理 `Skipped`（编译会报）；把它当「无事发生」 |
| `collab/src/render.rs` 测试 `every_injected_line_carries_its_number`（M3b 约 :1161，`directed_to: directed` 在 :1167；**自动合并后编不过**） | `MsgLine.directed_to` 删，换成 `mentions: Vec<String>` | 改成 `mentions: directed.unwrap_or_default()`；`render.rs` 冲突段的取法：N1 的 @ 标记与 M3b 的 `#seq` 前缀**两边都要**，期望文本是 `"…, 人类） （回复 人类） #"`，只取任何一边都会让 `markers_are_separated_from_the_timestamp` 红 |
| `server/src/agent/port.rs` 测试 `narrowed_messages_are_hidden_from_other_readers`（:1410）、`filtered_messages_do_not_shorten_a_page`（:1490）（**静默**：编译照过，运行时红） | 测的是已删的收窄 | 删，或改写成「@ 只叫醒、不藏消息」 |
| `port.rs` 测试 `listening_clause_and_set_listen_gate`（:1929，:1946 的绕过；**静默**） | 绕过用的 `listens::set` 现在会拒 `listen.not_readable` | 那条旧的收听行改用裸 SQL 插入（模拟遗留数据） |
| 提示词文字（**静默**，进 T15c 那一行）：`world-model.{zh,en}.md` 第 65 行仍教收窄（与第 58–59 行同列）；「根机组人类群」的说法在 `world-model.{zh,en}.md` 第 32、41、42、120、126、133 行与 `tools.{zh,en}.md` 第 66、156 行；`tools.{zh,en}.md` 第 342 行「内建的总机组不能被收编」 | 人类群归 home（顶层机组）；总机组在树外 | 「根机组人类群」→ 「home（顶层机组）的人类群」；第 342 行 → 「管理机组在树外，不能被收编」；与第 58–59 行一并列入 T15c |
| 预计的合并冲突 | — | `collab/src/prompts.rs` 的 `REQUIRED`：留 M3b 的 `line` `["seq","at","sender","text"]` 与 N1 的 `line.at` / `line.at.you`，删 `line.directed` / `line.directed.unknown`；`render.rs` 测试段（约 1605 / 1614 行：N1 的 @ 标记 + M3b 的 `#seq` 前缀，两边都要）；`worldview.rs` 的 `WorldCtx`（见上）；最后重生成 `schema/crewd-api.schema.json` |

### 26.7 给 M3c 的合并注意（T10a 的唤醒侧跟进）

| 位置 | 变化 | 怎么改 |
|---|---|---|
| `crates/wake/src/plan.rs` 的 `Mention::Broadcast`、`visible_to` 与表驱动测试里的 broadcast 用例；`wake/src/breaker.rs:622` 与 `server/src/orchestra/testkit.rs:1579` 的 `Mention::Broadcast` | 收窄与 broadcast 删除 | 删 `visible_to`：候选 = 群成员 ∪ 收听者，只排除发送者；删 `Mention::Broadcast` 与这两处用例 |
| T10a 要**新加**的契约测试（原「C22 对拍」的说法作废：M3c 里 C22 指的是 `port.rs:339` 的 `ChannelCtx` 填法） | collab 一侧的 `visible_to` 已删 | 加一条：注入面（`cursors::unread_for`）与规划的候选在同一条消息上一致——都是成员（含收听）、都只排除自己 |
| `plan.rs` 人发、没 @ 的兜底链里的 `dm_peer` | dm 默认叫醒对方（§24.2） | session 在 dm 里发言 = 隐式 @ 对方：`ctx.dm_peer` 由 DbPort 用 `channels::dm_peer` 填；表驱动加一例；环路熔断兜底 |
| `server/src/orchestra/testkit.rs` MemPort 的未读镜像用 `visible_to` | 删 | 只排除自己发的（同 `cursors::unread_for`） |
| `render::build_batch` 的放行 | 按**成员资格**：`channels_of_session` ∪ 收听行（收听还要读得到且群没归档）；不在群里的排队行进 `unusable` | DbPort 不要另做一道「读得到」过滤；收听的 `until` 要靠 M3c 把 `now_ms` 传进来（26.5） |
| `orchestra` 里「审批有结果」的唤醒处理 | 审批删除 | 只剩 `orchestra/deliver.rs:758` 一行注释提到审批结果，删掉那几个字即可；v4 已把排队中的审批唤醒作废 |
| 待决策 / 熔断 / 投递放弃的「找人类群」 | 人类群归 home（顶层祖先） | DbPort 用 `crews::home_of` + `crewd_store::channels::human_id` |
| `Core::start_new` 注册 session | 标题必填或 brief | `StartRequest` 的 title / brief 原样传下去 |
| 已移除的机组 | 只读、不收新 session；移除时它的 session 的 **pending** 唤醒已作废，有 inflight 唤醒时移除被拒（`crew.wakes_inflight`）；`wakes::enqueue` 对它的 session 返回 `Enqueued::Skipped` | 起 / 恢复已移除机组里的 session 前查 `crews::is_archived`，拒并说明；唤醒准入与重启答复按 session 跳过（不让整批失败）；`claim` 撞上在 `due_for` 与 `claim` 之间被作废的行时这一批失败、下一拍再来 |
| `collab/src/hibernate.rs:73` `CollabError::Invalid(format!(…))` | 变体装 `Refusal` | 读不出休眠记录是存储侧的坏数据：`CollabError::Store(StoreError::Invalid(…))`（连同它的测试） |
| `settings.rs:58` 休眠期限不合法 | 同上 | 新码 `settings.bad_hibernate_deadline`（占位符 `["value"]`），两种语言的 md 各加一节 |
| `restart.rs`（预计 4 个冲突段） | N1 按 `home_of` 分组（`by_home`）；拒绝装码 | 留 N1 的 `by_home` / `home_of` 分组，再加 M3c 的休眠计数与 `notice.restart.hibernate`；`get` 找不到 → `Code::RestartNotFound`；`restart.rs:28` 的 `eprintln!` 改成英文或走 LOG_ONLY |
| `wakes.rs`（预计 2 个冲突段） | 同上；N1 加了 `count_inflight_for_crew` / `drop_for_crew` / `Enqueued::Skipped` | M3c 的 `apply` / `ack_delivered` / `ack_failed` 错误 → `WakeNotFound` / `WakeBadTransition(from, to)`；坏行 → `Store(Invalid)`；session 对不上 → 新码或 `Store(Invalid)`；N1 的三样都留；对 `Enqueued` 的穷举 match 处理 `Skipped` |
| N1 自己的测试 `collab/src/crews.rs:1840` 与 `:1879` 调 `wakes::mark_inflight` | M3c 把它换成 `claim`（**合并后编不过**） | 改成 `wakes::claim(tx, &[id]).map(\|_\| ())` |
| `hibernate.rs:73` 与 wakes「坏行 → `Store(Invalid)`」那几行 | collab 的文字扫描测试连 `Store(Invalid(format!(…)))` 里的中文也报 | 写英文，或进 `LOG_ONLY` 白名单（同 `restart.rs:28` 的做法） |
| `dispatch.rs` 里 M3c 加的方法若构造 `CollabError` 字符串 | 变体装 `Refusal` | 用码（`collab::refusal::Code`）；`dispatch::collab_err` 会带上 `data.refusal` |

## 27. M3b agent 面交付记录与遗留（2026-10-09）

> **代码快照基准 `b5bfd9b`，导入基准 `d641ebe576e28fc7afe1367033a44ae99e83db52`**。计划名 `2026-10-06-crewd-m3b-agent-surface`；本节写入者只在原生隔离 task branch 上做 T16，控制者另行集成，**本节不表示 M3b/M3c 已合并或真 Claude 行为已通过**。T16 代码提交 `98b524c`（确定性接线 / 配置 / surface）与 `b5bfd9b`（有界 opt-in smoke / eval）。T12–T15c 的功能是导入基准已有的交付；下面分清当前代码、确定性实测与 live 未验证边界。

### 27.1 当前已交付

- 线协议 `proto::agent`：attach / tools / call / hook 及 attach 连接专属 tools_changed；`HookKind::SessionStart`（线上 `session-start`），attach 返回角色、机组、label、md instructions、surfaceHash。共享 SchemaGenerator，契约已重生成，顶层 `$defs` 不重复。
- helper：`agent_client` 分发 response / events，`Response::into_result()`；`mcp_cmd` 每次现问 serve、不缓存；`hook_cmd` 总期限 5s、静默 exit 0。坏 token / 未 attach 拒绝；命令行只有数据目录、session、token 文件。
- T12 消息：`post` 必须选 channel / to / reply_to，地址经目录解析、跨机组走 dm；读 / 搜索覆盖真实可读范围，分页 / 已读 / 回复及回执；`list_channels` 合并旧 human_watching、home 人类群优先；无默认大厅目标。
- T13 群：转让 / 接受 / 强制合并 channel_owner，收听 / 取消合并 channel_listen(off)；dm 只由 post(to) 打开；陌生 @ 不藏消息，行内带目标标注。队友 progress / ack 只折进摘要，自己的不读。
- T14 人类决定 / Todo / 额度 / timer / profile：ask 非阻塞，H 号 / resume_note / 可撤回默认方案按 collab；withdraw_ask、respond_todo（只回复 Agent 那本，不改状态）、get_quota、schedule_wakeup、set_session_profile。
- T15a/b 机长：现角色权限 + acts_as_captain_of；管理机长对所有机组按机长算，其他管理 session 仍是 worker 表。handoff_captain、delegation_policy、crew_structure 合并旧入口；不提供排序 / 移除 / 永久删除工具。start_session 的 isolation 必填 bool 原样传 StartRequest；M3c 创建 worktree 尚未在本分支联验。
- T15b annotations：所有工具 openWorldHint=false；RO / destructive / idempotent 来自 catalog；被动 = RO ∪ talk(post, channel_listen)，用同一份目录判产出，不再把所有 crew 工具一概当被动。
- T15c 世界观：固定机组模型 / 行为习惯与实时事实分层；无审批 / 单根 / 可见性收窄 / broadcast 老机制。Stop 未叫到人的问句以 **该 session** 发 question，非 System，按 prompt 去重。
- T16 SessionStart：生成 matcher `resume|compact|clear|fork`，startup 排除。真配置 → shell helper → agent.hook → port 现读 `context_for` → `render_live`；输出 hookEventName 正确。live 文本必要字段预算（单值 256，角色 / 机长 / 额度块各 1,000，谱系 4,000），最终不超 10,000 字符；launch 完整世界观不按这个预算裁。
- T16 launch：crew alwaysLoad=true；产品生成 permissions.deny=[AskUserQuestion]，不禁用 ToolSearch，不加 PreToolUse。官方支持的 spinnerTipsEnabled=false、prefersReducedMotion=true、feedbackSurveyRate=0、autoUpdatesChannel=stable 与两项 disable-survey/title env；SSH_AUTH_SOCK、HTTP_PROXY、HTTPS_PROXY、NO_PROXY 及小写透传。stable 是更新渠道，**不是锁版**，Homebrew 忽略这个 setting。
- T16 surface：tools md 的 instructions（两行）经 attach 进入 rmcp InitializeResult.with_instructions；catalog 名字 / schemas / annotations / locale 描述与 instructions 生成 worker/captain 两张表的 golden，`surface.{zh,en}.json` 漂移即红，审查后 CREWD_BLESS=1 才更新。FNV-1a/64 指纹追加 serverInfo.version、attach 记 stderr；非安全哈希。r.unknown_tool 的 hint 来自 REMOVED map，旧名字说下一步调哪个。
- T16 验证工具：`agent_e2e` 直接读生成的 mcp.json/settings.json 执行真 helper（不另拼 argv），9 项确定性测试；Fixture 只在 test/testkit 构建记已完成的 hook 名称、工具名称和 context 字数。live stdout 的结构化完整 tool-use/result 顺序另行观测，绝不读用户 transcripts。

### 27.2 工具目录：历史基线 → 本快照

补充清单的历史基线 `82918d4`：40 个（任何角色 21、机长额外 19）；本快照由两种语言 golden 与真 helper 验证：28 个（任何角色 17、机长额外 11）。总机组没有 catalog 特例；admin 与 worker 同表。下表当前名称来自本快照的同一次 golden 读取，RO / D / I 分别是 readOnly / destructive / idempotent；全部 openWorld=false。

| 当前工具 | 谁 | RO/D/I | 历史来源 / 改动 |
|---|---|---|---|
| post | any | –/–/– | 增 to/address→dm；无默认目标；contact / report_to_parent / message_child_crew → post(to) |
| read_channel | any | ✓/–/✓ | 同名，页与已读语义 |
| search | any | ✓/–/✓ | 同名，readable ∪ valid listens |
| list_channels | any | ✓/–/✓ | list_channels + human_watching |
| channel_create | any | –/–/– | 只建 topic；dm → post(to) |
| channel_invite | any | –/–/– | collab 决定权限 |
| channel_leave | any | –/–/– | 同名 |
| channel_archive | any | –/✓/– | collab 群主 / 机长 |
| channel_owner | any | –/–/– | channel_transfer + channel_accept |
| channel_listen | any | –/–/✓ | channel_listen + channel_unlisten(off) |
| directory | any | ✓/–/✓ | 目录统一地址，contact 删除 |
| ask | any | –/–/– | H 项，非阻塞；管理机组也有 |
| withdraw_ask | any | –/–/– | 同名；目标 crew 可指定 |
| respond_todo | any | –/–/– | 管理机组也有；只追加 Agent 回复 |
| get_quota | any | ✓/–/✓ | 同名 |
| schedule_wakeup | any | –/–/– | 同名 |
| set_session_profile | any | –/–/✓ | 同名，下一次启动 |
| rename_crew | captain | –/–/✓ | 同名 |
| change_workdir | captain | –/✓/✓ | preview→confirm；不搬项目权限 / 记忆 |
| set_crew_status | captain | –/–/✓ | 同名 |
| delegation_policy | captain | –/–/✓ | read_delegation_policy + update_delegation_policy |
| start_session | captain | –/–/– | isolation 必填 bool，title / brief |
| stop_session | captain | –/✓/– | 先发 reason 再停 |
| inspect_session | captain | ✓/–/✓ | 真状态与屏幕 |
| nudge_session | captain | –/–/– | esc / interrupt；不是替人选项 |
| list_sessions | captain | ✓/–/✓ | 同名，可指定 crew |
| handoff_captain | captain | –/✓/– | handoff_captain_to_session + create_and_handoff_captain |
| crew_structure | captain | –/✓/– | create_child / create_parent / adopt / adopt_parent / release 五入口合并 |

失效名字及替代调用唯一维护在 `catalog::REMOVED`；不会在这节再维护第二份完整删除名单。净减少 12 个，主动弃掉的是 system 署名的跨机组发言和 agent 直接 channel_create(kind=dm)，其余是合并入口而非丢能力。

### 27.3 原计划与 spec 的出入（1–9 原文；后续裁定紧接说明）

1. `CommsSpec` 比 §22.5 多一个字段 `approval_gates`（裁定 3）。
2. `listen(channel?, minutes?, senders?, off?)` 换成 `channel_listen` / `channel_unlisten`（§22.7 Q2）；新增 `channel_transfer` / `channel_accept`；归档权限 = 群主或机长。
3. 回合记账（这一轮叫没叫过人、上一轮的 prompt_id）存在 collab 的 `meta` 表（键 `agent.turn.<session>`），不加迁移；M3a 冻结的 `turn_trace::on_turn_end(tx, …)` 在 collab 里实现落库（代发 + `awaiting_question`），M3c 的 codex `TurnEnded` 直接调它；新增 `turn_trace::note_post(tx, session, mentions)` 由 M3b 的工具在发消息后调。
4. agent RPC 必须按连接记 attach 状态，所以 M3b 要改 `server/src/conn.rs`（§22.5 的文件归属没列它；M3c 不碰 conn.rs，不冲突）。
5. 可见性（`CrewWhiteboardVisibility.swift:48`）与「发给 X 的」标注用 M3a 的 `cursors::unread_for` / `render::blocks_for`（§22.3 写的「M3b 移植可见性」由 M3a 做掉了，M3b 不另写一份）；M3b 只加注入的总字数上限与「省略了几条」的自报。M3c 给 codex 组 `additionalContext` 时调 `server::agent::inject::prepare / commit`。
6. 审批照 Swift #75②**不阻塞**（`McpPermissionHook.swift:69-76`：当场拒 + 开待确认 + 一次性放行票；控制者裁定 C2，spec §22.3 已改）。放行票、去重、公告都在 M3a 的 `approvals::gate` 一个事务里，M3b 只把结果翻成 hook 输出。
7. `post` 暂不收 `attachments`：M3a 的 `message.post` 附件非空一律拒（M3a 计划裁定 14，附件上传不在 M3）。Swift 一直有这个参数，等 M3a 之后支持附件再加回（遗留 L8）。
8. 机长注入面上的附加物（命名提醒、拆组信号、计划陈旧度、全机组织树、派活策略更新，`HookEmitter.swift:246-311`）不移植：计划板已删（§9），派活策略改由 `read_delegation_policy` 现读；组织树由世界观的 lineage 节 + `directory` 取代（遗留 L1）。
9. 世界观去掉 Swift 的 `humanRoster` / `sharesBlock` / `tiebreakerBlock` / `captainRunnerBlock` 与入门机组的 SKILL.md 提示（`LocalSessionWorldModel.swift:56-70`）；人类只有一个人，runner 偏好设置 crewd 里还没有（遗留 L2）。

**这些是计划历史文本，不是当前接口承诺**：1 / 6 已由 §24.1 B16 全部作废（无审批、无 approval_gates）；2 / 8 的独立工具名按 T13/T15b 合并（见本节 catalog）；5 的收窄 / broadcast 已由 N1 删除，成员消息全见。原计划后续 10–13：产出如今按 catalog passive 判；isolation 仍必填、M3c 创建未联验；新机组缺省标题来自 md 而非地名池；seq 引用已用 M3a.1 get_by_seq，不再逐群扫描。

### 27.4 M3a 接口核对、缺口及跨计划吸收

Task 1 原始对齐记录与当前实现一致的差异：safe_name 已是 pub，无需另包；M3a.1 已有 get_by_seq / display_label / sender_label / Db::shutdown（**没有另一个 Db::join**）；Role 是封闭 Captain/Worker/Admin（缺省 Worker）；agent schema 的真签名是 `schema(&mut SchemaGenerator) -> Value`，不是无参 schema。

| 缺口代号 | 原来的供给与最终落点 | T16 边界 |
|---|---|---|
| G3 Todo 回复 | M3a todos::respond 已提供；M3b asks::respond_todo 通过 port 追加 Agent 回应 | 没补表 / SQL；已纳入全量测试 |
| G-F fill 再展开 | M3a prompts::fill 一遍扫描已提供；M3b 保留值内占位符不再展开的测试 | 没复制 fill |
| G-S StartRequest | M3a control.rs 已有 executor / isolation；T15a 保留必填 bool 并传递 | 真 worktree 创建归 M3c，当前只有 FakeSessionControl / request 验证 |
| G-T TurnEnd | M3a 提供五态与 stub；M3b turn_trace 实现记账 / 问句 / 代发，T15c 改 session sender | T16 用真 Stop helper 验一次 / 去重 / category；codex 集成未验 |
| G9 AppState.agent | M3a 已有 field + mod/runtime 占位，M3b 换实现 | T16 未改 AppState 结构 |
| G11 夹具 / conn | M3a 已有 FakeSessionControl、conn::run、dispatch::handle 与 testkit 入口；M3b 实现 Fixture/socket | T16 crewd dev feature 开 testkit，实际 helper 连接它；不是真执行器 |

§23 末给 M3b 的三点已吸收：helper 用 Response::into_result；proto agent schema 共用同一生成器 / 顶层 defs；port 的 blocks_for / build_batch 传 Locale。safe_name / sender_label / display_label 都复用 collab，不自造。默认方案定时执行、token revoke、shutdown 次序与真正启动 / 停止仍由 M3c 接；本分支不代表这些动作已联验。

### 27.5 §26.6 合 main 注意的七项纠正

历史 main/N1 合并报告（基准 `4c06b0e`，整合后 `82918d4`）记录的七项，在导入基准已有对应实现；这里纠正 §26.6 那张预告表，**T16 没再合旧 worktree 或 main**。

1. `From<CollabError>` 不知道 locale：NotFound 保持 NotFound，其他规则拒绝装 Reason::Rule，到 tools::Ctx::fail 才按 agent locale 渲染；不是在那里直接 r.text(locale)。
2. 裸 SQL 造旧收听行不够：必须翻转「旧行授予可读」的断言；listens::channels_of 会过滤不再可读的群，旧收听不能扩权。
3. B15 规则②改变同群成员的机组同事 / 跨机组 dm 的另一机组同事的可读预期；单看旧门会编译过却语义错。
4. 地址 session 参数走 directory::resolve；**crew** 号码不能强行走一个依赖现任机长存在的 session resolver，按相同 created_at/id 顺序解析 crew，已移除回 directory.crew_removed。
5. 总机组 ask 恢复会翻转 catalog、直接调用拒绝测试和世界观工具字断言；T15b 进一步去掉全部管理 catalog 特例，不保留 not_on_management。
6. 顶层人类群不是旧 root，world-model 的 role/lineage 测试也要改；管理机组是树外，必须有自己的 lineage，而不只是改一句名称。
7. render.rs 的 MsgLine.directed_to→mentions 合并断点在合并树原报告为 1076，不是预告的约 1167。**行号是历史读数，今后 grep 当前树**；N1 @ 标记与 M3b #seq 都保留。

### 27.6 T16 确定性与变异实测

macOS、本分支 `b5bfd9b`，默认 cargo runner（未跑 nextest / Linux / 真机）：

- `agent_e2e`：9 passed，真生成配置 / socket / helper；四种 SessionStart source、startup 与近似值排除、超长值≤10k，角色升降即时 list_changed / 17↔28 / 调用权限，post 注入 / 他人 @ 标记 / progress 摘要 / 一次性游标，PostToolUseFailure wire name、Stop sender/category/去重、token 与未 attach、removed 提示。
- 首个实现 gate：1378 passed / 0 failed / 6 ignored。live harness 的两个纯结构化 trace 测试分别进入 smoke 与 eval test binary 后，gate 为 1382 passed / 0 failed / 8 ignored。
- 保存提交后 pre-live **三次 check.sh**：各自 1382 passed / 0 failed / 8 ignored，fmt / clippy -D warnings 均通过；每次保留完整独立日志。更早全量 RED：旧 scripted-serve attach fixture 缺 instructions/surfaceHash，导致 helper_cli 两项测试失败；随后 RPC exact-JSON 期望没加这两项；修正 fixture/期望后全绿。这些不是「重跑吞掉的 flaky」。
- 提交后 cp backup/restore 做 **10 次有效变异**：SessionStart RPC context、fork matcher、alwaysLoad、AskUserQuestion deny、initialize instructions、wire annotations、constant hash、golden content、removed hint、fixture log 接线。8 次各执行 9 个 agent_e2e，2 次各执行 2 个 surface 测试，合计 76 次测试执行；每次预期用例真红，编译失败 / 执行数不符不算有效。还原后三个完整 gates 全绿。

rmcp 3.5.1 的实际本地源码已核：InitializeResult.with_instructions(impl Into<String>)，enable_tools + enable_tool_list_changed 设置 tools.listChanged，ToolListChangedNotificationMethod 的 wire 是 notifications/tools/list_changed，Peer<RoleServer>::notify_tool_list_changed 存在。不是按 API connector 的字段猜 MCP 配置。T8 原始 lock diff（`ab3f469^..ab3f469`）新增 **10** 条 package：equivalent、futures、futures-executor、futures-macro、futures-sink、indexmap、pastey、rmcp、tokio-util、tracing-attributes；T16 只增加本仓库 dev dependency/feature 关联，没引入新第三方 crate。

### 27.7 授权 live smoke 与三 seed 结果（不涂绿）

2026-10-09，仅运行一次：`CREWD_LIVE_CLAUDE=1 cargo +1.94.0 test -p crewd --test live_agent -- --ignored --nocapture --exact real_claude_uses_crewd_mcp_and_receives_injection`（在 own worktree，manifest/target 都指向本分支）。测试内部读产品生成配置，print-mode stream-json stdin/stdout；使用真实 build_env，restricted + strict-mcp-config，只提供 Bash / ToolSearch / AskUserQuestion 内置候选（最后一项仍由产品 deny 移除），正常 CLI auth，不读凭据或用户 transcripts。

**结果：0 passed / 1 failed / 2 filtered out，exit 101。** native Claude **2.1.295**；init model `claude-opus-5-5`，permissionMode auto。四个 test-owned stdout 事件里 crew connected，17 个 worker 工具 upfront；其他工具恰好 Bash / ToolSearch，AskUserQuestion 不在 init。随后 result **is_error=true**、terminal_reason=api_error，原文 `Not logged in · Please run /login`。duration_ms=340、duration_api_ms=0、input/cache/output tokens 全 0、total_cost_usd=0；测试自身观测 CLI wall_ms=1689（全测试含 fixture 1.86s）。不把 subtype=success 当成功。

没有任何 tool_use，hooks 0。**不存在 first crew call，因此 ToolSearch 顺序不可验证**；MCP 握手和 upfront 列表是已观测到的，但 post / secret 返回 / started-failure hook / live role-refresh 未观测。第二个 smoke prompt 未发送，角色未改变，Bash false 没启动；零失败 hook 不是通过。停止后没有登录绕路、paid retry、额外 prompt、真实 Codex 或终端 fixture。

每 prompt 8 turns / 240s，预算每 CLI process 客户端估算 2 美元（不是实际余额 / 硬 token 上限）；remaining quota 未量。不因账号失败改 launch 的 auth 策略，不用全局 ToolSearch 禁用把结果改绿。

| seed | 实际状态 | posted / right channel / message count / ask / injected chars |
|---|---|---|
| @ 问题 → 一句 ack 再回答 | NOT RUN：smoke 登录错误后停止 | 全部未量 |
| 需要人决定 → ask、不在大厅 @human | NOT RUN：同上 | 全部未量 |
| 完成 → 一个 done、≤2 posts | NOT RUN：同上 | 全部未量 |

三个 seed 的 named ignored test 已实现且普通检查只编译 / 跑纯 parser 控制样本；**未发送三个付费 prompts、没有行为分数**。live 范围已用完的 smoke 不自动再跑；后续验证需新的明确授权与正常账号环境。

官方文档 2026-10-09 核对：[settings-reference](https://code.claude.com/docs/en/settings-reference)、[env-vars](https://code.claude.com/docs/en/env-vars)、[hooks](https://code.claude.com/docs/en/hooks)、[mcp](https://code.claude.com/docs/en/mcp)、[prompt-caching](https://code.claude.com/docs/en/prompt-caching)、[CLI](https://code.claude.com/docs/en/cli-reference)、[headless](https://code.claude.com/docs/en/headless)、[SDK message types](https://code.claude.com/docs/en/agent-sdk/typescript)。文档支持≠runtime 合规；ToolSearch 的 hook 覆盖没有被文档建立，故必须用独立完整结构化流，不能用未证明 hook 的零计数。

### 27.8 遗留、集成 hold 与验证边界

- **CROSS-BRANCH HOLD（M3i-a）**：M3b/M3c 合并后，Stop 的自动 Sender::Session question **不得调用 M3c breaker.on_post**。T16 不修、不宣称集成已过；M3i-a 要用合并树做专门回归。
- **CROSS-BRANCH NOTE**：T13 progress/ack digest folding；T15c stop sender/session trace；T16 SessionStart HookKind、launch pins、env.rs 的 transport 白名单。M3c T11 AgentPaths 的共享 env.rs 修改须保留双方，不挑边合；schema 后合者重生成。
- S44 hook 注入 / wake 双发与 receipt→wake::plan 归 M3i-a；当前 E2E 的「至多一次」只覆盖 hook 游标，不是跨执行器唤醒集成。
- N2 在这条 SessionStart 上扩 UserPromptSubmit / StopFailure / Notification / session id 信号，不另立第二套 hook；AgentPaths 每次 launch 重解析归 M3c T11；statusLine 稍后。
- L1 机长附加注入面（命名 / 拆组 / 陈旧度 / 全树 / 派活更新）仍未搬；计划板已删，其他由 M4b/M5 行政简报设计决定。
- L2 runner 偏好 / 能力块 / 入门 SKILL.md 未搬；L3 change_workdir 不搬 Claude 项目记忆与权限（回执说明）；L4 仍用 shell 引号，不冒用未锁 CLI 版本的 hook args exec；L6 isolation bool 保留，只有 StartRequest 路由验证，M3c 真 worktree 创建 / 真 session 生命周期未联验。
- L5 Codex token 形状已有原有 M2c 路；此任务没跑真实 Codex，M3c 必须接 runtime_dir、inject prepare/commit、turn_end 与进展信号；不恢复应用审批。
- L7 meta 回合键删除 session 时仍留软状态；L8 post 附件未接；L9 侧栏人手拖拽，不自动排序（Q4），无排序工具；L10 新机组默认名来自 md，不是 Swift 地名池。
- `context_for` 的 quota_claude / quota_codex 仍初始化为 None（代码明确把启动时档位填充交给 M3c），本任务的 SessionStart DB refresh 输出未知档位，不是实测额度。M3c 需明确 live 档位数据源 / 填充并联验，避免把 launch 里已知档位覆盖成未知；这里不把渲染了 quota 块称为「真实档位刷新通过」。 **本项是 b5bfd9b 的缺口记录；fix-r1 的 typed 接线/联合契约已在 §27.9 闭环，实际 provider feeding / 生产 launch 仍归 M3c，未声称真实额度通过。**
- T10d idle terminal 样式 / placeholder / cursor **没有新 terminal fixture 授权，未验证**。不能用本次 print-mode MCP init 代替它。
- eval 扩展与文案版本比较归 M3c/M5b。生产 serve 编排、真实应用数据、Linux、本机已运行 app、iOS/iPad 真机、codex、paid 模型行为和账号配置均不在这趟已验证范围。

### 27.9 T16 fix-r1：进程停止、预检期限与 live quota 联合契约

> **代码基准 `1ff705b`**；`68a25fa` 修 harness，`1ff705b` 修 typed quota。起点 `3657dbb`，这节补充而不改写上面 `b5bfd9b` 的历史实测。没有新 live 权限：本轮未启动任何真实 Claude/Codex，连 --version 也没跑；原 auth BLOCKED、三个 seed NOT RUN、real TUI/live 未验证及 relay/breaker HOLD 永久保留这次读数。

- **F1**：原 finish 在错误后仅关 stdin、等 15s，不会取消当前回合；独立合成探针确实写出迟到 sentinel。修成 prompt 失败即 `Child::start_kill`、不可逆 stop、禁止新 prompt；finish 只在成功终态走 EOF/15s 优雅退出，其他路径立即取消并最多 2s 等收尸。收尸失败诊断，kill_on_drop/Tokio 后台收尸是最终 fallback，无 name/pattern kill、无无界 wait。测试刻意把 finish 推迟 400ms，确认 prompt Err 本身已停止 Child，且 300ms 延迟工作不发生；成功对照必须走 EOF 并完成收尾。
- **F2**：--version 从无 deadline 的 output().await 改为独立 5s 的输出+退出期限，stdout 4KiB 上限；失败/超时取消 exact Child、2s 有界收尸。240s prompt 期限同时包住 stdin 写入和输出读取，背压不能绕过它。这里只声明这些异步等待边界，不把 CLI 的预算参数说成整个启动/文件 I/O 的绝对 wall-time 保证。
- **quota**：`port::world_context` 是 launch 与 SessionStart 的同一入口，DB-only `context_for` 不承担 live 数据。复用已有 `SessionControl::quota -> QuotaReport/QuotaLine`，按 executor 取非空 plan；最多 1s 查询。WorldCtx 用 `QuotaTier::{Known,Unknown,Unavailable,Failed}`，成功但没档位=Unknown，Unsupported=Unavailable，其他错误/超时=Failed；后两项在 world-model zh/en md 独立显示，不回传原始错误、不编余额、不令整个身份/角色刷新静默消失。limited_until/detail 仍是 get_quota 的原有数据，不当作档位或余额。
- **联合 launch 合同**：已提交的 opt-in harness 启动方消费 world_context；确定性 E2E 读取生成的完整 launch 世界观，FakeControl 的已知档位更新后，四个 SessionStart source 都返回新的已知值，两种 locale 均验。M3c 生产启动方必须复用该入口，不能再自行从 context_for 留空；实际 provider feeding / 生产编排未联验，没有查询真实余额。
- **验收**：原 harness 新 10 项合成测试 RED 2 passed / 8 failed，再 GREEN 10/10；旧 9 项 agent_e2e 加两项 quota 用例 RED 9/2，再 GREEN 11/11。harness 单元完整 check.sh 1402/0/8；quota 单元完整 check.sh 1404/0/8（默认 cargo、macOS、fmt/clippy clean）。提交后四个有效变异：取消信号、预检期限各执行 10 项；SessionStart 绕开 typed 源、Unavailable 压 Unknown 各执行 11 项，合计 42 次实际执行，预期用例都红，复制备份还原。不是新的一轮付费 smoke。
- **非阻塞审查项**：no_search helper 现在也拒绝失败终态，和注释的成功终态证明对齐；此前 smoke 自己已检查 prompt success，不将这项加强冒称为已证实的 smoke 假绿。initialize E2E 直接比 md instructions / surface hash，不只检查非空和加号。settings/hooks/annotations/schema/goldens 保持原契约，quota 文案仅扩世界观四态。

## 28. M3c 交付记录与遗留边界（2026-10-10，本地组合树）

本节承接 §27，记录已接受的小单元及当前组合证据；T12 文档与 whole-M3c 尚未接受，不代表整个产品已交付。已接受 T11/T11b 组合锚点为 `648124c08a717376069e2925de1c2361768f204c`；后续 ack 五文件单元 `7b72a156e604f3e1bc15fbc403d3f5d92b07d939` 已由另一独立上下文完成 Spec Compliance PASS / Code Quality PASS，仅接受该修补范围。

以下 S/C 均以仓库根为基准：S = `.claude/worktrees/crewd-m3c/.superpowers/sdd/2026-10-06-crewd-m3c-orchestration`；C = `.claude/worktrees/agent-a624116fb76233769/.superpowers/sdd/2026-10-10-codex-continuation`。这些是私有留存证据根，不是已发布公共工件；通配标签指对应日志/JSON，完整索引见 C/t12-evidence-inventory.md。缺失历史元数据明确记为 MISSING，不由摘要补造。

协调者在本次 T12 文档编辑前只读核对：HEAD 为上述完整 7b72a156，父提交为上述完整 648124c0，MERGE_HEAD 空，工作树与 index 干净。此身份记录归协调者实测；本文作者仅核源码/留存日志，本轮没有重跑测试。ack 独立最终报告为 C/C/flake-write-completion-review-prep.md；其中旧 PENDING 检查点已被末尾 Final scoped closure 取代，复审者执行测试数为 0，证据类型是 SOURCE 检查与独立留存日志核验。

### 28.1 模块与实现边界

| Module | Implemented behavior and evidence boundary |
|---|---|
| wake::plan | Members plus active listeners minus sender; mentions decide wake targets, not visibility; dm wakes peer, human dm bypasses Arbiter. |
| wake::breaker | Edge/crew no-progress loop latches persisted before effect; only human recovery; causal multi-party hop count deferred. |
| wake::queue | Highest-tier batches, claimed riders, per-channel continuous unread window and actual cursor_targets; deferred rows remain pending. |
| wake::policy | Tier, stagger, restart hold, optional concurrency and human bypass; no default max-live restriction. |
| wake::notice | Localized safe-name restart/hibernate/failure/recovery text follows actual lifecycle. |
| wake::explain | Shared why_not_woken/live_blocker used by view and admission. |
| proto::wake_lifecycle | Shared collab/MemPort transition table; claim counts attempts, Requeue refunds, boot rescue retains failed attempt. |
| orchestra::Core | Single-actor inputs, launch/delivery ownership, deadlines and FIFO; durable human-stop cancellation; bounded direct-Core simulation. |
| orchestra::deliver | Atomic rows/riders/cursors/origins ack; deferred retained; no queue-as-Delivered claim. |
| orchestra::boot | Boot-only inflight rescue, watermark before rescan, human restart choice, staggered per-session resume. |
| orchestra::hibernate | Stop new work, safe points, deadline interrupt, bounded exit, requeue, record and resume note; not OS checkpointing. |
| orchestra::notify | Transition notices in home human channel, active decisions, continuous-draft timing, historical quota-episode membership; in-memory notifier. |
| orchestra::db | Real collab transactions and dual-Port contracts; production DbLedger uses same Db FIFO; quota caller releases DB before one-second shared-source timeout. |
| orchestra::control | Production SessionControl and removed-crew refusals; accepted API call does not prove provider consumption. |
| orchestra::rpc | Stable refusals, numbered internal failures, loop/settings/session/decision/receipt/hibernate methods. |
| exec answer/draft | Per-key fingerprint and selected CAS; raw provenance and Unknown fail-closed; fresh full PTY drains. Typed positive write progress is not completion; body must fully write before submit, each beat's full actual completion starts the next gap, final CR actual completion precedes fresh full drain + authoritative Empty + baseline change. |
| term selected/input_draft | Actual highlighted zero-based option; Empty/Text/Unknown parsing; real grey typed draft remains unverified. |
| serve / CLI | Library without executor remains NoSessions; CLI defaults Claude; resolve AgentPaths each launch, current_exe helper identity, external runtime, ordered actor/connection/Ticker/DB shutdown. |

### 28.2 控制器裁定 C1–C28

C numbers in the following table are controller-rulings identifiers. Old sync C3(note_progress), old sync C8(worktree never deleted) and old message-model C7(contact syntactic sugar) are distinct namespaces.

| 裁定 | 记录必须讲清 |
|---|---|
| C1 | 库 serve() 无 executor 不开 orchestra；CLI serve 传 Some |
| C2 | Spawned 前/投递前死亡失败次数有界、backoff/give-up |
| C3 | boot 自身 rescan 前记录 restart watermark |
| C4 | shutdown gate 独立 flag，不被 shutdown_done 消耗 |
| C5 | due 合批测试只改与 R6 实际 delivered max cursor 相冲突的断言 |
| C6 | on_restart_answered 在 T8 定义，后续不得重定义 |
| C7 | Rig cfg 保留 stand-in，只有明确 temp repo 测试用 Git |
| C8 | 外部进程测试置 integration binary，保护 lib 锁测试 |
| C9 | restart E2E crash window 固定 w1 idle；记录最终 kill9 offline fixture 演进 |
| C10 | settings 参数验证/null_ok；最终改名 settings.get |
| C11 | health 1:1；Db shutdown/join 已修，不列旧缺口 |
| C12 | 插入 notice/note 名称走 safe_name/标准 label |
| C13 | resume refused、progress 规则及后续 refinement；跨分支真实注入等留 M3i |
| C14 | internal 编号/stderr，客户端稳定码 |
| C15 | 被其他 guard 掩盖 mutation 要替换或诚实报 uncaught |
| C16 | 机械 compile/lint 修正不变语义；Result alias 后续裁定保留实际形状 |
| C17 | DbPort tests 固定 synthetic ids、经 store seed |
| C18 | desired=running 仅在 Spawned 后写 |
| C19 | M3b form-B、shared union、每种优先级 give-up；当前双父整合事实 |
| C20 | 所有名字/外部自由文本插槽 neutralise，双语文案 |
| C21 | latch id 只经 Scope、U+2192；Option origin 显式转换 |
| C22 | ctx 完整填充；旧 visible_to/narrowing 契约已被 T10a membership/injection 对拍替代 |
| C23 | 历史 Pending→Retry 桥；被 C24 carried Inflight supersede，不能当现行规则 |
| C24 | 共用状态表、claim计attempt、每批原子ack rows/cursors/origins、not_before真值、seedjitter、persist latch |
| C25 | boot救回计attempt、panic loud exit、hook超时、why_not_woken、2sarbiter、sim；hopcount后续 |
| C26 | hibernate deadline/safe-point/interrupt/requeue/record/boot人问/note；不是OScheckpoint |
| C27 | PendingCrew无审批；runner权限请求交人，不自动替人答 |
| C28 | T10a→b→d→c→T11/T11b→flakes→T12；dm/版本/receipt/Unknown/humanchannel/hibernate0/systemtopic/改名/locale/splitmix64 |


C22 old narrowing cross-check is superseded by membership/injection contracts. C23 Pending-to-Retry bridge is superseded by C24 claimed Inflight riders. C19 form-A waiting is historical: combined source uses M3b shared world_context/render_live and method/topic union. C27 no product approval does not authorize bypassing runner permission refusal. C28 adopted answers remain binding without restarting design approval.

### 28.3 单元 → 完整提交 → 证据

Full commit objects are repository identities. A source report that omits precise historic HEAD/command/exit is not silently upgraded to exact execution metadata. Each supplement headline is indexed below, followed by its implementation commits. A report tag names its retained original evidence; C/t12-evidence-inventory.md preserves missing metadata and historical failure details.

| 单元 | 必须记录的 headlines |
|---|---|
| T9a | restart/timer/halt actor；C25；settle-before-sideeffects/boot-only恢复边界、MemPort映射、bounded prune、worktree冷启动flake修补、N4join |
| T10a | 2.1 membership/dm/listener planner；2.2 DbPort每批Tx映射；2.3 Hub bridge/Lagged；2.4 SessionControl/removedcrew；2.5 lang/2sArbiter/9bfolds；2.6 collab预算/latches；2.7 双Port契约；2.8提交粒度仅历史实施说明 |
| T10b | 2.1方法表；2.2共享explain；2.3hello/build/compat/Unknown；2.4错误码；2.5settings/hibernate；2.6receipts/eventids；2.7dispatch；2.8历史提交粒度 |
| T10d | 2.1selected/draft；2.2exec每键CAS/验证/输入guard；2.3answerplan；2.4actorFIFO/refusal；后续R1–R4优先于初始DIM/原agentkeys设计 |
| T10c | 2.1human决策；2.2raise/clear human notices；2.3nudge仅interrupt/安全文字/终局receipt；2.4双语safe_name；2.5refusal；2.6M3d结构化answer_choice合同；typedWorktree和draft计时foldins |
| T11 | 2.1Options/AgentPaths/helperidentity/runtime；2.2有序serve/drain；2.3CLI；2.4inprocfake；2.5actor/DB生命周期；2.6binarykill9/hibernate/SIGTERM三链；2.7遗留边界；T11新增DbLedger FIFO/永久reentryguard与quota caller |
| T11b | 2.1直接Core seededwalk/crash；2.2十条不变量；2.3真实具名mutation；121b→8c两项repair以及组合证据 |


| Unit / requirement | Full commit | Retained evidence |
|---|---|---|
| T8b: crewd-proto: wake_lifecycle, the one wake transition table (C24): states x events, attempts counted at claim and returned by requeue | `f9a0e14ff71a3ea63959ef63e1c28307ac776d78` | S/task-8b-report.md |
| T8b: crewd-collab: wake state changes go through the shared table (C24): claim counts attempts, ack_delivered settles rows + riders + cursors in the caller's Tx, ack_failed retries or gives up by the stored count, all-or-nothing | `0ccfa4d8d82e67d65ba1c0c006bd96ea6fd83d2b` | S/task-8b-report.md |
| T8b: crewd-server: orchestra switches to the shared wake table (C24): riders are claimed with their batch, attempts come from the store, each batch ends in one atomic Port::ack; a Delivered receipt during a stop still settles Delivered | `b4bd798b0a8cfd6a21a0d90e6401919b11e7b50a` | S/task-8b-report.md |
| T8b: crewd-server: delete the in-memory backoff map; the row's not_before is the only retry time (C24 P2 / R10) | `b4efc1df6129670c06576edacd7b5377dfbdff2d` | S/task-8b-report.md |
| T8b: crewd-wake + crewd-server: retry jitter (+-20%) from a seeded splitmix, one draw per failed batch; tests run without jitter unless they pass a seed (C24 P2 / R11) | `5af5f40ac8e05b9ef4a2143904f96f48c944a47a` | S/task-8b-report.md |
| T8b: crewd-wake + crewd-server: a loop latch is persisted before it takes effect; judge only reports a trip, the caller latches after put_latch succeeds (C24 P2 / R12) | `2c3c19fc066a0f5c81fd0465f282aa5678cca06f` | S/task-8b-report.md |
| T9a: crewd-server: worktree deadline tests warm the fake git and use a 2 s deadline (cold exec under load raced 300 ms) | `72bf67d66a8f0e704c17557fac831e6917b10492` | S/task-9a-report.md including fix1 |
| T9a: crewd-collab + crewd-server: failed acks check the event actually applied; MemPort refuses another session's rows; state round-trip and not_before cross-check (8b review 2-4) | `9c0c546997f34197945edd3cca7e4f5d96890c8c` | S/task-9a-report.md including fix1 |
| T9a: crewd-wake + crewd-collab + crewd-server: the boot reset of inflight wakes counts as an attempt; at the limit it gives up (C25) | `ca40866b756c3334e90b84b5e90d6c6cac69f64c` | S/task-9a-report.md including fix1 |
| T9a: crewd-collab: wakes::prune deletes at most a given number of rows per call, settled earliest first (S28) | `9e4a47287d7fdc05fb03feb370a58bf897d30ea9` | S/task-9a-report.md including fix1 |
| T9a: crewd-server: restart asks the human before resuming, staggered resume, ask defaults and bounded wake pruning on tick, shutdown, actor loop and Handle | `756650656612de62dad6064767b2125a84028230` | S/task-9a-report.md including fix1 |
| T9a: crewd-server: hooks run under a time budget; the IdleHook is asked outside the actor and a stale answer is dropped (C25); halting branches of rulings (4)(6) tested | `840967786d75d7b0b0cf8162c53b6ad05c9a3474` | S/task-9a-report.md including fix1 |
| T9a: crewd-server: side effects at exit / launch failure log and continue so the carried batch is always settled; the invariant and its boot-only cases are written down (8b review 1, 5) | `0476490a6a5de346440834d202311ea7b32a7d81` | S/task-9a-report.md including fix1 |
| T9a: crewd-server: spawn hands back the actor task; a panic in the actor is surfaced by Actor::finished instead of leaving a silently dead handle (C25) | `3f271ea52f0895fa6ed90697d6b42218729d27ab` | S/task-9a-report.md including fix1 |
| T9a: crewd-server: Actor::finished reports the JoinError text as is (it already carries the panic message) | `db44cf3915d83188ab2c2488b8f8928726c086a2` | S/task-9a-report.md including fix1 |
| T9a: crewd-server: fail_inflight hands back the given-up rows so the boot alert never looks them up in the capped wake list; MemPort caps list_wakes and records last_error like the store (9a review 1, 4) | `6b6b3166e268985a2503d7a27d4d3505f3d65fe9` | S/task-9a-report.md including fix1 |
| T9a: crewd-server: Handle::shutdown says whether it finished within the limit; Actor::abort_handle lets serve stop the actor after a timed-out shutdown (9a review 2) | `221571b116cd0664d8d3f74b2dcd69e4d08ef36f` | S/task-9a-report.md including fix1 |
| T9a: crewd-server: the next not_before is cached and only re-read after the queue changes, instead of a store read after every actor input (9a review 3) | `bdea73f3e50c449bf5061fc1c3899ae9a165e0d8` | S/task-9a-report.md including fix1 |
| T9a: crewd-server: a future wake Core queues itself also invalidates the cached deadline (test) | `256e32ee792a5beec23067070492a23d4ef2a0d8` | S/task-9a-report.md including fix1 |
| T9a: crewd-server + crewd-collab: in-actor hook contract written on AskAnsweredHook / ContextExtras; prune notes its unindexed scan (9a review 5) | `3971177c0f53bcb9952a76f636eec59b46c095b5` | S/task-9a-report.md including fix1 |
| T9b: crewd-wake: hibernate resume note (zh/en) and a wake batch summary for the hibernate record (C26) | `8da1f630cf03c8aa517f63f194821bc324e66991` | S/task-9b-report.md including fix1 |
| T9b: crewd-proto + crewd-collab: hibernate record in meta, hibernate deadline setting, restart prompt says reason=hibernate with the interrupted list (C26) | `48debcf7e92d74b5b715b5ccd60e57ebf1e51da2` | S/task-9b-report.md including fix1 |
| T9b: crewd-server: the cached next not_before is trusted for at most one heartbeat (9a re-review, folded into T9b) | `b4b52612ab020098f43347d0ec88e79784912940` | S/task-9b-report.md including fix1 |
| T9b: crewd-server: hibernate — stop new work, let working sessions reach a safe point until the deadline, interrupt the rest, requeue every wake, record each session; boot asks as hibernate and resumes with the hibernate note (C26) | `0bc4d50acdd2291f28195aeabcc7c1d30616edec` | S/task-9b-report.md including fix1 |
| T9b: crewd-server: hibernate test pins that the record is written before anything is stopped | `07203ffcf8d9d30e515499b4c7a87233f8ea6299` | S/task-9b-report.md including fix1 |
| T9b: crewd-server: the hibernate handle test waits with a bound, so a hibernate that never reports fails instead of hanging | `da6d4f0b7b652c1b02ff0f6989e475179c0ba8d9` | S/task-9b-report.md including fix1 |
| T9b: crewd-wake + crewd-server: sessions idle at hibernate resume silently, no note (9b review I1) | `28484304238bd29af1c4f8ac4dbaaff47ef7f505` | S/task-9b-report.md including fix1 |
| T9b: crewd-server: the heartbeat is a fixed interval that inputs never reset, so a busy actor still ticks (9b review I2) | `dc3fa8215acd971b0c1edd6d911fcee6a819ac40` | S/task-9b-report.md including fix1 |
| T9b: crewd-collab + crewd-server: hibernate entries carry a pending/finished/interrupted outcome (a crash mid-hibernate resumes as interrupted); the record stays linked to its prompt after a resume; a corrupt record degrades to a plain restart; the notice counts hibernated sessions only (9b review I3, I4, m3, m4) | `395b7202dd367a52e8502c3dcd395a102198156b` | S/task-9b-report.md including fix1 |
| T9b: crewd-server: after Esc a hibernating session gets up to 5 s to end its turn before the stop; a hibernate after the report gets that report at once; only held sessions' unresumed entries are carried over (9b review m1, m2, m4) | `e26273f4351554406418c89352d56cc552a81849` | S/task-9b-report.md including fix1 |
| Merge N0/N4/N1 and corrected codes: merge and retained report resolution | `808362f64f1fde2485f89ef127f390c259350fb6` | S/merge-main-report.md |
| Merge N0/N4/N1 and corrected codes: merge and retained report resolution | `936765f980e73a2728b173eace3a0bcd479950df` | S/merge-main-report.md |
| Merge N0/N4/N1 and corrected codes: crewd-server: drop the approval-result mention from a deliver.rs comment (no approvals, C27 / spec §26.7) | `fe50a9e51dbc08901a0df56937a509c066a3bdfb` | S/merge-main-report.md |
| T10a: crewd-wake: @ only decides who wakes (no narrowing, no broadcast); a dm wakes the other party; candidates = members + listeners minus the sender | `bfaa92f9a3d9e3365067320e503d23224acefe91` | S/task-10a-report.md |
| T10a: crewd-collab: build_batch takes now_ms (an expired listen no longer delivers), the wake message part has a byte budget, latch Scope::id / Scope::parse are public | `1b0f1baa2c452196cbb7cc945bd82b9c8b32d785` | S/task-10a-report.md |
| T10a: crewd-server: DbPort over collab and the collab -> orchestra bridge (fact_of, pump_collab, forward_collab) | `8b7456958647894a4b734b80bb4fa307bbe0a17e` | S/task-10a-report.md |
| T10a: crewd-server: OrchestraControl implements SessionControl; a removed crew's sessions are refused or skipped, never fail a batch; Core's language comes from the setting | `58846d2790d5394f0d56b8b6d2a7d30e11007ef0` | S/task-10a-report.md |
| T10a: crewd-server: test pins that a skipped removed-crew session keeps its unconsumed hibernate note | `72cae8034f0e125756066b4b152ca144224b3c14` | S/task-10a-report.md |
| T10a: crewd-server: arbiter budget 2 s (C25); after Esc wait at most half the remaining grace; an exact tick restarts the heartbeat; no nudges to a session cut by hibernate | `6416c725652fca86dc46c2eda8f72bf47b7b4026` | S/task-10a-report.md |
| T10a: crewd-server: one Port contract suite runs against MemPort and DbPort, plus the injection-vs-planner contract | `eff46b27bc1673c9c981d18b2e39c3734010ffd3` | S/task-10a-report.md |
| T10a: crewd-server: tests for a claim that loses a race (the next tick goes without the dropped row) and for facts_of's time fallback and target-less mentions | `57e6da670f97ee3f14e80685d0c971cfec43bab3` | S/task-10a-report.md |
| T10b: crewd-wake + crewd-server: why_not_woken is the one place that says why a wake has not gone out; try_deliver decides with it and wake.list shows it (C25 / R9) | `64ebd100cae2a675d853d673454d2781b5e60e17` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-collab + crewd-server: every orchestra refusal carries a stable code; collab refusals (insert_session) pass through with theirs; storage failures are -32010 (C14, S26) | `b186a4e57bea5075f79195c3309dcc5c56647542` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-server: test that a storage failure wrapped in CollabError::Store (DbPort's path) is still Storage (mutation C2 was green, C15) | `1a863e03fcdef1d6246ef57a1d6868deb9cd7fa6` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-proto + crewd-server: M3c client RPC — session.*, crew.setCaptain, wake.list (blockers + targets), loop.list / loop.recover, settings.get / set, crewd.hibernate, message.receipts; dispatch hands them to the orchestra or answers orchestra.not_running | `2f0415590f93845229eb196c9d59f699419e9c51` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-proto + crewd-server: system.hello carries minApiVersion, the build identity (git sha, dirty, exe, exe still on disk) and capabilities; clients report name / version / build / apiVersion; the compatibility rule is written down (U1, S6, S9) | `c1bfc6bf6988d9e75688080bb56c946bd748eb66` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-server + crewd-collab: T10a review folds — contract cases for settle, rider release / drop and ownership, prune by settle time, post_system mentions, message() channel mismatch, archived dm, unknown crew; DbPort render_wake test; S45 multi-channel budget tests | `8a4e2c4ce562725a3814ab46c54a28a1113db92a` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-server: after the actor has exited every handle call is ShuttingDown (-32009 orchestra.shutting_down), not a numbered internal error; exit_requested documents reply-before-socket-write and sender-dropped = exit (T10b fix I1, I3) | `67f26095b58fb3a3e23cb43724c683e36d3c8e35` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-server: build.rs runs git with GIT_OPTIONAL_LOCKS=0, scopes dirty to the crewd workspace, and reruns when crates/ or Cargo.lock change, so dirty does not go stale and a second build stays Fresh (T10b fix I2) | `712b72370ec6b9fb62099dcde882e36ee13a4c76` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-wake + crewd-server: explain::live_blocker is the only readiness rule; why_not_woken and Core's quota-health clear both use it (T10b fix M1) | `5f5f7f3d5632e24820e806481728ce44c3048fbb` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-server: test that a due quota wake does not clear quota health while the target is busy (mutation M1b was green) | `5a988b82a95e9be0bb25e8cd48ad09626bdbbe69` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-server + crewd-proto: message.receipts reports a rider of the batch in flight as queued, and working means the target is busy now (T10b fix M2) | `9e4eb01f3d01cc1c0bbb6ec5f37308470a5c29f5` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd: regenerate the API schema for the receipts doc text (9e4eb01f was committed with a stale schema; the gate was red) | `bca72d7a3092a2c3f0f75b333f405c300b94a7a3` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-server + crewd-proto: set_captain's not-found carries the session id only; WakeBlocker documents what a missing blocker means (T10b fix M6, M7) | `2ef07a1c6e001b0928a44a6a61d0c4c352252214` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-collab + crewd-server: an isolated start in a crew without a working directory is crew.no_workdir [crew], not a Chinese sentence; a scanner keeps prose out of every ControlError built in orchestra (T10b fix M3) | `1a30c9c20dab0faadf26615fa7d3ecf07009f6a5` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-server: settings.set writes in one transaction (Port::set_settings); after a saved latch release or limit change a failed rescan is logged, not returned (T10b fix M4) | `143873c85d5e791ecaf55122cf20f08c66d909d7` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10b: crewd-proto + crewd-server: settings.set rejects locale null with settings.bad_locale (null only means "clear", and a language cannot be cleared); the schema shows locale as non-nullable (T10b fix M5) | `cb225667f14503821810a700f33722557b3ca791` | S/task-10b-report.md including fix1; 9e4eb gate-red schema history retained |
| T10d: crewd-term: PendingDecision.selected (highlighted option) and Screen::input_draft (Empty / Text / Unknown) for T10d | `f95f232d7feba8d323b232f36524b60b7654c006` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: crewd-exec: answer a pending dialog with re-read keys (fingerprint + highlight CAS) and hold bodies behind an input-box draft (T10d) | `5aff213b10b98be25e02a963c1d7141932dd1096` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: crewd-wake: answer_plan (digits on numbered boxes up to 9, else arrows from the highlight, else needs_terminal), up/down in NUDGE_KEYS, Blocker::InputDraft in live_blocker (T10d) | `7fa4ba9f301f1804cb09d8782b1e1086e9958018` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: crewd-server: answers and key nudges go through the executor's safe keys (no sleeps in orchestra); input drafts hold deliveries and refuse text nudges (T10d) | `cd5dfb561d1f7916055738f9c1cca9cedf37c2d9` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: crewd-server: Cmd::Answer hands the executor the caller's fingerprint, not the box the actor holds (T10d) | `9699994dc9157724507cd5a89b06623ba2b131b2` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: crewd-exec: the box-replaced answer test keeps the highlight where the next key expects it, so only the fingerprint check can stop it (T10d mutation M1 was masked) | `fe5cd6922d047c304718cca55c584d1bd0623d9e` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: crewd-exec + crewd-server: the draft guard keys on input provenance (raw input since the box last read blank); Unknown is reported as unknown, not as the visible row; answer keys timed from the previous write; drain PTY output before ticks and answers (T10d fix R1, R3, M2, M3) | `63f11e67a51bcb2cb841ddd85714b198e9a5d9aa` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: crewd-term: mid greys count as placeholder colour: 256-colour 232..=250 and truecolor r~g~b within 16 in 96..=200 (T10d fix R2) | `aea2e30e58fd4514cb4ff690be8d0fa6550863be` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: crewd-server: named-key nudges check the highlight at acceptance; `interrupt` interrupts; idle esc / interrupt get session.nothing_to_interrupt; a text nudge's Ok means queued (T10d fix M1, M4, M6, M7) | `7ef4b0937f4080526070a620947209b88f819d25` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: crewd-exec: drain all readable PTY chunks before answer keys (T10d fix R2) | `fc421d65a53f864a81da5fdd72746ffcf46b1812` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: fix(crewd-exec): hold pending raw echo and revoke colliding PTY writes | `d5af330b77aed1da10e8e3c01d1ae94342c32518` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10d: fix(exec): observe fresh input lifecycle after timer drains | `7b88bf7e676c13df244a0da2ac4ac43460909bab` | S/task-10d-report.md; original owned R3 report; C/t12-evidence-inventory.md R4 named logs |
| T10c initial and R1: feat(crewd): human pending decisions and transition-only notices | `839ff4d2822d1e49578bd668e59c5848ec2cf099` | C/t12-evidence-inventory.md initial+R1 named logs; original full prose report MISSING |
| T10c initial and R1: fix(crewd): separate nudge receipts from answer writes and recover historical quota members | `018780c9526094be98590d3f5e37e8c007634123` | C/t12-evidence-inventory.md initial+R1 named logs; original full prose report MISSING |
| Durable human-stop: fix(crewd): persist human stop cancellation across recovery | `6340584bb2c0a96da8b6a9eea2a974c6590e3da3` | C/t12-evidence-inventory.md human-stop named logs |
| T11 combined production: Integrate accepted M3b and wire T11 orchestration into serve | `c1600f2f8a8ca7b428edc927bcc4e4871e8a82c5` | C/t11-candidate-evidence.md; T11 four mutation red/restore JSON/log; combined-independent-review.md |
| T11b original/scoped corrections: test(orchestra): add seeded Core crash and fault walk | `121b5b2ad2028e93f67c5fbf526e8ed1f3872c7b` | C/t12-evidence-inventory.md T11b logs and R1/R2 metadata; combined-independent-review.md |
| T11b original/scoped corrections: test(crewd): preserve abrupt simulation crashes and exact replay prefixes | `8c0dae560533d575158caeed47259e40ab3742de` | C/t12-evidence-inventory.md T11b logs and R1/R2 metadata; combined-independent-review.md |
| Accepted combination anchor: Integrate accepted T11b simulation and scoped crash/replay fixes | `648124c08a717376069e2925de1c2361768f204c` | C/combined-independent-review.md; historical1984/0/8 |
| Ack accepted five-file unit: crewd-exec: confirm delivery only after completed PTY submit | `7b72a156e604f3e1bc15fbc403d3f5d92b07d939` | C/launch-unit/write-completion-handback.md; C/flake-ack-*; C/C/flake-write-completion-review-prep.md FINAL scoped Spec/Quality PASS |

### 28.4 hooks 与后续绑定责任

M4b IdleHook/AskAnsweredHook tolerates missed calls after Lagged (cleared list cannot be reconstructed); work commands must call sessions::note_progress, chat is not work progress. M5b Arbiter uses two-second budget, owner fallback and drops late answers. QuotaSource/ContextExtras must run off actor or be memoised before real M3d/M4b/M5 hooks; the T11 one-second caller is not proof that all actor hooks or real provider feeding are implemented.

M3d inherits actual Codex request id/options -> RunnerRequest fingerprint -> human CAS answer_choice, with structured zero-based choice and no simulated keys/default approval/automatic refusal. StandIn coverage is contract evidence only. Delivered-before-Working(true) hibernate gap, per-MCP-server Notice and with_untrusted_context frame neutralisation also remain M3d work.

### 28.5 supplement 的 19 项 carry TODO

Explicit supplement owners are binding. Otherwise the engineering coordinator owns tracking; suggested downstream placement is marked [推], not a newly approved product requirement. Accepted behavioral limits are not reopened by this record.

| # | Remaining behavior / status | Owner and boundary |
|---|---|---|
| 1 | Three-party loops not caught | M3i-a causal hop count, human starts new chain, real work resets; no automatic latch recovery. |
| 2 | Captain progress depends on work commands | M4b note_progress mandatory, not last_output_at/chat receipt. |
| 3 | System->target latch blocks quota and stop-check too | Intentional retained design; coordinator documents; only human loop.recover. |
| 4 | Non-message wakes at-least-once | Retained contract; M3i-a integration must not claim universal exactly-once after crashes. |
| 5 | Resume note consumed even if subsequent launch fails | T9b boundary, coordinator tracks; [推] M3i-a future refinement, no automatic replay promise. |
| 6 | Corrupt hibernate record logs every boot until next hibernate | Honest fallback, bad row not repaired; coordinator tracks [推] N3/storage diagnostics, no deleting real data. |
| 7 | Old unanswered hibernate prompt reads restart after newer hibernate | Retained prompt linkage boundary; coordinator tracks [推] M3i-a. |
| 8 | Notifier/aggregation state in memory; restart re-raises once | Coordinator tracks [推] N2/client observability; no durable notification guarantee. |
| 9 | kill_group misses setsid pipe holders | Exec lifecycle follow-up; coordinator tracks [推] N3; no name-based process killing. |
| 10 | worktree duplicates exec pgid guard | Coordinator tracks [推] N3 cleanup; retain present guard. |
| 11 | set_profile cannot clear model/effort through COALESCE | Coordinator/config follow-up; next launch only; no T12 API change. |
| 12 | No standalone last_active_at writer, exit time lost | Coordinator tracks [推] N2 signals; output time is not progress. |
| 13 | Worktrees never deleted (old sync C8) | Intentional retained design, deletion belongs to separately authorized human operation. |
| 14 | Legacy StoreLedger bypasses N4 identity checks | N3; production T11 now uses DbLedger/same Db FIFO, legacy independent path still open. |
| 15 | FakeSessionControl looser than OrchestraControl | M3i-a contracts; fake passing is not real permissions proof. |
| 16 | Hook injection drops vs riders / double delivery | M3i-a reservation cursor + real caller wiring tests; not yet implemented. |
| 17 | Shared injection / wake message byte cap | M3i-a; retained current one-budget behavior, no claimed split-budget implementation. |
| 18 | Shared vocab Unknown and additive schema snapshot | iOS/client prep; tolerant decoding plus actual compatibility verification. |
| 19 | Socket / EPERM | N3 legal runner permission and startup diagnostics; earlier valid run0/frame0 is not behavioral RED and never authorizes bypass. |

Additional carried NOTE boundaries (S/progress.md:37,47): failed notification posts during raise/escalate are not retried; no eventual notice-delivery guarantee is claimed. Whether real Claude menus accept two-digit typed answers remains unverified; synthetic key coverage does not validate real TUI behavior, and this record does not authorize live recording.

Additional boundaries: original plan question2 real quota data source stays OPEN/Unknown with default45-minute fallback; question15 due-default accuracy remains heartbeat30s, large-organization earliest-deadline optimization unmeasured. wake.list does not fully simulate already-injected/batchcap; receipt working is target current busy; builddirty freshness minor and prose scanner coverage remain recorded. Real grey typed draft after Shown may parse Empty; no authorized live recording performed, Unknown/raw fail-closed must remain. HealthKindView turn_failed/version_incompatible and Db JoinHandle are resolved by M3a.1/N4, not remaining deficiencies.

### 28.6 既有 spec 条目落点

Section24.3 M3c T9a/T9b/T10a/T10b/T10c/T10d/T11 map to the full unit/commit/evidence table28.3. N3 runtime token/helper/launch files are partly implemented by T11; that is not completion of socket/EPERM or all data-dir diagnostics.

| Section26.5 item | Placement / boundary |
|---|---|
| work refusal/parameter stable codes | Unchanged/deferred work/client task, not M3c code completion. |
| listen until | T10a1b0f1baa2c452196cbb7cc945bd82b9c8b32d785: now_ms and batch eligibility. |
| Old long titles | N1 migration boundary unchanged; cannot refuse rename solely because inherited title is too long. |
| Crew number unique index | Deferred schema migration with collision detection, no M3c index change. |
| Removed-session skip/claim race | T10a58846d2790d5394f0d56b8b6d2a7d30e11007ef0 and57e6da670f97ee3f14e80685d0c971cfec43bab3: per-session skip; failed claim batch retries next tick. |
| answered_ask receipt | Client Message wire unchanged; agent receipt belongs M3b section27. |
| Management non-member post replies | Current membership-based wake boundary; later receiver receipt linkage belongs M3i-a. |
| readable full scan/lazywrite/forest constraints | Unchanged N1/storage boundaries. |
| ensure_management id collision | Fail-loud migration boundary unchanged, never rebuild by deleting data. |
| Object purge | M4b must delete objects with purged crew, removed crew objects freeze but remain readable. |
| Historical approval/single-root plan | Section24/26 supersede; old plans remain history, no revival. |

| Section26.7 row | Task/commit/evidence |
|---|---|
| 1 broadcast/narrowing removal is semantic, not compile | `bfaa92f9a3d9e3365067320e503d23224acefe91`; S/task-10a-report.md §2.1: plan.rs removes Broadcast/visible_to; S/merge-main-report.md “§26.7 rows that were wrong or missing” records why there was no compile break. |
| 2 new membership injection/planner contract | `eff46b27bc1673c9c981d18b2e39c3734010ffd3`; S/task-10a-report.md §2.7: orchestra/port_contract.rs shared injection-vs-planner fixture and fifteen cases on both Ports. |
| 3 dm peer default/ctx | `bfaa92f9a3d9e3365067320e503d23224acefe91` + `8b7456958647894a4b734b80bb4fa307bbe0a17e`; S/task-10a-report.md §2.1 + §2.2: planner/MemPort dm_sessions followed by DbPort::channel_ctx filling actual dm session membership. |
| 4 MemPort unread mirror | `bfaa92f9a3d9e3365067320e503d23224acefe91` + `eff46b27bc1673c9c981d18b2e39c3734010ffd3`; S/task-10a-report.md §2.1 + §2.7: visible_to removal and mem_port_unread_mirrors_collab_unread_for first; later shared contract refinements, not first removal. |
| 5 build_batch membership/listen/now_ms | `1b0f1baa2c452196cbb7cc945bd82b9c8b32d785`; S/task-10a-report.md §2.6: collab render build_batch eligibility and real now_ms; DbPort caller documented in §2.2. |
| 6 old approval comment | `fe50a9e51dbc08901a0df56937a509c066a3bdfb`; S/merge-main-report.md “T10a items pulled forward”: orchestra/deliver.rs comment only; C27, no approval feature restored. |
| 7 home human channel | `8b7456958647894a4b734b80bb4fa307bbe0a17e`; S/task-10a-report.md §2.2: DbPort::post_system(HumanChannel) uses home_of; §2.7 shared human-home fixture. |
| 8 StartNew title/brief | `8b7456958647894a4b734b80bb4fa307bbe0a17e` + `58846d2790d5394f0d56b8b6d2a7d30e11007ef0`; S/task-10a-report.md §2.2: DbPort::insert_session forwards title/brief to collab; §2.4 adds removed-crew admission/locale, not the title/brief forwarding. |
| 9 removed admission/restart/claim race | `58846d2790d5394f0d56b8b6d2a7d30e11007ef0` + `57e6da670f97ee3f14e80685d0c971cfec43bab3` + `72cae8034f0e125756066b4b152ca144224b3c14`; S/task-10a-report.md §2.4 and mutations C15 note: removed-crew gates, a_row_dropped_between_pending_and_claim_fails_that_batch_and_the_next_tick_goes_without_it, plus at-most-once hibernate note regression protecting the skipped session. |
| 10 bad hibernate StoreInvalid | `936765f980e73a2728b173eace3a0bcd479950df`; S/merge-main-report.md “Compile / test breaks after the merge”: collab/hibernate::get bad meta row -> Store(Invalid), updated test. |
| 11 settings bad deadline refusal | `936765f980e73a2728b173eace3a0bcd479950df`; S/merge-main-report.md “Compile / test breaks after the merge”: settings.bad_hibernate_deadline code, bilingual placeholders and refusal assertion. |
| 12 restart by_home/refusal/hibernate count | `808362f64f1fde2485f89ef127f390c259350fb6`; S/merge-main-report.md “Conflicts (2 files, 6 hunks)”: collab/restart.rs by_home/home_of, per-home hibernate count, RestartNotFound and unreadable-record diagnostic. |
| 13 wakes codes/ack_failed/session mismatch StoreInvalid | `808362f64f1fde2485f89ef127f390c259350fb6`; S/merge-main-report.md “Conflicts” and “§26.7 rows that were wrong or missing”: collab/wakes apply/ack_failed stable refusals and ack_delivered_refuses_a_bad_row_without_touching_the_rest expects Store(Invalid). 936765f changes no wakes.rs. |
| 14 N1 crews tests mark_inflight -> claim | `936765f980e73a2728b173eace3a0bcd479950df`; S/merge-main-report.md “Compile / test breaks after the merge” item3: crews.rs calls changed to claim(tx, &[id]).map(|_| ()). |
| 15 stored bad-row English / prose scanner | `808362f64f1fde2485f89ef127f390c259350fb6` + `936765f980e73a2728b173eace3a0bcd479950df`; S/merge-main-report.md “Readings and their limits” / “Conflicts” / “Compile / test breaks”: bad stored wakes state/session mismatch and restart diagnostic at merge; hibernate invalid row follow-up. Prose-scan scope is reported, not claimed exhaustive wording review. |
| 16 coded RPC dispatch | `2f0415590f93845229eb196c9d59f699419e9c51`; S/task-10b-report.md §2.1 / §2.5 / §2.6 / §2.7: orchestra method dispatch after agent.attach; §2.4 stable error/refusal mapping. S/merge-main-report.md records dispatch had not yet been wired at earlier merge. |

No exhaustive Enqueued matches existed in M3c at merge: old testkit used let-else; T10 DbPort handles Skipped. restart stderr historical line34 rather than28, crews calls1866/1905 rather than original prediction; current functions beat old line numbers. Session-mismatch ack_delivered is Store(Invalid), with its regression; ack_failed also carries stable codes. Merge report1391/0/6 is a historical single sample, not final count.

### 28.7 跨计划 notes、S101 与未采用方案

Section23 continuous window/deferred/cursor_targets/backlog-only/per-channel UNREAD_CAP/filter-already-injected land in T8->T8b shared ack and T10a render/contracts; full commits28.3 identify accepted implementation anchors, not invented first-introduction chronology. Render corruption is fail-loud, not tight retry; bad latch ids are Invalid. Timestamp fallback and targetless mentions land57e6da67; origin/explicit-woke/near-monotonic, dedup mentions and single batch FIFO are pinned by shared lifecycle/Port contracts. Old Pending-to-Retry is superseded C24. System alert/human-only transition notification refinement lands T10c839/018.

Boot-only rescue is ca40866; halting/preparing/noPump, settle before exit side effects and loud panic are756/840/047/3f; bounded prune/cache/heartbeat/hibernate notes are T9a/b. Ask origin and Lagged rescan are T10a8b745/57e6, while missed cleared hooks remain tolerated by M4b. Shared schema generator/union, token revoke, Locale and serve DBjoin land T10b/T11. Codex untrusted context/Notice remain M3d. Non-code NOTE categories not tied to unique historical metadata are mapped by category rather than assigned fabricated first completion dates.

S101 sidebar human-order text already exists in1G3/7.2/8.3/11Q4; crossed-out paragraphs remain history. Section20's old Jev urgency-cache guidance is explicitly superseded by S101 / section24.4; historical measurements remain unchanged. Old message-model C7 contact/report_to_parent/message_child_crew syntax is void, cross-crew communication is dm; controller C7 Rig safety is not void.

Reasons touching M3c: loom/turmoil/madsim deferred/rejected because races concern message order; token bucket damages normally busy crews. Human-only loop recovery is preserved, made visible rather than automatically cleared. Restart asks human and captain replacement belongs human, reminders do not take over. proptest/shrink/minidump await demonstrated value and no unapproved new dependency. launchd/SMAppService conflicts with no-login-item decision and requires measuring EPERM first. Full stream-json/SDK replacement loses TUI attach. Real quota double-wake/settings, Codex automatic compression and Claude structured signals remain later M3d/N2 verification. Early synthesis approval-ledger question is superseded by human removal of product approvals and must not reappear as an approval gate.

### 28.8 实测、变异与 flake 记录

These final results are personally parsed retained logs/JSON, not newly executed by this preparer. One gate invocation is one sample.

| Evidence | Measured record |
|---|---|
| Precommit source gate | C/flake-ack-full-gate.log/json:1997/0/8 exit0, HEAD648124c0 with uncommitted ack edits. |
| Committed restored gate | C/flake-ack-restored-full-gate.log/json:1997/0/8 exit0, HEAD7b72a156, MERGE_HEAD empty. |
| Same declared source | Both source_before=source_after=1e4cc22c8992963b30402b33ad7f0ee32e7a9cd0cd09778e6be871438922a296. This is two same-source green gates, not two committed7b72 runs. |
| Three full synthetic chains | C/flake-ack-chain1/2/3-{build-fake,server,restart,e2e}.log/json:each build exit0/server4/0/restart3/0/e2e9/0, all7b72 and same source hash. |
| Sim | C/flake-ack-final-sim.log/json:32seeds0..31 x400=12800random steps, wall8.248490417s, test1/0 exit0; compile time separate. Crash selected298/applied276/skipped22; skipped pending-RPC crash cases are not passes. |
| Environment | Pinned1.94.0/offline/jobs2/threads2/cargo runner, synthetic HOME/config/cache, false provider/SHELL and own target. Eight ignored not run; no real CLI/auth/live/paid API. |

Core/MemPort sim is not DbPort real-daemon evidence. Pending-RPC abruptcrash skip is not a pass. The coordinator's pre-document identity record is given above; filename alone does not prove clean. Historical648 gate1984/0/8 and wall8.127502458s remain historical evidence only.

Final-table mutation counting is distinct variants, not tests or all attempts. Earlier masked/setup failures remain visible. Full named-log/HEAD/command/exit gaps are in C/t12-evidence-inventory.md, never treated as zero.

| Unit/table | Final distinct variants | Caught | Uncaught | Equivalent | Earlier/non-final evidence |
|---|---:|---:|---:|---:|---|
| Task8b table |13|13|0|0|m3b early test non-discriminating; actual early mutation execution/count MISSING|
| T9a original table |23|21|1|1|c3 masked stays green; p1 explicitly equivalent; full raw attempt history MISSING|
| T9a fix1 prose list |6|6|0|0|1 earlier weak expiry mutant stays green, replaced; count it as an additional historical uncaught variant, not final caught|
| T9b original table |21|20|0|1|m17 explicitly equivalent; m4 first run hung/exact-PID killed, then bounded regression caught; exact first-run completed count MISSING|
| T9b fix1 table |12|12|0|0|No setup failure declared in this table; exhaustive earlier attempt totals MISSING|
| T10a expanded table |39|38|0|1|P6 equivalent; K4 initially masked green then note assertion caught. F1 given-up differs from fold F1 namespace|
| T10b initial expanded sets |62|62|0|0|C2 first GREEN then new test caught; D3 first SKIPPED because fmt text mismatch then caught: one documented setup skip|
| T10b fix1 table |6|6|0|0|M1b first GREEN then caught; B3 has 2 failing script checks but is one mutant|
| T10d initial table |8|8|0|0|M1 first unit GREEN but e2e RED; later corrected unit RED. M1/M2 reruns not new variants|
| T10d fix1 table |5|5|0|0|f1 gate old-artifact path setup error is not a mutant|
| T10d fix2 table |2|2|0|0|Each ran 3 tests; rows not tests|
| T10d fix3 |1|1|0|0|One Typed+Empty common policy mutant, one named PTY failure|
| T10d fix4 |1|1|0|0|Same timer lifecycle mutant observed via 8-test run (5/3) and 1-test full-loop run (0/1)|
| T10c initial |4|4|0|0|Four retained named logs each 0/1; original report file MISSING (text handback only)|
| T10c R1 |5|5|0|0|Six intended failing test executions across five mutants; no file report, original text-only handback|
| Durable human-stop |3|3|0|0|Four named RED logs: cancellation predicate exercised old and new-work paths twice; requeue and monotonic-floor one each|
| T11b 121b |3|2|1|0|P7a default+old regression each GREEN; split-ack and boot-refund each one caught test|
| T11b 8c R1/R2 |2|2|0|0|R1 2 failing executions, R2 1; restoration/gate separate|
| T11 c160 |4|4|0|0|Each 0/1 exit101→1/0 exit0. Wiring first restore stayed RED from stale artifact; fresh compile restore then green. Restore error is not mutant setup failure|

Exact arithmetic sum of these 19 final tables (not historical attempt totals): 220 variants = 215 caught + 2 uncaught + 3 equivalent. Cross-task duplicate effects are not deduplicated. This count excludes historical replacement/setup attempts and is not coverage percentage. No approximate ledger “~45” used.



Ack postcommit adds4 final distinct/caught,0uncaught/0equivalent: empty-authority, actual-write-gate, pending-bound, empty-initial. C/flake-ack-mutation-record.json and each red/restore.log/json show0/1/0exit101 ->1/0/0exit0, original/restored byteSHA equal. Combined final-table arithmetic224=219caught+2uncaught+3equivalent, not coverage percentage, excluding unquantified all-attempt/setup history.

Keep T11b P7a guard-only UNCAUGHT. Original121b B/C and8c R1/R2 do not imply all pending-RPC crash cases covered. T11 four meaningful behavior REDs: omitted wiring refuses orchestra.not_running; omitted final shutdown event fails event-before-EOF; omitted identity notice fails synthetic admission closure (not actual inode replacement); quota1s->3s exceeds outer1.5s actual shared caller bound. Wiring first restoration stayed101 due oldmtime/stale artifact and is retained separately; fresh compiling restoration passed. DB abort barrier is inside job before Tx, not midway through multiple SQL writes. EPERM probe valid-run0/frame0 is permission evidence, not behavior RED.

| Flake register | Status and finite evidence |
|---|---|
| T9a worktree cold deadline |72bf67d warms syntheticGit and uses2s deadline; old20load evidence stays S/task-9a-report.md. Not newly rerun here. |
| Exec huge brief | Original fixed20 planned rounds stopped at round12 fail on648 after11green; C/flake-exec-load-record.json + C/flake-exec-load-12.log. FalseDelivered despite missing actual submission drove ack repair. Control coalescing is not proven as every failure's cause. |
| Slow echo | Original50ms Delivered expectation failure retained. Parent trace shows67bodychunks consumed after finite3x800ms ack budget and independent one-byte controls; not this preparer's new execution.50ms becomes bounded/no-unconfirmed-delivery negative control, with10ms strict one-complete-submit positive; unchanged window lengths. |
| Ack7b72 accepted scope | Four postcommit mutants/restorations, same-source two gates, three chains, fixed20 rounds x2positive cases=40invocations all exit0 (C/flake-ack-load-record.json). Independent final scoped Spec/Quality PASS at C/C/flake-write-completion-review-prep.md; five-file unit accepted, not T12/whole-M3c acceptance. |
| Helper closed_stdout/hookstdout | Each3baseline+20load observations, forty load invocations0exit, C/helper-flake-prep/first-load-report.md and summary:completed_samples_only_not_fixed. Identity is HEAD648124c08a717376069e2925de1c2361768f204c / declared source468958, not final7b72 source; final gate is not a new dedicated helper load sample. OPEN / UNREPRODUCED: no demonstrated root cause or repair. |

Initial pending-Enter behaviorRED, empty/bounded-write RED, compile/lint/setup failures and all restorations remain retained in C/launch-unit and C/flake-ack-*. None implies real provider or user-data validation.
