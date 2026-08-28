# 前后端分离调研清单（2026-08-19）

本附录只依据当前仓库 Swift 源码做静态盘点；没有编译、运行 app 或启动 GUI。行号以本提交为准。这里的“持有”指对象、`Task`、`Timer`、订阅或连接的生命周期由谁控制，不代表逻辑一定定义在该文件。

## 清单 A — 界面持有的后台职责

| # | 谁 | 创建/启动所在视图（文件:行号） | 长期职责 | 定时器/轮询 |
|---:|---|---|---|---|
| A1 | `AppUpdater.shared` | `Sources/PendingCrewApp.swift:16,21-28`（`App.init` 首次触达）；`Sources/Mac/Views/MacRootView.swift:249-252`（注入 busy 判定） | Sparkle 自动更新检查；是否允许更新由当前是否有 running session 决定。 | 有，调度由 Sparkle 内部持有；仓库源码看不到周期。 |
| A2 | `CrewStore` + `LocalWhiteboardStore` 目录监听 | `Sources/PendingCrewApp.swift:8,16-19` 创建 app 级 `@StateObject`；`Sources/Mac/Views/MacRootView.swift:239` 首刷；`Sources/Stores/CrewStore.swift:126-127,326-350` 真正启动 | 常驻监视 `whiteboards/`，刷新侧栏末条消息，并排空 helper 写入的 rename、attention、session/组织/唤醒/listen 命令。 | 无周期 Timer；`DispatchSource` 文件系统事件，250 ms 合流见 `Sources/Stores/LocalWhiteboardStore.swift:95-116`。 |
| A3 | `CrewSessionRunner` | `Sources/Mac/Views/MacRootView.swift:21`（`MacThreePaneView @StateObject`）；命令入口 `Sources/Mac/Views/MacRootView.swift:71-218`；启动后台项 `Sources/Mac/Views/MacRootView.swift:224-228` | 持有全部本地 run/PTY/app-server、选中态、session 启停、profile、wakeup、listen、inspect/nudge/stop、快照、mailbox waker、permission relay；因此主窗口卸载即失去整个本地 session 编排所有者。 | 有：每 run 0.6 s、全局 2 s、任意时刻的一次性 wakeup；另有若干有界轮询，详见 B。 |
| A4 | `CrewRelayAgent` | `Sources/Mac/Views/MacRootView.swift:26`（`@StateObject`），`Sources/Mac/Views/MacRootView.swift:247` 启动 | 本地白板与 edge 双向搬运；维护每个 relay crew 的 realtime hub 连接，并处理远端 `task_request` 自动起 session。 | 5 s Timer 兜底；每个 hub 有 20 s WebSocket ping 和重连。 |
| A5 | `CrewLocalMentionWaker` | `Sources/Mac/Views/MacRootView.swift:230-235` 在 `.task` 中 `new` + `start`；`Sources/Mac/Services/CrewLocalMentionWaker.swift:59-83` | 监听 app 内 append 与 helper 跨进程目录变更，扫描新增定向 @，向 idle run 注入或拉起缺席目标；游标/指纹在内存。 | 无 Timer；事件订阅常驻，投递后会启动 B20 的回执采样。 |
| A6 | `LocalAgentUsageMonitor` | `Sources/Mac/Views/CrewSidebarView.swift:28,128`（`@StateObject` + `.onAppear start`） | 扫 Claude JSONL 与 Codex sqlite，汇总当天 token 用量供侧栏显示。 | 60 s `Task` 轮询。 |
| A7 | `QuotaCenter.shared` | `Sources/Mac/Views/CrewSidebarView.swift:29,65`（`.task start`） | 实探 Claude/Codex 订阅额度并写 `quota.json` 给 helper；快照变化还经 `MacRootView` 触发额度警戒广播。 | 立即一次 + 600 s Timer。 |
| A8 | `ModelCatalogCenter.shared` | `Sources/Mac/Views/CrewSidebarView.swift:65`（`.task start`） | 实探两家 CLI 可用模型/effort 并写 `models.json`，供 UI picker 和 helper 校验。 | 立即一次 + 6 h Timer。 |
| A9 | 白板变更订阅 | `Sources/Mac/Views/CrewChatView.swift:153,935-952` | 当前 crew 首刷后持续订阅本地目录事件或 edge realtime hub，刷新聊天时间线。 | 无固定周期；事件流。登录态底层包含 20 s hub ping。 |
| A10 | 机长待决策订阅 | `Sources/Mac/Views/CrewCenterView.swift:115-121` | 当前 crew 的 approval 变化到达时，找在跑 captain 并把新决策注入其会话。 | 无固定周期；目录/进程内事件流。 |
| A11 | session 审批卡订阅 | `Sources/Mac/Views/Chat/SessionApprovalCardsView.swift:38-44` | 当前 session 的 permission/decision 卡片随共享 approval 文件变化刷新。 | 无固定周期；目录/进程内事件流。 |
| A12 | Todo 面板订阅 | `Sources/Mac/Views/CrewTodoPanel.swift:66-70` | 当前 crew Todo 随 app/helper 写入刷新。 | 无固定周期；目录/进程内事件流。 |
| A13 | Todo 详情窗口订阅 | `Sources/Mac/Views/CrewTodoDetailWindow.swift:111-115` | 独立 Todo 窗口持续刷新当前 crew Todo。 | 无固定周期；目录/进程内事件流。 |
| A14 | 驾驶舱 Todo 订阅 | `Sources/Mac/Views/CockpitTasksView.swift:39-44` | 驾驶舱任务页持续刷新当前 crew Todo。 | 无固定周期；目录/进程内事件流。 |
| A15 | roster/白板订阅 | `Sources/Mac/Views/CrewSessionWindowView.swift:74,548-558` | 右栏成员列表首刷并随 backend 白板相关事件刷新 roster。 | 无固定周期；事件流。登录态底层包含 20 s hub ping。 |
| A16 | edge queued-session auto-claim | `Sources/Mac/Views/CrewSessionWindowView.swift:72,949-968` | 原设计每 4 s 拉 edge queued session、claim 后本机运行。 | 代码有 4 s 循环，但 `edgeQueueBindingReady` 在 `Sources/Mac/Views/CrewSessionWindowView.swift:952` 恒为 `false`，当前实际不启动。 |
| A17 | 远端 session durable poll | `Sources/Views/Remote/RemoteSessionsView.swift:85,268-273` | sheet 打开期间拉 session 列表、事件与待交互项，作为可靠 transcript 数据源。 | 3 s `Task` 轮询。 |
| A18 | `SessionProxyClient` viewer | `Sources/Views/Remote/RemoteSessionsView.swift:86,319-341` | 选中远端 session 时新建 viewer WebSocket，消费 live state；切换/消失时取消并关闭。 | 阻塞 receive loop + 20 s ping + 1/2/4…30 s 重连退避。 |
| A19 | `CrewMailboxWaker` | 登录态 session 由 `Sources/Mac/Views/CrewSessionWindowView.swift:882,1013,1027-1031` 接线；runner 在 `Sources/Mac/Services/CrewSessionRunner.swift:978-986` 懒建/持有 | 每 crew 共用一个 realtime hub，事件到达或 run 转 idle 时拉各 session inbox，确认注入成功才 mark-delivered。 | 无业务轮询；底层 `CrewRealtimeClient` 有阻塞 receive、20 s ping、重连退避，投递另有 B20 回执采样。 |
| A20 | `SessionPermissionRelay` + runner-role `SessionProxyClient` | 登录态 session 由 `Sources/Mac/Views/CrewSessionWindowView.swift:883,1016,1035-1039` 接线；runner 在 `Sources/Mac/Services/CrewSessionRunner.swift:235-248` 懒建/持有 | 在本地 approval store、远端 WS permission frame 和 HTTP mirror 之间同步审批状态。 | 事件订阅 + 底层 20 s WS ping；另有 20 s permission retry（B19）。 |
| A21 | QR 登录 `pollTask` | `Sources/Views/CrewQRLoginView.swift:66,127-159` | 请求 challenge 后持续查询批准/拒绝/过期，视图消失取消。 | 2 s，最多 4 min。 |
| A22 | iOS Welcome 重发倒计时 | `Sources/Views/CrewWelcomeView.swift:26,87-89` | OTP 重发冷却每秒 tick。 | `Timer.publish` 1 s，视图存在即 autoconnect。 |
| A23 | macOS Welcome 重发倒计时 | `Sources/Mac/Views/CrewMacWelcomeView.swift:30,84-86` | OTP 重发冷却每秒 tick。 | `Timer.publish` 1 s，视图存在即 autoconnect。 |

