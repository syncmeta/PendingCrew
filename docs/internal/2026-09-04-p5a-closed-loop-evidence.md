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
