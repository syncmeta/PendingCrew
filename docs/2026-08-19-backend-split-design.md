# 常驻后台 · 前后端分离 设计文档

- 日期：2026-08-19
- 对应人类 Todo：**#58**（父 crew「PendingCrew」的 Todo 面板）
- 状态：**待评审**（父机长 + 人类过目后才动代码）
- 配套调研清单：`docs/2026-08-19-backend-split-inventory.md`

---

## 1. 要解决的问题

人类原话：

> 每次更新我都得等 session 完成工作，有没有办法重启 PendingCrew 之后能恢复 session 而不用等它？**就像休眠而不是关机**。

选定的做法是**真休眠**：session 不再由 app 进程养，改由一个**常驻后台进程**养。app 退化成「连上去看的那个窗口」。更新 app、重开 app、app 崩溃，都不打断任何 agent —— **连正在进行的那一轮都不断**。

被否掉、不要退回去做的：「关机恢复」（退出时记下谁在跑、重开时重新拉起并告知它被打断）。

### 1.1 硬性验收项

| # | 验收项 | 判定方式 |
|---|---|---|
| A1 | 更新 app 不打断任何在跑的 session | §9 端到端剧本，**三条路径都要验**：①协议同版（常态）②daemon 较旧但兼容 → 新 app 降级运行、session 不断 ③daemon 较旧且不兼容 + 有 session 在跑 → 选「稍后」→ session 不断、等空闲自动换代。只验①不算过（见 §4.4） |
| A2 | 终端手感不退化：选中复制 / 回滚缓冲 / 改宽度重排 | §7 可复现验证 |
| A3 | 不出现双头：同一批账、同一批定时器只有一个所有者 | §6 单一所有权闸门 |
| A4 | 崩溃与孤儿、版本不一致的行为是显式设计的 | §8 |

### 1.2 顺带解掉的结构债

`docs/tech-debt.md`：**PTY 的每一批输出都要过主线程，代价随 session 数线性增长**（不管有没有人在看）。#59 只把单价打下去了（253ms → 0.94ms 等四条），结构没动。

本设计从结构上解掉它：字节读取与状态扫描全部在后台进程的 per-session 队列上；**只有当前被"看着"的那一个 session** 才往 app 主线程送字节。没人看的 session 对 app 主线程的开销是**零**。

### 1.3 明确不做

- **不绑云端分家**（PendingCrew 从 PendingBot 的云端独立、自己的账号自己的服务器）—— 另一条轴。本地这刀做完，云端那半是可选叠加。
- **不做手机远程遥控** —— 但它是这套常驻 runner 的已知下游消费者，协议设计要为它留门（§4.1 传输层可替换）。
- 不动签名、不动公证、不新增可执行文件。

---

## 2. 现状复核（父机长的三条判断，逐条核过）

### 2.1 ✅ 控制 + 状态接口确实抽干净了

`Sources/Mac/LocalRunner/SessionBackend.swift` —— 一个 `@MainActor protocol SessionBackend`，覆盖控制面（`send` / `interrupt` / `stop` / `clearQuotaHealth` / `applyProfileSwitch`）与状态面（`status` / `isBusy` / `isWorking` / `displayIsTyping` / `health` / `pendingDecision` / `kind`），claude（`AgentTerminalSession`）与 codex（`CodexAppServerBackend`）各自实现。

**结论：判断成立。**「搬出去 = 再加第三个实现」这条路是通的。上层编排（`CrewSessionRunner`）只通过这个协议 + `CrewSessionRun` 的 `@Published` 看 session。

**但有一处泄漏必须先补**（§5.1）：`CrewSessionRun` 在协议之外还暴露了 `terminalView: ActivityTerminalView?` 和 `agentTerminalSession`，共 4 个文件、约 23 处引用（`CrewSessionRunner.swift` 7 处、`CrewSessionWindowView.swift` 3 处、`AgentTerminalView.swift` 2 处、`PlainTerminalSession.swift` 11 处）。规模很小，是可控的。

### 2.2 ✅ 数据已经在进程外

`Sources/Stores/` 下的共享 JSON store（白板 / Todo / 审批 / 控制通道 / 账本）本来就要被 MCP helper 子进程和 app 同时读写，已有互斥锁与损坏归档。**这块基本不用动**，只需要改「谁来写」（§6）。

### 2.3 ✅ 同一个二进制已经会以第二身份跑

`Sources/PendingCrewEntry.swift`：argv 带 `--mcp-serve` / `--mcp-hook` / `--mcp-permission-hook` / `--mcp-turn-hook` 时当 helper 跑、不起 GUI。后台常驻就是**第三个身份**：`--daemon`。不新增可执行文件，签名/公证零改动。

### 2.4 ✅ 最难那块比预想的轻——终端库本来就是分好的

