# 「待审批列表这次读不出来」——查一个被判过一次「没事」的故障（2026-09-08）

计划 #48。前置：`2026-09-05-whiteboard-oneway-outage-capture.md`（白板那一侧的现场读数）。

三类分开写：**实测**（我跑了、亲眼看到输出）/ **读代码读到的**（白纸黑字）/ **推的**。
每一节自己标。最后一节是「我哪段没做到」——**那一节不是谦辞，是读数的边界**。

---

## 0. 结论先说

1. **改掉的根因**：那句提示把**查下去所需要的两样东西亲手删掉了** —— 底层 errno 和
   绝对路径。两样都本来就装在同一个错误对象里。这不是「查不下去」，是我们自己造的
   信息黑洞，于是每一个来查的人都从同一个地方重新开始猜。**已修 + 已测 + 已变异自证。**
2. **没改掉的根因**：`EPERM` 本身是谁发的，**没查清**。9-05 那次现场把它定位到
   内核层、且只打在 claude 谱系进程上，但「谁吊销、为什么」至今没有被直接观测到。
   我这一趟**没能复现**（下面第 5 节写清了我试了什么）。
3. **两个现场是不是同一个病**：**证据指向「不是同一次事件，但很可能是同一条通道」**，
   依据在第 4 节 —— 而且**判定它的那个数据，恰恰就是被扔掉的那一个**。修完之后，
   下一次发作会自己回答这个问题。

---

## 1. 既有痕迹的时间线（实测）

**先把口径说清楚，因为我数出来的跟机长报的不一样。**

`grep -o "未能打开文件"` 全目录 = **18 次**。逐层剥：

| 口径 | 次数 | 说明 |
|---|---|---|
| 全目录原始命中 | 18 | 含 `crew-sessions.json` 里**我自己这次的任务 brief**（2 次，不是故障） |
| 白板 / 账本文件里的命中 | 16 | 我上一趟 `json.load` 扫出来的数，与裸 grep 对得上（18−2） |
| 其中**真正的系统警示**（`senderKind=pendingcrew`，即故障本身） | **10** | **9 条 approvals + 1 条 todos，跨 7 个 crew** |
| 其余 6 条 | 6 | 机长/人转述与引用（Todo #97 正文、#48 计划正文、三条机长报告） |

**机长报的是「11 approvals + 1 todos、8 个 crew」，我按上表数是「9 + 1、7 个 crew」。**
差值我没能对齐 —— 我把每一条的 id/时刻/crew 都列在下面，谁要复核直接对这张表；
**如果错的是我，那就是这张表里少了两行，不是数字算错。**（两个数都不影响本文任何
结论：性质、时间线形状、根因判断在 9 条和 11 条上是一样的。）

| 时刻 (UTC) | 账本 | crew |
|---|---|---|
| 2026-09-01T04:46:02Z | approvals | 71e4de6f |
| 2026-09-02T05:40:14Z | approvals | 71b7fd5b |
| 2026-09-03T02:07:44Z | approvals | 51aaafbf |
| 2026-09-04T01:48:20Z | approvals | ca06cc2a |
| 2026-09-05T13:12:44Z | approvals | 71e4de6f |
| 2026-09-07T12:05:18Z | approvals | 71e4de6f |
| 2026-09-07T13:44:32Z | **todos** | 71e4de6f |
| 2026-09-08T00:25:46Z | approvals | 17c599b2 |
| 2026-09-08T02:09:51Z | approvals | 102ca280 |
| 2026-09-08T08:25:39Z | approvals | 480a5577 |

（另有 09-04 06:40 / 09-05 03:10 两条是**白板写失败**那一族，机长在群里转述，
不是 approvals。）

### 1.1 跟什么同时发生 —— 三条对齐，两条否定一条正面

**否定一：跟后台重启无关（实测）。**
把白板里全部「后台进程重启」系统消息拉出来做 ±30 分钟对齐，**只有 1 次**
附近有重启，而且那次重启在故障之后 17 分钟。

**否定二：跟并发写入无关（实测）。**
全库 12365 条消息、跨度 57 天，±60s 窗口的期望值是 0.30 条。对每次故障数窗口：

```
2026-09-02  ±60s: 1   ±300s: 1
2026-09-03  ±60s: 1   ±300s: 1
2026-09-04  ±60s: 1   ±300s: 1
2026-09-05  ±60s: 1   ±300s: 1
2026-09-07  ±60s: 1   ±300s: 1   （两次都是）
2026-09-08  ±60s: 1   ±300s: 1   （三次都是）
```

