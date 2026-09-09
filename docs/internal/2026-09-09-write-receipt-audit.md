# 「回执说成功了，而那件事其实没发生」—— MCP 写盘工具全量审计

日期：2026-09-09
基准提交：**审计与实测跑在 `0d4a4fa`**（本 worktree 的起点）；
落 main 时合了 `7eb2b88`（当时的 main HEAD）。
**这两棵树的工具全集不同**，下面每个数都注明是哪一棵 —— 见 §6。
产出：`Sources/Support/WriteReceipt.swift`、`Tests/PendingCrewTests/McpWriteFailureHonestyTests.swift`，
以及 10 个 store / `McpServer` 的写路径改造。

---

## 0. 一句话结论

**病根不在任何一个工具里，在两条共用的写入漏斗上**：

| 漏斗 | 位置 | 修前的形状 |
|---|---|---|
| 账本整写 | `MultiProcessJSONStore.saveRowsLocked` | `try?` + 返回 `Void` |
| 控制通道入队 | `LocalCrewControlStore.enqueue` / `requestRename` / `writeAttention` | `try? data.write` + 返回 `Void` |

（每条漏斗底下挂着多少个工具，见 §4.2 的变异表 —— **那里的数是量出来的**，
所以本节不另给一份，避免这份报告自带两份互相不对账的名单。）

这个仓库的**读**那一侧早就是 fail-closed 的（`.unreadable`、拒空写闸、两次复验才归档，
全是 2026-08-12 那次事故换来的）。**写这一侧一直是敞开的。** 这个不对称就是全部。

`report_to_parent` 之所以是最阴的一个，不是因为它的代码更差 —— 它和另外十几个工具
走的是**同一行** `try?`。它只是碰巧是子 crew 唯一的向上通道。

---

## 0.5 这不是历史包袱，是仍在生产的缺陷

三个作为起点的现场（`report_to_parent` 故障窗口 0 落盘、`respond_todo` 写错账本、
`reply_to` 旧 helper 不 @ 人）**都在老代码里**，读起来像一堆等着被清理的旧账。

**第四个现场推翻了这个读法**：「机组群聊体验」在 2026-09-08~09 之间**新写的**分条发送
路径上，`Entry.args` 只带那一项自己的字典，顶层的 `mentions` / `reply_to` /
`attachments` 一个字都不会跟过去 —— 而回执照回「已发到」。
那条已经被它自己堵掉了（顶层给了就整批拒），**但值钱的不是修法，是日期**。

**结论直接影响这一单三件事的权重**：修掉现存那 28 条只是清账，
**§4 那把尺子才是让这个形状停止再生产的那一半。**
名单会过期，漏斗会新增，而全集闸对着 `tools/list` 现算 —— 下一个人加工具时它还在。

（第五个现场发生在本单进行中：机长 10:47 在群里说「已替某 worker 按下 Enter、正在续跑」，
信的是 `nudge_session` 的回执；54 分钟后查 transcript，那个 session 最后一次写盘停在 10:30。
**回执说动了，那件事没发生。** 这一条的机制是别的（PTY 那侧），不在本单范围，
但它说明这个形状不止一条通道。）

---

## 1. 全集是怎么得到的（判据：可复算）

不是凭印象列，是从 `McpServer` 的真注册表出发，两条独立路径**互相对账**：

1. `tools/list` 的应答里 `"name"` 字段 → 39 个（worker 面 ∪ 机长面，合并去重）
2. `handleToolCall` 的 `case "..."` 分派 → 39 个
3. 两个集合**双向差集都为空**（`comm -23` / `comm -13` 均无输出）

这道对账现在长在测试里（`test_全集闸_名单与真注册表逐字对齐`），**两个方向都断**：
少了（加了工具没表态）红，多了（删了工具名单没删）也红。
只断一个方向正是今天全组栽过四次的那个形状。

按「写不写跨进程共享文件」分：

| 分类 | 条数 |
|---|---|
| 写共享文件、**且被尺子直接量过** | 28 |
| 写共享文件、但这把尺子造不出它的写失败（见 §7.1 ①） | 4 |
| 只读 | 7 |
| **合计** | **39** |

**注意 28 和 32 的区别**（这是本报告最容易读错的一处）：
会写盘的是 **32** 条，尺子直接量到的是其中 **28** 条。
后文说「28」时一律指后者。

---

## 2. 名单（39 条，全 —— 这是 `0d4a4fa` 那棵树；合 main 后是 38，见 §6）

「修前回执」列是**实测**：在只读数据目录下真调一遍工具，抄回执原文（`/tmp/ruler-before-fix.log`）。

