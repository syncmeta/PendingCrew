# 管理后端 + 更新联动 + resume 恢复 —— 方案

<!-- doc-ref-base: fcc2c28 -->

- 日期：2026-09-11
- **基准提交：`fcc2c28`**（写这份时的 main）。下面所有 file:line 以这棵树为准；
  树移动之后先核行号。`df -h /` = 25 Gi 可用，够。
- 人类原话（父机长转述）：
  > 我希望 pendingcrew 要有管理后端的能力 本机的后端也是一个 要能管理这些的更新
  > 如果前端更新了 本机有后端 也一起更新 然后我就是想要可以用 resume 恢复就可以了
  > 不需要一直保持
- **状态：方案，未动一行代码。**

---

## 0. 先确认那个读法：我同意，而且它作废了一份现有设计

「resume 恢复就够、不需要一直保持」**取消了一个约束**。

被它作废的是 `docs/internal/2026-08-19-backend-split-design.md`（`doc-ref-base: 7065048`）
的立论 —— 那份文档第 1 节白纸黑字写着做法是「**真休眠**……更新 app、重开 app、
app 崩溃，都不打断任何 agent —— **连正在进行的那一轮都不断**」。

那句「连正在进行的那一轮都不断」正是人类刚说不需要的东西。**这不是文档过期，
是它的前提被拿掉了** —— 常驻方向已在 `8a9b19a` 删掉自启时走了一半，这次是另一半。
**建议：给那份设计文档加一段「2026-09-11 前提变更」，不要删。**

---

## 1. 那 75 条到底是什么（父机长排的第一件）

### 实测

口径：账本 `agent-sessions.json` 里 `kind == claude_code` 的 484 条，
逐条看 `~/.claude/projects/*/<agentSessionId>.jsonl` 在不在。

**按天拆开之后，形状非常干净：**

| 日期 | 总数 | 找不到 | 缺失率 |
|---|---:|---:|---:|
| 2026-08-08 | 4 | 4 | **100%** |
| 2026-08-09 | 4 | 4 | **100%** |
| 2026-08-10 | 14 | 14 | **100%** |
| 2026-08-11 | 34 | 34 | **100%** |
| 2026-08-12 | 81 | 12 | 14% |
| 2026-08-13 … 2026-09-03（21 天） | ~300 | **0** | **0%** |
| 2026-09-04 | 16 | 3 | 18% |
| 2026-09-05 | 7 | 0 | 0% |
| 2026-09-06 | 20 | 4 | 20% |
| 2026-09-07 … 2026-09-11 | 37 | **0** | **0%** |

**75 = 68（8-08…8-12 的悬崖）+ 7（9-04 与 9-06 两个孤立簇）。**

再逐条查那 7 条**有没有在自己 crew 的白板上发过言**（判据是消息的
`senderSessionId` 字段，不是全文 grep）：

```
2026-09-04T15:52:57Z  captain-74825976   发过言=False
2026-09-04T16:00:29Z  fd2d9978-…         发过言=False
2026-09-04T16:01:16Z  captain-0ceea850   发过言=False
2026-09-06T08:58:29Z  worker-aba15a17    发过言=False
2026-09-06T09:15:50Z  captain-2d5377d6   发过言=False
2026-09-06T09:18:02Z  captain-a4b5047f   发过言=False
2026-09-06T09:19:40Z  captain-18fa6bb9   发过言=False
```

**7/7 一句话都没说过。** 对照组（找得到日志的 409 条）里 388 条发过言（95%）。

另外排除掉的两条：
- **不是日志被清了**：盘上现存最早的 `.jsonl` mtime 是 2026-08-06 20:37，
  而那 75 条里 **0 条**的 `updatedAt` 早于它。保留窗不背这个锅。
- **不是记错家**（claude 的号其实是 codex thread）：75 个号与 611 个 codex rollout
  的交集 = **0**。

### 读代码读到的

`Sources/Mac/Services/CrewSessionRunner.swift:1447` 一带的注释写明：
「Todo #28：claude 的会话号**由我们指定**（`--session-id`）**并立刻记账**」；
生成与拼参在 `Sources/Mac/LocalRunner/SessionConfig.swift:141-146`。
也就是说**账本条目在 claude 写下第一个字节之前就存在了**。
而 claude 的 `.jsonl` **要有第一条用户消息才生成**。

