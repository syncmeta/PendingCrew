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
- **证据**: `docs/internal/2026-08-19-ui-jank-profile.md`。sample 实测：单个忙碌 session 的 `dataReceived` 子树占主线程 **7.9%**；机长常态派 3～5 个 → 24%～40%。
- **本次做了什么**: 把这条回调上**每一段**旁路工作的单价打下去了（squeeze O(n²)→O(n)、健康短语改字节匹配、打字指纹单趟折叠），合计把单 session 的稳态占比压掉一个数量级以上，并消掉了「每起一个 session 主线程被占死 45 秒」那条。
- **没做也不该在这条分支上做的**: **结构本身没变** —— 代价仍然随 session 数线性涨，仍然全在主线程，仍然与「用户在看哪个终端」无关。真正的解法是把 session 搬进常驻后台进程（机长 2026-08-19 已在单独排期，与「前后端分离」同一刀），app 退化成看的那个窗口。**这条不要顺手在功能分支里动。**
- **修后复采（2026-08-19）**: 结构没变，但单价打下去之后实测 **10 个 session（6 个在忙）合计只占主线程 1.51%**（≈0.25%/忙碌 session）。
  也就是说这条 🔴 现在**不再是当前的卡顿来源**，它仍然是 🔴 是因为「代价随 session 数线性涨、且与用户在看哪个终端无关」这条结构事实没变 ——
  派到几十个 session 时它会重新变成主导项。见 `docs/internal/2026-08-19-ui-jank-profile.md`「现场复采」。
- **中间态的可选缓解**（如果地基活拖久了）: 后台 session 的旁路扫描改成攒批 / 挪出主线程；或把 scrollback 与扫描窗按「是否前台」分档。都属于治标，记在这里免得被当成已解决。

### 🔴 #63 第一期删完之后，远端 / 登录整层变成「编得过、永不执行」的安静死代码
- **发现**: 2026-08-25 · Todo #63（删登录与远端客户端）第一期落地时
- **怎么来的**: #63 删掉了**取得凭据的所有入口**（Auth 三件套 / Supabase 栈 / 两个登录页 / 扫码页 / 侧栏登录入口 / RootView 的登录分支）。凭据层本身（`AppModel.credential`）没删 —— 它被 `Sources/Remote/`、`CrewRelayAgent` 这批「一个字不许动」的文件顶着，删不动，留给第二期。
- **于是变成什么**: `AppModel.isAuthenticated` **恒 false**，`loggedAPIClient()` 恒抛 `notAuthenticated`，`imageAuth` 恒 nil。所有调用点都是 `guard ... else { return }` / `try?` 早退 —— **编译器不报、测试不红、看代码也看不出来**。这正是本仓库反复栽的那一类：一种安静的死。
- **第二期一刀端掉，触发条件已满足。** 下面是逐个文件的现成名单，**别再重新考古**（行数/处数为 2026-08-25 在 `main` 上实测）：

  | 文件 | 死在哪 |
  |---|---|
  | `Sources/Remote/CrewSessionServerLink.swift` | 整个 struct（`let api: PendingCrewAPI`，唯一构造点在 `CrewSessionWindowView.prepareServerSession`） |
  | `Sources/Remote/SessionProxyClient.swift` | 整个 actor（viewer / runner 两侧都只在登录态起） |
  | `Sources/Remote/SessionProxyProtocol.swift` | 只服务上面那个（codec 单测 `SessionProxyProtocolTests` 还绿，测的是纯 codec） |
  | `Sources/Views/Remote/RemoteSessionsView.swift` | 3 处 `loggedAPIClient()`；`CrewCenterView:126` 是唯一挂载点 |
  | `Sources/Mac/Services/CrewRelayAgent.swift` | 2 处 `loggedAPIClient()` + 3 处 `PendingCrewAPI` 形参（`pull` / `push` / hub 订阅）；`imageAuth` 作绑定判据 |
  | `Sources/Mac/LocalRunner/SessionPermissionRelay.swift` | 它持有的 `SessionProxyClient`（`CrewSessionRunner:245` 那条 `ensurePermissionRelay`） |
  | `Sources/Mac/Services/CrewMailboxWaker.swift` | 整个类（`private let api: PendingCrewAPI` + `CrewRealtimeClient`） |
  | `Sources/Mac/Services/CrewSessionRunner.swift` | 2 处 `PendingCrewAPI` 形参（`ensurePermissionRelay:242` / `ensureMailboxWaker:981`） |
  | `Sources/Services/PendingCrewAPI.swift` | 整个 751 行远端 HTTP 客户端 |
  | `Sources/Services/PendingCrewBackend.swift` | `EdgeBackend`（109–240 行）。**`PendingCrewBackend` 协议本身和 `LocalBackend` 是活的，别连坐** |
  | `Sources/Services/CrewRealtimeClient.swift` | 整个 actor（唯二消费者 `EdgeBackend.whiteboardChanges` / `CrewMailboxWaker` 都在本表上） |
  | `Sources/Stores/AppModel.swift` | 凭据层整片：`credential` / `currentUserId` / `isAuthenticated` / `isConfigured`（**已无消费者**）/ `saveDeviceGrantToken` / `clearAuth` / `saveFamilyCredential` / `familySSOAvailable` / `tryFamilySSO`（**已无调用方**）/ `imageAuth` / `loggedAPIClient()` / `ensureRunnerHost` / `edgeBackend` / `apiBaseURL` |
  | `Sources/Stores/CrewStore.swift` | 3 处 `loggedAPIClient()`（169 / 203 / 222） |
  | `Sources/Mac/Views/CrewDetailInspector.swift` | 5 处 `loggedAPIClient()` |
  | `Sources/Mac/Views/CrewSessionWindowView.swift` | 2 处 `loggedAPIClient()` + `ensureRunnerHost` + `prepareServerSession` |
  | `Sources/Mac/Views/CrewChatView.swift` | 2 处 `loggedAPIClient()`（edge 附件上传那条路） |
  | `Sources/Support/KeychainStore.swift` / `FamilyCredentialStore.swift` | 只服务上面的凭据层（`LocalDataReset` 里的清理调用要跟着一起看） |
  | `Sources/Chat/Shims/ServerImage.swift`、`Chat/Vendored/BubbleView.swift`、`Mac/Support/CrewImageLoader.swift` | **文件是活的，只有 `imageAuth` 那条远端分支死了** —— 本地附件路径还在用它们，第二期只切分支不删文件 |