其余 `@StateObject` 已逐一排除：`AppModel`/`CaptainTemplateStore`（`Sources/PendingCrewApp.swift:7,11`）只有状态/持久化，没有自主循环；两个 `CrewWelcomeViewModel`（`Sources/Views/CrewWelcomeView.swift:20`、`Sources/Mac/Views/CrewMacWelcomeView.swift:24`）只有用户动作触发的有限请求；两个 `CrewDragState`（`Sources/Mac/Views/CrewDAGTreeView.swift:26`、`Sources/Mac/Views/CrewTimelineListView.swift:23`）只有拖拽期间的短轮询（已列 B28），不属于常驻后台职责。其余 `.task`/`.onAppear` 只是一次 refresh/load/render，也没有列成后台所有者。本文盘点时存在的 `WorkspaceSyncStore` 已在 Todo #78 中随跨机 Workspace 同步整层删除。

## 清单 B — 所有定时器与轮询

| # | 文件:行号 | 周期/上限 | 做什么 | 谁持有 |
|---:|---|---|---|---|
| B1 | `Sources/Mac/Services/CrewRelayAgent.swift:56-65` | 5 s，重复 | relay pull/push 兜底、hub 连接对账、远端任务请求处理。 | `MacThreePaneView` 的 `@StateObject relayAgent`。 |
| B2 | `Sources/Mac/Services/CrewSessionRunner.swift:435-440` | 到 `fireAt` 的一次性 Timer（最少延迟 1 s） | 持久化 `schedule_wakeup` 到点后清账并注入目标 run。 | `CrewSessionRunner.wakeupTimers[id]`。 |
| B3 | `Sources/Mac/Services/CrewSessionRunner.swift:608-612` | 2 s，重复 | 重算 awaiting-reply/health roster 并原子写 `crew-sessions.json`。 | `CrewSessionRunner.snapshotTimer`。 |
| B4 | `Sources/Mac/Services/QuotaCenter.swift:53-58` | 600 s，重复；启动立即一次 | 实探额度并刷新/写 `quota.json`。 | `QuotaCenter.shared.timer`。 |
| B5 | `Sources/Mac/Services/ModelCatalogCenter.swift:37-42` | 6 h，重复；启动立即一次 | 实探模型/effort 并刷新/写 `models.json`。 | `ModelCatalogCenter.shared.timer`。 |
| B6 | `Sources/Mac/Services/LocalAgentUsageMonitor.swift:21-30` | 60 s，重复 | 重扫 Claude/Codex 当日 token 使用量。 | `LocalAgentUsageMonitor.pollTask`（侧栏 `@StateObject`）。 |
| B7 | `Sources/Mac/LocalRunner/AgentTerminalSession.swift:359-368` | 0.6 s，重复 | 从 `lastOutputAt` 重算 working/typing，轮询待决策消失，并同步滚动几何。 | 每个 `AgentTerminalSession.busyTimer`。 |
| B8 | `Sources/Mac/LocalRunner/AgentTerminalSession.swift:414-435`；周期常量 `Sources/Mac/LocalRunner/SessionLaunchProbe.swift:40` | 1 s，直到 alive/失败/退出 | Claude PTY 拉起看门狗，防 fork/秒退/零输出被误报 idle。 | 每个 `AgentTerminalSession.launchWatchdog`。 |
| B9 | `Sources/Mac/LocalRunner/CodexAppServer/CodexAppServerBackend.swift:184-204`；周期常量 `Sources/Mac/LocalRunner/SessionLaunchProbe.swift:40` | 1 s，直到 alive/失败/退出 | Codex app-server 拉起看门狗。 | 每个 `CodexAppServerBackend.launchWatchdog`。 |
| B10 | `Sources/Views/Remote/RemoteSessionsView.swift:268-273` | 3 s，视图存在期间 | 拉远端 session、event transcript 与 interaction。 | `RemoteSessionsView .task`。 |
| B11 | `Sources/Views/CrewQRLoginView.swift:145-159,216` | 2 s，最多 4 min | 查询扫码登录 challenge 状态。 | `CrewQRLoginView.pollTask`。 |
| B12 | `Sources/Mac/Views/CrewSessionWindowView.swift:954-968` | 4 s，重复 | edge queued-session auto-claim。 | `CrewSessionWindowView .task`；当前被 `edgeQueueBindingReady == false` 禁用。 |
| B13 | `Sources/Remote/SessionProxyClient.swift:157-173` | 无固定周期，`receive()` 阻塞 | 收 session proxy WS 帧。 | 每个 `SessionProxyClient.receiveLoop`。 |
| B14 | `Sources/Remote/SessionProxyClient.swift:178-193`；常量 `Sources/Remote/SessionProxyClient.swift:62` | 20 s，重复 | session proxy WS ping/断线检测。 | 每个 `SessionProxyClient.pingLoop`。 |
| B15 | `Sources/Remote/SessionProxyClient.swift:232-250` | 失败后 1/2/4/8/16/30 s，封顶 30 s | session proxy 重连；再次断线会继续形成循环。 | `SessionProxyClient` 的重连 `Task`。 |
| B16 | `Sources/Services/CrewRealtimeClient.swift:93-108` | 无固定周期，`receive()` 阻塞 | 收 crew realtime hub WS 帧。 | 每个 `CrewRealtimeClient.receiveLoop`。 |
| B17 | `Sources/Services/CrewRealtimeClient.swift:112-127`；常量 `Sources/Services/CrewRealtimeClient.swift:45` | 20 s，重复 | crew realtime hub ping/断线检测。 | 每个 `CrewRealtimeClient.pingLoop`。 |
| B18 | `Sources/Services/CrewRealtimeClient.swift:144-162` | 失败后 1/2/4/8/16/30 s，封顶 30 s | crew hub 重连；再次断线会继续形成循环。 | `CrewRealtimeClient` 的重连 `Task`。 |
| B19 | `Sources/Mac/LocalRunner/SessionPermissionRelay.swift:184-195`；常量 `Sources/Mac/LocalRunner/SessionPermissionRelay.swift:150` | 20 s，重复 | 对未完成 permission mirror 做慢速 retry/reconcile。 | 每个 `SessionPermissionRelay.retryTask`，由 runner 字典持有。 |
| B20 | `Sources/Mac/Services/CrewSessionRunner.swift:744-766`；参数 `Sources/Mac/LocalRunner/CrewMailboxWakeLogic.swift:84-85` | 先等 2 s，再每 1 s，最多 10 s | 注入 wake 后采样 busy/working，确认真送达后才推进游标/receipt。 | 每次投递创建的 `Task`（`CrewSessionRunner.confirmWake`）。 |
| B21 | `Sources/Mac/LocalRunner/AgentTerminalSession.swift:576-610`；参数 `Sources/Mac/LocalRunner/SessionProfileSwitch.swift:43-56` | 0.25 s；等 idle 单次最多 900 s，回显单次最多 8 s，最多 3 次 | 等 Claude 空闲后注入 `/model`/`/effort` 并核验回显。 | 调用 `AgentTerminalSession.applyProfileSwitch` 的任务。 |
| B22 | `Sources/Mcp/McpPermissionHook.swift:94-112` | 0.5 s，最多 1 h | Claude permission hook 等 app/human 对共享 approval 项 allow/deny。 | `pendingcrew --mcp-permission-hook` helper 进程当前调用栈。 |
| B23 | `Sources/Mcp/McpServer.swift:973-992` | 0.25 s，默认 40 次≈10 s；`change_workdir` 调用显式放大到约 120 s（`Sources/Mcp/McpServer.swift:635`） | 等 app 写 `.crewresp.json`。 | MCP helper 的同步工具调用栈。 |
| B24 | `Sources/Mcp/McpServer.swift:997-1007` | 0.5 s，最多 3600 次≈30 min | `ask` 等共享 approval reply。 | MCP helper 的 `awaitReply` 调用栈。 |
| B25 | `Sources/Stores/LocalApprovalStore.swift:199-246` | 默认 0.5 s，最多 3600 次≈30 min | Codex manual-review provider 等共享 approval decision；超时主动关陈旧卡。 | app 内 `CodexAppServerBackend` 注入的 provider 调用。 |
| B26 | `Sources/Mac/Services/QuotaCenter.swift:120-126,168-170` | Claude process 每 0.2 s、最多 30 s；Codex process 另有一次性 20 s watchdog | 防额度探测子进程挂死。 | 一次 `QuotaCenter.refresh` 的后台 probe。 |
| B27 | `Sources/Mac/Services/ModelCatalogCenter.swift:142-146,171-173` | Claude process 每 0.2 s、最多 30 s；Codex process 另有一次性 20 s watchdog | 防模型探测子进程挂死。 | 一次 `ModelCatalogCenter.refresh` 的后台 probe。 |
| B28 | `Sources/Mac/Views/CrewDAGTreeView.swift:101-106` | 0.15 s，按住鼠标期间 | 监测拖拽按键释放，避免拖拽状态卡住。 | `CrewDragState.releaseWatch`。 |
| B29 | `Sources/Views/CrewWelcomeView.swift:26,87-89` | 1 s，重复 | iOS OTP 重发冷却倒计时。 | `CrewWelcomeView` 的 autoconnect publisher。 |
| B30 | `Sources/Mac/Views/CrewMacWelcomeView.swift:30,84-86` | 1 s，重复 | macOS OTP 重发冷却倒计时。 | `CrewMacWelcomeView` 的 autoconnect publisher。 |

