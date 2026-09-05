# 技术债 / 项目根基风险登记册

这个文件是 PendingCrew 的**结构性问题登记册**，由各个 Claude Code / Codex session 在日常开发中**顺手记录**，不是一次性审计的产物。

记什么（按严重度从高到低）：

- 🔴 **根基级** — 影响整个项目、让根基不稳的问题。错误的数据模型、会随规模放大的设计错误、跨模块的隐性耦合、安全/数据一致性隐患之类。
- 🟡 **拆东墙补西墙** — 为了让 A 跑通而硬塞进去、把代价转嫁到 B 的修复；绕过类型/校验/约束的 workaround；复制粘贴而非抽象；"先这样以后再说"的临时实现。
- 🟢 **不规范** — 偏离本仓库既定约定的写法、命名、结构；不致命但会慢慢腐蚀一致性。

**不记**：linter/typecheck 已经能抓的纯风格问题、个人口味、与项目健康无关的琐碎 TODO。

---

### ✅ session 协议的收包路径假设「一次投递 == 正好一整帧」（**P4 `f34d7c9` 已还，2026-08-29**）
- **发现**: 2026-08-29 · 父 crew Todo #44（Fly 远程主机接入复核，`docs/internal/2026-08-29-fly-remote-host-review.md` §2 A-2/A-3）
- **已还**: server 每条连接与 client 各持一个 `SessionFrameDecoder`，endpoint 面向
  `SessionMessageLink` 的可靠有序字节流语义；新增 transport 替身把第一帧切成半包、再与第二帧
  粘在同一次投递里。旧代码实跑 6 tests / 4 failures，修后专项 32 / 0；合最新 main 后全量
  1766 tests / 3 个登记 skip / 0 failures，iOS Simulator build 通过。
- **位置**: `Sources/Mac/LocalRunner/RemoteSessionBackend.swift:567-568`（server 侧 `receive`）、
  `:855-859`（client 侧 `receive`）、`Sources/Mac/LocalRunner/SessionProtocol.swift:469` / `:486` →
  `:503-509` `exactlyOneFrame(_:)`。
- **问题**: 两个 endpoint 收到 `Data` 后都直接 `codec.decodeApp/decodeDaemon`，而它们经 `exactlyOneFrame`
  要求这一次投递**恰好解出一帧**；不满足就 `throw`，调用点是 `guard let … = try? … else { return }`。
  于是**半帧到达 → 丢；两帧粘在一起 → 两帧都丢**。不断连、不报错、不落日志。
  正确的增量缓冲 `SessionFrameDecoder`（带 `buffer`、能处理半帧，`SessionProtocol.swift:81-107`）已经写好了，
  **只是没接到 endpoint 的收包路径上**。
- **为什么今天照不出来**: `InProcessTransport.sendFromApp/sendFromDaemon` 把整个 `Data` 原样交给对端回调
  （`InProcessTransport.swift:22-30`），投递边界恒等于帧边界。**现有测试全部跑在这条传输上，所以这条永远是绿的。**
  UDS 上偶尔踩；WAN + TLS（Fly 远程主机、手机遥控）上必然拆包粘包。
- **连带一条**: 两个 endpoint 的 init 签名是 `init(transport: InProcessTransport, …)`（`:479` / `:763`），
  不是同文件里已经定义好的 `SessionTransport` 协议（`InProcessTransport.swift:6`）—— 换传输必须改这两个 init。
- **该怎么还**: 归 P4（真进程分家）的验收，**不要平行改**（那是设计 §6 警告的双头）。两步：
  ① 每条连接各持一个 `SessionFrameDecoder`，`receive` 改成「喂字节 → 拿 0..n 帧 → 逐帧处理」；
  ② endpoint 面向 `SessionTransport` 而非具体类。
  验收要求**先证明尺子会红**：写一个按任意字节边界切分/合并投递的传输替身，跑当前代码必须红，接上增量解码后转绿。
- **已还（2026-09-04 核实，随 P4 合入 main `a8f4597`）**：两个 endpoint 各自持一个
  `SessionFrameDecoder`（server 侧在 `Connection` 上，`SessionProtocolEndpoints.swift:21`；
  client 侧 `:541`，重连时 `:593` 重置），收包路径改成「喂字节 → 拿 0..n 帧 → 逐帧处理」。
  传输面向 `SessionMessageLink`（`UnixSocketTransport.swift:15`）而非具体类，
  今天有四个实现：`InProcessSessionLink` / `UnixSocketTransport` / 测试替身
  `ByteStreamLink`（**按任意字节边界切分与粘包**）/ `BackpressureLink`。
  尺子也照要求写了：`SessionProtocolOverSocketTests.test_server接受任意切分与粘包的可靠字节流`
  与 `test_client接受任意切分与粘包的可靠字节流`。
  **没验到的**：真 WAN + TLS 上没跑过（异机传输是 P6，见
  `docs/internal/2026-09-04-cross-machine-transport-scope.md`，尚未实现）。

### 🟡 唤醒投递闸和它的重试路共读同一个瞬时布尔 —— 只改一边会打架

- **发现**: 2026-09-05 · 修「压缩上下文被误判卡死」（`63f39ac`）时顺手看到，**没修**
- **现状**: `CrewSessionRunner.deliverOrDeferWake` 用 `run.activityIsWorking`
  （「最近 1s 内有 PTY 输出」）判目标闲不闲、决定立刻投还是排队；
  `scheduleDeferredWakeRetry` 0.5s 后**再读同一个布尔**决定要不要补投。
  claude 压缩上下文/长思考时这个布尔读到 false，于是照投不误。
- **为什么这一笔没顺手改**: 注入进一个忙碌的 claude 是安全的（TUI 自带输入排队，
  2026-09-05 人类那条消息就是这么送到的），所以它不是**故障**，只是不精确。而闸
  和重试路读的是同一个信号，只给闸换上更准的判据（`63f39ac` 新加的忙碌指示）、
  重试路还读老布尔，两者就会打架：闸判「忙，排队」，0.5s 后重试路判「闲，投」——
  排队等于白排。要改就得两处一起换，那是另一笔。
- **代价**: 目前只有「投得早了一点」，没有可观测的坏结果。真要动的时候记得：
  **先看这两处是不是还共读一个信号**，别只改看得见的那一处。

### 🔴 所有在跑 session 的 PTY 输出都要过主线程 —— 界面代价随派活数线性增长
- **发现**: 2026-08-19 · `fix/ui-jank-pty-scan`（Todo #59 界面卡顿排查）
- **位置**: `Sources/Mac/LocalRunner/AgentTerminalSession.swift` 的 `dataReceived` 回调（`ActivityTerminalView.dataReceived(slice:)` → `MainActor.assumeIsolated { … }`）。
- **问题**: session 本体是 `LocalProcessTerminalView`（SwiftTerm 的 `NSView` 子类），PTY 每一批输出都在**主线程**交付，**不管这个 session 在不在前台、用户有没有在看它**。挂在这条回调上的旁路工作（打字指纹、健康扫描、菜单检测、启动参数回显、切档回显）因此全部是主线程同步工作，且**按在跑的 session 数线性叠加**。
- **证据**: `docs/internal/2026-08-19-ui-jank-profile.md`。sample 实测：单个忙碌 session 的 `dataReceived` 子树占主线程 **7.9%**；机长常态派 3～5 个 → 24%～40%。
- **本次做了什么**: 把这条回调上**每一段**旁路工作的单价打下去了（squeeze O(n²)→O(n)、健康短语改字节匹配、打字指纹单趟折叠），合计把单 session 的稳态占比压掉一个数量级以上，并消掉了「每起一个 session 主线程被占死 45 秒」那条。
- **没做也不该在这条分支上做的**: **结构本身没变** —— 代价仍然随 session 数线性涨，仍然全在主线程，仍然与「用户在看哪个终端」无关。真正的解法是把 session 搬进常驻后台进程（机长 2026-08-19 已在单独排期，与「前后端分离」同一刀），app 退化成看的那个窗口。**这条不要顺手在功能分支里动。**
- **修后复采（2026-08-19）**: 结构没变，但单价打下去之后实测 **10 个 session（6 个在忙）合计只占主线程 1.51%**（≈0.25%/忙碌 session）。
  也就是说这条 🔴 现在**不再是当前的卡顿来源**，它仍然是 🔴 是因为「代价随 session 数线性涨、且与用户在看哪个终端无关」这条结构事实没变 ——
  派到几十个 session 时它会重新变成主导项。见 `docs/internal/2026-08-19-ui-jank-profile.md`「现场复采」。
- **中间态的可选缓解**（如果地基活拖久了）: 后台 session 的旁路扫描改成攒批 / 挪出主线程；或把 scrollback 与扫描窗按「是否前台」分档。都属于治标，记在这里免得被当成已解决。

### ✅ 远端 / 登录整层的「安静死代码」（**随 #63 第二期删除，2026-08-26**）

> 跨端遥控，端掉。以后前后端解耦时重新做

上面这句是人类原话，一个字没改。**这不是清理垃圾，是有意移除、将来在新架构下
重建**——写在这里是为了将来有人翻到这段历史时，不会以为这块是被谁偷偷删掉的。

- **原问题（2026-08-25 · #63 第一期落地时记）**: 第一期删掉了**取得凭据的所有入口**
  （Auth 三件套 / Supabase 栈 / 两个登录页 / 扫码页 / 侧栏登录入口 / RootView 的登录
  分支），凭据层本身却删不动（被 `Sources/Remote/`、`CrewRelayAgent` 那批文件顶着）。
  于是 `AppModel.isAuthenticated` **恒 false**、`loggedAPIClient()` 恒抛、`imageAuth`
  恒 nil，所有调用点都是 `guard … else { return }` / `try?` 早退 —— 编译器不报、测试
  不红、看代码也看不出来。
- **第二期做了什么**: 从调用点往被调用方走，分九笔删干净：「服务端 session」面板 →
  crew 详情页的「接入 PendingBot」节 → serverLink 那条链（`Sources/Remote/` 三个文件
  + `SessionPermissionRelay` + `CrewMailboxWaker`）→ `CrewRelayAgent` 与两个纯逻辑
  → 本地两个 store 上的 relay 残留 → edge 交互卡 → `attachmentIds` 通道 →
  `EdgeBackend` + `CrewRealtimeClient` → `AppModel` 凭据层 + `PendingCrewAPI` +
  Keychain / 家族凭据。**没有用 `#if false` / 注释掉 / 空 stub**，全仓 `#if false` 零命中。

