# 群聊消息重放 —— 投递面与游标账（人类 Todo #105 第一阶段：量，不修）

> 口径来自 4-1：**先量清有几个投递面、各自读写哪个游标，再谈修；不许只在渲染层去重
> 把重复藏起来。** 本文只做第一件事。三层分开写：**实测到的 / 读代码读到的 / 推出来的**。

## 0. 一句话结论

重放不是一个 bug，是**三个互不相干的机制**，共同的病根是：
**「这条消息投过了吗」有三本账，两本在内存里，没有任何东西对账。**

---

## 1. 投递面清单（读代码读到的）

能把白板内容送进一个 claude session 的路，共 **6 条**：

| # | 投递面 | 代码位置 | 读哪个游标 | 推进它吗 |
|---|---|---|---|---|
| S1 | PostToolUse hook（每次工具调用） | `Sources/Mcp/McpHelperMain.swift:76` → `HookEmitter.emitAndAdvance` | C1 | ✅ |
| S2 | 开场 prompt 的首轮注入 | `Sources/Mac/LocalRunner/LocalSessionLaunch.swift:20` → `emitContextAndAdvance` | C1 | ✅ |
| S3 | codex `turn/start` 两阶段 | `Sources/Mac/Services/CrewSessionRunner.swift:1415` → `prepareContext`/`commit` | C1 | ✅（受理后） |
| S4 | 定向 @ 唤醒注入 | `Sources/Mac/Services/CrewLocalMentionWaker.swift:143-207` | C2 决定投什么；C1 只在回执确认后被推 | 部分 |
| S5 | 缺席目标拉起时的 `wakeText` | `CrewSessionRunner.startCaptain(wakeText:)` `:2055`／`restartMember` `:2289` | **不碰任何游标** | ❌ |
| S6 | `listen` 收听 | `CrewSessionRunner.swift:714-726` | C3 | ✅（仅 C3） |

游标（= 「已投递」账）共 **3 本**：

| | 账 | 位置 | 活多久 |
|---|---|---|---|
| C1 | per-session 盘上游标 `<crewId>.<sessionId>.cursor` | `Sources/Mcp/WhiteboardCursor.swift` | 永久（按 sessionId） |
| C2 | per-crew 扫描游标 `CrewLocalMentionWaker.cursors` | 内存 | 随 app 进程；启动时钉到白板当前尾 |
| C3 | per-crew `listenCursors` | 内存 | 随 app 进程 |

另有一道 **C4**：唤醒队列自己的去重集 `CrewDeferredWakeQueue.deliveredKeys`（内存，上限 512 条 key）。
它按 `sourceKey|target:<sessionId>` 去重，**只认自己发过的**，对 C1 一无所知。

> **单一事实源的缺口就在这里**：一条消息可以同时在 **C1 里「已投」** 和 **C4 里「未投」**。
> 没有任何代码把这两本账放在一起看。

---

## 2. 重放机制 R1：换了 sessionId 就等于「从没投递过」

### 读代码读到的

- 机长的 sessionId **每次启动都新造**：`CrewSessionRunner.swift:2113`
  `let localSessionId = "captain-" + String(UUID().uuidString.lowercased().prefix(8))`
- 而**对话是接回来的**：`:2141-2150` 查 `LocalAgentSessionStore.latestCaptainRecord`
  拿 agent 会话号，带 `--resume` 起。
- C1 的文件名是 `<crewId>.<sessionId>.cursor` → 新 sessionId ⇒ 文件不存在 ⇒
  `WhiteboardCursor.read() == .absent` ⇒ `unread()` 返回 `all.suffix(30)`
  （`WhiteboardCursor.swift:56,104`，`firstDeliveryLimit = 30`）。

**⇒ 对话记得，游标不记得。** 续跑回来的机长被重新灌进它上一世已经读过、已经回过的最近 30 条。

`:2126-2133` 的注释把这写成了**有意为之**：

> 不动 id 还顺手躲开一个硬伤：复用旧 localSessionId = 复用旧的白板读游标，机长隔几天
> 醒来会被一次性灌进几百条未读……新 id → 游标 `.absent` → 只投最近一批。

那时要解的是「几百条洪水」，解法是把游标清零。**清零挡住了洪水，也造出了重放**：
它把「隔几天醒来」和「三分钟前刚被重启」当成同一种处境。

对照组：worker 走 `restartMember`，**复用 `member.sessionId`**（`:2257`），游标延续 —— 所以
重放的抱怨全来自机长，不是巧合。

### 实测到的

```
本机 captain 游标文件 408 个，涉及 47 个 crew  ⇒ 「非首任」机长 361 次
每次上限 30 条                                 ⇒ 重放上限 10,830 条·次
按创建日：2026-09-05 = 63 个，2026-09-06 = 38 个
crew 4（PendingCrew）一个群就有 50 个
```

**4-1 那次，两头的游标值都量到了：**

```
14:16:03Z  captain-17bf5b87 游标 = 786568ed（白板 #1933，重启前最后一条）
14:27:38Z  后台重启，3 个 session 被中断（白板 #1934 f8fec21b）
14:28:18Z  captain-5e324881 到岗，无游标文件 ⇒ .absent ⇒ 重投 #1904–#1933
           这 30 条时间跨度 11:16:57Z – 14:16:03Z（约 3 小时），
           且**全部**落在 17bf5b87 已消费的范围内
```