补查结论：全仓没有 `DispatchSourceTimer`；`DispatchQueue.*asyncAfter` 也没有自递归循环。现有 `asyncAfter` 都是一次性——目录事件 250 ms 合流（`Sources/Stores/LocalWhiteboardStore.swift:110`）、终端滚动条 1.5 s 隐藏（`Sources/Mac/Views/TerminalScrollbarOverlay.swift:93`）、窗口 chrome 0.3 s 重设（`Sources/Mac/Views/MacRootView.swift:319`）以及两个 20 s 子进程 watchdog（已并入 B26/B27）。同样没有把普通的有限重试、UI 动画、一次性 `Task.sleep` 冒充长期轮询。

## 清单 C — UI 层对终端对象的直接依赖

| # | 分类 | 文件:行号 | 直接依赖与数据方向 |
|---:|---|---|---|
| C1 | 只是显示 | `Sources/Mac/Views/CrewSessionWindowView.swift:1150-1165` | 从 `CrewSessionRun.terminalView` 取真 PTY 视图，交给 `AgentTerminalView`；agent 终端再叠外置滚动条，plain terminal 不叠。 |
| C2 | 只是显示 | `Sources/Mac/Views/AgentTerminalView.swift:4,6-16,34-52` | 唯一直接 `import SwiftTerm` 的 View；`NSViewRepresentable` 原样返回同一个 `ActivityTerminalView`，设置字体、压缩优先级和主题。不是终端状态的只读 DTO。 |
| C3 | 只是显示 | `Sources/Mac/Services/CrewSessionRunner.swift:1672-1682` | `CrewSessionRun` 直接 downcast `AgentTerminalSession`/`PlainTerminalSession` 暴露 `ActivityTerminalView`，并给 overlay 暴露 agent session 本体。 |
| C4 | 只是显示 | `Sources/Mac/LocalRunner/AgentTerminalSession.swift:5,13,156-164,265-266` | `import SwiftTerm`，构造并终身持有 `ActivityTerminalView: LocalProcessTerminalView`；进程退出还直接收窄它的 scrollback。 |
| C5 | 只是显示 | `Sources/Mac/LocalRunner/PlainTerminalSession.swift:5,15-21,59-61` | `import SwiftTerm`，同样构造/持有 `ActivityTerminalView`，但切回 SwiftTerm 原生 scroller。 |
| C6 | 读画面内容 | `Sources/Mac/Services/CrewSessionRunner.swift:784-824,827-881` | `inspect_session` 的关键路径：`applySessionOp` → `inspect`，对 PTY downcast 后调用 `terminalTail`；后者 `view.getTerminal()`，按 `0..<terminal.rows` 调 `getLine(row:)`，`translateToString(trimRight:)`，去尾部空行后只回最后 40 行。它读的是 **当前 yDisp 可见屏幕**；用户手动上滚时，机长读到上滚处，不是必然最新尾部。 |
| C7 | 读画面内容 | `Sources/Mac/LocalRunner/AgentTerminalSession.swift:116-125,269-305`；`Sources/Mac/Services/CrewSessionRunner.swift:1871-1882` | 覆盖 `dataReceived` 并给 `terminalView.onData` 挂旁路，直接读 PTY 字节做 health、rate-limit 菜单、待决策、typing、profile/launch 参数扫描；runner 还 downcast 该具体类型订阅解析出的启动参数问题。不是通过后端中立事件。 |
| C8 | 写入字节 | 创建点 `Sources/Mac/Services/CrewSessionRunner.swift:1110-1115`；实现 `Sources/Mac/LocalRunner/AgentTerminalSession.swift:337,548-557,614,666-676` | runner 直接构造具体 PTY backend；`startProcess` 启 PTY，`send`、`interrupt`、`sendRaw` 最终都到 `terminalView.process?.send(data:)`，`stop` 直接 `terminalView.terminate()`。 |
| C9 | 写入字节 | `Sources/Mac/Services/CrewSessionRunner.swift:901-930` | `nudge_session` 对 `AgentTerminalSession` downcast：Enter=`0x0d`、Esc=`0x1b`，其它文本调 `term.send`；因此 session 操作接口仍知道 PTY 按键语义。 |
| C10 | 写入字节 | `Sources/Mac/LocalRunner/PlainTerminalSession.swift:72-78,84-97` | plain shell 直接 `startProcess`；程序化 send/Ctrl-C 写 `process.send`，stop 调 `terminalView.terminate()`。 |
| C11 | 读几何（行列/滚动） | `Sources/Mac/LocalRunner/AgentTerminalSession.swift:48-53,63-68,80-91` | `ActivityTerminalView` 直接读 `getTerminal().rows/options.scrollback`、改 history，并拦截零尺寸 `setFrameSize`，防 2 列 reflow 截断历史。 |
| C12 | 读几何（行列/滚动） | `Sources/Mac/LocalRunner/AgentTerminalSession.swift:248-255,353-365` | SwiftTerm delegate 的 `sizeChanged(newCols:newRows:)` 与 `onScroll` 回调直接驱动 typing 宽限和滚动状态更新。 |
| C13 | 读几何（行列/滚动） | `Sources/Mac/LocalRunner/AgentTerminalSession.swift:501-513` | 直接读 `canScroll`、`scrollPosition`、`scrollThumbsize` 生成 UI `ScrollState`；拖动反向调用 `terminalView.scroll(toPosition:)`。 |
| C14 | 读几何（行列/滚动） | `Sources/Mac/Views/TerminalScrollbarOverlay.swift:8-13,35-58,82-93` | UI 观察整个 `AgentTerminalSession`，用其 `scrollState` 算 knob，并直接回调 `scrollTerminal`；还自行持有 1.5 s 隐藏 work item。 |