`1` = 那五分钟里**只有这条故障消息本身**。**8 次故障发生在完全安静的时刻。**
所以「大家一起写、撞上了」这条可以划掉 —— 它恰恰发生在没人写的时候。

**正面（弱，但值得记）：两次落在唤醒的那一秒（实测）。**
`pmset -g log` 覆盖 2026-09-01 23:53 +0800 起（09-01 那次在日志开始之前，对不了）。
**11 次可对齐**（10 条系统警示里的 9 条 + 两条白板写失败现场）里：

```
2026-09-02T05:40:14Z   Δ  -2s   Wake from Deep Idle
2026-09-08T00:25:46Z   Δ  ±0s   DarkWake to FullWake（due to UserActivity）
其余 9 次              Δ 数千~数万秒，无关
```

7 天里 2879 个电源事件，随机落在某个事件 ±2s 内的概率约 1.9%/次；11 次里中 2 次
≈ 2% 的偶然概率。**这不足以下结论，只够立一条待验的线索**：唤醒瞬间可能是
其中一类触发器（不是全部——另外 9 次跟电源毫无关系）。

### 1.2 fd 耗尽（8-12 那次的真身）这次不成立（实测，**非发作时刻**）

```
kern.maxfilesperproc = 122880       launchctl limit maxfiles = 256 unlimited
界面进程 15213  打开 fd 106         后台 15229  打开 fd 30
7 个 mcp helper 各 11~12
setrlimit(65536) 在本机实测成功（软上限 256 → 65536）
```

`PendingCrewEntry` 启动时抬软上限那一步是有效的，helper 分支之前就抬（读代码读到的
+ 实测复刻）。**边界：这是非发作时刻量的，它证明的是「平时离上限很远」，
不是「发作那一刻没顶穿」。** 要证后者只能在发作当口量。

---

## 2. 那句错误信息是谁产生的、从哪一层冒出来的（实测 + 读代码读到的）

**产生点（读代码读到的）**：
`MultiProcessJSONStore.readDataIfExists` 里的 `Data(contentsOf:)` 抛
`NSCocoaErrorDomain 257`（`NSFileReadNoPermissionError`）
→ `loadRowsLocked` 吞成 `[]` 并回调 `.unreadable(error)`
→ `LocalApprovalStore.reportIncident` 拼「待审批/待决策列表：」+ `incident.summary`
→ 落白板 → 注入面。

白板那一侧是同一条路的另一个出口：
`LocalWhiteboardStore.appendReportingFailure` 里的 `loadLockedReportingFailure` 抛
→ 包成 `WhiteboardPersistenceError.unreadableAndPreserved` → 回执给 agent。

**「只有文件名、没有路径」这条假设，证伪了（实测）。**
写了个最小程序造同一个错误（`chmod 000` + `Data(contentsOf:)`）：

```
domain=NSCocoaErrorDomain code=257
localizedDescription = The file "sample.approvals.json" couldn't be opened
                       because you don't have permission to view it.
userInfo keys        = ["NSFilePath", "NSURL", "NSUnderlyingError"]
NSFilePath           = /var/folders/…/errfmt-36671/sample.approvals.json
NSUnderlyingError    = NSPOSIXErrorDomain Code=13 "Permission denied"
```

→ **`localizedDescription` 天生只取 `lastPathComponent`，跟数据根对不对没有一点关系。**
「读的进程数据根不对」那条假设**不能**用这句话作证据。
（它本身还没被证伪，只是这条证据不成立；修完之后路径会直接印出来，一眼可判。）

→ **而路径和 errno 一直就在同一个对象里。是我们在写文案时扔的。**

**这一层的丢弃造成了什么（实测，有原文）**：2026-09-01 的 Agent Todo #97，
机长第二条更新原话——

> 已确认不是文件权限或损坏：原件 644、20 KB、完整 JSON 且当前可读，数据安全专项
> 12/12 通过。提示来自瞬时 open/read 失败；**旧提示未保存底层 errno，无法事后证明
> 具体是哪种系统错误。**

然后这条 Todo 被翻成 completed。**一个被反复放弃的调查，本身就是这个缺陷的证据。**

### 2.1 为什么 errno 是决定性的（实测）

