# PendingCrew 界面卡顿采样报告（Todo #59）

采样对象：`/Applications/PendingCrew.app`，版本 0.1.13 (20684.08542)，pid 9214，
macOS 26.6.1 / arm64。工具：`sample`（本机已有进程，无权限弹框；**没有驱动任何
图形界面、没有模拟输入**）。符号：线上二进制是 strip 过的，用同一 commit
（`0300ddf`）本地重编的 Release 二进制做 `atos` 符号化。

原始采样件不入库，留在会话 scratchpad：`sample-idle-1.txt` / `sample-idle-2.txt`。

---

## 结论先行

主线程有**三段**在 PTY 每一批输出上跑的同步工作，按代价排序：

| # | 位置 | 代价 | 触发条件 |
|---|---|---|---|
| 1 | `SessionProfileEchoVerdict.squeeze` （被 `SessionLaunchParameterScanner` 调用） | **63.6 ms / 笔**（8K 尾窗）、**253.7 ms / 笔**（16K 尾窗），复杂度 **O(n²)** | 每个 session 拉起后**头 45 秒**，前提是显式传了 `--model` / `--effort`（机长起 session 一律会传） |
| 2 | `SessionHealthScanner` / `RateLimitMenuScanner` 的 `tail.lowercased()` + 11 次 `String.contains` | **6.3 ms / 笔**（8K）、**10.2 ms / 笔**（16K） | **常驻**，每个在跑的 session 的每一批 PTY 输出 |
| 3 | `TypingActivityTracker.signature`（`split`+`joined`+`count` 走完整段再截断） | 稳态下占主线程 **5.7%**（单个忙碌 session） | **常驻**，同上 |

另有一条非 PTY 的：`CrewLocalMentionWaker.scan` 在**主线程**上读 + JSON 解白板
（`MultiProcessJSONStore.decodeRows`），稳态占主线程 **1.5%**。

`#1` 就是「派的活越多越卡」的直接答案：**每起一个 session，主线程被占死 45 秒**。

---

## 场景一：开着不动，刚起了一个 session（11:55，采样 10 s）

进程 CPU 69–100%（`top`），**没有人在操作界面**。

主线程 8413 个采样点里 **8222（97.7%）** 落在 `__CFRUNLOOP_IS_SERVICING_THE_MAIN_
DISPATCH_QUEUE__` 之下 —— 也就是说这 10 秒里 run loop 几乎没有空当去处理事件和
绘制，用户视角就是「打字不跟手、滑动一顿一顿」。

叶子节点分布（`sample` 的 top-of-stack 汇总）：

```
hasBreakWhenPaired #1 in _quickHasGraphemeBreakBetween   1763
_StringGuts._opaqueComplexCharacterStride(startingAt:)   1684
_GraphemeBreakingState.shouldBreak(between:and:)         1230
_swift_stdlib_getGraphemeBreakProperty                   1226
String.distance(from:to:)                                1159
Unicode._GraphemeBreakProperty.init(from:)                861
                                            合计 7923 ≈ 主线程的 94%
```

符号化后的完整调用链（无一处推测）：

```
ActivityTerminalView.dataReceived(slice:)                 ← 主线程，每批 PTY 输出
  └ closure #1 in AgentTerminalSession.init  (AgentTerminalSession.swift:300)
     └ SessionLaunchParameterScanner.feed(_:now:)         (SessionLaunchParameterEcho.swift:184)
        └ SessionLaunchParameterVerdict.classify(_:model:effort:)  (:89)
           ├ SessionProfileEchoVerdict.squeeze(_:)        (SessionProfileSwitch.swift:92)  ← 94%
           └ SessionLaunchParameterVerdict.excerpt(...)   (:112)
```

**为什么是平方级** —— `squeeze` 的循环体：

```swift
while origin.count < text.count { origin.append(i) }
```

