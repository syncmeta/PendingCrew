import XCTest

final class CaptainDelegationPolicyStoreTests: XCTestCase {
    private func directory() -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("delegation-policy-\(UUID().uuidString)")
        try! FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    func testManualPolicySurvivesAutomaticReviewAndOnlyInjectsWhenChanged() throws {
        let dir = directory()
        let policy = CaptainDelegationPolicyStore(directory: dir)
        try policy.ensurePolicy(crewId: "crew-a")
        try policy.updatePolicy(crewId: "crew-a", text: "先让合适的其他 crew 接长期主题。")
        policy.record(crewId: "crew-a", route: .localWorker,
                      now: Date(timeIntervalSince1970: 1_000_000))
        let first = policy.prepare(crewId: "crew-a", sessionId: "captain-1",
                                   now: Date(timeIntervalSince1970: 1_000_001))
        XCTAssertNotNil(first)
        XCTAssertTrue(first!.text.contains("先让合适的其他 crew"))
        policy.commit(first!)
        XCTAssertNil(policy.prepare(crewId: "crew-a", sessionId: "captain-1",
                                    now: Date(timeIntervalSince1970: 1_000_002)))
        let nextDay = policy.prepare(crewId: "crew-a", sessionId: "captain-1",
                                     now: Date(timeIntervalSince1970: 1_090_000))
        XCTAssertNotNil(nextDay)
        XCTAssertTrue(nextDay!.text.contains("本组 worker"))
        XCTAssertEqual(try policy.readPolicy(crewId: "crew-a"), "先让合适的其他 crew 接长期主题。")
    }

    func testCaptainCanReceivePolicyWithoutUnreadWhiteboard() throws {
        let dir = directory()
        let policy = CaptainDelegationPolicyStore(directory: dir)
        try policy.ensurePolicy(crewId: "crew-a")
        let emitter = HookEmitter(store: LocalWhiteboardStore(directory: dir),
                                  crewId: "crew-a", sessionId: "captain-1",
                                  cursorDir: dir, isCaptain: true)
        let prepared = emitter.prepareContext()
        XCTAssertTrue(prepared?.context?.contains("派活策略") == true)
        emitter.commit(prepared!)
        XCTAssertNil(emitter.prepareContext())
    }

    func testWorkerNeverReceivesCaptainPolicy() throws {
        let dir = directory()
        try CaptainDelegationPolicyStore(directory: dir).ensurePolicy(crewId: "crew-a")
        let emitter = HookEmitter(store: LocalWhiteboardStore(directory: dir),
                                  crewId: "crew-a", sessionId: "worker-1",
                                  cursorDir: dir, isCaptain: false)
        XCTAssertNil(emitter.prepareContext())
    }

    func testChiefPolicyKeepsExecutionInOtherCrews() throws {
        let policy = CaptainDelegationPolicyStore(directory: directory())
        try policy.ensurePolicy(crewId: LocalCrew.chiefCrewId)
        let text = try policy.readPolicy(crewId: LocalCrew.chiefCrewId)
        XCTAssertTrue(text.contains("不在总机组运行 worker"))
        XCTAssertTrue(text.contains("已有执行 crew"))
        let context = policy.prepare(crewId: LocalCrew.chiefCrewId,
                                     sessionId: "captain-chief")?.text ?? ""
        XCTAssertTrue(context.contains("总机组只负责协调和验收"))
    }

    func testExistingWhiteboardHistoryChangesCurrentFocus() throws {
        let dir = directory()
        let whiteboard = LocalWhiteboardStore(directory: dir)
        whiteboard.appendSessionMessage(crewId: "crew-a", sessionId: "worker-1",
                                        text: "还在等接口定义", category: "question")
        let policy = CaptainDelegationPolicyStore(directory: dir)
        try policy.ensurePolicy(crewId: "crew-a")
        let context = policy.prepare(crewId: "crew-a", sessionId: "captain-1")?.text ?? ""
        XCTAssertTrue(context.contains("提问 1"))
        XCTAssertTrue(context.contains("先核对问题是否已回应"))
    }

    func testPolicyToolsAreCaptainOnlyAndEditsReachNextInjection() throws {
        let dir = directory()
        func server(_ captain: Bool) -> McpServer {
            McpServer(store: LocalWhiteboardStore(directory: dir),
                      approvals: LocalApprovalStore(directory: dir),
                      control: LocalCrewControlStore(directory: dir),
                      crewId: "crew-a", sessionId: captain ? "captain-1" : "worker-1",
                      isCaptain: captain, quotaDirectory: dir)
        }
        func call(_ server: McpServer, _ name: String, _ arguments: [String: Any]) -> String {
            let request: [String: Any] = ["jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                          "params": ["name": name, "arguments": arguments]]
            let data = try! JSONSerialization.data(withJSONObject: request)
            return server.handleLine(String(decoding: data, as: UTF8.self)) ?? ""
        }
        XCTAssertTrue(call(server(false), "update_delegation_policy", ["text": "新原则"])
            .contains("仅机长可用"))
        let captain = server(true)
        XCTAssertTrue(call(captain, "update_delegation_policy", ["text": "新原则"])
            .contains("已更新"))
        XCTAssertTrue(call(captain, "read_delegation_policy", [:]).contains("新原则"))
        let context = HookEmitter(store: LocalWhiteboardStore(directory: dir),
                                  crewId: "crew-a", sessionId: "captain-1",
                                  cursorDir: dir, isCaptain: true).prepareContext()
        XCTAssertTrue(context?.context?.contains("新原则") == true)
    }
}
