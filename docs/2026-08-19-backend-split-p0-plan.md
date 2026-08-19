# P0 · 所有权归拢 实施计划

> **For agentic workers:** 用 `superpowers:executing-plans` 逐任务执行。步骤是 `- [ ]` checkbox，做一条勾一条。

**Goal:** 把当前挂在 SwiftUI 视图上的所有长期职责（编排器、中继、三个唤醒器、用量监视、两个轮询中心）交给一个 app 级的单一所有者 `SessionHost`，并加一道运行期硬闸防止将来有人又把它们挂回视图上。

**Architecture:** 新增 `ProcessRole`（进程角色，启动时算一次）与 `SessionHost`（长期职责的唯一所有者，app 级持有）。`MacRootView` 里那一串把 `CrewStore` 的请求数组泵给 `CrewSessionRunner` 的 `.onChange` 修饰符，原样搬成 `SessionHost` 内部的 Combine 订阅。视图从「创建者」退化成「观察者」。

**Tech Stack:** Swift / SwiftUI / Combine / XcodeGen / XCTest

**Spec:** `docs/2026-08-19-backend-split-design.md`（本计划实现其 §9 的 P0 行；§6.2 闸门 1 在本阶段落地）

**调研清单（必读）:** `docs/2026-08-19-backend-split-inventory.md` —— 23 条界面持有的后台职责、30 条定时器、逐条带文件行号。**开工前整份读一遍**，本计划的任务边界是照它划的。

## Global Constraints

- 编译用 `PendingCrew.xcodeproj` / scheme `PendingCrew`（**不是** PendingBot）。
- **新增 Swift 文件必须改 `project.yml` 并重跑 `xcodegen`，把重生的 `PendingCrew.xcodeproj/project.pbxproj` 一起提交**，否则新 worktree 编不过。
- 单测 bundle 是 **standalone**：`Sources/Mac/LocalRunner/` 整目录已编进 bundle，`Sources/Mac/Services/` **没有**。要被单测直接测的类型放 `LocalRunner/`。
- **本阶段行为零变化。** P0 是纯搬家：不改任何编排逻辑、不改任何文案、不改任何时序。任何「顺手优化」都不要做。
- 小单元 auto-commit。**不要 `git push`。**
- 落 main 前跑全量测试，基线 **1428 条 0 失败**（只许多，不许少，不许有失败）。
- 三端都要编一遍（macOS build / iOS Simulator build / macOS test）——只编 Mac 会让漏 `#if os(macOS)` 的 AppKit 调用把 iOS 端静默打红。
- 不许驱动图形界面（osascript / cliclick / screencapture / 起会开窗口的程序）。验证只走命令行 + 单测。

命令：

```bash
xcodegen
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' test
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'generic/platform=iOS Simulator' build
```

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `Sources/Mac/LocalRunner/ProcessRole.swift`（**新建**） | 纯判定：由 argv + 总闸算出本进程角色。放 LocalRunner 是为了能被单测直接编进 bundle。 |
| `Sources/Mac/Services/SessionHost.swift`（**新建**） | 长期职责的唯一所有者：持有 runner / relay / usage，启动两个轮询中心与三个唤醒器，并用 Combine 承接 `CrewStore` 的请求数组。 |
| `Sources/PendingCrewApp.swift`（改） | 在 app 级 `@StateObject` 持有 `SessionHost`，注入环境。 |
| `Sources/Mac/Views/MacRootView.swift`（改） | 删掉 3 个 `@StateObject`/局部创建、11 个编排 `.onChange`、整个 `.task`。改成从环境取 `SessionHost` 并只观察。 |
| `Sources/Mac/Views/CrewSidebarView.swift`（改） | 删掉 `usageMonitor` 的 `@StateObject` 与 `.task { quota.start(); ModelCatalogCenter.shared.start() }`，改成观察。 |
| `Sources/Mac/Services/QuotaCenter.swift` 等 6 个服务（改） | 各自 `start()` 首行加 `ProcessRole` 断言。 |
| `Tests/PendingCrewTests/ProcessRoleTests.swift`（**新建**） | `ProcessRole.resolve` 的判定表。 |
| `Sources/Mac/Views/CrewSessionWindowView.swift`（改） | 交出每 session 的信箱唤醒器 / 审批中继接线；删掉被关着的 4s 死循环（Task 3b）。 |

---

### Task 1: ProcessRole —— 进程角色判定

