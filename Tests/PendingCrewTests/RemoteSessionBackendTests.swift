#if os(macOS)
import Combine
import XCTest

@MainActor
final class RemoteSessionBackendTests: XCTestCase {
    func testSessionBackendControlsAndStateCrossTheFramedTransport() async {
        let direct = ProtocolTestBackend(kind: .codex)
        direct.profileOutcome = .applied("Set model to gpt-5")
        let bridge = InProcessSessionProtocolBridge()
        let remote = bridge.exposeAttached(sessionId: "session-1", backend: direct)

        XCTAssertEqual(remote.status, .running)
        XCTAssertEqual(remote.kind, .codex)
        XCTAssertTrue(remote.isProtocolConnected)

        remote.send("hello")
        remote.interrupt()
        remote.clearQuotaHealth()
        XCTAssertEqual(direct.sent, ["hello"])
        XCTAssertEqual(direct.interruptCount, 1)
        XCTAssertEqual(direct.clearQuotaCount, 1)

        direct.wakeResults = [.retry, .accepted]
        let firstWake = await remote.submitWake("wake-1")
        let secondWake = await remote.submitWake("wake-2")
        XCTAssertEqual(firstWake, .retry)
        XCTAssertEqual(secondWake, .accepted)
        XCTAssertEqual(direct.submittedWakes, ["wake-1", "wake-2"])

        let outcome = await remote.applyProfileSwitch(.init(knob: .model, value: "gpt-5"))
        XCTAssertEqual(outcome, .applied("Set model to gpt-5"))
        XCTAssertEqual(direct.profileCommands, [.init(knob: .model, value: "gpt-5")])

        direct.health = CrewSessionHealth(kind: .usageLimit, detail: "limit")
        direct.isWorking = true
        direct.launchParameterProblem = .effortIgnored(value: "auto", quote: "ignored")
        direct.status = .exited(23)
        XCTAssertEqual(remote.health, CrewSessionHealth(kind: .usageLimit, detail: "limit"))
        XCTAssertTrue(remote.isWorking)
        XCTAssertEqual(remote.launchParameterProblem,
                       .effortIgnored(value: "auto", quote: "ignored"))
        XCTAssertEqual(remote.status, .exited(23))

        direct.inspectionText = "authoritative tail"
        XCTAssertEqual(remote.screenText(maxLines: 17), "authoritative tail")
        XCTAssertEqual(direct.requestedScreenTextLineLimits, [17])
        try? await remote.updateApprovalsReviewer(.user)
        XCTAssertEqual(direct.approvalsReviewers, [.user])

        remote.stop()
        XCTAssertEqual(direct.stopCount, 1)
    }

    func testTerminalBytesUseKindOneAndKeyboardResizeUseInputMessages() {
        let direct = ProtocolTestBackend(kind: .claudeCode)
        let bridge = InProcessSessionProtocolBridge()
        let remote = bridge.exposeAttached(sessionId: "session-terminal", backend: direct)

        bridge.publishTerminalBytes(sessionId: "session-terminal", bytes: [0xff, 0x00, 0x41])
        XCTAssertEqual(remote.lastTerminalFrameBytes, [0xff, 0x00, 0x41])

        remote.sendRaw([0x1b, 0x0d])
        remote.resizeTerminal(cols: 132, rows: 43)
        XCTAssertEqual(direct.rawInputs, [[0x1b, 0x0d]])
        XCTAssertEqual(direct.resizes, [
            .init(cols: 80, rows: 25), // attach preserves AgentSessionCore's default viewport
            .init(cols: 132, rows: 43),
        ])
    }

    func testPlainTerminalInterruptKeepsCtrlCBehaviorAcrossInputMessage() {
        let direct = ProtocolTestBackend(kind: .terminal)
        let bridge = InProcessSessionProtocolBridge()
        let remote = bridge.exposeAttached(sessionId: "plain-terminal", backend: direct)

        remote.interrupt()

        XCTAssertEqual(direct.rawInputs, [[0x03]])
    }