父机长说「claude 那条腿的会话对象本体就是一个界面控件」—— 属实（`AgentTerminalSession` 持有 `ActivityTerminalView: LocalProcessTerminalView`，PTY、屏幕、渲染、状态扫描全长在一个 UI 对象上、跑主线程）。

但**我们不需要自己去劈这个终端**。SwiftTerm 1.18 本身就是分层的：

- `Terminal.swift` / `Buffer.swift` / `LocalProcess.swift` —— **纯 Foundation，零 AppKit**。`HeadlessTerminal.swift` 就是官方给的「无画面终端 + 本地进程」组合。
- `Mac/MacTerminalView.swift` —— 只负责画。
- `TerminalView.feed(byteArray:)` 是 **public** 的：一个终端视图可以被字节流喂养，而不必自己开进程。

所以劈的动作是：**后台进程养 `Terminal` + `LocalProcess`（内核），窗口里的 `TerminalView` 只吃字节流（画面）**。这是这个库设计上就支持的用法。

由此 **A2（手感不退化）的风险远低于预估**：选中复制、回滚缓冲、改宽度 reflow 全部由窗口侧那个**原生真终端控件**提供，代码路径一行不改，只是数据来源从「自己 fork 的子进程」换成「后台送来的字节」。

**唯一真正新写的东西**是「缓冲区快照序列化」（§5.3）—— 把后台那份终端缓冲区还原成一段字节流，喂进新连上来的窗口。这是全项目风险最高的一块，单独立阶段 + 属性测试钉死（§7.1）。

### 2.5 ✅ 后台职责挂在界面上——「关掉 app 就全停」的根

已核实的挂载点：

| 长期职责 | 挂在哪 |
|---|---|
| `CrewSessionRunner`（编排本体） | `MacRootView.swift:21` `@StateObject`（**view-local，一个窗口一份**） |
| `CrewRelayAgent`（云端中继，5s 循环） | `MacRootView.swift:26` `@StateObject` |
| `CrewLocalMentionWaker`（点名唤醒器） | `MacRootView.swift:232` 在 view 里 new |
| `LocalAgentUsageMonitor` | `CrewSidebarView.swift:28` `@StateObject` |
| `QuotaCenter` / `ModelCatalogCenter` 轮询 | `CrewSidebarView.swift:65` `.task { quota.start(); … }` |
| 机长派工 / 建 crew / 自动起机长的**编排 glue** | `MacRootView.swift` 的一串 `.onChange` 修饰符里 |

最后一条是最麻烦的：编排逻辑本身长在 SwiftUI 的 `.onChange` 里。**这是 P0 阶段的主要工作量**。

完整清单见 `docs/2026-08-19-backend-split-inventory.md`（23 条界面持有的后台职责、30 条定时器/轮询、逐条带文件行号）。清单又补出三条 P0 必须一并处理的：

- **每 session 的两个后台服务是从视图接线的**：`CrewMailboxWaker`（信箱唤醒）与 `SessionPermissionRelay`（审批中继）由 `CrewSessionWindowView` 在展示 session 时创建、交给 runner 持有。**右栏没打开过那个 session，它们就没被接上** —— 这既是「关掉 app 就全停」的另一半，也是今天就存在的一个隐患。
- `MacRootView` 还直接写了一次控制通道回应（`change_workdir` 的回执）—— 视图在写共享账本，P0 要一并收走。
- `CrewSessionWindowView` 里有一条 4s 的 edge queued-session 轮询，被一个恒 `false` 的开关关着（死代码）。P0 顺手删掉，别把它搬进 daemon。

---

## 3. 总体架构

```
┌────────────────────────────────────────────────────────────┐
│  PendingCrew.app（同一个二进制，身份一：GUI）                  │
│                                                            │
│   窗口 = 观察者 + 遥控器                                      │
│   · TerminalMirrorView（原生 SwiftTerm 视图，只吃字节）        │
│   · RemoteSessionBackend  ← SessionBackend 的第三个实现       │
│   · 不持有任何定时器、不写任何共享账本                          │
└───────────────────────┬────────────────────────────────────┘
                        │  Unix domain socket
                        │  ~/Library/Application Support/PendingCrew/daemon.sock
┌───────────────────────┴────────────────────────────────────┐
│  PendingCrew --daemon（同一个二进制，身份三）                  │
│                                                            │
│   SessionHost（唯一所有者）                                   │
│    ├─ AgentSessionCore ×N                                  │
│    │    · LocalProcess（PTY 属主）                           │
│    │    · Terminal（无画面，权威缓冲区）                        │
│    │    · 全部扫描器：health / pendingDecision / typing /     │
│    │      rateLimitMenu / profileEcho / launchParameter     │
│    ├─ CodexAppServerBackend ×M（本来就没画面，白送）            │
│    ├─ 唤醒器（点名 / 信箱 / 定时 / 额度重置）                   │
│    ├─ 云端中继 CrewRelayAgent                                │
│    ├─ 额度轮询 QuotaCenter / ModelCatalogCenter              │
│    └─ 共享账本的唯一写入方（白板 / Todo / 审批 / 快照）           │
└───────────────────────┬────────────────────────────────────┘
                        │  fork/exec + PTY
              ┌─────────┴─────────┐
              │  claude / codex   │  ← 一直活着，与 app 无关
              └───────────────────┘

（身份二：--mcp-serve 等 helper 子进程，本次不动）
```

