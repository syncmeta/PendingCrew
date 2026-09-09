import XCTest
// McpPermissionHook.swift + LocalApprovalStore.swift 编进 test bundle（见 project.yml）。

final class McpPermissionHookTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("permhook-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    /// ⚠️ 每个 store 都显式给 `dir`。漏一个就静默落到**人的真实数据目录**
    /// （2026-09-08/09 连栽两次，见 `McpServerTestDirectoryContractTests`）。
    private func hook(_ dir: URL, gates: [String], sessionId: String = "local-x") -> McpPermissionHook {
        McpPermissionHook(approvals: LocalApprovalStore(directory: dir), crewId: "c",
                          sessionId: sessionId, gates: gates,
                          board: LocalWhiteboardStore(directory: dir),
                          todos: LocalTodoStore(directory: dir, ledger: .human),
                          grants: PermissionGrantStore(directory: dir))
    }

    func testNonGatedToolPassesThrough() {
        let out = hook(tempDir(), gates: ["mcp__computer-use"])
            .handle(#"{"tool_name":"Read","tool_input":{"file_path":"/x"},"session_id":"s"}"#)
        XCTAssertNil(out, "未 gate 的工具应放行（nil）")
    }

    // MARK: - #75 ②：口径整个换了，不是这几条测试写错了
    //
    // **旧契约**：gate 命中 → raise 一条 `kind: "permission"` 待审批 → 阻塞 long-poll
    // 最多一小时 → 到点保守判 deny。`awaitDecision` 就是那条 long-poll。
    //
    // **新契约**（人类拍板）：gate 命中 → **当场 deny** + 提一条人类 Todo 引导人放行；
    // agent 去干别的；人同意留下一张一次性票，重跑时见票放行。`awaitDecision` 随之删除。

    /// gate 命中：**立刻拒**，并把请求提进人类 Todo（不再进待审批列表）。
    func testGatedToolDeniesImmediatelyAndFilesAHumanTodo() {
        let dir = tempDir()
        let out = hook(dir, gates: ["computer-use"]).handle(
            #"{"tool_name":"mcp__computer-use__left_click","tool_input":{"command":"click 10 20"},"session_id":"s"}"#)!
        XCTAssertTrue(out.contains("\"permissionDecision\":\"deny\""), "该当场拒")
        XCTAssertTrue(out.contains("\"hookEventName\":\"PreToolUse\""), "输出带 PreToolUse 事件名")

        XCTAssertTrue(LocalApprovalStore(directory: dir).pending(crewId: "c").isEmpty,
                      "还往待审批列表 raise —— 权限类该走 Todo，不是两套并存")
        let todos = LocalTodoStore(directory: dir, ledger: .human).list(crewId: "c")
        XCTAssertEqual(todos.count, 1, "请求没进人类 Todo，那人根本不知道它要放行什么")
        XCTAssertEqual(todos.first?.permissionTool, "mcp__computer-use__left_click",
                       "没记下是哪个工具 —— 去重和放行票都靠它")
        XCTAssertEqual(todos.first?.createdBySessionId, "local-x",
                       "归档在本地 sessionId 而非 stdin 里 claude 的 session_id")
        XCTAssertTrue(todos.first?.text.contains("click 10 20") ?? false, "没说清要跑什么")
    }

    /// **承重点**：同一个工具已经挂着一条请求时，重试**不许**再提一条。
    func testRetryWhilePendingDoesNotFileASecondTodo() {
        let dir = tempDir()
        let stdin = #"{"tool_name":"mcp__computer-use__left_click","tool_input":{},"session_id":"s"}"#
        _ = hook(dir, gates: ["computer-use"]).handle(stdin)
        _ = hook(dir, gates: ["computer-use"]).handle(stdin)
        _ = hook(dir, gates: ["computer-use"]).handle(stdin)
        XCTAssertEqual(LocalTodoStore(directory: dir, ledger: .human).list(crewId: "c").count, 1,
                       """
                       重试三次提了三条 Todo —— agent 每重试一次就往人类账上加一条垃圾。\
                       人类刚因为「账上挂着一堆」质问过；做成这样等于亲手造一台灌垃圾的机器。
                       """)
    }

    /// 有票 → 放行，**并且票用掉就没了**（一次同意 = 一次放行）。
    func testGrantAllowsOnceAndIsConsumed() {
        let dir = tempDir()
        let grants = PermissionGrantStore(directory: dir)
        grants.grant(crewId: "c", tool: "toolX")
        let stdin = #"{"tool_name":"toolX","tool_input":{},"session_id":"s"}"#
        let first = McpPermissionHook(
            approvals: LocalApprovalStore(directory: dir), crewId: "c", sessionId: "local-x",
            gates: ["toolX"], board: LocalWhiteboardStore(directory: dir),
            todos: LocalTodoStore(directory: dir, ledger: .human), grants: grants).handle(stdin)!
        XCTAssertTrue(first.contains("\"permissionDecision\":\"allow\""), "人同意过还被拒 —— 那就是死循环")

        let second = McpPermissionHook(
            approvals: LocalApprovalStore(directory: dir), crewId: "c", sessionId: "local-x",
            gates: ["toolX"], board: LocalWhiteboardStore(directory: dir),
            todos: LocalTodoStore(directory: dir, ledger: .human), grants: grants).handle(stdin)!
        XCTAssertTrue(second.contains("\"permissionDecision\":\"deny\""),
                      "一张票放行了两次 —— 一次同意不该变成常设权限")
    }

    func testMalformedStdinPassesThrough() {
        let h = hook(tempDir(), gates: ["X"])
        XCTAssertNil(h.handle("not json"))
        XCTAssertNil(h.handle(#"{"no_tool_name":true}"#))
    }

    /// 通知要 @ 到能处理的人（#491）—— 这一半没变，变的只是通知里说什么：
    /// 从「待审批：…（去审批卡 allow/deny）」变成「人类 To do +1: #N 要跑 X 需要你放行」。
    func testGatedToolPostsWhiteboardNotificationMentioningHumanAndCaptain() {
        let dir = tempDir()
        _ = hook(dir, gates: ["computer-use"]).handle(
            #"{"tool_name":"mcp__computer-use__left_click","tool_input":{"command":"click"},"session_id":"s"}"#)
        let note = LocalWhiteboardStore(directory: dir).list(crewId: "c")
            .first { $0.text.contains("mcp__computer-use__left_click") }
        XCTAssertNotNil(note, "gate 命中应往白板贴一条通知")
        XCTAssertTrue(note?.text.contains("人类 To do +1") ?? false,
                      "通知没说这条已经进了人类 Todo —— 人不知道该去哪儿处理")
        XCTAssertEqual(Set(note?.mentions?.map(\.kind) ?? []), ["human", "captain"],
                       "通知应 @human + @captain")
    }

    /// **旧契约里那条「等超时了、已代拒」的路没有了** —— 因为不再等。
    ///
    /// Todo #6 当初要治的是「无限干等、静默失踪」，办法是加个上限 + 到点说出来。
    /// #75 ② 把「等」整个去掉之后，那条超时路径连同它的话术一起消失：现在是
    /// **第一次撞到就当场拒 + 提 Todo**，不存在「等到没人管」这个状态。
    /// 这条测试留着名字翻成反面，是为了让下一个人看得见它为什么消失。
    func testThereIsNoTimeoutPathAnyMore() {
        let dir = tempDir()
        let out = hook(dir, gates: ["computer-use"]).handle(
            #"{"tool_name":"mcp__computer-use__left_click","tool_input":{},"session_id":"s"}"#)
        XCTAssertTrue(out?.contains("\"permissionDecision\":\"deny\"") ?? false, "该当场拒")
        XCTAssertNil(LocalWhiteboardStore(directory: dir).list(crewId: "c")
                        .first { $0.text.contains("没人审批") },
                     "还在说「等了多少分钟没人审批」—— 那说明等待路径没拆干净")
    }

    func testNonGatedToolPostsNoNotification() {
        let dir = tempDir()
        _ = hook(dir, gates: ["computer-use"])
            .handle(#"{"tool_name":"Read","tool_input":{},"session_id":"s"}"#)
        XCTAssertTrue(LocalWhiteboardStore(directory: dir).list(crewId: "c").isEmpty,
                      "未 gate 工具不应产生通知")
    }
}
