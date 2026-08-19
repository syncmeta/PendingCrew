# P1 · 终端劈成两半 实施计划

> **For agentic workers:** 用 `superpowers:executing-plans` 逐任务执行。步骤是 `- [ ]` checkbox，做一条勾一条。

**Goal:** 把 `AgentTerminalSession`（现在是「PTY + 屏幕 + 渲染 + 状态扫描」长在同一个 AppKit 视图对象上、全跑主线程）劈成两半：**无画面的 `AgentSessionCore`**（进程 + 终端缓冲区 + 全部扫描器，纯 Foundation）与**只负责画的 `TerminalMirrorView`**（AppKit，吃字节流）。**本阶段仍是单进程**，两半之间是直接函数调用 —— 但劈完之后 core 就可以原样搬进后台进程。

**Architecture:** SwiftTerm 本身就是分层的：`Terminal` / `LocalProcess` 纯 Foundation 无 AppKit（`HeadlessTerminal` 就是官方给的组合），`TerminalView.feed(byteArray:)` 是 public。所以 core 养「无画面的真终端 + 子进程」，mirror 是一个被字节喂养的 `TerminalView`。两侧各有一份 `Terminal`，喂同一批字节，天然一致：**core 那份是状态权威，mirror 那份提供原生的选中复制 / 回滚 / reflow。**

**Tech Stack:** Swift / SwiftTerm 1.18 / AppKit / Combine / XCTest

**Spec:** `docs/2026-08-19-backend-split-design.md`（实现其 §5.1、§5.2 与 §9 的 P1 行）

**前一阶段:** `docs/2026-08-19-backend-split-p0-plan.md`（已完成，main 上）

## Global Constraints

- 编译用 `PendingCrew.xcodeproj` / scheme `PendingCrew`。加 Swift 文件必须改 `project.yml` + `xcodegen` + 提交重生的 `project.pbxproj`。
- `Sources/Mac/LocalRunner/` 整目录已编进 standalone 测试 bundle —— **本阶段所有新文件都放这里**，这样每一块都能被单测直接测。
- **三端都要编**（macOS build / iOS Simulator build / macOS test）。
- 全量测试基线：**1433 条 0 失败**（主目录、fixtures 齐全的真口径）。worktree 里会有若干条因缺 gitignored fixtures 假红 —— **开工前先跑一遍未改动状态拿你自己的基线**，用它对照；**合回 main 后必须在主目录再跑一遍，口径是真的 0 failures。**
- 小单元 auto-commit。**不要 `git push`。**
- **不许驱动图形界面**（osascript / cliclick / CGEvent / AXUIElement / screencapture，以及为验证起一个会开窗口的程序）。所有验证走命令行 + 单测。
- **行为变化只允许一处**（Task 5 的 `inspect_session` 语义），其余一律零变化。

### 已核实的 SwiftTerm 公开接口（不用改库、不用绕）

| 用途 | API |
|---|---|
| 无画面终端 + 子进程 | `Terminal(delegate:options:)`、`LocalProcess(delegate:dispatchQueue:)`，参考 `HeadlessTerminal.swift` |
| 给 mirror 喂字节 | `TerminalView.feed(byteArray: ArraySlice<UInt8>)` |
| mirror 的键盘输出 | `TerminalViewDelegate.send(source:data:)` |
| mirror 的尺寸变化 | `TerminalViewDelegate.sizeChanged(source:newCols:newRows:)` |
| 推 winsize 给 PTY | `PseudoTerminalHelpers.setWinSize(masterPtyDescriptor:windowSize:)` |
| PTY fd / pid / 存活 | `LocalProcess.childfd` / `.shellPid` / `.running`（都是 public） |
| 读画面成文本 | `Terminal.getLine(row:)` + `BufferLine.translateToString(trimRight:)` —— **在无画面 Terminal 上一模一样** |
| 回滚缓冲区 | `Terminal.changeScrollback(_:)` |

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `Sources/Mac/LocalRunner/AgentSessionCore.swift`（**新建**） | 无画面内核：`LocalProcess` + `Terminal` + 全部扫描器 + 派生状态 + 拉起自检。**不许 import AppKit / SwiftUI。** |
| `Sources/Mac/LocalRunner/TerminalMirrorView.swift`（**新建**） | 只负责画的那半：`TerminalView` 子类，吃字节、回传键盘与尺寸、外置滚动条几何、零尺寸守卫。 |
| `Sources/Mac/LocalRunner/AgentTerminalSession.swift`（大改） | 退化成**薄门面**：持有一个 core + 一个 mirror，`SessionBackend` 全部转发给 core。上层一行不用改。 |
| `Sources/Mac/LocalRunner/PlainTerminalSession.swift`（改） | 同样改成 core + mirror（Task 4）。 |
| `Sources/Mac/Services/CrewSessionRunner.swift`（小改） | `terminalTail` 改读 core 的权威画面（Task 5）。 |
| `Tests/PendingCrewTests/AgentSessionCoreTests.swift`（**新建**） | core 的 headless 集成测试（真起子进程、真读画面）。 |
| `Tests/PendingCrewTests/TerminalMirrorParityTests.swift`（**新建**） | **本阶段最重要的测试**：同一批字节喂 core 与 mirror，逐格断言两份缓冲区一致。 |

---

### Task 1: AgentSessionCore —— 无画面内核跑起来