- **名单的修正**（比删除本身更值钱的那部分，逐条列）:
  - **`CrewRelayAgent` 不是「部分死」，是整个死。** 上面那张表把它记成「被
    `LocalWhiteboardStore` / `LocalCrewStore` / `SessionHost` / `CrewLocalTodoLanding` /
    `CrewRelaySyncLogic` 五个本地活路径引用着」——实测**只有 `SessionHost` 那一处是真
    代码引用**（`let relay: CrewRelayAgent`），其余四处全是注释里提到它的名字。
  - **`CrewMailboxWakeLogic` 不该跟着 waker 走。** 它一半是 edge mailbox 决策
    （`decide` / `renderInjection`，跟着删），另一半是**唤醒投递回执**
    （`receiptVerdict` / `wakeFailureAlert`）—— 跟 edge 无关，本地 @ 直投在用，留下。
  - **`senderDisplayName` 有两个，同名不同物。** 存储层
    `LocalWhiteboardMessage.senderDisplayName` 唯一写入方是 `appendRelayMessage`，
    死了；线上模型 `CrewWhiteboardEntry.senderDisplayName` **是活的** ——
    `LocalBackend` 用本地 `senderName` 主动合成它，中栏靠它把本地 session 的消息显示
    成「机长」而不是兜底「会话」，`CrewChatAdapterTests` 第 6 例专门钉着。只删了前者。
  - **`relayRemoteId` 不是孤儿。** 它有活读者 `CrewLocalMentionWakeLogic:62`
    （#554 断链修复的规则 3）。删是删了，但那是**一条真修过的 bug 的防线**，见下面
    单独一条。
  - **`CrewChatView.swift` 的 `loggedAPIClient()` 是 3 处不是 2 处**（表上写 2）。
  - **表上完全没列、但引用了这批符号的**：`CrewModels.swift`、`ModelCatalogEntry.swift`、
    `LocalRunnerPlaceholder.swift`、`CrewRelayHubLogic.swift`、`CrewMailboxWakeLogic.swift`、
    `CrewRelaySyncLogic.swift`、`LocalDataReset.swift`、`CrewSettingsView.swift`、
    `CrewRealtimeClientTests.swift`、`CrewSummary.swift` / `CrewRootLineage.swift` /
    `CrewListView.swift`（EdgeBackend 注释）、`LocalCrewStore` 上的四个 relay 持久化字段。
  - **交互卡整套在本地路径上一次都没渲染过**：`LocalBackend.listCrewWhiteboard` 把
    `payload.kind` 写死 nil，而 `isInteraction` 就是 `payload?.kind == "interaction"`。
    （**注意本仓有两套「待审批」**：`LocalApprovalStore` 那套是本地权限审批，活的，
    此刻磁盘上就有真实数据，一个字没碰。）

### 🟡 #554「远端人类的 @ 也唤醒」那条防线随 relay 一起没了 —— 重建时要一起重建
- **发现**: 2026-08-26 · Todo #63 第二期
- **原来是什么**: `CrewLocalMentionWakeLogic.pending` 的**规则 3**：`relayRemoteId != nil`
  的 **user** 条目（远端人类经 relay 落进本地白板的 @）也收。理由是 composer 直投只
  覆盖本机人类，远端 iOS 用户 `@session` 落到 Mac 白板后**没有任何投递者**把它转成注入，
  session 就此断链收不到 —— 那是 #554 真修过的一个 bug。
- **现在为什么没了**: 判据 `relayRemoteId` 随 relay 整层删除，条件恒 false。规则、它的
  两个用例、以及 `LocalWhiteboardMessage.relayRemoteId` 字段一起去掉。
- **该怎么还**: **前后端解耦重建 relay 那一刀，必须把这条一起重建。** 判据换成新架构里
  「这条是从远端搬进来的」的等价标记；不重建的话，远端人类的 @ 会重新变成断链，而且
  症状和 #554 当年一模一样（没有任何报错，就是收不到）。

### 🟢 `Sources/Models/ModelCatalogEntry.swift` + `PendingCrewBackend.listModels()` 已无消费者 —— 但**不是 #63 造成的**
- **发现**: 2026-08-26 · Todo #63 第二期（零残留自查时撞见）
- **问题**: `listModels()` 全仓没有任何调用方；`ModelCatalogEntry` 只被 `listModels()`
  的签名引用。新建 session 页早就改读本机实探的 `ModelCatalogCenter` /
  `AgentModelCatalog`（models.json，形状不同）。
- **为什么这一期没动**: 它**在 #63 之前就已经是死的**，不是这一刀造成的孤儿，也不属于
  遥控 / 登录层。按「清单外的不自己扩」留着，只在类型注释里写清楚。
- **该怎么还**: 确认 `ModelCatalogCenter` 那条路是唯一供数方之后，把 protocol 上的
  `listModels()`、`LocalBackend` 的空实现、以及 `ModelCatalogEntry.swift` 一起删。

### 🟢 两个名字在这一刀之后名不副实 —— `CrewMailboxWakeLogic` / `CrewRemoteImage`
- **发现**: 2026-08-26 · Todo #63 第二期
- **问题**: `CrewMailboxWakeLogic` 现在只剩「唤醒投递回执」那半（edge mailbox 决策已删），
  名字里的 mailbox 不再指任何东西；`CrewRemoteImage` 现在只从 `file://` 读本地图，
  Remote 也不再指任何东西。
- **为什么没改名**: 两个都是别处的构造点（前者被 `CrewSessionRunner` 调、后者被
  `ServerImage` / `BubbleView` 构造），改名会把这一刀的 diff 摊进不相干的文件。
  两处都在类型注释里写了「名字是历史」。
- **该怎么还**: 顺手改名的时候一起改（`CrewMailboxWakeLogic` → 回执判定；
  `CrewRemoteImage` → 本地附件图），不值得单开一笔。

### 🟢 `CrewSummary.rootCrewTitles` 与 `CrewRootLineage` 的服务端回退分支现在恒空
- **发现**: 2026-08-26 · Todo #63 第二期
- **问题**: `rootCrewTitles` 原本是服务端算好下发的根 crew 血缘，给看不到本地 DAG 的
  iPad/iPhone 用。云端整层删掉后它恒空，`CrewRootLineage.rootTitles` 里「本地算不出
  才用服务端这份」的回退分支因此不再会被走到。
- **为什么留着**: 判定本身是对的，重建前后端时第二个来源会重新出现在这个位置；
  而且删字段要动 `CrewSummary` 的 Codable 与它的一批测试，收益为零。已在注释里写明。


### ✅ 登录态 session 的信箱唤醒与审批中继从未接通（**随 #63 第二期删除，2026-08-26**）
- **发现**: 2026-08-19 · 前后端分离 P0（所有权归拢）
- **位置**: `Sources/Mac/Views/CrewSessionWindowView.swift` 手动起 session 那条路里的 `let serverLink: CrewSessionServerLink? = nil`；实现在 `Sources/Mac/Services/CrewSessionRunner.swift` 的 `ensureMailboxWaker` / `ensurePermissionRelay`。
- **问题**: 这两个服务原本由视图在 run 起好后接线，两个调用点**都在死路上** —— 一条被上面那个写死的 `nil` 挡着，另一条在被 `edgeQueueBindingReady == false` 关着的 auto-claim 死循环里。也就是说它们**一次都没被调用过**。调研清单（`docs/internal/2026-08-19-backend-split-inventory.md` A19/A20）当时的判断是「右栏没打开过的 session 才没接」，实际比这更糟：**所有 session 都没接**。
- **症状**: 登录态下 edge 信箱的定向投递不会唤醒本机 session；远端 viewer 的审批镜像（#204 permission over WS）不生效 —— 都是静默不工作，没有任何报错。
- **根因**: edge session 通道（接合 v2 block 3，本地 crew ↔ edge 行的绑定）没开，所以 `serverLink` 一直是 nil。不是这两个服务本身有问题。
- **P0 做了什么**: 只删掉视图侧那段永不执行的接线（连同 auto-claim 死循环），**实现原样留在 runner 上并加了注释说明当前无调用点**。P0 的约束是行为零变化，真接上属于行为变化，不在本阶段做。
- **解法归属（历史）**: P4（编排整体搬进 daemon，届时由 runner 侧统一接线，别再从视图接）或云端那条轴（先把 edge session 通道打开）。
- **为什么结掉（2026-08-26 · Todo #63 第二期）**: 这两个服务本身删掉了 —— `CrewMailboxWaker` / `SessionPermissionRelay` / `SessionProxyClient` / `CrewSessionServerLink` 连同 `ensureMailboxWaker` / `ensurePermissionRelay` 两个无调用点的入口一起走。人类原话「跨端遥控，端掉。以后前后端解耦时重新做」——**这条不是修好了，是连同它描述的东西一起没了**。
- **重建时要注意的**: 当年的病根不是这两个服务有问题，是**接线接在视图上**（`CrewSessionWindowView` 起 run 时接）。重建时按 P0 的结论从 runner / 常驻编排侧接，别再从视图接。

### 🟡 点名唤醒器把「读增量」当成廉价操作 —— 每个目录 tick 全量重解白板
- **发现**: 2026-08-19 · `fix/ui-jank-pty-scan`（同上）
- **位置**: `Sources/Mac/Services/CrewLocalMentionWaker.swift` 的 `directoryChanged` 扇出；同款注释「读增量靠游标，很廉价；与 listen 路同款策略」。
- **问题**: 目录事件不带文件名，所以一个 tick 要把 `watched` 里每个 crew 都扫一遍；而「扫」= 取文件锁 + 整份读 + 整份 JSON 解码，**游标只裁剪解完之后的行，读和解一分钱不省**。本机白板目录 67 个 json / 3.8 MB，全量走一遍实测 **9～11 ms**，全在主线程；helper 子进程每发一条 `post_to_crew` 就是一个 tick。
- **本次做了什么**: 给唤醒器补上 `FileChangeGate` 文件指纹门（那正是 #443 建它时写明的用途）：9～11 ms → 0.07～0.10 ms。
- **留着的尾巴**: 注释里点名的「listen 路同款策略」**没查**，很可能同病；另外 `LocalWhiteboardStore.list` 本身仍是「每次调用整份读+整份解」，没有按文件指纹缓存解码结果 —— 指纹门只是让**不必要的调用**不发生，真正需要读的那次仍然是全量。白板越长这一下越贵（本机最大的一份已经 860 KB）。

### ✅ 卡顿修复的现场复采（**已还，2026-08-19**）
- **原问题**: 四条修改的「修前/修后」数字只到函数级（`swiftc -O` 实测 + 耗时红线单测），没有对线上进程 `sample` 复采过 —— 因为复采要先装一次新版 app。
- **已完成**: 新版（0.1.13 / 20684.16770，`BuildStampCommit` = `a4f8f5d` = 当时的 main HEAD）装好后，对真进程做了四份符号化采样，两个场景都覆盖到了。结果贴在 `docs/internal/2026-08-19-ui-jank-profile.md` 的「现场复采」一节。
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