一句话：**所有会随时间自己动的东西都在后台；窗口只做「看」和「点」。**

---

## 4. 连接协议

### 4.1 传输：Unix domain socket + 长度前缀二进制分帧

选它而不是 XPC 的三条理由：

1. **可测**：两端可以在同一个进程里用 `socketpair()` 对接，整条协议进单元测试，不需要 launchd、不需要 GUI。
2. **可调试**：`nc -U` 就能看帧。
3. **可延伸**：同一套分帧换个传输就能跑在网络上 —— 手机遥控是已知的下游消费者，我们不为它写代码，但也不给它砌墙。XPC 换不了。

不选它的代价：要自己写握手、心跳、重连。这部分是有限且已知的（§4.5）。

**分帧**：

```
[u32 length][u8 kind][payload…]

kind = 0  控制帧    payload = UTF-8 JSON
kind = 1  终端字节  payload = [u32 handle][原始 PTY 字节]
kind = 2  快照片段  payload = [u32 handle][u32 seq][序列化字节]
```

控制帧走 JSON（好改、好读、字段少）；终端字节走定长头（每秒几千帧，不能有 JSON/base64 开销）。`handle` 是 attach 时分配的 u32，避免热路径上带字符串 session id。

### 4.2 消息集

**app → daemon**

| 消息 | 作用 |
|---|---|
| `hello{protocolVersion, appBuild}` | 握手，见 §8.3 |
| `listSessions{}` | 拿全量 session 摘要（重连后的第一件事） |
| `attach{sessionId, cols, rows}` | 开始看某个 session；daemon 回 handle + 快照 + 后续实时字节 |
| `detach{handle}` | 不看了；daemon 停止推字节（session 照跑） |
| `resize{handle, cols, rows}` | 窗口宽度变了 |
| `input{handle, bytes}` | 键盘/程序化写入（`send` / `interrupt` 都走它） |
| `control{op, …}` | 非终端控制：`stop` / `startSession` / `applyProfileSwitch` / `clearQuotaHealth` / … |
| `ping{}` | 心跳 |

**daemon → app**

| 消息 | 作用 |
|---|---|
| `hello{protocolVersion, daemonBuild, sessionCount, pid}` | 握手回应 |
| `sessions{[SessionSummary]}` | 全量摘要 |
| `attached{sessionId, handle, snapshotFrames}` | attach 回应 |
| `state{sessionId, delta}` | 状态变更（见 §4.3） |
| `data(handle, bytes)` | 实时 PTY 字节（kind=1 帧） |
| `resync{handle}` + 快照帧 | 背压溢出后的重同步（见 §5.4） |
| `event{kind, …}` | 需要 app 弹 UI 的事件（审批卡片等） |
| `pong{}` | 心跳回应 |

### 4.3 状态如何两端同步

**后台是唯一权威**。`AgentSessionCore` 持有全部扫描器，扫描发生在字节到达的那一刻（在后台的 per-session 队列上）。派生状态变化时，daemon 推一条 `state{sessionId, delta}`。

app 侧的 `RemoteSessionBackend` 实现 `SessionBackend`：收到 delta → 更新自己的 `@Published` → 上层编排（如果那时还有的话）和 UI 一行不用改。**这就是父机长预言的「第三个实现」，字面成立。**

同步的字段就是协议里已有的那些：`status` / `isWorking` / `displayIsTyping` / `health` / `pendingDecision` / `launchParameterProblem` / `scrollState`（后两者是 `AgentTerminalSession` 的扩展面）。

**幂等 + 全量兜底**：每条 delta 带一个 per-session 单调递增 `stateSeq`。app 发现跳号 → 发 `listSessions` 拉全量覆盖。不做增量重放，不做冲突合并 —— 单向、后台权威，没有合并的必要。

### 4.4 兼容纪律（R1 —— 这条直接决定 A1 是不是纸面的）

`protocolVersion` 一旦变成「每次发版顺手 +1」的东西，§8.3 那张表就会把「更新要等 session 空闲」原样还给用户 —— 而那正是人类唯一要解决的痛点。所以纪律定死：

