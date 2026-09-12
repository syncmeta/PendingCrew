# app 重启之后能不能把原来那些 session 重新接上 —— 可行性读数

<!-- doc-ref-base: f61a7b2 -->
- **日期**：2026-09-08
- **基准提交**：`9ac552a`（main）。下面所有 file:line 都是在这棵树上数的；
  树移动之后先核行号再引用。
- **测量环境**：本机，`claude 2.1.263`，PendingCrew 0.1.27
- **背景**：2026-09-07 人类否掉常驻方向，原话「不要开机自启。我不是要常驻后台。
  我意思是 **session 能恢复就可以了**，不需要常驻，然后**重启 pendingcrew 我希望能接上
  原来的 session**」。开机自启已在 `8a9b19a` 整个删掉。
- **现场**：2026-09-08 装 0.1.27 那次重启，全机十几个 crew 的 session 一起断，人类得
  一个个叫醒。这份读数就是对着这个现场写的。

---

## 结论（一句话）

**接得上，而且比预期便宜 —— 接回对话的机器已经全部就位并在用；缺的只有一个触发点：
启动路径上没有任何人去调它。**

代价有一项、且只有一项是真的：**那一轮跑到一半的输出会丢**（问题和续跑指令都还在）。

---

## 一、接得上的依据

### 1. 每一条 session 都记着 agent 侧的会话号 —— 707/707

账本 `agent-sessions.json`（`Sources/Stores/LocalAgentSessionStore.swift`）：

| 量什么 | 结果 |
|---|---|
| 账本条数 | **707**（claude_code 466 / codex 241） |
| 记了 `agentSessionId` 的 | **707 / 707 = 100%** |
| claude 会话号在盘上有 `.jsonl` | **397 / 404 = 98%** |
| codex thread 在盘上有 rollout | **77 / 77 = 100%** |

写入点（每次起 session 都写，所以这是承重数据不是尽力而为）：
`Sources/Mac/Services/CrewSessionRunner.swift:1383`、`:1437`、`:1769`、`:1814`、`:2026`。

### 2. 两侧都已经有真正的续接通道，而且**在用**

- claude：`--resume <id>`；决策在 `Sources/Mac/LocalRunner/AgentSessionResume.swift:67`
  （`decide(recordedId:)`）。**记了就直接带 `--resume` 去起，不预判**，claude 自己拒了
  再降级重起并把它的原话如实带进白板（`CrewSessionRunner.swift:1540` 那条注释）。
- codex：`thread/resume`；`CodexAppServer/CodexAppServerBackend.swift:157`，
  失败降级 `thread/start`，同样 fail-loud。

### 3. `--resume` 跟工作目录无关 —— 今天在 2.1.263 上重新实测过

A 目录 `claude --session-id <uuid> --print "…记住暗号 XK7-QUARTZ"`
→ **换到 B 目录** `claude --resume <同一个 uuid> --print "暗号是什么"` → 原样答出
`XK7-QUARTZ`。`.jsonl` 仍然留在 A 目录的 slug 下。

（这条复核的是 Todo #68 当初的结论在当前 claude 版本上仍然成立。）

### 4. 回它当初那个目录跑，这件事也已经做了

`AgentSessionResume.restartDirectory(recorded:crewDirectory:)`
（`AgentSessionResume.swift:205`）：账本记着的目录还在就用它，不在了回落 crew 共享目录。

---

## 二、缺的那一件：**启动路径上没人调它**

`restartMember`（`CrewSessionRunner.swift:2419`）全仓**三个**调用点：

| file:line | 触发条件 |
|---|---|
| `Sources/Mac/Views/CrewSessionWindowView.swift:442` | 人在 session 窗口里点 |
| `Sources/Mac/Services/CrewLocalMentionDelivery.swift:117` | 有人 @ 它 |
| `Sources/Mac/Services/CrewLocalMentionWaker.swift:310` | 定向唤醒器投递 |

**没有一个在启动路径上。** 启动时跑的是 `SessionHost.start`
（`Sources/Mac/Services/SessionHost.swift:147`）：

- `:155` `rearmWakeups()` —— 重挂定时唤醒
- `:157` `startSessionsSnapshotTimer()` —— 点名快照
- `:160` 装 `CrewLocalMentionWaker`
- `:184` `resumePendingCaptainReassignments` —— **只管机长交接请求**

也就是说：今天的「重启之后接上」实际含义是 **「等有人 @ 它，它才回来」**。
今天早上人类挨个叫醒十几个 crew，叫的就是这个触发点。

---

## 三、代价：三项，只有第一项是真的

### ① 跑到一半那轮的**输出**会丢（问题和续跑指令不丢）

实测（同 session 内做的，`claude 2.1.263`）：起一个正在数数的 claude，`kill -9`
模拟「退出 app 带走 agent 进程」，然后看它的 transcript：

- 被杀那一刻：transcript 里**有那条用户消息，没有任何 assistant 回复**。
- `--resume` 之后 grep transcript，第一手看到 **claude CLI 自己插了一条
  `isMeta: true` 的用户消息「Continue from where you left off.」**。

**所以「半截回合会丢」比担心的轻**：丢的是那一轮已经产出的文字，问题本身和「接着做」
的指令都在，续上之后模型会从头把那一轮再做一遍。

> 未测的边界：codex 侧被 SIGKILL 打断时 rollout 留下什么、`thread/resume` 会不会补一条
> 等价的续跑指令 —— **没量过**。codex 周窗当时已用 98%，没有为这条烧额度。

### ② 工作目录这条曾经有个缺口，但它是历史性的、已经自愈

账本里 `workingDirectory` 字段的按月分布：

| 月份 | 条数 | 其中有 workdir |
|---|---|---|
| 2026-08 | 477 | 67 |
| 2026-09 | 230 | **230（100%）** |
| 合计 | 707 | 297（42%） |

**那个 42% 是纯历史包袱**：`workingDirectory` 是 Todo #68 才加的字段，8 月的行天生没有。
**9 月之后写的每一条都有。** 没有 workdir 的行回落到 crew 共享目录
（`restartDirectory` 的 else 分支），对今天还活着的 session 不构成影响。

> **更正我自己 9-07 那份报告**：我当时只报了「41%」这个总数，没做按月拆分，
> 把一个正在自愈的历史缺口说成了当前缺口。**总数把两个不同的群体混在了一起。**

### ③ 「同时拉起十几个 agent」是设计代价，不是技术障碍

启动即全自动接回 = 开机一下子起十几个 agent 进程、各自烧一轮额度。
人类刚说完「不要常驻」，全自动很可能踩到同一条不适。这是**范围问题，要人拍**，
不是能不能做的问题。

---

## 四、明确不成立的一条

**「不常驻」和「接上原来的 session」不冲突。**

冲突只存在于把「接上」理解成「接回同一个活进程」时 —— 那确实需要常驻。
但实测表明「重新拉起 + 接回同一段对话」这条路已经铺好了（第一节四条），
而它不需要任何常驻。

---

## 五、这份读数没覆盖的

- **codex 侧的半截回合**（见 ①）。
- **一次同时 resume 十几个** claude 的实际表现（额度、启动风暴、`--resume` 并发）——
  没测过。
- **人类到底想要哪一档**（启动即全自动 / 显示成「可接回」等人点）—— 这是拍板项。