**Files:**
- Create: `Sources/Mac/LocalRunner/ProcessRole.swift`
- Test: `Tests/PendingCrewTests/ProcessRoleTests.swift`
- Modify: `project.yml`（测试 target 不用改，`Sources/Mac/LocalRunner` 整目录已在；**app target 也不用改**，`Sources/Mac/LocalRunner` 同样整目录在 —— 先确认这两条再动手，确认方式见 Step 0）

**Interfaces:**
- Produces:
  - `enum ProcessRole: String { case orchestrator, viewer, helper }`
  - `static func resolve(argv: [String], backendFlag: String?) -> ProcessRole`
  - `static var current: ProcessRole`（进程内只算一次）

- [x] **Step 0: 确认 LocalRunner 目录在两个 target 里都是整目录纳入**

```bash
grep -n "Sources/Mac/LocalRunner" project.yml
```

预期：两处（app target 一处、test target 一处）。**如果只有一处，把缺的那处补上再继续**，否则新文件不会被编译，后面所有步骤都会以奇怪的方式失败。

- [x] **Step 1: 写失败的测试**

新建 `Tests/PendingCrewTests/ProcessRoleTests.swift`：

```swift
#if os(macOS)
import XCTest

/// `ProcessRole` 判定表（前后端分离 P0，spec §6.2 闸门 1）。
///
/// 这条判定是「谁有资格跑长期定时器」的唯一真值 —— 判错的后果是双头
/// （两个进程各跑一套唤醒器往同一批账上写），所以每条分支都钉死。
final class ProcessRoleTests: XCTestCase {

    func testHelperArgvWinsOverEverything() {
        // helper 是短命子进程，无论总闸怎么设都不是编排者。
        for flag in [nil, "inproc", "daemon"] as [String?] {
            XCTAssertEqual(
                ProcessRole.resolve(argv: ["PendingCrew", "--mcp-serve", "--crew", "c1"],
                                    backendFlag: flag),
                .helper, "flag=\(String(describing: flag))")
        }
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew", "--mcp-hook"], backendFlag: nil), .helper)
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew", "--mcp-permission-hook"], backendFlag: nil),
            .helper)
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew", "--mcp-turn-hook"], backendFlag: nil), .helper)
    }

    func testDaemonArgvIsOrchestrator() {
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew", "--daemon"], backendFlag: nil), .orchestrator)
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew", "--daemon"], backendFlag: "daemon"),
            .orchestrator)
    }

    func testGuiIsOrchestratorWhenBackendFlagAbsentOrInproc() {
        // P0~P3 期间总闸没设 / 设成 inproc —— GUI 进程**就是**所有者。
        XCTAssertEqual(ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: nil), .orchestrator)
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: "inproc"), .orchestrator)
        // 不认识的值按 inproc 兜底：写错环境变量不该让 app 变成没人管账的空壳。
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: "banana"), .orchestrator)
    }

    func testGuiIsViewerWhenBackendFlagIsDaemon() {
        XCTAssertEqual(ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: "daemon"), .viewer)
    }

    func testFlagIsCaseInsensitiveAndTrimmed() {
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: " Daemon "), .viewer)
    }
}
#endif
```

- [x] **Step 2: 跑测试确认它失败**

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' test -only-testing:PendingCrewTests/ProcessRoleTests 2>&1 | tail -20
```

预期：编译失败，`cannot find 'ProcessRole' in scope`。

- [x] **Step 3: 写最小实现**

新建 `Sources/Mac/LocalRunner/ProcessRole.swift`：

```swift
#if os(macOS)
import Foundation

/// 本进程在「前后端分离」里扮演的角色（spec `docs/2026-08-19-backend-split-design.md` §6.2）。
///
/// 存在的理由只有一个：**防双头**。同一批共享账本（白板/Todo/账本）和同一批长期
/// 定时器（唤醒器/中继/额度轮询）必须只有一个所有者。这个枚举把「我有没有资格
/// 跑这些」变成一条可断言的事实，而不是靠每个人自觉。
///
/// 判定在进程启动时算一次、之后只读 —— 中途不切（切了就等于中途换所有者）。
enum ProcessRole: String {
    /// 长期职责的所有者：跑定时器、写编排性账目、养 agent 子进程。
    case orchestrator
    /// 只看不管：连上去显示、发指令，不持有任何长期定时器。
    case viewer
    /// MCP helper 短命子进程（跑完即退）。直写共享账本是它的正常工作，
    /// 但它不构成「第二个编排者」—— 它不长期存活、不持有定时器。
    case helper

    /// 总闸环境变量名。`inproc`（默认）= GUI 进程自己就是所有者；
    /// `daemon` = 所有权在常驻后台进程，GUI 退化成 viewer。
    static let backendEnvKey = "PENDINGCREW_BACKEND"