| 变更 | 允许 +1？ |
|---|---|
| 新增一种消息 | ❌ 不加。旧端收到未知 `type` **必须忽略**（不断连、不报错） |
| 已有消息新增字段 | ❌ 不加。旧端**必须忽略未知字段**；新端对缺失字段用默认值 |
| 新增能力（如新的 `control.op`） | ❌ 不加。走 `hello` 里的 `capabilities: [String]` 协商；对端没有就降级，不是断连 |
| 改帧头布局、改字段语义、删字段 | ✅ 才 +1 |

配套要求：

- **新 app 必须能在旧 daemon 上降级运行** —— 缺哪个能力就少哪个功能，不是拒连。
- 每个协议消息的编解码单测里带一条「多塞一个未知字段 / 未知 type，行为不变」的用例。**这条测试就是 A1 的护栏**，谁哪天想加字段顺手 +1，先得删掉这条测试，那时会有人问为什么。
- `+1` 需要在 PR 里显式写明为什么不可避免。

### 4.5 断线与重连

- app 侧：socket 断 → 指数退避重连（0.2s 起，上限 5s）→ 重连后 `hello` + `listSessions` + 对当前正在看的那个重新 `attach`（拿新快照，**不是**接着旧字节流 —— 中断期间的字节已丢，只有快照是对的）。
- daemon 侧：viewer 断开 → 所有 handle 作废、停止推字节，**session 照常跑**。不做任何「等 app 回来」的阻塞。
- 心跳：app 每 10s `ping`，30s 无 `pong` 判定断线。daemon 侧对 60s 无任何帧的连接主动关闭（防半开连接堆积）。

---

## 5. 终端劈成两半

### 5.1 先补协议泄漏（P0/P1 的前置）

今天 `CrewSessionRun.terminalView` 把 `ActivityTerminalView`（AppKit）直接漏给了编排层和视图层。搬家前要按用途拆成三个能力：

| 今天的用法 | 拆成 | 搬家后由谁提供 |
|---|---|---|
| 视图里 `AgentTerminalView(terminalView:)` 挂 NSView | `TerminalMirror` | app 侧（原生视图，吃字节流） |
| `terminalTail()` 读画面成文本（`inspect_session`） | `screenText(maxLines:)` | **daemon**（读它那份权威 `Terminal`，只用 `getLine` / `translateToString`，**这两个 API 在无画面的 Terminal 上一模一样**） |
| 写字节（`send` / `interrupt`） | 已在 `SessionBackend` 里 | daemon |
| 读几何（行列数、滚动位置，外置滚动条用） | `TerminalGeometry` | app 侧（本地视图自己就有） |

**顺带定死一个今天含糊的语义**：`inspect_session` 现在读的是 SwiftTerm 的**当前可见区**（按 `yDisp`）—— 也就是说**人把那个终端往上滚，机长看到的文本就跟着变**。后台进程没有「用户滚到哪」这个概念，所以搬家时必须选一个：

> **选「权威屏幕」**：daemon 那份 `Terminal` 当前的屏幕内容（相当于永远贴底），与任何窗口的滚动位置无关。

理由：机长要的是「它现在卡在哪一屏」，不是「人正在看哪一屏」。今天那个行为是实现细节漏出来的，不是设计。改完之后同一个 `sessionId` 在任何时刻问到的都是同一份画面 —— 严格变好，且可复现。

### 5.2 劈开后的两半

```
后台：AgentSessionCore（Foundation only，无 AppKit，不在主线程）
  LocalProcess ──PTY字节──> Terminal（权威缓冲区，scrollback 10000 行）
                    │
                    ├─> SessionHealthScanner
                    ├─> RateLimitMenuScanner（命中即自动应答）
                    ├─> PendingDecisionTracker
                    ├─> TypingActivityTracker（+ AnsiPlainTextTail）
                    ├─> SessionProfileEchoScanner
                    ├─> SessionLaunchParameterScanner
                    └─> 有人 attach 时：转发给 app

窗口：TerminalMirrorView（AppKit：SwiftTerm TerminalView 子类）
  收到字节 ──> feed(byteArray:) ──> 自己的 Terminal ──> 画
  键盘输入 ──> input 帧 ──> daemon ──> PTY
  选中复制 / 上滚 / reflow：**全部本地原生，零改动**
```

两侧各有一份 `Terminal` 实例，喂同一批字节，因此**天然一致**。这是刻意的冗余：后台那份为了状态权威与断线时不丢历史，窗口那份为了原生手感。

### 5.3 缓冲区快照（全项目最高风险的一块）

**问题**：app 是在 session 跑了两小时之后才连上来的。要让窗口里出现完整的历史（含 10000 行回滚缓冲），必须把 daemon 那份 `Terminal` 的缓冲区**还原成一段字节流**喂给窗口。

**为什么不能用「保留原始字节的环形缓冲区、重放」**：agent 的 TUI 靠整屏重绘刷新，几分钟就能刷掉几 MB，而这些字节里绝大部分是覆盖同一块屏幕的重绘 —— 几 MB 的原始字节换不来 10000 行历史。而且环形缓冲区的头部会切在转义序列中间，重放直接乱码。**必须序列化缓冲区本身。**