    func testMissingCapabilityDegradesWithoutRejectingConnection() {
        let direct = ProtocolTestBackend(kind: .codex)
        let bridge = InProcessSessionProtocolBridge(
            appCapabilities: ["transcript-events", "approval-mode"],
            daemonCapabilities: ["transcript-events"])
        let remote = bridge.exposeAttached(sessionId: "session-old-daemon", backend: direct)

        XCTAssertTrue(remote.isProtocolConnected)
        XCTAssertEqual(remote.negotiatedCapabilities, ["transcript-events"])
        XCTAssertFalse(remote.supportsCapability("approval-mode"))
        XCTAssertFalse(remote.supportsCapability("screen-text"))
        direct.inspectionText = "must not be read"
        XCTAssertEqual(remote.screenText(maxLines: 20), "（daemon 不支持读取输出）")
        XCTAssertEqual(direct.requestedScreenTextLineLimits, [])

        let historyOnlyOnApp = ProtocolTestBackend(kind: .codex)
        historyOnlyOnApp.codexHistory = [
            .init(id: "old", kind: .agentMessage(text: "unsupported", phase: nil)),
        ]
        let oldDaemonBridge = InProcessSessionProtocolBridge(
            appCapabilities: ["transcript-events"], daemonCapabilities: [])
        let degraded = oldDaemonBridge.exposeAttached(
            sessionId: "old-daemon-history", backend: historyOnlyOnApp)
        XCTAssertTrue(degraded.isProtocolConnected)
        XCTAssertEqual(degraded.transcript?.items, [],
                       "旧 daemon 缺 transcript-events 时少历史功能，但不拒连")
    }

    func testAuthoritativeScreenTextLookupCoversDirectAndRemoteBackends() {
        let direct = ProtocolTestBackend(kind: .claudeCode)
        direct.inspectionText = "resume rejection"
        XCTAssertEqual(
            SessionAuthoritativeScreenText.read(from: direct, maxLines: 40),
            "resume rejection")

        let bridge = InProcessSessionProtocolBridge()
        let remote = bridge.exposeAttached(sessionId: "remote-screen", backend: direct)
        XCTAssertEqual(
            SessionAuthoritativeScreenText.read(from: remote, maxLines: 20),
            "resume rejection")
        XCTAssertEqual(direct.requestedScreenTextLineLimits, [40, 20])
    }

    func testOutputProducedBeforeRegistrationAndAttachIsFlushedThroughProtocol() {
        let bridge = InProcessSessionProtocolBridge()
        let output = bridge.terminalOutputSink(sessionId: "early-terminal")
        output([0x65, 0x61, 0x72, 0x6c, 0x79])

        let remote = bridge.exposeAttached(
            sessionId: "early-terminal", backend: ProtocolTestBackend(kind: .claudeCode))

        XCTAssertEqual(remote.lastTerminalFrameBytes, Array("early".utf8))
    }

    func testCodexNotificationsBeforeAndAfterAttachReachRemoteTranscriptAsEvents() {
        let bridge = InProcessSessionProtocolBridge()
        let notification = bridge.codexNotificationSink(sessionId: "codex-events")
        notification("item/completed", [
            "item": ["id": "before", "type": "agentMessage", "text": "one"],
        ])
        let remote = bridge.exposeAttached(
            sessionId: "codex-events", backend: ProtocolTestBackend(kind: .codex))
        notification("item/completed", [
            "item": ["id": "after", "type": "agentMessage", "text": "two"],
        ])

        XCTAssertEqual(remote.transcript?.items.count, 2)
    }