    /// 纯判定（可单测）。优先级：helper argv > daemon argv > 总闸。
    ///
    /// 兜底选 `.orchestrator` 而不是 `.viewer`：总闸拼错时，「没人管账」比
    /// 「两个人管账」更难发现 —— 唤醒器全不跑、session 静静地没人叫醒，
    /// 而那正是我们最怕的静默失效。
    static func resolve(argv: [String], backendFlag: String?) -> ProcessRole {
        let helperFlags = ["--mcp-serve", "--mcp-hook", "--mcp-permission-hook", "--mcp-turn-hook"]
        if argv.contains(where: helperFlags.contains) { return .helper }
        if argv.contains("--daemon") { return .orchestrator }
        let flag = (backendFlag ?? "").trimmingCharacters(in: .whitespaces).lowercased()
        return flag == "daemon" ? .viewer : .orchestrator
    }

    /// 本进程的角色。第一次取用时算一次，之后固定。
    static let current: ProcessRole = resolve(
        argv: CommandLine.arguments,
        backendFlag: ProcessInfo.processInfo.environment[backendEnvKey])
}
#endif
```

- [x] **Step 4: 跑测试确认它通过**

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' test -only-testing:PendingCrewTests/ProcessRoleTests 2>&1 | tail -20
```

预期：`Test Suite 'ProcessRoleTests' passed`，5 条全过。

- [x] **Step 5: 重生工程并提交**

```bash
xcodegen
git add project.yml PendingCrew.xcodeproj/project.pbxproj \
        Sources/Mac/LocalRunner/ProcessRole.swift \
        Tests/PendingCrewTests/ProcessRoleTests.swift
git commit -m "feat(split): ProcessRole —— 防双头的进程角色判定

前后端分离 P0 第一块。判定优先级 helper argv > daemon argv > 总闸;
总闸拼错时兜底选 orchestrator —— 「没人管账」比「两个人管账」更难发现。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

（若 `xcodegen` 没有改动 pbxproj，`git add` 那一项会是空操作，正常。）

---

### Task 2: SessionHost 立起来，接管三个视图创建的对象

**Files:**
- Create: `Sources/Mac/Services/SessionHost.swift`
- Modify: `Sources/PendingCrewApp.swift`
- Modify: `Sources/Mac/Views/MacRootView.swift:21,26`（删 `@StateObject`）
- Modify: `Sources/Mac/Views/CrewSidebarView.swift:28`（删 `@StateObject`）
- Modify: `project.yml`（确认 `Sources/Mac/Services` 在 app target；本任务**不**加进 test target）

**Interfaces:**
- Consumes: `ProcessRole.current`（Task 1）
- Produces:
  - `@MainActor final class SessionHost: ObservableObject`
  - `var runner: CrewSessionRunner`
  - `var relay: CrewRelayAgent`
  - `var usage: LocalAgentUsageMonitor`
  - `func start(model: AppModel, crewStore: CrewStore)`（幂等）

- [x] **Step 1: 建 SessionHost（只搬持有权，先不搬 .onChange）**

新建 `Sources/Mac/Services/SessionHost.swift`：

```swift
#if os(macOS)
import Foundation
import Combine

/// **长期职责的唯一所有者**（spec `docs/2026-08-19-backend-split-design.md` §6）。
///
/// 在这个类型出现之前，编排器 / 云端中继 / 三个唤醒器 / 用量监视 / 两个轮询中心
/// 是随 `MacThreePaneView` 和 `CrewSidebarView` 两个**视图**一起生出来的 ——
/// 这就是「关掉 app 就全停」的根，也是把 session 搬进常驻后台进程时最先撞上的墙。
///
/// 现在它们都归这里。视图退化成观察者：只读 `@Published`，不创建、不启动。
///
/// P0 阶段这个类还活在 GUI 进程里（`ProcessRole.current == .orchestrator`）；
/// P4 之后同一个类原样跑在 `--daemon` 进程里，GUI 侧变成 `.viewer` 不再持有它。
/// **所以这里不许出现任何 SwiftUI / AppKit 依赖** —— 它将来要在没有画面的进程里跑。
@MainActor
final class SessionHost: ObservableObject {
    let runner: CrewSessionRunner
    let relay: CrewRelayAgent
    let usage: LocalAgentUsageMonitor

    private var bag = Set<AnyCancellable>()
    private var started = false

    init(runner: CrewSessionRunner = CrewSessionRunner(),
         relay: CrewRelayAgent = CrewRelayAgent(),
         usage: LocalAgentUsageMonitor = LocalAgentUsageMonitor()) {
        self.runner = runner
        self.relay = relay
        self.usage = usage
    }