`String.count` 就是 `distance(from: startIndex, to: endIndex)`，对 Swift String 是
**O(n) 的 grapheme cluster 遍历**（不是 O(1)）。它在每个字符上被求值一次 → O(n²)。
`n` 是 `AnsiPlainTextTail` 的尾窗，`tailLimit: 8192`，超过 2 倍才截 → 实际 8192～16384。

实测（同一段代码，`swiftc -O`，输入是仿真的 claude TUI 尾窗：ASCII 为主 + 少量
CJK/emoji + 大量空白换行）：

| 尾窗 | 每次 `squeeze` |
|---|---|
| 2048 字符 | 4.26 ms |
| 4096 字符 | 15.87 ms |
| 8192 字符 | **63.64 ms** |
| 16384 字符 | **253.72 ms** |

claude 的 TUI 每秒重绘若干次 → 每秒若干笔 PTY 输出 → 单个 session 的 45 秒窗口就
足以把一整个核占满。这与 `top` 观察到的 69–100% 完全吻合。

---

## 场景二：稳态，1 个 session 在跑（12:00，采样 12 s）

`#1` 的窗口已过，那批 grapheme 叶子从 7923 掉到 53 —— **反证成立**：场景一的
主导项确实来自 45 秒窗口内的 `SessionLaunchParameterScanner`，不是别的东西。

主线程 10287 个采样点，PendingCrew 自己的帧按归属聚合：

| 主线程占比 | 归属 |
|---|---|
| 816 / 10287 = **7.9%** | `ActivityTerminalView.dataReceived` 整个子树（**1 个** session） |
| ↳ 590 = 5.7% | `AgentTerminalSession.swift:280` → `TypingActivityTracker.signature` |
| ↳ ~200 = 2.0% | `AgentTerminalSession.swift:281` → `SessionHealthScanner.feed`（`SessionHealth.swift:206/213`） |
| 173 / 10287 = **1.7%** | `MultiProcessJSONStore.withFileLock` ← `LocalWhiteboardStore.loadLockedReportingFailure` |
| 157 / 10287 = **1.5%** | `CrewLocalMentionWaker.scan(crewId:)` → `decodeRows` → `LocalWhiteboardMessage.init(from:)` |

**这一项是按 session 线性叠加的** —— 7.9% 是一个 session 的价钱。机长常态派 3～5 个，
就是主线程 24%～40% 被 PTY 扫描吃掉，全部发生在**不管你有没有在看那个终端**的前提下。

`SessionHealthScanner` / `RateLimitMenuScanner` 的实现是每笔输出：
`stripper.tail.lowercased()`（整窗新建一份 String）+ 逐条 `lower.contains(phrase)`。
一个 session 挂着 3 个独立的 `AnsiPlainTextTail`，共 11 条短语。实测：

| 尾窗 | 现状（3 scanner × 全窗 lowercased + 11 次 Character 级 contains） | 只 lowercased 一次 | 11 次 UTF-8 字节搜索 |
|---|---|---|---|
| 8192 | **6.33 ms** | 0.18 ms | 0.083 ms |
| 16384 | **10.25 ms** | 0.27 ms | 0.109 ms |

Foundation 的 `String.contains` 走 `BidirectionalCollection._range(of:anchored:backwards:)`，
是 grapheme 级的朴素搜索 —— 采样里 `Substring.subscript.getter`(211) 和
`Substring.index(_:offsetBy:)`(104) 就是它。短语表全是 ASCII，用字节搜索代价差两个数量级。

---

## 对三个高嫌疑点的裁决

1. **PTY 每批输出过主线程 / scrollback 10000 行** —— **坐实，但不是 SwiftTerm 的锅**。
   代价压倒性地来自我们挂在 `dataReceived` 上的三个旁路扫描器（上表 #1/#2/#3），
   不是终端渲染本身。SwiftTerm 的渲染在采样里几乎看不见。
   「每个 session 无论前台后台都要过主线程」这条结构性事实成立，但**在当前架构里
   把旁路扫描的代价打下去，就能拿到绝大部分收益**，不必等 session 搬进后台进程。
