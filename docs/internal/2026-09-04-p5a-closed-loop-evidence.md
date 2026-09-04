# P5a 闭环证据（claude + codex，全程零 nudge）

> **为什么这份东西在仓库里而不在群聊里**：2026-09-04 14:35 前后，本 session 的
> `post_to_crew` 连续报「未能打开文件 `local-71b7fd5b-….json`，因为你没有查看它的
> 权限」，白板写入通道单向断了（`report_to_parent` 那个方向仍通）。已核实**不是文件坏了、
> 也不是权限位不对**：`ls -l` 是正常的 `-rw-r--r-- hey staff`、mtime 还在跟着别的进程更新。
> 这与 `2026-08-27-p4-handoff-report.md` 第八节记的是同一种故障形状（那次根因也没查出来）。
> **同一条教训第二次成立**：结论落在仓库里比落在群聊里耐得住。

- 日期：2026-09-04
- 被测 commit：**`7e7d11d`**（`merge(P5a): 屏幕→文本口径统一（NUL→空格）+ --daemon-attach 支持 codex transcript`）
- 数据根：`PENDINGCREW_DATA_DIR=~/Library/Caches/pcsmoke2`（**未触碰真数据目录**）
- 工作目录：`.claude/worktrees/frontend-backend-separation-c0665c`（claude 已信任，避开信任对话框那条分支）

---

## 一、全量判据（共享目录实跑，不是 worktree）

```
Executed 1875 tests, with 6 tests skipped and 0 failure
** TEST SUCCEEDED **
```

**1869 passed / 6 skipped / 0 failed**；macOS 与 iOS Simulator 两端 `** BUILD SUCCEEDED **`。

### ⚠️ skip 从 3 变成 6，逐条说明（不许含糊过去）

多出来的三条**全部**是：

- `CrewMentionFilterRealWhiteboardTests.testAWrongLocalUserIdDoesNotKeepThem`
- `CrewMentionFilterRealWhiteboardTests.testEveryRealHumanMessageCarriesTheLocalSentinelId`
- `CrewMentionFilterRealWhiteboardTests.testFilterKeepsEveryRealMessageTheHumanSent`

它们读 `~/Library/Application Support/PendingCrew/whiteboards/`，而**本 session 的进程此刻正好
被挡在那个目录外面**——与上面那条 `post_to_crew` 故障是同一件事。所以这三条是
**前提没成立，不是回归**。这正是 `2026-08-27-p4-handoff-report.md` 第六节写死的那条：
> `skip == 3` **当且仅当** fixture 在场且真白板目录可读。

**这条判据前提今天第二次被兑现了**，它不是纸面上的谨慎。

### 由此定下一条对账规矩：**只看「执行数」和「失败数」，别拿 skip 数当基线**

`skip` 是**环境条件的函数**，不是代码的函数 —— fixture 在不在场、真白板目录读不读得动，
都会让它变。今天一天里它取过 3、6、11 三个值，而代码一行没变。
对账时把 `Executed N tests` 与 `failures` 两个数对上就够硬：

| 读数 | commit | 执行 | skip | 失败 |
|---|---|---|---|---|
| 合 NUL/codex 之前 | `0307e1b` | 1866 | 3 | 0 |
| 当前 | `7e7d11d` | **1875** | 6 | 0 |

`1875 − 1866 = 9`，正好等于 attach worker 新增的 9 条。**两个人各自独立量到同一组数**
（我在共享目录、他在自己那趟），这比任何一方单独报数都硬。

### ⚠️ 一处必须更正的历史读数

群聊里出现过的「全量 1863 通过 / 3 跳过 / 0 失败」**量的是 `0307e1b`，不是当前 main**：
那趟测试日志最后写入 14:34:05，而 `7e7d11d` 提交于 14:34:22，**晚 17 秒**。
worker 的对账 `1847 + 16 = 1863` 也只覆盖他那 16 条、不含 NUL/codex 那批。
**病根不是算错，是没写清读数是在哪个 commit 上量的。** 1863 只能当 `0307e1b` 的历史读数。

