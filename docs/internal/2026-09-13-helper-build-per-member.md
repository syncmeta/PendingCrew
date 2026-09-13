# 成员行上直接看得出「helper 跑在哪一版、是不是旧的」

<!-- doc-ref-base: cfc1671 -->
- 日期：2026-09-13
- 机长计划 #88 第二件
- 前情：`docs/internal/2026-09-11-stale-helper-toolset.md`（为什么会有旧 helper）、
  `Sources/Mcp/HelperBuildWatch.swift`（helper 在拒绝话术里自报家门）

## 0. 一句话

**取证走编排者（daemon）一侧**：每 2 秒的点名快照那一拍，按 `--mcp-serve --session <id>`
找到每个成员的 helper 进程，读**它正在执行的那个文件**的 inode / 大小 / mtime
（`proc_pidinfo(PROC_PIDREGIONPATHINFO)` 的 vnode stat，不走路径），跟它 argv 里那个路径上
**现在**的文件比。结果写进 `crew-sessions.json` 每个成员的 `helperBuild` 一格；
`list_sessions` 和成员列表都只读这一格，不各算各的。

比对口径没有新发明：两侧都是 `HelperBuildStamp`，判定是 `HelperBuildVerdict.judge`，
而旧的拒绝话术 `HelperBuildWatch.notice` 现在也改走同一个 `judge`。

## 1. 两条路各在什么情况下说错

### A. helper 自报（启动时把自己的 stamp 交给 daemon）

| 情况 | 它会怎么说 | 对不对 |
|---|---|---|
| **今天已经在跑的旧 helper** | 一个字不说 —— 它们是这个功能之前的二进制，没有自报的代码 | 永远「判不了」。而**这批正是要抓的那批**：旧的之所以旧，就是因为它比新代码早 |
| Sparkle 挪包 | 启动时记下的是挪包之前那份（对），之后再比磁盘 → 判旧 | 对 |
| 装版刚好落在 exec 和取 stamp 之间 | 记下的是新文件 → 判「当前」 | **错，而且是往「新」的方向错**。窗口很小 |
| 自报要写一个文件给 daemon 读 | 数据目录 EPERM 发作时 helper 那一侧写不进去/读不出来 | 判不了（发作时恰恰是最想知道「谁是旧的」的时候） |
| session 重启、pid 换了 | 旧 pid 的报告还躺在文件里 | daemon 要核「这个 pid 还活着、还是同一个进程」—— **那就又回到了 B 要做的进程取证** |

### B. daemon 侧对进程取证（本次选的）

| 情况 | 它会怎么说 | 对不对 |
|---|---|---|
| 今天已经在跑的旧 helper | 不需要 helper 配合，照样量得出来 | 对 —— **daemon 换成新版的那一刻，全体成员一起有答案** |
| Sparkle 挪包（argv 还写着 `/Applications/...`） | vnode 的 inode 是挪走那份的，原路径上是新文件 → 判旧 | 对（真进程测过，见 §2） |
| 挪走的旧包又被删掉 | vnode 还在（进程攥着它），inode 照样对不上 → 判旧；版本串读不出来 → 显示「版本读不出来」而不是冒充新版本 | 对 |
| **原地覆写**（inode 不变，内容换了） | vnode stat 反映的是**新**内容的大小 / mtime，跟磁盘一样 → 判「当前」 | **错**；A 在这里是对的。我印象里 macOS 上原地覆写正在执行的已签名 Mach-O 会让进程在下一次缺页时被杀，所以这个形状活不长 —— **这句是记忆，没量过，也没造样本** |
| 写快照的 daemon 自己是旧版 | 快照里压根没有这一格 | 判不了，并明说「写快照的后台是旧版」 |
| 找不到 helper 进程（MCP 还没起来 / 已退出 / 读不到进程参数） | 没有样本 | 判不了，说原因 |
| 同一个 session 名下有多个 helper | 任一个旧 → 整格旧（旧的那个也在回它的工具调用）；有判不了的 → 整格判不了 | 保守方向 |
| 读进程信息被拒（不同用户 / 加固进程） | stamp 为 nil | 判不了 |

### 选 B 的理由

A 最大的问题不是哪一格错，是**它对现存的那批旧 helper 天生无能为力**，而那批就是这件事的
全部对象。B 唯一往「新」方向错的情况是原地覆写正在执行的二进制（我推断它在 macOS 上活不长，没验证过，见上表）；
A 往「新」方向错的那个窗口虽然小，但它每次装版都会出现。