    func testCodexTurnEventImmediatelyCorrectsRemoteWorkingState() {
        let bridge = InProcessSessionProtocolBridge()
        let notification = bridge.codexNotificationSink(sessionId: "codex-turn-state")
        let remote = bridge.exposeAttached(
            sessionId: "codex-turn-state", backend: ProtocolTestBackend(kind: .codex))

        XCTAssertFalse(remote.isWorking)
        notification("turn/started", ["turn": ["id": "turn-1"]])
        XCTAssertTrue(remote.isWorking)
        XCTAssertTrue(remote.isBusy)
        notification("turn/completed", ["turn": ["id": "turn-1"]])
        XCTAssertFalse(remote.isWorking)
        XCTAssertFalse(remote.isBusy)
    }

    func testLateOldCompletionCannotMakeNewRemoteTurnIdle() {
        let bridge = InProcessSessionProtocolBridge()
        let notification = bridge.codexNotificationSink(sessionId: "codex-overlap")
        let remote = bridge.exposeAttached(
            sessionId: "codex-overlap", backend: ProtocolTestBackend(kind: .codex))

        notification("turn/started", ["turn": ["id": "turn-a"]])
        notification("turn/started", ["turn": ["id": "turn-b"]])
        notification("turn/completed", ["turn": ["id": "turn-a"]])

        XCTAssertTrue(remote.isWorking)
        XCTAssertTrue(remote.isBusy)
        XCTAssertTrue(remote.transcript?.turnActive == true)
        XCTAssertEqual(remote.transcript?.activeTurnId, "turn-b")
    }

    func testAttachBranchesTerminalSnapshotFromCodexStructuredHistory() {
        let terminal = ProtocolTestBackend(kind: .claudeCode)
        terminal.terminalSnapshot = .init(cols: 80, rows: 25, bytes: Array("screen".utf8))
        let terminalBridge = InProcessSessionProtocolBridge()
        let terminalRemote = terminalBridge.exposeAttached(sessionId: "terminal-history", backend: terminal)

        XCTAssertEqual(terminalRemote.lastCompletedSnapshotBytes, Array("screen".utf8))
        XCTAssertEqual(terminalRemote.completedSnapshotCount, 1)

        let codex = ProtocolTestBackend(kind: .codex)
        codex.codexHistory = [
            .init(id: "u1", kind: .userMessage(text: "question")),
            .init(id: "a1", kind: .agentMessage(text: "answer", phase: nil)),
            .init(id: "r1", kind: .reasoning(summary: "summary", content: "detail")),
            .init(id: "p1", kind: .plan(text: "plan")),
            .init(id: "c1", kind: .commandExecution(.init(
                command: "swift test", cwd: "/tmp/work", status: "completed",
                aggregatedOutput: "ok", exitCode: 0,
                actions: [.init(kind: .read, command: "sed -n '1p' Package.swift",
                                name: "Package.swift", path: "Package.swift", query: nil)]))),
            .init(id: "f1", kind: .fileChange(.init(status: "completed", summary: "a.swift"))),
            .init(id: "t1", kind: .toolCall(name: "crew.post", status: "completed")),
            .init(id: "w1", kind: .webSearch(query: "protocol")),
            .init(id: "x1", kind: .unknown(type: "futureItem")),
        ]
        let codexBridge = InProcessSessionProtocolBridge()
        let codexRemote = codexBridge.exposeAttached(sessionId: "codex-history", backend: codex)

        XCTAssertEqual(codexRemote.transcript?.items, codex.codexHistory)
        XCTAssertEqual(codexRemote.completedSnapshotCount, 0,
                       "Codex 没有 PTY；attach 必须走结构化历史，不造 kind=2 快照")
    }

    func testCodexAttachReplaysDaemonMemoryAfterViewerReconnectWithoutDuplicates() {
        let direct = ProtocolTestBackend(kind: .codex)
        direct.codexHistory = [
            .init(id: "before", kind: .agentMessage(text: "still in daemon", phase: nil)),
        ]
        let bridge = InProcessSessionProtocolBridge()
        let remote = bridge.exposeAttached(sessionId: "codex-reopen", backend: direct)
        XCTAssertEqual(remote.transcript?.items, direct.codexHistory)

        bridge.disconnectViewer()
        direct.codexHistory.append(
            .init(id: "offline", kind: .agentMessage(text: "while app was closed", phase: nil)))
        bridge.reconnectViewer()

        XCTAssertEqual(remote.transcript?.items, direct.codexHistory,
                       "重开 app 的 attach 必须从 daemon 内存拿全量历史，并按 item id 幂等覆盖")
    }