三个 errno 走同一条 Foundation 映射、`localizedDescription` **一字不差**，
可它们是三种病、三个「该找谁」：

| errno | 是什么 | 该找谁 |
|---|---|---|
| `EMFILE`/`ENFILE` | 句柄耗尽（8-12 那次的真身） | 抬上限、收敛文件数 |
| `EACCES` (13) | 文件权限位真的不给读 | `ls -l` |
| `EPERM` (1) | **环境层**拒绝（沙盒 / TCC / 授权吊销） | 跟文件无关，查授权 |

测试里有一条断言专门钉这个前提（三者 `localizedDescription` 去重后 == 1 条）。

---

## 3. 修了什么（已测 + 已变异自证）

`3e3df34`：

1. `MultiProcessJSONStore.diagnose(_:fallbackPath:)` —— 沿 `NSUnderlyingErrorKey`
   链把 errno（带名字）、Cocoa code、绝对路径取出来；错误对象没带路径时**退到读的人
   自己用的那个 URL**（那才是回答「数据根对不对」的东西）。
2. approvals 的 `.unreadable` 与白板的 `.unreadableAndPreserved` 都接上它。
   **两个现场第一次共用同一份诊断** —— 它们本来就是同一个 `open()` 的两张脸，
   以前文案各写各的，于是同一个病看起来像两件事。
3. 白板那句里的「且**归档失败**」删掉：读不出来时我们从来不试归档（8-12 P0 的
   不变式），那半句是假的。
4. 新增**只写不读**的持久痕迹 `whiteboards/diagnostics/read-failures.log`
   （时刻 / errno / 绝对路径 / pid / **argv 原样**，512KB 轮转、不静默停）。

**第 4 条为什么必须存在（读代码读到的）**：读失败的播报是往白板 `append`，
而 `append` 自己**要先把白板读出来**（`appendReportingFailure` → `loadLockedReportingFailure`）。
同一刻两个文件都读不出来时，那条播报被 `_ = try?` 静默吞掉。
**于是「这个错到底发作过多少次」永远查不清，而它是排查里最便宜的那份证据。**

**为什么记 argv 而不是记「角色」**：`ProcessRole` 是 `#if os(macOS)`，这一层同时编进
iOS；何况**原始 argv 比一个我们自己算出来的二手结论硬** —— `--mcp-serve` /
`--daemon` / 什么都没有，读的人自己就分得出 helper / 后台 / 界面。

**红→绿 + 变异自证（实测）**：7 条新测试。两次变异：
- 把文案退回只印 `localizedDescription` → `testUnreadableSummaryCarriesErrnoAndAbsolutePath`
  与 `testWhiteboardUnreadableReceiptCarriesErrnoAndAbsolutePath` 具名红（4 个断言）；
- 拿掉留痕那一行 → 两条痕迹测试具名红。

全量 macOS `Executed 2229 tests, with 11 tests skipped and 0 failures`；iOS build 通过。

---

## 4. 两个现场是不是同一个病

**A = approvals/todos 读失败**（11+1 次，本文第 1 节的表）
**B = 白板写失败**（#48 原案、9-05 抓到现场那一次）

### 同的部分（读代码读到的，硬）

**同一个 `open()`、同一条错误、同一个基座。** A 和 B 都是
`MultiProcessJSONStore.readDataIfExists` 里那一次 `Data(contentsOf:)` 失败；
B 只是因为写路径必须先读，所以表现成「写不进去」。
**它们从来不是两个机制，是一个机制的两个出口。**

### 但很可能不是同一次事件（这一条是**推的**，前提是读代码读到的）

9-05 那次现场量到的 B 是**整棵子树、持续 8 分钟、只打 claude 谱系进程**的拒绝。
如果 A 也发生在那种窗口里，那么：

- 读 approvals 失败 → `reportIncident` 去 append 白板 → **append 要先读白板** →
  白板在同一棵子树里、同一个窗口内 → 读也失败 → **整条播报被静默吞掉**。

**换句话说：那 11 条我们能看见的 approvals 警示，恰恰证明写它的那个进程当时
「读得动白板、读不动 approvals」。** 这跟 B 的「整棵子树一起拒」不是同一个形状。

**这条推理的洞我自己标出来**：9-05 那份现场的 §9.1 量到**恢复是逐对象的**——
同一棵子树里不同文件先后解禁。所以「白板通、approvals 不通」在 B 的框架里
**也解释得通**，只要那一刻正好落在逐对象解禁的中间。**所以我不能把它说成结论。**

