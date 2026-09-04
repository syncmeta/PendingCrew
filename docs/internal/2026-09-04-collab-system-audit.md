# 协作系统现状审计与主线收敛（2026-09-04）

- 日期：2026-09-04
- 触发：人类一次性列出约 16 个协作治理与产品问题，要求「先规划、不要批量开工」
- 范围：**只做 current main 与既有 Todo 的证据化去重与分层**。不改产品代码、不批量派活、不另开任务号。
- 基线：`main` @ `b9f37e4`（P5a 收尾）
- 账本基线：PendingCrew crew 的 Agent Todo 共 **99 条 / 95 completed / 4 in_progress**（#18 #49 #58 #84）；人类 Todo 5 条全部 completed。

## 证据边界（先说清楚，别让下一个人把它当实测）

本页全部结论来自**读 current main 的代码 + git log + docs/internal 既有文档**。

- **没有**编译、**没有**跑测试、**没有**启动 GUI 验证任何一条。
- 凡写「已实现」，指的是**代码在 main 上并有 file:line**，不等于人类实际用起来是对的。
- 少数条目标注了「本次未核」，那是真的没看，不是看了没写。

---

## 1. 去重矩阵：人类这次列的议题 → 既有 Todo → current main 现状

「映射」列写的是**既有 Todo 号**。本次一律只做映射，**不新开号**。