    func testReconnectInvalidatesOldHandleThenHelloListsAndReattaches() {
        let direct = ProtocolTestBackend(kind: .claudeCode)
        let bridge = InProcessSessionProtocolBridge()
        let remote = bridge.exposeAttached(sessionId: "reconnect", backend: direct)

        bridge.disconnectViewer()
        XCTAssertFalse(remote.isProtocolConnected)
        remote.sendRaw([1])
        XCTAssertEqual(direct.rawInputs, [], "断线后的旧 handle 必须失效")

        direct.isWorking = true
        XCTAssertFalse(remote.isWorking, "断线期间的增量不能假装已送达")

        bridge.reconnectViewer()
        XCTAssertTrue(remote.isProtocolConnected)
        XCTAssertTrue(remote.isWorking, "重连后的 listSessions 必须全量覆盖")
        remote.sendRaw([2])
        XCTAssertEqual(direct.rawInputs, [[2]], "重新 attach 分配的新 handle 可继续输入")
        XCTAssertEqual(direct.resizes, [
            .init(cols: 80, rows: 25),
            .init(cols: 80, rows: 25),
        ])
    }

    /// **viewer 侧的后端比它那条链路活得久 —— 这是设计，不是意外。**
    ///
    /// 链路断了之后 `CrewSessionRunner.viewerLinkClosed()` 明写着「镜像 run 全部标成
    /// 连不上，但**不删** —— 删了右栏会闪一下变空」。所以 `RemoteSessionBackend`
    /// （连同它那个还挂在 SwiftUI 树上的 `TerminalMirrorView`）必然会遇上
    /// 「client 已经没了、我还在」这一拍。
    ///
    /// 而 client 有好几条**自发**关闭路径不经过 `transportDisconnected()`：心跳判定
    /// 对端没回应、`ViewerSessionClient.stop()`、`closeLink()` 走的都是
    /// `link.close()`，那条按设计不通知上层（`UnixSocketTransport` 把 `close()` 和
    /// `peerDisconnected()` 分成两条正是为此）。于是 `handle` 还在、能力表还在，
    /// 而 client 已经被置 nil 析构。
    ///
    /// 2026-09-09 10:42 的闪退（人类 Todo #138，0.1.28）走的就是这条：
    ///   `AgentTerminalView.makeNSView` → `TerminalView.font.setter` → `resetFont()`
    ///   → `sizeChanged(source:)` → `TerminalMirrorView.sizeChanged` → `onResize`
    ///   → `resizeTerminal` → `client.resize` → `swift_unownedRetainStrong`
    ///   → `swift_abortRetainUnowned` → SIGABRT
    /// （这条链是拿 0.1.28 那个 commit 重建出的 dSYM 逐帧符号化来的。）
    ///
    /// ⚠️ `client` 是 `unowned` 时这条测试**不是红，是把整个测试进程 abort 掉** ——
    /// 这正是它要钉的东西：SwiftUI 布局这一拍碰到死引用没有「失败」这个中间态。
    func testBackendOutlivingItsClientStaysInertInsteadOfAborting() async {
        let direct = ProtocolTestBackend(kind: .claudeCode)
        var bridge: InProcessSessionProtocolBridge? = InProcessSessionProtocolBridge()
        let remote = bridge!.exposeAttached(sessionId: "outlives-client", backend: direct)
        weak var released = bridge

        // 自发关闭那条形状：谁都没喊 `transportDisconnected()`，handle 还在。
        bridge = nil
        XCTAssertNil(released, "client 必须真的析构了，否则这条测试是空跑的")

        // 崩溃现场那一拍：布局驱动的 resize。
        remote.resizeTerminal(cols: 132, rows: 43)
        XCTAssertEqual(remote.requestedTerminalSize, .init(cols: 132, rows: 43),
                       "视口尺寸照旧记在本地，只是发不出去")

        // 同一颗雷的其它引信 —— 这几条连 `handle` 那道 guard 都没有。
        remote.stop()
        remote.clearQuotaHealth()
        remote.sendRaw([0x03])
        _ = await remote.submitWake("wake")
        _ = await remote.applyProfileSwitch(.init(knob: .model, value: "gpt-5"))
        _ = remote.screenText(maxLines: 10)

        XCTAssertEqual(direct.stopCount, 0, "链路没了就送不到对端，但也不许崩")
        XCTAssertEqual(direct.rawInputs, [])
    }