---

## 二、claude 闭环：daemon 起 → 开场任务自己送到 → 断开 → 重连恢复

1. `PendingCrew --daemon` 起在隔离数据根上，pid 47777。
2. 往 `<数据根>/whiteboards/` 投一条 `start_session` 命令文件（`<crewId>.<uuid>.crewcmd.json`）。
3. `--daemon-status` 报出 `- 闭环靶子 · local-smoke-0001 · worker-45b08f89`。
4. **开场任务自己送到、自己被提交、拿到真回答**，`~/.claude/projects/…/7612e984-….jsonl` 逐行核过：

```
user      | 这是一次后台闭环冒烟。请只回一句「后台闭环：开场任务自己送到了」，然后停下等待。…
assistant | 后台闭环：开场任务自己送到了
```

**全程一次 `nudge_session` 都没发** —— 这正是 2026-09-04 上午那条 bug 的反面判据。

5. `--daemon-attach worker-45b08f89 --attach-reconnect`：

```
── 第 1 份快照（首次 attach）· 1632 字节 · 本地渲染 200×50 ──
 ▐▛███▛█   Claude Code v2.1.260
▝▜██████▀  Opus 5 (1M context) with high effort · Claude Max
…
⏺ 后台闭环：开场任务自己送到了
── 第 2 份快照（断开后重连）· 1632 字节 · 本地渲染 200×50 ──
（同上）
两份快照文本一致：是
```

重连走的是**新开一条 socket**（socket 真断开就是 fd 没了），比 `reconnect()` 更接近
「关掉 app 再打开」这个真实动作。

### NUL→空格那条修复，在真字节上确认了

同一位置，**修之前**是 `ClaudeCodev2.1.260` / `Opus5(1Mcontext)withhigheffort·ClaudeMax`，
**修之后**是 `Claude Code v2.1.260` / `Opus 5 (1M context) with high effort · Claude Max`、
`⏵⏵ auto mode on (shift+tab to cycle)`。中文也没被撑开成「我 在」——
两种 NUL 分开处理（没写过的格 `width==1` → 空格；全角字后半格 `width==0` → 丢掉）是对的。

---

## 三、codex 闭环：没有终端，走 transcript 契约

同一个 daemon 里起真 codex session（`worker-2b74ae4f`）：

```
── 第 1 份transcript（首次 attach）· 2 条 · 这是**对话记录**，不是终端画面（codex 没有终端） ──
[输入] 这是一次后台闭环冒烟。请只回一句「codex 后台闭环：开场任务自己送到了」…
[回复] codex 后台闭环：开场任务自己送到了
── 第 2 份transcript（断开后重连）· 2 条 · …
（同上）
两份 transcript 文本一致：是
```

**开场任务同样是自己送到的，零 nudge。** 输出里明写「这是对话记录，不是终端画面」——
不让人误以为 codex 也有终端快照。

daemon 日志逐步有据，且每次断开都记着 **session 不受影响**：

```
握手：app build=daemon-attach protocol=1 协商能力=approval-mode,launch-parameter-problem,
      profile-switch,screen-text,terminal-bytes,transcript-events
attach worker-2b74ae4f → handle 3（0×0）
viewer 断开（剩 0 条）；session 不受影响
… attach worker-2b74ae4f → handle 4（0×0）
viewer 断开（剩 1 条）；session 不受影响
```

`0×0` 是探针**刻意不报视口**：报了就等于把在跑的 TUI 重排一次 + 给 agent 发一次 SIGWINCH，
为「看一眼」付这个副作用不划算。代价是渲染宽高用本地值（200×50），
**证的是「两份文本相等」，不是「逐格等于 daemon 那份」**。

---

## 四、四道闸门对账（机长 2026-09-04 定的收口顺序）

| # | 闸门 | 状态 |
|---|---|---|
| 1 | 开场 brief 可靠投递，真 daemon 里不靠 nudge 自动开始 | ✅ 本页第二节 |
| 2 | 无界面 attach 探针，断开后重连恢复同一 session | ✅ 本页第二、三节 |
| 3 | `--daemon-status` 不把 exited 报成运行中 | ✅ `036e3db` |
| 4 | 可信目录复跑整条真实闭环，claude + codex 各一条 | ✅ 本页第二、三节 |

