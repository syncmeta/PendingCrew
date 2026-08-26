# session 续跑与工作目录：查实、修法，以及 `WorkdirMigrationPlan` 还剩多少存在理由

> 人类 Todo #68。本文只做两件事：**把查实的事实摆出来**，**给第 4 件（搬家逻辑还要不要）
> 一个有依据的结论**。所有数字都是 2026-08-26 在本机盘上跑出来的，不是引用。
> 环境：claude 2.1.246、macOS 25.6、本机账本 `agent-sessions.json` 383 条。

---

## 0. 一句话

**我们自己造了一道门，用一个 claude 从来不看的东西去替它回答，于是把五分之一能续的
会话挡在了门外。**「没记工作目录」是另一个真毛病（进程会跑错目录），但它不是这道门的病因。

> **能力判断要问那个有能力的系统。** 我们拿「文件在不在我们以为的位置」代替了
> 「claude 能不能续上」—— 而只有 claude 能回答后者。**代理量替代真观测**，这次的
> 特别之处是：**我们不只是量错了，我们量了一个 claude 从来没在用的东西。**

---

## 1. 查实

### 1.1 `--resume` 的查找范围：整个 `~/.claude/projects` 树，只认文件名

三组实验，两侧都钉死：

| # | 做法 | 结果 |
|---|---|---|
| 1 | A 目录起一轮 → **B 目录** `--resume <同一个 id>` | **接上了**；两轮写在**同一个** jsonl 里，文件始终在 A 的 slug 下；B 的 slug 下没生成任何 jsonl |
| 2 | 把那个 jsonl **手工挪到一个跟任何真实路径都对不上的目录**（`…/projects/-tmp-pendingcrew-e5-unrelated-slug/`）→ **第三个目录** resume | **接上了**，并继续追加写进那个不相干目录里的原文件 |
| 3 | 把同一个文件挪到 `~/.claude/projects` **树外** → resume | 当场 `No conversation found with session ID: <id>`，exit 1，不静默新开、不留下文件 |

独立佐证（官方 `--help`，本机 2.1.246）：

```
-c, --continue        Continue the most recent conversation in the current directory
-r, --resume [value]  Resume a conversation by session ID, or …
```

**写 CLI 的人恰恰在这两者之间划了这条界**：按目录续的是 `-c`，按 id 续的是 `-r`。

顺带一条结构事实（实验 1 的副产品）：**同一个 jsonl 内部 `cwd` 是逐行变的** ——
前半段记 A、`--resume` 之后追加的行记 B。所以**没有哪一行代表「日志在哪儿」**；
「日志在哪儿」只有一个事实源：**它实际躺在哪个目录**，那个目录在创建时就冻住了。

### 1.2 那道门挡掉了多少（本机全量）

- claude 记录 **339** 条，日志 **339/339 全在盘上，一条没丢**。
- 按旧判据（crew 当前 `workingDirectory` 反推 slug）**对不上的 69 条 = 20.4%**：
  - **62 条**躺在 isolation worktree 的 slug 下；
  - **7 条**躺在 crew 搬家前的旧 slug 下。
- **这 69 条 claude 全都续得回来** —— 按 §1.1，它根本不看目录。

### 1.3 slug 规则我们写错了

claude 把路径里**每一个非 `[A-Za-z0-9-]` 的字符**都换成 `-`；我们只换 `/` 和 `.`。

- 证据 A：本机 145 个项目目录，**无一例外只含 `[A-Za-z0-9-]`**。
- 证据 B（实测，非推断）：在 `…/scratchpad/e1_a` 起一个 session，日志落进
  `…-scratchpad-e1-a/`，**下划线也变成了 `-`**。
- 影响面：当时 21 个 worktree 里 **4 个**名字带下划线（`in_progress`、`respond_todo`…），
  按旧规则一个都对不上。写这份文档的那个 session 自己就是其中之一。

今天不咬人（唤醒查的是 crew 共享目录，路径里没下划线），但**只要开始按真实 worktree
路径算 slug，这条不修就等于新字段对这批 worktree 直接失效**。已修 + 单测钉住。

### 1.4 世界观在 `--resume` 下不会叠加（第 2 件的实测之一）

双向证据：

- 带 `--append-system-prompt-file` 起一轮 → resume 时**不带**：那条附加规则**完全丢**
  （问它规则里的值，它瞎猜一个）。
- resume 时**带上**：值答对，且问它「这节规则你看到几次」答 **1**。

结论：**它是每次调用现给的、不进会话**。所以续跑不会叠加两份世界观，反过来说
**每次都必须带**。这条是障碍的假设被推翻。

### 1.5 白板游标确实会灌（第 2 件的实测之二）

