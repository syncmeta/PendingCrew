# P4 收尾交付报告（数据根 + 编排闸门）

> **为什么这份东西在仓库里而不在群聊里**：2026-08-27 00:00 前后，本 session 的
> `post_to_crew` / `read_whiteboard` 全部报权限错，白板通道单向断了（机长那条方向仍
> 通）。机长让我把报告写成仓库里的文件。**这也顺带证明了一件事**：结论落在仓库里
> 比落在群聊里耐得住 —— 那份逐跳链在群聊里被截断过一次，这次整条通道直接没了。
>
> 分支：`pendingcrew/session-49b1c7`（接自前任 `pendingcrew/session-773939`）。

---

## 一、这一段做了什么（七笔，按依赖序）

| commit | 是什么 |
|---|---|
| `a4f7bfa` | `PendingCrewDataRoot` —— 数据根收成一道缝，可整个挪走（约束 1/2/3/6 的 daemon 侧） |
| `0a592a5` | `SessionOrchestratorLock` —— 单实例锁从「daemon 的」变成「编排者的」（daemon 侧） |
| `0bdb815` | **编排闸门搬到进程入口** —— app 侧终于也取那把锁 |
| `f26b9d0` | iOS 端从 `5284ba6` 起一直编不过（`#endif` 位置），修 |
| `0f2362d` | 拒绝要有出口 —— 界面态做成纯判定；`ProcessRole.current` → `requested` |
| `c2de5dd` | `docs/architecture.md` 补上这条线的全账 |
| `6627cc6` | 合 main 0.1.17 |

前两笔是把机长代提的 WIP `3399640` 按约束 5「独立提交、可单独 revert」**重切**出来的。
重切结果与 `3399640` 的树**对字节比过，只差一处**：原来那笔留了一句「见
`SessionDaemonLock` 的注释」，而那个类型已被它自己删掉 —— 顺手把注释补成实话。

---

## 二、①「app 到底取没取锁」——先量，再改

机长（正确地）把 brief 里那句「app 那边还没取那把编排者锁」的口径降了回去：他只
grep 到 `acquire` 单一调用点，从那儿到「app 不取锁」中间那段没查。所以先量。

**结论：app 确实没取锁。但更值钱的是量的过程本身。**

### 那份逐跳链（源码级读数，当时未运行；行号是 `0a592a5` 那一刻的）

1. `Sources/PendingCrewEntry.swift:19` `PendingCrewEntry.main()`
2. `Sources/PendingCrewEntry.swift:30` `if SessionDaemonMain.runIfDaemon(...) { return }`
   → `Sources/Mac/Services/SessionDaemonMain.swift:21`
   `guard argv.contains("--daemon") else { return false }`（GUI 走 false 那支）
3. `Sources/PendingCrewEntry.swift:33` `PendingCrewApp.main()`
4. `Sources/PendingCrewApp.swift:12` `@StateObject private var sessionHost = SessionHost()`
   —— init 在 `Sources/Mac/Services/SessionHost.swift:29–33`，**不取锁**
5. `Sources/PendingCrewApp.swift:37–38` `WindowGroup { RootView() }`
   → `Sources/PendingCrewApp.swift:101` `MacThreePaneView()`
6. `Sources/Mac/Views/MacRootView.swift:68` `.task {` → `:71` `sessionHost.begin(...)`
7. `Sources/Mac/Services/SessionHost.swift:47` `begin(...)` → `:49–50`
   `case .orchestrator: start(...)`
8. `Sources/Mac/Services/SessionHost.swift:72–105` `start(...)` 全文 —— **没有任何取锁**

### 四条封边（也是源码级）

- `SessionOrchestratorLock.acquire` 全仓唯一调用点：`SessionDaemonHost.swift:163`。
- `SessionOrchestratorLock` 这个名字全仓只出现在 3 个文件：它自己、
  `SessionDaemonHost.swift`、`SessionDaemonHostTests.swift`。**GUI 那条链上一个都没有。**
- `SessionDaemonHost(` 生产构造点唯一：`SessionDaemonMain.swift:35`，在 `--daemon`
  分支里（其余 7 处全在测试里）。
- **没有第二套单实例机制兜底**：全仓 `flock(` 只有 `MultiProcessJSONStore.swift:37/38`、
  `WhiteboardCursor.swift:182/183` 和锁自己；`LSMultipleInstancesProhibited` /
  `NSRunningApplication` 在 `Info.plist`、`project.yml`、全部 Swift 源码里**零命中**。

### 机长提的第三条路（「让 app 那条编排入口在单测里跑一趟」）走不通，而那就是答案

第 6 跳是一个 SwiftUI 视图钩子 —— **没有不开窗口的入口能进去**。所以：

1. 这道闸门在 app 侧**无法被证明**；
2. **闸门挂错了对象**：「谁是编排者」是**进程身份**的属性，不是某个视图的属性。
   挂在视图上，换任何一个新入口（第二个窗口、菜单栏 extra、将来的 headless）都得
   各自记得再问一遍 —— 而当时连第一个入口都没问。

