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

    func testRescueScopeAllowsSelfByDefaultAndOnlyDirectChildren() throws {
        XCTAssertEqual(try CaptainHandoffAuthorization.resolveTargetCrewId(
            sourceCrewId: "parent", requestedTargetCrewId: nil,
            targetParentIds: []), "parent")
        XCTAssertEqual(try CaptainHandoffAuthorization.resolveTargetCrewId(
            sourceCrewId: "parent", requestedTargetCrewId: "child",
            targetParentIds: ["parent"]), "child")

        for (target, parents) in [
            ("ancestor", ["root"]),
            ("peer", ["root"]),
            ("grandchild", ["child"]),
            ("missing", [String]()),
        ] {
            XCTAssertThrowsError(try CaptainHandoffAuthorization.resolveTargetCrewId(
                sourceCrewId: "parent", requestedTargetCrewId: target,
                targetParentIds: parents)) { error in
                    XCTAssertEqual(error as? CaptainHandoffAuthorizationError, .notDirectChild)
                }
        }
    }

    func testDirectChildRescueRequiresTheCurrentParentCaptain() throws {
        XCTAssertNoThrow(try CaptainHandoffAuthorization.validateLiveRequester(
            sourceCrewId: "parent", targetCrewId: "child",
            requesterSessionId: "parent-captain", currentCaptainSessionId: "parent-captain"))

        for requester in [nil, "parent-worker", "former-parent-captain"] as [String?] {
            XCTAssertThrowsError(try CaptainHandoffAuthorization.validateLiveRequester(
                sourceCrewId: "parent", targetCrewId: "child",
                requesterSessionId: requester, currentCaptainSessionId: "parent-captain")) { error in
                    XCTAssertEqual(error as? CaptainHandoffAuthorizationError, .notCurrentCaptain)
                }
        }

        XCTAssertThrowsError(try CaptainHandoffAuthorization.validateLiveRequester(
            sourceCrewId: "child", targetCrewId: "child",
            requesterSessionId: "former-child-captain",
            currentCaptainSessionId: "new-child-captain")) { error in
                XCTAssertEqual(error as? CaptainHandoffAuthorizationError, .notCurrentCaptain)
            }
    }

    func testDirectChildExistingSuccessorMustBelongToThatChild() throws {
        let member = LocalSessionMember(
            sessionId: "child-worker", displayName: "接任者",
            createdAt: "2026-08-30T00:00:00Z")
        let peerRecord = LocalAgentSessionStore.Record(
            crewId: "peer", sessionId: "child-worker", kind: "codex",
            agentSessionId: "thread-1", updatedAt: "2026-08-30T00:00:00Z")

        XCTAssertThrowsError(try CaptainHandoffCandidate.resolve(
            crewId: "child", sessionId: "child-worker",
            members: [member], record: peerRecord)) { error in
                XCTAssertEqual(error as? CaptainHandoffValidationError, .notAgentSession)
            }
    }

    func testDirectChildRescueUsesTheSharedSingleCaptainTransaction() async throws {
        _ = try CaptainHandoffAuthorization.resolveTargetCrewId(
            sourceCrewId: "parent", requestedTargetCrewId: "child",
            targetParentIds: ["parent"])
        let state = CaptainState()

        try await CaptainHandoffTransaction.perform(
            stopOld: { state.replace([], event: "stop-old-child-captain") },
            startNew: { state.replace(["new-child-captain"], event: "start-new-child-captain") },
            persistNew: {
                state.persistedKind = "claude_code"
                state.events.append("persist-child-captain")
            },
            stopNew: { state.replace([], event: "stop-new-child-captain") },
            restoreOld: { state.replace(["old"], event: "restore-old-child-captain") })

        XCTAssertEqual(state.active, ["new-child-captain"])
        XCTAssertEqual(state.persistedKind, "claude_code")
        XCTAssertEqual(state.maximumActive, 1)
    }

    func testDirectChildRescueLaunchFailureRestoresOldChildCaptain() async throws {
        enum Expected: Error { case launchFailed }
        _ = try CaptainHandoffAuthorization.resolveTargetCrewId(
            sourceCrewId: "parent", requestedTargetCrewId: "child",
            targetParentIds: ["parent"])
        let state = CaptainState()

        do {
            try await CaptainHandoffTransaction.perform(
                stopOld: { state.replace([], event: "stop-old-child-captain") },
                startNew: { throw Expected.launchFailed },
                persistNew: { state.persistedKind = "claude_code" },
                stopNew: { state.replace([], event: "stop-new-child-captain") },
                restoreOld: { state.replace(["old"], event: "restore-old-child-captain") })
            XCTFail("expected failure")
        } catch {}

        XCTAssertEqual(state.active, ["old"])
        XCTAssertEqual(state.persistedKind, "codex")
        XCTAssertEqual(state.maximumActive, 1)
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

    func testRollbackDoesNotRestoreOldWhenNewCaptainCannotStop() async {
        enum Expected: Error { case persistenceFailed, stopFailed }
        let state = CaptainState()
        do {
            try await CaptainHandoffTransaction.perform(
                stopOld: { state.replace([], event: "stop-old") },
                startNew: { state.replace(["new"], event: "start-new") },
                persistNew: { throw Expected.persistenceFailed },
                stopNew: {
                    state.events.append("stop-new-failed")
                    throw Expected.stopFailed
                },
                restoreOld: { state.replace(["old", "new"], event: "restore-old") })
            XCTFail("expected rollback failure")
        } catch {
            guard case CaptainHandoffTransactionError.rollbackFailed = error else {
                return XCTFail("unexpected error: \(error)")
            }
        }

        XCTAssertEqual(state.events, ["stop-old", "start-new", "stop-new-failed"])
        XCTAssertEqual(state.active, ["new"], "新机长停不掉时绝不能再拉起旧机长")
        XCTAssertEqual(state.maximumActive, 1)
    }
}