**做法**：`TerminalSnapshotEncoder`（新写，约 150 行）

1. `\ec`（硬复位）+ 设定尺寸
2. 回滚缓冲区逐行输出：按 `Attribute` 变化点插 SGR，行尾 `\r\n`
3. 主屏幕内容同上
4. 若当时在 alt-screen：`\e[?1049h` + alt 屏内容
5. 光标位置 `\e[r;cH`、光标可见性
6. 需要保留的 DEC 模式：括号粘贴 `?2004`、应用光标键 `?1`、鼠标上报 `?1000/?1002/?1003/?1006`

SwiftTerm 的 `Attribute` 字段（`fg` / `bg` / `style` / `underlineStyle` / `underlineColor`）都是 public，但它内部的 `toSgr()` 是 internal —— 所以这 40 行 SGR 生成我们自己写。**风险已知，用 §7.1 的往返属性测试钉死。**

分片传输：快照可能几 MB，切成 64KB 的 `kind=2` 帧带 `seq`，末帧后才开始推实时字节。daemon 在自己的 session 队列里**原子地**「序列化 + 切到推流」，保证快照与后续字节之间不丢不重。

### 5.4 背压：宁可重同步，绝不丢字节

agent TUI 可以瞬间打出很多字节。如果 app 消费不过来：

- daemon 每个 attach 维护一个有上限的待发队列（默认 2 MB）。
- 未溢出 → 原样转发。
- **一旦溢出 → 立刻丢弃整个待发队列，标记 dirty，发 `resync`，重发快照。**

关键：**绝不允许「丢掉中间一段字节继续发后面的」** —— 转义序列被拦腰截断会让窗口那份 `Terminal` 永久错乱。要么完整的字节流，要么一份干净的快照重来。后台那份 `Terminal` 从不丢字节，所以快照永远是对的。

（同一条规则也覆盖 A2 的隐藏风险：手感不退化的前提是窗口那份缓冲区始终正确。）

### 5.5 窗口尺寸的归属

daemon 是 PTY 属主，所以 `TIOCSWINSZ` 只能由它发。

- 有窗口在看 → 以窗口视口为准（`attach` / `resize` 帧带 cols/rows），daemon 同步 resize 自己那份 `Terminal` + PTY。
- 没有窗口在看 → **保持最后一次的尺寸**，绝不 resize 成 0。
- 已有的「零尺寸不下传」守卫（`ActivityTerminalView.isRealLayout`，SwiftUI 重挂视图时必经 `.zero`，会把历史按 2 列重排炸掉）保留在 app 侧 —— 那条防线仍然必要，只是现在它拦的是「要不要发 resize 帧」。

---

## 6. 不许出现双头

### 6.1 唯一所有权

要守的不变量是「**只有一个长期编排者**」，**不是**「只有一个进程碰这些文件」（R5）。这个区别很重要：共享账本从设计之初就是多进程共享的（带 flock、带损坏归档），**MCP helper 子进程照旧直写，这条不变、也不该变** —— 后来人别按字面去禁 helper 直写，那会把工具服务器改坏。

危险的从来不是「多个进程写同一个文件」，而是「**两个长期存活的进程各自跑一套定时器、各自按自己的世界观往账上写**」。helper 是每次工具调用起、跑完就退的短命进程，不构成第二个编排者。

| 资源 | 唯一所有者 |
|---|---|
| agent 子进程 + PTY | daemon |
| 所有唤醒器、中继、额度/模型轮询（**所有长期定时器**） | daemon |
| 编排（派工、建 crew、起机长、拍板转发、快照落盘） | daemon |
| 共享账本的**编排性写入**（唤醒回执、状态快照、系统通告） | daemon |
| 共享账本的**工具性写入**（`post_to_crew` / `respond_todo` 等） | MCP helper 短命子进程，**照旧直写，不变** |
| 界面状态（选中了哪个 crew、草稿、滚动位置） | app |

app 对共享账本**只读**，用于显示。

**共享文件不是一套模型，别当成一次「JSON store 迁移」**（调研清单 D 的结论）：

| 模型 | 谁 | 搬家时怎么处理 |
|---|---|---|
| flock + read-modify-write | 白板 / Todo / 审批 / agent-sessions / wakeups | 天生多进程安全，**不用动** |
| 无锁、一文件一命令、drain 后删 | crew-control（`.crewcmd.json` / `.crewresp.json`） | 不用动；但**排空方**要从 app 搬到 daemon（否则 helper 的命令没人接） |
| 单 writer + 原子整写 | `crew-sessions.json` / `quota.json` / `models.json` / `local-crews.json` | **必须换 writer**：从 app 换成 daemon。这四个是「单 writer」假设，两个进程同时写就是无声的互相覆盖 —— 双头在这里最致命 |
| UserDefaults（**不是**跨进程 store） | `SessionUnreadStore`（上次看到哪儿） | **留在 app**。它记的是「用户看到哪儿了」，本来就是界面状态，不该搬 |