    // MARK: - 断链必须是一个会被观察到的事件（Todo #138 ①②）

    /// **本端主动关闭时，状态必须跟着断。**
    ///
    /// `SessionMessageLink.close()` 的文档里写着「不触发 `onClose`」，理由是正当的：
    /// 谁主动关的谁自己知道，重连策略不该被自己的 detach 触发。**问题是 `onClose`
    /// 后面挂着两件事**：`transportDisconnected()`（状态跟着断）和 `onLinkClosed?()`
    /// （重连策略）。自发关闭只想退订后者，结果把前者一起退掉了。
    ///
    /// 于是 `ViewerSessionClient` 那三条自发关闭路径（心跳判定对端没回应 /
    /// `stop()` / `closeLink()`）之后，后端的连接句柄和能力表全都还挂着**最后一次
    /// 成功的值**。三态里最危险的正是这一种：陈旧的成功值跟真的成功长得一模一样。
    /// 改 weak 之后它不再崩，但开始骗人 —— 「读屏」「投唤醒」这类先看能力表的调用
    /// 会以为通道还在。
    func testSelfInitiatedCloseTearsDownEveryBackendState() {
        let direct = ProtocolTestBackend(kind: .claudeCode)
        let bridge = InProcessSessionProtocolBridge()
        let remote = bridge.exposeAttached(sessionId: "self-close", backend: direct)

        XCTAssertTrue(remote.isProtocolConnected)
        XCTAssertTrue(remote.supportsCapability("screen-text"), "先确认关之前它确实是通的")

        bridge.closeViewerLink()

        XCTAssertFalse(remote.isProtocolConnected, "本端关的也是断，状态必须跟着断")
        XCTAssertEqual(remote.negotiatedCapabilities, [],
                       "能力表必须表达「已经没了」，不许留最后一次成功的值")
        XCTAssertFalse(remote.supportsCapability("screen-text"))

        remote.sendRaw([0x41])
        remote.resizeTerminal(cols: 132, rows: 43)
        XCTAssertEqual(direct.rawInputs, [], "句柄清了就不该还发得出去")
        XCTAssertEqual(direct.resizes, [.init(cols: 80, rows: 25)],
                       "只该留下 attach 那一次")
    }

    /// **在途的切档位调用，断链时必须以失败恢复。**
    ///
    /// `applyProfileSwitch` 把 continuation 存进 `pendingProfile` 就等回应，而
    /// `transportDisconnected()` 从来不碰这张表 —— 链路一断，那次调用**永远挂着**。
    /// 投唤醒那条有 5 秒兜底会返回，切档位和切审批模式这两条一个都没有。
    ///
    /// 测试自己不许挂死：`wait(for:timeout:)` 兜住，所以修之前它是「3 秒后红」，
    /// 不是把整个套件拖住。
    func testProfileSwitchInFlightWhenLinkDiesFailsInsteadOfHangingForever() {
        let link = SilentSessionLink()
        let client = SessionProtocolClient(link: link, capabilities: [])
        _ = client.attach(sessionId: "in-flight-profile", kind: .claudeCode)

        let returned = expectation(description: "断链后 applyProfileSwitch 必须返回")
        var outcome: SessionProfileSwitchOutcome?
        Task { @MainActor in
            outcome = await client.applyProfileSwitch(
                sessionId: "in-flight-profile", command: .init(knob: .model, value: "gpt-5"))
            returned.fulfill()
        }
        // 请求真的发出去了 = continuation 已经登记（登记在 send 之前）。
        // 不靠「yield 一下大概够了」，靠这条可观测的先后。
        waitUntil(link.sentControlOps.contains("applyProfileSwitch"),
                  "切档位请求没发出去，后面的断链就不是在途状态了")

        client.close()

        wait(for: [returned], timeout: 3)
        XCTAssertEqual(outcome, .linkDown("后台链路断了，切没切成不确定"))
    }