先只做「起进程 + 喂终端 + 读画面 + 停」，扫描器留到 Task 2。

**Files:**
- Create: `Sources/Mac/LocalRunner/AgentSessionCore.swift`
- Test: `Tests/PendingCrewTests/AgentSessionCoreTests.swift`

**Interfaces:**
- Produces:
  - `final class AgentSessionCore` —— `TerminalDelegate` + `LocalProcessDelegate`
  - `init(config: SessionConfig, executable: String, workdir: String, env: [String: String])`
  - `var onOutput: ((ArraySlice<UInt8>) -> Void)?` —— mirror 挂这里拿字节
  - `func write(_ bytes: [UInt8])` / `func resize(cols: Int, rows: Int)` / `func stop()`
  - `func screenText(maxLines: Int) -> String` —— 权威画面
  - `var lastOutputAt: Date`
  - `static let scrollbackLines = 10_000`

- [x] **Step 1: 写失败的测试**

新建 `Tests/PendingCrewTests/AgentSessionCoreTests.swift`：

```swift
#if os(macOS)
import XCTest

/// `AgentSessionCore` 的 headless 集成测试（前后端分离 P1）。
///
/// 这些测试**真的起子进程、真的走 PTY**，但全程无画面 —— 这正是要证明的事：
/// 终端内核不需要 AppKit 也能跑，所以它可以整个搬进后台进程。
final class AgentSessionCoreTests: XCTestCase {

    /// 等某个条件成立，最多 `timeout` 秒。core 的输出是异步到达的。
    private func waitUntil(
        _ timeout: TimeInterval = 5, _ cond: @escaping () -> Bool
    ) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if cond() { return true }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        return cond()
    }

    func testRunsProcessAndCapturesScreenWithoutAnyView() async throws {
        let core = await AgentSessionCore(
            config: .plainShellForTesting(),
            executable: "/bin/sh",
            workdir: NSTemporaryDirectory(),
            env: ["TERM": "xterm-256color"])
        await core.write(Array("echo PENDINGCREW_MARKER\n".utf8))

        let ok = await waitUntil { 
            await MainActor.run { core.screenText(maxLines: 40) }.contains("PENDINGCREW_MARKER")
        }
        XCTAssertTrue(ok, "无画面 core 应当能读到子进程输出的画面")
        await core.stop()
    }

    func testOnOutputDeliversRawBytes() async throws {
        let core = await AgentSessionCore(
            config: .plainShellForTesting(),
            executable: "/bin/sh",
            workdir: NSTemporaryDirectory(),
            env: ["TERM": "xterm-256color"])
        let box = ByteBox()
        await MainActor.run { core.onOutput = { box.append($0) } }
        await core.write(Array("echo BYTES_HOOK\n".utf8))

        let ok = await waitUntil { box.text.contains("BYTES_HOOK") }
        XCTAssertTrue(ok, "onOutput 应当把原始 PTY 字节交出来（mirror 靠它画）")
        await core.stop()
    }

    func testStopEndsTheProcess() async throws {
        let core = await AgentSessionCore(
            config: .plainShellForTesting(),
            executable: "/bin/sh",
            workdir: NSTemporaryDirectory(),
            env: ["TERM": "xterm-256color"])
        _ = await waitUntil { await MainActor.run { core.isProcessRunning } }
        await core.stop()
        let stopped = await waitUntil { !(await MainActor.run { core.isProcessRunning }) }
        XCTAssertTrue(stopped, "stop() 之后子进程不该还活着")
    }
}

/// 线程安全的字节累加器（`onOutput` 从 PTY 队列回调）。
private final class ByteBox: @unchecked Sendable {
    private let lock = NSLock()
    private var bytes: [UInt8] = []
    func append(_ slice: ArraySlice<UInt8>) {
        lock.lock(); bytes.append(contentsOf: slice); lock.unlock()
    }
    var text: String {
        lock.lock(); defer { lock.unlock() }
        return String(decoding: bytes, as: UTF8.self)
    }
}
#endif
```

> `SessionConfig.plainShellForTesting()` 需要你按 `SessionConfig` 的真实构造器补一个 `#if DEBUG` 的测试构造（或直接在测试里构造一个最小 config）。**先读 `Sources/Mac/LocalRunner/SessionConfig.swift` 再决定怎么写** —— 别硬套上面的名字。上面的 `await MainActor.run` 包法也要按 core 实际的隔离标注调整；**测试的形状可以改，要断言的三件事不能少**：无画面能读到画面、`onOutput` 给出原始字节、`stop()` 真的停掉。

- [x] **Step 2: 跑测试确认它失败**

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' test -only-testing:PendingCrewTests/AgentSessionCoreTests 2>&1 | tail -20
```

预期：编译失败，`cannot find 'AgentSessionCore' in scope`。

- [x] **Step 3: 实现 core 的骨架**

新建 `Sources/Mac/LocalRunner/AgentSessionCore.swift`。**结构照抄 `HeadlessTerminal.swift`**（它就是官方给的「无画面终端 + 本地进程」组合），再加上我们要的东西：

```swift
#if os(macOS)
import Foundation
import Combine
import SwiftTerm