- **另外两件跟着这一刀走的**:
  - **iOS 是空壳**：`isConfigured` 在 iOS 上是 `credential != nil`，登录入口删完后恒 false，`AppModel.backend` 恒 nil。iPhone/iPad 直进 `IPadShell` 但拿不到任何数据。**这是 #63 时上级明确接受的已知代价**（iOS 端至今 0 构建、0 分发），不是删漏。第二期或本地后端跨平台时一起收。
  - **上面那条 🔴「`serverLink` 写死 nil」**（信箱唤醒 / 审批中继从未接通）现在是本条的子集 —— 它描述的两个服务整个在本表上。第二期端掉这层时那条一并结掉。
- **别做的**: 不要用 `#if false` / 注释掉 / 留空 stub 来「先关掉」这层。那会把一种安静的死换成另一种。要么整块删，要么原样留着等第二期。

### 🔴 登录态 session 的信箱唤醒与审批中继**从未接通** —— `serverLink` 写死 nil
- **发现**: 2026-08-19 · 前后端分离 P0（所有权归拢）
- **位置**: `Sources/Mac/Views/CrewSessionWindowView.swift` 手动起 session 那条路里的 `let serverLink: CrewSessionServerLink? = nil`；实现在 `Sources/Mac/Services/CrewSessionRunner.swift` 的 `ensureMailboxWaker` / `ensurePermissionRelay`。
- **问题**: 这两个服务原本由视图在 run 起好后接线，两个调用点**都在死路上** —— 一条被上面那个写死的 `nil` 挡着，另一条在被 `edgeQueueBindingReady == false` 关着的 auto-claim 死循环里。也就是说它们**一次都没被调用过**。调研清单（`docs/internal/2026-08-19-backend-split-inventory.md` A19/A20）当时的判断是「右栏没打开过的 session 才没接」，实际比这更糟：**所有 session 都没接**。
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
- **为什么结掉（2026-08-25 · Todo #63）**: PendingCrew 不再登录到任何地方，登录入口整块删了 —— **没有任何路径会再往钥匙串写登录态**，这条债咬不到人了。`KeychainStore` / `FamilyCredentialStore` 代码还在（跟凭据层一起等第二期），但已无调用方，见上面那条「安静的死」名单。签名默认值本身（ad-hoc）不变，也不需要改。
- **什么情况下要复活这条**: 哪天 PendingCrew 又要存跨启动的凭据，这条原样有效，别重新踩一遍。
- **发版不受影响**: `scripts/release/build-macos-update.sh` 在 xcodebuild 命令行上显式传 `CODE_SIGN_STYLE=Automatic DEVELOPMENT_TEAM=…`，命令行优先级最高，Developer ID 分发路径与这里的默认值无关。

