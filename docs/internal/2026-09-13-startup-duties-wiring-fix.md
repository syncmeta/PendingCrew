# 恢复弹窗 / 启动换代 / app 退出印记：默认模式下没接上 —— 实测与修法

<!-- doc-ref-base: 5f02cb2 -->

- 日期：2026-09-13
- **基准提交：`5f02cb2`**。file:line 以这棵树为准。
- 前情：`272282d`（退出印记）、`96dbde8`（恢复弹窗）、`fa0c475`（前端更新→后端换代）
  三笔的判定层都有测试；**接线只有构建证明**（三笔的提交说明里都写了这条边界）。
  这份记的就是那条边界里藏着的东西。

---

## 1. 读代码读到的（白纸黑字）

| 事实 | 位置 |
|---|---|
| 不设 `PENDINGCREW_BACKEND` 时 GUI 的角色是 `.viewer` | `Sources/Mac/LocalRunner/ProcessRole.swift` `resolve`：`flag == "inproc" ? .orchestrator : .viewer` |
| GUI 唯一入口是 `begin`，viewer 分支只走 `beginViewer()`，**不调 `start`** | `Sources/Mac/Views/MacRootView.swift:88`；`Sources/Mac/Services/SessionHost.swift:64-98` |
| app 退出印记、`BackendUpdateCoordinator.runIfNeeded`、`restoreOffer` 的计算、⌘Q 观察者**全在 `start` 里** | `SessionHost.swift:191-237` |
| `start` 的唯一非 GUI 调用者是**后台进程** | `Sources/Mac/Services/SessionDaemonMain.swift:66-68` |
| 弹窗读的是 **GUI 进程里那个** `SessionHost` 的 `restoreOffer` | `Sources/PendingCrewApp.swift:19`、`:163` |
| `restoreOffer` 没有任何协议转发（全仓只在 SessionHost 与 PendingCrewApp 出现） | grep `restoreOffer` |
| `restartMember` / `launchWorker` **没有 viewer 转发**，协议里也没有「重启/恢复成员」操作 | `CrewSessionRunner.swift:2592-2650`；`SessionProtocolEndpoints.swift:961-974` |
| 后台遇到不认识的编排请求：只写一行日志就丢 | `SessionDaemonMain.swift` `handle` 的 `default:` |

## 2. 推出来的（推理链）

默认模式（viewer）下：

1. **GUI 从不写 `app.lastexit.json`**，所以「上次界面崩了」永远判不出来。
2. **GUI 从不做换代检查**。换代检查跑在后台进程里，而它探的「本机后台」就是它自己
   → 版本恒等 → `leaveAlone`。**前端更新带着后台换代这件事在默认模式下不会发生**，
   而且不是第一次的鸡生蛋，是每一次。
3. **弹窗永远不弹**：算 `restoreOffer` 的是后台进程里的 SessionHost，弹窗读的是
   GUI 进程里那个从没算过的。
4. 后台进程还会**冒名写 `app.lastexit.json`**（role `.app`），一旦 GUI 那侧接上，
   两边会互相覆盖。
5. 就算把弹窗接上，「接回来」在 viewer 里调 `restoreSessions → restartMember →
   launchWorker`，**会在界面进程里直接拉 agent**——与后台双头。
6. 原 ⌘Q 观察者对每个在跑的 run 调 `stop()`；viewer 里这些 run 是镜像、`stop` 会转发
   `stopRun` 给后台 → **挪过去而不改的话，关界面等于停掉后台全部 session**。

只在 `PENDINGCREW_BACKEND=inproc` 且拿到锁时这三件事才在 GUI 里跑。

## 3. 实测到的

（只读探针与文件列表，没碰界面。）

- 界面进程 pid 18872，`/Applications` 版本 **0.1.37(20708.82403)**，启动于 2026-09-13 10:18:46。
- 后台进程 pid 53445，`--daemon-status` 自报 **0.1.34(20707.55457)**，启动于 2026-09-11T16:14:26Z。
- tag 日期：v0.1.35 2026-09-12 02:51（第一个含 `fa0c475`），v0.1.37 2026-09-13 06:16。
- 数据根 `~/Library/Application Support/PendingCrew/` **列得出来**（对照：`daemon.registry.json`
  mtime 今天 10:30、`orchestrator.lock` 在），但**一枚 `*.lastexit.json` 都没有**。