    /// 启动全部长期职责。**幂等** —— 重复调用是 no-op（SwiftUI 的 `.task` 会因
    /// 视图重挂而重跑，这在切 crew 时是常态）。
    ///
    /// 第一行的断言是 spec §6.2 的闸门 1：viewer 进程里误起一套定时器 = 当场崩，
    /// 不是悄悄跑起来变成双头。双头的症状（账被两个进程交替覆盖、唤醒发两遍）
    /// 事后极难定位，所以宁可在这里响。
    func start(model: AppModel, crewStore: CrewStore) {
        precondition(
            ProcessRole.current == .orchestrator,
            "SessionHost.start 只能在编排者进程里调用，当前角色=\(ProcessRole.current.rawValue)")
        guard !started else { return }
        started = true

        // app 重启后重挂持久化的定时唤醒（schedule_wakeup 不因重启失约）。
        runner.rearmWakeups()
        // 成员状态快照定时器（机长 list_sessions 的数据源）。
        runner.startSessionsSnapshotTimer()
        // 本地 mention 唤醒器：session/机长 post_to_crew 的定向 @ → 注入 idle run /
        // 拉起缺席目标。幂等。
        if runner.localMentionWaker == nil {
            let waker = CrewLocalMentionWaker(
                runner: runner, backendProvider: { [weak model] in model?.backend })
            runner.localMentionWaker = waker
            waker.start()
        }
        // 额度中心 + 可用模型表中心（都是幂等启动、都要落文件给 helper 读）。
        QuotaCenter.shared.start()
        ModelCatalogCenter.shared.start()
        // relay 同步代理常开（幂等启动）；未登录时 tick 是 no-op。
        relay.start(appModel: model, sessionRunner: runner)

        wire(crewStore: crewStore, model: model)
    }

    /// 承接 `CrewStore` 排空共享控制文件后发布的请求数组。
    /// Task 3 填充；先留空让 Task 2 可以单独编译通过。
    private func wire(crewStore: CrewStore, model: AppModel) {}
}
#endif
```

- [x] **Step 2: app 级持有并注入**

`Sources/PendingCrewApp.swift`：在 `@StateObject private var crewStore: CrewStore` 旁边加

```swift
    #if os(macOS)
    /// 长期职责的唯一所有者（spec §6）。**必须挂在 App 上而不是任何视图上** ——
    /// 挂视图上就会随视图生灭，那正是我们要修的病。
    @StateObject private var sessionHost = SessionHost()
    #endif
```

并在注入 `crewStore` 的同一处 `.environmentObject(crewStore)` 后面补：

```swift
    #if os(macOS)
        .environmentObject(sessionHost)
    #endif
```

> 动手前先 `grep -n "environmentObject(crewStore)" Sources/PendingCrewApp.swift` 找到全部注入点，**每一处都要补**（漏一处会在运行时崩在 `@EnvironmentObject` 找不到）。

- [x] **Step 3: 三个视图改成观察者**

`Sources/Mac/Views/MacRootView.swift`：

```swift
// 删除这两行：
//     @StateObject private var sessionRunner = CrewSessionRunner()
//     @StateObject private var relayAgent = CrewRelayAgent()
// 换成：
    @EnvironmentObject private var sessionHost: SessionHost
    private var sessionRunner: CrewSessionRunner { sessionHost.runner }
```

`relayAgent` 在本文件只有 `.task` 里 `relayAgent.start(...)` 一处用到，那一处随 Task 3 一起删，所以这里直接删掉属性即可。

`Sources/Mac/Views/CrewSidebarView.swift:28`：

```swift
// 删除：@StateObject private var usageMonitor = LocalAgentUsageMonitor()
// 换成：
    @EnvironmentObject private var sessionHost: SessionHost
    private var usageMonitor: LocalAgentUsageMonitor { sessionHost.usage }
