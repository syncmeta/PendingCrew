import XCTest

final class SessionContinuationStoreTests: XCTestCase {
    private func tempDir() -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("continuation-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    func testPolicyResumesOnlyExplicitCurrentTurnPromiseWithoutTerminalOutcome() {
        XCTAssertTrue(SessionContinuationPolicy.shouldResume(.init(
            promisedThisTurn: true, outcome: .continuing,
            hasHistoricalInProgressTodo: false, alreadyConsumed: false)))

        for outcome in [SessionContinuationPolicy.Outcome.completed, .blocked, .awaitingExternal] {
            XCTAssertFalse(SessionContinuationPolicy.shouldResume(.init(
                promisedThisTurn: true, outcome: outcome,
                hasHistoricalInProgressTodo: false, alreadyConsumed: false)))
        }
        XCTAssertFalse(SessionContinuationPolicy.shouldResume(.init(
            promisedThisTurn: false, outcome: .continuing,
            hasHistoricalInProgressTodo: true, alreadyConsumed: false)),
            "仅历史 Todo/plan in_progress 不能制造续跑")
        XCTAssertFalse(SessionContinuationPolicy.shouldResume(.init(
            promisedThisTurn: true, outcome: .continuing,
            hasHistoricalInProgressTodo: false, alreadyConsumed: true)))
    }

    func testLeaseIsNotReadyUntilTurnCompletionThenFiresOnlyOnce() {
        let store = SessionContinuationStore(directory: tempDir())
        XCTAssertTrue(store.arm(crewId: "c", sessionId: "s", note: "继续修竞态"))
        XCTAssertNil(store.takeReady(sessionId: "s"), "本轮尚未结束，不能抢跑")

        store.finishTurn(crewId: "c", sessionId: "s", outcome: .continuing)
        XCTAssertEqual(store.takeReady(sessionId: "s")?.note, "继续修竞态")
        XCTAssertNil(store.takeReady(sessionId: "s"), "同一租约不得重复续跑")
    }

    func testTerminalTurnOutcomesCancelArmedLease() {
        for outcome in [SessionContinuationPolicy.Outcome.completed, .blocked, .awaitingExternal] {
            let store = SessionContinuationStore(directory: tempDir())
            XCTAssertTrue(store.arm(crewId: "c", sessionId: "s", note: "不该续"))
            store.finishTurn(crewId: "c", sessionId: "s", outcome: outcome)
            XCTAssertNil(store.takeReady(sessionId: "s"))
        }
    }

    func testReadyLeaseSurvivesStoreRecreation() {
        let dir = tempDir()
        let first = SessionContinuationStore(directory: dir)
        XCTAssertTrue(first.arm(crewId: "c", sessionId: "s", note: "重启后继续"))
        first.finishTurn(crewId: "c", sessionId: "s", outcome: .continuing)

        let afterRestart = SessionContinuationStore(directory: dir)
        XCTAssertEqual(afterRestart.takeReady(sessionId: "s")?.note, "重启后继续")
        XCTAssertNil(SessionContinuationStore(directory: dir).takeReady(sessionId: "s"))
    }

    func testMcpToolArmsAndStopHookSealsTheSameLease() {
        let dir = tempDir()
        let board = LocalWhiteboardStore(directory: dir)
        let server = McpServer(
            store: board, approvals: LocalApprovalStore(directory: dir),
            control: LocalCrewControlStore(directory: dir),
            crewId: "c", sessionId: "s", quotaDirectory: dir,
            continuations: SessionContinuationStore(directory: dir))
        let listed = server.handleLine(
            #"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#) ?? ""
        XCTAssertTrue(listed.contains("continue_work"))
        let armed = server.handleLine(
            #"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"continue_work","arguments":{"note":"跑定向测试"}}}"#) ?? ""
        XCTAssertTrue(armed.contains("一次性续跑"), armed)
        XCTAssertNil(SessionContinuationStore(directory: dir).takeReady(sessionId: "s"))

        let hook = McpTurnHook(
            board: board, crewId: "c", sessionId: "s", sessionLabel: "worker",
            isCaptain: false, markerDirectory: dir)
        _ = hook.handle(
            #"{"hook_event_name":"Stop","prompt_id":"p1","last_assistant_message":"继续。"}"#)
        XCTAssertEqual(SessionContinuationStore(directory: dir).takeReady(sessionId: "s")?.note,
                       "跑定向测试")
    }
}