### 推出来的（推理链写出来）

- 08-08…08-12 那 68 条：缺失率从 100% 一路掉到 14% 再到 0，**是一道上线悬崖**，
  不是损坏率 —— 那几天 `--session-id` 记账刚上线/刚铺开。**它们是历史，不会有人去
  resume 一个 8 月的机长。**
- 9-04 / 9-06 那 7 条：账本先写、claude 没写成第一轮 → 孤儿条目。
  7/7 从没发过言这件事**与这个解释一致**，与「做过活但日志没了」**不一致**。
  （9-06 那 4 条落在 08:58–09:19，正是当天 daemon 被未捕获 NSException 打死的那段；
  这条关联**是推的，我没去核崩溃日志的时刻**。）

### 结论

> **「做过活、而 transcript 不见了」的条目：0 条。**
> 那 75 条里没有一条代表丢失的工作。**所以「resume 够用」这个前提成立。**

**并且这修正了一个数的读法**：75/484 ≈ 15% **不是失败率**。把一道上线悬崖和两个
故障日的孤儿条目，和 21 天连续 0% 的稳定期，算进同一个分母里，得出的比例没有意义。
（同一个毛病我 9-08 犯过一次：报「只有 41% 的记录带工作目录」，按月拆开才看到
9 月那 230 条是 100%。**总数会把两个不同的群体混成一个假问题。**）

---

## 2. 「管理后端」的形状 —— 最难那半已经有了

### 读代码读到的：已经存在的东西

| 已有的 | 在哪 | 它已经回答了什么 |
|---|---|---|
| 后端的**身份与实况** | `Sources/Mac/LocalRunner/SessionDaemonStatus.swift:5` `SessionDaemonStatusSnapshot`（`hello` + `sessions`） | 版本、协议号、启动时刻、连着几个前端、名下几个 session |
| **怎么问它** | 同文件 `:59` `UnixSocketTransport.connect(toPath:)` + 握手 | 无界面探活，`--daemon-status` 就是它的 CLI 外壳 |
| **选哪个后端** | `Sources/Mac/LocalRunner/ProcessRole.swift:23` `PENDINGCREW_BACKEND` | 总闸已经是「后端是可选的」这个形状，不是写死 inproc |
| **前端退化成 viewer** | `Sources/Mac/Services/ViewerSessionClient.swift:7` | 「app 只是连上去看的那个窗口」已经实现 |
| **停一个后端** | `--daemon-stop` / `DaemonStopper`（`Sources/Mac/LocalRunner/DaemonStop.swift`） | 三态探测、只停 daemon 不打 GUI、不升级 SIGKILL |

**所以「后端的共同表示」不用从零设计 —— `SessionDaemonHello` 已经是它，
只差两样：① 一个「我认识哪些后端」的列表（今天只有隐含的那一个）、
② 地址不再写死成本机 socket 路径。**

### 建议的形状（最小改动，不做死在本机上）

```
BackendRef            = { id, 显示名, 传输方式, 地址 }
   传输方式 ∈ { .localSocket(path), .remote(url) }   ← remote 这一档现在不实现，只占位
BackendStatus         = SessionDaemonHello + sessions   ← 已经有了，别新造
BackendRegistry       = [BackendRef]，落一个 json；本机那条是**内置的第 0 条**，不可删
```

- 「本机的后端也是一个」→ 本机那条就是 registry 里的一行，走同一条状态查询、
  同一个更新流程、同一个停用入口。**不给它开特例**，那个「也」字就落实了。
- 远程那一档**只出现在类型里，不实现**：`.remote(url)` 现在一律返回「未实现」，
  有测试钉住它不会被静默当成本机。（**不许悄悄降级** —— 那是今天全组抓到最多的一类
  bug。）

---

## 3. 更新怎么联动，以及那个鸡生蛋

### 实测

本机现状（`ps` + `--daemon-status` 口径）：后台进程连续运行数天、名下十几个 session；
同时有多个不同版本的 claude CLI 在跑。**后台确实是最不换代的那个。**