另有一条顺序约束：「清除本机所有数据」会删掉整个 Application Support 目录（`LocalDataReset`）。**分家后必须先停 daemon 再删**，否则 daemon 会立刻把刚删掉的快照重新写回来。这条要在 P4 落地时一并处理。

### 6.2 三道闸门（不靠自觉）

1. **进程角色**：引入 `ProcessRole.current`（`.orchestrator` / `.viewer` / `.helper`，进程启动时由 argv + 总闸一次性算出、之后只读）。每个长期服务的 `start()` 第一行 `precondition(ProcessRole.current == .orchestrator)`。**在 viewer 里误起一个唤醒器 = 当场崩，不是悄悄跑起来。**<br>映射：`--daemon` → `.orchestrator`；GUI 且总闸=`daemon` → `.viewer`；GUI 且总闸=`inproc` → `.orchestrator`（那时它确实是所有者）；`--mcp-*` → `.helper`。
2. **单实例锁**：daemon 启动时对 `daemon.lock` 取排他 `flock`，拿不到就立刻退出。保证任何时刻只有一个 daemon。
3. **迁移期总闸**：`PENDINGCREW_BACKEND=inproc|daemon`（默认 `inproc` 直到 A1–A4 全绿）。`inproc` 时 `ProcessRole.current` 在 GUI 进程里被显式设成 `.daemon`（因为那时它确实是所有者）—— 于是**任何一个时刻只有一种所有权，闸门 1 在两种模式下都成立**。

---

## 7. A2 怎么验（不许驱动图形界面，所以全部要能自动跑）

### 7.1 快照往返属性测试（最关键的一条）

```
随机 ANSI 语料 → 无画面 Terminal T1
                   ↓ TerminalSnapshotEncoder
                 字节流 → 干净 Terminal T2
断言：T1 与 T2 的 主屏 + alt 屏 + 回滚缓冲区 + 光标 + 模式 逐格相等
```

语料分两组，**两组都要过**：

1. **合成语料**：SGR 全家（含 256 色 / 真彩 / 下划线样式）、CJK 宽字符、组合字符、滚动区域（DECSTBM）、alt-screen 进出、清屏、绝对/相对光标移动、超出 scrollback 上限的溢出。负责覆盖面。
2. **真 · agent TUI 录制**（fixtures，claude 与 codex 各一段）：把真实 session 的原始 PTY 字节流录下来存进 `Tests/Fixtures/`。随机语料测不出真实 TUI 的怪癖，而我们要还原的**恰恰就是它**。录制方式：P1 阶段在 `AgentSessionCore` 上挂一个临时的字节转储开关，跑一段真 session（含一次撞额度菜单、一次待决策菜单、一次窗口改宽），存盘后关掉开关。

### 7.2 reflow 一致性（对应「改窗口宽度重排」）

reflow 逻辑在 `Buffer.resize`，**在无画面 Terminal 上一模一样**，所以可以 headless 验：

```
T1 喂语料 → 快照 → T2
T1.resize(60) 与 T2.resize(60) 逐格相等
再 T1.resize(160) 与 T2.resize(160) 逐格相等
```

### 7.3 回滚缓冲区长度

喂 12000 行 → 断言还原后的 T2 尾部 10000 行与 T1 完全一致（且首行不是断在半截的碎片 —— 这是 Todo #34 踩过的坑）。

### 7.4 选中复制

选中走 `SelectionService` + `Terminal.getText(start:end:)`，都是缓冲区上的操作。断言还原后 `T2.getText(...)` 与 `T1.getText(...)` 相等，即证明选出来的文本一致。**鼠标拖选本身是原生代码路径，一行不改。**

### 7.5 背压/重同步

用一个故意慢的假 viewer 灌爆队列 → 断言收到 `resync` 且重同步后逐格相等；断言**任何情况下都没有出现「丢一段继续发」**。

### 7.6 人工清单（GUI 不许自动驱动，这几条留给人类点一下）

1. session 跑着 → 退出 app → 重开 → 终端画面完整、历史在、agent 那一轮没断
2. 拖窗口改宽度 → 历史重排正常、不掉字
3. 上滚到顶 → 历史够长；选中一段跨行文本复制 → 粘贴出来是对的

---

## 8. 崩溃、孤儿、版本不一致

### 8.1 app 崩了 / 被关掉

daemon 无感。所有 session 照跑。重开 app → 握手 → 拉列表 → attach → 拿快照。**这就是 A1 本身。**

### 8.2 daemon 崩了