### ✅ 仓库默认签名改成 ad-hoc 会静默丢登录态（**随 #63 删除，2026-08-25**）
- **发现**: 2026-08-20 · 开源准备（签名解耦）
- **位置**: `Config/Signing.xcconfig`（仓库默认值）、`Config/Local.xcconfig.example`、`project.yml` 的 `configFiles`。
- **为什么这么改**: 原来 `DEVELOPMENT_TEAM: M42BKJN82S` 硬编码在 `project.yml` 里，外部贡献者 clone 下来签不了名、编不过 —— 开源的第一道硬门槛。改成默认 ad-hoc 之后任何人都能编能跑。
- **代价转嫁到哪**: `KeychainStore`（云端 crew 的 device-grant token）的 ACL 绑当前签名身份，ad-hoc 每次重建身份就变 → 反复弹「存取钥匙串」授权框或 `-34018` 存不住 → **登录态静默丢失**。这个坑 2026-06 已经踩过一次并用「稳定的 Apple Development 身份」根治过，现在把根治手段挪到了一个 **gitignored 的文件**里。
- **谁受影响**: 只有要动云端登录/钥匙串那条路径的人。只跑本机 crew（起 claude / codex 子进程的主路径）完全不受影响 —— 那条路径不碰钥匙串。
- **失败长什么样**: 不报错。app 编得出、装得上、跑得动，只是登录态存不住。所以**症状和病因隔着十万八千里**，别再从后端/Supabase 那头查。
- **该怎么还**: 构建期没有可靠判据区分「贡献者本来就该 ad-hoc」和「本机开发者忘了装覆盖」，所以没加编译告警（那会给每个贡献者的每次构建都挂一条黄色噪音，反而训练人无视告警）。真要还，正确的地方是**运行时**：走云端登录路径时若检测到 ad-hoc 签名（`csops` / `SecCodeCopySigningInformation` 读不到 team identifier），直接在界面上说清「这个构建签名不稳定，登录态存不住」，而不是让它静默失败。
- **为什么结掉（2026-08-25 · Todo #63）**: PendingCrew 不再登录到任何地方，登录入口整块删了 —— **没有任何路径会再往钥匙串写登录态**，这条债咬不到人了。`KeychainStore` / `FamilyCredentialStore` 当时还在（跟凭据层一起等第二期），**2026-08-26 第二期已连文件一起删掉** —— 全仓再无任何 Keychain 调用。签名默认值本身（ad-hoc）不变，也不需要改。`Resources/PendingCrew.entitlements` 里那两个 keychain 组留着没删（动它可能影响本机签名，收益为零），注释里已如实写明「当前没有消费者」。
- **什么情况下要复活这条**: 哪天 PendingCrew 又要存跨启动的凭据，这条原样有效，别重新踩一遍。
- **发版不受影响**: `scripts/release/build-macos-update.sh` 在 xcodebuild 命令行上显式传 `CODE_SIGN_STYLE=Automatic DEVELOPMENT_TEAM=…`，命令行优先级最高，Developer ID 分发路径与这里的默认值无关。

### ✅ 没有 CI（**已还，2026-08-21**）
- **发现**: 2026-08-20 · 技术栈梳理（只读盘点）
- **位置**: `.github/` 下只有 `ISSUE_TEMPLATE/` 与 `pull_request_template.md`，**没有 `workflows/`**；仓库根也没有 Makefile / justfile / pre-commit。
- **问题**: `CONTRIBUTING.md`「六条硬规矩」的第 1、2 条把三件事定成硬规矩 ——「改了 `project.yml`（含新增 Swift 文件）必须 `xcodegen` 并提交 `.pbxproj`」「三端都要编一遍」「跑测试」—— 但没有任何自动化在 PR 上核这三条。其中第一条**已经踩过并且症状是「只有别人的机器编不过」**（`CONTRIBUTING.md` 第 1 条自己写着「**在别的机器上编不过**。这条真的踩过。」）：提交者本机 Xcode 会自动发现新文件，所以他永远看不到红。
- **为什么现在要记**: 之前仓库只有作者一个人、一台机器，靠纪律够用。开源之后进来的每个 PR 都是「另一台机器」，而这正是这条规矩失效时唯一会暴露的场景。
- **代价转嫁到哪**: 维护者的人工 review。pbxproj 漂移与 iOS 端静默打红这两类问题都不会在 PR 页面上显形，只能靠维护者自己 checkout 下来跑三条命令。
- **该怎么还**: 一个 macOS runner 上的 workflow，三步即可覆盖：`xcodegen && git diff --exit-code PendingCrew.xcodeproj/project.pbxproj`（抓漏 regen）、macOS build + test、iOS Simulator build。测试跑满约 3 分钟（2026-08-20 本机实测 184s / 1443 tests）。**注意**：`CrewChatOpenCostTests` 在 CI 上会 skip（fixture 不入 git），这是预期的，别为了让它绿而把 fixture 提交进去。
- **已完成（`.github/workflows/ci.yml`）**: 两个 job —— ①「pbxproj 与 `project.yml` 同步」重跑 xcodegen 后比 diff；② 三端编译 + 单测。落地过程中还顺带查出并根治了一个真问题：**仓库根目录的 xcconfig 会让 `xcodegen` 输出不确定**（`project.pbxproj` 里那条文件引用的 uuid 每次 regen 都变），那道 diff 检查因此永远红 —— 修法是把 xcconfig 挪进 `Config/`（`4dc855a`）。
- **一处措辞更正**: 上面「测试跑满约 3 分钟」是**本机热构建的测试执行时间**，不是 CI 的墙钟。CI 是冷机，要连 SPM 解析和两轮全量编译一起算。别拿本机数字当 CI 预算。

### 🟢 `docs/architecture.md` 还差这几处 —— 逐处名单，别重新考古
- **发现**: 2026-08-25 · Todo #63 第一期；2026-08-26 第二期扩大一次并**改掉了其中的事实性错误**
- **已经改掉的（2026-08-26，一笔独立提交）**: 34–37 行的正文口径、目录表里两条已不存在的
  目录（`Sources/Auth/` / `Sources/Remote/`）、`Sources/Services/` 的职责描述、
  「11 个非 macOS 专有文件」里那条已删的 `CrewInteractionCard.swift`、vendored 一节里的
  `Sources/Auth/` 与 `AttachmentDownload`、测试地图里的 `CrewHostedConfigTests` 行、
  8.3 的 skip 表（11 → 10）、长期服务列表里的 `CrewRelayAgent`、P0 那段
  `CrewMailboxWaker` / `SessionPermissionRelay` 的时态、速查表里的 `CrewHostedConfig` 行。
- **故意没动、要留着的**: **第 2 节的依赖表与第 3 节整节**（约 164–290 行）。那不是「过时的
  描述」，是**删除决策的证据** —— 删了或按新数字重写，将来的人只看得到「这里曾经有过依赖」，
  看不到「为什么删」。两节节首各加了一条带日期的横幅说明数字是 2026-08-25 之前的实况。
  第 269 行那句「判据是 `CrewHostedConfig` 的四个占位常量」落在这个范围里，同理不动 ——
  单独挖掉它会把证据链弄断。
- **还差什么（逐条，下一个人照着做即可）**:
  1. **重测规模数字**：第 39–41 行的「249 个 Swift 文件 / 48,038 行 / 1,443 个 `func test`」
     是 2026-08-20 实测，#63 两期删掉约 20 个文件之后没重跑。已在原地标了日期，但没改数。
  2. **重测两张目录表**：第 4 节的「文件数 / 含 `#if os(macOS)` 的文件数」与 5.1 的行数表，
     同样是 2026-08-20 的数，只删了已不存在的目录行。同样已标日期。
  3. **写一节「现在的架构长什么样」**：第 2、3 节讲的是删除前的成本结构，删除后**没有任何
     一节讲现在的依赖构成**（5 个直接依赖、还剩几个 pin、体积多少）。这是唯一需要**新写**
     的一块，所以第二期没动 —— 边界是「只改事实、不动作者行文」。
  4. **README「状态」一节**：第 346 行原来指着它说 `Sources/Remote/` 未接通，那条已删；
     README 本身在作者手里有未提交改动，两期都没碰，落地后要看一眼口径还对不对。

### 🟢 `Sources/Mac/` 名不副实，而且没有任何编译期的「层」
- **发现**: 2026-08-20 · 技术栈梳理（只读盘点）
- **位置**: `project.yml` 的 `PendingCrew` target 只有一条 `- path: Sources`；`Sources/Mac/` 下 110 个 swift 文件里 **11 个不含 `#if os(macOS)`**。
- **问题（两条，互相放大）**:
  1. **一个 target = 一个 module**，`Sources/` 下的目录只是目录，没有 `import` 边界。所谓「分层」全靠约定，编译器一条都不管。
  2. **`Sources/Mac/` 里混着跨平台文件**，于是别的目录必须反向引用它才能拿到那些类型：
     - `Sources/Mcp/McpServer.swift:640` 用 `AgentQuotaFile`、`:1043-1083` 用 `AgentModelCatalog` 一族（都在 `Sources/Mac/LocalRunner/`）
     - `Sources/Support/QuotaRingLayout.swift:62,84` 用 `AgentQuotaSnapshot` / `AgentQuotaWindow`（同上）
     - `Sources/Chat/Adapter/CrewComposerMentions.swift:358` 用 `CrewSenderResolver`（在 `Sources/Mac/Views/Chat/`）
     - `Sources/Views/IPadShell.swift:47` 直接构造 `CrewChatView`（`Sources/Mac/Views/CrewChatView.swift`，1437 行，**两端都编**，它就是 iPad/iPhone 的群聊页）
- **为什么不是 🟡**: 那几个文件的头注释都明写了「纯 Foundation、不带平台门 —— McpServer（跨平台编译）要用」，是**有意为之、只是放错了目录**，不是把 macOS 代码偷渡进跨平台路径。所以它腐蚀的是可读性，不是正确性。
- **失败长什么样**: 新人（含三个月后的作者）按目录名判断「这是 macOS 专有的、我随手 import 个 AppKit」→ iOS 端静默打红，而且只在别人跑 iOS 构建时才发现。
- **该怎么还**: 把那 11 个文件挪到 `Sources/Support/` 或新建的 `Sources/Agent/`；`CrewChatView` 归到 `Sources/Chat/` 或 `Sources/Views/`。纯搬家、无行为变化，但会动一批 import-free 的引用点和 `project.yml` 的测试文件清单，属于**大改一批文件**的动作，不要顺手夹在功能分支里。真正的分层保证（拆 target / SPM local package）代价大得多，不在这条的范围内。

### 🟢 `whiteboards/` 目录只增不减 —— per-session 文件从不回收
- **发现**: 2026-08-20 · 技术栈梳理（只读盘点，复核了 2026-08-18 那份调查）
- **位置**: `~/Library/Application Support/PendingCrew/whiteboards/`；产生方 `Sources/Mcp/WhiteboardCursor.swift`（`.cursor` / `.cursor.lock`）与 `Sources/Mcp/SessionTurnTrace.swift`（`.turn`）。
- **问题**: 每起一个 session 就多三个小文件，**session 退出后没有任何清理**。已有一份逐项盘点与清理方案：`docs/internal/2026-08-18-whiteboards-directory-cleanup-plan.md`（当时 1051 个文件，其中约 341 个属于早已不存在的成员）—— 但那份文档写明「本轮只调查、只出方案，一个文件都没删、没移、没改」，所以**账上一直没有一条活的登记**。
- **证据（本次实测）**: 同一目录今天 **1346 个文件 / 36 个 crew**。两天涨了约 295 个。
- **为什么值钱的不是磁盘**: 主线程仍在列这个目录 —— 见上面那条 🟢「`drainRenames` 每 tick 全量列目录」（`getattrlistbulk` 的开销直接乘以文件数）。文件数是那条的乘数。
- **没做**: 本次是只读梳理，一个文件都没动。真要做的话方案已经在上面那份 internal 文档里写好了，包括「`.corrupt-*` 是 8-12 事故的现场证据、该先移进归档目录由人拍板再删」这条纪律。