## 2. 真进程样本（不是替身）

`HelperBuildPerMemberTests.test_真进程_Sparkle把包挪走换上新版_argv路径没变_必须判旧`：

1. 把同一次构建出的**真 PendingCrew 可执行文件**拷进临时目录下的 `Applications/PendingCrew.app`，
   Info.plist 写 0.1.30；
2. 以真的 `--mcp-serve --crew … --session …` 起起来（`PENDINGCREW_DATA_DIR` 指临时目录）；
3. 扫描器必须找到它，且判「当前」、报得出 0.1.30（**绿面**）；
4. 照 Sparkle 的形状把整个包 `mv` 进 `Caches/org.sparkle-project.Sparkle/Installation/…`，
   原路径放一份新包（0.1.32，内容多 64 字节）；
5. 断言 argv 里的路径**一字未变**（陷阱本身），而判定是「旧」、跑的版本 0.1.30、磁盘 0.1.32；
6. 把挪走的包删掉，判定仍不许是「当前」，跑的版本不许被说成 0.1.32。

读数见 §5。

## 3. 判不了的时候显示什么

四种来源，文案各不相同（`HelperBuildLookup`），**没有一种会显示成版本号或「一致」**：

- 取证判不了（带原因）
- 写快照的后台是旧版（没有这一格）
- 快照里还没有这个人（刚起来，下一拍才写到）
- 快照文件读不出来

成员行上是一枚「版本?」灰标，悬停看原因；旧的是橙色「旧版」，悬停看两个版本和
「要新工具只能重开这个 session」。点名那一列同理，每行一句。

## 4. 它被谁调了（生产路径，行号取自 `57b5605`）

| 新东西 | 谁在用 |
|---|---|
| `HelperProcessForensics.reports(for:)` | `Sources/Mac/Services/CrewSessionRunner.swift:995`（点名快照每拍，后台线程） |
| `scanHelpers()` / `runningStamp(pid:)` | `HelperProcessForensics.swift:144` / `:135`（上面那条的内部） |
| `Entry.helperBuild`（写） | `CrewSessionRunner.swift:1069` |
| `Entry.helperBuild`（读 → 点名列） | `CrewSessionsSnapshot.swift:144`，经 `renderRoster` 被 `McpServer.swift:1808`（`list_sessions`）调 |
| `CrewSessionsSnapshot.helperBuildLookupTable` | `Sources/Mac/Views/CrewSessionWindowView.swift:80`（5 秒一次，后台读） |
| `HelperBuildBadge.make` / `HelperBuildBadgeLabel` | `CrewSessionWindowView.swift:546` / `:549`（成员行） |
| `HelperBuildVerdict.judge` | `HelperProcessForensics.swift:92`（点名）、`HelperBuildWatch.swift:219`（旧的拒绝话术，现在也走它） |

判定全在 test bundle 里的纯类型上。View 和 `CrewSessionRunner` 不进 test bundle，那两处接线只能按源码盯
（`HelperBuildPerMemberTests.test_接线_*`），见 §5 的变异表。

**界面效果我没亲眼看过。** 成员行上那枚标长什么样、悬停说明出不出来，只是读代码判断的。

## 5. 读数

### 5.1 真进程样本

**测试里的样本**（`test_真进程_Sparkle…`）：进程是同一次构建产出的 Debug 版 PendingCrew。
⚠️ Debug 版的主可执行文件是个桩，代码在旁边的 `PendingCrew.debug.dylib` 里；拷出去单跑会
`dyld: Library not loaded: @rpath/PendingCrew.debug.dylib`。测试里能跑起来，是因为子进程继承了
Xcode 给测试进程设的 DYLD 环境变量（**推断**，没逐个核那几个变量）。所以这个样本的形状是
「桩可执行文件被挪走」，不是 Release 单文件。

**Release 形状的样本是本机上现成的：常驻后台 daemon 自己**（2026-09-13 12:16 之前几分钟量的，确切时刻没记）。