/// **无画面的终端内核**（spec `docs/2026-08-19-backend-split-design.md` §5.2）。
///
/// 在这个类型出现之前，PTY、屏幕缓冲区、渲染、以及「它在忙 / 撞额度了 / 卡在
/// 选择菜单等人按」那套从画面上认状态的逻辑，全长在**同一个 AppKit 视图对象**上、
/// 全跑主线程。那既是「关掉 app 就全停」的一半，也是 tech-debt 里
/// 「PTY 每批输出都要过主线程、代价随 session 数线性涨」那条的结构成因。
///
/// 现在它们在这里，而这里**没有一行 AppKit**：`Terminal` 与 `LocalProcess` 都是
/// 纯 Foundation（SwiftTerm 自带的 `HeadlessTerminal` 就是这个组合）。
/// 所以 P4 时这个类可以原样搬进没有画面的后台进程。
///
/// ⚠️ **不许在本文件 import AppKit / SwiftUI。** 有一条测试盯着这件事。
@MainActor
final class AgentSessionCore: NSObject, TerminalDelegate, LocalProcessDelegate {
    /// 回滚历史行数上限。理由与代价见 `TerminalMirrorView` 上同名常量的长注释
    /// （两侧必须一致，否则 mirror 能滚到的历史比 core 记得的多/少）。
    static let scrollbackLines = 10_000

    private(set) var terminal: Terminal!
    private(set) var process: LocalProcess!

    /// mirror 挂这里拿字节。**core 自己不知道有没有人在看** —— 没人挂就没人收，
    /// 那正是 P4 之后「没人看的 session 不占主线程」的形状。
    var onOutput: ((ArraySlice<UInt8>) -> Void)?
    /// 子进程退出回调（门面据此翻 status）。
    var onExit: ((Int32?) -> Void)?

    /// 最近一次收到子进程输出的时刻；busy 判定 = now - lastOutputAt < 阈值。
    private(set) var lastOutputAt = Date.distantPast
    /// 当前视口尺寸（没有 mirror 在看时保持最后一次的值，**绝不 resize 成 0**）。
    private(set) var cols = 80
    private(set) var rows = 24

    var isProcessRunning: Bool { process?.running == true }

    init(config: SessionConfig, executable: String, workdir: String, env: [String: String]) {
        super.init()
        var opts = TerminalOptions.default
        opts.scrollback = Self.scrollbackLines
        terminal = Terminal(delegate: self, options: opts)
        process = LocalProcess(delegate: self)

        var envArr = env.map { "\($0.key)=\($0.value)" }
        if !envArr.contains(where: { $0.hasPrefix("TERM=") }) { envArr.append("TERM=xterm-256color") }
        process.startProcess(
            executable: executable, args: config.argv(),
            environment: envArr, currentDirectory: workdir)
    }

    // MARK: - LocalProcessDelegate

    func processTerminated(_ source: LocalProcess, exitCode: Int32?) { onExit?(exitCode) }

    /// PTY 有新字节：**先喂自己那份权威缓冲区，再转给看的人**。顺序不能反 ——
    /// mirror 可能不存在，权威那份必须无条件收到。
    func dataReceived(slice: ArraySlice<UInt8>) {
        terminal.feed(buffer: slice)
        lastOutputAt = Date()
        onOutput?(slice)
    }

    func getWindowSize() -> winsize {
        winsize(ws_row: UInt16(rows), ws_col: UInt16(cols), ws_xpixel: 16, ws_ypixel: 16)
    }

    // MARK: - TerminalDelegate

    /// 终端要往「主机」写字节（回复设备查询之类）——原路送回子进程。
    func send(source: Terminal, data: ArraySlice<UInt8>) { process.send(data: data) }
    func showCursor(source: Terminal) {}
    func hideCursor(source: Terminal) {}
    func setTerminalTitle(source: Terminal, title: String) {}
    func setTerminalIconTitle(source: Terminal, title: String) {}
    func sizeChanged(source: Terminal) {}
    func mouseModeChanged(source: Terminal) {}
    func hostCurrentDirectoryUpdate(source: Terminal, directory: String?) {}
    func colorChanged(source: Terminal, idx: Int?) {}
    func bell(source: Terminal) {}

    // MARK: - 控制面

    /// 写字节进 PTY（键盘、程序化 send、菜单按键都走这里）。
    func write(_ bytes: [UInt8]) {
        guard isProcessRunning else { return }
        process.send(data: bytes[...])
    }

    /// 视口尺寸变了：同步自己那份 `Terminal`，并把 winsize 推给 PTY。
    /// 零/退化尺寸一律忽略 —— 2 列下 reflow 会把历史按 2 字宽重排、顶部被永久裁掉
    /// （Todo #34 实测：100 列 400 行历史过一次退化尺寸，10000 行上限下只剩 223 行，
    /// 且首行断在半截）。守卫在 mirror 侧也有一道，这里是第二道。
    func resize(cols newCols: Int, rows newRows: Int) {
        guard newCols >= 2, newRows >= 1 else { return }
        guard newCols != cols || newRows != rows else { return }
        cols = newCols; rows = newRows
        terminal.resize(cols: newCols, rows: newRows)
        guard isProcessRunning, process.childfd >= 0 else { return }
        var size = getWindowSize()
        _ = PseudoTerminalHelpers.setWinSize(masterPtyDescriptor: process.childfd, windowSize: &size)
    }

