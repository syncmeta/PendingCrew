# 账读不出来时，核账提醒按退避叫（机长计划 #98）

基准：main `995ffa5`。分支 `pendingcrew/session-e6947a`。

## 一、15 秒是从哪来的

**是循环，不是提醒自带的间隔。** 提醒只由「机长从忙变闲」这个事件触发，没有定时器：

- `Sources/Mac/Services/CrewSessionRunner.swift:1650` —— `run.onBecameIdle` → `runBecameIdle`
- `Sources/Mac/Services/CrewSessionRunner.swift:339-356` —— `runBecameIdle` 在没有补投、没有 `continue_work` 时落到 `remindCaptainToSweepTodos`
- `Sources/Mac/Services/CrewSessionRunner.swift:454-466` —— 补投重试（0.5 秒）结束时也会走一次 `runBecameIdle`

于是：空闲 → 提醒 → 机长回一句（它交不出账）→ 又空闲 → 又提醒。间隔就是「机长回一句要多久」。

### 实测（读的是 claude 自己的对话记录，`~/.claude/projects/-Users-hey-Untitled-Pendingname-PendingCrew/*.jsonl`）

只数「用户消息里含『这本 Todo 账这次读不出来』、且不是 tool_result」的记录，按 uuid 去重：

| 记录 | 条数 | 时间（UTC） | 相邻两次间隔 中位 / p90 | 距上一条机长回复 中位 | 用的是旧文案 |
|---|---|---|---|---|---|
| c7d1b1e7 | 105 | 09-13 03:59:34 – 04:16:21 | 6.6 s / 12.6 s | 4.3 s | 105 / 105 |
| dda645e6 | 2958 | 09-11 17:00:05 – 09-13 01:29:32 | 9.0 s / 15.6 s | 5.6 s | 2957 / 2958 |
| c51cdab6 | 3426 | 09-11 16:49:41 – 09-13 01:28:10 | 12.8 s / 18.1 s | 4.9 s | 3426 / 3426 |

- 第一行的时间（本地 11:59–12:16）对得上「机组群聊体验」机长报的现场；另外两份是哪两个机长**没核**。
- 「距上一条机长回复 4–6 秒」就是循环的指纹：定时器不会恰好卡在对方说完之后。
- 「旧文案」= 含「群聊白板上应该有一条系统警示」。那句在 `6ac5718`（v0.1.36）删掉了。

## 二、地板间隔（`6786d13`）为什么没生效

**因为跑着的常驻后台里根本没有它。**

- 后台进程：`pid 53445`，`--daemon`，启动于 2026-09-12 00:14:26（`ps -o lstart`）。
- `lsof -p 53445` 的 txt 指向 `~/Library/Caches/com.pendingname.pendingcrew/org.sparkle-project.Sparkle/Installation/…/PendingCrew.app/Contents/MacOS/PendingCrew`，inode 184531536；`/Applications/PendingCrew.app` 里那份是 0.1.37，inode 187757457。
- `v0.1.35` 那笔提交是 09-12 02:51，晚于后台启动。所以后台的二进制不会比 0.1.34 新 —— 这一步是**推的**；旁证是上表里几乎全是旧文案。
- `v0.1.34` 的 `CaptainTodoSweepStore.row` 是 `withFileLock(crewId) { loadLocked(crewId) } ?? Row()`：账读不出来时 `lastRemindedAt` 恒为 nil，`decide` 里那道地板永远放行。
- 补这件事的两笔 —— `10f665f`（进程内退路）、`6786d13`（写进文件名）—— 都是 v0.1.36 才带上的。

所以在当前 main 上，这个循环已经不存在了；剩下的问题是**地板是固定的 15 分钟**，一窗 9 小时仍要叫 37 次。

## 三、改了什么

**口径**：读不出来时仍然提醒（不熄灭），但两次之间至少隔 `地板 × 1、1、2、4、8、8…`。地板仍是 `CaptainTodoSweep.minimumRemindInterval` = 15 分钟，所以间隔是 15 → 30 → 60 → 120 → 120… 分钟。读得回来的那一拍档位清零。