所以修法不是「往 `begin` 里补一句取锁」（那还是挂在视图上），而是把闸门搬到
`PendingCrewEntry.main()`，跟 `ProcessRole.requested` 同一拍解析。

---

## 三、拿不到锁时怎么办：daemon 和 app **不是同一个**反应

- **daemon 拿不到 = 拒绝启动**（不变）。第二个 daemon 就是双头本身。
- **app 拿不到 ≠ 拒绝启动**（「用户双击图标没反应」是这条线上最贵的翻车）。
  语义是「已经有人在编排了」，所以看**是谁**：

| 锁被谁占着 | app 怎么办 |
|---|---|
| `kind == "daemon"` | `.followDaemon` → 退化成 viewer 连上去（那边真的在 socket 上听） |
| 别的 / 读不出 / 打不开锁文件 | `.conflict` → **既不编排也不退化**，把冲突摆到用户面前 |

最后一行堵的是**这道闸门自己会造出来的新静默态**：锁被一个不听 socket 的东西占着时
若也退化，`ViewerSessionClient` 会去拉 daemon → 那个 daemon 因锁被占**当场 exit 0**
→ 连不上 → 退避重连 → 永远循环，用户看到「界面在、什么都不动、不报错」。
**方向反过来的同一种静默。**

### 顺带挡下的第二个双头：`ProcessRole.requested` / `.effective`

新状态「身份是 `.orchestrator` 但没拿到锁」会让
`CrewStore.ownsSharedControlChannel` 照样去排空共享控制通道 —— 那三条通道是
「一文件一命令、排空后删」的无锁模型，两边都排会让机长的 `start_session` 被随机
一方吞掉，不报错、不重试、命令文件已经删了。**修一个双头顺手造出另一个**，
所以拆成两个字段。

**改名不是审美**：机长指出「手改五处 precondition + 控制通道」是一份手工维护的
名单，而名单和它要防的东西不在同一个地方。所以把 `current` 改成 `requested`，
**让编译器把每个调用点顶出来逐个定**。而且不改成同义词 —— 叫 `current` 时它读起来
像「当前实际角色」，那正是这次差点出事的误读。

---

## 四、②「把拒绝关掉，测试必须红」——真跑了两趟

验法：`OrchestrationGate.refuse(_:dataRoot:)` 改成恒返回 `.takeOver`。

**第一趟（闸门层，`0bdb815` 时）**，`** TEST FAILED **`，四条具名红：

```
test_拒绝里三样齐 : failed - 应当是冲突，实际 takeOver
test_拿不到锁的进程在effective上不再是编排者 : XCTAssertEqual failed:
    ("orchestrator") is not equal to ("viewer")
test_锁被daemon占着时退化成viewer : failed - 锁被 daemon 占着时应当退化成 viewer，实际 takeOver
test_锁被另一个app窗口占着时报冲突不退化 : failed - 锁被另一个 app 占着时不许退化成 viewer，实际 takeOver
```

**第二趟（接完界面层，`0f2362d` 时）**，同一处改动，**多红一条**：

```
OrchestrationNoticeTests test_锁被别人占着时界面必须是错误态 : failed -
    闸门拒绝了，界面却是 none —— 拒绝没有出口，等于没拒绝
```

两趟还原后都 `** TEST SUCCEEDED **`（18 条全绿）。

**每条红在自己该红的地方**，不是一条 crash 带倒一片。

### 那个运行时读数

`OrchestrationGateTests.test_没人占着时app接管并且锁文件里真的是本进程` ——
它断言的不是函数返回了什么，是**锁文件里真的躺着本进程**（`currentHolder` 只看
不写）。**这条在闸门搬家之前写不出来**，它就是「app 到底取没取锁」的运行时答案。

`OrchestrationNoticeTests.test_锁被别人占着时界面必须是错误态` 是整条链：
**真占一把锁 → 真跑 `installForGUIProcess` → 裁决喂给 `resolve` → 断言屏幕上是
错误态**，中间没有替身。

---

## 五、六条约束逐条对账

| # | 约束 | 状态 |
|---|---|---|
| 1 | 只开一道缝，不许各自 `getenv` | ✅ 全仓只剩 `PendingCrewDataRoot` 一处算 `applicationSupportDirectory` |
| 2 | 启动解析一次，之后不变 | ✅ `static let url` |
| 3 | 默认路径逐字不变 + **一条会红的测试** | ✅ `PendingCrewDataRootTests` 第一条断言写的是**字面路径**，不是自证 |
| 4 | 锁和 registry 跟着数据根走 | ✅ 有测试（`test_锁跟着数据根走`，且断言它**不**等于真数据根下那条） |
| 5 | 独立提交、可单独 revert | ✅ 重切成 `a4f7bfa` / `0a592a5` 两笔，各自 BUILD SUCCEEDED |
| 6 | 启动打一行数据根 | ✅ **两条路都打**：daemon 在 `SessionDaemonHost.start`，app 在 `OrchestrationGate.installForGUIProcess`；后者有测试（`test_app启动时把数据根打出来`） |

约束 3 那条「把默认解析改坏、看它红」**我没有真跑**（跑了的是约束 2 那条验法：
把拒绝关掉）。**这一条按「测试写成了会红的形状」记，不按「验过了」记。**