| # | 人类议题 | 映射既有 Todo | current main 现状 | 判定 |
|---|---|---|---|---|
| A | 跨实例/跨机器 session 与人协作（自部署、不依赖 PendingBot 账号） | #20 #44 #91 #84 #76 | P0–P3 早已合 main；P4 `a8f4597` 真进程分家；**P5a 今天刚把默认翻到 daemon**（`bdf92f3`/`f5749cc`，闸门 `ProcessRole.swift:38-43`）。异机直连（P6）**一行代码都没写**，只有范围页 `2026-09-04-cross-machine-transport-scope.md` | **部分**：同机常驻已通，异机未做 |
| B | 机长主动编排、上下级同步、防跑偏、定期反思、总机长监督 | 无既有 Todo（#16 #74 #91 只覆盖组织调整与救援） | 编排工具齐（`McpServer.swift` 34 个工具，19 个机长专用）；上下级同步只有**组织树 + 每 crew 最后一句 30 字预览**（`HookEmitter.swift:194-241`）；**防跑偏/定期反思/上级抽查：代码里零命中** | **未做**（核心缺口） |
| C | Todo 完成监督与自动收账 | #75 #94 #64 | **不存在任何停滞检测**。`CockpitPlan.swift:124-129` 明文写死「不做成提醒、不弹、不变红」，靠人眼看板 | **未做**（核心缺口） |
| D | 按上下文/专长/负载分配，而非只看空闲 | 无既有 Todo | `CrewSessionsSnapshot.Entry`（`:9-32`）只有 name/role/brief/state；state 仅 working/idle/awaiting*/rateLimited/error。分配依据是提示词散文（`crew-captain.zh.md:27`），代码无判据 | **未做** |
| E | 跨 harness / 跨 session 交叉验证 | 无既有 Todo | 提示词与工具面搜「复核/交叉验证」零命中 | **未做** |
| F | Agent 自有计划 / Todo | #66 #46 | 计划板已落地：四档 + `blocked` 必须指向人类 Todo #N（`CockpitPlan.swift:42-61,77-87,177-197`）+「最后更新 N 天前」展示 | **已解决** |
| G | 回复消息与 Todo 联动 | #64 #62 | 人类那本**已实现**（`CrewHumanTodoRespond.swift` 三步落账→发群→唤醒，`HumanTodoWakePlan.swift:71-109` 提问者退出则回落机长）；**agent 那本完全静默** —— `respond_todo`（`McpServer.swift:1095-1125`）不进群、不 @ 派活者，`LocalTodoStore.swift:104` 注释自认此事 | **部分** |
| H | 人类可见消息 vs agent 协调消息分层 | #61 #69 #79 | 已实现且是全项目唯一判定点：`CrewWhiteboardVisibility.swift`（session/captain 收窄、human 不收窄、broadcast 显式放宽），与「该不该叫醒」正交 | **已解决** |
| I | 群聊复制 / 历史连续加载 / 滚动稳定 / 到底箭头 | #45 #47 #54 #56 #60 #89 #28 | 全部已实现：复制 `CrewChatView.swift:1592-1608`；分页保位 `CrewChatWindow.swift` + `:1022-1121`；默认底部 `.defaultScrollAnchor`（`:1580-1586`）；跟随/未读/自动消失 `CrewChatBottomFollow.swift` | **已解决** |
| J | 群聊搜索聊天记录 | #49（账本仍 in_progress） | PendingCrew 侧**已闭环**：群内 `.searchable` 过滤 + 跳转（`CrewCenterView.swift:72`、`CrewChatView.swift:650-694`），跨群 `CrewGlobalSearchSheet.swift`。**PendingBot 群聊那半在另一仓库，本次未核** | **部分**（本仓已完成，账本没翻牌） |
| K | Sidebar 层级按更新时间排序、状态准确 | #67 #2 #50 #27 #71 #73 | 排序取白板最后消息时间而非 `updatedAt`（`CrewSidebarView.swift:376-382`）；时间流视图 `:26-48`；拖拽改父子已落地；状态灯优先级红>黄(呼吸)>绿>无（`CrewStatusAggregation.swift:1-64`）且父 crew 沿 DAG 聚合子 crew 黄点（`CrewHumanTodoAttentionCache.aggregate()`） | **已解决** |
| L | Cockpit 回归正确定位 | #3 #31 #46 #81 #96 | 已从多 tab 重构为单一「Agent 计划与想法」（`CockpitAgentMindView`），数据源是机长自己写的 `CockpitPlanStore`。旧 `CockpitRoadmapView.swift`/`CockpitPageView.swift` 成**孤儿代码**（仍在编译目标里）。开关慢有针对性修复 `5e758c8`，**commit 自述未做 GUI 手测** | **已解决 + 尾巴** |
| M | 账号 / 旧云代码清理 | #63 #85 #78 | 全仓无 Supabase / Keychain / 登录态实代码。`LocalCodingAgentSpec.swift:81-91` 的 `SUPABASE_` 前缀是**禁止透传给子进程的防御清单**（该留，不是残留）；`DeviceIdentity.swift` 用的是 UserDefaults 不是 Keychain | **已解决**（曾被误报为未清理，已复核否定） |
| N | Harness / effort / 首次选择 / 内置终端 UI | #36 #82 #70 #56 #83 #90 #37 | 见 §1.1（harness 分线专项） | 见 §1.1 |
| O | 上下文注入与 Codex 五小时额度 / 适配 | #65 #39 #26 #33 #12 #72 | 见 §1.1 | 见 §1.1 |
| P | session 初始历史 / worktree 纪律 / 任务类型扩展 | #56 #34 #68 #77 #58 #80 | 重启后**已是真常驻**（daemon 持锁，app 退化 viewer 重连），attach 断线重连有实测输出（`p5a-closed-loop-evidence.md`）。**但全部证据来自 CLI 身份，双击图标那条路没验过** | **部分** |
| Q | 人 / 人类称呼统一与整体 UI 完成度 | #19 #55 #85 #92 #95 #87 | 「人（本机）」零命中，称呼已统一 | **已解决** |
| R | session 卡在待回复 / 待决策不发声 | #6 #25 #98 | 判据 `SessionAwaitingReply.reason` 三态（approval/menu/question）；**只有 menu 场景**有 5 分钟自动升级 @human（`SessionPendingDecision.swift:202,212` + `CrewSessionRunner.swift:2549-2559`）；question/approval 无自动升级 | **部分** |
| S | 父机长救援不响应的子机长 | #91 #74 | 工具已落地且是 MCP 级：`create_and_handoff_captain(target_crew_id=直系子)`，`LocalCaptainReassignmentStore.swift:16-33` 限直系子 + fail-closed 回滚；人类另有右键 UI 路径 | **已解决（触发式）** —— 缺的是「谁发现它不响应」 |

### 1.1 harness / 额度 / 注入分线（矩阵 N / O）

