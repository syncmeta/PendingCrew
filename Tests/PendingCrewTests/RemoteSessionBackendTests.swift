#if os(macOS)
import Combine
import XCTest

@MainActor
final class RemoteSessionBackendTests: XCTestCase {
    func testSessionBackendControlsAndStateCrossTheFramedTransport() async {
        let direct = ProtocolTestBackend(kind: .codex)
        direct.profileOutcome = .applied("Set model to gpt-5")
        let bridge = InProcessSessionProtocolBridge()
        let remote = bridge.expose(sessionId: "session-1", backend: direct)

        XCTAssertEqual(remote.status, .running)
        XCTAssertEqual(remote.kind, .codex)
        XCTAssertTrue(remote.isProtocolConnected)

        remote.send("hello")
        remote.interrupt()
        remote.clearQuotaHealth()
        XCTAssertEqual(direct.sent, ["hello"])
        XCTAssertEqual(direct.interruptCount, 1)
        XCTAssertEqual(direct.clearQuotaCount, 1)

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
        let remote = bridge.expose(sessionId: "session-terminal", backend: direct)

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
        let remote = bridge.expose(sessionId: "plain-terminal", backend: direct)

        remote.interrupt()

        XCTAssertEqual(direct.rawInputs, [[0x03]])
    }

    func testMissingCapabilityDegradesWithoutRejectingConnection() {
        let direct = ProtocolTestBackend(kind: .codex)
        let bridge = InProcessSessionProtocolBridge(
            appCapabilities: ["transcript-events", "approval-mode"],
            daemonCapabilities: ["transcript-events"])
        let remote = bridge.expose(sessionId: "session-old-daemon", backend: direct)

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
        let degraded = oldDaemonBridge.expose(
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
        let remote = bridge.expose(sessionId: "remote-screen", backend: direct)
        XCTAssertEqual(
            SessionAuthoritativeScreenText.read(from: remote, maxLines: 20),
            "resume rejection")
        XCTAssertEqual(direct.requestedScreenTextLineLimits, [40, 20])
    }

    func testOutputProducedBeforeRegistrationAndAttachIsFlushedThroughProtocol() {
        let bridge = InProcessSessionProtocolBridge()
        let output = bridge.terminalOutputSink(sessionId: "early-terminal")
        output([0x65, 0x61, 0x72, 0x6c, 0x79])

        let remote = bridge.expose(
            sessionId: "early-terminal", backend: ProtocolTestBackend(kind: .claudeCode))

        XCTAssertEqual(remote.lastTerminalFrameBytes, Array("early".utf8))
    }

    func testCodexNotificationsBeforeAndAfterAttachReachRemoteTranscriptAsEvents() {
        let bridge = InProcessSessionProtocolBridge()
        let notification = bridge.codexNotificationSink(sessionId: "codex-events")
        notification("item/completed", [
            "item": ["id": "before", "type": "agentMessage", "text": "one"],
        ])
        let remote = bridge.expose(
            sessionId: "codex-events", backend: ProtocolTestBackend(kind: .codex))
        notification("item/completed", [
            "item": ["id": "after", "type": "agentMessage", "text": "two"],
        ])

        XCTAssertEqual(remote.transcript?.items.count, 2)
    }

    func testCodexTurnEventImmediatelyCorrectsRemoteWorkingState() {
        let bridge = InProcessSessionProtocolBridge()
        let notification = bridge.codexNotificationSink(sessionId: "codex-turn-state")
        let remote = bridge.expose(
            sessionId: "codex-turn-state", backend: ProtocolTestBackend(kind: .codex))

        XCTAssertFalse(remote.isWorking)
        notification("turn/started", ["turn": ["id": "turn-1"]])
        XCTAssertTrue(remote.isWorking)
        XCTAssertTrue(remote.isBusy)
        notification("turn/completed", ["turn": ["id": "turn-1"]])
        XCTAssertFalse(remote.isWorking)
        XCTAssertFalse(remote.isBusy)
    }

    func testAttachBranchesTerminalSnapshotFromCodexStructuredHistory() {
        let terminal = ProtocolTestBackend(kind: .claudeCode)
        terminal.terminalSnapshot = .init(cols: 80, rows: 25, bytes: Array("screen".utf8))
        let terminalBridge = InProcessSessionProtocolBridge()
        let terminalRemote = terminalBridge.expose(sessionId: "terminal-history", backend: terminal)

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
                aggregatedOutput: "ok", exitCode: 0))),
            .init(id: "f1", kind: .fileChange(.init(status: "completed", summary: "a.swift"))),
            .init(id: "t1", kind: .toolCall(name: "crew.post", status: "completed")),
            .init(id: "w1", kind: .webSearch(query: "protocol")),
            .init(id: "x1", kind: .unknown(type: "futureItem")),
        ]
        let codexBridge = InProcessSessionProtocolBridge()
        let codexRemote = codexBridge.expose(sessionId: "codex-history", backend: codex)

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
        let remote = bridge.expose(sessionId: "codex-reopen", backend: direct)
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
        let remote = bridge.expose(sessionId: "reconnect", backend: direct)

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
}

@MainActor
private final class ProtocolTestBackend: SessionBackend, SessionProtocolTerminalControlling,
    SessionProtocolScreenTextProviding, SessionProtocolApprovalControlling,
    SessionProtocolLaunchProblemProviding, SessionProtocolTerminalSnapshotProviding,
    SessionProtocolCodexHistoryProviding {
    let kind: LocalCodingAgentKind
    @Published var status: SessionStatus = .running
    var statusPublisher: Published<SessionStatus>.Publisher { $status }
    var isBusy = false
    @Published var isWorking = false
    var isWorkingPublisher: Published<Bool>.Publisher { $isWorking }
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

    init(kind: LocalCodingAgentKind) { self.kind = kind }

    func send(_ text: String) { sent.append(text) }
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