agent 子进程的 PTY 主端随 daemon 一起没了 → 子进程下次读写拿到 EIO/SIGHUP → 会死。**这个损失当前方案救不了**（要救得有第三个进程替它持 fd，不划算）。所以显式设计的是**善后**：

- daemon 持续把 `{sessionId, pid, pgid, 启动时刻(kinfo_proc 的 p_starttime), argv0}` 写进 `sessions.registry.json`。
- 被系统守护自动拉起后，daemon 第一件事是清理孤儿 —— **但必须双重核对再动手（R2）**：

  ```
  对 registry 里每条记录：
    pid 已不存在                  → 记账，无事
    pid 存在，且 kinfo_proc 的启动时刻与 registry 记的一致
                                  → 确认是我们的孤儿 → kill 进程组
    pid 存在，但启动时刻对不上     → **pid 被复用了，绝不动手**
                                  → 记账 + 白板说明 + 日志留痕
  ```

  理由：这台机器上常年十几个 agent 进程，pid 会复用。只凭 pid/pgid 下手迟早误杀无辜进程，而且是事后查不出来的那种事故。**宁可留一个孤儿，也不能误杀。** 留下的孤儿写进日志与白板，由人决定怎么处理。
- 然后往受影响的每个 crew 白板发一条：「后台进程重启，N 个 session 被中断」，逐个点名；有对不上账的 pid 时额外说明。**fail loud，不静默。**

### 8.3 版本不一致（新 app + 旧 daemon）

握手交换 `protocolVersion`、`build`、`capabilities`。**先读 §4.4 的兼容纪律** —— 下面这张表只在「真的发生了不兼容变更」时才走得到，而那应当是罕见事件。日常发版走的永远是第一行。

| 情况 | 行为 |
|---|---|
| 版本相同（**常态**） | 正常连接（build 不同不管；能力差异走 `capabilities` 降级，不断连） |
| daemon 较旧，且**没有 session 在跑** | app 静默换代（停旧 → 起新 → 连上） |
| daemon 较旧，且**有 session 在跑** | app 顶一条横幅：「后台是旧版；换新会打断 N 个在跑的 session」+ [稍后] / [立即换代]。选「稍后」→ 记住，等到全部 session 空闲的那一刻自动换代 |
| daemon 较**新**（用户降级了 app） | app 拒绝连接并说明，不尝试兼容 |

**将来可做、本期不做**：新旧 daemon 之间用 `SCM_RIGHTS` 传递 PTY 主端 fd + session 元数据，让后台自己升级也不打断。技术上可行，但先把主线做完。

### 8.4 daemon 必须跑在用户域（R3，技术边界，不是偏好）

**必须是用户级 agent（`SMAppService.agent`，登录项），不考虑 system daemon。**

理由不是习惯问题：agent 子进程要读钥匙串里的 claude/codex 订阅登录态、要用户的环境变量与 PATH、要用户的 TCC 授权。跑在 system domain 里这些会**静默**失效或换成另一套 —— 而「登录态悄悄降级、然后被误诊成后端问题」正是本项目吃过大亏的病（见 `feedback_supabase_swift_silent_anon` / `feedback_sim_unsigned_keychain` 那两次误诊）。

因此 P5 的验收里有一条**不可省略**的：**daemon 拉起的 session 能真正读到订阅登录态**——起一个 session、让它跑一次真需要登录态的动作、确认成功。「进程起来了」不算过。

（「进不进登录项、要不要用户可见」那半仍是人类的产品决定，见 §11。）

### 8.5 daemon 的日志与自检（R4）

app 退化成 viewer 之后，后台出问题**没有画面可看**。所以 daemon 自带可观测性：

- **滚动日志**（`~/Library/Logs/PendingCrew/daemon.log`，按大小滚动，保留最近几份）。至少记：握手（双方版本/build）、attach/detach、resync 次数、背压溢出次数、孤儿回收的每一条判定（含「pid 复用故不动手」）、上次退出原因、每个 session 的拉起与终止。
- **`PendingCrew --daemon-status`**：在终端里直接问出「几个 session 在跑（分别是谁）、连着几个 viewer、daemon 启动于何时、上次重启原因、协议版本」。不需要开 app、不需要 GUI。
- 这条同时是**排查纪律**：以后「后台好像不对劲」的第一步永远是 `--daemon-status` + 看日志，不是猜。

### 8.6 明确扛不住的

- **整机重启 / 关机**：进程没了，session 断。（系统守护会在下次登录时把 daemon 拉回来，但那一轮 agent 的工作丢了。）
- **daemon 自己升级**：见 8.3，本期会打断。
- **磁盘满 / 共享账本损坏**：沿用现有的损坏归档 + 白板回执机制，不新设计。

---

## 9. 分阶段落地（每阶段可独立回退）

原则：**P0–P3 全部在 app 单进程内完成，对用户零可见变化**。真正的进程分家在 P4 才发生，且藏在总闸后面。任何一个阶段都可以单独 revert 而不留半截状态。