2. **后台观察者挂在 SwiftUI 视图上、定时器叠加** —— **本次采样没有证据**。
   `CrewRelayAgent` / `LocalAgentUsageMonitor` / `QuotaCenter` 在两份采样的主线程上
   都没有出现。唯一出现的是 `CrewLocalMentionWaker.scan`（1.5%），且只有一份在跑，
   没有看到重复实例或叠加定时器。**这条判为未坐实**（不等于不存在，等于本次没抓到）。
3. **白板目录文件数 + 全量重读 + 主线程解 JSON** —— **部分坐实，但量级远小于 #1**。
   `CrewLocalMentionWaker.scan` 确实在**主线程**上取文件锁 + 全量 `decodeRows`
   解白板 JSON，合计约 3.2%（1.7% 锁 + 1.5% 解码）。是真问题，但不是本次卡顿的主因。
   目录扫描本身（`getattrlistbulk`）只有 68 个采样点，可忽略。

---

## 已知边界

- 两份采样都是 **1 个** session 在跑的情况。「2 个以上 session 同时打字/滚动」的
  场景没有直接采到 —— 机长在群里明确说了这期间不宜多派 session（每起一个就是 45 秒
  主线程被占）。按 #2/#3 的代价结构，多 session 是**线性叠加**，这是推断不是实测，
  报告里不当成实测结论。
- 修完之后的**现场复采**需要装一次新版 app（要人按 ⌘Q）。在那之前，修前/修后的
  对比数字来自函数级基准和回归测试，不是线上进程采样 —— 这条尾巴记在 tech-debt。

---

## 修了什么 · 修前/修后

四条，一条一个提交，每条都带回归单测。全量测试 **1426 条 0 失败**
（本分支基线 1419 条 0 失败，新增 7 条）。

| 修改 | 修前 | 修后 | 倍数 |
|---|---|---|---|
| `SessionProfileEchoVerdict.squeeze` O(n²)→O(n)<br>（每笔 PTY 输出，45 秒窗口内） | 63.6 ms（8K）<br>253.7 ms（16K） | 0.52 ms<br>0.94 ms | **122×**<br>**270×** |
| 健康/菜单短语改 ASCII 字节匹配<br>（每笔 PTY 输出，**常驻**） | 6.33 ms（8K）<br>10.25 ms（16K） | 0.083 ms<br>0.109 ms | **76×**<br>**94×** |
| 「正在输入」指纹单趟折叠 + 够 512 字符就停<br>（每笔 PTY 输出，**常驻**） | 与输入长度成正比 | 与输入长度**脱钩** | 长输入下量级差 |
| 点名唤醒器补文件指纹门<br>（每个目录 tick，本机全部 crew） | 9～11 ms | 0.07～0.10 ms | **110～124×** |

数字口径：前三条是 `swiftc -O` 下对同一段代码的函数级实测（输入是仿真 claude TUI
尾窗）；第四条是对本机真实白板目录（67 个 json / 3.8 MB）实测。

回归单测（都进了仓库，跟着全量跑）：

- `squeeze` 与老实现**逐字符对拍**（老实现原样拷进测试当参照系）+ 16K 尾窗耗时红线；
- 字节匹配与老的 `lowercased()+contains` 在**整张短语表** × 三种输入上对拍
  （原样 / 全大写 / 差一个字符）+ 截窗后镜像仍同步 + 整窗匹配耗时红线；
- 指纹与老实现在 3 种 `limit` × 10 种输入上逐字符对拍（含 900 连组合记号、ZWJ、
  旗帜、4000 字长文）+ 「代价与输入长度脱钩」红线；
- 指纹门依赖的性质本来就有单测（`DirectoryWatchCoalescingTests`：白板 append 必
  yield、别人写状态文件不 yield、别的 crew 变了不 yield、文件从无到有算变化）。

## 现场复采（2026-08-19 13:15–13:21，装上新版之后）