### 2.1 写共享文件的 28 条

| 工具 | 写什么 | 修前：写失败时回什么 | 修前：调用方分得出来吗 |
|---|---|---|---|
| `report_to_parent` | `<crewId>.<uuid>.crewcmd.json` | 「已提交向上汇报。」 | ❌ |
| `message_child_crew` | 同上 | 「已提交给子 crew「X」的消息。」 | ❌ |
| `start_session` | 同上 | 「已安排起 session：X。」 | ❌ |
| `handoff_captain_to_session` | 同上 | 「机长交接请求已受理」 | ❌ |
| `create_and_handoff_captain` | 同上 | 「新机长交接请求已受理」 | ❌ |
| `create_child_crew` | 同上 | 「已安排建子 crew。」 | ❌ |
| `create_parent_crew` | 同上 | 「已安排在本 crew 头上新建父 crew。」 | ❌ |
| `adopt_crew` | 同上 | 「已提交收编「X」。」 | ❌ |
| `adopt_parent` | 同上 | 「已提交认「X」为父 crew。」 | ❌ |
| `release_crew` | 同上 | 「已提交把直系子「X」摘出到顶层。」 | ❌ |
| `schedule_wakeup` | 同上 | 「已设定时唤醒：<ISO>。」 | ❌ |
| `listen` | 同上 | 「已开启群聊收听至 <ISO>」 | ❌ |
| `set_session_profile` | 同上 | 「已排队切换：effort→high。」 | ❌ |
| `inspect_session` | 同上 + long-poll | 只返回 id → 之后「超时无应答」 | ❌ **且指错病因**（说 app 没在跑） |
| `nudge_session` | 同上 | 同上 | ❌ |
| `stop_session` | 同上 | 同上 | ❌ |
| `change_workdir` | 同上 | 同上（超时预算 12 倍，等更久） | ❌ |
| `rename_crew` | `<crewId>.crewmeta.json` | 「已把 crew 改名为「X」。」 | ❌ |
| `raise_attention` | `<crewId>.crewattention.json` | 「已记录兼容 attention 文案：X。」 | ❌ |
| `clear_attention` | 同上 | 「已清除兼容 attention 文案」 | ❌ |
| `respond_todo` | `<crewId>.todos.json` | 「已回应 Todo #1（状态：待办）。」 | ❌ |
| `plan_add` | `<crewId>.plan.json` | 「已排上 计划 #2：X（没做）。」 | ❌ |
| `plan_update` | 同上 | 「计划 #1：X → 没做 · 最后更新 刚刚」 | ❌ |
| `answer_decision` | `<crewId>.approvals.json` | 「已答复，发起的 session 将继续。」 | ❌ |
| `confirm_todo_sweep` | `<crewId>.todo-sweep.json` | 「记下了：1 条未完成已逐条归桶。」 | ❌ |
| `continue_work` | `session-continuations.json` | 「已登记本轮一次性续跑」 | ❌ |
| `add_human_todo` | `<crewId>.human-todos.json` + 白板 | 「**已记入**人类 Todo #2 …⚠️ 但群里那行没发出去」 | ⚠️ **半对**：说了群消息没发，但「已记入」是假的（账也没落） |
| `withdraw_human_todo` | 同上 | 「**已撤回**人类 Todo #1 …⚠️ 但群里那行没发出去」 | ⚠️ 同上 |
| `post_to_crew` | `<crewId>.json`（白板） | 「ERROR: 没能写进 crew 群聊白板 …请当作未送达处理」 | ✅ 本来就诚实 |
| `contact` | 目标 crew 的 `<crewId>.json` | 「ERROR: 没能写进 X 的群聊白板 …」 | ✅ 本来就诚实 |
| `ask` | approvals + 白板 | 「ERROR: 问题没能贴到 crew 群聊白板 …」 | ✅ 本来就诚实 |
| `arrange_crews` | `crew-arrangement.json` | 「ERROR: 排布没写进去（磁盘写失败）。」 | ✅ 本来就诚实 |

> 上表 32 行 = 全部会写盘的工具。其中 4 行（`inspect/nudge/stop/change_workdir`）
> 在尺子里归 `.uncovered`（见 §7.1 ①），但**仍然在修复范围内**。

### 2.2 只读的 7 条

| 工具 | 读什么 |
|---|---|
| `directory` | `local-crews.json` + `crew-sessions.json` |
| `read_whiteboard` | 白板 |
| `search_whiteboard` | 白板 |
| `get_quota` | `quota.json` |
| `plan_list` | `<crewId>.plan.json` |
| `list_sessions` | 点名快照 + 会话号账本 + `~/.claude`、`~/.codex` 取证面 |
| `crew_ordering_signals` | 组织树 + 白板 + viewed 镜像 |

