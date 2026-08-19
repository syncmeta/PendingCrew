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
