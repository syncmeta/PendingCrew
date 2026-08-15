import XCTest

/// `respond_todo` 工具（task #478）：机器人对人类 Todo 的追加式回应 + 状态推进。
final class McpServerTodoTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("mcp-todo-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }
    private func server(_ dir: URL, isCaptain: Bool = false, label: String? = "机长") -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: "sess-1", isCaptain: isCaptain,
                  sessionLabel: label, todos: LocalTodoStore(directory: dir))
    }
    private func call(_ s: McpServer, _ args: String) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"respond_todo","arguments":\(args)}}
        """) ?? ""
    }

    /// respond_todo 对所有 session 可见（worker 也能回应），非机长专用。
    func testToolsListHasRespondTodoForWorker() {
        let r = server(tempDir(), isCaptain: false)
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        XCTAssertTrue(r.contains("respond_todo"))
    }

    func testRespondAppendsResponseAndAdvancesStatus() {
        let dir = tempDir()
        let s = server(dir)
        _ = s.todos.add(crewId: "c", text: "修复登录")
        let r = call(s, #"{"number":1,"response":"收到，开始修","status":"in_progress"}"#)
        XCTAssertFalse(r.contains("ERROR"))
        XCTAssertTrue(r.contains("进行中"))
        let item = s.todos.item(crewId: "c", number: 1)
        XCTAssertEqual(item?.status, "in_progress")
        XCTAssertEqual(item?.responses.count, 1)
        XCTAssertEqual(item?.responses[0].text, "收到，开始修")
        XCTAssertEqual(item?.responses[0].sessionId, "sess-1")
        XCTAssertEqual(item?.responses[0].senderName, "机长")
    }

    func testRespondWithoutStatusOnlyAppends() {
        let s = server(tempDir())
        _ = s.todos.add(crewId: "c", text: "调研")
        let r = call(s, #"{"number":1,"response":"看了一圈"}"#)
        XCTAssertFalse(r.contains("ERROR"))
        XCTAssertTrue(r.contains("待办"))
        XCTAssertEqual(s.todos.item(crewId: "c", number: 1)?.status, "pending")
    }

    /// 找不到 #N → 错误里带当前列表，agent 能自纠。
    func testRespondUnknownNumberListsTodos() {
        let s = server(tempDir())
        _ = s.todos.add(crewId: "c", text: "存在的条目")
        let r = call(s, #"{"number":9,"response":"?"}"#)
        XCTAssertTrue(r.contains("ERROR"))
        XCTAssertTrue(r.contains("存在的条目"))
    }

    func testRespondRejectsBadArgs() {
        let s = server(tempDir())
        _ = s.todos.add(crewId: "c", text: "x")
        XCTAssertTrue(call(s, #"{"response":"缺编号"}"#).contains("ERROR"))
        XCTAssertTrue(call(s, #"{"number":1,"response":""}"#).contains("ERROR"))
        XCTAssertTrue(call(s, #"{"number":1,"response":"ok","status":"done"}"#).contains("ERROR"))
        // 坏参数全被拒 → 条目未被写。
        XCTAssertEqual(s.todos.item(crewId: "c", number: 1)?.responses.count, 0)
    }

    /// MCP 面上没有任何“新增 todo”的通道 —— 只有人类（app 面板）能加。
    func testToolsListHasNoAddTodo() {
        let r = server(tempDir(), isCaptain: true)
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        XCTAssertFalse(r.contains("add_todo"))
        XCTAssertFalse(r.contains("create_todo"))
    }
}