    /// **权威画面**（`inspect_session` 用）：当前屏幕内容，与任何窗口滚到哪无关。
    func screenText(maxLines: Int) -> String {
        var lines: [String] = []
        for row in 0..<terminal.rows {
            guard let line = terminal.getLine(row: row) else { continue }
            lines.append(line.translateToString(trimRight: true))
        }
        while let last = lines.last, last.trimmingCharacters(in: .whitespaces).isEmpty {
            lines.removeLast()
        }
        return lines.suffix(maxLines).joined(separator: "\n")
    }

    func stop() {
        guard isProcessRunning else { return }
        let pid = process.shellPid
        process.terminate()
        Task { await terminateTree(pid: pid, graceSeconds: 2.0) }
    }
}
#endif
```

> `TerminalDelegate` 的必需方法集合以**编译器报错为准** —— SwiftTerm 1.18 的协议可能与上面列的不完全一致，缺哪个补哪个空实现。`TerminalOptions` 的 `scrollback` 字段名同理，编不过就照 `TerminalOptions.swift` 改。

- [x] **Step 4: 加进 project.yml（如需）并跑测试**

`Sources/Mac/LocalRunner` 是整目录纳入，通常不用改 `project.yml`。确认一下再跑：

```bash
grep -n "Sources/Mac/LocalRunner" project.yml
xcodegen
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' test -only-testing:PendingCrewTests/AgentSessionCoreTests 2>&1 | tail -20
```

预期：三条测试全过。

- [x] **Step 5: 提交**

```bash
git add Sources/Mac/LocalRunner/AgentSessionCore.swift \
        Tests/PendingCrewTests/AgentSessionCoreTests.swift project.yml \
        PendingCrew.xcodeproj/project.pbxproj
git commit -m "feat(split): AgentSessionCore —— 无画面的终端内核

Terminal + LocalProcess 都是纯 Foundation(SwiftTerm 自带 HeadlessTerminal 就是
这个组合),所以内核不需要 AppKit 也能跑 —— 三条 headless 集成测试真起子进程、
真走 PTY、真读画面来证明这件事。P4 时这个类原样搬进后台进程。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: 把全部扫描器与派生状态搬进 core

**Files:**
- Modify: `Sources/Mac/LocalRunner/AgentSessionCore.swift`
- Modify: `Sources/Mac/LocalRunner/AgentTerminalSession.swift`（退化成薄门面）

**要搬的（全部从 `AgentTerminalSession` 原样搬，连注释一起）：**

| 搬什么 | 现位置（`AgentTerminalSession.swift`） |
|---|---|
| `SessionHealthScanner` + `health` + `quotaHealthAt` + `clearQuotaHealth` | init 的 `onData` 闭包 + `:445` 一带 |
| `RateLimitMenuScanner` + `answerRateLimitMenu` | 同上 + `:621` |
| `PendingDecisionTracker` + `pendingDecision` + `pollPendingDecision` | 同上 + `:645` |
| `TypingActivityTracker` + `AnsiPlainTextTail` + `displayIsTyping` | 同上 + `recomputeWorking` |
| `SessionProfileEchoScanner` + `applyProfileSwitch` + `waitForTerminalIdle` | `:571`–`:612` |
| `SessionLaunchParameterScanner` + `launchParameterProblem` | init 闭包 |
| 0.6s `busyTimer` + `recomputeWorking` + `isWorking` + `workingSince` | `:359`–`:368`、`:520` |
| 拉起自检：`launchWatchdog` / `reportLaunchFailureIfStillborn` / `reportLaunchFailure` / `reapIfExited` / `decodeWaitStatus` | `:380`–`:500` |
| `status` + `SessionExitReason` 相关 | `:157` |
| `send` / `interrupt` / `sendRaw` / `stop` / `inject` | `:530`–`:545`、`:615`、`:665` |

**搬迁时唯一允许的改动**：把 `terminalView.lastOutputAt` 换成 `lastOutputAt`、`terminalView.process?.send` 换成 `write(...)`、`terminalView.process?.shellPid` 换成 `process.shellPid`、`terminalView.getTerminal()` 换成 `terminal`。**其余逐字不动。**

- [x] **Step 1: 搬扫描器与派生状态**

按上表逐条搬。core 上把这些声明成 `@Published private(set)`（`status` / `isWorking` / `displayIsTyping` / `health` / `pendingDecision` / `launchParameterProblem`），门面转发它们的 publisher。

`dataReceived` 里的旁路顺序**保持原样**（原 `onData` 闭包里那一串：launchFailed 自我纠正 → healthScanner → rateLimitScanner → decisionTracker → typingStripper → profileEchoScanner → launchParameterScanner）。

- [x] **Step 2: `AgentTerminalSession` 退化成薄门面**

它继续实现 `SessionBackend`（上层一行不用改），但内部只做三件事：持有 core、持有 mirror（Task 3 之前先临时持有一个空的）、把协议方法转发给 core。

```swift
@MainActor
final class AgentTerminalSession: ObservableObject, SessionBackend {
    let kind: LocalCodingAgentKind
    let core: AgentSessionCore
    // Task 3 之后：let mirror: TerminalMirrorView

    var status: SessionStatus { core.status }
    var statusPublisher: Published<SessionStatus>.Publisher { core.$status }
    var isBusy: Bool { false }   // 语义与理由见 SessionBackend 里那段注释，不变
    var isWorking: Bool { core.isWorking }
    var isWorkingPublisher: Published<Bool>.Publisher { core.$isWorking }
    // …其余同构转发…
    func send(_ text: String) { core.send(text) }
    func interrupt() { core.interrupt() }
    func stop() { core.stop() }
    func clearQuotaHealth() { core.clearQuotaHealth() }
    func applyProfileSwitch(_ c: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        await core.applyProfileSwitch(c)
    }
}
```