`WhiteboardCursor.unread` 的 `.anchored` 那支**不设上限**（`firstDeliveryLimit = 30`
只管 `.absent`）。父群白板 **993 条、每天 60~80 条** —— 复用旧 localSessionId 的机长
隔三天醒来就是一次性灌进两百多条。**是硬伤。**

处置：机长续跑**不动 id 生成**，只查「本 crew 最近一条 `captain-*` 记录」拿会话号。
新 localSessionId → 游标 `.absent` → 只投最近一批，**这个硬伤自己就没了**，不用另加
截断逻辑。三道守卫（单机长槽 / `launchesInFlight` / `runs.removeAll`）全按 crewId 判，
不受影响（逐条看过代码）。

### 1.6 claude 没有「查会话在不在」的离线接口

`claude --help` 的 Commands 逐条看过：`agents / auth / auto-mode / doctor / gateway /
import / install / mcp / plugin / project / setup-token / ultrareview / update`。
**没有任何列会话或查会话的子命令。**

这条是负面结论，但它承重：**不存在一个不花额度就能问 claude 的办法**，所以
「真去试、失败了降级重起」不是我们懒，**是唯一诚实的形状**。

---

## 2. 坐实一条假说：每次搬家都会漏掉执行搬家的那个机长

`WorkdirMigrationPlan` 有一条明写的规矩：**活着的成员，会话记录一律不搬**
（`sessionStillLive`），要等它停了再调一次补搬。而**机长正是执行搬家的那位** ——
搬家当下它必然活着。

**假说成立。** 三次搬家，逐条对时间：

| crew | 搬家时刻（`crew.updatedAt`） | 留在旧目录的记录 | 时间差 |
|---|---|---|---|
| #4 PendingCrew | 2026-08-19T05:09:49Z | `captain-50a9fee5`（05:08:32Z） | **早 77 秒** |
| #42 pending.name | 2026-08-24T06:42:28Z | `captain-8bd8cdda`（06:33:44Z） | **早 9 分钟** |
| #22 文档站（第二跳） | 2026-08-24T10:15:04Z | `captain-1928e8d2` + `worker-6651738f` | 早约 1 天 |

crew #4 尤其干净：**全部 50 条记录里只有 `captain-50a9fee5` 没被搬走**，其余全部
落在新目录。那正是执行搬家的那一任机长。

**另一条谁都没提到、比假说更要紧的发现：`previousWorkingDirectory` 只记一层。**

crew #22「文档站」搬过**两次**家（`…/Pendingname/website` → `…/docs/pending.name`
→ `…/docs/PendingName`）。第一次搬家漏下的 3 条现在躺在 `-Users-hey-Untitled-Pendingname-website`
下，而 crew 的 `previousWorkingDirectory` 已经被第二次搬家覆盖成 `…/docs/pending.name`
—— **补搬工具永远够不着那 3 条了**。而且 `…/Pendingname/website` 这个目录本身已被删除。

这不是「补搬工具有个 bug」，**是「按 crew 的路径链去找日志」这条路子的结构上限**：
只记一层，就只能补救最近一次搬家。**而按会话号找根本不需要那条链** —— 那 3 条孤儿
在新实现下当场就能续回来。

---

## 3. 第 4 件的结论：`WorkdirMigrationPlan` 该删哪半、该留哪半

父机长的原话是「记住路径之后搬家根本不需要搬日志，整套移文件逻辑失去存在理由」。
**前提只对了一半，但结论对，而且理由比原话更硬**：不是因为我们记了 workdir，
**是因为 claude 压根不按目录找会话**。

### 3.1 必须留（跟工作目录记不记完全无关）

| 分支 | 为什么留 |
|---|---|
| `copyClaudeProjectSettings` | `~/.claude.json` 的 `projects["<绝对路径>"]` 信任/权限条目。新目录没条目 → **第一个 session 撞信任弹框卡死**。本文档写作过程中在 `/private/tmp` 起 claude 时**当场复现**了这个弹框，不是理论风险。 |
| `copyCodexTrust` | `~/.codex/config.toml` 的 `[projects."<路径>"] trust_level`，同理。 |
| `copyClaudeMemoryFile` | 项目记忆 `memory/` 按路径分家，只能复制不能搬（旧路径还有别的 crew 在用）。 |
| `setCrewWorkingDirectory` | 这是改目录这件事本身。 |
| `claudeProjectSettings*` / `codexTrust*` / `memory*` 的 Skip | 上面三条的说明面。 |
| `crewHasNoWorkingDirectory` / `crewAlreadyAtNewWorkdir` | 与 transcript 无关。 |

### 3.2 可以整段删（transcript 那半）