**总闸 `PENDINGCREW_BACKEND` 仍是 `inproc`，默认没翻。** 下一步是默认切换 + §9.2 那套
防分裂降级契约的实现。

---

## 五、这一趟仍然没验到的（不许算进上面的绿）

1. **没验安装态**：全部跑在 Debug 构建的二进制上，不是签名安装版；「更新 app 不断线」
   （A1 三条路径）没跑。
2. **没验开机自启 / 崩溃自拉**（`SMAppService`）——那是 P5b。
3. **没验半开连接回收**：`SessionReconnectPolicy.daemonIdleTimeout` 全仓仍无第二处引用。
4. **没验背压/重同步在真 socket 上的表现**：探针一次收完就走，从不落后；那条另有单测背书。
5. **渲染宽高**见第三节末尾那段。
6. **未信任目录**那条分支（claude 的信任对话框）本次刻意避开，只有单测覆盖。

---

## 六、翻默认之后的复跑（2026-09-04，`bdf92f3` 之后）

总闸默认已从 `inproc` 翻成 daemon（`ProcessRole.resolve`），§9.2 那套防分裂降级契约
同一笔落地。按 §9 P5a 的纪律「证据先于翻默认，不许倒过来」，翻完之后**两条闭环各复跑
一遍**，仍然是纯 CLI、隔离数据根（`PENDINGCREW_DATA_DIR=~/Library/Caches/pcsmoke-flip-145751`，
跑完已删；**未触碰真数据目录**），工作目录是 claude 已信任的那个 worktree。

**claude（`worker-a9322fe3`）—— 全程零 nudge**：

```
── 第 1 份快照（首次 attach）· 1634 字节 · 本地渲染 200×50 ──
 ▐▛███▛█   Claude Code v2.1.260
▝▜██████▀  Opus 5 (1M context) with high effort · Claude Max
❯ 这是一次翻默认之后的后台闭环冒烟。请只回一句「翻默认后闭环：开场任务自己送到
  了」，然后停下等待，不要用任何工具、不要读任何文件。
⏺ 翻默认后闭环：开场任务自己送到了
── 第 2 份快照（断开后重连）· 1634 字节 …（同上）
两份快照文本一致：是
```

transcript 逐行核过（`…/c100b408-….jsonl`）：

```
[user]      这是一次翻默认之后的后台闭环冒烟。请只回一句「翻默认后闭环：开场任务自己送到了」…
[assistant] 翻默认后闭环：开场任务自己送到了
```

**codex（`worker-5f634b00`）—— 同样零 nudge**：

```
── 第 1 份transcript（首次 attach）· 2 条 · 这是**对话记录**，不是终端画面（codex 没有终端） ──
[输入] 这是一次翻默认之后的后台闭环冒烟。请只回一句「codex 翻默认后闭环：开场任务自己送到了」…
[回复] codex 翻默认后闭环：开场任务自己送到了
── 第 2 份transcript（断开后重连）· 2 条 …（同上）
两份 transcript 文本一致：是
```

### 这一趟仍然没验到的（在第五节那六条之上再加三条）

7. **降级契约那条路没在真机上走过**：§9.2 的三种结局（临时接管 / 继续重连 / 拒绝接管）
   全部只有单测覆盖 —— 表本身、执行顺序、界面态三层各有一组，且都先证明过会红。
   真机上没造出「daemon 拉不起来」这个前提（那要把二进制弄坏或把目录弄成不可写），
   本轮没做。
8. **GUI 那一半没验**：横幅只验到「态到没到界面层」（`OrchestrationNotice` 是纯判定），
   像素与交互没验，也不该为验证开窗口。