    /// 同上，审批模式那条走的是 `pendingControls`，抛错而不是返回值。
    func testApprovalsReviewerInFlightWhenLinkDiesThrowsInsteadOfHangingForever() {
        let link = SilentSessionLink()
        let client = SessionProtocolClient(link: link, capabilities: [])
        _ = client.attach(sessionId: "in-flight-approval", kind: .codex)

        let returned = expectation(description: "断链后 updateApprovalsReviewer 必须返回")
        var thrown: Error?
        Task { @MainActor in
            do {
                try await client.updateApprovalsReviewer(
                    sessionId: "in-flight-approval", reviewer: .user)
            } catch {
                thrown = error
            }
            returned.fulfill()
        }
        waitUntil(link.sentControlOps.contains("updateApprovalsReviewer"),
                  "审批模式请求没发出去，后面的断链就不是在途状态了")

        client.close()

        wait(for: [returned], timeout: 3)
        XCTAssertNotNil(thrown, "不许静默丢 —— 断链要以明确的失败恢复")
    }

    /// 投唤醒那条**本来就有** 5 秒兜底，所以它不像上面两条那样永远挂着。
    /// 但断链是当场就知道的事，没有理由让调用方再干等 5 秒 —— 这条测试用 3 秒的
    /// 上限把「断链即返回」和「靠 5 秒兜底」区分开：只靠兜底的话它会红。
    func testWakeInFlightWhenLinkDiesReturnsAtOnceInsteadOfWaitingOutTheFallback() {
        let link = SilentSessionLink()
        let client = SessionProtocolClient(link: link, capabilities: [])
        _ = client.attach(sessionId: "in-flight-wake", kind: .claudeCode)

        let returned = expectation(description: "断链后 submitWake 必须马上返回")
        var submission: SessionWakeSubmission?
        Task { @MainActor in
            submission = await client.submitWake(sessionId: "in-flight-wake", text: "醒醒")
            returned.fulfill()
        }
        waitUntil(link.sentControlOps.contains("submitWake"),
                  "投唤醒请求没发出去，后面的断链就不是在途状态了")

        client.close()

        wait(for: [returned], timeout: 3)
        XCTAssertEqual(submission, .retry)
    }

    /// 在主线程上把 runloop 转起来直到条件成立，最多 `limit` 秒。
    /// （被测的东西全是 `@MainActor`，这里不能整块阻塞住主线程。）
    private func waitUntil(_ condition: @autoclosure () -> Bool,
                           _ message: String,
                           limit: TimeInterval = 3,
                           file: StaticString = #filePath, line: UInt = #line) {
        let deadline = Date().addingTimeInterval(limit)
        while !condition(), Date() < deadline {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(condition(), message, file: file, line: line)
    }
}

/// 一条**只收不回**的链路：请求发得出去，回应永远不来。
/// 用来把「在途」这个状态钉住 —— 同进程桥是同步的，请求当场就有回应，造不出在途。
@MainActor
final class SilentSessionLink: SessionMessageLink {
    var onReceive: ((Data) -> Void)?
    var onClose: (() -> Void)?
    private(set) var isOpen = true
    let isSynchronous = false
    let pendingWriteBytes = 0
    /// 发出去过的 control 帧的 op 名（测试据此判断请求真的上了链路）。
    private(set) var sentControlOps: [String] = []