| 条目 | 现状 | 判定 |
|---|---|---|
| 三种 runner | `LocalCodingAgentKind.swift:8-14` 定义 claude_code / codex / terminal；claude 走 PTY（`AgentSessionCore`），codex 走 app-server JSON-RPC（`CodexAppServerBackend.swift:38-46`），terminal 是纯 PTY 不接编排 | 已解决 |
| 新建 session UI | 三药丸 + 「设为机长」勾选（`CrewSessionWindowView.swift:799-824`）；model/effort **故意不在建前选**，建好后在终端页头部切（`:752` 注释） | 已解决 |
| 详情页三排 | 名字/停止(1142-1161) → model+effort(1164-1166) → 审批模式(**仅 codex**，1168-1177) | 已解决 |
| session 自切 model/effort | claude：等空闲注入斜杠命令 + 核对回显。**codex 也已实现**（`CodexAppServerBackend.applyProfileSwitch` → `thread/settings/update`，`:217-238`，成功返回 `.applied`） | 已解决 —— 但**说明书没跟上**，见 §2.3 |
| 模型表 | `AgentModelCatalog` probe/manual 两态；`ModelCatalogCenter.swift:41-50` 只在编排者进程起、6 小时探一轮，探不到保留旧值并记 error 不静默 | 已解决 |
| 额度采集 | `QuotaCenter.swift:57-65` 编排者 10 分钟一轮（claude `/usage`；codex `account/rateLimits/read`，问不到才回落 rollout jsonl）；viewer 60s 只读文件 | 已解决 |
| 额度将尽的收活/广播 | `QuotaWarningPlan`（按档位门槛 + 临近重置抑制，只提醒不代 session 收尾），对应 #26/#39 | 已解决 |
| 撞额度自动唤醒 | `autoScheduleQuotaWakeup`（`CrewSessionRunner.swift:846-874`）+ `QuotaWakeupPlan`：重置+1 分钟，解析不到退避 45 分钟并 fail-loud | **claude 已解决 / codex 有缺陷**，见 §2.4 |
| 上下文注入 | 白板**自动注入**不必显式查：claude 首轮 `initialPromptWithWhiteboard`（`LocalSessionLaunch.swift:13-35`）+ 后续 PostToolUse hook；codex 每轮 `whiteboardProvider`（`CrewSessionRunner.swift:1244-1245`）。新 session 注入最近 30 条历史全文（`WhiteboardCursor.swift:56-66`，Todo #56③） | 已解决 |
| 注入面截断 | 白板 hook 路**不截字数**只截条数(30)；唤醒预览/群聊摘要另一条通道单行截 200 字（`CrewRecentContextRender.swift:19-73`） | 已解决（两条通道不同口径，属已知性质） |
| codex UI 自然语言化 | `CodexActivityPresentation.command`（`CodexThreadItem.swift:167-224`）归纳成「已读取档案/已执行指令…」，细节收进 DisclosureGroup（`CodexTranscriptView.swift:170-225`） | 已解决（#83/#90） |
| worktree 纪律 | `SessionWorkspace.swift:5-30`：isolation=on 且非 git repo / detached HEAD 直接 throw，未见静默落回共享目录的分支 | 已解决 |

## 2. 新发现：不在人类清单上、但值得单列的风险

### 2.1 `local-crews.json` 在翻默认到 daemon 之后成了双进程写者

- **读代码读到的**：`LocalCrewStore.persistToDiskReportingFailure()`（`:652-658`）每次改动**整份 atomic 覆写**，没有跨进程锁，也没有版本号；它**不走** `MultiProcessJSONStore`。
- **读代码读到的**：daemon 进程会建 `CrewStore`（`SessionDaemonMain.swift:47`），排空控制通道时经 `setTitle` / `setAttention` 写它（`CrewStore.swift:453,464`）——这条有 `ownsSharedControlChannel` 门管着，写者是编排者。
- **读代码读到的**：app(viewer) 侧的人类操作**没有那道门**——隐藏 crew（`CrewStore.swift:245,256`）与人类改名（`:261`）直接写。
- **推出来的（未实测）**：P5a 翻默认之后 daemon 与 app 常态同时活着，两个进程各持内存副本再整份覆写 → **后写方整份抹掉先写方的改动，不报错、不留痕**。
- 同批被怀疑的另外两个「漏网单 writer」经复核**不成立**：`CrewChatAttachmentStore` 每份附件写独立 UUID 文件，无共享索引；`CaptainTemplateStore` 只有 GUI 侧 View 在用，daemon 不碰。