9. **翻默认之后没有走过一次「双击图标」的真实启动**：本轮所有证据都来自 CLI 身份
   （`--daemon` / `--daemon-status` / `--daemon-attach`），viewer 自动拉起 daemon 那条路
   （`ViewerSessionClient.connect`）在真 GUI 里没跑过。它属于安装态验收那一档。


---

## 九、P5a 收尾：翻默认之后又逮到的四条，以及最终交付状态

> 这一节是**在上面那份闭环之后**发生的事。上面记的是「默认还没翻」时的证据；
> 这一节记「翻默认」本身，以及翻完之后**在真机上逐条逮出来的四个洞**。

### 已打包那份（基线 `9823c44`，这一栏之后不再变动）

- 包 = 该 commit 的 **Release** 构建，**已签名**（`codesign --verify --deep --strict`
  通过 + `satisfies its Designated Requirement`，zip 完整性通过），**未公证**
  —— 它不走任何发布渠道，只在本机跑。
- 全量：`Executed 1940 tests, with 3 tests skipped and 0 failures`；macOS + iOS
  Simulator 两端 `** BUILD SUCCEEDED **`。
- 真机：**精确 2 个命令文件 → 恰好 2 个不同 session**，两个 brief 各执行一次。
- claude / codex **两条 CLI 闭环，均零 nudge**；断开 → **新开连接**重连 → 画面 /
  transcript 一致。
- **未 push、未写 Sparkle feed、未发 GitHub Release、未动 Homebrew、未打 tag。**

### 翻默认之后逮到的四个洞（按发现顺序，全部已修）

| # | 洞 | 怎么被逮到的 |
|---|---|---|
| 1 | daemon **起来即退、退出码 0**，而拉起方只看 `process.run()` 没抛错 → 记成「起来了」→ 无限「正在连接」，**契约里唯一允许接管的那一支在最常见的失败原因下根本到不了** | 真机：数据根 `chmod 500` |
| 2 | `openLink` 把「**socket 连上**」当成「**握上手**」（`isConnected` 紧跟 `connect()`，从不等 `daemonHello`）→ 接受连接但不回话的 daemon 会让降级判定永不被调用 | 读代码；修法是把 `Bool` 换成三态，**让填错的写法表达不出来** |
| 3 | daemon 与 GUI **各自捧着启动时的快照整份覆写** `local-crews.json` → 后写的抹掉先写的、无声 | 真机：外部改名 + 隐藏，daemon 写一次盘**两处全没** |
| 4 | 派活命令**逐条 append 一个 `@Published` 数组**，消费方拿到的是每次 append 各发一次的快照 → **第一条被处理两遍**（花订阅额度） | 真机：**输入数过**，2 个命令文件 → 3 个 session |

**四个洞的共同形状**：都不是「算错了」，是**把一个更弱的信号当成了想要的那个事实**
（进程起来了≠连得上；socket 连上≠握上手；我内存里那份≠盘上那份；发布的变化≠待办队列）。

### 修法上反复出现的那一手

三次都不是「改对那一行」，而是**让错的那种写法表达不出来**：
`Bool` → 三态 `LinkState`；12 个落盘点 → 一个 `mutatingCrews` 收口；
入队与发脉冲 → 绑进同一个 `enqueue`。**接线里不许留判断只是下限，上限是这个。**

### 仍未验（独立记账，不算 P5a 完成）

1. 安装态的「更新 app 不断线」（A1 三条路径）。
2. `SMAppService` 开机自启 + 崩溃自拉；菜单栏常驻。
3. `SessionReconnectPolicy.daemonIdleTimeout` 的半开连接回收（仍无第二处引用）。
4. 「消费期间新到的进下一批」只在队列层与复刻 harness 上验过，**没在真 daemon 上
   造出排空与消费交叠**。
5. 九条同形队列里**只有 `start_session` 有真机读数**，其余八条共用同一收口但各自没有。
6. `ViewerSessionClient` 那条腿只在 GUI 里活，所以「socketOpen 但等不到 hello →
   到上限 → fail-closed」**只有单测覆盖**。
7. **人类的真人验收尚未进行**（人类 Todo #3）：关掉 app / 重开之后 session 还在不在。