### 🟢 驾驶舱有一半的数据契约只存在于另一个仓库
- **发现**: 2026-08-20 · 技术栈梳理（只读盘点）
- **位置**: `Sources/Models/CockpitModel.swift:362-383` 的 `CockpitLoader.load`；空态文案在 `Sources/Mac/Views/CockpitView.swift:112`。
- **问题**: 驾驶舱从 **crew 的工作目录**读四样东西：`docs/roadmap.md`、`docs/handbook/`、`docs/state/`、`docs/tasks/`。其中 `docs/roadmap.md` 缺失时有一份**自带格式模板的空态引导**（`CockpitRoadmapView.swift:369-393`），照着建就能用；但 `docs/handbook/` 与 `docs/state/` 的格式**这个仓库里没有任何地方写过**，而 `CockpitView.swift:112` 的空态直接告诉用户「让某个 crew 的工作目录指向带这些账的仓库（比如大绿豆自己）」—— 那是另一个**未开源**的仓库，外部贡献者拿不到，也无从照着造一份。
- **牵连**: `README.md:69` 把「驾驶舱」列在「真跑过、天天在用的」里，没有任何限定语。对一个把工作目录指向自己 clone 的人来说，驾驶舱的任务段能用（人类 Todo + `~/.claude/tasks` 都在 app 数据目录），路线段照引导建一份 `docs/roadmap.md` 也能用，**但期望/现状那两栏永远是空的，而他不知道为什么**。
- **该怎么还**（三选一，都不大）: ① 给 `docs/handbook/` 与 `docs/state/` 也补上同款自带模板的空态引导；② 在 README 的能力清单里给「驾驶舱」加半句限定；③ 把这两本账的格式写进 `docs/`。**别改代码去删功能** —— 它对作者本人是天天在用的。

### 🟢 「加载更早」那一帧：查清了，也修了 —— 但「未走的路」是走不通的那条

- **发现**: 2026-08-23 · 人类 Todo #60；**2026-08-26 返工并了结**（人类验收没过）
- **位置**: `Sources/Mac/Views/CrewChatView.swift` 的 `expandEarlier` / `topAnchorBox` /
  `ChatScrollAnchor`；探针在 `Tests/PendingCrewTests/CrewChatWindowTests.swift` 的
  `CrewChatExpandAnchorProbeTests`。

#### 当初那条警告，兑现了

原文一字不改留在这儿：

> **第 4 条要是真出了问题，正确的方向是走这条未走的路（先消灭翻面 + anchor），不是给
> `scrollTo` 加补丁。别被「anchor 试过了不行」误导 —— 它没被试过。**

**这句在 2026-08-26 兑现了：它被试过了。** 留着它不是为了记账 —— 一条警告完成它使命的
方式就是变成一条结论，删掉它等于告诉后来者「写了也会被覆盖掉」。

两处要接着往下写，因为**兑现的方式和当初预想的不一样**：

1. **它的触发条件从来没被满足。** 那句「第 4 条」指的是清单里「点完静置两秒，看懒行真实
   高度回填之后锚点漂没漂」。人类照着念了，回答是「**位置倒是一样**」—— 第 4 条过了，
   锚点没漂。真正出问题的是清单**没问**的那件事。
2. **它推荐的那条路走不通**，见下。所以它对方向的判断（「别给 `scrollTo` 加补丁」）是对的，
   对手段的判断是错的。

#### 真凶：那记补偿结构上必然晚一帧

原探针量首尾（不补偿跳 680pt、补偿后 14pt / 1pt），**从来没量过中间经过哪里**。补上路径
探针（判据是 CoreAnimation 的提交边界，一次提交 ≈ 一帧），起点 694：

| 点击那一下 | 第一帧 | 第二帧 |
| --- | --- | --- |
| 什么都不做（对照） | 1374 | 1374 |
| 上一版 `scrollTo` + 主线程 hop | **1374** | 680 |
| 现在（`.scrollPosition(id:anchor:.top)`） | **698** | 698 |

**两趟的第一帧是同一个数** —— 那记 `scrollTo` 一帧都没提前。而且**没有动画**
（offset 56→740 一步到位），所以人类那句「滑」不是动画，是 680pt 出现一帧又弹回去。
这不是补偿量不够，是结构性的：`renderLimit` 写下去那一刻新的一页还没进视图树，
同一拍 `scrollTo` 抓不到目标 ⇒ hop 必需 ⇒ 两者必然落在两次提交里。

#### 「未走的路」：走过了，走不通

那条路写的是「内容在上面长、锚底部 = 视口一像素不动，原生机制」。**实测不成立。**
`.bottom` 的语义是「把视口钉在内容底部」，不是「保持与底部的相对距离」—— 它只在视口
**已经贴底**时做事，而「加载更早」的现场按定义就是人已经滑上去了
（`anchorOnExpand` 在跟随时返回 nil），正是它不响的那个现场。人滑上去之后 `.bottom` 与
`.top` 读数**逐字相同**（都是 1382）。就算它响了，它会把人一把拽到底部，比原来还糟。

这不是「尺子没量到锚」：同一把尺子，视口**贴底**时两个锚值差 **707pt**。
用例：`test_未走的路_人滑上去之后bottom锚与top锚读数相同` + `test_锚标定_贴底时必须分得出bottom和top`。

#### 落地的那条：`.scrollPosition(id:anchor: .top)`

点击那一拍先把「顶上那条」的 id 写进绑定，再改上限，**两件事落在同一次 body 更新里**。
没有 hop、没有程序化 `scrollTo`，也就没有「被拽回来」这个动作 —— **不是把中间帧修小，
是让它没有理由存在。**

两条容易被下一个人误删的细节，都量过：

- **那一笔写入是承重的，不是防御性的。** 只挂绑定不写 id → 偏 **680pt**（等于没修）；
  多静置一拍 → 仍然 680pt；写了 id → **24pt**。用例 `test_诊断_按住位置靠的是哪一步`。
- **绑定落进一个不被观察的引用盒子，不是 `@State`。** 回写 30 次：落进被观察的存储 →
  body 求值 **30 次**；落进盒子 → **0 次**。它照样生效，因为紧接着 `renderLimit` 那一改会
  引发更新，SwiftUI 在那次更新里读 getter，正好读到刚写的值。

#### 还欠着的三件

1. **容器身份翻面还留着。** `usesEagerInitialLayout` 的 `limit <= pageSize` 判据仍会让
   12→24 那一下换容器、整棵树重建。**它不再是承重项**：翻面在场第一帧偏 −24pt，消灭之后
   偏 +4pt —— 从 680pt 降到 21pt 的差。消灭它要动首屏 eager 测量那条路（Todo #56 的地盘），
   是独立一笔。**代价**：第一次点击比后续每一次多 21pt 的位移。
2. **`.top` 那一臂是不是空转，未决。** 人已经滑上去、内容在下面长时，挂 `.top` 与
   **不挂任何锚**读数相同（都是 +673，差 0）—— 也就是**读数分不出它做没做事**，倾向空转。
   ⚠️ **不许据此写成「它是死代码」**：没做「删掉它再量」那一步，一次读数不成立不构成证伪
   （这条 tech-debt 自己刚在 anchor 上栽过同一个跟头）。用例
   `test_记录_松开跟随时top那一臂是否在做事` 只打印不断言。**别为它改代码。**
3. **人眼那一关还没过。** 探针证明的是「离屏下第一次提交画的已经是终点位置」；
   **「真窗口里人眼看不看得见」的唯一证据，始终是人类那句话。** 两者互相支撑，但不是同一个
   证据 —— 合并了就没有外部校准点了，而当初那套探针出问题的最后一块，正是它自己说自己是对的。

#### 尾巴：给人念的清单，别再问探针已经能答的东西

当初为这件事写过一份「人类装完更新后照着念」的四条清单，人类真念了、四条都答了，**但他报
回来的那个问题四条里一条都没问到** —— 四条问的全是「位置对不对」。**清单继承了探针的盲区。**
这条已经作为实例三写进 `CONTRIBUTING.md` 第 5 条，不在这儿重复。

所以这次不再列坐标清单，只留一句给人类：**点几次「加载更早」，看它是不是「本来就在那儿」，
而不是「移过去的」。** 位置对不对不用他核，探针能答。

#### 尾巴二：一个**从未被定位**的红，别当它已经被治好了

某次全量跑里见过 **1 个 failure**，把探针改成「等几何量连续 25 拍不变」之后就没再复现过
—— **但它具体是哪条用例、为什么红，从头到尾没有定位到**。

所以：**这一族将来再冒红，不许默认已经被 `788fca9` 治好。** 没抓住的红不算被治好，
只算没再出现。

### 🟡 机长作战板对人类只读 —— 是**刻意推迟**，不是没想到
- **发现**: 2026-08-25 · 人类 Todo #66 A 段（`CockpitPlanStore` / `plan_*` 三个 MCP 工具）
- **位置**: `Sources/Stores/CockpitPlanStore.swift`（唯一写入口在 `Sources/Mcp/McpServer.swift` 的 `guard isCaptain` 后面）。
- **现状**: 第六本账「机长作战板」**只有机长写得动**：人类在驾驶舱里能看，不能改、不能追问、不能翻状态。这是按规格做的 —— 这本账的定位就是「机长自己整理的」，与人类 Todo 那两本（人类写 / agent 写）方向不同。
- **为什么记在这**: 人看到一条写错的计划必然想动手，而**「不能改」和「不知道怎么改」是两回事**。UI 上必须明说怎么让它改（一句「让机长改」或者把这条带进群聊输入框的入口），否则这块板在人眼里就是死的 —— 这是 B 段（第三个药丸 + 面板）必须带上的一条，不是可选装饰。
- **推迟的是什么**: 双向编辑（人类直接改/追问机长的计划）。第一版不做，理由是别在方向都没跑顺之前就把它做成双向；不是没想到。
- **要动的时候怎么动**: 人类那一侧照 `LocalTodoStore` 的 `followUp` 语义走（追加式、不覆盖、任何状态都能追问），别新造第二套编排。

### 🟡 `CrewChatOpenCostTests` 里的**计时断言在飘** —— 一族，不是一条
- **发现**: 2026-08-26 · 侧栏「手动藏起来一个 crew」落地时取全量基线，撞上前提对不上
- **位置**: `Tests/PendingCrewTests/CrewChatOpenCostTests.swift` —— **已撞到三句，两种形状**：
  - `:493` `XCTAssertLessThan(cost, budgetMs)` — 绝对毫秒预算
  - `:342` `XCTAssertLessThan(costWindowNoSel, budgetMs)` — 绝对毫秒预算（同型）
  - `:346` `XCTAssertLessThanOrEqual(costWindowNoSel, costWindow)` — **相对比较，跟 100.0 无关**
- **机制（2026-08-26 改写过一次，别退回旧版）**: 不是「绝对毫秒预算对负载敏感」——**那个表述盖不住 `:346`**。真正共有的是「**在功能全量里做计时测量，而第一次触碰是冷的**」：
  - 绝对预算（`:493` / `:342`）：冷跑把数值整体抬高 → 越过常数；
  - 相对比较（`:346`）：冷启动**抬得不均匀** —— 同一个 test 里几个测量点谁先跑谁吃冷启动，于是 D 可能比 B 贵，**即使代码上 D 更省**。
  **写成「绝对预算」那版的后果是可预见的：下一个人按那条还法把预算调宽或门控掉，`:346` 照样飘。**
