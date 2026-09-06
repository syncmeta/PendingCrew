# session 到底产出过没有：两家 runner 的取证面（2026-09-06 本机实测）

> 起因：人类 Todo #107 第三件「判活不判状态」。机长的 `list_sessions` 只报状态，
> 而**状态会骗人** —— 当天两个 session，一个卡 90 分钟、一个卡 110 分钟，状态
> 全显示「空闲」，其实任务书压根没提交。所以点名补了一列**产出证据**：不问它
> 显示什么，问它**最近真的写出过什么、什么时候**。
>
> 这份文档记的是那一列**取数的地方**。全部结论都在本机量过，量法附在每条后面 ——
> 下一个人不必再探一遍，但**要改判据前请重新量**（上游随时会变）。
>
> 实现：`Sources/Support/SessionOutputEvidence.swift`（`SessionOutputProbe`）。
> 单测：`Tests/PendingCrewTests/SessionOutputEvidenceTests.swift`。

## 一句话结论

| 取证面 | 能证明什么 | 什么时候沉默 | 我们用不用 |
|---|---|---|---|
| `~/.claude/projects/<slug>/<会话号>.jsonl` 的 **mtime** | 这个 claude 会话最后一次写东西是什么时候 | 会话从没提交过第一条用户消息 → 文件**根本不生成** | ✅ 用 |
| `~/.codex/sessions/<年>/<月>/<日>/rollout-<时间戳>-<threadId>.jsonl` 的 **mtime** | 同上，codex 侧 | app-server 握手没成功 → 没有 threadId，无从查起 | ✅ 用 |
| `~/.claude.json` 的 `projects[<cwd>]`（`lastDuration` / `lastFpsAverage` / `lastGracefulShutdown`） | 这个**目录**上一次跑完的 claude 会话的收尾统计 | **我们的场景下几乎总是沉默**，见下 | ❌ 不用 |

## 1. claude：会话成绩单

- 形状：`~/.claude/projects/<项目 slug>/<会话号>.jsonl`，一个会话一个文件。
- **会话号是我们自己指定的**：`CrewSessionRunner` 起进程前用 `--session-id` 定死，
  并立刻写进 `LocalAgentSessionStore`（`agent-sessions.json`）。所以**账本里有会话号
  ≠ 它写过东西** —— 这正是 #107 那个现场的形状：卡在信任提示 / 任务书没提交的
  session，账本有号、磁盘上没有那份 `.jsonl`。
- **「文件不存在」证明不了「没跑」**：那份 `.jsonl` 要有第一条用户消息之后才生成。
  当天已经有人被它骗过一次。所以这一列必须是三态，不许压成 Bool（见第 4 节）。

**实测（2026-09-06 23:5x）**：`agent-sessions.json` 里 446 条 `claude_code` 记账，
**439 条找得到成绩单，7 条没有**。那 7 条全部集中在 09-04 与 09-06（其中 4 条是
09-06 09:15–09:19 四分钟内起的三个机长 + 一个 worker，**一个字没产出**）——
账本本身从 08-08 起，08 月那批全都还在。也就是说：

- 成绩单**至少留存一个月**（这个数是量出来的下界，更久的没量）；
- 「找不到成绩单」在这台机器上**不是清理造成的**，就是真没产出。

**不按工作目录推 slug**：那个 slug 的编码规则（路径里 `/`、`.`、大小写怎么换）
是上游的实现细节，猜错一次的代价是**静默变成「确实没有产出」**——又一句言之凿凿
的假话。所以实现按**会话号**在 `projects/` 底下逐目录问「有没有这个文件」，
编码规则怎么变都不影响。（本机 168 个项目目录，一次扫可忽略。）

## 2. codex：rollout

- 形状：`~/.codex/sessions/<年>/<月>/<日>/rollout-<ISO 时间戳>-<threadId>.jsonl`。
  **threadId 在文件名尾段**，所以按后缀 `-<threadId>.jsonl` 整段匹配，
  不用 `contains`（别的 thread 名里同形的片段会撞上）。