```

并删掉 `:65` 的 `.task { quota.start(); ModelCatalogCenter.shared.start() }` 整行（两者已由 `SessionHost.start` 负责）。

> `CrewSidebarView` 里 `usageMonitor` 若有 `@ObservedObject` 语义需求（它的 `@Published` 变化要刷新视图），把 `private var usageMonitor` 保留为计算属性即可 —— `sessionHost` 本身是 `@EnvironmentObject`，但 `usage` 的变化不会经由它传播。**所以这里改成：**
> ```swift
>     @EnvironmentObject private var sessionHost: SessionHost
>     // 计算属性拿不到刷新，用 @ObservedObject 包一层才会随它的 @Published 重绘。
> ```
> 具体做法：在 `body` 里用 `UsageObserver(monitor: sessionHost.usage) { … }` 这类小包装，或者把 `SessionHost` 里的 `usage` 改成 `@Published private(set) var`。**选后者，改动最小**：
> ```swift
>     // SessionHost 里
>     let usage: LocalAgentUsageMonitor
>     // 视图里
>     @EnvironmentObject private var sessionHost: SessionHost
>     ...
>     UsageFooter(monitor: sessionHost.usage)   // 一个 @ObservedObject 的小 View
> ```
> **动手前先 `grep -n "usageMonitor" Sources/Mac/Views/CrewSidebarView.swift` 看它到底被怎么用**：如果只在 `Text(LocalAgentUsageMonitor.formatTokens(n))` 这种静态方法上用（`:378`、`:385` 看起来就是），那它根本不需要重绘订阅，直接计算属性即可，上面这一整段顾虑作废。**以 grep 结果为准。**

- [x] **Step 4: 编译三端**

```bash
xcodegen
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build 2>&1 | tail -5
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'generic/platform=iOS Simulator' build 2>&1 | tail -5
```

预期：两条都 `BUILD SUCCEEDED`。

此时 app 的行为已经**坏了一半**（`.task` 里的启动还在 MacRootView 里、`.onChange` 还在），Task 3 会补齐。**所以本任务不单独运行 app 验证，只要求编译过。**

- [x] **Step 5: 提交**

```bash
git add project.yml PendingCrew.xcodeproj/project.pbxproj \
        Sources/Mac/Services/SessionHost.swift Sources/PendingCrewApp.swift \
        Sources/Mac/Views/MacRootView.swift Sources/Mac/Views/CrewSidebarView.swift
git commit -m "refactor(split): SessionHost 接管视图创建的长期对象

编排器/中继/用量监视从 MacThreePaneView 与 CrewSidebarView 的 @StateObject
搬到 app 级 SessionHost;两个轮询中心的启动一并收走。start() 首行断言
ProcessRole == orchestrator —— 双头的症状事后极难定位,宁可在这里当场响。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: 把 11 个编排 `.onChange` 搬进 SessionHost

**Files:**
- Modify: `Sources/Mac/Services/SessionHost.swift`（填充 `wire`）
- Modify: `Sources/Mac/Views/MacRootView.swift`（删掉搬走的修饰符与整个 `.task`）

**Interfaces:**
- Consumes: `SessionHost.wire(crewStore:model:)`（Task 2 留的空方法）
- Produces: 无新公开接口

**要搬的 11 个（`MacRootView.swift` 现状）**：`captainAutostartRequests`、`sessionSpawnRequests`、`profileChangeRequests`、`wakeupRequests`、`sessionOpsRequests`、`workdirChangeRequests`、`listenRequests`、`crewMessageWakes`、`quotaCenter.claude`、`quotaCenter.codex`，外加整个 `.task { … }` 里的启动序列（已在 Task 2 搬进 `start()`）。

**留在视图里不动的**：`.onChange(of: crewStore.selectedCrewId)`（`switchCrew` 是**界面选中态**，属于视图）、`.background(WindowSeparatorRemover(...))`、`.onAppear { AppUpdater.shared.isBusy = … }`（后者见 Step 4）。

- [x] **Step 1: 关键陷阱先读一遍（否则会写出难查的 bug）**

`.onChange(of:)` 在**值已经写入之后**触发；而 `@Published` 的 publisher 是 **willSet** 语义 —— 在赋值**之前**发。所以直接 `crewStore.$sessionSpawnRequests.sink { … }` 里再去读/写 `crewStore.sessionSpawnRequests` 会读到旧值、写入会被随后的赋值覆盖。

**统一解法：所有订阅都加 `.receive(on: DispatchQueue.main)`**，把回调推到下一个 runloop 转，那时值已落定，语义与 `.onChange` 一致。这条不是优化，是正确性前提。

- [x] **Step 2: 填充 `wire`**

`SessionHost.swift` 里把空的 `wire` 换成（**逐条对照 `MacRootView.swift` 原文照搬，注释一起搬走**，不要重写逻辑）：