这 7 条不在诚实闸里，但它们的**读**失败早就是 fail-closed 的
（`directory` 明确拒绝把读不出来渲染成空表；`crew_ordering_signals` 把
「读不出来」和「确实一条都没有」分成两态）。这一侧不是这一单的问题面。

---

## 3. 修法（照抄仓库既有的正确孪生，不发明第二种）

仓库里本来就有三个正确范式，全部沿用：

1. `LocalWhiteboardStore.appendSessionMessageReportingFailure` —— throwing 变体给「回执必须如实」的调用点
2. `MultiProcessJSONStore.loadRowsLocked(onIncident:)` —— 用回调把事故递给关心的调用方
3. `CrewArrangementStore.save -> Bool` —— 写入口返回成败

具体：

- **`saveRowsLocked` 从 `Void` 改成返回 `Error?`**（nil = 真落盘了），保留 `@discardableResult`
  —— 忽略必须是一次显式选择，而不是这一层根本说不出话。
- **各 store 的 `saveLocked` 跟着返回 `Error?`**，并且**写失败时不发变更信号**
  （发了会让界面重读一份没变的文件，把「刷新过了」误当成「改动生效了」）。
- **mutator 的表达方式按各自已有的形状走**，不统一成一种：
  - 返回 `Item?` 的（`add` / `respond`）加 `onWriteFailure:` 回调（范式 2）
  - 已有富 outcome 的（`WithdrawOutcome` / `UpdateFailure`）加 `.notWritten` 分支
  - 返回 `Bool` 的（`delete` / `setDismissed` / `arm` / `register`）改成写失败即 false
- **`LocalCrewControlStore` 全部写入口返回 `Error?`**；四个要 long-poll 的改成返回
  `EnqueuedCommand(id:failure:)` —— 只返回 id 是这一族的另一个入口：
  拿到 id 就以为排上了，然后对着一条不存在的命令等到超时，而超时那句话把病因指向「app 没在跑」。
- **`McpServer` 每条受影响的回执**改成先判 failure，失败走
  `WriteReceipt.notWritten(what:error:consequence:)`。`consequence` 必填，
  因为**只说「失败了」agent 会瞎重试**，说清「上级那边什么都没有」它才知道要换条路。

另外顺手修掉一处独立缺陷：`confirm_todo_sweep` 用的是 `CaptainTodoSweepStore.shared`
（指向真实数据根），**单测一跑就把确认写进人的数据目录**，而用例自己读临时目录。
改成注入。这与本单主题无关，但它挡住了对这个工具的任何验证。

---

## 4. 尺子（`McpWriteFailureHonestyTests`，5 条）

两半，缺一不可：

- **全集闸**（2 条）：名单对着 `tools/list` 真注册表双向断 + 条数断。
  新加工具而不表态 → 红。这是「下一个人加工具时谁拦住他」的那道门。
- **诚实闸**（2 条）：
  - 写失败时回执**必须**带 `WriteReceipt.notWrittenMarker`（`【没写进去】`）
  - 写成功时**必须不**带 —— 没有这一半，前一半可以被「到处硬写记号」满足，
    而一把永远红的尺子和一把永远绿的一样没用
- **尺子自证**（1 条）：现造一条「写没成、回执像成功」的字符串喂给同一判据，确认它被抓住。

**写失败是造出来的，不是等来的**：数据根与白板目录 `chmod 0o500`，
原子写要在同目录建临时文件再 rename，两样都被拒 → 每一条 `data.write` 都真抛错；
已存在文件的读不受影响（0o500 保留 r-x），所以量到的确实是**写**失败。

路径全部经生产代码自己的常量推导（`TodoLedger.fileSuffix` 等），一个都不手拼 ——
造错对象的结果是「不红」，而不红跟「已经修好了」长得一模一样。

### 4.1 先证明它会红

**修之前跑同一把尺子**：`test_诚实闸_写失败的回执必须说没写进去` 红，
一次列出 **28** 个不合格的工具（= 尺子覆盖的全部写工具，一条不落）（全日志 `/tmp/ruler-before-fix.log`，摘要见 §2 表格「修前」列）。

### 4.2 变异自证（逐项做，不是一趟跑完就信）

删掉某一处修复 → 尺子是否红、抓到哪些工具：