> `SessionBackend` 要求的是 `Published<T>.Publisher`，转发别人的 `@Published` 需要 core 把它们暴露出来（`core.$status`）。若类型对不上，改成协议里用 `AnyPublisher` 也可以 —— **但那要改 `SessionBackend`，属于额外风险，优先用直接转发。**

- [x] **Step 3: 编译 + 全量测试**

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build 2>&1 | tail -5
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' test 2>&1 | tail -30
```

预期：现有的扫描器/终止/拉起失败那批测试（`AgentTerminationTests`、`AgentTerminalLaunchFailureTests` 等）**全部照旧通过** —— 它们是这次搬家的回归网。挂了就是搬错了，**别改测试去迁就实现**。

- [x] **Step 4: 提交**

```bash
git add Sources/Mac/LocalRunner/AgentSessionCore.swift Sources/Mac/LocalRunner/AgentTerminalSession.swift
git commit -m "refactor(split): 扫描器与派生状态搬进无画面内核

health/rate-limit 菜单/待决策/正在输入/切档回显/启动参数六个扫描器,以及
0.6s busy 轮询与拉起自检,全部从 AppKit 视图对象搬进 AgentSessionCore。
AgentTerminalSession 退化成薄门面,SessionBackend 逐条转发 —— 上层零改动。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: TerminalMirrorView —— 只负责画的那半

**Files:**
- Create: `Sources/Mac/LocalRunner/TerminalMirrorView.swift`
- Modify: `Sources/Mac/LocalRunner/AgentTerminalSession.swift`（接上 mirror）
- Test: `Tests/PendingCrewTests/TerminalMirrorParityTests.swift`

**Interfaces:**
- Consumes: `AgentSessionCore.onOutput` / `.write(_:)` / `.resize(cols:rows:)`
- Produces: `final class TerminalMirrorView: TerminalView, TerminalViewDelegate`

- [x] **Step 1: 先写一致性测试（本阶段最重要的一条）**

新建 `Tests/PendingCrewTests/TerminalMirrorParityTests.swift`：

