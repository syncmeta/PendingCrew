# P4 交接（写给下一个人，很可能是 codex）

> **这份是自足的，读它就够，不用去翻群聊。** 更长的那份在
> `docs/internal/2026-08-27-p4-handoff-report.md`（已提交），两份不冲突：这份是
> 「接手要知道的」，那份是「量到了什么、怎么量的」。
>
> **落 main 之前把根目录这个文件删掉** —— 它是交接件，不是仓库该长期带着的东西。
> 那份 `docs/internal/` 的报告留着。
>
> 分支：`pendingcrew/session-49b1c7`。工作树**干净**，所有东西都在提交里。
> **main 已经合进分支了**（`6627cc6`，0.1.17），落 main 时不需要再占用 main 当基线。

---

## 0. 现在的状态：一句话

**能编（macOS + iOS 两端）、能测（全量两趟绿）、没合 main、没报段。**
人类叫停 claude 这条线，所以是**在一个干净边界上停的**，不是半成品。

---

## 1. 已落哪几笔，各是什么

| commit | 是什么 | 能不能单独 revert |
|---|---|---|
| `a4f7bfa` | `PendingCrewDataRoot` —— 数据根收成一道缝，可整个挪走（`PENDINGCREW_DATA_DIR`） | 能 |
| `0a592a5` | `SessionOrchestratorLock` —— 单实例锁从「daemon 的」变成「编排者的」（**daemon 侧**） | 能 |
| `0bdb815` | **编排闸门搬到进程入口**，app 侧终于也取那把锁 | 能 |
| `f26b9d0` | iOS 端从 `5284ba6` 起一直编不过（`#endif` 位置错），修 | 能（但别 revert，revert 了 iOS 就红） |
| `0f2362d` | 拒绝要有出口：界面态做成纯判定；`ProcessRole.current` → `requested` | 能 |
| `c2de5dd` | `docs/architecture.md` 补上这条线的全账 | 能 |
| `6627cc6` | 合 main 0.1.17 | 合并笔 |
| `7417378` | `docs/internal/` 那份长报告 | 能 |

前两笔是把机长代提的 WIP `3399640` 按约束 5「独立提交、可单独 revert」**重切**出来
的。重切结果与 `3399640` 的树**对字节比过，只差一处**（一句指向已被删掉的
`SessionDaemonLock` 的注释，顺手补成实话）。

---

## 2. ① app 侧取锁：**做完了**

### 病根不是「忘了取」，是闸门挂错了对象

改之前量过：app 通往编排的**唯一**入口是 `MacRootView.swift` 的
`.task { sessionHost.begin(...) }` —— 一个 SwiftUI 视图钩子。于是①单测进不去，
②「谁是编排者」这件**进程身份**的事被挂在了某个视图上。

所以修法是把闸门搬到 `PendingCrewEntry.main()`
（`OrchestrationGate.installForGUIProcess`），跟 `ProcessRole.requested` 同一拍解析。
**不是**往 `begin` 里补一句取锁 —— 那还是挂在视图上。

（完整的 8 跳链 + 四条封边在 `docs/architecture.md` 和那份长报告里，都标了
「源码级、当时未运行」和行号属于哪个 commit。）

### 拿不到锁时：daemon 和 app **不是同一个**反应

- **daemon** 拿不到 → 拒绝启动（不变）。
- **app** 拿不到 → 看**是谁**占着：
  - `kind == "daemon"` → `.followDaemon`，退化成 viewer 连上去；
  - 别的 / 读不出 / 打不开锁文件 → `.conflict`，**既不编排也不退化**，摆给用户看。

第二条堵的是这道闸门自己会造出来的新静默态：不听 socket 的东西占着锁时若也退化，
`ViewerSessionClient` → 拉 daemon → 那个 daemon 因锁被占 **exit 0** → 连不上 →
退避重连 → 永远循环，用户看到「界面在、什么都不动、不报错」。

---

## 3. ②「把拒绝关掉，测试必须红」：**做完了，真跑过两趟**

开关就是 `OrchestrationGate.refuse(_:dataRoot:)`。改成恒返回 `.takeOver`：

- **第一趟**（只有闸门层时）→ 4 条具名红。
- **第二趟**（接完界面层）→ **5 条**，多的那条是
  `OrchestrationNoticeTests.test_锁被别人占着时界面必须是错误态`
  （`failed - 闸门拒绝了，界面却是 none —— 拒绝没有出口，等于没拒绝`）。

两趟还原后都 `** TEST SUCCEEDED **`。**原始输出逐字抄在那份长报告第四节。**

