#if os(macOS)
import XCTest

/// Pure state-machine tests for `PermissionRelayLogic` — the runner-side
/// bridge between the local approvals store (PreToolUse permission hook /
/// codex approval provider) and the SessionProxyDO WebSocket:
///
///   * pending local permission item → raise `permission.request` (once)
///   * `permission.request.ack` → map server request id ↔ local approval id
///   * inbound `permission.decision` → decide the local item (unblocks hook)
///   * locally-decided item → mirror the decision to the server row (once),
///     but never mirror back a decision that CAME from the server.
final class PermissionRelayLogicTests: XCTestCase {

    private let sessionId = "sess-1"

    private func item(
        _ id: String,
        kind: String = "permission",
        sessionId: String? = nil,
        status: String = "pending",
        decision: String? = nil
    ) -> ApprovalItem {
        ApprovalItem(
            id: id, kind: kind, sessionId: sessionId ?? self.sessionId,
            summary: "请求使用 computer-use", status: status,
            reply: nil, decision: decision, createdAt: "2026-07-11T00:00:00Z")
    }

    // MARK: - raise

    func testPendingPermissionItemRaisesExactlyOnce() {
        var logic = PermissionRelayLogic(sessionId: sessionId)
        let first = logic.scan([item("a1")])
        XCTAssertEqual(first.raises.map(\.id), ["a1"])
        XCTAssertTrue(first.mirrors.isEmpty)
        // Same pending item again → already in flight, no duplicate raise.
        let second = logic.scan([item("a1")])
        XCTAssertTrue(second.raises.isEmpty)
    }

    func testIgnoresOtherSessionsAndDecisionKind() {
        var logic = PermissionRelayLogic(sessionId: sessionId)
        let out = logic.scan([
            item("other", sessionId: "sess-2"),
            item("ask", kind: "decision"),
        ])
        XCTAssertTrue(out.raises.isEmpty)
        XCTAssertTrue(out.mirrors.isEmpty)
    }

    func testAlreadyAnsweredItemNeverRaisedIsIgnored() {
        // Decided before the relay started (e.g. local card answered while
        // offline) — nothing to raise, and unmapped so nothing to mirror.
        var logic = PermissionRelayLogic(sessionId: sessionId)
        let out = logic.scan([item("old", status: "answered", decision: "allow")])
        XCTAssertTrue(out.raises.isEmpty)
        XCTAssertTrue(out.mirrors.isEmpty)
    }

    // MARK: - ack + remote decision

    func testAckMapsServerIdAndRemoteDecisionResolvesLocalItem() {
        var logic = PermissionRelayLogic(sessionId: sessionId)
        _ = logic.scan([item("a1")])
        logic.ack(clientRequestId: "a1", serverRequestId: "srv-9")

        let applied = logic.remoteDecision(serverRequestId: "srv-9", decision: "approve")
        XCTAssertEqual(applied?.localId, "a1")
        XCTAssertEqual(applied?.localDecision, "allow")
        // Replay of the same decision (e.g. queued + live duplicate) → nil.
        XCTAssertNil(logic.remoteDecision(serverRequestId: "srv-9", decision: "approve"))
    }

    func testRejectMapsToDeny() {
        var logic = PermissionRelayLogic(sessionId: sessionId)
        _ = logic.scan([item("a1")])
        logic.ack(clientRequestId: "a1", serverRequestId: "srv-9")
        XCTAssertEqual(
            logic.remoteDecision(serverRequestId: "srv-9", decision: "reject")?.localDecision,
            "deny")
    }

    func testUnknownServerIdReturnsNil() {
        var logic = PermissionRelayLogic(sessionId: sessionId)
        XCTAssertNil(logic.remoteDecision(serverRequestId: "nope", decision: "approve"))
    }

    // MARK: - local decision mirror

    func testLocalDecisionMirrorsToServerOnce() {
        var logic = PermissionRelayLogic(sessionId: sessionId)
        _ = logic.scan([item("a1")])
        logic.ack(clientRequestId: "a1", serverRequestId: "srv-9")

        // Human decided on the Mac card → local item flips to answered.
        let out = logic.scan([item("a1", status: "answered", decision: "deny")])
        XCTAssertEqual(out.mirrors, [PermissionRelayLogic.Mirror(
            serverRequestId: "srv-9", decision: "reject")])
        // Mirror is one-shot.
        let again = logic.scan([item("a1", status: "answered", decision: "deny")])
        XCTAssertTrue(again.mirrors.isEmpty)
    }

    func testAllowMirrorsAsApprove() {
        var logic = PermissionRelayLogic(sessionId: sessionId)
        _ = logic.scan([item("a1")])
        logic.ack(clientRequestId: "a1", serverRequestId: "srv-9")
        let out = logic.scan([item("a1", status: "answered", decision: "allow")])
        XCTAssertEqual(out.mirrors.first?.decision, "approve")
    }

    func testRemoteDecisionIsNotMirroredBack() {
        var logic = PermissionRelayLogic(sessionId: sessionId)
        _ = logic.scan([item("a1")])
        logic.ack(clientRequestId: "a1", serverRequestId: "srv-9")
        _ = logic.remoteDecision(serverRequestId: "srv-9", decision: "approve")
        // The store now shows the item answered — but the decision came FROM
        // the server (already persisted by the DO); echoing it back would 409.
        let out = logic.scan([item("a1", status: "answered", decision: "allow")])
        XCTAssertTrue(out.mirrors.isEmpty)
    }

    // MARK: - unacked raise retry

    func testRetryWindowReRaisesUnackedItemWithCap() {
        var logic = PermissionRelayLogic(sessionId: sessionId, maxRaiseAttempts: 2)
        XCTAssertEqual(logic.scan([item("a1")]).raises.map(\.id), ["a1"]) // attempt 1
        // No ack arrived (frame sent pre-subscribe / socket blip).
        logic.retryWindowElapsed()
        XCTAssertEqual(logic.scan([item("a1")]).raises.map(\.id), ["a1"]) // attempt 2
        logic.retryWindowElapsed()
        XCTAssertTrue(logic.scan([item("a1")]).raises.isEmpty, "attempt cap reached")
    }

    func testRetryWindowDoesNotReRaiseAckedItem() {
        var logic = PermissionRelayLogic(sessionId: sessionId)
        _ = logic.scan([item("a1")])
        logic.ack(clientRequestId: "a1", serverRequestId: "srv-9")
        logic.retryWindowElapsed()
        XCTAssertTrue(logic.scan([item("a1")]).raises.isEmpty)
    }
}
#endif