- **实测读数，每条写具体那一趟（commit + 毫秒 + 单跑/全量），不写「与基线一致」**:
  - `:493` — **2026-08-25** `main` @ `e17b268` / `ab0942c` 两趟全量：**红** `112.464084` / `111.977917`；**2026-08-26** `main` @ `3dade35` 两趟全量：**绿**；同日 `4e30434` 全量 **绿 0.103s**、`7c39f39` 全量 **绿 0.256s**。
  - `:342` — **2026-08-26** `b8bd679`（0.1.16 发版闸门，钉死 worktree）全量：**红 2.453s**；**同一二进制单跑：绿 0.422s**。同日 `7c39f39` 全量：**红 109.73**（邻居 `:493` 同趟 0.256s，2.5×）；单跑三次 **102.4 红 / 42.3 绿 / 39.6 绿**。
  - `:346` — **2026-08-26** `4e30434` 单跑三次：**86.2 红**（`86.165874 > 66.62446`）/ 56.1 绿 / 29.2 绿。
  - **冷 vs 热（实测，不是推的）**：六次单跑里**两次红都是各自 checkout+重编之后的第一趟**；同格「修之前」冷跑 598.1 / 309.6 ms，热跑远低于此。**全量的第一次触碰天然是冷的。**
- **判据（照这个走，别重新设计实验）**:
  1. **单跑绿 + 全量下红** ⇒ 属本族，不是回归；
  2. **配对载荷计**（成本为零，数已经在日志里）：每个全量样本**同时记目标 + 邻居 `:493` 两个毫秒数**。目标红而邻居也慢 ⇒ 那趟机器忙；**目标红而邻居正常（~0.1s）⇒ 往回归想**；
  3. **跨 commit 出现过**：`:342` 在 `b8bd679` 就红过，而它之后的 `c24eef8`/`f17ae13`/`7c39f39` 三笔与它无关 —— **一条在更早 commit 上的红，比任何 A/B 对照都强，因为它不需要对照组。**
- **问题**: 于是它**既不是稳定红也不是稳定绿**。危害不在这条用例本身，在它会**污染别人的对账**：2026-08-25 之后「这条在 main 上本来就红」被当成既有结论沿用，下一个人拿它当前提时前提已经不成立了；反过来，谁哪天撞见它红，也很容易以为是自己改出来的。
- **别把它跟 fixture 那两条混成一件事**（本仓库现有两处讲的都是 fixture，与本条无关）：`CONTRIBUTING.md`「有几个测试需要现取 fixture」那节的「`CrewChatOpenCostTests` 用的是真实群聊数据（不入版本历史，见 `.gitignore`）」、本文件「没有 CI」那条里的「`CrewChatOpenCostTests` 在 CI 上会 skip（fixture 不入 git），别为了让它绿而把 fixture 提交进去」。**那两条说的是「在 CI 上跑不了」，这一条说的是「在本机跑得了、但结果在飘」。**
- **另有一条同类（别合并成一条）**: 下面那条 `CrewLocalImageCacheTests` 也是「在飘」，但**机制不同、还法也不同** —— 那条是 `NSCache` 的可回收语义，这条是冷启动下的计时测量。读到任一条的人应该知道还有一条。
- **还有一条同类，而且方向相反（更别合并）**: `LayoutLoopRegressionTests:245` 的 `testSwiftUIRepeatForeverInLazyListSelfExcites` 也在飘，但**这一族量耗时、忙让数变大→越过上界**，那条**量固定窗口内的次数（是速率）、忙让数变小→跌破下界**。**一个机制解释不了两个相反的方向**，别把「冷启动首触」这套搬过去。见本文件下方那条。
- **为什么这条按「一族」记而不是一条一笔**: 三句共用一个机制、一条还法。**债本里一族洞开三条账，等于把一次修复拆成三次判断** —— 再撞到第四句，追加进本条的实测清单，**不要新开。**
- **该怎么还**: **先加一趟丢弃的预热**（测量前先跑一次、不断言）—— **只有这一条同时修好两种形状**：它既压掉绝对预算那边的冷启动溢出，也消掉相对比较那边「谁先跑谁吃冷启动」的不均匀。其余几种（相对基准 / 多次取中位数 / 挪进专门的性能 job / `XCTSkipUnless` 门控）**只修得了绝对预算那种，`:346` 那类照样飘** —— 别只做这几种就当还完了。总之别让计时测量混在功能全量里给出会飘的红绿。**在改成不飘之前，任何人拿「这条在 main 上是红/绿的」当前提，都要当场重跑一趟核实。**

### 🟡 `CrewLocalImageCacheTests` 两条**在飘** —— 断言把 `NSCache` 当成了「存了就一定在」
- **发现**: 2026-08-26 · 侧栏「手动藏起来一个 crew」落地时，在共享目录取合前基线撞上

**实测到的**（同一台机器、同一天，两趟全量之间隔约 20 分钟）:

- `main @ 6b3db2f` 一趟：**两条红**，`Executed 1643 tests, with 3 tests skipped and 2 failures`。
  ```
  Tests/PendingCrewTests/CrewLocalImageCacheTests.swift:33: error: -[PendingCrewTests.CrewLocalImageCacheTests testStoreThenPeekHits] : XCTAssertTrue failed - 同一 key 必须命中同一张，不该重解
  Tests/PendingCrewTests/CrewLocalImageCacheTests.swift:59: error: -[PendingCrewTests.CrewLocalImageCacheTests testDifferentMaxPixelIsDifferentEntry] : XCTAssertNotNil failed
  ```
  断言原文（`CrewLocalImageCacheTests.swift`，逐字）：
  ```swift
  XCTAssertTrue(cache.peek(key) === image, "同一 key 必须命中同一张，不该重解")   // :33
  XCTAssertNotNil(cache.peek(thumbKey))                                          // :59
  ```
- `main @ aa05a2a` 一趟：**两条绿**，`Executed 1670 tests, with 3 tests skipped and 0 failures`。
- 两趟之间落地的是侧栏可见性那六笔，**没有一笔碰 `Sources/Mac/Support/CrewLocalImageCache.swift` 或它的测试**。

**读代码读到的**（打开文件看过的那两行，不是推的）:
`Sources/Mac/Support/CrewLocalImageCache.swift:52-53` —— 底座是 `NSCache<NSString, NSImage>`，
文件自己的注释写着「`NSCache` 自带线程安全 + **内存压力下自动清空**，按像素字节数计成本」。
也就是说 `store` 之后 `peek` 返回 nil 是 `NSCache` 的**合法行为**，不是 bug。

**排掉「是不是 cost limit 设小了」这条**（同样是打开文件看到的，省下一个人重新去猜）：
`CrewLocalImageCache.init(costLimitBytes: Int = 64 * 1024 * 1024)`，而每个用例都是
`let cache = CrewLocalImageCache()` **新实例**、存的是 200×200 / 400×400 且已降采样到
≤100px 的小图 —— **不可能是自己撑爆 cost limit**，只可能是**系统级内存压力**触发
`NSCache` 全局清空。这跟「跑全量的那台机器同时在跑别的活」对得上，也解释了为什么它
偏偏在取基线那趟撞上。

另一条同层的：**全文件只有两处断言依赖「存了就一定在」，红的正是那两处**
（`:33` 的 `peek(key) === image`、`:59` 的 `XCTAssertNotNil(peek(thumbKey))`）。
同文件里其余几处 `peek` 断言全是 `XCTAssertNil`（"还没存过" / "覆盖后不该拿到旧解码
结果" / "看大图不该拿到缩略图"），对回收免疫，所以一次都没飘过 —— 这条对应关系本身
就是这个诊断最硬的一块。

**推出来的（标明是推的，没验证过）**:
- 「那两趟的红是全量满载下 `NSCache` 真被回收了」—— 机制说得通、也与 `peek` 返回 nil 的症状一致，
  **但我没有在回收发生的那一刻抓到证据**（没加计数器、没复现）。只跑到「实现允许这件事发生」为止。
- 「本次侧栏改动与图片解码缓存无交集」—— 依据是两条路没有共同调用点（侧栏可见性 vs 图片解码），
  **是从文件与职责推的，没有跟到调用链级别去证**。

- **同 commit 的一红一绿（2026-08-26 · `#14 reply_to 接自动@`）—— 「在飘」到此不必再排干扰项**:
  同一个 worktree、同一台机器、**同一个 commit `622fc8d`**，两趟全量隔约 20 分钟：
  **第一趟这两条红，第二趟绿**（`Executed 1690 tests, with 11 tests skipped and 0 failures`；
  两趟跑前跑后 `HEAD` + `git diff | shasum` 都逐字未变）。第一趟红时按本条判据单跑
  `-only-testing:CrewLocalImageCacheTests` **6 条全绿 0.08s**。
  **这对样本的价值在于它不需要对照组**：本条上面记的那对红绿在**两个不同 commit** 上
  （`6b3db2f` 红 / `aa05a2a` 绿），所以必须先花一段排「是不是那几笔改出来的」；
  **同 commit 的一红一绿没有这个退路可排。**
  ⚠️ **边界，跟证据一起读**：这对样本**只坐实「在飘」，没有指向机制** —— 它跟上面
  「`NSCache` 真被回收了」那条推测**方向不矛盾**，但两趟之间**没有任何内存压力的观测**，
  所以**不构成对该机制的确认**。「方向不矛盾」不是证据，是没有反证。
  ⚠️ **别把本条上方那条 `CrewChatOpenCostTests` 的「冷启动首触」搬到这条上** —— 见那条末尾
  「另有一条同类（别合并成一条）」：**那条是冷启动下的计时测量，这条是 `NSCache` 的可回收语义。**
  照冷启动那条的还法（加一趟丢弃的预热）来修这条，修不动。

- **为什么记**: 危害不在这两条用例本身，在**它们会污染别人的对账**。任何人拿全量差分判断
  「我这一改打红了什么」时，一条会自己红自己绿的用例就是一个假信号 —— 而它红的时候看起来
  非常像真 bug（"同一 key 必须命中同一张"）。
- **该怎么还 —— 两条用例情况不同，分开办**:

  **`testStoreThenPeekHits`（:33）不许改成条件断言。** 整条用例的存在理由就是「存了要
  命中」，把它写成 `if let hit = cache.peek(key) { XCTAssertTrue(hit === image) }`，在
  `peek` 恒返回 nil 时照样通过 —— **一个名字承诺「会命中」、断言却不再检查命中的测试**，
  正是本仓库反复在清的那种安静的死：编译器不报、测试不红、看代码也看不出来。两条正路：
  - 给缓存留一道缝：把存取抽到一个可注入的协议后面，测这条契约时用一个**不会被系统回收**
    的实现（一个普通字典就够）。`NSCache` 那半留给真跑的路径。
  - 或者**改名 + 注释写明**「`NSCache` 不承诺命中，本例只钉『命中时必须是同一张』」。
  **二选一，不许只改断言不改名** —— 名字和断言必须对得上。

  **`testDifferentMaxPixelIsDifferentEntry`（:59）可以放宽，但只放宽一半。** 它真正要防的
  是最后那句 `XCTAssertNil(cache.peek(fullKey), "看大图不该拿到 100px 的缩略图")` ——
  **那句对回收天然免疫**（回收只会让它更容易过）。飘的只有前面那句
  `XCTAssertNotNil(cache.peek(thumbKey))`，它在这条用例里只是个前置铺垫，不是被测契约。
  放宽它，并**在注释里写清楚为什么只有这半可以放宽**，别让下一个人照着把上面那条也放宽了。

  **一条别走的弯路**：测试里那个 `let image = ...` 的强引用**挡不住** `NSCache` 在内存
  压力下清空 —— 别以为多持一个引用就修好了。