```swift
    /// 承接 `CrewStore` 排空共享控制文件后发布的请求数组。
    ///
    /// 这些订阅在此之前是 `MacThreePaneView` 上的一串 `.onChange` 修饰符 ——
    /// 也就是说**编排逻辑长在界面上**。搬到这里是 P0 的主要工作量。
    ///
    /// ⚠️ 每条都必须 `.receive(on: DispatchQueue.main)`：`@Published` 是 willSet
    /// 语义（赋值**前**发），不推一拍就会读到旧值、且「读完清空」会被随后的赋值
    /// 盖掉。`.onChange` 是 didSet 语义，推一拍才对得上。
    private func wire(crewStore: CrewStore, model: AppModel) {
        let runner = self.runner

        // 建 crew 后自动起机长（新建即启动 + 群里报到，无需手动点「启动 Captain」）。
        crewStore.$captainAutostartRequests
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore, weak model] reqs in
                guard !reqs.isEmpty, let crewStore, let model else { return }
                crewStore.captainAutostartRequests = []
                Task { @MainActor in
                    // ……原样搬 MacRootView.swift 里那一整段循环体……
                }
            }
            .store(in: &bag)

        // 机长 start_session 命令排空后的待起 worker session 队列。
        crewStore.$sessionSpawnRequests
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore, weak model] reqs in
                guard !reqs.isEmpty, let crewStore, let model else { return }
                crewStore.sessionSpawnRequests = []
                Task { @MainActor in
                    // ……原样搬……
                }
            }
            .store(in: &bag)

        // 其余九条同构，逐条照搬：profileChangeRequests / wakeupRequests /
        // sessionOpsRequests / workdirChangeRequests / listenRequests /
        // crewMessageWakes / QuotaCenter.shared.$claude / $codex。
    }
```

**照搬纪律**：每一条的循环体、错误处理、fail-loud 落白板、`refreshDetail` 缓存 miss 兜底，全部一个字不改地搬过来，连注释一起。P0 是搬家不是重构 —— 任何改动都会让「行为零变化」这条验收失效。

额度那两条的原文是 `.onChange(of: quotaCenter.claude)`，搬过来对应：

```swift
        QuotaCenter.shared.$claude
            .receive(on: DispatchQueue.main)
            .sink { [weak runner] _ in runner?.broadcastQuotaWarningIfNeeded() }
            .store(in: &bag)
        QuotaCenter.shared.$codex
            .receive(on: DispatchQueue.main)
            .sink { [weak runner] _ in runner?.broadcastQuotaWarningIfNeeded() }
            .store(in: &bag)
```

- [x] **Step 3: 从 MacRootView 删掉搬走的部分**

删掉那 10 个 `.onChange` 与整个 `.task { … }`；保留 `.onChange(of: crewStore.selectedCrewId)`、`.background(WindowSeparatorRemover…)`。

`.task` 里还有两条**不属于编排**的：

```swift
            await crewStore.refreshList()
            await crewStore.refreshSubjects()
```

这两条是**界面首屏数据预取**，留在视图里，所以 `.task` 不整个删，缩成：

```swift
        .task {
            sessionHost.start(model: model, crewStore: crewStore)
            // 首次进入时把列表 + subjects 都拉一遍 —— subjects 用于创建 crew
            // sheet 的 picker，提前 prefetch 避免 sheet 打开时空。
            await crewStore.refreshList()
            await crewStore.refreshSubjects()
        }
```

- [x] **Step 4: `AppUpdater.shared.isBusy` 也搬进 SessionHost**

`MacRootView.swift` 的 `.onAppear` 里：

```swift
        AppUpdater.shared.isBusy = { [weak sessionRunner] in
            sessionRunner?.runs.contains { $0.status == .running } ?? false
        }
```

这是「有 session 在跑就别自动更新」的闸门 —— **它是编排职责，不是界面**（而且 P4 之后 app 更新恰恰**不该**再看这个，见 spec A1）。搬进 `SessionHost.start()` 末尾：

```swift
        // 有 session 在跑就别自动更新（P4 之后这条会随 A1 一起去掉 —— 那时更新
        // app 本就不打断后台的 session）。
        AppUpdater.shared.isBusy = { [weak runner] in
            runner?.runs.contains { $0.status == .running } ?? false
        }
```

并删掉视图里的 `.onAppear`。

- [x] **Step 5: 编译 + 全量测试**

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build 2>&1 | tail -5
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'generic/platform=iOS Simulator' build 2>&1 | tail -5
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' test 2>&1 | tail -30
```

预期：两条 build `BUILD SUCCEEDED`；test `TEST SUCCEEDED`，**执行条数 ≥ 1433**（原 1428 + ProcessRoleTests 的 5 条），**0 failures**。条数变少或有失败 → 停下来查，不要继续。

- [x] **Step 6: 提交**

```bash
git add Sources/Mac/Services/SessionHost.swift Sources/Mac/Views/MacRootView.swift
git commit -m "refactor(split): 编排 glue 从 SwiftUI .onChange 搬进 SessionHost