### ✅ 没有 CI（**已还，2026-08-21**）
- **发现**: 2026-08-20 · 技术栈梳理（只读盘点）
- **位置**: `.github/` 下只有 `ISSUE_TEMPLATE/` 与 `pull_request_template.md`，**没有 `workflows/`**；仓库根也没有 Makefile / justfile / pre-commit。
- **问题**: `CONTRIBUTING.md:22-49` 把三件事定成硬规矩 ——「改了 `project.yml`（含新增 Swift 文件）必须 `xcodegen` 并提交 `.pbxproj`」「三端都要编一遍」「跑测试」—— 但没有任何自动化在 PR 上核这三条。其中第一条**已经踩过并且症状是「只有别人的机器编不过」**（`CONTRIBUTING.md:31-33` 自己写着「这条真的踩过」）：提交者本机 Xcode 会自动发现新文件，所以他永远看不到红。
- **为什么现在要记**: 之前仓库只有作者一个人、一台机器，靠纪律够用。开源之后进来的每个 PR 都是「另一台机器」，而这正是这条规矩失效时唯一会暴露的场景。
- **代价转嫁到哪**: 维护者的人工 review。pbxproj 漂移与 iOS 端静默打红这两类问题都不会在 PR 页面上显形，只能靠维护者自己 checkout 下来跑三条命令。
- **该怎么还**: 一个 macOS runner 上的 workflow，三步即可覆盖：`xcodegen && git diff --exit-code PendingCrew.xcodeproj/project.pbxproj`（抓漏 regen）、macOS build + test、iOS Simulator build。测试跑满约 3 分钟（2026-08-20 本机实测 184s / 1443 tests）。**注意**：`CrewChatOpenCostTests` 在 CI 上会 skip（fixture 不入 git），这是预期的，别为了让它绿而把 fixture 提交进去。
- **已完成（`.github/workflows/ci.yml`）**: 两个 job —— ①「pbxproj 与 `project.yml` 同步」重跑 xcodegen 后比 diff；② 三端编译 + 单测。落地过程中还顺带查出并根治了一个真问题：**仓库根目录的 xcconfig 会让 `xcodegen` 输出不确定**（`project.pbxproj` 里那条文件引用的 uuid 每次 regen 都变），那道 diff 检查因此永远红 —— 修法是把 xcconfig 挪进 `Config/`（`4dc855a`）。
- **一处措辞更正**: 上面「测试跑满约 3 分钟」是**本机热构建的测试执行时间**，不是 CI 的墙钟。CI 是冷机，要连 SPM 解析和两轮全量编译一起算。别拿本机数字当 CI 预算。

### 🟢 `docs/architecture.md` 的依赖章节被 #63 打成过时，本次**没改**
- **发现**: 2026-08-25 · Todo #63
- **问题**: #63 删掉了 `GoogleSignIn` 和 `Supabase` 两个直接依赖（`Package.resolved` 少了 16 个包），但 `docs/architecture.md` 里整节「依赖构成」还在按旧事实写：第 165 / 172–173 / 180–182 / 241–245 / 250–266 / 283 行讲「两个直接依赖」「GoogleSignIn 独自拖进 7 个传递包」「云端登录那两族占 65% 的 pin / 76% 的模块」——**这些数字现在全部为 0**。另有 35 / 261 / 737 / 859 行引用已删的 `CrewHostedConfig.swift` 与 `CrewHostedConfigTests`，341 行的 `Sources/Services/` 说明还写着「Supabase 栈」。
- **为什么没改**: #63 的边界由上级机长划死，要动的文档只点了 `README.md` 和本文件。`architecture.md` 是作者手写的长文，逐节重写属于扩范围 —— 但留着不说就成了「文档谎报」，所以登记在这里。
- **该怎么还**: 重跑一遍那节的实测（`Package.resolved` 的 pin 数、编译出的模块数、`.o` 体积），把「云端登录那两族」整节删掉或改写成「已随 #63 移除」，并清掉 `CrewHostedConfig` 的四处引用。**别照抄旧数字改个百分比** —— 那节的价值就在于它是实测出来的。

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

### 🟡 「加载更早」的位置补偿只验到了机制，**真窗口里的观感一次都没看过**；而且容器身份翻面被原样留着
- **发现**: 2026-08-23 · 人类 Todo #60（加载更早要保持阅读位置）
- **位置**: `Sources/Mac/Views/CrewChatView.swift` 的 `expandEarlier` / `ChatScrollAnchor` / 那个 `Group { if usesEagerInitialLayout { VStack } else { LazyVStack } }`；判定在 `Sources/Chat/Adapter/CrewChatWindow.swift` 的 `anchorOnExpand`。
- **本次做了什么**: 展开前记住当时窗口最顶那条，展开后一次确定性主线程 hop 里 `scrollTo(它, anchor: .top)`、不加动画。离屏窗口探针（`CrewChatExpandAnchorProbeTests`）量到：不补偿的对照跳 **680pt / 683pt**，补偿后位移 **14pt / 1pt**；展开同一拍来新消息时 `contentOffset` 与静态那一趟一模一样（视口没被下面新长出来的那条顶走）。