- **另有两条同类（别合并成一条）**: 上面那条 `CrewChatOpenCostTests` 也在飘，机制是
  **在功能全量里做计时测量、而第一次触碰是冷的**（别写成「绝对毫秒预算对负载敏感」——
  那个表述盖不住它的 `:346`，见那条自己的说明）；下方 `LayoutLoopRegressionTests:245`
  那条则是**速率判据跟负载耦合、方向相反**。三条的机制与还法各不相同。

### 🟡 `LayoutLoopRegressionTests` 那条自激判据**在飘** —— 量的是速率，却跟一个绝对次数比

- **发现**: 2026-08-26 · #66 B 段取全量基线时撞上
- **位置**: `Tests/PendingCrewTests/LayoutLoopRegressionTests.swift:245`
  `testSwiftUIRepeatForeverInLazyListSelfExcites` 的第二句：
  `XCTAssertGreaterThan(n, 1000, …)`（第一句 `XCTAssertLessThan(idle, 10)` 不在此列，见下）
- **实测读数（每条写清那一趟是单跑还是全量）**:
  - **929** —— 全量、机器满载：**红**（离阈值 1000 只差 8%）
  - **10105** —— 单跑：绿
  - **28917** —— 全量：绿
  **两趟全量差 31 倍**，这是本条区别于隔壁那族的硬证据。
- **机制**: 这条量的是**固定时间窗（安静窗口）内 `layout()` 被调用的次数** —— 那是**速率**，
  不是耗时。**机器越忙，同样一段墙钟时间里跑得越少，数就越小**，于是撞下界。
  自激本身有没有发生跟机器忙不忙无关，但**这把尺子的读数跟负载死死绑着**。
- **⚠️ 别把上方 `CrewChatOpenCostTests` 那族的「冷启动首触」搬到这条上 —— 方向是相反的**:
  - 那族量**耗时毫秒**，冷/忙让数**变大** → **越过上界**（`XCTAssertLessThan`）；
  - 这条量**窗口内次数**，忙让数**变小** → **跌破下界**（`XCTAssertGreaterThan`）。
  **一个机制解释不了两个相反的方向。** 而且冷启动也解释不了「同为全量的两趟差 31 倍」——
  两趟的首触都是冷的，性质相同；**机器忙不忙才解释得了。**
- **为什么另外两句不在此列**（别顺手一起改）:
  - 同一 test 的第一句 `idle < 10`：idle 实测恒为 0，**负载让它更小**，只会更安全；
  - 隔壁 `testBreathingSymbolDoesNotSelfExcite` 的 `n < 10`：同理，**负载让它更容易绿**。
  **只有「要求次数足够多」的那一句会被负载压红。**
- **该怎么还**: **跟同条件下的 idle 基线比，别跟绝对数比。** 这条 test 里**已经先量了 idle**
  （`quietTodoLayouts(breathingRow: -1)`，实测恒为 0），两个测量点同机同时同负载 ——
  把判据改成「有呼吸那趟比 idle 高出一个数量级以上」，负载对分子分母同向作用，就抵掉了。
  **别用「把阈值从 1000 调到 500」了事** —— 那只是把同一把跟负载耦合的尺子往下挪，
  下一台更忙的机器照样红，而且越挪越接近「自激真的没发生」也能过。
- **在改之前**: 谁撞见这条红，**先看同一趟日志里 `[quietTodoLayouts]` 打的 idle 数** ——
  idle 是 0 而这条只是没到 1000 ⇒ 属本条，不是回归；**idle 自己就不是 0 ⇒ 骨架真出事了，去查骨架。**

### ✅ `WorkdirMigrationPlan` 搬 transcript 那半已失去存在理由（**已还，2026-08-26 · 机长作战板 #12**）

- **在哪**: `Sources/Mac/LocalRunner/WorkdirMigrationPlan.swift` 与 `WorkdirMigrationExecutor`
  里所有与 claude 会话日志搬运相关的分支：`Action.moveClaudeTranscript` /
  `moveClaudeTranscriptSidecar`、`Skip.transcriptSourceMissing` / `transcriptTargetExists` /
  `sessionStillLive` / `codexSessionNeedsNoMove` / `unknownAgentKind`、
  `Plan.claudeTranscriptMoveCount` / `affectedMembers` / `pendingSweepMembers` / `isSweep`，
  以及**整个清扫模式**（`isSweep` + `sourceDirectory` 走 `previousWorkingDirectory` 那条分支）。
- **为什么失去理由**: **不是因为我们后来记了工作目录**，而是因为 **claude 压根不按目录找会话**。
  2026-08-26 实测（claude 2.1.246，Todo #68）：把 jsonl 挪到一个跟任何真实路径都对不上的
  目录，再换第三个目录 `--resume <同一个 id>` **照样接上**；挪到 `~/.claude/projects` 树外
  才报 `No conversation found with session ID: <id>`。官方 `--help` 划的是同一条界：
  `--continue` 写明 *in the current directory*，`--resume` 一个字都没提目录。
  **搬它零功能收益。** 完整查实见 `docs/internal/2026-08-26-session-resume-workdir-evaluation.md`。
- **必须留，别一起删**: `copyClaudeProjectSettings`（`~/.claude.json` 的
  `projects["<绝对路径>"]` 信任条目）、`copyCodexTrust`（`~/.codex/config.toml` 的
  `trust_level`）、`copyClaudeMemoryFile`、`setCrewWorkingDirectory`。**这四样跟记不记
  工作目录完全无关** —— 少了第一条，新目录下第一个 session 会**挂在**信任提示上
  （不是弹个框就过去，是停住不动，而点名显示为「空闲」，见 `CrewSessionsSnapshot.state` 注释）。
- **删了会漏掉什么（诚实的那一栏）**: 旧 slug 下会永久留一堆不再对应任何真实目录的文件夹
  —— **整洁问题，不是正确性问题**，而且今天本来就有 62 个 worktree 的日志是这个状态
  （其中 52 个 worktree 已被删除）。万一将来 claude 改成按目录找，这套又需要；但风险可观测：
  新的降级路径会当场把 claude 的原话报进群里，不会静默失忆。
  `sessionsBusy` 建议保留，但注释要改 —— 它从「保护正在写的文件」降级成一条常识判断。
- **为什么当时没顺手删**: 那一版（Todo #68）改的是「续不上」的**病根**，属修复；删搬运是
  **清理**，风险面不同，diff 会盖过修复本身。**所以单独排了机长作战板 #12。**
- **已完成（2026-08-26）**: 照 §3.2 逐条删净 —— 两条 `Action`、五条 `Skip`、
  `Plan` 的四个输出面、整个清扫模式（`sourceDirectory` 回落 `previousWorkingDirectory`
  那支），连带 `Inputs.agentSessions` / `AgentSessionInput` 及其上游
  （`WorkdirChangeCommand` 里那次 `LocalAgentSessionStore.list()` 与 `memberName`、
  `LocalCrewStore` 的 `previousWorkingDirectory` 透传）、执行层两条动作与回执/预览渲染、
  界面预览两行。**§3.1 那四样一个字没动。**
- **两处只改语义不改行为**: `sessionsBusy` 保留、注释改了（它不再保护任何文件，
  从硬约束降级成常识判断）；`LocalCrewStore.previousWorkingDirectory` **字段保留**
  （持久化留痕，删它会把已写进 `local-crews.json` 的历史一次丢掉），注释写明
  **当前无消费者**。
- **评估漏了一处，删的时候才发现**: 机长 `change_workdir` 的**工具描述**里写着
  「留待清扫 / 幂等 / 再调一次」，还有第三个测试文件 `McpServerWorkdirToolTests`
  钉着那两个词。**漏的原因是评估按实现词（`transcript`）grep，而那几句里一个
  `transcript` 都没有** —— 这一刀的**对外文案面比代码面散**。已改口为「一次做完、
  没有第二趟」并把依据（`--resume` 按会话号找全盘）写进描述：**留旧文案比留死代码糟，
  死代码不骗人，过期的工具描述会让机长照着再调一次、以为自己补上了什么。**
  收尾时按语义（清扫/幂等/留待/再调一次）又扫了一遍，没有第四处。
- **删了会漏掉什么，仍然照上面那栏算数** —— 旧 slug 下会留着一堆不再对应任何真实
  目录的文件夹，是整洁问题不是正确性问题。

### 🟡 终端镜像视图里跨行拖选 + ⌘C 复制不出正确文本
- **发现**: 2026-09-02 · 人类在 P4 的 A2 测试包（0.1.23 build 2026090201，P4@77b996a）上实测，
  原话「选中文字还是不行」。2026-09-04 由父机长明确**拆成独立终端 bug 记账**，
  不再作为「常驻后台 / 前后端分离」P4→main 的合并闸门。
- **位置（未定位到行，只框到面）**: `Sources/Mac/LocalRunner/TerminalMirrorView.swift`
  —— 终端劈成两半之后（`c169fd5`），窗口侧是一个只吃字节流的 SwiftTerm 镜像视图，
  选中/复制由这个控件自己提供。设计文档 §2.4 当初的判断是
  「选中复制、回滚缓冲、reflow 全部由原生真终端控件提供，代码路径一行不改」——
  **这条判断被这次实测证伪了，但证伪到哪一层还没查。**
- **还不知道的（别当成已知）**: ① 只在 daemon/镜像路径出现，还是 inproc 老路也一样；
  ② 是选区几何算错、还是取文本时按行截断；③ 中文宽字符是否是触发条件之一。
  人类只回了「还不行」，**没有逐项现象**，所以上面三条一条都没有排除。
- **为什么没有自动测试照出来**: 选中与复制走 AppKit 的鼠标事件与 pasteboard，
  现有 A2 自动化（`TerminalMirrorParityTests` 等）验的是**缓冲区内容一致**，
  不是**选区→pasteboard 这条链**。这两件别混。
- **该怎么还**: 先在**不开窗口**的前提下把能验的那半验掉——直接对 SwiftTerm 的
  `Terminal`/选区 API 做单测：跨三行构造一段中英文混排的缓冲区、取选区文本、
  断言换行与宽字符。**先证明这把尺子会红**（用人类描述的那种形状），再修。
  只有确实非 GUI 不可的那一小段（真鼠标拖拽）才请人点一下。

### 🔴 「把文本从终端格子里弄出来」有三条各走各的路，各错各的
- **发现**: 2026-09-04 · P5a 真 daemon 冒烟。三条路是在同一天被三件不相干的事分别照出来的，
  这本身就是判据：**它们已经分叉到不会一起对、也不会一起错。**