11 条 .onChange(派工/建 crew/起机长/切档/唤醒/收听/机长操作/改工作目录/
跨 crew 唤醒/额度警戒)与 .task 里的启动序列全部搬走,视图只留界面自己的事
(选中态、首屏预取、窗口 chrome)。

订阅一律 .receive(on: .main):@Published 是 willSet 语义,不推一拍就会读到
旧值、且「读完清空」会被随后的赋值盖掉 —— .onChange 是 didSet,推一拍才对得上。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3b: 把从视图接线的三处后台职责收走

调研清单（`docs/2026-08-19-backend-split-inventory.md` 清单 A19/A20、D4、A16/B12）查出来的，Task 3 那 11 条之外还漏在视图里的三处。

**Files:**
- Modify: `Sources/Mac/Views/CrewSessionWindowView.swift`
- Modify: `Sources/Mac/Views/MacRootView.swift`
- Modify: `Sources/Mac/Services/SessionHost.swift`

- [x] **Step 1: 每 session 的两个后台服务改由 runner 自己接线**

`CrewSessionWindowView.swift:882,1013,1027-1031`（`CrewMailboxWaker`）与 `:883,1016,1035-1039`（`SessionPermissionRelay`）现在是**视图在展示某个 session 时**才去 `ensureMailboxWaker` / `ensurePermissionRelay`。

后果：**右栏没打开过那个 session，它的信箱唤醒和审批中继就没被接上**。这不只是搬家问题，是今天就存在的隐患。

改法：把这两个 `ensure…` 的调用点从视图移到 `CrewSessionRunner` 里 run **启动成功之后**的那一拍（`start` / `startCaptain` / `startForBrief` 三条路汇合处）。两个 `ensure…` 本来就是幂等的，改完视图侧直接删掉调用。

> 动手前先读一遍这两处：`grep -n "ensureMailboxWaker\|ensurePermissionRelay" Sources/`，把参数（api / baseURL / token 从哪来）看清楚 —— 视图里能拿到的登录态，runner 里未必是同一条路径。**拿不到就在群里问，不要凭猜换来源。**

- [x] **Step 2: 视图不许再写共享账本**

`MacRootView.swift:168` 里视图直接调了 `LocalCrewControlStore.shared.writeCommandResponse(...)`（`change_workdir` 的回执）。这一整段随 Task 3 的 `workdirChangeRequests` 订阅一起搬进 `SessionHost` 后，视图里就不该再有这一行。

验证：

```bash
grep -rn "LocalCrewControlStore.shared.write\|LocalWhiteboardStore.shared.append" Sources/Mac/Views/
```

预期：`CrewTodoFollowUp.swift` 那几处是**用户在界面上的操作**（人点了追问/重开），属于界面职责，留着；除此之外应当为零。有别的就在汇报里点名。

- [x] **Step 3: 删掉那条被关着的死循环**

`CrewSessionWindowView.swift:949-968` 有一条 4s 的 edge queued-session auto-claim 轮询，被 `:952` 的 `edgeQueueBindingReady` 恒 `false` 关着 —— 死代码。**删掉整段**，别把它搬进后台。

删之前确认它真的恒假：

```bash
grep -n "edgeQueueBindingReady" Sources/
```

若发现别处会把它置真，**停手改为在群里报告**，不要删。

- [x] **Step 4: 编译 + 全量测试 + 提交**

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build 2>&1 | tail -5
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'generic/platform=iOS Simulator' build 2>&1 | tail -5
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' test 2>&1 | tail -30
```

```bash
git add Sources/Mac/Views/CrewSessionWindowView.swift Sources/Mac/Views/MacRootView.swift \
        Sources/Mac/Services/SessionHost.swift Sources/Mac/Services/CrewSessionRunner.swift
git commit -m "refactor(split): 收走三处从视图接线的后台职责

① 信箱唤醒器与审批中继本来由 CrewSessionWindowView 在展示 session 时才接上
   —— 右栏没打开过那个 session 它们就没被接上,今天就是隐患。改由 runner 在
   run 启动成功后自己接。
② MacRootView 直接写控制通道回应那一行随 workdirChange 订阅一起搬走,视图不
   再写共享账本。
③ 删掉被 edgeQueueBindingReady 恒 false 关着的 4s 死循环,别把它搬进后台。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: 给六个长期服务加防双头断言

**Files:**
- Modify: `Sources/Mac/Services/QuotaCenter.swift`（`start()`）
- Modify: `Sources/Mac/Services/ModelCatalogCenter.swift`（`start()`）
- Modify: `Sources/Mac/Services/CrewRelayAgent.swift`（`start(...)`）
- Modify: `Sources/Mac/Services/CrewLocalMentionWaker.swift`（`start()`）
- Modify: `Sources/Mac/Services/CrewMailboxWaker.swift`（`start()` 或等价的启动方法）
- Modify: `Sources/Mac/Services/LocalAgentUsageMonitor.swift`（若有启动方法）