### 2.2 「活干完了没人翻牌」已经在账本上实际发生

Todo #49 的代码在本仓已闭环（§1 J），账本仍是 in_progress。这不是个别疏忽，是 §1 C 那条缺口的直接产物——**没有任何东西会在活干完时提醒任何人去翻牌**。

### 2.3 说明书说 codex 不能中途切模型，实现说能

- `Resources/Prompts/session-world-model.zh.md:172` 写死：「codex 无中途切换通道……让机长 `start_session` 带 model/effort 另起 session 接手」。
- `Sources/Mac/Services/CrewSessionRunner.swift:2735` 的方法注释同样写「codex=不支持」。
- 但下游 `CodexAppServerBackend.applyProfileSwitch`（`:217-238`）**是真实现**，走 `thread/settings/update`，成功返回 `.applied`。
- **后果（推论，未实测）**：codex session 撞额度或要换配置时，会照说明书去**另起一个 session**，而不是就地切 —— 白烧一份上下文与额度。
- 边界：我没有在真 codex 上跑过这条调用；它 catch 后返回 `.rejected`，所以「代码路径存在」不等于「真 codex 接受」。**修法的第一步是实测一次，而不是直接改文案。**

### 2.4 codex 撞额度的自动唤醒读错了窗

- `autoScheduleQuotaWakeup`（`CrewSessionRunner.swift:861`）对两家一律取 `snap?.fiveHourWindow?.resetsAt`。
- `fiveHourWindow` 认的是 label `session` / `5小时窗`（`AgentQuota.swift:181-184`）。
- 而 codex 的**主路径** app-server `account/rateLimits/read` 实测样例（`AgentQuota.swift` 注释）是 `primary.windowDurationMins = 10080`（周窗）、`secondary: null`；UI 侧也已写明「Codex 一个环：周（codex 侧已无 5 小时分档）」（`QuotaRingsFooter.swift:9`）。
- **后果（推论，未实测）**：codex session 撞额度后 `resetsAt` 恒 nil → 永远走 45 分钟盲目退避，而不是按真实重置时刻醒。claude 不受影响。
- 正确形状应是「按这一家真实存在的那个窗取重置时刻」，而不是硬取 5 小时窗。

---

## 3. 收敛：六条主线

去重之后，**人类列的 16 个议题里有 9 个已经在 main 上做完了**。真正没做的集中成下面六条。
每条都标了它吃掉矩阵里的哪几行，**不新开任务号**。

### L1 · 让「没动静」自己发声（P0 · 系统性根因）

- **吃掉**：矩阵 B（防跑偏/反思/监督）、C（Todo 监督与收账）、R（question/approval 卡住不升级）、S（子机长不响应没人发现）、§2.2
- **为什么是一条而不是四条**：这四件事缺的是同一个东西 —— 系统提供了工具和提示词，却**没有任何东西会在「该动而没动」时发出声音**。逐条打补丁会造出四个互不知情的提醒器。
- **最小闭环**：把已经存在的**单场景**升级器（`SessionPendingDecision.escalateAfter = 300` → 5 分钟后 @human，`CrewSessionRunner.swift:2549-2559`）抽成一个通用的「停滞判定 + 出口」，首批接三个源：① agent Todo 长期 in_progress；② 计划板条目久未更新；③ session `awaitingReply` 的 question / approval 态超时。出口三档：进群 @ 机长 → 升级成人类 Todo → 翻侧栏黄点（三档已经全部存在，只是没人触发它们）。
- **依赖**：无新架构。全部数据已在 `LocalTodoStore` / `CockpitPlanStore` / `CrewSessionsSnapshot` 里。
- **风险与失败保护**：吵。① 只在**状态发生变化**时发一次，绝不周期性重播（一条永远在报「已知没事」的提醒会训练所有人忽略它）；② 每条有抑制窗；③ 判定必须**先证明它会红** —— 造一条超期条目跑当前代码必须触发，再看它在正常数据上不触发。
- **验收**：造一条超期 Todo → 群里出现一条提醒，且**只出现一条**；把它翻牌 → 不再出现。

### L2 · Todo 与消息闭环（P1）

