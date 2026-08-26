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
        direct.status = .exited(23)
        XCTAssertEqual(remote.health, CrewSessionHealth(kind: .usageLimit, detail: "limit"))
        XCTAssertTrue(remote.isWorking)
        XCTAssertEqual(remote.status, .exited(23))

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
            .init(cols: 80, rows: 24), // attach carries the initial viewport
            .init(cols: 132, rows: 43),
        ])
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
    }
}

@MainActor
private final class ProtocolTestBackend: SessionBackend, SessionProtocolTerminalControlling {
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
}
#endif