- threadId 要 app-server **握手回来**才有（`notifyThreadId`）。所以 codex 成员
  刚起来的头几秒天然是「看不出来」，那**不是**「没干活」。

**实测**：238 条 `codex` 记账，**238 条全都找得到 rollout**（本机 rollout 共 611 个）。
与 claude 那 7 条缺口的差别正来自上面那点：codex 的号是**握手之后**才有的，
有号基本就意味着已经跑起来了。

- 另有 `~/.codex/session_index.jsonl`（`id` / `thread_name` / `updated_at`）。
  **没用它**：本机只有 144 行 vs 611 个 rollout，它不是全集，拿它当判据会漏。

## 3. `~/.claude.json` 那条建议：量过，对我们没用

有人建议改读 `~/.claude.json` 里 `projects[<cwd>]` 的 `lastDuration` / 帧数 /
`lastGracefulShutdown`（「被杀也会写」）。**在本机量过，对 PendingCrew 的场景是哑的**：

- 565 个 `projects` 条目里，**只有 36 个**带 `lastStartTime` / `lastDuration` 这类
  telemetry；
- 其中**跑在 worktree 里的目录 507 个，带 telemetry 的是 0 个** —— 而 PendingCrew 的
  worker 恰恰全跑在 worktree 里；
- 更要命的是**它是收尾时才写的**：写这段代码的这个 session **正在**该目录里跑，
  而 `~/.claude.json` 里**压根没有这个 cwd 的条目**。它证明不了「现在还活着」。

量法（只读，复制即可复现）：

```bash
python3 -c "
import json,os
d=json.load(open(os.path.expanduser('~/.claude.json')))['projects']
rich=[k for k,e in d.items() if e.get('lastStartTime') or e.get('lastDuration') is not None]
wt=[k for k in d if 'worktrees' in k]
print(len(d),'条；带 telemetry',len(rich),'；worktree',len(wt),
      '，其中带 telemetry',len([k for k in wt if k in set(rich)]))
"
```

另外它是**按 cwd**索引的，不是按会话 —— 同一个目录跑过多个 session 时它只记最后一个，
天然回答不了「**这一个** session 产出过没有」。

## 4. 为什么必须是三态

`有产出（附时间）` / `确实没有产出` / **`看不出来`**。

「看不出来」= **取证面自己不在场**（还没记下会话号 / runner 不是 claude 或 codex /
那两个目录读不出来）。把它压成 Bool，或写成 `Optional<Date>` 再 `??` 掉，就等于
告诉机长「它确实一个字没产出」—— 一句言之凿凿的假话，比不报还坏，而且它会让机长
把**表象当状态**再犯一次，正是这一列存在要治的那个病。

渲染出来的三句话也必须分得清，「看不出来」那句里**不许出现「没有产出」**，
并且要带上原因和一句「这不等于它没干活」。

守卫这条的是 `test_看不出来与确实没有产出是两态_压成Bool会把看不出来算成没跑`。
它是**先写的**，并且先在一版**故意**压成 `Optional`-then-`??` 的实现上跑红过
（9 项里 6 项红，全部红在 `实际是 noOutput`）——尺子自己先证明会红，才轮到信它的绿。

## 5. 这份判据**没有**回答的事（边界）

- **它不区分「写的是有用的东西」还是「写的是废话」。** mtime 只说「写过」。
  一个在死循环里反复自言自语的 session，这一列会显示「刚刚产出」。
- **它不是活性探针。** 进程死了但成绩单留在磁盘上，这一列照样报「最近产出 X 前」——
  X 变大就是唯一的信号，判「是不是卡住」仍要机长看那个 X 配合状态一起读。
- **成绩单留存期只量到「至少一个月」。** 若上游哪天开始清理，很久以前的会话会
  从「有产出」翻成「确实没有产出」。判据要改前请重新量第 1 节那两个数。
- **只在本机（同一个用户）成立。** 跨机器的成员，helper 读不到对面的 `~/.claude`。
- **只有单测。** 「机长在真 app 里调一次 `list_sessions`、这一列真的出现在它眼前」
  这条整链没有验 —— 那要装含本次改动的构建再点一次名，属设备 QA（挂 task #443）。