**想复验这道闸门还活着**：把 `refuse` 改成恒 `.takeOver`，跑

```
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' \
  -only-testing:PendingCrewTests/OrchestrationGateTests \
  -only-testing:PendingCrewTests/OrchestrationNoticeTests test
```

**必须红 5 条。跑完记得还原。**

---

## 4. `ProcessRole.requested` / `.effective` 改名：**做完了**

- `current` → `requested`（我这副身份**想**当什么）；新增 `effective`
  （过了闸门之后我**当上**了什么）。
- 全仓 `ProcessRole.current` **零残留**，编译器逐个顶出来改的，**没有手工名单**。
- 谁该问哪个：问「我该不该动共享账 / 起长期定时器」的一律问 `effective`
  （`CrewStore.ownsSharedControlChannel` + `SessionHost.start` / `QuotaCenter` /
  `ModelCatalogCenter` / `LocalAgentUsageMonitor` / `CrewLocalMentionWaker`
  五处 precondition）；`SessionHost.begin` 的身份分岔问 `requested`。
- **为什么非改名不可**：新状态「身份是 `.orchestrator` 但没拿到锁」会让
  `ownsSharedControlChannel` 照样排空共享控制通道 —— 那三条通道是「一文件一命令、
  排空后删」的无锁模型，两边都排会让机长的 `start_session` 被随机一方吞掉，
  不报错、不重试、命令文件已经删了。**修一个双头不能顺手造出另一个。**

---

## 5. `OrchestrationNotice.resolve` 那层：**做完了**

- `Sources/Support/OrchestrationNotice.swift` —— **纯判定**：
  输入 `(OrchestrationGate.Decision?, ViewerLinkState?)`，输出
  `.none` / `.conflict(detail:)` / `.connecting(detail:)`。
- `SessionHost` 发布的是**裁决本身**（`orchestrationDecision`），不是预先格式化好的
  字符串 —— 这样「锁被别人占着 → 界面必须是错误态」才是一条跑得出来的测试。
- `Sources/Mac/Views/OrchestrationNoticeBar.swift` **里面没有判断**，只负责画；
  挂在 `MacThreePaneView` 的三栏之上，正常编排时渲染成空、不占一个像素。
- 顺带把 `ViewerSessionClient.isConnected` / `.lastError` 接了出来 ——
  它们在这之前**全仓零消费者**，于是「总闸=daemon 的 viewer 断线」这一态屏幕上
  一直什么都没有。
- ⚠️ **横幅本身没有被人眼看过。** GUI 验不了（不许为验证开窗口），能证的只有
  「状态到没到界面层」，**不是「它长什么样」**。这两件别混。

---

## 6. 判据

**两趟全量都跑了**（CONTRIBUTING：一趟绿是一个样本，不是结论）：

| | 结果 | 具名失败 | passed | skip |
|---|---|---|---|---|
| 第一趟 | `** TEST SUCCEEDED **` | **0** | 1752 | **6** |
| 第二趟 | `** TEST SUCCEEDED **` | **0** | 1755 | **3** |

第二趟那 3 条 skip 就是判据要的那三条，逐条：

1. `AgentTuiFixtureRecorder.testRecord`
2. `CrewLastMessageCacheTests.test_基准_现场白板目录`
3. `SessionAwaitingReplyInputsCacheTests.test_基准_现场目录`

### ⚠️ 判据要补两个前提 —— 今天两个各被违反了一次

「`skip == 3`」这条尺子**从来没写自己成立的条件**：

1. **fixture 在场** —— 新 worktree 里没有那份 gitignore 的真实群聊 fixture
   （`Tests/PendingCrewTests/Fixtures/LEDDriverCrew`），`CrewChatOpenCostTests`
   那 8 条会 skip。**我最初那趟基线量到的就是 skip 11。** 修法：从主 checkout
   `cp -R` 那个目录过来（gitignored，不会进提交）。
2. **真白板目录读得动** —— `CrewMentionFilterRealWhiteboardTests` 那 3 条读
   `~/Library/Application Support/PendingCrew/whiteboards/`，读不到就 skip。
   **第一趟（skip 6）就是撞上了这条**，第二趟那个条件自己好了 → 回到 skip 3。

**这两个前提请写进判据。** 一把没有前置条件的尺子，会让下一个人在新 worktree 里
量到 11 而当成回归。

### 关于基线 1679

**别拿 1755 减 1679 下结论。** 我不知道 1679 是在哪一次跑上量的（哪个 commit、
fixture 在不在场、真白板读不读得动），三者任一不同这个数就不同。要证「非本刀的类
一条不少」，用**测试类名集合**比对，比数字硬。**这份比对我没做完**，见待办。