```swift
#if os(macOS)
import XCTest
import SwiftTerm

/// **两份缓冲区必须逐格一致**（前后端分离 P1）。
///
/// core 那份是状态权威（`inspect_session` 读它、扫描器喂它）；mirror 那份提供
/// 原生的选中复制 / 回滚 / reflow。两份喂同一批字节，就必须得出同一个画面 ——
/// 一旦分叉，人看到的和机长看到的就不是同一件事，而那种 bug 极难被发现。
final class TerminalMirrorParityTests: XCTestCase {

    /// 逐格比较两个终端的主屏内容 + 光标位置。
    private func assertGridsEqual(
        _ a: Terminal, _ b: Terminal, _ what: String, file: StaticString = #filePath, line: UInt = #line
    ) {
        XCTAssertEqual(a.cols, b.cols, "\(what)：列数", file: file, line: line)
        XCTAssertEqual(a.rows, b.rows, "\(what)：行数", file: file, line: line)
        for row in 0..<min(a.rows, b.rows) {
            let la = a.getLine(row: row)?.translateToString(trimRight: true) ?? ""
            let lb = b.getLine(row: row)?.translateToString(trimRight: true) ?? ""
            XCTAssertEqual(la, lb, "\(what)：第 \(row) 行", file: file, line: line)
        }
    }

    /// 覆盖面语料：SGR、CJK 宽字符、alt-screen 进出、滚动区域、清屏、
    /// 绝对/相对光标移动、超出 scrollback 的溢出。
    private var corpus: [UInt8] {
        var s = ""
        s += "\u{1b}[2J\u{1b}[H"
        s += "\u{1b}[1;31m红色粗体\u{1b}[0m 普通\r\n"
        s += "\u{1b}[38;5;208m256色\u{1b}[0m \u{1b}[38;2;10;200;30m真彩\u{1b}[0m\r\n"
        s += "中文宽字符测试 CJK 宽度对齐\r\n"
        s += "\u{1b}[4;10r"                       // DECSTBM 滚动区域
        for i in 0..<200 { s += "line \(i)\r\n" } // 越过屏幕、进回滚
        s += "\u{1b}[?1049h"                      // 进 alt-screen
        s += "ALT SCREEN CONTENT\r\n"
        s += "\u{1b}[?1049l"                      // 回主屏
        s += "\u{1b}[5;3H光标绝对定位"
        return Array(s.utf8)
    }

    @MainActor
    func testMirrorGridMatchesCoreGridForTheSameBytes() {
        let cols = 100, rows = 30
        let core = Terminal(delegate: NullTerminalDelegate(),
                            options: Self.options(cols: cols, rows: rows))
        let mirror = TerminalMirrorView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
        mirror.getTerminal().resize(cols: cols, rows: rows)

        core.feed(buffer: corpus[...])
        mirror.feed(byteArray: corpus[...])

        assertGridsEqual(core, mirror.getTerminal(), "同一批字节喂两份缓冲区")
    }

    @MainActor
    func testMirrorStaysEqualAfterWidthChanges() {
        let core = Terminal(delegate: NullTerminalDelegate(),
                            options: Self.options(cols: 100, rows: 30))
        let mirror = TerminalMirrorView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
        mirror.getTerminal().resize(cols: 100, rows: 30)
        core.feed(buffer: corpus[...])
        mirror.feed(byteArray: corpus[...])

        // 改窗口宽度 = reflow。两侧走的是同一段 Buffer.resize，结果必须一致。
        for width in [60, 160, 80] {
            core.resize(cols: width, rows: 30)
            mirror.getTerminal().resize(cols: width, rows: 30)
            assertGridsEqual(core, mirror.getTerminal(), "reflow 到 \(width) 列后")
        }
    }

    /// **agent 的 TUI 大部分时间活在 alt-screen 里**（claude/codex 的全屏界面），
    /// 所以「退回主屏之后两份一致」根本没验到我们真正要还原的那块画面。
    /// 这条在**仍处于 alt-screen 时**断言，并且在 alt-screen 里改宽度。
    ///
    /// 注意 alt-screen 没有回滚缓冲、resize 语义也与主屏不同（不 reflow，由应用
    /// 自己重画）—— 这里不假设哪种语义对，只要求**两份缓冲区得出同一个结果**。
    @MainActor
    func testMirrorMatchesCoreWhileInsideAltScreen() {
        let core = Terminal(delegate: NullTerminalDelegate(),
                            options: Self.options(cols: 100, rows: 30))
        let mirror = TerminalMirrorView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
        mirror.getTerminal().resize(cols: 100, rows: 30)

        var s = ""
        s += "\u{1b}[2J\u{1b}[H主屏上先留一点历史\r\n"
        for i in 0..<50 { s += "history \(i)\r\n" }
        s += "\u{1b}[?1049h"                       // 进 alt-screen，**不再退出来**
        s += "\u{1b}[2J\u{1b}[H"
        s += "\u{1b}[1;36m╭─ AGENT TUI ─────────╮\u{1b}[0m\r\n"
        s += "\u{1b}[36m│\u{1b}[0m 中文宽字符 + emoji ✅ \u{1b}[36m│\u{1b}[0m\r\n"
        s += "\u{1b}[36m╰─────────────────────╯\u{1b}[0m\r\n"
        s += "\u{1b}[2;5H"                          // alt 屏里定位光标
        let bytes = Array(s.utf8)

        core.feed(buffer: bytes[...])
        mirror.feed(byteArray: bytes[...])
        assertGridsEqual(core, mirror.getTerminal(), "仍在 alt-screen 内")

        // alt-screen 里改宽度 —— agent TUI 运行期间拖窗口就是这条路径。
        for width in [60, 160, 100] {
            core.resize(cols: width, rows: 30)
            mirror.getTerminal().resize(cols: width, rows: 30)
            assertGridsEqual(core, mirror.getTerminal(), "alt-screen 内 resize 到 \(width) 列后")
        }

        // 退回主屏后，之前那 50 行历史两边都要还在、且一致。
        let leave = Array("\u{1b}[?1049l".utf8)
        core.feed(buffer: leave[...])
        mirror.feed(byteArray: leave[...])
        assertGridsEqual(core, mirror.getTerminal(), "退出 alt-screen 回到主屏后")
    }

    private static func options(cols: Int, rows: Int) -> TerminalOptions {
        var o = TerminalOptions.default
        o.cols = cols; o.rows = rows; o.scrollback = AgentSessionCore.scrollbackLines
        return o
    }
}

/// 什么都不做的 `TerminalDelegate`（测试里只关心缓冲区）。
private final class NullTerminalDelegate: TerminalDelegate {
    func send(source: Terminal, data: ArraySlice<UInt8>) {}
    func showCursor(source: Terminal) {}
    func hideCursor(source: Terminal) {}
}
#endif
```

> `TerminalOptions` 的字段名、`NullTerminalDelegate` 需要实现的方法集合，**以编译器为准**。断言的三件事不能少：**同一批字节两份缓冲区逐格一致**、**reflow 之后仍逐格一致**、**alt-screen 内（含在 alt-screen 内改宽度）仍逐格一致**。
>
> 第三条是父机长点出来的、我原本会漏的：agent 的 TUI 大部分时间就活在 alt-screen 里，「退回主屏后一致」等于没验到真正要还原的那块画面。

- [x] **Step 2: 跑测试确认它失败**

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' test -only-testing:PendingCrewTests/TerminalMirrorParityTests 2>&1 | tail -20
```

预期：`cannot find 'TerminalMirrorView' in scope`。

- [x] **Step 3: 实现 mirror**

新建 `Sources/Mac/LocalRunner/TerminalMirrorView.swift`。它是 `TerminalView` 的子类（**不是** `LocalProcessTerminalView` —— 它不再自己开进程），并把 `ActivityTerminalView` 里**与画面有关的那半**原样搬过来：

- `scrollbackLines` 常量 + `collapseScrollbackAfterExit()`（连那两段长注释一起搬）
- `setFrameSize` 的零尺寸守卫 + `isRealLayout`（连注释）
- `scrolled(source:yDisp:)` 的用户主动判定 + `onScroll`
- `didAddSubview` / `hideNativeScroller` / `useNativeScroller` 的内部 scroller 隐藏
- `ScrollState` 与 `refreshScrollState`（几何是视图的事，从门面搬到这里）

新增的接线：

```swift
    /// 内核 —— mirror 只画它、把人的输入回传给它，自己不碰进程。
    weak var core: AgentSessionCore?

    /// 「用户主动滚」的判定要看最近有没有 PTY 输出；那个时刻归内核记。
    /// mirror 自己不再有 `lastOutputAt`。
    private var lastOutputAt: Date { core?.lastOutputAt ?? .distantPast }

    // MARK: - TerminalViewDelegate

    /// 人敲键盘 / 粘贴 → 回传给内核写进 PTY。
    func send(source: TerminalView, data: ArraySlice<UInt8>) {
        core?.write(Array(data))
    }

    /// 视口行列数变了 → 告诉内核，由它 resize 自己那份缓冲区并推 winsize。
    /// **mirror 不直接碰 PTY** —— PTY 的属主是内核（P4 之后它在另一个进程里）。
    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
        core?.resize(cols: newCols, rows: newRows)
        onViewportChange?()
    }