| 分支 | 删的理由 |
|---|---|
| `Action.moveClaudeTranscript` | 文件在哪儿都能 resume（§1.1 实验 2）。搬它**零功能收益**。 |
| `Action.moveClaudeTranscriptSidecar` | 同上。 |
| `Skip.transcriptSourceMissing` | 只服务上面两条。 |
| `Skip.transcriptTargetExists` | 同上。 |
| `Skip.sessionStillLive` | 同上 —— **而且它正是 §2 那条「每次搬家漏掉机长自己」的直接成因**。删掉 transcript 搬运，这个漏网连同它的成因一起消失。 |
| `Skip.codexSessionNeedsNoMove` | 它存在只是为了解释「codex 的不用搬」。没有搬运就不需要解释。 |
| `Skip.unknownAgentKind` | 只在 transcript 那个循环里产生。 |
| `Plan.claudeTranscriptMoveCount` / `affectedMembers` / `pendingSweepMembers` / `isSweep` | 全是 transcript 搬运的输出面。 |
| **清扫模式（`isSweep` + `sourceDirectory` 的 `previousWorkingDirectory` 分支）** | 清扫**只**为补搬「当时还活着所以没搬」的会话。没有搬运就没有清扫。连带 `LocalCrewStore.previousWorkingDirectory` 在这条线上变成纯留痕（它今天没有第二个消费者）。 |

粗略量：`WorkdirMigrationPlan` 527 行 + `WorkdirMigrationExecutor` 511 行里，transcript
那半加上它的回执/预览渲染，估计 150~200 行；两个测试文件里约 31 处断言提到 transcript。

### 3.3 删了会漏掉什么（诚实的那一栏）

1. **旧目录里的 jsonl 会永久留在旧 slug 下。** 功能上无影响（按 id 找得到），但
   `~/.claude/projects/` 会长期留着一堆已经不对应任何真实目录的文件夹。这是**整洁问题，
   不是正确性问题**，而且今天本来就有 62 个 worktree 的日志是这个状态（其中 52 个
   worktree 已被删除）。
2. **如果哪天 claude 改成按目录找**（`--resume` 变成 `--continue` 的语义），
   这套搬运就又需要了。风险真实但可观测：新实现的降级路径会**当场在群里报出 claude
   的原话**，不会静默失忆。这正是 §1.6 那条形状的价值。
3. **`sessionsBusy` 这道拦路** 今天有两层意思：数据完整性（别搬正在写的文件）和
   常识（别把目录从正在干活的人脚下抽走）。删掉搬运之后**只剩后者** —— 它从硬约束
   降级成一条判断。**建议保留**，但注释要改，别让下一个人以为它还在保护文件。

### 3.4 建议

**单独一条做，不并进这一版。** 理由：

- 这一版改的是「续不上」的**病根**，是修复；删搬运是**清理**，两件事的风险面完全不同。
- 删存量代码要连带改两个测试文件（约 31 处断言）和执行层的回执渲染，diff 会盖过修复本身。
- 当前 main 上有七条线在并行落地，一个大范围删除的 diff 是纯粹的冲突源。
- 而且**留着它不产生任何危害** —— 它只是白搬文件，搬不搬 resume 都能成。

**评估完等人发话，本版一行存量代码没删。**

---

## 4. 本版实际改了什么

| 件 | 落点 |
|---|---|
| 1（账本记 workdir） | `LocalAgentSessionStore.Record.workingDirectory`（Optional，旧记录解成 nil）；`CrewSessionRunner.start` 的两处 `record(...)` 把**真实 cwd** 记下；`AgentSessionResume.restartDirectory` 决定这一轮跑哪儿（**记的目录还在就回去，不在就回落 crew 共享目录** —— 本机 62 个记过的 worktree 只剩 10 个，回落是常态）。 |
| 2（机长续跑） | `LocalAgentSessionStore.latestCaptainRecord`（按 kind 过滤、按解析后的日期取最新）；`CrewSessionRunner.startCaptain` 查它拿会话号。**不动 id 生成**。 |
| 3（那句错诊断） | 依据整个换掉：`FreshReason.transcriptMissing` → `agentRejectedResume(id:agentSaid:diagnosis:)`。主句是 **claude 的原话**，按 id 翻盘的结果降级成一句诊断补充。 |
| 顺带 | `projectSlug` 改成 claude 的真规则；`AgentSessionResume.decide` 不再预判；`CrewSessionRunner.retryWithoutResumeIfClaudeRefused` 降级重起。 |

**判据的形状**（这条比代码重要）：claude 拒绝时打的那句 `No conversation found with
session ID: <id>` 是**唯一决策依据**；5 秒窗口只是廉价护栏。窗口开宽了，一个跑到第
55 秒才因别的原因死掉的 session 会被判成 resume 失败、悄悄换个新脑子重起 ——
**那才是真把记忆弄丢，而且长得跟 Todo #68 的病一模一样。**
