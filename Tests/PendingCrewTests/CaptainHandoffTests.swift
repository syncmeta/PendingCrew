import XCTest

@MainActor
final class CaptainHandoffTests: XCTestCase {
    private final class CaptainState {
        var active: [String] = ["old"]
        var maximumActive = 1
        var events: [String] = []
        var persistedKind = "codex"

        func replace(_ ids: [String], event: String) {
            active = ids
            maximumActive = max(maximumActive, active.count)
            events.append(event)
        }
    }

    func testExistingCandidateRequiresCrewMembershipAndUsesLedgerKindNotDisplayName() throws {
        let member = LocalSessionMember(
            sessionId: "worker-1", displayName: "完全猜不出 runner 的标题",
            createdAt: "2026-08-27T00:00:00Z")
        let record = LocalAgentSessionStore.Record(
            crewId: "crew-a", sessionId: "worker-1", kind: "codex",
            agentSessionId: "thread-1", updatedAt: "2026-08-27T00:00:00Z")

        let target = try CaptainHandoffCandidate.resolve(
            crewId: "crew-a", sessionId: "worker-1", members: [member], record: record)
        XCTAssertEqual(target.kind, "codex")
        XCTAssertEqual(target.agentSessionId, "thread-1")
        XCTAssertEqual(target.displayName, member.displayName)

        XCTAssertThrowsError(try CaptainHandoffCandidate.resolve(
            crewId: "crew-b", sessionId: "worker-1", members: [member], record: record))
        XCTAssertThrowsError(try CaptainHandoffCandidate.resolve(
            crewId: "crew-a", sessionId: "outsider", members: [member], record: record))
        let terminal = LocalAgentSessionStore.Record(
            crewId: "crew-a", sessionId: "worker-1", kind: "terminal",
            agentSessionId: "pty-1", updatedAt: "2026-08-27T00:00:00Z")
        XCTAssertThrowsError(try CaptainHandoffCandidate.resolve(
            crewId: "crew-a", sessionId: "worker-1", members: [member], record: terminal))
    }

    func testSuccessfulHandoffStopsOldBeforeStartingNewAndPersistsAfterLaunch() async throws {
        let state = CaptainState()
        try await CaptainHandoffTransaction.perform(
            stopOld: { state.replace([], event: "stop-old") },
            startNew: { state.replace(["new"], event: "start-new") },
            persistNew: {
                XCTAssertEqual(state.active, ["new"])
                state.persistedKind = "claude_code"
                state.events.append("persist-new")
            },
            stopNew: { state.replace([], event: "stop-new") },
            restoreOld: { state.replace(["old"], event: "restore-old") })

        XCTAssertEqual(state.events, ["stop-old", "start-new", "persist-new"])
        XCTAssertEqual(state.active, ["new"])
        XCTAssertEqual(state.persistedKind, "claude_code")
        XCTAssertEqual(state.maximumActive, 1, "任何时刻都不应稳定存在两个 live captain")
    }

    func testLaunchFailureRollsBackToOldCaptainWithoutChangingPersistedKind() async {
        enum Expected: Error { case launchFailed }
        let state = CaptainState()
        do {
            try await CaptainHandoffTransaction.perform(
                stopOld: { state.replace([], event: "stop-old") },
                startNew: {
                    state.events.append("start-new-failed")
                    throw Expected.launchFailed
                },
                persistNew: {
                    state.persistedKind = "claude_code"
                    state.events.append("persist-new")
                },
                stopNew: { state.replace([], event: "stop-new") },
                restoreOld: { state.replace(["old"], event: "restore-old") })
            XCTFail("expected failure")
        } catch {}

        XCTAssertEqual(state.events, ["stop-old", "start-new-failed", "restore-old"])
        XCTAssertEqual(state.active, ["old"])
        XCTAssertEqual(state.persistedKind, "codex")
        XCTAssertEqual(state.maximumActive, 1)
    }

    func testPersistenceFailureStopsNewAndRestoresOldCaptain() async {
        enum Expected: Error { case persistenceFailed }
        let state = CaptainState()
        do {
            try await CaptainHandoffTransaction.perform(
                stopOld: { state.replace([], event: "stop-old") },
                startNew: { state.replace(["new"], event: "start-new") },
                persistNew: { throw Expected.persistenceFailed },
                stopNew: { state.replace([], event: "stop-new") },
                restoreOld: { state.replace(["old"], event: "restore-old") })
            XCTFail("expected failure")
        } catch {}

        XCTAssertEqual(state.events, ["stop-old", "start-new", "stop-new", "restore-old"])
        XCTAssertEqual(state.active, ["old"])
        XCTAssertEqual(state.persistedKind, "codex")
        XCTAssertEqual(state.maximumActive, 1)
    }
}