| 变异（把修复删掉） | 结果 | 抓到 |
|---|---|---|
| `saveRowsLocked` 吞掉错误返回 nil | 🔴 | `answer_decision` `confirm_todo_sweep` `continue_work` `plan_add` `plan_update` `respond_todo`（6） |
| `enqueue` 改回 `try? write` | 🔴 | `adopt_crew` `adopt_parent` `create_and_handoff_captain` `create_child_crew` `create_parent_crew` `handoff_captain_to_session` `listen` `message_child_crew` `release_crew` `report_to_parent` `schedule_wakeup` `set_session_profile` `start_session`（13） |
| `requestRename` 改回 `try?` | 🔴 | `rename_crew`（1） |
| `writeAttention` 改回 `try?` | 🔴 | `raise_attention` `clear_attention`（2） |
| 白板 append 改回 `try?` | 🔴 | `post_to_crew` `contact`（2） |
| `CrewArrangementStore.save` catch 里返回 true | 🔴 | `arrange_crews`（1） |
| **`saveRowsLocked` + 白板 append 同时** | 🔴 | 上面 6 + 2，**外加** `ask` `add_human_todo` `withdraw_human_todo`（3） |

6+13+1+2+2+1+3 = **28**，与 `.writesSharedFile` 的条数一致。逐个数过。

**最后一行是这次变异测试里唯一有信息量的那一条，必须留在记录里**：

`ask` / `add_human_todo` / `withdraw_human_todo` 各写**两个**地方（账本 + 白板）。
只变异其中一处时，**另一处仍然诚实地报了失败、回执照样带着记号，尺子全绿** ——
于是「这个工具被测到了」和「这个工具没被测到」长得一模一样。
这正是「链上更早（或更晚）的短路会挡住变异并伪装成全绿」的实例。
只有让两条路同时不成立，那三条才现形。

---

## 6. 合并进 main 之后，数变了（这一节存在的理由）

合并时 main 已经把 **`answer_decision` 整个删掉**了（驾驶舱计划 #75 ①：
`ask` 不再产生待决策也不再 long-poll，那个工具变成一个永远找不到目标的工具）。
同一笔改动还把 `ask` 从「写 approvals + 阻塞」改成「写人类 Todo + 不阻塞」。

所以：

| | 审计那棵树（`0d4a4fa`） | 落 main 那棵树（合 `7eb2b88` 后） |
|---|---|---|
| 工具总数 | 39 | **38** |
| 写共享文件 | 32 | **31** |
| 尺子直接量到 | 28 | **27** |

**前面 §1–§4 的每个数都是在 `0d4a4fa` 上量的，我没有回去改它们** ——
改了就看不见这个变化，而这个变化本身正是这一单结论的证据：
**工具集会动，手写名单会过期，只有对着 `tools/list` 现算的全集闸不会。**
合并当天它就用上了：`answer_decision` 一没，全集闸立刻红，逼我把名单跟着改。

合并后在新树上重跑尺子：**5 条全绿**。
`ask` 的写入口换成了 `humanTodos.add`，诚实性跟着换到 `onWriteFailure:` 那条路上，
仍然带记号。

---

## 7. 边界（这一栏必须写满）

### 7.1 我没能覆盖的

**① 四个 long-poll 工具的回执没被尺子直接量过。**
`inspect_session` / `nudge_session` / `stop_session` / `change_workdir` 入队后要
long-poll 等 app 侧应答，而尺子里 app 不在跑，每次固定等满超时预算。
**它们的修复做了**（改成 `EnqueuedCommand`，入队失败当场返回不再 long-poll），
但「回执如实」这半在它们身上只有**代码路径同构**的推理（与 `report_to_parent`
共用 `enqueue`），没有实测。它们在尺子里标 `.uncovered` 而不是从名单消失。

**② 「写成功了但写错了对象」这一类完全不在范围内。**
现场②（`respond_todo` 指着人类那本的 #N 调用、写的是 agent 那本、回执照说成功）
**没有被这一单修掉**。这一单修的是「写失败被报成成功」；
写错账本时那次写是**成功的**，尺子照定义不会红。
我只在 `respond_todo` 找不到 #N 的错误文案里加了一句提示（两本账号码会撞，核对一下）——
那是缓解，不是修复。**要真修它需要另一种判据**（号码带账本前缀，或工具要求显式指定账本）。

**③ 「调用方给的参数被悄悄丢掉」这一类不在范围内**（机长在群里提出的扩展）。
它和本单是同一形状的两种表现（调用方给了东西、系统没用上、回执说一切正常），
但需要的判据完全不同：不是造写失败，而是**落盘后逐参数回读比对**。
混进这一单会让「列全」失去可验证性 —— 那时的全集不再是「39 个工具」，
而是「39 个工具 × 每个的参数」，而参数集合没有一个像 `tools/list` 那样的单一真源。
**建议单开一单**，判据形状：对每个工具的每个 schema 参数，构造一次调用，
落盘后回读，断言那个值出现在磁盘上。