```

`onViewportChange` 接到 core 的 `typingActivity.noteViewportChange(at:)`（原来挂在 `delegate.onSizeChanged` 上的那条：切 crew 导致的整屏重绘不该点亮「正在输入」气泡，Todo #32）。

- [x] **Step 4: 门面接上 mirror**

`AgentTerminalSession` 里：

```swift
        self.mirror = TerminalMirrorView(frame: .zero)
        self.mirror.core = core
        self.mirror.terminalDelegate = mirror     // 自己当自己的 delegate
        core.onOutput = { [weak mirror] slice in
            MainActor.assumeIsolated { mirror?.feed(byteArray: slice) }
        }
```

`var terminalView: ...` 这类对外暴露改成返回 mirror。`Sources/Mac/Views/AgentTerminalView.swift` 与 `CrewSessionWindowView.swift` 的类型跟着改（把 `ActivityTerminalView` 换成 `TerminalMirrorView`）。

- [x] **Step 5: 编译三端 + 全量测试 + 提交**

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build 2>&1 | tail -5
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'generic/platform=iOS Simulator' build 2>&1 | tail -5
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' test 2>&1 | tail -30
```

```bash
git add Sources/Mac/LocalRunner/TerminalMirrorView.swift Sources/Mac/LocalRunner/AgentTerminalSession.swift \
        Sources/Mac/Views/AgentTerminalView.swift Sources/Mac/Views/CrewSessionWindowView.swift \
        Tests/PendingCrewTests/TerminalMirrorParityTests.swift \
        project.yml PendingCrew.xcodeproj/project.pbxproj
git commit -m "feat(split): TerminalMirrorView —— 只负责画的那半

不再是 LocalProcessTerminalView(自己开进程),改成被字节喂养的 TerminalView:
内核吐字节它就画,人敲键盘它回传给内核,视口尺寸变了它告诉内核去推 winsize。
选中复制/回滚/reflow 全是原生代码路径,一行没改。

一致性测试钉死两条:同一批字节两份缓冲区逐格相等、reflow 到 60/160/80 列后
仍逐格相等。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: PlainTerminalSession 同样劈开，删掉 ActivityTerminalView

**Files:**
- Modify: `Sources/Mac/LocalRunner/PlainTerminalSession.swift`
- Delete: `ActivityTerminalView`（它在 `AgentTerminalSession.swift` 顶部）

- [x] **Step 1: PlainTerminalSession 改成 core + mirror**

它只有 102 行、没有任何扫描器，改法与门面一致：持有一个 `AgentSessionCore`（config 用它的 shell argv）+ 一个 `TerminalMirrorView`（`useNativeScroller()`，普通 shell 保留 SwiftTerm 原生滚动条）。

- [x] **Step 2: 删掉 `ActivityTerminalView`**

它是 `LocalProcessTerminalView` 的子类 —— 只要还有人用它，就还有「视图自己开进程」的路子。删干净，然后：

```bash
grep -rn "ActivityTerminalView\|LocalProcessTerminalView" Sources/
```

预期：**零结果**。**这是 P1 是否真的做完的判定条件** —— 有残留就说明还有一条路绕过了劈分。

- [x] **Step 3: 编译三端 + 全量测试 + 提交**

```bash
git add Sources/Mac/LocalRunner/PlainTerminalSession.swift Sources/Mac/LocalRunner/AgentTerminalSession.swift
git commit -m "refactor(split): 普通终端也劈开,删掉 ActivityTerminalView

LocalProcessTerminalView 的最后一个子类没了 —— 视图自己开进程的路子被彻底
堵死,全仓 grep 零结果。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: inspect_session 改读权威画面 + 验收

**Files:**
- Modify: `Sources/Mac/Services/CrewSessionRunner.swift`（`terminalTail` / `inspect`）

- [ ] **Step 1: 改读权威画面**

`CrewSessionRunner.terminalTail(_ view:maxLines:)`（`:851`、`:856` 两处调用）改成读 `core.screenText(maxLines:)`。

**这是本阶段唯一允许的行为变化**，理由写进代码注释：

```swift
    /// 读**权威画面**（core 那份无画面 Terminal 的当前屏幕），与任何窗口滚到哪无关。
    ///
    /// 改动前读的是 SwiftTerm 的当前可见区（按 `yDisp`）—— 也就是**人把那个终端
    /// 往上滚，机长看到的文本就跟着变**。那是实现细节漏出来的，不是设计：机长要的
    /// 是「它现在卡在哪一屏」，不是「人正在看哪一屏」。改完之后同一个 sessionId
    /// 在任何时刻问到的都是同一份画面（spec §5.1）。
```

