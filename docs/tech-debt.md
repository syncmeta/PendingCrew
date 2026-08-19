# 技术债 / 项目根基风险登记册

这个文件是 PendingCrew 的**结构性问题登记册**，由各个 Claude Code / Codex session 在日常开发中**顺手记录**，不是一次性审计的产物。

记什么（按严重度从高到低）：

- 🔴 **根基级** — 影响整个项目、让根基不稳的问题。错误的数据模型、会随规模放大的设计错误、跨模块的隐性耦合、安全/数据一致性隐患之类。
- 🟡 **拆东墙补西墙** — 为了让 A 跑通而硬塞进去、把代价转嫁到 B 的修复；绕过类型/校验/约束的 workaround；复制粘贴而非抽象；"先这样以后再说"的临时实现。
- 🟢 **不规范** — 偏离本仓库既定约定的写法、命名、结构；不致命但会慢慢腐蚀一致性。

**不记**：linter/typecheck 已经能抓的纯风格问题、个人口味、与项目健康无关的琐碎 TODO。

---

### 🔴 所有在跑 session 的 PTY 输出都要过主线程 —— 界面代价随派活数线性增长
- **发现**: 2026-08-19 · `fix/ui-jank-pty-scan`（Todo #59 界面卡顿排查）
- **位置**: `Sources/Mac/LocalRunner/AgentTerminalSession.swift` 的 `dataReceived` 回调（`ActivityTerminalView.dataReceived(slice:)` → `MainActor.assumeIsolated { … }`）。
- **问题**: session 本体是 `LocalProcessTerminalView`（SwiftTerm 的 `NSView` 子类），PTY 每一批输出都在**主线程**交付，**不管这个 session 在不在前台、用户有没有在看它**。挂在这条回调上的旁路工作（打字指纹、健康扫描、菜单检测、启动参数回显、切档回显）因此全部是主线程同步工作，且**按在跑的 session 数线性叠加**。
- **证据**: `docs/2026-08-19-ui-jank-profile.md`。sample 实测：单个忙碌 session 的 `dataReceived` 子树占主线程 **7.9%**；机长常态派 3～5 个 → 24%～40%。
- **本次做了什么**: 把这条回调上**每一段**旁路工作的单价打下去了（squeeze O(n²)→O(n)、健康短语改字节匹配、打字指纹单趟折叠），合计把单 session 的稳态占比压掉一个数量级以上，并消掉了「每起一个 session 主线程被占死 45 秒」那条。
- **没做也不该在这条分支上做的**: **结构本身没变** —— 代价仍然随 session 数线性涨，仍然全在主线程，仍然与「用户在看哪个终端」无关。真正的解法是把 session 搬进常驻后台进程（机长 2026-08-19 已在单独排期，与「前后端分离」同一刀），app 退化成看的那个窗口。**这条不要顺手在功能分支里动。**
- **修后复采（2026-08-19）**: 结构没变，但单价打下去之后实测 **10 个 session（6 个在忙）合计只占主线程 1.51%**（≈0.25%/忙碌 session）。
  也就是说这条 🔴 现在**不再是当前的卡顿来源**，它仍然是 🔴 是因为「代价随 session 数线性涨、且与用户在看哪个终端无关」这条结构事实没变 ——
  派到几十个 session 时它会重新变成主导项。见 `docs/2026-08-19-ui-jank-profile.md`「现场复采」。
- **中间态的可选缓解**（如果地基活拖久了）: 后台 session 的旁路扫描改成攒批 / 挪出主线程；或把 scrollback 与扫描窗按「是否前台」分档。都属于治标，记在这里免得被当成已解决。

### 🔴 登录态 session 的信箱唤醒与审批中继**从未接通** —— `serverLink` 写死 nil
- **发现**: 2026-08-19 · 前后端分离 P0（所有权归拢）
- **位置**: `Sources/Mac/Views/CrewSessionWindowView.swift` 手动起 session 那条路里的 `let serverLink: CrewSessionServerLink? = nil`；实现在 `Sources/Mac/Services/CrewSessionRunner.swift` 的 `ensureMailboxWaker` / `ensurePermissionRelay`。
- **问题**: 这两个服务原本由视图在 run 起好后接线，两个调用点**都在死路上** —— 一条被上面那个写死的 `nil` 挡着，另一条在被 `edgeQueueBindingReady == false` 关着的 auto-claim 死循环里。也就是说它们**一次都没被调用过**。调研清单（`docs/2026-08-19-backend-split-inventory.md` A19/A20）当时的判断是「右栏没打开过的 session 才没接」，实际比这更糟：**所有 session 都没接**。
- **症状**: 登录态下 edge 信箱的定向投递不会唤醒本机 session；远端 viewer 的审批镜像（#204 permission over WS）不生效 —— 都是静默不工作，没有任何报错。
- **根因**: edge session 通道（接合 v2 block 3，本地 crew ↔ edge 行的绑定）没开，所以 `serverLink` 一直是 nil。不是这两个服务本身有问题。
- **P0 做了什么**: 只删掉视图侧那段永不执行的接线（连同 auto-claim 死循环），**实现原样留在 runner 上并加了注释说明当前无调用点**。P0 的约束是行为零变化，真接上属于行为变化，不在本阶段做。
- **解法归属**: P4（编排整体搬进 daemon，届时由 runner 侧统一接线，别再从视图接）或云端那条轴（先把 edge session 通道打开）。