**④ 非 MCP 面的写路径没有全面审。**
本单只审 MCP 工具面。app 内部还有若干 `saveRowsLocked` 调用点在这次改造后
**返回值被丢弃**（`SessionContinuationStore.finishTurn`、`LocalAgentSessionStore`、
`LocalWakeupStore.remove`、`LocalWhiteboardStore` 的重建路径、
`LocalCaptainReassignmentStore` 的 drain）。它们现在**有能力**报告失败但没有调用方去接。
`@discardableResult` 让这成为一次显式选择而不是一层哑巴，但**它们仍然是静默的**。

**⑤ 我造的写失败，和 2026-09-08 现场量到的那次故障，方向相反。这条最要紧。**

机长在故障当口实测（`docs/internal/2026-09-08-eperm-live-capture.md`，commit `7eb2b88`），
同一目录同一秒的允许/拒绝集是：

| 允许 | 拒绝 |
|---|---|
| 列目录、取元数据、**整份创建/覆盖一个新文件**、删除 | 按**只读**打开、按**追加**打开 |

**也就是说：那次故障里写是通的，读是断的。** 而我这把尺子造的是反过来的一半
（`chmod 0o500`：读通、写断）。两者都真实，但**不是同一种事故**。

由此产生三条必须说清的话：

1. **这一单修的是「写失败被报成成功」，它不覆盖 2026-09-08 那个形状。**
   在那个形状下 `enqueue` 的原子整写多半是**成功**的，所以我改的那些回执**不会触发**。
2. **那个形状下的诚实性靠的是既有的读侧 fail-closed**（`.unreadable` / 拒空写闸 /
   `drainCommands` 读不出来就原地留着下个 tick 再来），不是这一单的产出。
   机长那两条工具当场如实报错，靠的是那半。
3. **所以现场①（`report_to_parent` 那 52 分钟）到底是不是被这一单修掉了，我不能说是。**
   在那个形状下命令文件多半写进去了、只是 app 侧读不出来因而**推迟**投递 ——
   「推迟」和「丢失」在父 crew 白板上都表现为 0 条。要区分它俩需要的是
   「命令文件在那段时间存不存在」这份证据，我没有。
   **我修的是同一族的另一半；把它说成「①已修」是我不该做的推断。**

**⑥ 只造出了一种写失败：权限（`EACCES`）。**
磁盘满（`ENOSPC`）、路径被占成目录（`EISDIR`）、只读文件系统（`EROFS`）没有实际造过。
判断它们会走同一条路的依据是：全部经由 `data.write(to:options:.atomic)` 抛 `NSError`，
而修复只判「抛没抛」不判 errno。**这是推理，不是实测。**

**⑦ 只在这个 worktree 里跑过全量。** worktree 全量：**2376 tests / 14 skipped / 0 failures**（`.test-archive/20260909T032702Z-*`）。
**worktree 的绿不算数** —— 落 main 后在共享主目录复跑一趟才算。

### 7.2 我的假设

- **测试进程不以 root 跑**。root 会绕过目录权限位，`chmod 0o500` 就造不出写失败，
  尺子会**全绿**而不是报错 —— 这是一个会静默失效的前提，写在这里。
- **flock 的 sidecar 文件在造失败前已经存在**（`seed` 保证）。若不存在，
  只读目录里 `open(O_CREAT)` 失败会退化成无锁执行，量到的就不是同一条路。
- **`tools/list` 的 worker 面 ∪ 机长面 = 全集**。依据是代码里 `tools` 数组
  先无条件填 worker 那批、再在 `if isCaptain` 里 append 机长那批，没有第三个条件分支。
- `WriteReceipt.notWrittenMarker` 是**唯一**抓手。措辞可以改，这个记号不许丢。
  换成正则去猜「这句话读起来像不像失败」就是又造一把措辞一变就静默失效的尺子。

### 7.3 一处我改了口径、说清楚

`WriteReceipt.notWrittenMarker` 最初的定义是「这次调用完全没写进去」。
写到 `add_human_todo`（账落了、群里那行没发出去）时改成了
**「这次调用要求写的东西没有全部落盘」** —— 半截也带记号。
理由：那一半确实没写进去，而 agent 需要因此改变行为（自己去群里补一句）。
正文负责说清哪一半成了；记号只负责让「有东西没写上」无法被略过。