- **三条路**:
  1. **AppKit 选中 / 剪贴板** —— `TerminalMirrorView`。人类实测跨行拖选 ⌘C 复制出来不对
     （2026-09-02，本文件另有一条独立记账；已从前后端分离主线摘出，别顺手一起改）。
  2. **无画面 `screenText`** —— `AgentSessionCore.screenText`（`inspect_session` 走它、
     写进白板的「它最后一句话」走它、`--daemon-attach` 探针也走它）。实测**本该是空格的
     位置输出 NUL**，而且不只在全角字后面：
     `'Claude\x00Code'`、`'auto\x00mode'` 全是纯 ASCII。病根是
     `line.translateToString(trimRight: true)` —— SwiftTerm 里没被写过的格 char 就是 `\0`，
     没人映射成空格。终端里 NUL 不显示，所以**肉眼一直看着是对的**，只有把它当文本用才露馅。
     修的时候要分清两种 NUL：没写过的格（`width == 1`）该变空格，全角字的后半格
     （`width == 0`）必须丢掉 —— 一视同仁会把「我在」变成「我 在」。
  3. **去 ANSI 的字节尾窗** —— `PendingDecisionTracker`（判「终端在等人选」）。
     claude 的「是否信任此文件夹」对话框在这条路上**认不出来**：序号后面没有空格、
     行尾 `\r\r\n` 又把连续的选项块切断。
- **为什么这是 🔴 而不是三条 🟡**: 三条路读的是同一份缓冲区、回答的是同一个问题
  （「屏幕上现在是什么字」），却各有各的解析。任何一条修对了都**不会**带动另外两条，
  而它们的错法互不相似（丢空格 / 认不出对话框 / 复制出错），所以没有一条测试能同时钉住。
  真正的债不是这三个 bug，是**没有一个「屏幕 → 文本」的单一实现**。
- **已经在做的**: 第 2 条正在修（映射成 U+0020，抽成共享纯函数 `TerminalScreenText`，
  让 `screenText` 与探针都调它）。**第 3 条与第 1 条明确不在那一期里**，各自独立。
- **该怎么还**: 等第 2 条那个共享纯函数落地后，把第 3 条改成读渲染后的画面
  （而不是自己去 ANSI 扫字节）—— 它要判的本来就是「屏幕上有没有一个选择菜单」，
  读画面比读字节流更接近问题本身。第 1 条走 AppKit，能不能并进来要单独评估，
  **不要假设它一定能**。


### 🟡 crew 白板通道会对某个 session **单向断开**，根因两次都没查出来
- **发现**: 2026-08-27（P4 交付期，见 `docs/internal/2026-08-27-p4-handoff-report.md` 第八节）
  与 **2026-09-04**（P5a 收口期）各一次。
- **症状**: 某个 session 的 `post_to_crew` / `read_whiteboard` / `directory` 全部报
  「未能打开文件 `<crewId>.json`，因为你没有查看它的权限」，**写入方向断、`report_to_parent`
  方向仍通**。工具回执明说「本次一个字都没写、原件未动」，所以不丢数据 —— 丢的是**话**。
- **已排除的**（2026-09-04 当场核过）: 不是文件坏了、也不是权限位不对 ——
  `ls -l` 是正常的 `-rw-r--r-- hey staff`，而且 **mtime 一直在跟着别的进程更新**
  （同一时刻别的 session 正常写同一个文件）。所以是**本进程被挡在那个目录外面**，
  不是文件的问题。
- **2026-09-04 新增的两个数据点**（比 08-27 那次多知道的）:
  1. **同一时间窗里有两个互不相干的 session 同时中招**（机长 + 一个 worker），
     所以不是某个 session 自己的状态问题。
  2. 它有一个**沉默的连带受害者**：`CrewMentionFilterRealWhiteboardTests` 那三条要读
     同一个目录，读不到就 `XCTSkip`。于是「通道断了」会伪装成「skip 从 3 变成 6」——
     **一次故障同时污染了沟通和判据**，而且两边都不报错。
- **代价**: 不是数据损坏，是**协作静默**。人和队友看不到你的话，而你自己以为发出去了
  （工具回执其实说了没发，但很容易被当成噪音划过去）。
- **绕法（两次都用了同一个）**: 结论写进仓库文件 + 走 `report_to_parent` 请上级代发。
  `docs/internal/2026-09-04-p5a-closed-loop-evidence.md` 就是这么落下来的。
- **该怎么查**: 下次发生时**当场**抓这三样（事后就没了）：① 本进程的 `sandbox` / TCC 状态；
  ② `ls -l@` 看扩展属性与 ACL（不只是权限位）；③ 同一时刻能写进去的那个进程是谁、
  它和中招的进程有什么不同。**别再只记「排除了什么」**——两次的排除项已经重合，
  再排一遍不会有新信息。

- **2026-09-05 第三次：边界量出来了，但我据此下的根因**（TCC）**当场被推翻了 ——
  两条都记在这，因为「被推翻」本身是这条目前最硬的信息。** 先说站得住的那一半：
  边界不是「某个文件」，是**整个数据根的文件内容**（这部分是量的）：

  | 探针 | 结果 |
  |---|---|
  | 列目录 `ls ~/Library/Application Support/PendingCrew/` | **可列** |
  | `stat` 取大小（元数据） | **可取**（112600 字节） |
  | `head -c 1 local-crews.json`（读内容） | **Operation not permitted** |
  | 在该目录下新建文件再读 | **失败** |
  | `~/.claude.json` / `~/.codex/config.toml` / 仓库文件 | **全部可读** |

  **形状是「元数据放行、内容拦截，读写都拦，且只拦这一棵子树」。** 这个形状本身是真的，
  它排除了 POSIX 权限和文件损坏 —— 但**推不出是谁在拦**：TCC 会长这样，一个沙箱层
  也会长这样。

- **⚠️ 我从这个形状推出「是 macOS TCC 保护 `~/Library/Application Support/<App>/`」，
  被两条证据当场推翻，别再照着它查：**
  1. **父 crew 机长同一时刻读写全通，而且责任进程是同一个** `PendingCrew(95591)`
     （他自己的链是 `zsh(18235) ← claude(7019) ← PendingCrew(95591)`）。
     同一个责任进程、几乎同一时刻、相反结果 → **不可能是挂在责任进程上的授权。**
  2. **它自己好了。** `02:41:53Z` 重探：`ls` OK / `stat` 112600 / `read` OK /
     `write` OK / 白板文件 OK，**中间我什么都没做**。
     **一个会自我恢复的东西，本来就不该往「授权被撤销」上想** —— 这是我推错的地方。

- **连带塌掉的还有我当时那条「关键证据」**：我拿「同一 session 内 `01:20Z` 读成功 →
  `01:43Z` 又成功 → `~01:50Z` 起全挂，中途没换进程」去论证「授权在会话中途丢了」。
  既然它随后又自己回来了，这段时间线**只能证明「有一段时间不通」**，证明不了授权丢失。
  **同一份观察，套上一个机制就变成了「证据」——机制是我加的，观察本身没这么说。**

- **前两次查不出来，现在有了解释：它是瞬时的、会自愈的。** 等有人去查的时候它已经好了，
  所以在文件那头怎么找都是干净的。**下次要抓就得在断的当口抓。**

- **同一次故障里还有第二个受害者，而且它也自愈了 —— 这一条收窄了范围。** 本 crew 的
  worker（`worker-2f960ba3`）在同一时间窗里 `post_to_crew` **连挂六次**，随后
  **不做任何事自己恢复**；而**父 crew 机长在同一时刻读写全通**。合起来：

  | | 状态 |
  |---|---|
  | 本 crew 机长（我） | 断 → 自愈 |
  | 本 crew worker | 断六次 → 自愈 |
  | 父 crew 机长 | 全程正常 |

  **同一个 crew 的两个 session 一起断、另一个 crew 的 session 不受影响。**
  这跟 2026-09-04 那次记的「两个互不相干的 session 同时中招」是同一形状 ——
  **两次都是「成对/成群地断」，不是某一个 session 倒霉。**

  所以「查那个 session 自己的沙箱 profile」这个方向**要打个折**：一个 session 独有的
  配置解释不了「两个 session 同时断、又同时好」。更像是**它们共用的那一层**
  （同 crew 的文件锁 / store / helper 进程），或者一个**短时的全局互斥**。
  但注意我这次读不了的不止本 crew 白板，`local-crews.json`（全局共享）也读不了 ——
  所以「按 crew 分隔」这个说法**本身还没被证明**，只是两个受害者恰好同 crew。
  **这句是推的，别当结论用**（上一轮我就是在这一步把推的写成了结论）。

- **下一步该查哪（父 crew 机长指的方向，我认）**: 差异在 **claude session 这一层**——
  两个 claude session，一个通一个不通，同机同责任进程同一时刻。查它们的**沙箱 profile /
  settings / 启动参数**的差异。**不是查文件（前两次），也不是查 PendingCrew 的 TCC（我这次）。**

- **连带**: 白板写不了的同时，**机长作战板（`plan_update`）也写不了** —— 它跟白板同在
  那棵子树下。所以症状不止「发不出话」，还有「进度板停更」，两者会被误当成两件事。

### 🟡 判断留在「接线层」= 只有「编译过」这一条保证（同一形状今天复发三次）
- **发现**: 2026-08-26（P4 编排闸门）与 **2026-09-04**（P5a 翻默认，同一天内两次）。
- **形状**: 一段判断挂在**进不了 test bundle 的接线上**（SwiftUI 视图、`Sources/Mac/Services`
  下的 client 对象）。它编得过、看起来也对，但**没有任何测试能证明它对**，
  于是错法只能靠人读代码发现——而它每次看起来都毫无异样。
- **三次实例**:
  1. P4：编排闸门挂在 `MacRootView` 的 `.task` 上 → 「app 到底取没取锁」在 app 侧
     **无法被证明**。修法不是补一句取锁，是把闸门搬到 `PendingCrewEntry.main()`
     ——**换到能被测的地方**（见 `docs/internal/2026-08-27-p4-handoff-report.md` 第二节）。
  2. P5a：`ViewerSessionClient` 里留了两处拉起判断（「锁上写着有 daemon 就不拉」
     「上次拉的还活着就不再拉」），其中第二条是实现者自己发明的规则。
     **发明的规则比继承来的更需要测试**——没有人替它背过书。已下沉为 `DaemonLaunchPlan`。
  3. P5a：`openLink()` 把「socket 连上了」当成「握上手了」——`isConnected = true`
     紧跟在 `connect()` 之后，从不等 `daemonHello`。判据要的是**握手**，实现拿了个
     **更早、更弱、本端单方面就能达成**的信号顶替。后果是接受连接但不回话的 daemon
     会让降级判定永远不被调用（读代码读到的，未复现）。
- **为什么它总是复发**: 接线是「把已经想清楚的东西连起来」的地方，所以往里塞一个
  `if` 感觉上不像在做决策。**判断力和可测性的边界不重合，是这条债的本体。**
- **该怎么还（两条口径，都已在用）**:
  - **接线里不许留判断**。留了就往下挪到纯判定层，连同它的理由一起挪。
  - **判据换了地方，就要跟着换到能被测的地方**——否则只是把一个「读代码才发现得了的
    错误」换成另一个「读代码才发现得了的错误」。
- **挑信号的判准**（第 3 例逼出来的）：一个能代表「对端确实在」的信号，
  **必须只有对端真的回过话才可能出现**。`connect` 成功不满足，`把 hello 发出去了`
  也不满足——两者本端单方面就能达成。