### 鸡生蛋，说清楚

**「前端更新后去换掉后端」这段策略代码，本身住在后端里。**
所以第一次它不可能自己生效 —— 装上带这个能力的版本之后，**必须人手重起一次后台**，
从那一次之后才自动。**这条要同时写进方案和发版说明**，不写的话人类会以为坏了。

### 建议的链路

```
① app 启动（或 Sparkle 装完新版重开）
② 问本机后端：你什么版本？        ← SessionDaemonHello.build，已有
③ 版本 == 我自己？  → 什么都不做
④ 版本 != 我自己？  → 走「换代」：
      a. 广播：我要换后端了，session 会断，之后自动接回
      b. --daemon-stop（已有：优雅停、停光 session、不留孤儿）
      c. 起新版 --daemon（已有：ViewerSessionClient.launchBundledDaemon）
      d. 对每个「刚才在跑」的成员做 resume（见第 4 节）
```

- ②③ 是**便宜的**：一次 socket 握手，没有副作用。
- **④a 那条广播不是装饰**：人类必须能分清「我的 session 断了」和「出事了」。
- 版本比较用 `SessionDaemonHost.currentBuild` 的同一个字符串，**别新造第二种版本表示**。

---

## 4. 恢复：缺的只有一个触发点

`docs/internal/2026-09-08-restart-session-reattach-feasibility.md` 已经量过并结论：
**接回对话的整套机器都在且在用，缺的是启动路径上没人调 `restartMember`。**
今天再核一次仍然成立：

- `restartMember`（`Sources/Mac/Services/CrewSessionRunner.swift:2419`）三个调用点
  —— 窗口按钮 / @ 投递 / 唤醒器，**没有一个在启动路径上**。
- 后台重启时现在只做收尸 + 报信：`Sources/Mac/LocalRunner/SessionDaemonHost.swift:300`
  一带（`SessionOrphanReaper.decide` / `.apply`，然后往每个 crew 发「后台进程重启，
  N 个 session 被中断」）。**到此为止，一行 resume 都没有。**

所以这一节的实现量 = **把"刚才在跑的那批"的名单留下来，在新后端起来后按名单调
已有的 `restartMember`**。名单本身也已经有了一半：`SessionOrphanReaper` 正在逐个
处置它们，处置的时候就知道是谁。

---

## 5. 恢复的规格（人类 2026-09-11 已定，不再是问句）

他的原话：

> 自动恢复这个 参考其他软件 比如浏览器 什么情况下会恢复原来的标签页？比如更新、
> 意外崩溃等等。这种情况下，弹出一个弹窗，问是否恢复上一次的 session。否则，就不要
> 自动恢复。

落成规格：

- **只有两种情况提恢复**：① 刚更新过；② 上一次是意外结束。
- 这两种情况下**弹窗问他**，他点了才恢复；**默认不恢复**。
- 其余（正常退出、他自己关、主动停的 session）**不恢复也不问**。
- **一次都不自动。** 第 4 节那个「全自动 / 半自动」的问题作废。

### 5.1 承重点：现在**分不出**「正常退出」和「意外结束」——查清了，不够

**读代码读到的：**

- `SessionDaemonHost.stop()`（`Sources/Mac/LocalRunner/SessionDaemonHost.swift:334`）
  全文只有四行：关 listener、写一行日志 `=== daemon 退出 ===`、放锁。
  **它不清 registry。**
- 那本 registry（`SessionProcessRegistry`，`SessionOrphanReaper.swift:138-147`）
  存的是 `daemonPid` / `daemonStartedAt` / `entries[{sessionId, crewId, identity}]`
  —— **没有任何一处记「上一次是怎么结束的」。**
- 开机那段 `reapOrphansFromPreviousRun()`（`SessionDaemonHost.swift:292` 起）只做
  存活核对与收尸，两条路（正常停 / 崩）**走的是同一段代码、发同一句白板消息**
  「后台进程重启，N 个 session 被中断」。

**实测：** 盘上 `daemon.registry.json` 现在 `daemonPid=15229, entries=23` ——
正常停之后这 23 条**照样留在盘上**，与崩溃后一模一样。