- **吃掉**：矩阵 G
- **现状**：人类那本已经闭环（`CrewHumanTodoRespond` 落账→发群→唤醒，提问者退出还会回落机长）；**agent 那本完全静默**，`respond_todo` 只写面板，不进群、不 @ 派活者。
- **最小闭环**：让 agent 本走同一条 landing flow —— 回应即进群、@ 派活的那一方；新建 Todo 时 @ 具体对象而不是泛标签。
- **依赖**：无。是现成代码的第二个调用方。
- **验收**：worker `respond_todo` 之后，群里出现一条 @ 派活者的回应。

### L3 · 派活依据：从「空闲/忙」到「上下文 + 负载 + 专长」（P1）

- **吃掉**：矩阵 D、E
- **现状**：机长点名只拿得到 `name/role/brief/state`，`state` 只有忙/闲/等待那几档。「按上下文和专长派活」今天完全是提示词里的一句期望，代码没有任何判据。
- **最小闭环**：给点名快照加几个**已经存在于系统里、只是没被暴露**的字段：在跑时长、工作目录/分支、harness+model+effort、连续失败次数、所属额度池。提示词从「先找空闲的」改成「按这几个字段选人」。交叉验证（矩阵 E）作为**派活的一种模式**挂在同一处：要复核某个结论时，优先挑**另一个 harness** 的 session。
- **依赖**：无。
- **验收**：`list_sessions` 输出含新字段；能据此写出「为什么派给它」。

### L4 · 常驻后台收尾与账本一致性（P0）

- **吃掉**：矩阵 A 的同机部分、P、§2.1
- **现状**：默认已经翻到 daemon（`ProcessRole.swift:38-43`），断线重连有实测输出 —— **但全部证据来自 CLI 身份，双击图标那条真实启动路径一次都没走过**（`p5a-flip-default-report.md` 自己写着）。P5b（开机自启 / 崩溃自拉 / 半开连接回收）未开始。叠加 §2.1 的 `local-crews.json` 双进程写。
- **最小闭环**：① 一次 GUI 真实路径的人工验收（双击起 → 关 app → session 不断 → 重开恢复同一 session）；② `local-crews.json` 收进多进程保护，或把 viewer 侧的写路统一改走控制通道；③ P5b。
- **依赖**：① 需要人类点一下（GUI 不许自动驱动）。
- **风险**：默认已经翻过去了，这三件是在**已上线的默认**下补验证 —— 顺序上 ①② 优先于 ③。
- **验收**：人工三条逐条「过/不过」；并发写不丢更新有测试钉住。

### L5 · 异机协作与移动端宿主（P2）

- **吃掉**：矩阵 A 的异机部分、Todo #18 / #44 / #91
- **现状**：P6 范围页已立、**一行代码没写**，接缝（`SessionMessageLink`）确实留好了，加第五个实现即可，不用重做。
- **需要人类拍板的一件事**：Todo #18 的原话是「PendingCrew 的 iOS 端弄到 PendingBot 的 crew tab」，而 `2026-08-10-pendingcrew-ios-driving-channel-design.md:199-200` 写死的方向**相反** —— 驾驶面宿主是 PendingCrew iOS，PendingBot 只留轻量消息 tab。两者不能同时成立。
- **依赖**：L4 稳。
- **验收**：两台机器一次配对后直连、断了能恢复、全程无账号无中继。

### L6 · 说明书与实现对齐（P1 · 最便宜、且正在持续造成浪费）

- **吃掉**：§2.3、§2.4、矩阵 L 的尾巴、矩阵 J 的账本
- **四件小事**：① 提示词说 codex 不能中途切模型、实现已支持（先实测再改文案）；② codex 撞额度的自动唤醒读的是它没有的 5 小时窗；③ `CockpitRoadmapView.swift` / `CockpitPageView.swift` 是孤儿代码但仍在编译目标里；④ Todo #49 在本仓已闭环、账本还挂着 in_progress（PendingBot 那半仍未核）。
- **为什么给到 P1**：①② 每天都在烧额度和时间，而修法都是几行。
- **验收**：四条各自消掉；②要有一条「codex 撞额度后按真实重置时刻醒」的证据。

### 优先级与顺序