仅注释/设计说明提到这些类型、没有运行时依赖的文件还有：`Sources/Mac/LocalRunner/LocalRunnerPlaceholder.swift:6-15`、`Sources/Mac/LocalRunner/SessionLaunchProbe.swift:32`、`Sources/Mac/LocalRunner/LocalCodingAgentSpec.swift:6`、`Sources/Mac/LocalRunner/TerminatedScrollbackPlan.swift:9-38`。全仓 `import SwiftTerm` 只有 C2、C4、C5 三处。

## 清单 D — 跨进程共享文件的当前写入方

“app”包括 GUI 进程内的 Codex app-server backend；“helper”是同一二进制以 `--mcp-serve` / hook 参数 re-exec 的短命子进程（入口 `Sources/Mcp/McpHelperMain.swift:3-54`）。`MultiProcessJSONStore.withFileLock` 的确调用 POSIX `flock(LOCK_EX/LOCK_UN)`，见 `Sources/Stores/MultiProcessJSONStore.swift:29-40`；下表的“有 flock”必须是具体 store 的 public read-modify-write 真正走到该入口，不能因为用了 `.atomic` 就算有锁。

| # | Store / 文件名后缀 | app 进程写入方（文件:行号） | MCP helper 子进程写入方（文件:行号） | flock |
|---:|---|---|---|---|
| D1 | `LocalWhiteboardStore`：`<crewId>.json`（定义 `Sources/Stores/LocalWhiteboardStore.swift:286-296`） | 人类发言 `Sources/Services/PendingCrewBackend.swift:416`；relay 落地 `Sources/Mac/Services/CrewRelayAgent.swift:162`；runner 的 session/system/唤醒/健康/决策回执 `Sources/Mac/Services/CrewSessionRunner.swift:292,300,347,357,409,463,730,769,799,965,1013,1153,1315,1350,1545,1824,1875,1899,1909`；crew 编排 `Sources/Stores/CrewStore.swift:524,579,783,800`；其它 app 服务/视图 `Sources/Mac/Views/MacRootView.swift:203`、`Sources/Mac/Views/CrewTodoFollowUp.swift:41,47,71`、`Sources/Mac/Services/CrewLocalMentionWaker.swift:228`、`Sources/Mac/Services/WorkdirChangeCommand.swift:57`。 | `post_to_crew`/组织联系等 `Sources/Mcp/McpServer.swift:417,473,911,927,1102`；permission hook 通知 `Sources/Mcp/McpPermissionHook.swift:59,71,82`；turn hook 兜底发言 `Sources/Mcp/McpTurnHook.swift:54`。Todo/approval/control 的事故报告也会在**当前调用进程**回写白板，见 `Sources/Stores/LocalTodoStore.swift:244-248`、`Sources/Stores/LocalApprovalStore.swift:144-151`、`Sources/Stores/LocalCrewControlStore.swift:368-376`，所以这些既可能是 app，也可能是 helper。 | **有**：`<crewId>.lock`，整个严格 read-modify-write 在锁内（`Sources/Stores/LocalWhiteboardStore.swift:291-296,422-450`）。 |
| D2 | `LocalTodoStore`：`<crewId>.todos.json`（`Sources/Stores/LocalTodoStore.swift:221-230`） | 新增 `Sources/Mac/Services/CrewLocalTodoLanding.swift:38`；追问/重开 `Sources/Mac/Views/CrewTodoFollowUp.swift:25-27`；编辑/软删 `Sources/Mac/Views/CrewTodoDetailWindow.swift:372,383`。 | `respond_todo` 追加回应 `Sources/Mcp/McpServer.swift:733`。 | **有**：`<crewId>.todos.lock`，见 `Sources/Stores/LocalTodoStore.swift:225-230,251-265`。 |
| D3 | `LocalApprovalStore`：`<crewId>.approvals.json`（`Sources/Stores/LocalApprovalStore.swift:121-131`） | UI 回答/审批 `Sources/Mac/Views/Chat/SessionApprovalCardsView.swift:111,116`；远端 permission mirror 决定 `Sources/Mac/LocalRunner/SessionPermissionRelay.swift:254`；runner 在 `Sources/Mac/Services/CrewSessionRunner.swift:1291` 注入的 Codex manual provider 会在 app 内 raise/超时 deny（实现 `Sources/Stores/LocalApprovalStore.swift:207-246`）。 | permission hook raise `Sources/Mcp/McpPermissionHook.swift:53`；MCP `ask` raise 与 captain answer `Sources/Mcp/McpServer.swift:450,508`。 | **有**：`<crewId>.approvals.lock`，见 `Sources/Stores/LocalApprovalStore.swift:127-131,153-174`。 |
| D4 | `LocalCrewControlStore`：`<crewId>.crewmeta.json`、`<crewId>.crewattention.json`、`<crewId>.<uuid>.crewcmd.json`、`<crewId>.<commandId>.crewresp.json`（`Sources/Stores/LocalCrewControlStore.swift:383-397`） | 排空并删除 rename/attention/command 文件：`Sources/Stores/CrewStore.swift:379,391,406`；写 inspect/nudge/stop 回应 `Sources/Mac/Services/CrewSessionRunner.swift:786`，写 change-workdir 回应 `Sources/Mac/Views/MacRootView.swift:168`。 | 写 rename/attention `Sources/Mcp/McpServer.swift:522,534,540`；写所有命令队列 `Sources/Mcp/McpServer.swift:572,589,599,611,629,690,697,710,763,794,804,811,818,826,833,841`；long-poll 成功时读取并删除 response `Sources/Mcp/McpServer.swift:987`。 | **无**，且是有意设计：meta/attention 原子整写 LWW；命令/响应一条一个原子文件，drain/take 后删，不做共享 RMW（`Sources/Stores/LocalCrewControlStore.swift:15-17,260-271,326-359`）。 |
| D5 | `LocalAgentSessionStore`：`agent-sessions.json`（`Sources/Stores/LocalAgentSessionStore.swift:75-83`） | runner 捕获 Claude session UUID/Codex thread id 时 record：`Sources/Mac/Services/CrewSessionRunner.swift:1106,1147`。 | 无（helper 不写；只会通过控制命令间接让 app 起/迁 session）。 | **有**：`agent-sessions.lock`。 |
| D6 | `LocalWakeupStore`：`wakeups.json`（`Sources/Stores/LocalWakeupStore.swift:65-73`） | runner 接到已排空的 `schedule_wakeup` 后 register，到点 remove：`Sources/Mac/Services/CrewSessionRunner.swift:392,425,446`。 | **不直接写**；helper 只写 D4 的 `.crewcmd.json`（`Sources/Mcp/McpServer.swift:690`）。 | **有**：`wakeups.lock`。 |
| D7 | `CrewSessionsSnapshot`：`crew-sessions.json`（`Sources/Stores/CrewSessionsSnapshot.swift:1-8,34`） | runner 每 2 s 组装并原子写：`Sources/Mac/Services/CrewSessionRunner.swift:673-705`。 | 无写入；只读用于 `list_sessions`/directory/hook：`Sources/Mcp/McpServer.swift:429,784-786`、`Sources/Mcp/HookEmitter.swift:246-248`。 | **无**；单 app writer + `.atomic`。 |
| D8 | `LocalCrewStore`：`local-crews.json`（`Sources/Stores/LocalCrewStore.swift:21-22,71`） | 所有直接 mutation 最终 `persistToDisk`：`Sources/Stores/LocalCrewStore.swift:93-168,227-249,343-372,429-466,501-550,661-668`；调用集中在 local backend、`CrewStore`、relay、runner、workdir 与 inspector（如 `Sources/Services/PendingCrewBackend.swift:307-318`、`Sources/Stores/CrewStore.swift:233,382,393,615,651,701,733`、`Sources/Mac/Services/CrewRelayAgent.swift:177,240,335,353`）。 | 无直接写；helper 只读组织树/directory，变更组织关系时写 D4 命令。读取入口见 `Sources/Stores/CrewDirectory.swift:114-129`。 | **无**；`@MainActor` 单 app writer + `.atomic`，helper 并发只读。 |
| D9 | `SessionUnreadStore`：**没有 JSON 文件**，是 UserDefaults key `PendingCrew.sessionLastViewed`（`Sources/Stores/SessionUnreadStore.swift:12-16`） | 切到 run 时 `markViewed`：`Sources/Mac/Views/CrewSessionWindowView.swift:76-79`；写入 `Sources/Stores/SessionUnreadStore.swift:28-31,53-55`。 | 无。 | **无**；它不是跨进程 store。把它列在这里是为了纠正“SessionUnreadStore 也是共享 JSON”的前提，搬后台时不应照搬。 |
| D10 | `QuotaCenter` 邻接快照：`quota.json`（`Sources/Mac/Services/QuotaCenter.swift:35`） | `QuotaCenter.refresh` 后原子写 `Sources/Mac/Services/QuotaCenter.swift:104`。 | 无写入；`get_quota` 读取 `Sources/Mcp/McpServer.swift:638`。 | **无**；单 app writer + `.atomic`。虽然定义不在 `Sources/Stores/`，但确属 app→helper 跨进程共享文件，搬家必须一并定所有者。 |
| D11 | `ModelCatalogCenter` 邻接快照：`models.json`（`Sources/Mac/LocalRunner/AgentModelCatalog.swift:183-184`） | `ModelCatalogCenter.refresh` 后原子写 `Sources/Mac/Services/ModelCatalogCenter.swift:84-87`。 | 无写入；helper 加载 `Sources/Mac/LocalRunner/AgentModelCatalog.swift:403` 后用于 `start_session`/`set_session_profile` 校验。 | **无**；单 app writer + `.atomic`。同样不在 `Sources/Stores/`，但属于实际跨进程契约。 |

