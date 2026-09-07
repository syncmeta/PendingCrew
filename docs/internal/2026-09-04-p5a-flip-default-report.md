# P5a 收尾：翻默认 + §9.2 防分裂降级契约（回报）

> **为什么这份东西在仓库里而不在群聊里**：2026-09-04 15:1x，本 session 的
> `post_to_crew` 又一次报「未能打开文件 `local-71b7fd5b-….json`，因为你没有查看它的权限」，
> 白板写入通道单向断了 —— 与 `2026-09-04-p5a-closed-loop-evidence.md` 开头记的是同一种故障
> 形状，**今天第三次**。同一趟里它还让 3 条读真白板目录的测试变成 skip（见下）。
> 结论落在仓库里比落在群聊里耐得住。

- 日期：2026-09-04
- 分支：`pendingcrew/session-971acc`
- 提交：`bdf92f3`（翻默认 + 契约）、`fb1b0ee`（降级顺序抽成可测的协调器 + 文档记账）
- 基线：main `7e7d11d`

---

## 一、做了什么

**总闸默认翻了。** `ProcessRole.resolve`：不设 `PENDINGCREW_BACKEND` = `.viewer`
（所有权在常驻后台，GUI 退化成 viewer）；`inproc` 变成**显式的回退开关**；
拼错的值按新默认走 —— 那是安全的那一侧（会有一个后台起来管账，而不是两个进程一起管）。

翻之前先把「后台起不来那一天」写死，实现分三层，每层各有一组先证明会红的测试：

| 层 | 代码 | 测试 |
|---|---|---|
| **表本身**（允不允许接管） | `OrchestrationFallback.decide`（纯判定） | `OrchestrationFallbackTests` |
| **顺序**（什么时候取锁 / 停腿 / 放锁） | `OrchestrationFallbackCoordinator` | `OrchestrationFallbackCoordinatorTests` |
| **界面态**（说出来没有） | `OrchestrationNotice` + `OrchestrationNoticeBar` | `OrchestrationNoticeTests` |

§9.2 表逐行：**唯一允许接管的一支 = 锁确实到手 + 拉 daemon 确实失败**；
锁被 daemon 占着 → 继续重连；`heldBy(nil)` 归属不明 / 非 daemon 占着 / 锁文件打不开
→ 一律 refuse；连上过的对端（协议不兼容 / 握手失败 / attach 失败）→ refuse，
**并且压过锁的观测** —— 能回话就说明那边有东西在，哪怕这一刻锁恰好到手也不许接管。

界面上新增 `.localFallback` / `.refused` 两态，且**裁决压过「连没连上」**：
归属不明不会自己变清楚，不许显示成「正在连接…」那种看着自己会好的样子。

`ProcessRole.effective` 认临时接管，并拆出纯判定版 —— 翻默认之后测试进程自己就是
viewer，原来那两条 `XCTSkipUnless(requested == .orchestrator)` 会**静默 skip**，
而它们守的正是「修一个双头顺手造出另一个」。

---

## 二、对契约的一处收窄（明写，请确认）

**附加约束 2「恢复连接之后不许残留第二个 host」，实现成「接管之后不交还，回后台模式的
走法是重开 app」。**

理由不是省事：

- 回退期间本进程已经在养真的 agent 子进程，**它们是这个进程的孩子，交不给 daemon**。
  半路把编排交还就是 §9.2 自己点名的「半停一半留 = 双头的另一种形状」。
- 而「不出现第二个 host」这条本身是**结构上**满足的，不靠谁记得去检查：接管时我们
  **持有编排锁**（任何 daemon 都会因此拒绝启动）并把 viewer 那条腿整个停掉，
  所以「本地编排 + 连上的 daemon」这个状态压根到不了。

代价：「重试」退化成「重开 app」，横幅上就是这么写的。要做**在线**交还，前提是先有
一条能把在跑的 agent 子进程交接出去的路 —— 那是另一期。同样记在设计 §9.2.1。

---

## 三、验收

### 全量（本 worktree 实跑）

```
Executed 1897 tests, with 14 tests skipped and 0 failures (0 unexpected)
** TEST SUCCEEDED **
```

macOS 与 iOS Simulator 两端 `** BUILD SUCCEEDED **`。

按「只看执行数与失败数」的口径对账：main `7e7d11d` 是 `Executed 1875`，
**1897 − 1875 = 22**，正好等于新增的 22 条（契约表 11 + 界面态 4 + 默认翻转 2 +
`effective` 1 + 协调器 4）。

**14 个 skip 逐条，一条都不是新增的**：

- 3 条 known（录制器 + 两条「现场目录」基准）
- 8 条 `CrewChatOpenCostTests` —— 要那份 gitignored 的真群聊 fixture，本 worktree 没有
- 3 条 `CrewMentionFilterRealWhiteboardTests` —— 读真白板目录，而本进程此刻读不了
  （与本文开头那条 `post_to_crew` 故障是同一件事）

### 两趟红的原始输出

**① 契约本体**（实现之前，故意先写成「连不上就自己接管」那条错路）：

```
OrchestrationFallbackTests: 归属不明 = 不许接管（读不出是谁≠没有人），实际：takeOverLocally(…)
OrchestrationFallbackTests: 锁被 daemon 占着还接管 = 当场双头，实际：takeOverLocally(…)
OrchestrationFallbackTests: 「协议不兼容」这一行不许接管 / 「握手失败」… / 「attach 失败」…
OrchestrationNoticeTests:   临时接管必须在界面上说出来，实际：connecting(…)
OrchestrationNoticeTests:   禁止接管必须给可操作错误，实际：connecting(…)
ProcessRoleTests:           XCTAssertEqual failed: ("orchestrator") is not equal to ("viewer")
	 Executed 29 tests, with 20 failures (0 unexpected) in 1.046 seconds
```

**② 顺序那一段**（把取锁改成无条件 —— 那正是最诱人的写法，也正是会让我们自己刚拉
起来的 daemon 因锁被占而退出的那个 bug）：

```
Tests/PendingCrewTests/OrchestrationFallbackCoordinatorTests.swift:58: 这一步根本不该去碰锁 —— 抢了它，我们刚拉起来的 daemon 就起不来了
Tests/PendingCrewTests/OrchestrationFallbackCoordinatorTests.swift:72: XCTAssertEqual failed: ("1") is not equal to ("0")
	 Executed 4 tests, with 2 failures (0 unexpected)
```

还原后 4 条转绿。

### 翻默认之后的闭环复跑

见 `2026-09-04-p5a-closed-loop-evidence.md` 第六节（claude + codex 各一条，全程零 nudge，
纯 CLI、隔离数据根、跑完已删，未触碰真数据目录）。

---

## 四、没验到的边界

1. **降级契约那条路没在真机上走过**：三种结局（临时接管 / 继续重连 / 拒绝接管）
   全部只有单测覆盖。真机要造出「daemon 拉不起来」这个前提，得把二进制弄坏或把数据
   目录弄成不可写，本轮没做。
2. **GUI 那一半没验**：横幅只验到「态到没到界面层」（纯判定），像素与交互没验，
   也没有为验证开窗口。
3. **翻默认之后没走过一次「双击图标」的真实启动**：所有证据都来自 CLI 身份
   （`--daemon` / `--daemon-status` / `--daemon-attach`），viewer 自动拉起 daemon 那条路
   （`ViewerSessionClient.connect`）在真 GUI 里没跑过 —— 属安装态验收那一档。
4. 安装态的「关掉 / 更新 / 重开不断线」（A1 三条路径）不在本轮，需要人做。