### 判定它的那个数据，正是被扔掉的那一个

- 若下次痕迹里 `errno=1 EPERM` + `argv=--mcp-serve…` → 同一个病（claude 谱系被拒）。
- 若是 `errno=1 EPERM` + `argv=--daemon` 或空 argv（界面）→ **不同的病**：
  9-05 明确量到那次 app 自己读写正常。
- 若是 `EMFILE` → 又是另一件事（8-12 的老病复发）。

**修完之后这个问题会自己回答。这就是这次修改的实际用途。**

---

## 5. 我哪段没做到（这一节是读数的边界，不是谦辞）

1. **我没有复现。** 探针窗口 **09:05Z–09:32Z（27 分钟）**，跟受害者同一条进程链
   （`python3 ← zsh ← claude ← PendingCrew --daemon ← PendingCrew`），5 秒一拍 +
   1 秒一拍两层，对照组按 9-05 那份现场的要求包含**别的 app 的 Application Support
   子目录**（`Code/`、`Claude/`）与子树之外的对照（`~/.claude.json`）。
   **全程 OK，一次都没红。** 这段时间里也没有新的白板警示出现——
   **所以这是「这 27 分钟没发作」，不是「复现不了」。** 历史频率是 1~3 次/天，
   按这个频率，27 分钟窗口本来就大概率什么都抓不到 —— **这个窗口太短，不构成证据。**
   探针脚本在本 session 的 scratchpad 里、**不入库也不长驻**，session 一收它就没了；
   仓库里已有的 `scripts/diag/whiteboard-access-probe.sh` 是同类工具、覆盖面更全，
   要长期蹲点用那个。**这次改动落下的那条 `diagnostics/read-failures.log` 才是
   不依赖任何人在场的那一半。**
2. **我没有主动触发它。** 唯一像样的候选是「让机器睡一次再唤醒」（第 1.1 节那两次
   对齐）。那要动人的机器（合盖 / `pmset sleepnow`），属于会打断人的动作，没做。
   **这是一条明确的、下一步就能做的实验，我把它留给了拍板的人。**
3. **`EPERM` 的发出者仍然未知。** 9-05 那份现场已经把它钉到内核层、且排除了 ACL /
   flags / provenance / 目录名 / 两份签名 / agent 权限层。剩下的「sandbox extension
   被吊销」是**推的、未观测到吊销事件**。我这一趟没有推进它——
   我拿到的新数据（tccd 30 小时全量里 `Failed to match existing code requirement`
   只有 **2 条**、都是 `kTCCServicePhotos`、且都紧贴一次 app 重启、跟 12 次故障
   一次都对不上）**只是又一次否定**，不是进展。
4. **1.2 节那组 fd 读数是非发作时刻量的。** 它说明「平时离上限很远」，
   **说明不了发作那一刻**。这正是 9-01 那次的错误形状（拿非故障时刻的「文件完整」
   去回答「为什么读不出来」），我不重复它，所以在原地标住。
5. **第 4 节的「不是同一次事件」是推的。** 前提是读代码读到的（append 先读），
   但它有一个已知的反例路径（逐对象解禁），我在那一节里写明了。
6. **1.1 节那条「两次落在唤醒那一秒」我没有把它做实。** 1.6% 的偶然概率不算低到
   可以下结论，而且它只覆盖 10 次里的 2 次。它现在是线索，不是原因。
7. **文案改动没有在真 app 上跑过一次。** 单测里造的是 `EACCES`（`chmod 000`），
   真现场是 `EPERM`；两者走同一段代码、同一张 errno 表，但**「测试里绿」和
   「下一次真发作时那句话是对的」之间还差一个真样本**。

---

## 6. 下一次发作时该做什么（给接手的人）

1. 先看 `~/Library/Application Support/PendingCrew/whiteboards/diagnostics/read-failures.log`
   —— errno / 绝对路径 / pid / argv 都在里面，**不需要任何人在场**。
2. 按第 4 节末尾那张表判「同一个病还是两件事」。
3. 若 argv 带 `--mcp-serve`/`--mcp-hook`（= claude 谱系），接
   `2026-09-05-whiteboard-oneway-outage-capture.md` 第 9 节那两个还没有模型罩住的
   矛盾（起点散 36 秒、终点聚 31 秒）继续；**别把那份文档里的两条反证拗回来当机制。**
