import XCTest
// McpPermissionHook.swift + LocalApprovalStore.swift 编进 test bundle（见 project.yml）。

final class McpPermissionHookTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("permhook-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    func testNonGatedToolPassesThrough() {
        let h = McpPermissionHook(approvals: LocalApprovalStore(directory: tempDir()), crewId: "c",
                                  gates: ["mcp__computer-use"])
        let out = h.handle(#"{"tool_name":"Read","tool_input":{"file_path":"/x"},"session_id":"s"}"#)
        XCTAssertNil(out, "未 gate 的工具应放行（nil）")
    }

    func testGatedToolRaisesPermissionAndDeniesOnTimeout() {
        let dir = tempDir()
        let store = LocalApprovalStore(directory: dir)
        // 传入本地 sessionId "local-x"；stdin 里 claude 的 session_id 故意给 "s"（须忽略）。
        let h = McpPermissionHook(approvals: store, crewId: "c", sessionId: "local-x", gates: ["computer-use"])
        let out = h.handle(
            #"{"tool_name":"mcp__computer-use__left_click","tool_input":{"command":"click 10 20"},"session_id":"s"}"#,
            pollInterval: 0.01, maxWaits: 1)!
        XCTAssertTrue(out.contains("\"permissionDecision\":\"deny\""), "超时保守判 deny")
        XCTAssertTrue(out.contains("\"hookEventName\":\"PreToolUse\""), "输出带 PreToolUse 事件名")
        // raise 了一条 permission 待审批
        let pend = store.pending(crewId: "c")
        XCTAssertEqual(pend.count, 1)
        XCTAssertEqual(pend[0].kind, "permission")
        XCTAssertEqual(pend[0].sessionId, "local-x", "归档在本地 sessionId 而非 stdin 的 claude session_id")
        XCTAssertTrue(pend[0].summary.contains("mcp__computer-use__left_click"), "摘要含工具名")
        XCTAssertTrue(pend[0].summary.contains("click 10 20"), "摘要含 command 入参")
    }

    func testAwaitDecisionAllow() throws {
        let dir = tempDir()
        let store = LocalApprovalStore(directory: dir)
        let h = McpPermissionHook(approvals: store, crewId: "c", gates: [])
        let id = try XCTUnwrap(store.raise(crewId: "c", kind: "permission", sessionId: "s", summary: "x"))
        store.decide(crewId: "c", id: id, decision: "allow")
        XCTAssertEqual(h.awaitDecision(reqId: id, pollInterval: 0.01), "allow")
    }

    func testAwaitDecisionDeny() throws {
        let dir = tempDir()
        let store = LocalApprovalStore(directory: dir)
        let h = McpPermissionHook(approvals: store, crewId: "c", gates: [])
        let id = try XCTUnwrap(store.raise(crewId: "c", kind: "permission", sessionId: "s", summary: "x"))
        store.decide(crewId: "c", id: id, decision: "deny")
        XCTAssertEqual(h.awaitDecision(reqId: id, pollInterval: 0.01), "deny")
    }

    func testGatedAllowEndToEnd() {
        // gate 命中 → 另起一个「人类先批准」的预置：先 raise 同 store 一条并 allow 不可行
        // （handle 内部 raise 新 id）。改为验证 awaitDecision 已是 allow 时 handle 输出 allow——
        // 用 maxWaits=nil + 预先把"下一条"answer 不可行，故此用例只验 awaitDecision 分支即可。
        // handle 的 allow 路径由 awaitDecision(allow) + hookOutput(allow) 组合，两者已分别验。
        let h = McpPermissionHook(approvals: LocalApprovalStore(directory: tempDir()), crewId: "c", gates: ["X"])
        let out = h.handle(#"{"tool_name":"toolX","tool_input":{},"session_id":"s"}"#, pollInterval: 0.01, maxWaits: 0)!
        XCTAssertTrue(out.contains("\"permissionDecision\":\"deny\""))
    }

    func testMalformedStdinPassesThrough() {
        let h = McpPermissionHook(approvals: LocalApprovalStore(directory: tempDir()), crewId: "c", gates: ["X"])
        XCTAssertNil(h.handle("not json"))
        XCTAssertNil(h.handle(#"{"no_tool_name":true}"#))
    }

    func testGatedToolPostsWhiteboardNotification() {
        let dir = tempDir()
        let board = LocalWhiteboardStore(directory: dir)
        let h = McpPermissionHook(approvals: LocalApprovalStore(directory: dir), crewId: "c",
                                  gates: ["computer-use"], board: board)
        _ = h.handle(#"{"tool_name":"mcp__computer-use__left_click","tool_input":{"command":"click"},"session_id":"s"}"#,
                     pollInterval: 0.01, maxWaits: 0)
        let msgs = board.list(crewId: "c")
        let note = msgs.first { $0.text.contains("待审批：") && $0.text.contains("mcp__computer-use__left_click") }
        XCTAssertNotNil(note, "gate 命中应往白板贴一条待审批通知")
        // #491：审批通知要 @ 到能处理的人 —— @human（只有人类能 allow/deny，且默认不进详情）
        // + @captain（兜底让机长知道有 session 卡在审批）。不 @ = 静默广播，没人会去处理。
        XCTAssertEqual(Set(note?.mentions?.map(\.kind) ?? []), ["human", "captain"],
                       "审批通知应 @human + @captain")
    }

    /// Todo #6：等到没人管**必须说出来**。此前这条路是无限干等（v1 明确不设超时），
    /// 结果是没人审就永远挂着、群里也没有第二句话 —— session 就此静默失踪。
    /// 现在到点保守判 deny 让 turn 继续，并往群里 @ 人说清「等超时了、已代拒」。
    func testTimeoutDeniesAndSaysSoInTheGroup() {
        let dir = tempDir()
        let board = LocalWhiteboardStore(directory: dir)
        let h = McpPermissionHook(approvals: LocalApprovalStore(directory: dir), crewId: "c",
                                  gates: ["computer-use"], board: board)
        let out = h.handle(#"{"tool_name":"mcp__computer-use__left_click","tool_input":{},"session_id":"s"}"#,
                           pollInterval: 0.01, maxWaits: 0)
        XCTAssertTrue(out?.contains("\"permissionDecision\":\"deny\"") ?? false,
                      "到点未决 → 安全侧 deny，别把 turn 永久挂着")
        let note = board.list(crewId: "c").first { $0.text.contains("没人审批") }
        XCTAssertNotNil(note, "超时代拒必须往群里说一句，不能静默")
        XCTAssertEqual(Set(note?.mentions?.map(\.kind) ?? []), ["human", "captain"])
    }

    func testNonGatedToolPostsNoNotification() {
        let dir = tempDir()
        let board = LocalWhiteboardStore(directory: dir)
        let h = McpPermissionHook(approvals: LocalApprovalStore(directory: dir), crewId: "c",
                                  gates: ["computer-use"], board: board)
        _ = h.handle(#"{"tool_name":"Read","tool_input":{},"session_id":"s"}"#)
        XCTAssertTrue(board.list(crewId: "c").isEmpty, "未 gate 工具不应产生通知")
    }
}