| 档 | 主线 | 理由 |
|---|---|---|
| **P0** | L4、L1 | L4 是**已上线默认**的补验证与数据一致性，出事就是丢账；L1 是所有治理缺口的共同根因 |
| **P1** | L6、L2、L3 | L6 最便宜且正在流血；L2 是现成代码的第二个调用方；L3 让派活有依据 |
| **P2** | L5 | 等 L4 稳，且有一个方向要人类先拍板 |

L1/L2/L3 之间无依赖，可并行。L6 可随时插队。

## 4. 治理机制怎么设计（人类特别点名的那五件）

贯穿的一条原则：**提示词不是机制**。今天「机长该常态审视组织树」「派活先点名」「Todo 该翻牌」全写在 `crew-captain.zh.md` 里，靠自觉；自觉不留痕、不可验、换个 session 就归零。下面每条都要落成**代码里的判定 + 一个出口**。

1. **机长治理与防跑偏**：不做「定期自省」（那只会产出更多自我感觉良好的文字）。做**偏离的可观测量**：计划板上 in_progress 却 N 天没更新的条目数、派出去没收回的活、被翻成 blocked 却没有对应人类 Todo 的条目 —— 任一越线，进群说一句。
2. **Todo 监督与自动收账**：不做「自动判定活干完了」（判不准）。做**两侧对不上就发声**：Todo 仍 in_progress，而它指向的分支/提交已经进 main，或对应 session 已退出 —— 这两种「对不上」是可判定的。
3. **任务匹配**：见 L3。关键是把判据**暴露成字段**，而不是写进散文让机长猜。
4. **上下级同步**：今天父机长只拿得到「每个子 crew 最后一句话的 30 字预览」。补一个结构化摘要：各子 crew 的计划板条目数 / 停滞数 / 未回应人类 Todo 数。有了它，「谁不响应」才是**看出来的**，不是等人类发现的。
5. **交叉验证**：作为派活模式落进 L3 —— 复核结论时优先挑另一个 harness 的 session，并在群里留痕「这条结论由 X 复核过」。

**共同的失败保护**（这一段比机制本身重要）：

- 每个检测器上线前**必须先证明它会红**：造出该报的场景，跑当前代码必须触发；否则它就是一把永远绿的尺子。
- **只在状态变化时发一次**。永远在报「已知没事」的提醒，会训练所有人忽略掉它 —— 那比没有更糟。
- **不许静默降级**：判定拿不到数据时说「读不到」，不许当成「没问题」。
- **人类那一档是最后一档**，前面两档（进群 @ 机长、翻黄点）先扛。

## 5. 给人类的进度图（非技术）

**现在到哪了**

- 你这次列的 16 件事，**9 件在 main 上已经做完了** —— 群聊的复制/加载/滚动/到底箭头、聊天记录搜索、侧栏排序与状态灯、驾驶舱改成「Agent 自己的计划」、账号与旧云代码清理、称呼统一、消息分层、机长的整套编排工具、Codex 的界面自然语言化。
- **常驻后台今天刚翻默认**：关掉 app、更新 app，session 不再断。但这条只在命令行那条路上验过，**双击图标那条真实路径还没人验过一次**。
- **异机（跨机器）一行代码都没写**，只有一页范围文档；接口是留好了的，不用重做。

**差什么**

- 一句话：**这套系统给了所有人工具，但没有任何东西会在「该动而没动」的时候出声。** Todo 干完了没人翻牌、计划板放三天没人碰、session 卡在问题上、子机长不响应 —— 全靠你自己发现。这是四个症状，一个病。
- 另外有两处「说明书和实现对不上」正在天天烧额度：系统告诉 Codex 的 session「你不能中途换模型，去另起一个」，而代码其实早就能换了。

**下一步需要你什么**

1. **点一下**：一次真实的 GUI 验收（双击起 app → 关掉 → 看 session 是否还活着 → 重开是否回到原 session）。这条只能人做，我们不驱动图形界面。
2. **拍一个板**：PendingCrew 的手机端，到底是 **PendingCrew 自己的 iOS app 当驾驶面**（设计文档写的），还是 **塞进 PendingBot 的 crew tab**（你 Todo #18 写的）？两者冲突，挡着异机那条线往下走。
3. **认一个顺序**：先补「没动静自己发声」这一条（治四个症状的病根），还是先把常驻后台的收尾做完。我的建议是**两条并行**，因为后者主要是验证和数据一致性，不占前者的人手。