- **挑落点的判准**（2026-09-04 又逼出来的一条，机长自己指错过一次）：说「收进一处」
  之后，必须再问一句 **「那一处到底包住了哪几步」**。`LocalCrewStore` 这条上，
  机长指的落点是「把锁收进 `persistToDisk`」——听起来就是标准答案，实际上
  **锁内只有「写」**：12 个方法都是「先改内存数组、再调 `persistToDisk`」，
  「改」在锁外，「读」更是在启动时就发生了。正确的收口点是
  `mutate { crews in … }`（**锁内重读 → 应用变更 → 落盘**），调用方只交出「改什么」。
  **一个听起来就是标准答案的落点，可能只包住了三步里的一步。**

### 🟡 「期望状态没达成」被编码成成功：daemon 与 `--daemon-status` 都 exit 0
- **发现**: 2026-09-04 · P5a 两次真机实测各照出一半。
- **两处**:
  1. **daemon 本体**：`SessionDaemonMain.run` 的 `exit(error is StartError ? 0 : 1)`。
     于是「锁被**另一个 daemon** 占着」（期望状态**已经成立**，exit 0 是对的）与
     「锁文件**打不开**」（期望状态**没达成**，exit 0 是谎报成功）**共用退出码 0**。
     实测：数据根 `chmod 500` → `打不开 …/orchestrator.lock：Permission denied` → **退出码 0**。
     **任何拉起方都无法从退出码分辨这两者**（这也是为什么 app 侧的判据刻意不解析退出码，
     改由锁的观测去分辨 —— 见 `OrchestrationFallback`）。
  2. **`--daemon-status`**：`SessionDaemonStatusMain.runIfRequested` 的 catch 里只
     `print(...)`，不设退出码。实测（对一个只 accept 不回话的哑监听）：
     `PendingCrew 后台未运行或不可连接：后台已占用 socket，但状态握手超时` → **退出码 0**。
     一个诊断命令**说了「不可连接」却报成功**，脚本化用法分不出「后台好着呢」和「连不上」。
- **它是怎么被发现的**（值得照抄的做法）：worker 在跑之前先把预测写下来（「应当以人话报错
  **并非 0 退出**」）。行为那半对上了，**退出码那半没对上** —— 而如果没有事先写下预测，
  那句措辞漂亮的错误信息会让人直接点头过去，`退出码=0` 根本不会被看见。
- **该怎么还**: 两处合成一笔：把「期望状态没达成」一律映射成非 0；daemon 保留
  「锁被别的 daemon 占着 → exit 0」这一支（它确实是正确结局），其余失败非 0。
  **要先证明尺子会红**：现有的两条真机复现（`chmod 500` 的数据根、哑监听）就是现成语料。
- **不要顺手做的事**: 别让 app 侧判据回头去解析退出码 —— 那条路今天是故意不走的。
- **✅ 已还（2026-09-04）**：`DaemonExitCode`（`SessionDaemonHost.swift` 尾部，纯判定 +
  `DaemonExitCodeTests`）。改后实测（**上面那两份 `退出码=0` 是当时的历史记录，不改**）：

  | 情形 | 改前 | 改后 |
  |---|---|---|
  | 数据根 `chmod 500` 起 daemon | 0 | **1** |
  | 后台没在跑时 `--daemon-status` | 0 | **1** |
  | 已有**另一个 daemon** 时再起一个 | 0 | **0**（期望状态已成立，不变） |
  | 后台正常在跑时 `--daemon-status` | 0 | **0**（不变） |

  顺带把 `StartError` 拆开了：`alreadyOrchestrated(_, holderIsDaemon:)` 与
  `lockUnavailable` —— 「已经有人在编排」和「连锁都打不开」原来挤在同一个 case 里，
  退出码分不开正是因为**语义先分不开**。
  **这笔改不构成任何契约**：`DaemonExitCode` 的类型注释里写死了「判据一律不许读退出码」
  及其因果（退出码是约定不是观测，两种情况本来就可能同码），免得将来有人看到
  「退出码变准了」就把 app 侧判据改回去解析它。

### 🟢 daemon 不记「排空了哪几条机长命令」，事后无法判定重复执行
- **发现**: 2026-09-04 · P5a 收尾闭环。投了两条 `start_session`（claude + codex），
  `--daemon-status` 报出**三个** session，其中两个 claude 的 brief **逐字相同**。
- **判定不了**: 命令文件是「排空即删」，daemon 日志**只记启动/连接/退出，不记排空**。
  等发现异常时，文件没了、日志里没有，**证据在事发那一刻就已经不存在了**。
- **受控复现（做了，结论是「没复现」）**: 起 daemon **之前**写好**恰好一个**命令文件、
  `ls | grep -c crewcmd` 确认 = 1，再起 daemon → 结果 `运行中 session：1`，等 15 秒
  再问仍是 1，且那条 session 的开场 brief 正常送达并有回答。**命令通道重复执行没有复现。**
- **所以这条账记的不是那个异常，是那个异常查不了**: 最可能的解释是发起侧（一个 shell
  循环）多写了一个文件，但**我给不出证据**，只能说「没复现」，不能说「不会发生」。
- **该怎么还（很便宜）**: 排空时往 daemon 日志写一行 —— 命令 id、kind、crewId。
  一行日志就能把这类问题从「查不了」变成「一眼看出是一条还是两条」。
  **同类价值**：`start_session` 是要花订阅额度的动作，重复执行的代价不只是多一个进程。
  **那一行要连命令**文件名**一起记**，不只是 id/kind/crewId —— 两个候选解释在日志里
  长得不一样：「发起侧多写了一个文件」是**两条不同文件名**，「同一个文件被排空两次」是
  **同一个文件名出现两次**。只记命令 id 的话，若重复写入用的是不同 uuid，**两种解释仍然
  分不开** —— 那这行日志就白记了。

**⚠️ 2026-09-04 稍后：复现了，上面那条「最可能是发起侧多写了一个文件」是错的。**
（原文保留不改 —— 账本回头改数字就不是账本了。）**这一趟输入是数过的**：
`起 daemon 前命令文件数 = 2` → `运行中 session：3`，两个 claude 的 brief 逐字相同，
**agent 会话号是两个不同的**（真起了两个进程，不是显示重复）。三次实验形状一致：
**1 条命令 → 1 个；2 条命令 → 3 个，重复的是排在前面的那条。**

**病根**（读代码，且解释了全部三次观测）：`CrewStore.drainPendingCommands` 在循环里
**逐条 `sessionSpawnRequests.append(...)`**，而它是 `@Published`；消费方
`SessionHost` `.receive(on: .main)` 拿到的是**每次 append 各发一次的快照**：
append cmd1 发 `[cmd1]`、append cmd2 发 `[cmd1, cmd2]`。两次投递都到主线程：
第一次处理 `[cmd1]` 并清空，第二次拿到的仍是发出时那份 `[cmd1, cmd2]` →
**cmd1 被处理第二遍**。

**形状本身错了**：发布的是**变化**，消费的却当成**待办队列**。别用「按 id 去重」
糊 —— 那是在下游擦屁股。`CrewStore` 里注释写着「数组语义同 `sessionSpawnRequests`」
的还有几条，**同一形状要一起看**。

**代价**：`start_session` 花订阅额度，机长连派两个活时第一个会起两遍。
**不是 P5a 回归** —— GUI 模式同一份代码，应该一直都在。

**给下一个人的那条教训**：我第一次查不下去时给了一个「听起来合理的归因」
（发起侧多写了一个文件），差点把这个 bug 结掉。**给不出证据时只说「查不了」，
别给归因** —— 一个合理的归因比一句「不知道」更能让人停止追查。
**更锋利的说法（worker 提的，比上面那句好）**：「最可能是**我自己**错了」听起来是谦虚，
其实和「最可能是**它**的错」一样是**归因**，只是方向朝内 —— 两者一样会把问题结掉，
**而朝内那种更难被人反驳**。当时唯一站得住的只有「查不了」。

### 🔴 「静默失效」的五种穿法 —— 这一期反复撞的其实是同一件事
- **发现**: 2026-09-04 · P5a 全程。**不是五个 bug，是同一类毛病的五种穿法**。
  之所以值得单列：修掉其中一种**完全不会**让另外四种露出来，而它们的症状互不相似，
  所以每一次都像是「又一个新问题」。
- **五种，各配当天的真实实例**:
  1. **不报错** —— 开场 brief 被首屏重绘吞掉，TUI 起来了、输入框空的、transcript 零条，
     状态照样 `.running`，点名显示「空闲」。
  2. **报了，但那句话不可操作** —— 卡住的 daemon 持着锁不回话，界面一直说
     「后台进程正在运行，本窗口继续重连」。措辞越让人安心越难被发现。
  3. **报成功，其实失败** —— daemon 拿不到锁时打一行原因然后 `exit(0)`；
     `--daemon-status` 说「不可连接」也 `exit 0`。**把「期望状态没达成」编码成了成功。**
  4. **无限期报「正在进行」** —— `openLink` 把「socket 连上」当「握上手」，
     于是接受连接但不回话的对端让降级判定永不被调用，界面在「重连中」里打转。
  5. **同一句话对应两种完全不同的处境** —— `--daemon-status` 的输出里**一行都没提数据根**，
     于是「后台确实没在跑」和「你问错了数据根」长得**一模一样**；
     失败那条同样不说它去哪儿找的。
- **它们的共同解药**（三条，都在这一期用过，按强度排）:
  - **把「我在看哪儿 / 我凭什么这么说」写进输出**（第 5 种、那行排空日志都是这一条）。
  - **让错的那种写法表达不出来**（`Bool` → 三态；12 个落盘点 → 一个收口；
     入队与发脉冲绑进同一个 `enqueue`）。
  - **给「等待」一个寿命** —— 任何「正在……」都必须有上限，超时后**文案升级成可操作的**
     （第 2、4 种）。
- **⚠️ 三条解药不能互换，别拿错工具**（2026-09-04 补，worker 提）:
  - 「**让错的写法表达不出来**」只在**错的形状进得了类型系统**时管用 ——
    `Bool` 换三态 / 落盘收一个口 / 入队与发脉冲绑死，都是因为错法能被编译器表达成
    「一个可以填错的参数」或「一个可以漏掉的调用点」。而**第 5 种（同一句话对应两种
    处境）没有这种把手**：没有哪个类型拦得住「这句话说得不够」。
  - 反过来，「**把我在看哪儿写出来**」对第 2、4 种**不够** —— 无限期「正在连接」
    当时说的话**没有错**，错的是它**永远不变**。
  - 粗略对应：**写出依据 → 第 3、5 种；让错法写不出来 → 第 1、2 种；给等待寿命 → 第 4 种。**
    三条一起才盖住这一族。
  - **给等待寿命有个反面**：寿命给短了会把「慢但没事」变成**新的假警报**。所以超时文案
    必须连「**等了多久、我据此认为什么**」一起说 —— 这又绕回第一条。
    **这一族的解药里，「写出依据」是地基。**
- **诊断时的第一步（零代码，现在就能用）**: 看到「后台未运行」**先别判故障**，
  先确认问的是不是**同一个数据根**（带上 `PENDINGCREW_DATA_DIR` 重问一次）；
  仍说没在跑，再看 `orchestrator.lock` 在谁手上 —— **锁是独立于 socket 的第二个观测量**，
  它能把「后台没了」和「socket 连不上」分开。
- **第 5 种为什么当天没修**: 那行输出是**验收路径上的诊断工具**，
  在验收进行中改它等于**把人正在照着用的尺子换掉**。记账，等验收有结论再动。