**结论：四条全部在真进程上兑现，没有一条被推翻。** 复采条件按上一节写死的两条判据核，
两条都过；另外抓到一条修前被 `squeeze` 完全淹没、现在浮上来当第一名的新项目（见文末）。

### 复采环境

- 采样对象：`/Applications/PendingCrew.app`，0.1.13 (**20684.16770**)，pid 1343，macOS 26.6.1 / arm64。
- **包里刻的提交 = `main` HEAD**：`Info.plist` 的 `BuildStampCommit = a4f8f5dfd8c4…`，
  与采样时的 `git rev-parse HEAD` 一致，工作区干净 —— 跑着的确实是带这四条修复的版本。
- 符号化：线上二进制仍是 strip 的（`nm -U` 只剩 579 个定义符号，自己的帧一律 `???`）。
  用**同一 commit 本地重编的 Release** 二进制 + `atos -offset` 符号化。
  等价性核过：两者 arm64 `__TEXT` 的 `vmaddr 0x100000000` / `vmsize 0x960000` 完全相同。
  934 个待符号化偏移解出 931 个。
- 工具：`sample(1)` + `top(1)`。**没有驱动任何图形界面、没有模拟输入**；负载由机长在群里派活制造。
- 原始采样件不入库，留在会话 scratchpad（`s0-current` / `s1-steady8` / `s2-launchA` / `s3-steadyB`）。

### 四份采样

| 编号 | 时刻 | 时长 | 现场 |
|---|---|---|---|
| S0 | 13:15:52 | 10 s | 7 个 session 在跑，**其中 3 个正处在拉起后的 45 秒窗口内** |
| S1 | 13:17:33 | 12 s | 9 个 session，1 个仍在窗口内 |
| **S2**（场景 A） | 13:18:17 | 90 s | 10 个 session；机长于 13:18:47 起了 1 个新 session，**它的 45 秒窗口整段落在采样内** |
| **S3**（场景 B） | 13:20:34 | 30 s | 10 个 session、其中 6 个在忙；**没有任何 session 处在窗口内**（启动参数扫描器采样点为 0，反证窗口确实只有 45 秒） |

### 主线程占用：修前 vs 修后

百分比 = 该子树采样点 / 该次采样主线程总采样点（S0 8463、S1 10197、S2 76126、S3 24554）。

| 项 | **修前**（2026-08-19 上午，同机同工具） | S0 | S1 | **S2 场景 A** | **S3 场景 B** |
|---|---|---|---|---|---|
| run loop **在干活**（`__CFRUNLOOP_IS_SERVICING_THE_MAIN_DISPATCH_QUEUE__`） | **97.7%**（*1 个* session 的窗口内） | 4.62% | 2.91% | **2.82%** | **2.16%** |
| run loop **空闲**（`mach_msg2_trap`） | ≈ 0 | 84.4% | 94.3% | **92.2%** | **93.3%** |
| `ActivityTerminalView.dataReceived` 整个子树 | **7.9%**（*1 个*忙碌 session） | 4.08% | 2.50% | **2.25%**（10 个） | **1.51%**（10 个 / 6 个在忙） |
| ↳ `SessionLaunchParameterScanner.feed`（那段平方级的宿主） | 97.7% 的主体 | 2.28% | 1.15% | **0.67%** | **0**（无窗口） |
| ↳↳ `SessionProfileEchoVerdict.squeeze` 本身 | **94%**（叶子合计 7923/8413） | 1.42% | 0.90% | **0.49%** | 0 |
| ↳ `SessionHealthScanner.feed` + `RateLimitMenuScanner.feed` | **≈2.0%**（*1 个* session） | 0.82% | 0.72% | 0.78% | **0.67%**（10 个） |
| ↳ `TypingActivityTracker`（「正在输入」指纹） | **5.7%**（*1 个* session） | 0.06% | 0.02% | 0.05% | **0.04%** |
| `CrewLocalMentionWaker.scan`（白板重读） | **1.5%** | 0.07% | 0.02% | 0.07% | **0.05%** |
| `MultiProcessJSONStore.withFileLock` | **1.7%** | 0.27% | 0.01% | 0.13% | **0.09%** |
| `MultiProcessJSONStore.decodeRows` | 含在上面 | 0.35% | 0.07% | 0.20% | **0.14%** |

