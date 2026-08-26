#if os(macOS)
import XCTest

final class InProcessTransportTests: XCTestCase {
    func testDirectFunctionCallsPreserveFrameBytesAndOrderInBothDirections() {
        let transport = InProcessTransport()
        var daemon: [Data] = []
        var app: [Data] = []
        transport.receiveFromApp = { daemon.append($0) }
        transport.receiveFromDaemon = { app.append($0) }

        let a = Data([0, 1, 2, 255])
        let b = Data([9, 8, 7])
        transport.sendFromApp(a)
        transport.sendFromApp(b)
        transport.sendFromDaemon(b)
        transport.sendFromDaemon(a)

        XCTAssertEqual(daemon, [a, b])
        XCTAssertEqual(app, [b, a])
    }

    func testUnknownMessageDoesNotDisconnectInProcessTransport() throws {
        let transport = InProcessTransport()
        let codec = SessionProtocolCodec()
        var received: [SessionAppMessage] = []
        transport.receiveFromApp = { data in
            if let message = try? codec.decodeApp(data) { received.append(message) }
        }

        let unknownJSON = try JSONSerialization.data(withJSONObject: [
            "type": "futureMessage", "newField": true,
        ])
        transport.sendFromApp(try SessionFrameEncoder.encode(.control(unknownJSON)))
        transport.sendFromApp(try codec.encode(.ping(.init(nonce: 1))))

        XCTAssertEqual(received, [.ping(.init(nonce: 1))])
        XCTAssertTrue(transport.isConnected)
    }

    func testStateSequenceGapRequestsFullListAndFullStateOverwritesLocalState() {
        var fullListRequests = 0
        var applied: [(UInt64, String)] = []
        let reconciler = SessionStateReconciler(
            requestFullList: { fullListRequests += 1 },
            apply: { seq, state in applied.append((seq, state.kind)) })

        reconciler.receiveDelta(sessionId: "s", stateSeq: 1, state: state(kind: "codex"))
        reconciler.receiveDelta(sessionId: "s", stateSeq: 3, state: state(kind: "claude_code"))

        XCTAssertEqual(fullListRequests, 1, "跳号必须拉 listSessions，不做增量重放")
        XCTAssertEqual(applied.map(\.0), [1], "跳号 delta 本身不能先落地")

        reconciler.receiveFull(.init(sessionId: "s", stateSeq: 8,
                                     state: state(kind: "claude_code")))
        reconciler.receiveDelta(sessionId: "s", stateSeq: 9, state: state(kind: "terminal"))
        XCTAssertEqual(applied.map(\.0), [1, 8, 9])
        XCTAssertEqual(applied.map(\.1), ["codex", "claude_code", "terminal"])
    }

    func testFirstDeltaMayStartAboveOneAfterReconnectButSubsequentGapStillResyncs() {
        var requests = 0
        let reconciler = SessionStateReconciler(requestFullList: { requests += 1 }, apply: { _, _ in })
        reconciler.receiveDelta(sessionId: "s", stateSeq: 41, state: state(kind: "codex"))
        reconciler.receiveDelta(sessionId: "s", stateSeq: 43, state: state(kind: "codex"))
        XCTAssertEqual(requests, 1)
    }

    private func state(kind: String) -> SessionProtocolState {
        .init(status: .running, isWorking: false, displayIsTyping: false,
              health: nil, pendingDecision: nil, kind: kind,
              launchParameterProblem: nil, scrollState: nil)
    }
}
#endif