---

## 7. 剩下的待办（逐条）

1. **报段**（机长排队用）。两样已经备好：
   - **动了哪些段**：`Sources/Mac/LocalRunner/`（新增 `OrchestrationGate`、
     新增 `SessionOrchestratorLock`、`ProcessRole` 改名、`SessionDaemonHost`）、
     `Sources/Mac/Services/`（`SessionHost`、四个 center 的 precondition、
     `CrewSessionRunner` 的合并冲突解法）、`Sources/Mac/Views/`（`MacRootView`
     挂横幅、新增 `OrchestrationNoticeBar`）、`Sources/Stores/`（新增
     `PendingCrewDataRoot` + 六个 store 改接线、`CrewStore` 一行）、
     新增 `Sources/Support/OrchestrationNotice.swift`、`Sources/PendingCrewEntry.swift`、
     `project.yml` + `pbxproj`、`docs/architecture.md`、`docs/internal/`。
   - **要不要占用 `main` 当基线**：**不要**（main 已合进分支）。
2. **「非本刀的类一条不少」的类名集合比对**（基线跑 vs 现在这趟）。两份日志都在
   scratchpad 里，但**那是我的私有产物，下一个人拿不到** —— 请重新跑一趟基线
   （`main` 上）和一趟分支，比类名集合。
3. **约束 3 那趟「真把默认解析改坏、看它红」没跑。** 测试写成了会红的形状
   （断言的是**字面路径**不是自证），但**这条按「形状对」记，不按「验过了」记**。
   要补就把 `PendingCrewDataRoot.resolve` 的默认分支改坏跑一次
   `-only-testing:PendingCrewTests/PendingCrewDataRootTests`。
4. **A2 手工三条**（`docs/internal/2026-08-19-backend-split-manual-checks.md`）——
   机长推，不用接手的人管。**注意它是五节不是三节**（§0 前置自检 + §1/2/3 + §4）。
5. **`PENDINGCREW_DATA_DIR` 做完了但没拿它跑过真 daemon。** 要冒烟测 daemon，
   **先核启动日志那一行**（`数据根 = ...`）确认真挪走了再动手。
   ⚠️ 前任在这儿翻过车：`HOME=<临时目录>` **改不动 `Application Support`**
   （macOS 按用户记录解析，不看 `$HOME`），于是 daemon 真跑在了人的真数据目录上，
   和 app 并存 27 秒。**这台机器上没有第二次机会。**
6. **落 main 前把根目录这个 `P4-HANDOFF.md` 删掉。**

---

## 8. 白板通道故障（已交给机长，别接手查）

2026-08-27 00:00 前后，**本 session** 的 `post_to_crew` / `read_whiteboard` 全部报
`… couldn't be opened because you don't have permission to view it`。

**已经排除的（都是量的，不是推的）**：

- **不是装包** —— `/Applications/PendingCrew.app` 仍自报 `0.1.15`、bundle mtime
  `Aug 26 08:29`，0.1.17 根本没装。
- **不是文件坏了** —— `whiteboards/` 目录 mtime 一直在跳，别的进程正常写；
  机长（同机另一条 Claude Code）读写都正常。
- **不是全机** —— 同一个数据根下 `local-crews.json` / `attachments/` / `backups/`
  我都读得动，只有 `whiteboards/` 这一层被挡。
- **不是 `.claude/settings*.json` 的 deny，也不是 app 自己加的沙箱**
  （仓库里 `sandbox-exec` / `sandbox_init` / `seatbelt` 在实现代码里零命中）。

**没查出根因**，机长接手了。**它自己好了一次**：第一趟全量因为它 skip 3 条，
第二趟那 3 条又 passed 了 —— 所以它是**间歇的**，不是永久的。

**这件事的副产品值得记一笔**：那份逐跳链先是在群聊里被截断（机长只收到第 7、8 跳），
接着整条通道断了。**结论落在仓库里比落在群聊里耐得住** —— 这也是为什么报告在
`docs/internal/` 而不是在群里。

---

## 9. 接手第一件事

```
git log --oneline -9          # 核你手上是不是 a4f7bfa … 7417378 这几笔
git status --short            # 应当是空的
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'generic/platform=iOS Simulator' build
```

**iOS 那条别跳过。** `f26b9d0` 就是漏编 iOS 攒出来的：macOS 编得过、测全绿，
只有 iOS 红，而它从 `5284ba6` 起一直红着没人知道。