| | 读数 |
|---|---|
| 进程 | pid 53445，`/Applications/PendingCrew.app/Contents/MacOS/PendingCrew --daemon`，起于 09-12 00:14:26 |
| `ps` 的 argv | `/Applications/...`（看着正常） |
| 它正在执行的文件（区段 vnode） | `~/Library/Caches/com.pendingname.pendingcrew/org.sparkle-project.Sparkle/Installation/vcQA1X2vF/8rDGQcwb1/PendingCrew.app/Contents/MacOS/PendingCrew`，inode **184531536**，18591696 字节 |
| 磁盘上现在那份 | inode **187757457**，19038672 字节，0.1.37 (20708.82403) · 07adc8e |
| `proc_pidpath` | **失败**（挪走的包已被 Sparkle 删掉；`Info.plist` 已不存在） |
| `runningStamp` + `judge`（`57b5605` 的代码，从 `git show HEAD:` 编出来跑的） | inode 184531536 / 版本 **读不出来** / 判定 **stale** |

也就是说 Sparkle 挪包再删包这个形状，现有代码靠的是区段遍历里「路径以 `/Contents/MacOS/PendingCrew`
结尾」那条兜底（`proc_pidpath` 拿不到路径），不是主路径。

**我自己在临时目录里拷 Release 可执行文件起 helper，没起来**（进程两秒内没了，stderr 为空，
原因没查 —— 有了上面那个现成样本就不再需要它）。

**横向对照**：扫描器对本机 6 个真实 helper 的读数（pid / inode / 判定当前）跟人手工那条
`lsof … txt` 命令逐个一致。扫一次 6 毫秒。**这一刻本机没有旧 helper**，所以「真 helper 判旧」
只有测试里那一个样本。

### 5.2 变异

变异前先提交（`57b5605`），每刀 `cp` 备份、`perl` 打变异（没打上就记「读数无效」）、跑三个测试类
共 44 条（`Executed` 不是 44 就不算）、`cp` 还原并核对与 HEAD 一致。

| 变异 | 红了哪些 |
|---|---|
| M1 编排者那一拍不取证 | `test_接线_编排者每拍取证并把结果写进快照` |
| M2 成员行上删掉那枚标 | `test_接线_成员列表挂了这枚标` |
| M3 点名不拼这一列 | `test_点名渲染…`、`test_list_sessions回执里真的有这一列` |
| M4 `judge` 恒判当前 | 10 条，含真进程那条和骨架阶段就绿的两条保守面用例（`任一侧…判不了`、`一个新一个判不了`），以及旧的两条拒绝话术用例 |
| M5 扫描器**看路径**（用 argv 路径的 stat 代替 vnode） | **只红真进程那一条** —— 纯判定用例全绿。没有真进程样本，「看路径」这个错会原样合进去 |
| M6a 查表去掉显式 `.some` | **一条都没红。** 我在代码注释里写的「往值是 Optional 的字典里直接赋 nil 会删键」对非字面量的 Optional 不成立 —— Swift 会自动包成 `.some`。已改注释 |
| M6b 没这一格当成没这个人 | `test_查表_文件不在_读不动_没这个人_没这一格_各是各的` |
| M8 拒绝话术退回全字段相等（不走 `judge`） | `test_拒绝话术与点名同一把尺子` |

## 6. 我没做到的 / 边界

- **写快照的 daemon 自己就是旧的**（§5.1）。这个功能合进去、装上新版之后，只要 daemon 不重启，
  点名和成员行上这一格就是「判不了（写这份快照的后台比这个功能早）」。**daemon 什么时候换代不在这件事的范围里**，
  但它决定了用户什么时候看得到这一格。
- 原地覆写正在执行的二进制，这条路会判「当前」（§1 表 B）。没造样本。
- 同一 session 名下多个 helper 的聚合只有纯函数用例，没造过真的双 helper。
- 扫描只认可执行文件名叫 `PendingCrew` 的进程（`proc_name`）。改名的构建不会被扫到 → 「没找到 helper」→ 判不了（不会判成新的）。
- 版本串在 `proc_pidpath` 拿不到路径时一律是「读不出来」，即使挪走的包还在。只影响给人读的那个字段，不影响判定。
- 数据目录这次发作：11:58:39 我这边第一次读不动，12:16:29 我这边再读已经读得动（中间没连续量）。守候日志 `~/Library/Logs/PendingCrew-eperm/watch-20260913-115906.log` 记下的恢复当口也是 12:16:29 —— 跟我那次读是同一秒，**是不是巧合我没查**（brief 要求别查成因），只记在这里。全量测试在恢复之后、干净树里跑，读数见交付回报。