---

## 六、判据

**全量第一趟**（合 main 后，fixture 在场）：

- `** TEST SUCCEEDED **`
- **具名失败 0**
- **passed 1752**
- **skip 6**，逐条：
  1. `AgentTuiFixtureRecorder.testRecord` ← 判据内
  2. `CrewLastMessageCacheTests.test_基准_现场白板目录` ← 判据内
  3. `SessionAwaitingReplyInputsCacheTests.test_基准_现场目录` ← 判据内
  4. `CrewMentionFilterRealWhiteboardTests.testAWrongLocalUserIdDoesNotKeepThem`
  5. `CrewMentionFilterRealWhiteboardTests.testEveryRealHumanMessageCarriesTheLocalSentinelId`
  6. `CrewMentionFilterRealWhiteboardTests.testFilterKeepsEveryRealMessageTheHumanSent`

### ⚠️ 判据要补两个前提，它们今天各被违反了一次

「`skip == 3`」这条尺子**没写自己成立的条件**，而今天两个条件先后失效：

1. **fixture 在场**。新 worktree 里没有那份 gitignore 的真实群聊 fixture
   （`Tests/PendingCrewTests/Fixtures/LEDDriverCrew`），`CrewChatOpenCostTests` 那 8 条
   会 skip → **基线那一趟量到的是 skip 11**。我从主 checkout 把 fixture 拷进来后
   那 8 条恢复。
2. **真白板目录读得动**。`CrewMentionFilterRealWhiteboardTests` 那 3 条读
   `~/Library/Application Support/PendingCrew/whiteboards/`，读不到就 skip ——
   **基线那趟它们是 passed，这一趟变成 skipped**，因为本 session 的进程线在
   00:00 前后被挡在那个目录外面（同一件事也让我的 `post_to_crew` 断了）。

**所以完整的判据应当是**：`skip == 3` **当且仅当** fixture 在场且真白板目录可读；
否则逐条枚举、说明每一条多出来的 skip 是哪个前提没成立。
**一把没有前置条件的尺子，会让下一个人在新 worktree 里量到 11 而当成回归。**

### 关于基线 1679

判据里写的基线是 1679，我这趟是 1752。**我不拿这两个数相减下结论** —— 我不知道
1679 是在哪一次跑上量的（哪个 commit、fixture 在不在场、真白板读不读得动）。
换用**测试类名集合**比对更硬，那份比对在通道恢复后补。

---

## 七、还没做 / 明确不做的

- **报段没报**（机长让等通道恢复）。报段要报的两样先写在这儿：
  - **动了哪些段**：`Sources/Mac/LocalRunner/`（`OrchestrationGate` 新增、
    `ProcessRole` 改名、`SessionOrchestratorLock` 新增、`SessionDaemonHost`）、
    `Sources/Mac/Services/`（`SessionHost` + 四个 center 的 precondition、
    `CrewSessionRunner` 的合并冲突解法）、`Sources/Mac/Views/`
    （`MacRootView` 挂横幅、`OrchestrationNoticeBar` 新增）、
    `Sources/Stores/`（`PendingCrewDataRoot` 新增 + 六个 store 改接线、`CrewStore`
    一行）、`Sources/Support/OrchestrationNotice.swift` 新增、
    `Sources/PendingCrewEntry.swift`、`project.yml` + `pbxproj`、
    `docs/architecture.md`。
  - **要不要占用 `main` 当基线**：**不要**。已经把 main 合进分支了（`6627cc6`），
    落 main 时只需要一次快进/合并，不需要我占着 main 跑什么。
- **A2 手工三条**（`docs/internal/2026-08-19-backend-split-manual-checks.md`）
  归机长推，我没碰。
- **没起过真 daemon 冒烟测。** 全部走单测。`PENDINGCREW_DATA_DIR` 虽然做完了，
  但我没有拿它去跑真 daemon —— 那需要先核启动日志那一行，而这台机器上没有第二次机会。
- **约束 3 那条「真把默认解析改坏看它红」没跑**（见第五节）。
- **横幅本身没有被人眼看过。** GUI 验不了（不许为验证开窗口），所以能证的只有
  「状态到没到界面层」，不是「它长什么样」。**这两件别混。**

---

## 八、留给下一个人的三条

1. **`whiteboards/` 那个权限故障我没查出根因**，机长接手了。已排除的：不是装
   0.1.17（app 仍是 0.1.15、bundle mtime 08:29）、不是文件坏了（别的进程还在正常
   写）、不是 `.claude/settings*.json` 里的 deny。
2. **iOS 端要真编。** `f26b9d0` 那条说明前任那几笔之后没人编过 iOS ——
   macOS 编得过、测全绿，只有 iOS 红，是最容易漏的一种。
3. **`OrchestrationGate.refuse` 是这道闸门的开关。** 想验它还活着，就把它改成恒
   `.takeOver` 跑一趟 `-only-testing:PendingCrewTests/OrchestrationGateTests
   -only-testing:PendingCrewTests/OrchestrationNoticeTests`，**必须红五条**。
   跑完记得还原。