#### 尾巴一：人类**下次装完更新后第一件事**要照着念的四条清单

在一个历史够长（能连点三四次「加载更早」）的 crew 里，往上翻到看得见顶部那条按钮，然后：

1. **第一次点击**（12→24）—— 这一下同时撞容器身份翻面，看眼前那条有没有留在原地。
2. **连点两次**（第二次是 24→36，纯 `LazyVStack`）—— 同样看位置。
3. **展开的同一拍正好来一条新消息**（挑一个有 session 在跑的 crew）—— 位置要按住，且不许被新消息顶到底部去。
4. **点完静置两秒**，别动鼠标 —— 上面那些懒行的**真实高度陆续回填**之后，锚点漂没漂。

**第 4 条是四条里唯一探针覆盖不到的一条，也是风险最高的一条。** 探针用的是等宽定高的占位行，`LazyVStack` 把估算高度换成真实高度那条路它根本走不到；而那条路正是 Todo #54 / #56 两次踩坑的地方。前三条只要探针还绿就大概率是绿的，**人类只有一次注意力，请优先花在第 4 条上**。

#### 尾巴二：**未走的路** —— 先消灭容器身份翻面，再用 anchor 保位置

记录纠正（2026-08-23，机长指出）：第一版注释与 commit 说明把「不选 anchor」写成了「anchor 不可行」，**那不是事实**。事实是：

- 落地的选择是 `scrollTo`，**代价是容器身份翻面（`limit <= pageSize` 判据在 12→24 那一下翻面、整棵内容树重建）被原样保留了下来**。
- anchor 那条路（这一拍把 `.sizeChanges` 的锚临时翻成 `.bottom`）**没有连同「先用单独一笔提交消灭翻面」一起评估过**。「anchor 救不了翻面」这个理由是在「翻面还在」的前提下说的 —— 前提本可以先被消灭，所以它**不构成对 anchor 的证伪**。
- 解冻判据也不需要新造：`isFollowing` 就是现成的那一个（跟随底部时边界照常漂、用户滚上去期间冻结、回到底部再同步）。

**这条路的收益正好压在上面第 4 条那个风险上**：内容在上面长、锚底部 = 视口内容一像素不动，**对懒容器估算高度回填天然免疫**（上面那些行真实高度陆续回填时也一样锚底部）。而落地的 `scrollTo` 这条**不免疫** —— 它只在点击那一拍把位置钉一次，之后高度再变就再没人管。

所以：**第 4 条要是真出了问题，正确的方向是走这条未走的路（先消灭翻面 + anchor），不是给 `scrollTo` 加补丁。** 别被「anchor 试过了不行」误导 —— 它没被试过。

#### 尾巴三：一个**从未被定位**的红，别当它已经被治好了

某次全量跑里见过 **1 个 failure**，把探针改成「等几何量连续 25 拍不变」之后就没再复现过 —— **但它具体是哪条用例、为什么红，从头到尾没有定位到**。

所以：**这一族将来再冒红，不许默认已经被 `788fca9` 治好。** 没抓住的红不算被治好，只算没再出现。

### 🟡 机长作战板对人类只读 —— 是**刻意推迟**，不是没想到
- **发现**: 2026-08-25 · 人类 Todo #66 A 段（`CockpitPlanStore` / `plan_*` 三个 MCP 工具）
- **位置**: `Sources/Stores/CockpitPlanStore.swift`（唯一写入口在 `Sources/Mcp/McpServer.swift` 的 `guard isCaptain` 后面）。
- **现状**: 第六本账「机长作战板」**只有机长写得动**：人类在驾驶舱里能看，不能改、不能追问、不能翻状态。这是按规格做的 —— 这本账的定位就是「机长自己整理的」，与人类 Todo 那两本（人类写 / agent 写）方向不同。
- **为什么记在这**: 人看到一条写错的计划必然想动手，而**「不能改」和「不知道怎么改」是两回事**。UI 上必须明说怎么让它改（一句「让机长改」或者把这条带进群聊输入框的入口），否则这块板在人眼里就是死的 —— 这是 B 段（第三个药丸 + 面板）必须带上的一条，不是可选装饰。
- **推迟的是什么**: 双向编辑（人类直接改/追问机长的计划）。第一版不做，理由是别在方向都没跑顺之前就把它做成双向；不是没想到。
- **要动的时候怎么动**: 人类那一侧照 `LocalTodoStore` 的 `followUp` 语义走（追加式、不覆盖、任何状态都能追问），别新造第二套编排。