### 🟡 点名唤醒器把「读增量」当成廉价操作 —— 每个目录 tick 全量重解白板
- **发现**: 2026-08-19 · `fix/ui-jank-pty-scan`（同上）
- **位置**: `Sources/Mac/Services/CrewLocalMentionWaker.swift` 的 `directoryChanged` 扇出；同款注释「读增量靠游标，很廉价；与 listen 路同款策略」。
- **问题**: 目录事件不带文件名，所以一个 tick 要把 `watched` 里每个 crew 都扫一遍；而「扫」= 取文件锁 + 整份读 + 整份 JSON 解码，**游标只裁剪解完之后的行，读和解一分钱不省**。本机白板目录 67 个 json / 3.8 MB，全量走一遍实测 **9～11 ms**，全在主线程；helper 子进程每发一条 `post_to_crew` 就是一个 tick。
- **本次做了什么**: 给唤醒器补上 `FileChangeGate` 文件指纹门（那正是 #443 建它时写明的用途）：9～11 ms → 0.07～0.10 ms。
- **留着的尾巴**: 注释里点名的「listen 路同款策略」**没查**，很可能同病；另外 `LocalWhiteboardStore.list` 本身仍是「每次调用整份读+整份解」，没有按文件指纹缓存解码结果 —— 指纹门只是让**不必要的调用**不发生，真正需要读的那次仍然是全量。白板越长这一下越贵（本机最大的一份已经 860 KB）。

### ✅ 卡顿修复的现场复采（**已还，2026-08-19**）
- **原问题**: 四条修改的「修前/修后」数字只到函数级（`swiftc -O` 实测 + 耗时红线单测），没有对线上进程 `sample` 复采过 —— 因为复采要先装一次新版 app。
- **已完成**: 新版（0.1.13 / 20684.16770，`BuildStampCommit` = `a4f8f5d` = 当时的 main HEAD）装好后，对真进程做了四份符号化采样，两个场景都覆盖到了。结果贴在 `docs/2026-08-19-ui-jank-profile.md` 的「现场复采」一节。
- **裁决**: **四条全部兑现，没有一条被推翻**。run loop 从「97.7% 在干活」变成「92～93% 空闲」；`dataReceived` 子树从 7.9%/session 变成 10 个 session 合计 1.51%（≈0.25%/忙碌 session，约 32 倍）；进程 CPU 从 69–100% 变成中位 7～8%。**场景 B（多 session 稳态）是本次第一次真正实测到，此前只是线性叠加的推断。**
- **一处措辞修正**: 原判据写「应当看不到 `squeeze` 那一族叶子」是错的 —— 那段代码本来就还该跑，正确的判据是单价，实测从主线程 94% 掉到 0.49%。
- Todo #59 据此翻 completed。

### 🟢 主线程上还剩一次目录枚举 —— `drainRenames` 每 tick 全量列目录
- **发现**: 2026-08-19 · Todo #59 现场复采
- **位置**: `Sources/Stores/LocalCrewControlStore.swift:49` `drainRenames()`，经 `CrewStore.applyPendingRenames()`（`CrewStore.swift:379`）挂在 `startRenameWatchIfNeeded` 的监视回调上。
- **问题**: 每个 tick 都 `-[NSFileManager contentsOfDirectoryAtURL:…]` 全量列一遍目录（`getattrlistbulk`），**在主线程**。与已修的第 4 条（唤醒器全量重读白板）是同一族病：主线程上的目录/文件轮询。
- **量级**: 四份采样里稳定占主线程 **0.44%–0.72%**（连外层闭包 1.34%）。比修前任何一项都小两个数量级，但**修完那四条之后，它是主线程上最大的单项**。
- **没做**: 本次只登记，没动 —— 复采那条分支的职责是验数字，不是顺手改。可仿照唤醒器补一道文件指纹门。

### 🟢 健康扫描的残余落在尾窗维护上，不在关键词匹配上
- **发现**: 2026-08-19 · Todo #59 现场复采
- **位置**: `Sources/Mac/LocalRunner/SessionHealth.swift` 的 `AnsiPlainTextTail.feed(_:)`。
- **问题**: 短语匹配改成 ASCII 字节搜索之后，**匹配本身在采样里已经看不见了**；`SessionHealthScanner.feed` 剩下的 0.47% 里约 0.4 个百分点落在 `AnsiPlainTextTail.feed` 内部的 `String.distance(from:to:)` —— 尾窗裁剪时按 grapheme 走查长度。
- **量级**: 10 个 session 合计 0.55%，可忽略。登记只是为了留个坐标：下次谁再来压这条回调，第一刀应该切在尾窗裁剪（按 UTF-8 字节裁）而不是匹配上。