    /// 帧 = 长度前缀 + JSON，整段当 UTF-8 解会是 nil，所以直接在字节里找。
    func send(_ framed: Data) {
        for op in ["applyProfileSwitch", "updateApprovalsReviewer", "submitWake"]
        where framed.range(of: Data(op.utf8)) != nil {
            sentControlOps.append(op)
        }
    }

    /// 本端主动关闭**不触发 `onClose`** —— 与 `SessionMessageLink` 的约定一致，
    /// 也正是这一组测试要覆盖的那条路。
    func close() { isOpen = false }
}

/// 协议两端共用的后端替身。`SessionProtocolOverSocketTests` 也用它 ——
/// 所以它是 internal，不是 private。
@MainActor
final class ProtocolTestBackend: SessionBackend, SessionProtocolTerminalControlling,
    SessionProtocolScreenTextProviding, SessionProtocolApprovalControlling,
    SessionProtocolLaunchProblemProviding, SessionProtocolTerminalSnapshotProviding,
    SessionProtocolCodexHistoryProviding {
    let kind: LocalCodingAgentKind
    @Published var status: SessionStatus = .running
    var statusPublisher: Published<SessionStatus>.Publisher { $status }
    var isBusy = false
    @Published var isWorking = false
    var isWorkingPublisher: Published<Bool>.Publisher { $isWorking }
    /// `SessionBackend` 的必答项：替身自己说了算，测试要什么就摆什么。
    var hasObservedLaunchSignal = false
    @Published var health: CrewSessionHealth?
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { $health }

    var sent: [String] = []
    var rawInputs: [[UInt8]] = []
    var resizes: [TerminalSize] = []
    var interruptCount = 0
    var stopCount = 0
    var clearQuotaCount = 0
    var profileCommands: [SessionProfileSwitchCommand] = []
    var profileOutcome: SessionProfileSwitchOutcome = .unsupported
    @Published var launchParameterProblem: SessionLaunchParameterProblem?
    var protocolLaunchParameterProblems: AnyPublisher<SessionLaunchParameterProblem, Never> {
        $launchParameterProblem.compactMap { $0 }.eraseToAnyPublisher()
    }
    var inspectionText = ""
    var requestedScreenTextLineLimits: [Int] = []
    var approvalsReviewers: [CodexProtocol.ApprovalsReviewer] = []
    var terminalSnapshot: TerminalSnapshotEncoder.Snapshot?
    var codexHistory: [CodexThreadItem] = []
    var wakeResults: [SessionWakeSubmission] = []
    var submittedWakes: [String] = []

    init(kind: LocalCodingAgentKind) { self.kind = kind }

    func send(_ text: String) { sent.append(text) }
    func submitWake(_ text: String) async -> SessionWakeSubmission {
        submittedWakes.append(text)
        return wakeResults.isEmpty ? .accepted : wakeResults.removeFirst()
    }
    func interrupt() { interruptCount += 1 }
    func stop() { stopCount += 1 }
    func clearQuotaHealth() { clearQuotaCount += 1 }
    func applyProfileSwitch(_ cmd: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        profileCommands.append(cmd)
        return profileOutcome
    }
    func sendRaw(_ bytes: [UInt8]) { rawInputs.append(bytes) }
    func resizeTerminal(cols: Int, rows: Int) { resizes.append(.init(cols: cols, rows: rows)) }
    func screenText(maxLines: Int) -> String {
        requestedScreenTextLineLimits.append(maxLines)
        return inspectionText
    }
    func updateProtocolApprovalsReviewer(_ reviewer: CodexProtocol.ApprovalsReviewer) async throws {
        approvalsReviewers.append(reviewer)
    }
    func protocolTerminalSnapshot() -> TerminalSnapshotEncoder.Snapshot? { terminalSnapshot }
    var protocolCodexHistory: [CodexThreadItem] { codexHistory }
}
#endif
