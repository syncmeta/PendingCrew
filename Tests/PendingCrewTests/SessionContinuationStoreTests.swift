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

    func testReadyLeaseSurvivesAdmissionDeferralUntilBackendAcceptance() {
        let dir = tempDir()
        let first = SessionContinuationStore(directory: dir)
        XCTAssertTrue(first.arm(crewId: "c", sessionId: "s", note: "保留原承诺"))
        first.finishTurn(crewId: "c", sessionId: "s", outcome: .continuing)
        guard let id = first.peekReady(sessionId: "s")?.id else { return XCTFail("missing lease") }
        XCTAssertEqual(SessionContinuationStore(directory: dir).peekReady(sessionId: "s")?.id, id,
                       "admission 暂缓或后端拒绝时仍须跨重启留约")
        XCTAssertTrue(first.acknowledgeReady(id: id))
        XCTAssertNil(SessionContinuationStore(directory: dir).peekReady(sessionId: "s"))
    }

    func testMcpToolArmsAndStopHookSealsTheSameLease() {
        let dir = tempDir()
        let board = LocalWhiteboardStore(directory: dir)
        let server = McpServer(
            store: board,
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
    func testTaskBoundCaptainPromiseTransfersDurablyAndSettlesOnlyOnAcceptance() throws {
        let dir = tempDir()
        let store = SessionContinuationStore(directory: dir)
        XCTAssertTrue(store.arm(crewId: "c", sessionId: "old", note: "验收 #1", captainPromise: true, todoNumbers: [1]))
        store.finishTurn(crewId: "c", sessionId: "old", outcome: .continuing)
        XCTAssertNil(store.readyForCaptain(crewId: "c", sessionId: "new", liveSessionIds: ["old", "new"], eligibleTodoNumbers: [1]))
        XCTAssertNil(store.readyForCaptain(crewId: "c", sessionId: "new", liveSessionIds: ["new"], eligibleTodoNumbers: []))
        let claimed = try XCTUnwrap(store.readyForCaptain(crewId: "c", sessionId: "new", liveSessionIds: ["new"], eligibleTodoNumbers: [1]))
        XCTAssertEqual(claimed.originalSessionId, "old")
        XCTAssertNil(store.peekReady(sessionId: "old", eligibleTodoNumbers: [1]))
        let restarted = SessionContinuationStore(directory: dir)
        XCTAssertEqual(restarted.peekReady(sessionId: "new", eligibleTodoNumbers: [1])?.id, claimed.id)
        XCTAssertNil(restarted.peekReady(sessionId: "new", eligibleTodoNumbers: []), "阻塞或完成后不可重放")
        XCTAssertTrue(restarted.acknowledgeReady(id: claimed.id))
        XCTAssertNil(store.peekReady(sessionId: "new", eligibleTodoNumbers: [1]))
    }

    func testLegacyOrphanRequiresExplicitTaskCheckedAdoptionAndHumanStopCancels() throws {
        let store = SessionContinuationStore(directory: tempDir())
        XCTAssertTrue(store.arm(crewId: "c", sessionId: "old", note: "旧步骤"))
        store.finishTurn(crewId: "c", sessionId: "old", outcome: .continuing)
        XCTAssertNil(store.readyForCaptain(crewId: "c", sessionId: "new", liveSessionIds: ["new"], eligibleTodoNumbers: [1]))
        let old = try XCTUnwrap(store.orphanedReady(crewId: "c", sessionId: "new", liveSessionIds: ["new"]).first)
        XCTAssertFalse(store.arm(crewId: "other", sessionId: "new", note: "新步骤", captainPromise: true, todoNumbers: [1], adoptingLeaseId: old.id))
        XCTAssertTrue(store.arm(crewId: "c", sessionId: "new", note: "按当前状态验收", captainPromise: true, todoNumbers: [1], adoptingLeaseId: old.id))
        XCTAssertNil(store.peekReady(sessionId: "old"))
        store.finishTurn(crewId: "c", sessionId: "new", outcome: .continuing)
        store.cancel(sessionId: "new")
        XCTAssertNil(store.peekReady(sessionId: "new", eligibleTodoNumbers: [1]))
    }

    func testCorruptContinuationRowsAndLockFailurePreserveDebtWithoutArming() throws {
        let dir = tempDir()
        let url = dir.appendingPathComponent("session-continuations.json")
        let damaged = Data("[{\"id\":\"old\"}]".utf8)
        try damaged.write(to: url)
        let store = SessionContinuationStore(directory: dir)
        XCTAssertFalse(store.arm(crewId: "c", sessionId: "new", note: "must not overwrite"))
        XCTAssertEqual(try Data(contentsOf: url), damaged)
        let locked = tempDir()
        try FileManager.default.createDirectory(at: locked.appendingPathComponent("session-continuations.lock"), withIntermediateDirectories: true)
        XCTAssertFalse(SessionContinuationStore(directory: locked).arm(crewId: "c", sessionId: "s", note: "must not start"))
    }

}