- [ ] **Step 2: 全量验收**

```bash
xcodegen
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build 2>&1 | tail -3
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'generic/platform=iOS Simulator' build 2>&1 | tail -3
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' test 2>&1 | tail -30
```

- [ ] **Step 3: 收集结构证据**

```bash
# 内核不许沾 UI —— 它要能原样搬进无画面进程
grep -n "import AppKit\|import SwiftUI" Sources/Mac/LocalRunner/AgentSessionCore.swift   # 期望零结果
# 视图自己开进程的路子已堵死
grep -rn "ActivityTerminalView\|LocalProcessTerminalView" Sources/                        # 期望零结果
# 上层没有因为劈分而改动
git diff --stat main~N -- Sources/Mac/Services/CrewSessionRunner.swift                    # 期望只有 terminalTail 那一处
```

- [ ] **Step 4: 合回 main（不要 push）+ 在主目录跑真口径全量**

```bash
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' test 2>&1 | tail -30
```

口径：**真的 0 failures**，条数 ≥ 1433 + 本阶段新增。

- [ ] **Step 5: 汇报（含给人类的手工清单）**

`post_to_crew` 报：

1. 测试「Executed N tests, with 0 failures」原文 + 三端全绿。
2. Step 3 的三条 grep 证据。
3. **一致性测试的具体断言**（同一批字节逐格相等、reflow 到 60/160/80 列后仍相等、**alt-screen 内及其 resize 后仍相等**）。
4. **给人类的三条手工清单** —— 写进 `docs/2026-08-19-backend-split-manual-checks.md`（新建），**不要催人现在就点**（见下）。

**⚠️ 节奏要求（父机长定的，别自作主张提前）**：这三下必须装新版才能验，而装新版要 ⌘Q、所有 session 一起死 —— **那正是本项目要消灭的痛点，为验证它而制造它是本末倒置**。所以清单**攒着**，等下一次本来就要装包的时候（P3 完成、或期间有别的原因要发版）一起点。P1 在单进程内、可单 commit revert，自动化已经覆盖了缓冲区语义 / reflow / 回滚长度 / 选中文本这些**数据层**一致性；人手这三下验的是**交互层手感**，晚几天验、真出问题也 revert 得掉。

清单必须写成**能照着点的具体步骤 + 明确判据**，不许写「看看正不正常」。照这个格式：

```markdown
## 手工验收 · 终端手感三条（P1 交付，攒到下次装包时点）

前置：装上含 P1 的新版本，随便起一个 claude session，让它跑到吐出至少两屏输出。

### 1. 改窗口宽度的重排
**做**：把 PendingCrew 窗口从当前宽度拖到约两倍宽，再拖回来。
**判据（全部满足才算过）**：
- 历史整齐重排，没有半截行、没有断在一半的中文字
- 原本一行的长命令，加宽后仍是一行（不是被切成两段拼起来的样子）
- 拖动过程不卡顿到肉眼可见掉帧

### 2. 往上滚（回滚缓冲）
**做**：在终端里用触控板/滚轮一直往上滚，滚到不能再滚为止。
**判据**：
- 能滚回去的历史远超一屏（这个 session 跑过的输出基本都在）
- **最顶上那一行是完整的一行**，不是从半个字开始的碎片
- 滚动过程中右侧细滚动条跟着动，松手后自动淡出

### 3. 选中复制
**做**：用鼠标拖选一段**跨越至少三行**的文本（含中文和英文），⌘C，粘到任意文本框。
**判据**：
- 粘出来的内容与屏幕上选中的完全一致，行数、换行位置都对
- 中文不出现乱码或半个字
- 行尾没有多出一堆空格

任何一条不过 → 在群里说是哪一条、什么现象，这一阶段可以整体 revert。
```

---

## 自查

**Spec 覆盖**：本计划实现 spec §5.1（协议泄漏三分：显示 / 读画面 / 写字节 / 读几何）、§5.2（劈开后的两半）与 §9 的 P1 行。

**不在本计划范围**：连接协议（P2）、缓冲区快照序列化与背压（P3）、真进程分家（P4）、常驻与善后（P5）。**特别是 §5.3 的快照序列化不要提前做** —— 它是 P3，而且是全项目风险最高的一块，需要独立的往返属性测试。

**已知的不确定点**（以现场为准，别硬套）：
- `TerminalDelegate` / `TerminalViewDelegate` 的必需方法集合、`TerminalOptions` 的字段名 —— 以编译器为准。
- `SessionBackend` 要求 `Published<T>.Publisher`，门面转发 core 的 `@Published` 若类型对不上，**优先改门面而不是改协议**。
- `SessionConfig` 用于普通 shell 的构造方式 —— 先读 `SessionConfig.swift`。

**遇到这几种情况停下来问机长，不要自己拍**：
- 一致性测试发现两份缓冲区在某类语料上就是对不齐（说明 mirror 的喂养路径与 core 不等价，是真问题）；
- 要为了让门面转发通过而修改 `SessionBackend` 协议；
- 现有的 `AgentTerminationTests` / `AgentTerminalLaunchFailureTests` 挂了（那是搬错了，**别改测试迁就实现**）。