- **倍数没有另写一份**：`SupervisionLease.backoffMultiplier(step:)` 是从 `reschedule` 里原样抽出来的（`Sources/Models/SupervisionLease.swift:136`），`reschedule` 和核账都调它（`Sources/Support/CaptainTodoSweep.swift:269`）。有一条测试钉着 `CaptainTodoSweep.swift` 里不许出现 `pow(`。
- **封顶 8×（2 小时）的理由**沿用督办租约那条：不封顶的指数退避等于「叫几次没人理就永远闭嘴」。按 15 分钟的地板算，一窗 9 小时的故障叫 7 次（固定地板要叫 37 次，旧后台那种循环要叫几千次），任何时刻离上一次被问都不超过 2 小时。
- **档位怎么走**（`CaptainTodoSweep.nextUnreadableStreak`）：读得回来就归零；读不出来并且真叫出去了就 +1；读不出来但这一拍没叫，就不动。最后这条和 `reschedule` 里 `delivered` 的纪律一样：没叫出去不算叫过一次。
- **正文开头就说**「这是第 N 次提醒 —— 不是新情况」，以及「下一次最早在 X 分钟后（而且要等你再停下来一次才会问）」。
- **档位跟「上次提醒时刻」存在同样的三层**：盘上那本账、进程内一份、文件名标记。文件名标记是 `<crewId>.<ISO8601>.u<档位>.marker`，没有 `.u` 的旧文件名按 0 档算。`row()` 原来是按层兜底，现在改成**三份里取时刻最新的**；时刻相同时取档位小的，因为同一时刻只有「清零」会改档位。
- **顺手修的一个真 bug**：原来 `recordReminded` 是先 `loadLocked ?? Row()` 再整份写回。写走的是 rename，故障期间照样落得了盘，于是**故障期间每提醒一次，就拿一个空 `Row()` 盖掉那份读不出来的账**。机长交过的确认就这么丢了，故障一过，同一批条目又被问一遍。现在读的时候报了事故就不写盘，时刻和档位已经存在进程内和文件名里了（`Sources/Stores/CaptainTodoSweepStore.swift:224`）。
- 判定、档位、记账从 `@MainActor` 的 runner 里挪进了 `CaptainTodoSweepStore.idleTick`，这样「9 小时读不出来」可以在单测里真跑一趟。runner 里只剩读账和发送。

## 四、它被谁调了

生产路径（行号取自本分支）：

1. `Sources/Mac/Services/SessionDaemonMain.swift:64` —— 常驻后台构造 `CrewSessionRunner`（界面侧 `Sources/Mac/Services/SessionHost.swift:33` 也构造一个；两个进程是否都给机长挂空闲钩子，**没核**）
2. `Sources/Mac/Services/CrewSessionRunner.swift:1637` —— `run.onBecameIdle` → `runBecameIdle`；补投重试收尾时也走这里（`:451`）
3. `Sources/Mac/Services/CrewSessionRunner.swift:339` `runBecameIdle` → `:355` `remindCaptainToSweepTodos(run)`（前面有补投、`continue_work` 认领时不走到这一步）
4. `Sources/Mac/Services/CrewSessionRunner.swift:388` → `CaptainTodoSweepStore.shared.idleTick(...)`
5. `Sources/Stores/CaptainTodoSweepStore.swift:48` `idleTick` → `CaptainTodoSweep.decide`（读不出来那一支在 `Sources/Support/CaptainTodoSweep.swift:127` 调 `unreadableGap`）→ `:59` `nextUnreadableStreak` → `:63` `recordReminded` / `:68` `remember`（清零）

接线由 `testIdleHookActuallyAsksTheCaptain` 钉着：runner 源码里必须出现 `CaptainTodoSweepStore.shared.idleTick(`。

**要生效，后台必须换成新二进制。** 眼下跑着的那个后台连 0.1.36 的地板间隔都没有（见第二节）。

## 五、红 / 绿 / 变异

（待补）

## 六、边界

以下都是**读代码推出来的**，没有在真故障里量过。

### 仍可能叫得太勤

- **账本忽好忽坏**：只要中间有一拍读得回来，档位就清零，重新从 15 分钟起算。故障如果是断续的，间隔就退回到固定的 15 分钟。按口径这是对的（读得回来就该恢复正常判定），代价是退避在这种情形下不起作用。
- **文件名标记写不进去**（建不了 `sweep-reminded/` 子目录），同时进程又重启了：档位和时刻一起丢，退回到「没有地板」。数据目录 EPERM 那种故障放行建新文件，所以在它里面不会发生；换一种连建文件也拦的故障就会。
- **读得出来的正常路径没动**：账上有没被确认覆盖的条目时，仍是每 15 分钟一次，不退避。

### 可能被压得太久

- **封顶 2 小时**：一窗故障已经退到 8× 之后，如果冒出一个不同的读失败原因，最多要 2 小时才会被问到。机长从提醒正文里分不出是不是同一件事。
- **清零只发生在机长空闲的那一拍**：账在机长一直忙的时候恢复了、没等到下一次空闲又坏了，档位就不会清零，旧档位继续生效。
- **「下次在 X 分钟后」是最早时刻，不是闹钟**：提醒只由空闲事件触发，这是既有设计，这次没改。机长到点时要是一直空闲、没有任何新事件，就不会被问。
- **helper 里的确认清不掉后台进程内那份**：`confirm_todo_sweep` 跑在 helper 进程里，它清的是自己进程内的记录和文件名标记；后台进程内的时刻和档位还留着。这一点是既有的，这次多带了一个档位。紧接着账又读不出来的话，会按那份旧档位算间隔。
- **时刻相同时取档位小的**：方向是「宁可多问一次」。按现在的写法，同一时刻只有清零会改档位，所以不会压住；将来如果有别的写入改档位却不改时刻，这条假设就不成立了。

### 样本的边界

- 测试用 `chmod 000` 造的是 EACCES，真故障是 EPERM。两者都落到 `MultiProcessJSONStore` 的 `.unreadable`，前置条件断言的是这个口径，不是 errno。**真 EPERM 下没跑过。**
- 「一窗 9 小时 7 次」是按每 5 分钟空闲一次模拟的。真实的空闲更密，但判定只看时间差，所以次数不变；空闲更稀的话只会更少。
