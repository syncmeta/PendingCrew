import XCTest

/// `add_human_todo` 工具（Todo #62 ④）：**agent 请人类拍板**的新增入口。
///
/// 这是两本账里方向反过来的那本的唯一新增口。三件必须同时成立：条目落进
/// `.human` 那本、记下是谁提的、群里出一行「人类 To do +1: #N …」。
final class McpAddHumanTodoTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcp-humantodo-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func server(_ dir: URL, isCaptain: Bool = false,
                        sessionId: String = "sess-1", label: String? = "两本账") -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: sessionId, isCaptain: isCaptain,
                  sessionLabel: label, todos: LocalTodoStore(directory: dir))
    }

    private func add(_ s: McpServer, _ text: String) -> String {
        let args = String(data: try! JSONSerialization.data(withJSONObject: ["text": text]),
                          encoding: .utf8)!
        return s.handleLine("""
            {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"add_human_todo","arguments":\(args)}}
            """) ?? ""
    }

    /// 全员可见 —— worker 也要能请人拍板，不是机长专用。
    func testToolIsAvailableToWorkersNotJustCaptain() throws {
        let r = try XCTUnwrap(server(tempDir(), isCaptain: false)
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#))
        XCTAssertTrue(r.contains("add_human_todo"))
    }

    /// description 必须把「和 ask 的分界」讲明白 —— 不讲，agent 会拿它当 ask 用
    /// （或者反过来，什么都去阻塞地问）。
    func testDescriptionDrawsTheAskBoundary() throws {
        let r = try XCTUnwrap(server(tempDir())
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#))
        XCTAssertTrue(r.contains("非阻塞"), "description 得说清这个是非阻塞的")
        XCTAssertTrue(r.contains("阻塞"), "description 得点名 ask 是阻塞的")
        XCTAssertTrue(r.contains("ask"))
    }

    /// respond_todo 得点明自己是 **Agent 那本** —— 两本都叫 Todo，不点名 agent
    /// 会拿它去回人类那本。
    func testRespondTodoSaysWhichLedgerItIs() throws {
        let r = try XCTUnwrap(server(tempDir())
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#))
        XCTAssertTrue(r.contains("add_human_todo"))
        XCTAssertTrue(r.contains("Agent 那本"))
    }

    // MARK: - 落账

    func testAddLandsInHumanLedgerAndRecordsTheAsker() throws {
        let dir = tempDir()
        let r = add(server(dir, sessionId: "worker-42", label: "两本账"), "上不上这个功能？我倾向上")
        XCTAssertTrue(r.contains("已记入人类 Todo #1"), r)

        let item = try XCTUnwrap(LocalTodoStore(directory: dir, ledger: .human)
            .item(crewId: "c", number: 1))
        XCTAssertEqual(item.text, "上不上这个功能？我倾向上")
        XCTAssertEqual(item.status, "pending")
        XCTAssertEqual(item.createdBySessionId, "worker-42")
        XCTAssertEqual(item.createdBySenderName, "两本账")
        XCTAssertTrue(item.isUnanswered)
    }

    /// **不许落进 Agent 那本** —— 两本账的隔离在工具这一层也得成立。
    func testAddNeverTouchesTheAgentLedger() {
        let dir = tempDir()
        _ = add(server(dir), "要人拍板的事")
        XCTAssertTrue(LocalTodoStore(directory: dir, ledger: .agent).list(crewId: "c").isEmpty)
        XCTAssertEqual(LocalTodoStore(directory: dir, ledger: .human).list(crewId: "c").count, 1)
    }

    /// 编号从 1 自增，且与 Agent 那本各算各的。
    func testNumbersIncrementIndependentlyOfTheAgentLedger() {
        let dir = tempDir()
        _ = LocalTodoStore(directory: dir, ledger: .agent).add(crewId: "c", text: "人类派的活")
        let s = server(dir)
        XCTAssertTrue(add(s, "第一件").contains("#1"))
        XCTAssertTrue(add(s, "第二件").contains("#2"))
    }

    func testEmptyTextIsRejected() {
        let r = add(server(tempDir()), "   ")
        XCTAssertTrue(r.contains("ERROR"), r)
        XCTAssertTrue(r.contains("text 不能为空"), r)
    }

    // MARK: - 群里那行

    func testAnnouncesInGroupChatWithLedgerPrefix() throws {
        let dir = tempDir()
        _ = add(server(dir, label: "两本账"), "选 A 还是 B")
        let msgs = LocalWhiteboardStore(directory: dir).list(crewId: "c")
        XCTAssertEqual(msgs.count, 1)
        // 必须带「人类」前缀 —— 两本各自从 #1 编号，不带前缀 #1 指两件事。
        XCTAssertEqual(msgs[0].text, "人类 To do +1: #1 选 A 还是 B")
        XCTAssertEqual(msgs[0].senderName, "两本账")
        XCTAssertEqual(msgs[0].senderSessionId, "sess-1")
    }

    /// 群里那行标 `@human`：讲给人听的，**别为它叫醒 agent**；但队友照样看得见
    /// （human mention 不收窄可见范围）。
    func testAnnouncementIsMarkedForHumanButStaysVisibleToTeammates() throws {
        let dir = tempDir()
        _ = add(server(dir), "要人拍板")
        let msg = LocalWhiteboardStore(directory: dir).list(crewId: "c")[0]
        XCTAssertEqual(msg.mentions?.map(\.kind), ["human"])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(msg, to: "some-other-worker"),
                      "@human 不该让这条对队友隐身")
    }

    /// 机长提的那条署机长身份（渲染端据此点星标/用机长头像）。
    func testCaptainAskedItemIsPostedAsCaptain() throws {
        let dir = tempDir()
        _ = add(server(dir, isCaptain: true, sessionId: "cap-1", label: "机长"), "要人拍板")
        XCTAssertEqual(LocalWhiteboardStore(directory: dir).list(crewId: "c")[0].senderKind,
                       "captain")
        XCTAssertEqual(LocalTodoStore(directory: dir, ledger: .human)
            .item(crewId: "c", number: 1)?.createdBySessionId, "cap-1")
    }

    // MARK: - 没落盘就别宣布（#577）

    func testUnreadableLedgerIsFailLoudAndDoesNotAnnounce() throws {
        let dir = tempDir()
        // 让列表文件读不出来：写成一个目录，读它必失败且非空判定成立。
        let file = dir.appendingPathComponent("c.human-todos.json")
        try FileManager.default.createDirectory(at: file, withIntermediateDirectories: true)

        let r = add(server(dir), "要人拍板")
        XCTAssertTrue(r.contains("ERROR"), r)
        XCTAssertTrue(r.contains("没有记下") || r.contains("没能记进"), r)
        // 群里**不许**出现「人类 To do +1」—— 人以为记下了其实没有，正是 #577 的病。
        let posted = LocalWhiteboardStore(directory: dir).list(crewId: "c")
            .filter { $0.text.contains("人类 To do +1") }
        XCTAssertTrue(posted.isEmpty, "没落盘就宣布了：\(posted.map(\.text))")
    }
}