**Interfaces:**
- Consumes: `ProcessRole.current`（Task 1）

- [x] **Step 1: 先把六处启动方法找出来**

```bash
grep -n "func start" Sources/Mac/Services/*.swift
```

把结果抄进任务笔记 —— 下面每一处都要加同一行。

- [x] **Step 2: 逐个加断言**

每个 `start`（或等价启动方法）的**第一行**加：

```swift
        precondition(
            ProcessRole.current == .orchestrator,
            "\(type(of: self)).start 只能在编排者进程里调用，当前角色=\(ProcessRole.current.rawValue)")
```

并在方法上方补一句注释：

```swift
    /// ⚠️ 只有编排者进程有资格起它（spec §6.2 闸门 1）。viewer 里误起 = 当场崩，
    /// 不是悄悄跑成双头 —— 双头会让同一批账被两个进程交替覆盖、唤醒发两遍，
    /// 而那种症状事后基本查不出来。
```

- [x] **Step 3: 编译 + 全量测试**

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build 2>&1 | tail -5
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' test 2>&1 | tail -30
```

预期：`BUILD SUCCEEDED` + `TEST SUCCEEDED`，0 failures。

⚠️ 如果**测试**因为这些 precondition 挂了，说明有单测在 `.helper` 或 `.viewer` 角色下起这些服务 —— 那是真发现，不是误报。停下来在群里报告，别把断言改松。

- [x] **Step 4: 提交**

```bash
git add Sources/Mac/Services/
git commit -m "feat(split): 六个长期服务加防双头断言

QuotaCenter/ModelCatalogCenter/CrewRelayAgent/两个唤醒器/用量监视的启动
方法首行断言 ProcessRole == orchestrator。viewer 里误起 = 当场崩,不是悄悄
跑成双头。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: 验收与回归证据

**Files:** 无（只跑验证）

- [x] **Step 1: 三端全绿**

```bash
xcodegen
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build 2>&1 | tail -3
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'generic/platform=iOS Simulator' build 2>&1 | tail -3
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' test 2>&1 | tail -30
```

把 test 的「Executed N tests, with 0 failures」原文抄进汇报。

- [x] **Step 2: 证明视图里已经没有长期对象了**

```bash
grep -n "@StateObject" Sources/Mac/Views/*.swift
```

预期：结果里**不含** `CrewSessionRunner` / `CrewRelayAgent` / `LocalAgentUsageMonitor`。若还有别的长期对象（调研清单 `docs/2026-08-19-backend-split-inventory.md` 的清单 A 会列全），在汇报里点名 —— 那是 P0 漏掉的，要补。

```bash
grep -rn "QuotaCenter.shared.start\|ModelCatalogCenter.shared.start\|\.start(appModel" Sources/Mac/Views/
```

预期：**零结果**。

- [x] **Step 3: 证明 SessionHost 里没有 UI 依赖**

```bash
grep -n "import SwiftUI\|import AppKit" Sources/Mac/Services/SessionHost.swift
```

预期：**零结果**。它将来要在没有画面的进程里跑，现在就不许沾。

- [x] **Step 4: 合回 main + 汇报**

按仓库惯例合回 `main`（**不要 push**），然后 `post_to_crew` 报一句：P0 完成、测试条数与 0 failures、Step 2/3 的 grep 证据、以及有没有发现漏网的长期对象。

---

## 自查

**Spec 覆盖**：本计划实现 spec §9 的 P0 行（所有权归拢）与 §6.2 闸门 1（ProcessRole 断言）。闸门 2（flock 单实例）属 P4，闸门 3（总闸默认值）属 P4/P5 —— `ProcessRole` 已经把总闸的读取实现了，P4 只需要接上。**不在本计划范围**：终端劈半（P1）、协议（P2）、快照（P3）、真分家（P4）、常驻（P5）。

**已知的不确定点**（执行时按现场为准，不要硬套）：
- Task 2 Step 3 里 `usageMonitor` 是否需要重绘订阅，**以 grep 结果为准**，计划里已写明两种走法。
- Task 4 的六个服务里，`LocalAgentUsageMonitor` 和 `CrewMailboxWaker` 的启动方法名要先 grep 确认（后者由 `CrewSessionRunner.ensureMailboxWaker` 按 crew 创建，可能没有独立的 `start`）——**有就加，没有就在汇报里说明，不要硬造一个方法出来**。