### 进程 CPU

`top -pid 1343 -s 2`，每格 2 秒（丢掉 top 恒为 0 的第一格）：

| | 修前 | S2（45 格 / 90 s） | S3（15 格 / 30 s） |
|---|---|---|---|
| 场景 | *1 个* session 刚拉起、无人操作界面 | 10 个 session、期间起了 1 个新的 | 10 个 session、6 个在忙 |
| CPU | **69–100%**（烧满一个核） | 中位 **8.2%**，5–10% 占 39/45 格 | 中位 **7.4%**，6–9% 占 12/15 格 |
| 尖峰 | 持续 45 秒不下来 | 偶发 21/25/33/33/38/60%，都是单格（≤2 s）、不连续 | 12/23/63%，同样是单格 |

尖峰是瞬时的、且主线程在整段采样里 92–93% 都空闲，所以它们不构成「打字不跟手」那种
持续卡顿。没有进一步坐实尖峰的来源，**不当成结论写**。

### 对两条判据的裁决

1. **场景 A（刚起一个 session 的头 45 秒）—— 过，但判据的措辞要修正。**
   原判据写的是「采样里应当**看不到** `squeeze` / `_opaqueComplexCharacterStride` 那一族叶子」。
   实测**仍然看得到**，因为这段代码本来就还在跑（`--model` / `--effort` 回显该验还得验）——
   它不是被删掉，是单价掉了两个数量级。诚实的说法是：`squeeze` 从主线程的 **94% 掉到 0.49%**，
   `SessionLaunchParameterScanner` 整个子树从**主导项**掉到 **0.67%**，
   run loop 从 **97.7% 在干活**变成 **92.2% 空闲**，CPU 从 **69–100%** 变成中位 **8.2%**。
   S0 更狠：**3 个 session 的 45 秒窗口叠在一起**，`dataReceived` 子树也只有 4.08%。
2. **场景 B（多 session 稳态）—— 过，而且是本次唯一一次真正实测到多 session。**
   上一轮的 7.9% 是 **1 个**忙碌 session 的价钱，多 session 只是按线性叠加**推断**的。
   S3 实测：**10 个 session（6 个在忙）合计 1.51%** —— 按在忙的算 **≈0.25%/session**，
   比修前的 7.9%/session 低 **约 32 倍**；按全部 10 个算 0.15%/session。
   换句话说，修前 5 个忙碌 session ≈ 主线程 40%，修后 10 个 session ≈ 主线程 1.5%。

**没有任何一条函数级结论被复采推翻。**

### 复采顺带抓到的两件事（都没修，只登记）

1. **主线程上现在最大的单项不再是 PTY 扫描，而是一次目录枚举。**
   `CrewStore.applyPendingRenames()` → `LocalCrewControlStore.drainRenames()`
   → `-[NSFileManager contentsOfDirectoryAtURL:…]` → `getattrlistbulk`，**在主线程**，
   四份采样里稳定占 **0.44%–0.72%**（S2 里连外层闭包算是 1.34%）。
   它和已修的第 4 条（唤醒器全量重读）是同一族病：**主线程上的目录/文件轮询**。
   量级比修前的任何一项都小两个数量级，但既然它现在是第一名，登记进 `docs/tech-debt.md`。
2. **健康扫描的残余不在匹配上，在喂尾窗上。**
   `SessionHealthScanner.feed` 的 0.47% 里有 0.4 个百分点落在 `AnsiPlainTextTail.feed`
   的 `String.distance(from:to:)`（尾窗裁剪的 grapheme 走查）——
   也就是说改成字节匹配之后，**匹配本身在采样里已经看不见了**，剩下的是维护尾窗那一步。
   量级极小（全场 0.55%/10 个 session），一并登记，不在本次动。