| 阶段 | 做什么 | 完成判据 | 回退方式 |
|---|---|---|---|
| **P0**<br>所有权归拢 | 引入 `ProcessRole`；把 `CrewSessionRunner` / `CrewRelayAgent` / 三个唤醒器 / `LocalAgentUsageMonitor` / 两个轮询中心从视图上摘下来，交给一个 app 级 `SessionHost` 单一所有者；`MacRootView` 那串 `.onChange` 编排 glue 挪进 `SessionHost` 的方法 | 全量测试绿；视图只观察不创建；`grep` 不到视图里 new 长期对象 | 单 commit revert |
| **P1**<br>终端劈半（仍单进程） | `AgentTerminalSession` 拆成 `AgentSessionCore`（Foundation only：`LocalProcess` + 无画面 `Terminal` + 全部扫描器）+ `TerminalMirrorView`（AppKit，`feed(byteArray:)`）；补掉 §5.1 的协议泄漏 | 全量测试绿；A2 三条手工项与旧版逐项对比无差异 | 单 commit revert |
| **P2**<br>协议 + 进程内传输 | 定义 §4 全部消息；实现 `InProcessTransport`（直接函数调用）；`RemoteSessionBackend` 实现 `SessionBackend` 并走传输层。**app 仍是单进程，但所有调用已经过协议。** | 全量测试绿；行为与 P1 完全一致；协议单测覆盖全消息集 | 传输层切回直连（一行） |
| **P3**<br>快照 + 背压 | `TerminalSnapshotEncoder`；resync；分片；§7.1–7.5 全部测试 | 往返/reflow/回滚/选中/背压五组测试全绿 | 单 commit revert（P2 不依赖它） |
| **P4**<br>真进程分家 | `--daemon` 身份；`UnixSocketTransport`；单实例锁；attach/detach/重连；编排整体搬进 daemon；app 退化成 viewer。**总闸 `PENDINGCREW_BACKEND` 默认仍是 `inproc`** | 手动切 `daemon` 跑通全部剧本；`inproc` 模式行为不变 | 总闸设回 `inproc` |
| **P5**<br>常驻与善后 | 用户级 agent 注册（`SMAppService.agent`，开机自启 + 崩溃自拉）；菜单栏常驻项；版本握手横幅；孤儿回收（含 pid 复用核对）；滚动日志 + `--daemon-status`；A1 端到端剧本；把总闸默认切成 `daemon` | A1（**三条路径**，见 §1.1）–A4 全绿；**daemon 拉起的 session 能真正读到订阅登录态**（§8.4，「进程起来了」不算过）；`--daemon-status` 能问出实况；#59 的性能采样复跑，证明未 attach 的 session 主线程开销为零 | 总闸设回 `inproc`（daemon 停掉即可） |

**排期**：P0 与 P1 各是一个 worker 的活（会大改同一批文件，**必须串行**）。P2/P3 可并行（不同文件）。P4/P5 串行。

---

## 10. 迁移期新旧两套怎么共存

不做「新旧并存的双轨运行」—— 那正是双头的温床。共存靠的是**同一套代码、两种传输**：

- P2 之后，只有一条编排实现、一条 backend 实现。区别只在 `SessionTransport` 是 `InProcess` 还是 `UnixSocket`。
- 总闸 `PENDINGCREW_BACKEND` 决定用哪个传输，**进程启动时读一次**，中途不切。
- `inproc` 模式下 GUI 进程的 `ProcessRole` 算出来就是 `.orchestrator` —— 于是「所有权只有一份」这条不变量在两种模式下用的是同一段断言代码，不需要两套。
- 不存在「一半 session 在 daemon、一半在 app」的中间态。P4 期间要么整体 inproc，要么整体 daemon。

---

## 11. 待人类拍板（已在群里问，默认按推荐走）

1. **后台怎么常驻** —— **技术边界已由父机长钉死：只能是用户级 agent（登录项），system daemon 不予考虑**（理由见 §8.4）。留给人类的只剩产品面那半：注册成登录项（开机自启、崩溃自拉、「系统设置 → 登录项」里多一条，推荐）vs app 拉起后脱离独活（不进登录项，但整机重启后要等下次开 app）。
2. **关掉 app 后怎么让人知道还有 agent 在跑** —— 推荐留一个菜单栏项（几个在跑 / 打开窗口 / 全部停）。备选：完全隐形。
3. **版本不一致的默认行为** —— 推荐 §8.3 那张表。

---

## 12. 参考

- `docs/2026-08-19-backend-split-inventory.md` —— 逐条带文件行号的现状清单
- `docs/tech-debt.md` —— PTY 输出过主线程那条结构债
- `docs/2026-08-19-ui-jank-profile.md` —— #59 的性能基线（P5 复跑它做对照）