**我自己这一轮同样量到：** 本 crew 白板 476 条；`captain-479e6b5d` 拿到的注入块
**正好是最后 30 条**，其中 29 条是 2026-09-05 的、包含我自己发的多条。

---

## 3. 重放机制 R2：`wakeText` 走的是一条不记账的路

### 读代码读到的

@ 一个没在跑的目标 → `CrewLocalMentionWaker.wakeAbsent` → `startCaptain(wakeText:)`
/ `restartMember(wakeText:)`。消息正文被**烤进开场 prompt**
（`CaptainBriefDelivery.openingPrompt` / `:2289` 的 `"有人在群里 @ 你：「\(wakeText)」"`）。

这条路**一个游标都不碰**。而同一条消息在白板上仍是未读，于是紧接着的 S2
（`initialPromptWithWhiteboard`）把它**再渲染一遍**。

**⇒ 这不是竞态，是必然：每一次「@ 一个睡着的目标」都产生两份。**

### 实测到的

4-1 于 15:01:33Z 发的 `e416d050`，在我这一轮的 prompt 里出现 **两次**：
一次在世界观的「刚有人在群里 @ 你」，一次在 `<external_crew_whiteboard>` 未读块末尾。

---

## 4. 重放机制 R3：队列里存的是一份渲染好的死字符串

**这条正面回答 4-1 的问题「投递的时候取的到底是哪个快照」。**

### 读代码读到的

- `CrewLocalMentionWaker.deliver` 在**扫描那一刻**把正文 + 最近 15 条上下文
  渲染成 `inj.text`（`:176-182`）。
- 交给 `deliverOrDeferWake` → `CrewDeferredWakeQueue.Delivery(key:targetSessionId:text:)`
  —— **存的是 `text`，一个字符串**（`CrewDeferredWakeQueue.swift:10-14`）。
- 目标忙 → `.deferred`，压在内存队列里，等 `runBecameIdle` 再 `popWhenIdle` 发出去。
  **中间从不重读白板。**
- 同一段时间里，S1（hook）在目标的每次工具调用后触发，把同一条渲染进「未读」块
  **并推进 C1**。C4 对此一无所知。

**⇒ 快照取自「扫描那一刻」，此后再未刷新。**
它不是缓存了旧数据 —— 它压根就是一份写死的字符串。

顺带：`confirmWake` 若判失败，`consume` 不执行 ⇒ C1 不推进 ⇒ 同一条下一拍又被 S1 渲染一遍。
那是 fail-open 的刻意设计（宁可重投不丢），但它也是重放的一个来源。

### 实测到的（时间线对得上）

```
14:33:20Z  #1940 cd859615  44-1 报到，mentions=[captain]
14:33:38Z  #1941            4-1 发言   ← 在忙
14:34:02Z  #1942            4-1 发言（正是他说的「14:34 已经回过并当面纠正」）
14:35:18Z  #1944            4-1：「这条是重放」
```

### 推出来的（未直接观测）

14:33:20 那一刻 4-1 的 run 处于 `activityIsWorking` ⇒ 唤醒被 `.deferred`；
其间 hook 路已把 #1940 渲染进未读块并推进 C1；14:35 前后转 idle，队列弹出
**14:33:20 那一刻的字符串**投出 = 第二份。
**这一环是从时间戳推的，不是量的** —— 我没有当时的 `isBusy` 采样。

---

## 5. 反面：查过但**不**成立的

- **重复写入**：4-1 已实测人类那 8 条消息在白板上各只存在一份。我复核 crew 4：
  `347aac5` 出现 17 次全是不同消息的正文引用，44-1 报到 `cd859615` 只有一条。
  **⇒ 是重复投递，不是重复写入。**
- **「提这条的 session 已退出 → 转投机长」会多复制一份**：不成立。
  `CrewHumanTodoRespond.perform` 只写**一条**白板消息，mentions 换成
  `[broadcast, 机长]`（`TodoLandingFlow.mentions`），走的是同一条投递路，不另开通道。
  它照样受 R1/R2/R3 影响，但自己不产生副本。

---

## 6. 修的方向（**未动手**，等 4-1 / 人类过目）

按 4-1 的禁令：不许只在渲染层去重。四条都指向同一件事 —— **把「已投递」收成一本账**：

1. **R1**：机长的白板游标应当跟着**对话身份**走，不是跟着 sessionId 走。续跑时继承
   上一任的 C1（30 条上限保留，当「真的睡了几天」的护栏 —— 那是它原本要解的问题）。
2. **R2**：`wakeText` 那条路要么把该条在 C1 上标成已投，要么开场不带正文、只说
   「有人 @ 你，见未读」。**选一个，别两边都出。**
3. **R3**：队列存 **messageId**，不存渲染好的字符串；真要发时现取白板当前值，
   并先问一句「这条在目标 C1 上是不是已经过去了」。
4. **总账**：C4 与 C2 都不该是独立的「已投递」判据，投递前后一律以 C1 为准。

**判据（等做的时候钉红）**：同一条消息对同一个 session 只应被投递一次，除非它被显式再次 @。