**哪条有判别力、哪条没有：**

- 「后台没被换」**没有判别力**：后台 0.1.34 早于换代逻辑，鸡生蛋也解释得通。
- 「0.1.37 界面跑了一个多小时、数据根里没有 `app.lastexit.json`」**有判别力**：
  按正确接线它在启动时就写（`markRunning`）。与第 2 节第 1 条一致。
  残余解释：写失败（`onWriteFailure` 只进 NSLog）。今天那个周期性 EPERM 窗口
  09:28 已结束，10:18 之后 registry 照常在写，**这条解释很弱，但我没去翻 NSLog 核**。
- `ps eww` 没看到 `PENDINGCREW_*` 环境变量——**没有判别力**，macOS 可能不给看别的进程的环境。

## 4. 修法（三个单元，按依赖顺序）

**单元 1：职责挪到界面进程。**
- `start` 里拿掉 app 印记 / 换代检查 / 恢复判定 / ⌘Q 观察者（后台进程有自己的
  daemon 印记，`SessionDaemonMain` 里已经写了）。
- `begin` 第一件事跑一次「界面启动职责」，**自带只跑一次的门**（`.task` 会重跑）：
  读上一轮 app 印记 → 记 running → 取候选 → 换代检查（**在 viewer 连上之前**，
  停掉旧的之后 viewer 自己会拉起新版）→ 算弹窗。
- ⌘Q 观察者：记 draining →（**只有本进程真是编排者时**才停 run）→ 记 clean。
- 接线测试钉住：`start` 里不许再出现这几样；`begin` 里必须有、且在分岔之前。

**单元 2：viewer 里「接回来」转交后台。**
- 新编排操作 `orchestration.restoreSessions`（带候选列表），后台调已有的
  `runner.restoreSessions`（失败各自落白板那套不变）。
- 后台在能力表里声明它；viewer **只在协商出这项能力时才转交**，否则大声失败——
  旧后台遇到不认识的操作只写日志就丢，那是静默失败。
- 回执多一态「已交给后台」，**永远不说「已接回」**（结果在各 crew 群里）。
- 弹窗在连上后台之前被点：等一小段，等不到就如实失败，不排队。

**单元 3：设置「后端」页补实况 + 重启入口**（原计划 #15，排在后面）。

### 4.1 单元 1 落地：`06894b1`

### 4.2 单元 2 实际形状

- 判定 `SessionRestoreRoute.decide(isViewer:connected:negotiated:)`（`SessionRestoreOffer.swift`）：
  编排者 → 本进程接；viewer 且连上且协商出 `restore-sessions` → 转交；**其余一律拒绝，
  拒绝理由按候选逐个落各自 crew 的群**（沿用原来那段 fail-loud）。
- 能力项加进 `SessionDaemonHost.defaultCapabilities`（界面与后台共用同一份，协商取交集）
  → 现在跑着的 0.1.34 后台协商不出，转交会被拒而不是被静默丢。
- 候选名单编解码：**一条坏的整份作废**，后台只写日志、一个也不接。
- `SessionHost.restoreOfferedSessions`：viewer 下最多等 15 秒连上后台，等不到交给判定拒绝。

**单元 2 的边界**：
- 转交之后界面回执只进 NSLog（「已把 N 个交给后台去接」）；接没接回来**只在各 crew 群里**
  看得到（后台 `restoreSessions` 的失败落群那一套），弹窗那侧不再回显。
- 名单解不开（只会是 bug）时只有后台日志一行，群里没有——界面那侧已经说了「已交给后台」。
- 协议往返本身没有端到端测过（真起一个后台再发请求）；只有判定测试 + 接线源码断言 + 构建。

## 5. 边界（这份修法**不**覆盖的）

- **后台单独崩了、界面一直开着**：viewer 会重连并拉起新后台，session 被打断，
  但界面的 app 印记是 running、不会问。要覆盖得读 daemon 印记，而新后台起来时会
  先把它覆盖掉——需要另想，这份不做。
- 换代那条仍没端到端跑过；弹窗仍没真机看过（界面自动化禁令）。
- 一次恢复十几个 session 的启动风暴、codex 侧 `thread/resume` 的半截回合，照旧没量。