跨表共同的删除入口：app 的“清除本机所有数据”会删除整个 PendingCrew Application Support 目录，因而覆盖 D1–D11（以及附件/模板），见 `Sources/Stores/LocalDataReset.swift:19-25`。这不是常态并发写入方，但后台拆分后需要先停后台所有者再执行，否则后台可能立即重建刚删掉的快照/账本。

## 搬迁时最容易踩的三个边界

1. `CrewSessionRunner` 不是单纯“起进程”的类：主视图还把八组 `.onChange` 请求队列、持久 wakeup、2 s roster 快照、listen、inspect/nudge/stop、mailbox/permission relay 和终端内容读取全接在它上面。只把 PTY/app-server 搬走而保留这些 `.onChange`，会形成两个所有者或留下无人 drain 的命令。
2. `inspect_session` 现在读取 SwiftTerm **当前可见区**，且受人的滚动位置影响；后台进程没有这个 UI/yDisp 概念。协议必须明确要“语义 transcript 尾部”还是“用户当前看到的屏幕”，两者并不等价。
3. 共享文件并非一套一致模型：白板/Todo/approval/agent-session/wakeup 是 flock+RMW；crew-control 是无锁的一文件一命令；crew-sessions/quota/models/local-crews 是单 writer 原子快照；SessionUnread 甚至只是 app UserDefaults。不能用一个笼统的“JSON store 迁移”策略处理。