**推出来的（唯一一个弱信号，不能用）：** 正常停会先把 session 一个个 `run.stop()`，
所以下次开机核对多半是 `.alreadyGone`；崩溃时 daemon 已 `setsid` 脱离，子进程往往
还活着，多半是 `.reap`。**但它不成立**：正常停时有子进程赖着不死同样得 `.reap`，
崩溃时子进程恰好也没了同样得 `.alreadyGone`。**拿它当判据就是又一次「代理量替代
那件事」。**

> **结论：不够。** 缺的不是判定逻辑，是**一条上次退出留下的痕迹**。

### 5.2 建议：一枚退出印记，而且是三态不是两态

在数据根写一枚 `daemon.lastexit.json`，**写两次**：

| 时刻 | 写什么 |
|---|---|
| 收尾一开始（`DaemonGracefulShutdown.begin()` 的第一件事） | `{phase: "draining", at, build, pid, startedAt, sessions:[…]}` |
| 真的走到 `exit` 之前 | `{phase: "clean", …}` |

开机时读它，**三态**：

| 盘上是什么 | 判定 | 要不要问 |
|---|---|---|
| `phase == "clean"` 且 pid/startedAt 对得上上一轮 | 正常退出 | **不问** |
| `phase == "draining"` | **收尾中途没了**（卡死被杀 / 收尾时崩） | 问 |
| 文件不在 / 解不开 / 对不上上一轮 | 意外结束 | 问 |

**为什么必须是三态**：把「收尾中途没了」并进任何一边都会说谎——并进「正常」会
漏掉真出事的那次，并进「崩溃」会把人自己按的停说成崩溃。这跟锁探测那次是同一族
（`SessionOrchestratorLock.Presence` 的三态）。

**为什么写在收尾开头而不是只写结尾**：只写结尾的话，收尾卡死被 SIGKILL 就什么都
没留下，与真崩溃无法区分——而这两件事对人的意义不同。

**「刚更新过」不需要新机制**：印记里带 `build`，与 `SessionDaemonHost.currentBuild`
一比即可。

**边界（必须写明，别当它盖住了）**：
- **断电 / SIGKILL 掉整个进程**，印记停在 `draining` 或压根没有 → 判成「问」。
  这是安全的一侧（宁可多问一次）。
- **印记自己写失败**（磁盘满、目录不可写）→ 按「文件不在」处理，即「问」。
  同样落在安全的一侧。**不许 `try?` 吞掉**。
- 它**不覆盖** app（GUI）那一侧的崩溃——这枚印记只说 daemon。GUI 自己那一侧要不要
  同款印记，这份方案没看。

### 5.3 弹窗

- 一个窗，「恢复上次的 session？」，**默认不恢复**（焦点不在「恢复」上）。
- **我们绝不替他点**，也不在 `--print`/无界面路径上自动走恢复。
- 恢复失败的：**每个失败的成员在它自己 crew 的白板上留一条，带 agent 的原话**。
  不静默，也不汇总成一句「部分失败」。
- 顺带一条现存的噪音：今天**正常停**也会往每个 crew 发「后台进程重启，N 个 session
  被中断」（`SessionDaemonHost.swift:318` 一带）。印记做出来之后，这句应该只在
  「意外结束」时发。

## 6. 边界 —— 我没看的、我假设的

- **codex 侧的半截回合没量过**：`kill -9` 之后 rollout 留下什么、`thread/resume`
  会不会像 claude 那样自己补一条「接着做」。claude 那侧我实测过（CLI 会插一条
  `isMeta` 的 "Continue from where you left off."），**codex 没有**。当时 codex
  周窗 98%，没为这条烧额度。
- **一次同时 resume 十几个**没试过：启动风暴、额度、`--resume` 并发都未知。
- **9-06 那 4 条与当天 daemon 崩溃的关联是推的**，没去核崩溃日志时刻。
- `2026-08-19-backend-split-*.md` 五份我只读了 design 的前 18 行与 inventory 的清单 A；
  **p0-plan / p1-plan / manual-checks 三份没读**，里面可能已经有我在第 2 节重新提的东西。
- 远程后端一律没设计，只在类型里占位。
- 本方案**未编译、未跑测试**（只出方案，没动代码）。
